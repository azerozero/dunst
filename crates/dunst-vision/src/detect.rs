//! Coarse-to-fine detection of whether a chart is actually **rendered** (vs a
//! blank/white plot) and where its content sits — a fast presence gate before
//! the expensive hover + OCR traversal.
//!
//! "Progressive de-pixelisation" (the operator's image): a very coarse luminance
//! grid answers present/absent in a flash, and only if there is content does a
//! finer grid run to locate the content's bounding box. Cheap reject, precise
//! accept.

use core_graphics::image::CGImage;

use crate::CaptureGeometry;
use dunst_core::Bbox;

/// Outcome of [`detect_chart_region`].
#[derive(Debug, Clone)]
pub struct ChartDetection {
    /// Rendered content found in the central plot band (vs a blank/white plot).
    pub present: bool,
    /// Bounding box of that content, in global screen **points** (`None` if absent).
    pub region: Option<Bbox>,
    /// Content density of the plot band, `0..1` — near 0 ≈ blank, higher ≈ a curve.
    pub fill_ratio: f32,
}

/// Luma at/above this is background; the curve, its (pale) area fill, and text
/// fall below it. Set high enough to include a pale gradient fill.
const CONTENT_LUMA_MAX: u8 = 236;
/// Coarse grid width — fast blank/present gate.
const COARSE_W: usize = 80;
/// Fine grid width — locate the content bbox.
const FINE_W: usize = 256;
/// Minimum overall fill (coarse) to bother running the fine pass.
const COARSE_MIN_FILL: f32 = 0.01;
/// A chart plot blob must span at least this fraction of the grid width and
/// height — wide AND tall separates a curve+fill from thin text lines.
const PLOT_MIN_W_FRAC: f32 = 0.33;
const PLOT_MIN_H_FRAC: f32 = 0.12;

/// Detect whether the target's chart is rendered and, if so, where its plotted
/// content lies (in screen points). Use a base window/display capture — the data
/// curve is base content (a window grab captures it; only the transient crosshair
/// overlay needs the composited path).
pub fn detect_chart_region(image: &CGImage, geometry: &CaptureGeometry) -> ChartDetection {
    let absent = |fill_ratio| ChartDetection {
        present: false,
        region: None,
        fill_ratio,
    };

    // L0 — very coarse: a near-empty grid rejects here without the fine pass.
    let Some((w0, h0, d0)) = luma_grid(image, COARSE_W) else {
        return absent(0.0);
    };
    if overall_fill(w0, h0, &d0) < COARSE_MIN_FILL {
        return absent(0.0);
    }

    // L1 — finer: the chart PLOT is the largest content blob that is both wide
    // AND tall (a curve + area fill), which separates it from thin text lines and
    // from a blank plot (which has no such blob).
    let (w, h, d) = luma_grid(image, FINE_W).unwrap_or((w0, h0, d0));
    let r1 = overall_fill(w, h, &d);
    let min_w = (w as f32 * PLOT_MIN_W_FRAC) as usize;
    let min_h = (h as f32 * PLOT_MIN_H_FRAC) as usize;
    let Some(plot) = largest_blob(w, h, &d).filter(|b| {
        (b.maxx - b.minx + 1) >= min_w && (b.maxy - b.miny + 1) >= min_h
    }) else {
        return absent(r1);
    };

    let (ox, oy) = geometry.window_origin_pt;
    let (sw, sh) = geometry.window_size_pt;
    let sx = |gx: usize| ox + (gx as f64 / w as f64) * sw;
    let sy = |gy: usize| oy + (gy as f64 / h as f64) * sh;
    ChartDetection {
        present: true,
        region: Some(Bbox {
            x: sx(plot.minx),
            y: sy(plot.miny),
            w: sx(plot.maxx + 1) - sx(plot.minx),
            h: sy(plot.maxy + 1) - sy(plot.miny),
        }),
        fill_ratio: r1,
    }
}

#[derive(Clone, Copy)]
struct Blob {
    minx: usize,
    miny: usize,
    maxx: usize,
    maxy: usize,
    pixels: usize,
}

/// Largest 4-connected component of content cells (luma < [`CONTENT_LUMA_MAX`]),
/// by pixel count. Iterative flood fill (no recursion).
fn largest_blob(w: usize, h: usize, data: &[u8]) -> Option<Blob> {
    let mut seen = vec![false; w * h];
    let mut best: Option<Blob> = None;
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for sy in 0..h {
        for sx in 0..w {
            let idx = sy * w + sx;
            if seen[idx] || data[idx] >= CONTENT_LUMA_MAX {
                continue;
            }
            let (mut minx, mut miny, mut maxx, mut maxy, mut pixels) = (sx, sy, sx, sy, 0usize);
            stack.push((sx, sy));
            seen[idx] = true;
            while let Some((x, y)) = stack.pop() {
                pixels += 1;
                minx = minx.min(x);
                miny = miny.min(y);
                maxx = maxx.max(x);
                maxy = maxy.max(y);
                let mut push = |nx: usize, ny: usize, stack: &mut Vec<(usize, usize)>| {
                    let nidx = ny * w + nx;
                    if !seen[nidx] && data[nidx] < CONTENT_LUMA_MAX {
                        seen[nidx] = true;
                        stack.push((nx, ny));
                    }
                };
                if x > 0 {
                    push(x - 1, y, &mut stack);
                }
                if x + 1 < w {
                    push(x + 1, y, &mut stack);
                }
                if y > 0 {
                    push(x, y - 1, &mut stack);
                }
                if y + 1 < h {
                    push(x, y + 1, &mut stack);
                }
            }
            if best.is_none_or(|b| pixels > b.pixels) {
                best = Some(Blob {
                    minx,
                    miny,
                    maxx,
                    maxy,
                    pixels,
                });
            }
        }
    }
    best
}

