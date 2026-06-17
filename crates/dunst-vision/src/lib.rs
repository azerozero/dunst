//! `dunst-vision` — P1a spike for the non-AX vision pipeline.
//!
//! Goal: settle the < 100 ms GO/NO-GO (docs/P1-vision-surfaces.md §10) by
//! measuring, **in Rust**, ScreenCaptureKit window capture + Apple Vision `.fast`
//! OCR over a fovea-sized region, and by building + unit-testing the
//! coordinate-transform chain (the #1 predicted bug source).
//!
//! Module ownership for the spike (disjoint, see docs/WP-H / WP-I):
//! - [`coords`]  — pure logic, no platform deps (owner: Claude).
//! - [`capture`] — ScreenCaptureKit window capture (owner: Codex, macOS only).
//! - [`ocr`]     — Apple Vision OCR (owner: Codex, macOS only).
//!
//! The three share the contract types below so the two owners never edit the
//! same file.

pub mod coords;

#[cfg(target_os = "macos")]
pub mod capture;
#[cfg(target_os = "macos")]
pub mod detect;
#[cfg(target_os = "macos")]
pub mod ocr;
#[cfg(target_os = "macos")]
pub mod shapes;

/// Everything needed to map Vision's normalised, **bottom-left** coordinates into
/// our top-left **screen-point** space. Produced by [`capture`], consumed by
/// [`coords`]. Express budgets/positions in points; remember the captured image
/// is in pixels (`size_pt * backing_scale`, e.g. 2× on Retina).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptureGeometry {
    /// Top-left of the captured window in global screen **points**.
    pub window_origin_pt: (f64, f64),
    /// Logical window size in **points**.
    pub window_size_pt: (f64, f64),
    /// Captured image size in **pixels** (Retina: `window_size_pt * backing_scale`).
    pub image_size_px: (f64, f64),
    /// Backing scale factor (e.g. `2.0` on Retina, `1.0` otherwise).
    pub backing_scale: f64,
}

/// A Vision-space normalised rectangle: `x/y/w/h` in `[0,1]`, origin **bottom-left**
/// (Vision's convention). `coords` converts this to a top-left screen-point
/// [`dunst_core::Bbox`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// One OCR result line: recognised `text`, its Vision-normalised box, and Vision's
/// confidence in `[0,1]`.
///
/// Design intent (§10.7): risk must stay **monotone in uncertainty** downstream —
/// low `confidence` should *raise* the gate, never lower it.
///
/// TODO P1: not wired yet. The POC is AX-only (`confidence` is effectively `1.0`),
/// and `RiskEngine` does not read `confidence` — see `dunst-graph::risk`. This
/// is a documented intent, **not** a current guarantee; do not rely on it until the
/// vision/OCR source is fed into the risk gate.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrBox {
    pub text: String,
    pub norm: NormRect,
    pub confidence: f32,
}
