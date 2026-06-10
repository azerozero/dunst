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
use visualops_core::Bbox;

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

/// Luma at/above this is treated as background; the (dark) curve and axis text
/// fall below it, a pale watermark on a blank plot does not.
const CONTENT_LUMA_MAX: u8 = 205;
/// Coarse grid width — fast blank/present gate.
const COARSE_W: usize = 80;
/// Fine grid width — locate the content bbox.
const FINE_W: usize = 256;
/// Plot band insets (skip the header / axis / surrounding UI chrome).
const BAND_X0: usize = 4;
const BAND_X1: usize = 97;
const BAND_Y0: usize = 5;
const BAND_Y1: usize = 78;
/// Minimum band fill to call the plot "rendered".
const PRESENT_RATIO: f32 = 0.008;

/// Detect whether the target's chart is rendered and, if so, where its plotted
/// content lies (in screen points). Use a base window/display capture — the data
/// curve is base content (a window grab captures it; only the transient crosshair
/// overlay needs the composited path).
pub fn detect_chart_region(image: &CGImage, geometry: &CaptureGeometry) -> ChartDetection {
    // L0 — very coarse: blank plots reject here without touching the fine grid.
    let Some((w0, h0, d0)) = luma_grid(image, COARSE_W) else {
        return ChartDetection {
            present: false,
            region: None,
            fill_ratio: 0.0,
        };
    };
    let (r0, _) = band_fill(w0, h0, &d0);
    if r0 < PRESENT_RATIO {
        return ChartDetection {
            present: false,
            region: None,
            fill_ratio: r0,
        };
    }

    // L1 — finer: confirm and locate the content bounding box.
    let (w, h, d) = luma_grid(image, FINE_W).unwrap_or((w0, h0, d0));
    let (r1, bb) = band_fill(w, h, &d);
    let present = r1 >= PRESENT_RATIO;
    let region = bb.filter(|_| present).map(|(minx, miny, maxx, maxy)| {
        let (ox, oy) = geometry.window_origin_pt;
        let (sw, sh) = geometry.window_size_pt;
        let sx = |gx: usize| ox + (gx as f64 / w as f64) * sw;
        let sy = |gy: usize| oy + (gy as f64 / h as f64) * sh;
        Bbox {
            x: sx(minx),
            y: sy(miny),
            w: sx(maxx + 1) - sx(minx),
            h: sy(maxy + 1) - sy(miny),
        }
    });
    ChartDetection {
        present,
        region,
        fill_ratio: r1,
    }
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

/// Fraction of non-background cells inside the central plot band, plus their
/// bounding box (in grid cells).
#[allow(clippy::type_complexity)]
fn band_fill(w: usize, h: usize, data: &[u8]) -> (f32, Option<(usize, usize, usize, usize)>) {
    let x0 = (w * BAND_X0 / 100).min(w.saturating_sub(1));
    let x1 = (w * BAND_X1 / 100).max(x0 + 1).min(w);
    let y0 = (h * BAND_Y0 / 100).min(h.saturating_sub(1));
    let y1 = (h * BAND_Y1 / 100).max(y0 + 1).min(h);
    let (mut content, mut total) = (0u32, 0u32);
    let (mut minx, mut miny, mut maxx, mut maxy) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for gy in y0..y1 {
        for gx in x0..x1 {
            total += 1;
            if data[gy * w + gx] < CONTENT_LUMA_MAX {
                content += 1;
                minx = minx.min(gx);
                miny = miny.min(gy);
                maxx = maxx.max(gx);
                maxy = maxy.max(gy);
            }
        }
    }
    let ratio = if total > 0 {
        content as f32 / total as f32
    } else {
        0.0
    };
    let bbox = (content > 0).then_some((minx, miny, maxx, maxy));
    (ratio, bbox)
}
