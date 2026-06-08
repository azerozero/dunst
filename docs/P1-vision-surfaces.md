# P1 — Non-AX Vision Surfaces — Plan & Deep-Think

> Target: pixels → the **same** affordance graph as the AX path, **< 100 ms**
> steady-state, **no LLM** in the hot loop.

## 0. Why
AX-first covers apps that expose an accessibility tree. It **fails** on:
Electron/Chromium with AX off, games, remote desktop (VNC/RDP) windows,
canvas/WebGL, video, screen-shared content, custom-drawn UIs. For those the only
truth is pixels. We must turn pixels into the identical `{id, role, label,
actions, risk}` graph, so the agent's interface is the same regardless of source.

## 1. Load-bearing insight — vision is a *second Perceptor*
The contract already anticipates this: `Perceptor` trait, `Source::{Accessibility,
Vision, Ocr}`, `confidence: f32` (1.0 for AX, < 1.0 for vision/OCR). So:
- `VisionPerceptor: Perceptor` returns `RawAxNode`s synthesised from pixels
  (role inferred, `label` = OCR text, bbox = pixel rect, `source = Vision/Ocr`,
  `confidence < 1`).
- **Everything downstream is reused unchanged**: scene graph, stable synth ids,
  affordances, risk engine, MCP tools, audit, diff. ⚠️ **But plumbing-true ≠
  intelligence-free** (Claude review A1): `RawAxNode` is AX-shaped — `ax_actions`
  is empty, `ax_identifier` absent, `children` is a flat OCR list, `ax_role` must
  be synthesised. The affordance & risk layers are **AX-keyword/AX-action driven**,
  so vision nodes get **near-empty graphs** until P1c adds a *role→SemanticAction*
  table + geometric parent/child synthesis. See §10. The reuse is in the data
  structures; the intelligence is rebuilt on the vision side.
- **Actions**: no AX `performAction` → route to **CGEvent pixel** click/type/drag
  at the bbox centre — which we already built in WP-F. The ActionExecutor just
  picks the CGEvent path for `source != Accessibility`.
Optionally **hybrid**: AX where present, vision to fill gaps.

## 2. The < 100 ms budget — the real constraint
Per incremental (dirty-region) refresh, not cold full-frame:

| Stage | Tech | Budget |
|---|---|---|
| Window capture | ScreenCaptureKit (GPU), 1 frame | 5–15 ms |
| Tile hash + dirty diff | fast hash per tile | 1–5 ms |
| OCR on **dirty** tiles | Apple Vision `.fast` (ANE) | 20–55 ms |
| Element detect + role infer | classical CV / light detector | 5–15 ms |
| Build RawAxNodes + graph | pure Rust (measured ~21 µs) | < 5 ms |
| **Total (steady-state)** | | **~40–95 ms** |

### Measured on this Mac (2026-06-08, Codex — Apple Vision, 10 warm runs)
| Case | `.fast` p95 | `.accurate` p95 |
|---|---|---|
| Full window (Retina 2000×1320 px) | **16.5 ms** | 90 ms |
| One tile 800×600 | **9.6 ms** | 66 ms |
| All 9 tiles (dirty grid) | **48 ms** | 264 ms ❌ |

So the OCR budget in the table above was **pessimistic**: full-window `.fast` is
~14–17 ms — meaning OCR latency alone does **not** force tiling. Cold start:
`.fast` full ≈ 37 ms (fine), `.accurate` full ≈ 195 ms (needs warmup). **Tiling /
foveal is still needed — but for the role-inference cost and to cut cross-frame
churn, not for the OCR latency itself.** `.accurate` over a full dirty grid (≈260 ms)
is out of the hot path.

**Hot-path rule (from the measurements):** `.fast` + `usesLanguageCorrection=false`,
OCR only dirty/foveal regions; `.accurate` only as a *targeted second pass* on an
ambiguous tile or for post-action validation. Express budgets in **points vs
pixels** (a 1000×660-pt window is 2000×1320 px on Retina — 4× the OCR area).
Caveat: SCK capture latency was **not** measured here (`screencapture` was used);
it must be benchmarked separately as a hot-loop.

Cold first frame may exceed 100 ms only for `.accurate`; `.fast` stays well under.
The < 100 ms claim holds comfortably for steady-state.

## 3. OCR without an LLM — the answer is YES
LLM-vision (GPT-4V, Claude vision) = 1–10 s + network → **categorically off the
hot path** (reserve only as an *offline* fallback for ambiguous regions, never
per frame). Fast **non-LLM** OCR is the only viable path and it exists:
- **Apple Vision `VNRecognizeTextRequest`** — primary candidate. On-device,
  Neural-Engine accelerated, ships on every Mac, no model to bundle, free. `.fast`
  is built for real-time; gives text + per-line bbox + confidence. Expected
  ~10–50 ms on a window region, less per tile.
