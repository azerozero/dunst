> **Historical note:** This file records an earlier VisualOps-era work package or review. Current crate names, setup commands, and status live in docs/README.md, docs/ARCHITECTURE.md, and docs/CONTRACTS.md.

# WP-H — P1a capture + OCR latency truth (owner: Codex / tmux %2)

Work **only** in `crates/visualops-vision` — files `src/capture.rs`, `src/ocr.rs`,
`Cargo.toml`, and a new bench binary/example. **Do NOT touch `src/coords.rs`**
(Claude owns it) nor `src/lib.rs` types (the shared contract is frozen). No other
crate. **No git.** Keep `cargo build -p visualops-vision` green.

Goal: produce the **authoritative Rust latency number** that resolves the §10
contradiction (Codex Swift 16.5 ms vs the public 131 ms) and the P1a GO/NO-GO.

## H1 — `capture::capture_window(window_id)`
One-shot capture of a window's pixels by `window_id` (= CGWindowID =
`SCWindow.windowID`). Prefer **ScreenCaptureKit** `SCScreenshotManager` (macOS 14+,
crate `screencapturekit` v6 or `objc2-screen-capture-kit`); fallback
`CGWindowListCreateImage` (`core-graphics`, already a dep). Return the captured
image (you pick the representation best handed to Vision — CGImage / CVPixelBuffer)
**plus** a filled `crate::CaptureGeometry` (window origin pt, window size pt, image
size px, backing scale). You own adding the SCK/objc2 deps to `Cargo.toml`.

## H2 — `ocr::ocr_region(image, region)` via Apple Vision
`VNRecognizeTextRequest` at **`.fast`**, **`usesLanguageCorrection = false`**, over
a region (the fovea) of the captured image. Return `Vec<crate::OcrBox>` (text +
Vision-normalised bbox + confidence). Map each box to screen points via
`crate::coords::vision_norm_to_screen_pt` (Claude's; a working first cut is already
in the tree). `.accurate` only as an optional second pass.

## H3 — bench binary (the GO/NO-GO)
Add `src/bin/vision_bench.rs` (or `examples/`) that takes a `window_id` arg
(Notes' main window was ~93 earlier — but resolve/accept it as an arg; you can
enumerate via `CGWindowListCopyWindowInfo` for a pid), then:
1. Warms up Vision (first request is cold), then times **capture** and **region
   OCR** separately over ~10–20 runs: print **p50/p95 ms** for each, plus #lines.
2. Runs both a fovea-sized region (~600×400 pt) and the full window.
3. Prints a verdict line vs the gate: **capture < ~15 ms** and **region OCR
   < ~60 ms warm** ⇒ `GO`, else `NO-GO`.
Report points-vs-pixels explicitly (Retina 2×).

## Pitfalls (from the reviews)
Vision coords are normalised **bottom-left** → always go through `coords`. Turn
language correction **off**. Watch cold-start (warm up before timing). Don't let
the bench move the real cursor. Note CJK/small-font/low-contrast caveats if you
hit them.

Finish: print `cargo build -p visualops-vision`, the bench output (p50/p95 capture
+ OCR, GO/NO-GO), the crate versions you settled on, and a short summary.
