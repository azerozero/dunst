//! Fast classical shape detection over a captured Core Graphics image.
//!
//! This is intentionally lightweight: downsample, luminance, simple edge maps,
//! connected components, and geometry heuristics. It is a spike layer for UI
//! rectangles / charts / diagrams that OCR does not see; it is not a general
//! purpose CV library.

use std::collections::VecDeque;

use core_graphics::image::CGImage;
use dunst_core::Bbox;

use crate::{coords::vision_norm_to_screen_pt, CaptureGeometry, NormRect};

const TARGET_WIDTH: usize = 320;
const MAX_SHAPES: usize = 200;
/// Luma delta (0–255) from the image median above which a pixel is foreground
/// for the bar/circle detector.
const FOREGROUND_DELTA: u16 = 38;
/// Lower foreground delta used **only** for panel (card/section) detection, so
/// low-contrast fills (e.g. light gray on white) the bar/circle threshold eats
/// are recovered; the strict panel filter rejects the extra noise.
const PANEL_FOREGROUND_DELTA: u16 = 14;
/// Opponent-chroma distance from the image's median color above which a pixel is
/// panel-foreground **regardless of luma** — recovers iso-luminant cards (same
/// brightness, different hue) that no luma threshold can see.
const CHROMA_DELTA: i16 = 40;

// --- Edge-shape detector thresholds (hollow rects / circles / lines) ---------

/// Minimum connected-component size (pixels) an edge blob needs to be considered.
const EDGE_MIN_COMPONENT_PIXELS: usize = 12;
/// Minimum width/height (px) and area for an edge component to classify.
const EDGE_MIN_SIDE: usize = 8;
const EDGE_MIN_AREA: usize = 80;
/// Aspect ratios **outside** this band read as a line segment, not a box/circle.
const LINE_ASPECT_RANGE: std::ops::RangeInclusive<f32> = 0.125..=8.0;
/// A hollow `Rect` needs border coverage above this and interior fill below it.
const RECT_MIN_BORDER: f32 = 0.44;
const RECT_MAX_FILL: f32 = 0.45;
/// Near-square aspect band an edge component must fall in to be circle-tested.
const EDGE_CIRCLE_ASPECT_RANGE: std::ops::RangeInclusive<f32> = 0.75..=1.35;
/// Minimum ring score for an edge component to be accepted as a `Circle`.
const EDGE_CIRCLE_MIN_SCORE: f32 = 0.45;

// --- Filled-shape detector thresholds (bars / filled circles / panels) -------

/// Minimum connected-component size (pixels) in the filled-foreground mask. The
/// single min-20 flood-fill is reused for bars, circles, and panels.
const FILLED_MIN_COMPONENT_PIXELS: usize = 20;
/// A filled circle needs at least this many pixels (stricter than the shared 20).
const FILLED_CIRCLE_MIN_PIXELS: usize = 28;
/// Bar candidate gates: min width/height, min area, min fill ratio, and the
/// height/width ratio above which a solid blob reads as a vertical bar.
const BAR_MIN_W: usize = 5;
const BAR_MIN_H: usize = 14;
const BAR_MIN_AREA: usize = 90;
const BAR_MIN_FILL: f32 = 0.55;
const BAR_MIN_HW_RATIO: f32 = 1.15;
/// Baseline-alignment tolerance (px) grouping bars that share a chart baseline.
const BAR_BASELINE_TOLERANCE: usize = 5;
/// Filled-circle gates: min side (px), near-square aspect band, and fill band.
const FILLED_CIRCLE_MIN_SIDE: usize = 14;
const FILLED_CIRCLE_ASPECT_RANGE: std::ops::RangeInclusive<f32> = 0.72..=1.38;
const FILLED_CIRCLE_FILL_RANGE: std::ops::RangeInclusive<f32> = 0.55..=0.88;

/// Classified kind of a detected shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShapeKind {
    /// A hollow rectangle (bordered box).
    Rect,
    /// A filled vertical bar (e.g. a bar-chart column).
    Bar,
    /// A filled rectangular region — a card, section, or panel container.
    Panel,
    /// A circular shape.
    Circle,
    /// An elongated line segment.
    Line,
    /// An unclassified shape.
    Unknown,
}

/// A detected shape with its screen-point bounds and confidence.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    /// Classified kind of the shape.
    pub kind: ShapeKind,
    /// Bounding box in screen points.
    pub bbox: Bbox,
    /// Detection confidence in `[0,1]`.
    pub confidence: f32,
}

