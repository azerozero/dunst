# WP-I — P1a coordinate-transform chain, hardened + tested (owner: Claude / tmux %3)

Work **only** in `crates/visualops-vision/src/coords.rs` (logic + its `#[cfg(test)]`
tests). **Do NOT touch** `src/lib.rs` (shared contract types are frozen),
`src/capture.rs`, `src/ocr.rs` (Codex's), or `Cargo.toml`. No other crate. **No
git.** Keep `cargo test -p visualops-vision` green.

This is the §10.8 "first build item" — the #1 predicted bug source. Pure logic, no
platform deps; make it bullet-proof so the rest of P1 builds on solid coordinates.

## I1 — verify & harden `vision_norm_to_screen_pt`
A working first cut exists. Verify the math (Vision normalised, **bottom-left**
origin, relative to the captured image → top-left **screen points**): Y-flip,
window-origin offset, and the points-vs-pixels relationship (`image_size_px =
window_size_pt * backing_scale`). Decide explicitly whether `backing_scale`
cancels (it does if the image covers exactly the window with uniform scale) and
document the assumption; handle non-uniform scale / letterboxing if it can occur.

## I2 — add the inverse + a fovea→ROI helper
The pipeline needs the **reverse** map too (for actions, and to tell Vision *where*
to OCR):
- `screen_pt_to_vision_norm(b: Bbox, geom) -> NormRect` — inverse of I1.
- `window_rect_to_vision_roi(rect_in_window_pt, geom) -> NormRect` — given a fovea
  rectangle in window points, the normalised bottom-left ROI to pass to Vision's
  `regionOfInterest`. Clamp to `[0,1]`.

## I3 — tests (make them thorough)
- Round-trip: `screen ← norm ← screen` and `norm ← screen ← norm` ≈ identity
  (within f64 epsilon), across several boxes.
- Retina (scale 2.0) **and** non-Retina (1.0) geometries give the same point-space
  result (scale cancels) — lock it with a test.
- Y-flip correctness: a box at Vision-y=0 lands at the window's **bottom** in
  screen space; Vision-y near 1 lands at the **top**.
- Window origin offset (incl. a negative origin = a display left of the main one).
- Degenerate/clamped cases: zero-size box, ROI partly outside `[0,1]` → clamped.

Coordinate types are in `lib.rs` (`CaptureGeometry`, `NormRect`) and
`visualops_core::Bbox` — use them as-is; if you think a shared type is missing,
STOP and say so in your summary rather than editing `lib.rs`.

Finish: print `cargo test -p visualops-vision`, list the functions + invariants you
locked, and a short summary.
