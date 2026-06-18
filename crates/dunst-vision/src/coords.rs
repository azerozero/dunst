//! Coordinate-transform chain (owner: Claude, P1a) — the #1 predicted bug source
//! (docs/P1-vision-surfaces.md §10.8).
//!
//! Vision returns normalised, **bottom-left** boxes relative to the captured
//! image. Everything else in the system (bbox, CGEvent actions) is top-left
//! **screen points**. This module owns that conversion, both directions, and
//! is thoroughly unit-tested (Y-flip, Retina/backing-scale, window origin
//! offset, multi-DPI). Pure logic — **no platform deps**.
//!
//! ## Frozen geometry assumption (P1a)
//! The captured image covers **exactly** the target window with **uniform**
//! scale on both axes: `image_size_px = window_size_pt * backing_scale`. This is
//! what a `ScreenCaptureKit` `SCContentFilter` over a single window yields. Under
//! that assumption `backing_scale` **cancels**: Vision's coordinates are already
//! normalised against the image pixel size, and we map straight into *points*, so
//! the pixel size (and hence the scale factor) never enters the point-space math.
//! `retina_and_non_retina_agree` locks this in. `image_size_px` /
//! `backing_scale` are therefore unused by the transforms below; they are kept on
//! [`CaptureGeometry`] for callers that need pixel-space (e.g. cropping the buffer
//! Vision OCRs) and as the place to detect **non-uniform scale / letterboxing**
//! if a future capture path ever breaks the "image == window" invariant
//! (see [`uniform_scale`]).
//!
//! ## Invariants locked here
//! 1. **Y-flip**: Vision `y=0` is the image bottom → window's **lower** screen
//!    edge; Vision `y≈1` → window **top**.
//! 2. **Round-trip identity**: [`vision_norm_to_screen_pt`] and
//!    [`screen_pt_to_vision_norm`] are exact inverses (modulo f64 epsilon).
//! 3. **Scale invariance**: the point-space result is independent of
//!    `backing_scale` (Retina 2× and non-Retina 1× agree).
//! 4. **ROI clamping**: [`window_rect_to_vision_roi`] always returns a valid
//!    sub-rectangle of the unit square (edge-clamped, never origin-shifted).

use crate::{CaptureGeometry, NormRect};
use dunst_core::Bbox;

/// Map a Vision normalised rect (bottom-left origin, relative to the captured
/// image) to a top-left **screen-point** [`Bbox`].
///
/// `NormRect { x, y, w, h }` has its **bottom-left** corner at `(x, y)` and
/// extends right by `w` and **up** by `h`, so its top edge sits at normalised-y
/// `y + h` (measured from the bottom). Converting that to a fraction down from the
/// top gives `1 - (y + h)`, hence the Y term below.
///
/// `backing_scale` cancels here — see the module-level note. Exact inverse of
/// [`screen_pt_to_vision_norm`]; this map does **not** clamp (it must round-trip
/// boxes that legitimately touch or slightly exceed the window edge).
pub fn vision_norm_to_screen_pt(n: NormRect, geom: &CaptureGeometry) -> Bbox {
    let (win_w, win_h) = geom.window_size_pt;
    let (origin_x, origin_y) = geom.window_origin_pt;
    Bbox {
        x: origin_x + n.x * win_w,
        // Flip Vision's bottom-left origin to our top-left origin: the rect's top
        // edge is at normalised-from-bottom `n.y + n.h`, i.e. `1 - n.y - n.h` down
        // from the top.
        y: origin_y + (1.0 - n.y - n.h) * win_h,
        w: n.w * win_w,
        h: n.h * win_h,
    }
}

/// Map a Vision normalised rect returned from a `regionOfInterest` request back
/// into screen points. Vision normalises OCR observations inside the ROI, not the
/// full image, so a `read_text(region=...)` result must be scaled by that region
/// before agents can safely click the returned bbox centre.
pub fn vision_norm_to_screen_pt_in_region(n: NormRect, region_screen_pt: Bbox) -> Bbox {
    Bbox {
        x: region_screen_pt.x + n.x * region_screen_pt.w,
        y: region_screen_pt.y + (1.0 - n.y - n.h) * region_screen_pt.h,
        w: n.w * region_screen_pt.w,
        h: n.h * region_screen_pt.h,
    }
}