- PaddleOCR PP-OCRv4-mobile / **RapidOCR** (ONNXRuntime + CoreML EP) — competitive
  but we ship models + a runtime.
- Tesseract — CPU, ~100–500 ms/page; only on tiny tiles.
- Detection-only (CRAFT/DBNet/EAST) + fast CRNN recognizer — more control, more
  work.
→ **DECIDED (measured, §2): Apple Vision `.fast` wins** — full-window p95 16.5 ms
on this Mac, zero deps, zero model shipping. `.accurate` kept only as a targeted
second pass. Rust path exists today: `objc2-vision` (exposes
`VNRecognizeTextRequest`), `screencapturekit` (v6, maintained), `ort` for a
RapidOCR fallback. (Existence proof: `andelf/picc` = Rust + objc2 + Vision OCR.)

**Known failure modes to design around (Codex review):** small fonts, low
contrast, complex/photographic backgrounds, CJK/RTL, auto-language detection +
linguistic correction (turn correction OFF on the hot path — it both slows and
"helpfully" rewrites UI tokens), Vision's bottom-left normalised coordinates
(transform to our top-left pixel space), and frame-to-frame OCR churn (ties to §5
stable-ids). Selection criterion is not just "< 60 ms" but "< budget **at the
target text density / language / Retina scale**".

## 4. The hard part — text ≠ affordances
OCR gives text + boxes. The graph needs **structure**: *this box is a clickable
button*, *this is a text field*, *this is a row*. Pixels don't say that. So we need
a **role-inference layer**:
- Classical CV: rectangles, borders, contrast steps, separators, focus rings →
  candidate clickable regions; group OCR text into them.
- Heuristics: short centred text in a rounded contrasting rect ≈ button; long
  left-aligned run ≈ label; boxed empty region with a caret ≈ text field;
  repeating horizontal bands ≈ list/rows.
- Optional light ML UI-element detector (small YOLO/DETR on UI screenshots) — P1d.
Vision is fundamentally weaker than AX here → vision nodes carry **lower
confidence** and **fewer guaranteed actions**; risk gating stays conservative.

## 5. Stable IDs across frames — the jitter problem
AX gives `ax_identifier`; vision gives nothing stable. OCR bbox/text jitters
frame-to-frame (±px, ±1 char) → naive `synth_id` churns every frame, breaking
diff/audit/approval. Need a **vision stable-id**: fuzzy key = (coarse grid cell +
normalised text + role), matched against the previous frame's nodes
(nearest-box + text similarity) to carry the id forward. Extends the WP-D stable-id
work into the spatial domain. Without it, verify-loop and approval are meaningless
on vision surfaces.

## 6. Tile + Foveal — the latency strategy
- **Tile**: grid the window (e.g. 256×256); hash each tile per frame; OCR only
  **dirty** tiles. Static chrome (most of the UI) is skipped → steady-state OCR
  area is tiny.
- **Foveal**: full-res OCR only in the fovea — around the cursor / last action /
  changed area; periphery low-res or deferred. Bounds per-frame cost regardless of
  window size.
- **Coalesce**: debounce animations; OCR on settle, not every animation frame.

## 7. Hybrid AX + Vision
When a window has *partial* AX (e.g. Electron), use AX nodes (conf 1.0) and vision
only for gaps (pixels with no AX node). Fuse by spatial overlap; AX wins ties. A
pure-vision app simply has zero AX nodes.

## 8. Phasing
- **P1a — Capture + OCR PoC + LATENCY TRUTH** (this round): SCK window grab + Apple
  Vision OCR on a region; measure real ms (`.fast`/`.accurate`, window vs tile).
  Pick the engine. **GO/NO-GO on < 100 ms.**
- **P1b — Tiling + dirty-region loop**: the steady-state < 100 ms engine.
- **P1c — Element detect + role inference**: text → affordances; `VisionPerceptor`
  emitting `RawAxNode`s.
- **P1d — Vision stable-ids + hybrid fusion**: cross-frame id stability, AX+vision
  merge, confidence-weighted risk.
- **P1e — Actions**: route vision nodes to CGEvent pixel click/type/drag (reuse
  WP-F); verify-loop via re-OCR.

## 9. Open questions — delegated to the agents now
1. **Real Apple Vision OCR latency on this Mac** (`.fast` vs `.accurate`, window
   vs tile)? Is non-LLM sub-100 ms real? → **Codex (empirical benchmark).**