/// For each screen-x in `xs`, the **curve's screen-y** inside `region`: the top
/// edge of the plotted (non-background) content in that column — i.e. the data
/// line of a line/area chart. `None` where the column has no content. The caller
/// maps screen-y → value with an axis calibration. Reads a window/display capture
/// of the **rendered** chart — no hover, no crosshair.
pub fn curve_screen_y(
    image: &CGImage,
    geometry: &CaptureGeometry,
    region: &Bbox,
    xs: &[f64],
) -> Vec<Option<f64>> {
    let Some((w, h, d)) = luma_grid(image, FINE_W) else {
        return xs.iter().map(|_| None).collect();
    };
    let (ox, oy) = geometry.window_origin_pt;
    let (sw, sh) = geometry.window_size_pt;
    if sw <= 0.0 || sh <= 0.0 {
        return xs.iter().map(|_| None).collect();
    }
    // region's vertical band in grid rows (a little inset to avoid the plot's
    // own top border).
    let row = |y: f64| (((y - oy) / sh) * h as f64).round();
    let gy0 = (row(region.y).max(0.0) as usize).min(h.saturating_sub(1));
    let gy1 = ((row(region.y + region.h)).min(h as f64) as usize).max(gy0 + 1);
    // Topmost content cell in the column = the curve's top edge. (The line is
    // ~1px over a pale, non-content area fill, so we don't require thickness; the
    // neighbourhood median below rejects the odd gridline/outlier instead.)
    let column_y = |gx: usize| -> Option<usize> {
        (gy0..gy1).find(|&gy| d[gy * w + gx] < CONTENT_LUMA_MAX)
    };
    xs.iter()
        .map(|&x| {
            let gxc = (((x - ox) / sw) * w as f64).round() as isize;
            // Sample a small column neighbourhood and take the MEDIAN curve row:
            // robust to a 1-column gap (a vertical gridline clears that exact x)
            // and to a single noisy column.
            let mut rows: Vec<usize> = (-2..=2)
                .filter_map(|dgx| {
                    let gx = gxc + dgx;
                    (gx >= 0 && (gx as usize) < w).then(|| column_y(gx as usize)).flatten()
                })
                .collect();
            if rows.is_empty() {
                return None;
            }
            rows.sort_unstable();
            let gy = rows[rows.len() / 2];
            Some(oy + (gy as f64 + 0.5) / h as f64 * sh)
        })
        .collect()
}

/// Downsample the CGImage to a `target_w`-wide luminance grid. Mirrors the
/// `shapes` sampler (BGRA/RGBA agnostic — averages the colour channels).
fn luma_grid(image: &CGImage, target_w: usize) -> Option<(usize, usize, Vec<u8>)> {
    let (src_w, src_h) = (image.width(), image.height());
    if src_w == 0 || src_h == 0 {
        return None;
    }
    let bpp = (image.bits_per_pixel() / 8).max(1);
    let bpr = image.bytes_per_row();
    let bytes = image.data();
    let raw = bytes.bytes();
    if raw.is_empty() || bpr == 0 {
        return None;
    }
    let dst_w = target_w.min(src_w).max(1);
    let dst_h = ((src_h as f64 * dst_w as f64 / src_w as f64).round() as usize).max(1);
    let mut data = vec![0u8; dst_w * dst_h];
    for y in 0..dst_h {
        let sy = (((y as f64 + 0.5) * src_h as f64 / dst_h as f64).floor() as usize).min(src_h - 1);
        for x in 0..dst_w {
            let sx =
                (((x as f64 + 0.5) * src_w as f64 / dst_w as f64).floor() as usize).min(src_w - 1);
            let off = sy.saturating_mul(bpr).saturating_add(sx.saturating_mul(bpp));
            data[y * dst_w + x] = if bpp >= 3 && off + 2 < raw.len() {
                ((raw[off] as u16 + raw[off + 1] as u16 + raw[off + 2] as u16) / 3) as u8
            } else if off < raw.len() {
                raw[off]
            } else {
                255
            };
        }
    }
    Some((dst_w, dst_h, data))
}

/// Fraction of non-background cells over the whole grid.
fn overall_fill(w: usize, h: usize, data: &[u8]) -> f32 {
    if w * h == 0 {
        return 0.0;
    }
    let content = data.iter().filter(|&&v| v < CONTENT_LUMA_MAX).count();
    content as f32 / (w * h) as f32
}