/// Inverse of [`vision_norm_to_screen_pt`]: a top-left **screen-point** [`Bbox`]
/// back to a Vision normalised, **bottom-left** [`NormRect`].
///
/// Used for the action path (graph bbox → where to synthesise a CGEvent) and to
/// drive Vision's `regionOfInterest`. Like the forward map this is **unclamped**
/// so the two compose to the identity; clamp explicitly via
/// [`window_rect_to_vision_roi`] (or [`clamp_unit`]) when a valid ROI is required.
///
/// Derivation of the Y term: from `b.y = origin_y + (1 - n.y - n.h) * win_h` and
/// `n.h = b.h / win_h`, solving for `n.y` gives
/// `n.y = 1 - (b.h + b.y - origin_y) / win_h`.
pub fn screen_pt_to_vision_norm(b: Bbox, geom: &CaptureGeometry) -> NormRect {
    let (win_w, win_h) = geom.window_size_pt;
    let (origin_x, origin_y) = geom.window_origin_pt;
    NormRect {
        x: (b.x - origin_x) / win_w,
        y: 1.0 - (b.h + b.y - origin_y) / win_h,
        w: b.w / win_w,
        h: b.h / win_h,
    }
}

/// Map a fovea rectangle expressed in **window-local points** (origin at the
/// window's top-left, `x` right / `y` down, range `~[0,win_w] × [0,win_h]`) to the
/// normalised **bottom-left** [`NormRect`] to hand to Vision's `regionOfInterest`,
/// clamped to the unit square.
///
/// Window-local input is translated to global screen points (`+ window_origin_pt`)
/// and run through [`screen_pt_to_vision_norm`], so the Y-flip lives in exactly one
/// place. The result is then edge-clamped to `[0,1]` via [`clamp_unit`] — a fovea
/// may legitimately spill past the window edge (cursor near a border); the ROI is
/// the in-window intersection.
pub fn window_rect_to_vision_roi(rect_in_window_pt: Bbox, geom: &CaptureGeometry) -> NormRect {
    let (origin_x, origin_y) = geom.window_origin_pt;
    let global = Bbox {
        x: origin_x + rect_in_window_pt.x,
        y: origin_y + rect_in_window_pt.y,
        w: rect_in_window_pt.w,
        h: rect_in_window_pt.h,
    };
    clamp_unit(screen_pt_to_vision_norm(global, geom))
}

/// Edge-clamp a [`NormRect`] to the unit square, returning a valid sub-rectangle.
///
/// Clamps the rect's **edges** (not its origin) so a partly-outside rect keeps its
/// in-bounds portion instead of being shifted: e.g. `x=-0.2, w=0.5` → `x=0, w=0.3`.
/// Negative widths/heights are normalised (min/max of the two edges). A fully
/// out-of-bounds rect collapses to a zero-size rect on the nearest edge.
pub fn clamp_unit(r: NormRect) -> NormRect {
    let x0 = r.x.min(r.x + r.w).clamp(0.0, 1.0);
    let x1 = r.x.max(r.x + r.w).clamp(0.0, 1.0);
    let y0 = r.y.min(r.y + r.h).clamp(0.0, 1.0);
    let y1 = r.y.max(r.y + r.h).clamp(0.0, 1.0);
    NormRect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    }
}