/// Detects UI rectangles, bars, circles, and lines in a captured image.
pub fn detect_shapes(image: &CGImage, geometry: &CaptureGeometry) -> Vec<Shape> {
    let Some(luma) = LumaImage::from_cg_image(image) else {
        return Vec::new();
    };
    let edges = edge_map(&luma);
    let mut shapes = Vec::new();

    detect_edge_shapes(&edges, geometry, &mut shapes);
    detect_filled_shapes(&luma, geometry, &mut shapes);
    dedupe_shapes(shapes)
}

/// `num / den` as `f32`, with `den` clamped to `>= 1` so a zero denominator
/// yields `num as f32` rather than `inf`/`NaN`. Precision loss above 2^24 is
/// irrelevant here: both operands are pixel counts / dimensions well under
/// that range.
fn ratio(num: usize, den: usize) -> f32 {
    num as f32 / den.max(1) as f32
}

/// Converts a resampling coordinate to a valid source index: caps the
/// truncating cast at `>= 0` then at `max_inclusive`. Equivalent to
/// `(value.floor() as usize).min(max_inclusive)` for the non-negative
/// `value`s this module always passes (resampling coordinates derived from
/// `usize` inputs), since a truncating `as usize` cast of a non-negative
/// float equals its floor.
fn to_index(value: f64, max_inclusive: usize) -> usize {
    (value.max(0.0) as usize).min(max_inclusive)
}

#[derive(Debug)]
struct LumaImage {
    width: usize,
    height: usize,
    data: Vec<u8>,
    /// Brightness-invariant opponent chroma per pixel, aligned with `data`: see
    /// [`opponent`]. Lets the panel detector separate iso-luminant regions (same
    /// brightness, different hue) that `data` alone cannot. Zero on grayscale
    /// sources, so luma-only behaviour is preserved there.
    rg: Vec<i16>,
    yb: Vec<i16>,
}

impl LumaImage {
    fn from_cg_image(image: &CGImage) -> Option<Self> {
        let src_w = image.width();
        let src_h = image.height();
        if src_w == 0 || src_h == 0 {
            return None;
        }

        let bits_per_pixel = image.bits_per_pixel();
        let bytes_per_pixel = (bits_per_pixel / 8).max(1);
        let bytes_per_row = image.bytes_per_row();
        let bytes = image.data();
        let raw = bytes.bytes();
        if raw.is_empty() || bytes_per_row == 0 {
            return None;
        }

        let dst_w = TARGET_WIDTH.min(src_w).max(1);
        // bounded: round() + max(1) keeps dst_h >= 1; shape differs from
        // `to_index` (floor+min), so left as a direct cast rather than routed
        // through the helper.
        let dst_h = ((src_h as f64 * dst_w as f64 / src_w as f64).round() as usize).max(1);
        let mut data = vec![0u8; dst_w * dst_h];
        let mut rg = vec![0i16; dst_w * dst_h];
        let mut yb = vec![0i16; dst_w * dst_h];

        // Source-x depends only on the column, so precompute it once (already
        // edge-clamped) instead of recomputing the same mul/floor for every row —
        // a loop-invariant hoist over the whole downsample grid.
        let sx_lut: Vec<usize> = (0..dst_w)
            .map(|x| to_index((x as f64 + 0.5) * src_w as f64 / dst_w as f64, src_w - 1))
            .collect();
        for y in 0..dst_h {
            let sy = to_index((y as f64 + 0.5) * src_h as f64 / dst_h as f64, src_h - 1);
            for (x, &sx) in sx_lut.iter().enumerate() {
                let (c0, c1, c2) = sample_channels(raw, bytes_per_row, bytes_per_pixel, sx, sy);
                let idx = y * dst_w + x;
                data[idx] = ((c0 as u16 + c1 as u16 + c2 as u16) / 3) as u8;
                let (p, q) = opponent(c0, c1, c2);
                rg[idx] = p;
                yb[idx] = q;
            }
        }

        Some(Self {
            width: dst_w,
            height: dst_h,
            data,
            rg,
            yb,
        })
    }

    fn at(&self, x: usize, y: usize) -> u8 {
        self.data[y * self.width + x]
    }
}

#[derive(Debug)]
struct BoolImage {
    width: usize,
    height: usize,
    data: Vec<bool>,
}