2. **Rust feasibility** of SCK window capture + calling Vision (objc2) + the
   dirty-tile loop? Crate maturity, fallbacks? → **Claude (feasibility + design).**

Their findings replace the assumptions in §2/§3 with measured numbers, then P1a is
GO/NO-GO.

## 10. Consolidated review (Codex §2/§3 + Claude §1/§5/§6/§7) — folded in

### The one number that decides everything — and it's contested
- **Codex**, measuring *directly in Swift, in-process, warm, on real Notes UI*
  (25–28 lines): full-window `.fast` **p95 16.5 ms**.
- **Claude**, citing the only *public* benchmark (`ocrmac`, a pyobjc wrapper, M3
  Max, full image): `.fast` **131 ms** — 8× higher.
- The gap is almost certainly **measurement method**: in-process Swift/objc2 vs a
  Python/pyobjc per-call wrapper, ± cold start, ± `usesLanguageCorrection`. Codex's
  figure is closer to the Rust + `objc2-vision` in-process path we'd ship — but
  **resolving this is P1a's #1 job**, measured in Rust, on UI-density text, warm,
  `.fast`, `usesLanguageCorrection=false`.
- **NO-GO trip-wire (Claude):** if `.fast` over a typical fovea region doesn't come
  in **under ~60 ms warm** on the target Mac, < 100 ms steady-state is out of reach
  with Apple Vision → fall back to RapidOCR/`ort` or rescope.

### Accepted corrections (fold into the phases)
1. **§1 plumbing-true, intelligence-false (A1).** Affordances/risk are AX-driven;
   vision nodes need **P1c: a role→`SemanticAction` table** (Button→[Click],
   TextField→[Type,Focus]) + parent/child by **geometric containment**. Emit
   canonical AX role strings (`"AXButton"`) so `visualops-graph` stays untouched.
2. **Tiles are a dirty-detector, never an OCR unit (A2).** OCR the **single union
   bbox of dirty tiles + a ½-line halo, in ONE request** — avoids straddle
   double-counting and the per-request fixed cost (why Codex's 9 *separate* tiles
   = 48 ms; one region is far less).
3. **Scroll is motion, not "all changed" (A2).** Estimate global Δy
   (phase-correlation / row probes); shift cached boxes by Δy, re-OCR only the
   newly-revealed band.
4. **Video/spinner/caret = no-OCR zones (A2).** Per-tile dirty-rate counter;
   permanently-dirty tiles get blacklisted (confidence≈0), not endlessly debounced.
5. **Fovea must NOT follow the OS cursor (A3).** Vision actions are CGEvent and
   **move the real cursor** (`platform/lib.rs:748`) → cursor-anchored fovea is a
   feedback loop + a no-foreground violation. Anchor the fovea to **last-action
   bbox + last dirty centroid** (internal state), never `CGEventGetLocation`.
6. **Stable-ids: bipartite matching, not a fuzzy hash (§5).** Match this frame's
   boxes to the previous by **IoU + text edit-distance**, with hysteresis to stop
   edge-flapping. `ax_identifier` is absent for *every* vision node → jitter is the
   default case, not an edge case.
7. **Risk monotone in uncertainty (§7).** Low OCR confidence must **raise** the
   risk floor, never lower the gate — else a misread destructive label ("Celete")
   slips under. Confidence-down ⇒ approval-up.
8. **The coordinate-transform chain is a first-class concern (A6 + Codex §2).**
   normalised(Vision, bottom-left) → pixel → point → global, with Y-flip + Retina
   scale + multi-DPI, on **both** perception and action paths (a 1000×660-pt window
   = 2000×1320 px). Build + unit-test this transform **first** — the #1 predicted
   bug source.
9. **Hot-path OCR config (Codex §3):** `.fast` + `usesLanguageCorrection=false`;
   `.accurate` only as a targeted second pass on an ambiguous tile / post-action
   verify.

### Revised P1a (GO/NO-GO) — what to build first
A Rust spike: `screencapturekit` (or `objc2-screen-capture-kit`) one-shot window
capture by `window_id` (= `Target.window_id` = `SCWindow.windowID`) →
`objc2-vision` `.fast` OCR over a fovea-sized region → time it (warm, p50/p95) on
UI-density text. Plus the coord-transform with unit tests.
**Gate:** region OCR < ~60 ms warm **and** capture < ~15 ms ⇒ GO to P1b; else
fall back to `ort`/RapidOCR or rescope. Whole-chain-in-Rust existence proof:
`andelf/picc`. Full feasibility + crate maturity: `docs/P1-vision-rust-feasibility.md`.