/// True if the captured image covers the window with **uniform** scale on both
/// axes (the P1a invariant), within `eps`. A `false` result means letterboxing or
/// anisotropic scaling — the point-space transforms above would no longer be
/// valid and the capture geometry needs revisiting.
pub fn uniform_scale(geom: &CaptureGeometry, eps: f64) -> bool {
    let (win_w, win_h) = geom.window_size_pt;
    let (img_w, img_h) = geom.image_size_px;
    if win_w <= 0.0 || win_h <= 0.0 {
        return false;
    }
    let sx = img_w / win_w;
    let sy = img_h / win_h;
    (sx - geom.backing_scale).abs() < eps && (sy - geom.backing_scale).abs() < eps
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-9;

    fn geom() -> CaptureGeometry {
        CaptureGeometry {
            window_origin_pt: (100.0, 50.0),
            window_size_pt: (1000.0, 600.0),
            image_size_px: (2000.0, 1200.0), // Retina 2×
            backing_scale: 2.0,
        }
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }
    fn bbox_approx(a: Bbox, b: Bbox) -> bool {
        approx(a.x, b.x) && approx(a.y, b.y) && approx(a.w, b.w) && approx(a.h, b.h)
    }
    fn norm_approx(a: NormRect, b: NormRect) -> bool {
        approx(a.x, b.x) && approx(a.y, b.y) && approx(a.w, b.w) && approx(a.h, b.h)
    }

    // --- I1: forward map ----------------------------------------------------

    #[test]
    fn full_image_maps_to_full_window() {
        let b = vision_norm_to_screen_pt(
            NormRect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            &geom(),
        );
        assert_eq!(
            b,
            Bbox {
                x: 100.0,
                y: 50.0,
                w: 1000.0,
                h: 600.0
            }
        );
    }

    #[test]
    fn bottom_left_unit_box_flips_to_top_left_screen() {
        // Vision (0,0) is the image's BOTTOM-left; in top-left screen space that is
        // the window's lower edge.
        let b = vision_norm_to_screen_pt(
            NormRect {
                x: 0.0,
                y: 0.0,
                w: 0.1,
                h: 0.1,
            },
            &geom(),
        );
        assert_eq!(b.x, 100.0);
        assert_eq!(b.y, 50.0 + 0.9 * 600.0); // 1 - 0 - 0.1 = 0.9
    }

    #[test]
    fn y_flip_bottom_vs_top() {
        let g = geom();
        // y=0 (image bottom) → window's LOWER screen edge.
        let bottom = vision_norm_to_screen_pt(
            NormRect {
                x: 0.0,
                y: 0.0,
                w: 0.2,
                h: 0.05,
            },
            &g,
        );
        assert!(approx(bottom.y, 50.0 + (1.0 - 0.05) * 600.0)); // near bottom (large screen-y)
                                                                // y≈1 (image top, with tiny height) → window's TOP screen edge.
        let top = vision_norm_to_screen_pt(
            NormRect {
                x: 0.0,
                y: 0.95,
                w: 0.2,
                h: 0.05,
            },
            &g,
        );
        assert!(approx(top.y, 50.0)); // 1 - 0.95 - 0.05 = 0 → origin_y
                                      // Top must be visually above bottom (smaller screen-y).
        assert!(top.y < bottom.y);
    }

    // --- I2: inverse + ROI --------------------------------------------------

    #[test]
    fn screen_to_norm_is_inverse_on_known_value() {
        let g = geom();
        let n = NormRect {
            x: 0.3,
            y: 0.4,
            w: 0.2,
            h: 0.1,
        };
        let back = screen_pt_to_vision_norm(vision_norm_to_screen_pt(n, &g), &g);
        assert!(norm_approx(n, back));
    }

    #[test]
    fn roi_full_window_is_unit_square() {
        let g = geom();
        let roi = window_rect_to_vision_roi(
            Bbox {
                x: 0.0,
                y: 0.0,
                w: 1000.0,
                h: 600.0,
            },
            &g,
        );
        assert!(norm_approx(
            roi,
            NormRect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0
            }
        ));
    }

    #[test]
    fn roi_top_left_window_quadrant_maps_to_top_left_normalised() {
        let g = geom();
        // Window-local top-left quadrant (top-down points) → Vision bottom-left
        // normalised: it's the UPPER half in normalised-y, so y starts at 0.5.
        let roi = window_rect_to_vision_roi(
            Bbox {
                x: 0.0,
                y: 0.0,
                w: 500.0,
                h: 300.0,
            },
            &g,
        );
        assert!(norm_approx(
            roi,
            NormRect {
                x: 0.0,
                y: 0.5,
                w: 0.5,
                h: 0.5
            }
        ));
    }

    #[test]
    fn roi_clamps_partly_outside() {
        let g = geom();
        // Fovea spilling past the LEFT and TOP edges of the window.
        let roi = window_rect_to_vision_roi(
            Bbox {
                x: -200.0,
                y: -100.0,
                w: 400.0,
                h: 300.0,
            },
            &g,
        );
        // x: [-200,200]pt → norm [-0.2,0.2] → clamp → [0,0.2].
        assert!(approx(roi.x, 0.0));
        assert!(approx(roi.w, 0.2));
        // Everything stays inside the unit square.
        assert!(roi.x >= 0.0 && roi.y >= 0.0);
        assert!(roi.x + roi.w <= 1.0 + EPS && roi.y + roi.h <= 1.0 + EPS);
    }

    #[test]
    fn roi_fully_outside_collapses_to_empty_on_edge() {
        let g = geom();
        // Entirely to the right of the window.
        let roi = window_rect_to_vision_roi(
            Bbox {
                x: 2000.0,
                y: 0.0,
                w: 100.0,
                h: 100.0,
            },
            &g,
        );
        assert!(approx(roi.w, 0.0));
        assert!(roi.x >= 0.0 && roi.x <= 1.0);
    }

    // --- I3: round-trips ----------------------------------------------------

    #[test]
    fn round_trip_norm_screen_norm() {
        let g = geom();
        let cases = [
            NormRect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            NormRect {
                x: 0.25,
                y: 0.1,
                w: 0.5,
                h: 0.3,
            },
            NormRect {
                x: 0.9,
                y: 0.85,
                w: 0.05,
                h: 0.1,
            },
            NormRect {
                x: 0.0,
                y: 0.5,
                w: 0.0,
                h: 0.0,
            }, // zero-size
        ];
        for n in cases {
            let back = screen_pt_to_vision_norm(vision_norm_to_screen_pt(n, &g), &g);
            assert!(
                norm_approx(n, back),
                "norm round-trip failed for {n:?} -> {back:?}"
            );
        }
    }

    #[test]
    fn round_trip_screen_norm_screen() {
        let g = geom();
        let cases = [
            Bbox {
                x: 100.0,
                y: 50.0,
                w: 1000.0,
                h: 600.0,
            },
            Bbox {
                x: 250.0,
                y: 200.0,
                w: 300.0,
                h: 120.0,
            },
            Bbox {
                x: 1080.0,
                y: 60.0,
                w: 20.0,
                h: 40.0,
            },
            Bbox {
                x: 600.0,
                y: 350.0,
                w: 0.0,
                h: 0.0,
            }, // zero-size
        ];
        for b in cases {
            let back = vision_norm_to_screen_pt(screen_pt_to_vision_norm(b, &g), &g);
            assert!(
                bbox_approx(b, back),
                "screen round-trip failed for {b:?} -> {back:?}"
            );
        }
    }

    #[test]
    fn roi_relative_vision_box_maps_inside_requested_screen_region() {
        let region = Bbox {
            x: 3120.0,
            y: 740.0,
            w: 620.0,
            h: 570.0,
        };
        let got = vision_norm_to_screen_pt_in_region(
            NormRect {
                x: 0.10,
                y: 0.20,
                w: 0.30,
                h: 0.25,
            },
            region,
        );

        assert!(bbox_approx(
            got,
            Bbox {
                x: 3182.0,
                y: 1053.5,
                w: 186.0,
                h: 142.5,
            }
        ));
        assert!(got.x >= region.x && got.x + got.w <= region.x + region.w);
        assert!(got.y >= region.y && got.y + got.h <= region.y + region.h);
    }

    // --- I3: scale invariance (backing_scale cancels) -----------------------

    #[test]
    fn retina_and_non_retina_agree() {
        let n = NormRect {
            x: 0.3,
            y: 0.2,
            w: 0.25,
            h: 0.15,
        };
        let non_retina = CaptureGeometry {
            window_origin_pt: (100.0, 50.0),
            window_size_pt: (1000.0, 600.0),
            image_size_px: (1000.0, 600.0), // 1×
            backing_scale: 1.0,
        };
        let retina = CaptureGeometry {
            window_origin_pt: (100.0, 50.0),
            window_size_pt: (1000.0, 600.0),
            image_size_px: (2000.0, 1200.0), // 2×
            backing_scale: 2.0,
        };
        // Same point-space result regardless of backing scale.
        assert_eq!(
            vision_norm_to_screen_pt(n, &non_retina),
            vision_norm_to_screen_pt(n, &retina),
        );
        // And a fractional scale agrees too.
        let frac = CaptureGeometry {
            image_size_px: (1500.0, 900.0),
            backing_scale: 1.5,
            ..retina
        };
        assert_eq!(
            vision_norm_to_screen_pt(n, &retina),
            vision_norm_to_screen_pt(n, &frac),
        );
    }

    // --- I3: window origin offset, incl. negative (display left of main) ----

    #[test]
    fn negative_window_origin_offsets_screen_coords() {
        // A window on a display to the LEFT of the main one → negative origin.
        let g = CaptureGeometry {
            window_origin_pt: (-1280.0, 50.0),
            window_size_pt: (800.0, 600.0),
            image_size_px: (1600.0, 1200.0),
            backing_scale: 2.0,
        };
        let b = vision_norm_to_screen_pt(
            NormRect {
                x: 0.5,
                y: 0.5,
                w: 0.1,
                h: 0.1,
            },
            &g,
        );
        assert!(approx(b.x, -1280.0 + 0.5 * 800.0)); // -880.0, negative is fine
        assert!(approx(b.y, 50.0 + (1.0 - 0.5 - 0.1) * 600.0));
        // Round-trip survives the negative origin.
        let back = screen_pt_to_vision_norm(b, &g);
        assert!(norm_approx(
            back,
            NormRect {
                x: 0.5,
                y: 0.5,
                w: 0.1,
                h: 0.1
            }
        ));
    }

    // --- I3: degenerate clamp helper ---------------------------------------

    #[test]
    fn clamp_unit_keeps_in_bounds_portion() {
        // Negative origin keeps the right portion, not a shifted full-width rect.
        let c = clamp_unit(NormRect {
            x: -0.2,
            y: 0.1,
            w: 0.5,
            h: 0.2,
        });
        assert!(norm_approx(
            c,
            NormRect {
                x: 0.0,
                y: 0.1,
                w: 0.3,
                h: 0.2
            }
        ));
        // Spilling past the far edge is trimmed.
        let c2 = clamp_unit(NormRect {
            x: 0.8,
            y: 0.8,
            w: 0.5,
            h: 0.5,
        });
        assert!(norm_approx(
            c2,
            NormRect {
                x: 0.8,
                y: 0.8,
                w: 0.2,
                h: 0.2
            }
        ));
        // Already-unit rect is untouched.
        let c3 = clamp_unit(NormRect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        });
        assert!(norm_approx(
            c3,
            NormRect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0
            }
        ));
    }

    #[test]
    fn uniform_scale_detects_letterboxing() {
        assert!(uniform_scale(&geom(), 1e-6));
        // Anisotropic: x scaled 2×, y scaled 1.5× → not uniform.
        let aniso = CaptureGeometry {
            window_origin_pt: (0.0, 0.0),
            window_size_pt: (1000.0, 600.0),
            image_size_px: (2000.0, 900.0),
            backing_scale: 2.0,
        };
        assert!(!uniform_scale(&aniso, 1e-6));
    }
}