impl BoolImage {
    fn at(&self, x: usize, y: usize) -> bool {
        self.data[y * self.width + x]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BoxI {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl BoxI {
    fn area(self) -> usize {
        self.w * self.h
    }
}

#[derive(Debug, Clone, Copy)]
struct Component {
    bbox: BoxI,
    pixels: usize,
}

/// Samples the raw channel triple `(c0, c1, c2)` at `(x, y)`. On <3-channel
/// (grayscale) buffers all three equal the single sample, so the averaged luma is
/// unchanged and [`opponent`] chroma reads as zero.
fn sample_channels(
    raw: &[u8],
    bytes_per_row: usize,
    bytes_per_pixel: usize,
    x: usize,
    y: usize,
) -> (u8, u8, u8) {
    let offset = y
        .saturating_mul(bytes_per_row)
        .saturating_add(x.saturating_mul(bytes_per_pixel));
    if offset >= raw.len() {
        return (0, 0, 0);
    }
    if bytes_per_pixel >= 3 && offset + 2 < raw.len() {
        (raw[offset], raw[offset + 1], raw[offset + 2])
    } else {
        let v = raw[offset];
        (v, v, v)
    }
}

/// Brightness-invariant opponent chroma of a raw channel triple:
/// `(c2 - c1, (c1 + c2)/2 - c0)`. Adding a constant to all three channels leaves
/// both components unchanged, so two colors with equal luma but different hue
/// still differ here. Channel *order* (BGRA vs RGBA) only flips signs/labels; the
/// distance between two pixels' opponent values is preserved either way, so the
/// detector needs no knowledge of the pixel format.
fn opponent(c0: u8, c1: u8, c2: u8) -> (i16, i16) {
    let rg = c2 as i16 - c1 as i16;
    let yb = (c1 as i16 + c2 as i16) / 2 - c0 as i16;
    (rg, yb)
}

fn edge_map(luma: &LumaImage) -> BoolImage {
    let mut mags = vec![0u16; luma.width * luma.height];
    let mut sum = 0u64;
    let mut count = 0u64;

    if luma.width < 3 || luma.height < 3 {
        return BoolImage {
            width: luma.width,
            height: luma.height,
            data: vec![false; luma.width * luma.height],
        };
    }

    for y in 1..luma.height - 1 {
        for x in 1..luma.width - 1 {
            let gx = luma.at(x + 1, y) as i16 - luma.at(x - 1, y) as i16;
            let gy = luma.at(x, y + 1) as i16 - luma.at(x, y - 1) as i16;
            let mag = gx.unsigned_abs() + gy.unsigned_abs();
            mags[y * luma.width + x] = mag;
            sum += mag as u64;
            count += 1;
        }
    }

    let mean = if count == 0 {
        0.0
    } else {
        sum as f64 / count as f64
    };
    // bounded: clamp(22.0, 80.0) keeps the value within u16 range before the cast.
    let threshold = mean.mul_add(1.8, 18.0).clamp(22.0, 80.0) as u16;
    let data = mags.into_iter().map(|mag| mag >= threshold).collect();
    BoolImage {
        width: luma.width,
        height: luma.height,
        data,
    }
}

fn detect_edge_shapes(edges: &BoolImage, geometry: &CaptureGeometry, out: &mut Vec<Shape>) {
    for component in components(edges, EDGE_MIN_COMPONENT_PIXELS) {
        if out.len() >= MAX_SHAPES {
            return;
        }
        let b = component.bbox;
        if b.w < EDGE_MIN_SIDE || b.h < EDGE_MIN_SIDE || b.area() < EDGE_MIN_AREA {
            continue;
        }
        let aspect = ratio(b.w, b.h);
        if !LINE_ASPECT_RANGE.contains(&aspect) {
            let confidence = (0.45 + ratio(component.pixels, b.area()).min(0.35)).min(0.8);
            out.push(shape(
                ShapeKind::Line,
                b,
                edges.width,
                edges.height,
                geometry,
                confidence,
            ));
            continue;
        }

        let border = border_score(edges, b);
        let fill = ratio(component.pixels, b.area());
        if border > RECT_MIN_BORDER && fill < RECT_MAX_FILL {
            let confidence = (0.35 + border * 0.75 - fill * 0.25).clamp(0.35, 0.95);
            out.push(shape(
                ShapeKind::Rect,
                b,
                edges.width,
                edges.height,
                geometry,
                confidence,
            ));
        } else if EDGE_CIRCLE_ASPECT_RANGE.contains(&aspect) {
            let confidence = circle_edge_score(edges, b);
            if confidence > EDGE_CIRCLE_MIN_SCORE {
                out.push(shape(
                    ShapeKind::Circle,
                    b,
                    edges.width,
                    edges.height,
                    geometry,
                    confidence,
                ));
            }
        }
    }
}

/// Builds a foreground mask: pixels whose luma differs from `median` by more
/// than `delta` (0–255), with sparse single-pixel noise removed. A lower `delta`
/// recovers lower-contrast regions at the cost of more spurious pixels.
fn foreground_mask(luma: &LumaImage, median: u8, delta: u16) -> BoolImage {
    let mut mask = BoolImage {
        width: luma.width,
        height: luma.height,
        data: luma
            .data
            .iter()
            .map(|&v| (v as i16 - median as i16).unsigned_abs() > delta)
            .collect(),
    };
    remove_sparse_noise(&mut mask);
    mask
}

/// Foreground mask for panels: a pixel is foreground when it differs from the
/// image's background reference in **luma** (by more than [`PANEL_FOREGROUND_DELTA`])
/// **or** in **opponent chroma** (by more than [`CHROMA_DELTA`]). The chroma arm
/// catches iso-luminant cards — same brightness, different hue — that the luma
/// arm and the bar/circle mask are both blind to. Sparse single-pixel noise is
/// removed; the strict panel filter downstream discards the rest.
fn panel_foreground_mask(luma: &LumaImage, median: u8) -> BoolImage {
    let med_rg = median_i16(&luma.rg);
    let med_yb = median_i16(&luma.yb);
    let mut mask = BoolImage {
        width: luma.width,
        height: luma.height,
        data: (0..luma.data.len())
            .map(|i| {
                let luma_fg =
                    (luma.data[i] as i16 - median as i16).unsigned_abs() > PANEL_FOREGROUND_DELTA;
                let chroma_fg =
                    (luma.rg[i] - med_rg).abs() + (luma.yb[i] - med_yb).abs() > CHROMA_DELTA;
                luma_fg || chroma_fg
            })
            .collect(),
    };
    remove_sparse_noise(&mut mask);
    mask
}

/// Median of `data` via a 512-bin histogram over the opponent range
/// `[-256, 255]`; mirrors [`median_luma`]. Returns `0` when empty.
fn median_i16(data: &[i16]) -> i16 {
    if data.is_empty() {
        return 0;
    }
    let mut hist = [0usize; 512];
    for &v in data {
        hist[(v as i32 + 256).clamp(0, 511) as usize] += 1;
    }
    let mid = data.len() / 2;
    let mut acc = 0usize;
    for (i, count) in hist.iter().enumerate() {
        acc += count;
        if acc >= mid {
            return i as i16 - 256;
        }
    }
    0
}

fn detect_filled_shapes(luma: &LumaImage, geometry: &CaptureGeometry, out: &mut Vec<Shape>) {
    let median = median_luma(&luma.data);
    let mask = foreground_mask(luma, median, FOREGROUND_DELTA);

    // One flood-fill for the whole mask. `components(mask, k)` runs the same
    // connected-components pass regardless of `k` — `k` is only the final size
    // gate — so `components(mask, 28)` is exactly `components(mask, 20)` filtered
    // to `pixels >= 28`. Compute the min-20 set once and reuse it for bars and
    // circles instead of flooding the mask a second time (O(W*H) saved per call).
    let comps = components(&mask, FILLED_MIN_COMPONENT_PIXELS);
    // Bars claim their components first; the panel pass skips those bboxes so a
    // chart column is not re-emitted as a card.
    let bar_bboxes = detect_bars(&comps, luma, geometry, out);
    detect_circles(&comps, luma, geometry, out);
    detect_panels(luma, median, &bar_bboxes, geometry, out);
}

/// Filled vertical bars (chart columns): solid, taller-than-wide components that
/// share a baseline (grouped by [`baseline_groups`], only groups of ≥ 2 emit).
/// Returns the bboxes it claimed so [`detect_panels`] can skip them.
fn detect_bars(
    comps: &[Component],
    luma: &LumaImage,
    geometry: &CaptureGeometry,
    out: &mut Vec<Shape>,
) -> Vec<BoxI> {
    let mut bar_candidates: Vec<Component> = comps
        .iter()
        .copied()
        .filter(|c| {
            let b = c.bbox;
            b.w >= BAR_MIN_W
                && b.h >= BAR_MIN_H
                && b.area() >= BAR_MIN_AREA
                && ratio(c.pixels, b.area()) > BAR_MIN_FILL
                && ratio(b.h, b.w) > BAR_MIN_HW_RATIO
        })
        .collect();

    bar_candidates.sort_by_key(|c| c.bbox.y + c.bbox.h);
    let mut bar_bboxes: Vec<BoxI> = Vec::new();
    for group in baseline_groups(&bar_candidates) {
        if group.len() < 2 {
            continue;
        }
        for c in group {
            if out.len() >= MAX_SHAPES {
                return bar_bboxes;
            }
            let fill = ratio(c.pixels, c.bbox.area());
            bar_bboxes.push(c.bbox);
            out.push(shape(
                ShapeKind::Bar,
                c.bbox,
                luma.width,
                luma.height,
                geometry,
                (0.45 + fill * 0.35).min(0.9),
            ));
        }
    }
    bar_bboxes
}

/// Filled near-square discs read as circles — the solid blobs the hollow-ring
/// edge test misses. Reuses the shared min-20 [`Component`]s.
fn detect_circles(
    comps: &[Component],
    luma: &LumaImage,
    geometry: &CaptureGeometry,
    out: &mut Vec<Shape>,
) {
    for component in comps
        .iter()
        .filter(|c| c.pixels >= FILLED_CIRCLE_MIN_PIXELS)
    {
        if out.len() >= MAX_SHAPES {
            return;
        }
        let b = component.bbox;
        let aspect = ratio(b.w, b.h);
        let fill = ratio(component.pixels, b.area());
        if b.w >= FILLED_CIRCLE_MIN_SIDE
            && b.h >= FILLED_CIRCLE_MIN_SIDE
            && FILLED_CIRCLE_ASPECT_RANGE.contains(&aspect)
            && FILLED_CIRCLE_FILL_RANGE.contains(&fill)
        {
            let confidence = (0.35 + (1.0 - (aspect - 1.0).abs()).max(0.0) * 0.25 + fill * 0.35)
                .clamp(0.35, 0.85);
            out.push(shape(
                ShapeKind::Circle,
                b,
                luma.width,
                luma.height,
                geometry,
                confidence,
            ));
        }
    }
}

/// Filled rectangular regions = cards / sections / panels: the solid containers
/// modern UIs draw *without* a visible border, which neither the hollow `Rect`
/// detector (needs an outline) nor the tall-`Bar` filter emits. Surfacing them
/// lets the zone tree nest a card's text and controls under the card instead of
/// leaving three flat lists.
///
/// Panels get their OWN, more sensitive mask ([`panel_foreground_mask`]). Cards
/// are often low-contrast fills (light gray on white) whose luma sits *inside*
/// the ±[`FOREGROUND_DELTA`] band, so the bar/circle mask never sees them — the
/// exact "even in grayscale the diff exists, but the threshold eats it" gap.
/// [`PANEL_FOREGROUND_DELTA`] is lower; the strict [`filled_panel_qualifies`]
/// gate (size/fill/aspect/area) rejects the extra text/antialias noise a looser
/// threshold surfaces. Bars and circles keep the original mask, so their output
/// is byte-for-byte unchanged. `bar_bboxes` are skipped so a chart column is not
/// re-emitted as a card.
fn detect_panels(
    luma: &LumaImage,
    median: u8,
    bar_bboxes: &[BoxI],
    geometry: &CaptureGeometry,
    out: &mut Vec<Shape>,
) {
    let panel_mask = panel_foreground_mask(luma, median);
    for component in &components(&panel_mask, FILLED_MIN_COMPONENT_PIXELS) {
        if out.len() >= MAX_SHAPES {
            return;
        }
        if bar_bboxes.contains(&component.bbox) {
            continue;
        }
        if filled_panel_qualifies(*component, luma.width, luma.height) {
            let fill = ratio(component.pixels, component.bbox.area());
            out.push(shape(
                ShapeKind::Panel,
                component.bbox,
                luma.width,
                luma.height,
                geometry,
                (0.4 + fill * 0.4).min(0.85),
            ));
        }
    }
}

/// Whether a filled connected component reads as a card/section/panel
/// container: solid, sizeable, roughly rectangular, and neither a thin rule nor
/// the whole window. Dimensions are in downscaled-image pixels
/// ([`TARGET_WIDTH`]-wide), so thresholds are relative to that frame.
fn filled_panel_qualifies(c: Component, img_w: usize, img_h: usize) -> bool {
    let b = c.bbox;
    if b.w < 24 || b.h < 16 {
        return false;
    }
    let fill = ratio(c.pixels, b.area());
    if fill < 0.60 {
        return false;
    }
    // Reject thin rules and tall bars; keep card-like aspect ratios.
    let aspect = ratio(b.w, b.h);
    if !(0.08..=12.0).contains(&aspect) {
        return false;
    }
    // At least ~1% of the frame (a real card), at most ~75% (not the page
    // background or the whole window).
    let img_area = (img_w * img_h).max(1) as f32;
    let area = b.area() as f32;
    (0.01 * img_area..=0.75 * img_area).contains(&area)
}

fn components(mask: &BoolImage, min_pixels: usize) -> Vec<Component> {
    let mut seen = vec![false; mask.width * mask.height];
    let mut out = Vec::new();
    let mut queue = VecDeque::new();

    for y in 0..mask.height {
        for x in 0..mask.width {
            let idx = y * mask.width + x;
            if seen[idx] || !mask.data[idx] {
                continue;
            }
            seen[idx] = true;
            queue.push_back((x, y));
            let mut min_x = x;
            let mut max_x = x;
            let mut min_y = y;
            let mut max_y = y;
            let mut pixels = 0usize;

            while let Some((cx, cy)) = queue.pop_front() {
                pixels += 1;
                min_x = min_x.min(cx);
                max_x = max_x.max(cx);
                min_y = min_y.min(cy);
                max_y = max_y.max(cy);
                for (nx, ny) in neighbours(cx, cy, mask.width, mask.height) {
                    let nidx = ny * mask.width + nx;
                    if !seen[nidx] && mask.data[nidx] {
                        seen[nidx] = true;
                        queue.push_back((nx, ny));
                    }
                }
            }

            if pixels >= min_pixels {
                out.push(Component {
                    bbox: BoxI {
                        x: min_x,
                        y: min_y,
                        w: max_x - min_x + 1,
                        h: max_y - min_y + 1,
                    },
                    pixels,
                });
            }
        }
    }
    out
}

fn neighbours(x: usize, y: usize, w: usize, h: usize) -> impl Iterator<Item = (usize, usize)> {
    let x0 = x.saturating_sub(1);
    let y0 = y.saturating_sub(1);
    let x1 = (x + 1).min(w - 1);
    let y1 = (y + 1).min(h - 1);
    (y0..=y1).flat_map(move |ny| {
        (x0..=x1).filter_map(move |nx| {
            if nx == x && ny == y {
                None
            } else {
                Some((nx, ny))
            }
        })
    })
}

fn border_score(edges: &BoolImage, b: BoxI) -> f32 {
    let band = 2usize.min(b.w / 2).min(b.h / 2).max(1);
    let mut border_hits = 0usize;
    let mut border_total = 0usize;
    for y in b.y..b.y + b.h {
        for x in b.x..b.x + b.w {
            let near_border =
                x < b.x + band || x + band >= b.x + b.w || y < b.y + band || y + band >= b.y + b.h;
            if near_border {
                border_total += 1;
                if edges.at(x, y) {
                    border_hits += 1;
                }
            }
        }
    }
    if border_total == 0 {
        0.0
    } else {
        ratio(border_hits, border_total)
    }
}

fn circle_edge_score(edges: &BoolImage, b: BoxI) -> f32 {
    let cx = b.x as f32 + b.w as f32 / 2.0;
    let cy = b.y as f32 + b.h as f32 / 2.0;
    let rx = b.w as f32 / 2.0;
    let ry = b.h as f32 / 2.0;
    let mut ring_hits = 0usize;
    let mut ring_total = 0usize;
    for y in b.y..b.y + b.h {
        for x in b.x..b.x + b.w {
            let dx = (x as f32 + 0.5 - cx) / rx.max(1.0);
            let dy = (y as f32 + 0.5 - cy) / ry.max(1.0);
            let r2 = dx * dx + dy * dy;
            if (0.70..=1.30).contains(&r2) {
                ring_total += 1;
                if edges.at(x, y) {
                    ring_hits += 1;
                }
            }
        }
    }
    if ring_total == 0 {
        0.0
    } else {
        (ratio(ring_hits, ring_total) * 1.8).min(0.85)
    }
}

fn median_luma(data: &[u8]) -> u8 {
    let mut hist = [0usize; 256];
    for &v in data {
        hist[v as usize] += 1;
    }
    let mid = data.len() / 2;
    let mut acc = 0usize;
    for (i, count) in hist.iter().enumerate() {
        acc += count;
        if acc >= mid {
            return i as u8;
        }
    }
    128
}

fn remove_sparse_noise(mask: &mut BoolImage) {
    let src = mask.data.clone();
    for y in 1..mask.height.saturating_sub(1) {
        for x in 1..mask.width.saturating_sub(1) {
            let idx = y * mask.width + x;
            if !src[idx] {
                continue;
            }
            let mut count = 0;
            for (nx, ny) in neighbours(x, y, mask.width, mask.height) {
                if src[ny * mask.width + nx] {
                    count += 1;
                }
            }
            if count <= 1 {
                mask.data[idx] = false;
            }
        }
    }
}

fn baseline_groups(comps: &[Component]) -> Vec<Vec<&Component>> {
    let mut groups: Vec<Vec<&Component>> = Vec::new();
    for comp in comps {
        let bottom = comp.bbox.y + comp.bbox.h;
        if let Some(group) = groups.iter_mut().find(|group| {
            group
                .first()
                .map(|first| bottom.abs_diff(first.bbox.y + first.bbox.h) <= BAR_BASELINE_TOLERANCE)
                .unwrap_or(false)
        }) {
            group.push(comp);
        } else {
            groups.push(vec![comp]);
        }
    }
    groups
}

fn shape(
    kind: ShapeKind,
    b: BoxI,
    width: usize,
    height: usize,
    geometry: &CaptureGeometry,
    confidence: f32,
) -> Shape {
    let norm = NormRect {
        x: b.x as f64 / width as f64,
        y: 1.0 - (b.y + b.h) as f64 / height as f64,
        w: b.w as f64 / width as f64,
        h: b.h as f64 / height as f64,
    };
    Shape {
        kind,
        bbox: vision_norm_to_screen_pt(norm, geometry),
        confidence,
    }
}

fn dedupe_shapes(mut shapes: Vec<Shape>) -> Vec<Shape> {
    shapes.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out: Vec<Shape> = Vec::new();
    for shape in shapes {
        if out.len() >= MAX_SHAPES {
            break;
        }
        if out
            .iter()
            .any(|kept| kept.kind == shape.kind && iou(kept.bbox, shape.bbox) > 0.55)
        {
            continue;
        }
        out.push(shape);
    }
    out
}

fn iou(a: Bbox, b: Bbox) -> f64 {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    let inter = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let union = a.w * a.h + b.w * b.h - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

#[cfg(test)]
mod panel_tests {
    use super::*;

    fn comp(x: usize, y: usize, w: usize, h: usize, fill: f32) -> Component {
        Component {
            bbox: BoxI { x, y, w, h },
            // bounded: test helper only; `fill` is always in [0, 1] and `w * h`
            // is a small synthetic fixture size, so no clamp is needed.
            pixels: ((w * h) as f32 * fill) as usize,
        }
    }

    /// A synthetic luma image: uniform `bg`, with one filled `card_luma` rect.
    /// A synthetic image: uniform `bg` color with one filled `card_rgb` rect.
    /// Fills `data`/`rg`/`yb` exactly as `from_cg_image` would.
    fn image_with_card(
        w: usize,
        h: usize,
        bg: (u8, u8, u8),
        card: (usize, usize, usize, usize),
        card_rgb: (u8, u8, u8),
    ) -> LumaImage {
        fn luma((c0, c1, c2): (u8, u8, u8)) -> u8 {
            ((c0 as u16 + c1 as u16 + c2 as u16) / 3) as u8
        }
        let (cx, cy, cw, ch) = card;
        let (brg, byb) = opponent(bg.0, bg.1, bg.2);
        let (crg, cyb) = opponent(card_rgb.0, card_rgb.1, card_rgb.2);
        let mut data = vec![luma(bg); w * h];
        let mut rg = vec![brg; w * h];
        let mut yb = vec![byb; w * h];
        for y in cy..cy + ch {
            for x in cx..cx + cw {
                let i = y * w + x;
                data[i] = luma(card_rgb);
                rg[i] = crg;
                yb[i] = cyb;
            }
        }
        LumaImage {
            width: w,
            height: h,
            data,
            rg,
            yb,
        }
    }

    /// Grayscale convenience over [`image_with_card`]: neutral gray triples.
    fn luma_with_card(
        w: usize,
        h: usize,
        bg: u8,
        card: (usize, usize, usize, usize),
        card_luma: u8,
    ) -> LumaImage {
        image_with_card(w, h, (bg, bg, bg), card, (card_luma, card_luma, card_luma))
    }

    fn unit_geometry(w: usize, h: usize) -> CaptureGeometry {
        CaptureGeometry {
            window_origin_pt: (0.0, 0.0),
            window_size_pt: (w as f64, h as f64),
            image_size_px: (w as f64, h as f64),
            backing_scale: 1.0,
        }
    }

    #[test]
    fn low_contrast_card_is_detected_as_a_panel() {
        // Gray card (luma 220) on a white page (240): a 20-level diff — below the
        // bar/circle FOREGROUND_DELTA (38, so the old mask ate it) but above
        // PANEL_FOREGROUND_DELTA (14). This is the exact case the fix recovers.
        let luma = luma_with_card(320, 200, 240, (40, 40, 120, 80), 220);
        let mut out = Vec::new();
        detect_filled_shapes(&luma, &unit_geometry(320, 200), &mut out);
        assert!(
            out.iter().any(|s| s.kind == ShapeKind::Panel),
            "expected a Panel for a 20-level card, got {:?}",
            out.iter().map(|s| s.kind).collect::<Vec<_>>()
        );
    }

    #[test]
    fn ultra_low_contrast_card_is_still_missed() {
        // 10-level diff is below PANEL_FOREGROUND_DELTA (14): honestly still
        // missed. Recovering this needs color/segmentation, not a lower luma
        // threshold — lowering further would flood the mask with noise.
        let luma = luma_with_card(320, 200, 240, (40, 40, 120, 80), 230);
        let mut out = Vec::new();
        detect_filled_shapes(&luma, &unit_geometry(320, 200), &mut out);
        assert!(!out.iter().any(|s| s.kind == ShapeKind::Panel));
    }

    #[test]
    fn isoluminant_card_is_detected_via_chroma() {
        // Card (168,128,88) and background (128,128,128) share luma 128 — the
        // luma arm is completely blind — but differ in hue. The opponent-chroma
        // arm (rg/yb) separates them, so the card still nests as a Panel.
        let luma = image_with_card(320, 200, (128, 128, 128), (40, 40, 120, 80), (168, 128, 88));
        assert_eq!(
            luma.data[0],
            luma.data[40 * 320 + 40],
            "card and bg share luma"
        );
        let mut out = Vec::new();
        detect_filled_shapes(&luma, &unit_geometry(320, 200), &mut out);
        assert!(
            out.iter().any(|s| s.kind == ShapeKind::Panel),
            "expected a Panel for an iso-luminant card, got {:?}",
            out.iter().map(|s| s.kind).collect::<Vec<_>>()
        );
    }

    // Frame is the downscaled TARGET_WIDTH-wide image; use 320x240 here.
    const W: usize = 320;
    const H: usize = 240;

    #[test]
    fn wide_filled_card_qualifies() {
        // 120x80 solid card at 90% fill, ~12.5% of the frame.
        assert!(filled_panel_qualifies(comp(40, 40, 120, 80, 0.90), W, H));
    }

    #[test]
    fn elongated_header_section_qualifies() {
        // A wide header band (aspect ~9.3, within the 12.0 cap).
        assert!(filled_panel_qualifies(comp(20, 10, 280, 30, 0.80), W, H));
    }

    #[test]
    fn thin_horizontal_rule_rejected() {
        // h < 16 (a divider line), not a container.
        assert!(!filled_panel_qualifies(comp(20, 100, 200, 3, 0.95), W, H));
    }

    #[test]
    fn tall_narrow_bar_rejected() {
        // w < 24 (a chart column), not a container.
        assert!(!filled_panel_qualifies(comp(50, 40, 6, 90, 0.95), W, H));
    }

    #[test]
    fn tiny_blob_rejected_by_area_floor() {
        // Just meets min dims but falls under the ~1%-of-frame area floor.
        assert!(!filled_panel_qualifies(comp(10, 10, 24, 16, 0.95), W, H));
    }

    #[test]
    fn whole_window_rejected() {
        // The full frame is the background, not a card (> 75% area cap).
        assert!(!filled_panel_qualifies(comp(0, 0, W, H, 1.0), W, H));
    }

    #[test]
    fn sparse_component_rejected() {
        // Right size, but only 30% filled — an outline/scatter, not a solid card.
        assert!(!filled_panel_qualifies(comp(40, 40, 120, 80, 0.30), W, H));
    }
}
