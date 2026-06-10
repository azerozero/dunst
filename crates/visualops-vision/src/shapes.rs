//! Fast classical shape detection over a captured Core Graphics image.
//!
//! This is intentionally lightweight: downsample, luminance, simple edge maps,
//! connected components, and geometry heuristics. It is a spike layer for UI
//! rectangles / charts / diagrams that OCR does not see; it is not a general
//! purpose CV library.

use std::collections::VecDeque;

use core_graphics::image::CGImage;
use visualops_core::Bbox;

use crate::{coords::vision_norm_to_screen_pt, CaptureGeometry, NormRect};

const TARGET_WIDTH: usize = 320;
const MAX_SHAPES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShapeKind {
    Rect,
    Bar,
    Circle,
    Line,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub kind: ShapeKind,
    pub bbox: Bbox,
    pub confidence: f32,
}

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

#[derive(Debug)]
struct LumaImage {
    width: usize,
    height: usize,
    data: Vec<u8>,
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
        let dst_h = ((src_h as f64 * dst_w as f64 / src_w as f64).round() as usize).max(1);
        let mut data = vec![0; dst_w * dst_h];

        for y in 0..dst_h {
            let sy = ((y as f64 + 0.5) * src_h as f64 / dst_h as f64).floor() as usize;
            for x in 0..dst_w {
                let sx = ((x as f64 + 0.5) * src_w as f64 / dst_w as f64).floor() as usize;
                data[y * dst_w + x] = sample_luma(
                    raw,
                    bytes_per_row,
                    bytes_per_pixel,
                    sx.min(src_w - 1),
                    sy.min(src_h - 1),
                );
            }
        }

        Some(Self {
            width: dst_w,
            height: dst_h,
            data,
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

#[derive(Debug, Clone, Copy)]
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

#[derive(Debug)]
struct Component {
    bbox: BoxI,
    pixels: usize,
}

fn sample_luma(raw: &[u8], bytes_per_row: usize, bytes_per_pixel: usize, x: usize, y: usize) -> u8 {
    let offset = y
        .saturating_mul(bytes_per_row)
        .saturating_add(x.saturating_mul(bytes_per_pixel));
    if offset >= raw.len() {
        return 0;
    }
    if bytes_per_pixel >= 3 && offset + 2 < raw.len() {
        // CGImage window captures are usually BGRA on macOS. Average RGB-like
        // channels so BGRA/RGBA ordering does not matter for edge geometry.
        let a = raw[offset] as u16;
        let b = raw[offset + 1] as u16;
        let c = raw[offset + 2] as u16;
        ((a + b + c) / 3) as u8
    } else {
        raw[offset]
    }
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
    let threshold = mean.mul_add(1.8, 18.0).clamp(22.0, 80.0) as u16;
    let data = mags.into_iter().map(|mag| mag >= threshold).collect();
    BoolImage {
        width: luma.width,
        height: luma.height,
        data,
    }
}

fn detect_edge_shapes(edges: &BoolImage, geometry: &CaptureGeometry, out: &mut Vec<Shape>) {
    for component in components(edges, 12) {
        if out.len() >= MAX_SHAPES {
            return;
        }
        let b = component.bbox;
        if b.w < 8 || b.h < 8 || b.area() < 80 {
            continue;
        }
        let aspect = b.w as f32 / b.h.max(1) as f32;
        if !(0.125..=8.0).contains(&aspect) {
            let confidence =
                (0.45 + (component.pixels as f32 / b.area() as f32).min(0.35)).min(0.8);
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
        let fill = component.pixels as f32 / b.area() as f32;
        if border > 0.44 && fill < 0.45 {
            let confidence = (0.35 + border * 0.75 - fill * 0.25).clamp(0.35, 0.95);
            out.push(shape(
                ShapeKind::Rect,
                b,
                edges.width,
                edges.height,
                geometry,
                confidence,
            ));
        } else if (0.75..=1.35).contains(&aspect) {
            let confidence = circle_edge_score(edges, b);
            if confidence > 0.45 {
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

fn detect_filled_shapes(luma: &LumaImage, geometry: &CaptureGeometry, out: &mut Vec<Shape>) {
    let median = median_luma(&luma.data);
    let mut mask = BoolImage {
        width: luma.width,
        height: luma.height,
        data: luma
            .data
            .iter()
            .map(|&v| (v as i16 - median as i16).unsigned_abs() > 38)
            .collect(),
    };
    remove_sparse_noise(&mut mask);

    let comps = components(&mask, 20);
    let mut bar_candidates: Vec<Component> = comps
        .into_iter()
        .filter(|c| {
            let b = c.bbox;
            b.w >= 5
                && b.h >= 14
                && b.area() >= 90
                && c.pixels as f32 / b.area() as f32 > 0.55
                && b.h as f32 / b.w.max(1) as f32 > 1.15
        })
        .collect();

    bar_candidates.sort_by_key(|c| c.bbox.y + c.bbox.h);
    for group in baseline_groups(&bar_candidates) {
        if group.len() < 2 {
            continue;
        }
        for c in group {
            if out.len() >= MAX_SHAPES {
                return;
            }
            let fill = c.pixels as f32 / c.bbox.area() as f32;
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

    for component in components(&mask, 28) {
        if out.len() >= MAX_SHAPES {
            return;
        }
        let b = component.bbox;
        let aspect = b.w as f32 / b.h.max(1) as f32;
        let fill = component.pixels as f32 / b.area() as f32;
        if b.w >= 14
            && b.h >= 14
            && (0.72..=1.38).contains(&aspect)
            && (0.55..=0.88).contains(&fill)
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
        border_hits as f32 / border_total as f32
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
        (ring_hits as f32 / ring_total as f32 * 1.8).min(0.85)
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
                .map(|first| bottom.abs_diff(first.bbox.y + first.bbox.h) <= 5)
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
