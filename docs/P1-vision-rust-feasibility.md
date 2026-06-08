# P1 — Vision Surfaces: Rust Feasibility + Critical Review

> Companion to `docs/P1-vision-surfaces.md`. Scope: **research + design only** —
> no crates touched, no prod code, no git. Deliverable answers the two delegated
> open questions (§9.2: Rust feasibility + design) and challenges the plan.
>
> **Verdict: GO to build the P1a benchmark.** Every piece (SCK window capture,
> in-process Apple Vision OCR, CGEvent actions) exists and is proven in Rust. But
> the **< 100 ms steady-state claim is unproven and the §2/§3 OCR numbers are
> contradicted by the only public benchmark** (full-image `.fast` = 131 ms on an
> M3 Max). The latency target is the gating risk, not a given. Treat §2 as a
> hypothesis to falsify in P1a, not a budget to spend.

---

## PART A — Critical review of `P1-vision-surfaces.md` (concrete corrections)

Ordered by blast radius. Each item is a correction, not just a complaint.

### A1. §1 "vision = a second Perceptor, everything downstream reused unchanged" — *structurally true, semantically optimistic*

The plumbing claim holds: `Perceptor` is the only perception boundary, `Source`
already has `Vision`/`Ocr`, `confidence: f32` exists. A `VisionPerceptor`
returning `RawAxNode`s will flow through scene-graph build, diff, audit, MCP
unchanged. **That part is real and is the plan's best insight.**

But `RawAxNode` is **AX-shaped**, and three of its load-bearing fields have no
vision equivalent — so "reused unchanged" hides where the real work moves:

| `RawAxNode` field | AX source | Vision reality | Consequence |
|---|---|---|---|
| `ax_role: String` | `"AXButton"`… | nothing — must **synthesise** a fake AX role string from CV role inference | role-inference (§4) is now on the critical path, not optional |
| `ax_actions: Vec<String>` | `["press","showmenu"]` | **empty** — pixels expose no verbs | `affordance::map_action` yields *nothing* → vision nodes get **zero affordances** unless we add a role→action map that does not exist yet |
| `ax_identifier: Option<String>` | `"closeAll:"` | **none** | stable-id falls straight to the label+geometry fallback → the §5 jitter problem is not an edge case, it is the *default* for every vision node |
| `children: Vec<RawAxNode>` | real AX tree | OCR returns a **flat** list of boxes | parent/child must be synthesised by geometric containment before `build_scene_graph` sees a tree |

**Correction:** §1 should state plainly that the affordance layer and risk layer
are **AX-keyword/AX-action driven** and will emit near-empty graphs for vision
input until P1c lands a *role→SemanticAction* inference table (e.g. inferred
`Button` → `[Click]`, inferred `TextField` → `[Type, Focus]`). Check
`crates/visualops-graph/src/affordance.rs`: `derive_affordances` only produces
actions from (1) `map_action(ax_action)`, (2) `Role::TextField|TextArea`, (3)
`drag_targets_for` rows/cells. A synthesised vision node whose `ax_actions` is
empty and whose role lands as `Unknown` gets an **empty affordance set** — the
agent sees the element but cannot act on it. The "reuse" is in the data
structures; the **intelligence has to be rebuilt** on the vision side.

Also add to §1: the synthesised role must round-trip through the existing
`ax_role`→`Role` mapping. Either emit canonical AX strings (`"AXButton"`) the
existing mapper already knows, or extend the mapper. Emitting AX-look-alike
strings is the lower-touch choice and keeps `visualops-graph` untouched.

### A2. §6 tiling — three pitfalls under-weighted; one mis-designed

1. **Text straddling a tile boundary** (called out in the prompt, absent from the
   plan). A 256×256 grid will cut words/lines. Per-tile OCR then either
   double-counts (a line OCR'd in two tiles) or clips it. **Correction:** do not
   OCR tiles. Use tiles **only as a dirty-detector** (hash per tile), then OCR
   the **single union bounding-box of all dirty tiles in one request**, snapped
   out to a margin (a "halo" of ~½ line-height). Tiling decides *where changed*;
   OCR runs on a region, not a tile. This also fixes A3.

2. **Scroll salts every tile.** A 1-px scroll shifts all content → every tile
   hash changes → full re-OCR → budget blown on the most common interaction.
   The plan never mentions it. **Correction:** before hashing, run a cheap
   global-translation estimate (phase correlation / a few row-shift probes) on
   the frame delta. If the frame is a pure vertical translation by Δy, **shift
   the cached OCR boxes by Δy and re-OCR only the newly-revealed band** at the
   edge, not the whole window. Treat scroll as motion, not as "everything
   changed."

3. **Permanently-dirty regions (video / spinner / blinking caret / cursor
   blink).** §6 says "coalesce / debounce → OCR on settle". A playing video tile
   *never settles* — debounce alone loops forever or starves. **Correction:** add
   a per-tile **dirty-rate** counter; a tile dirty for N consecutive settle
   windows is **blacklisted** (marked `source = Vision, confidence ≈ 0`, no OCR)
   until its rate drops. Video ≠ pending OCR; it is a no-OCR zone.

### A3. §6 foveal + §1 actions — the foveal-around-cursor idea fights our own action path

§6 puts the fovea "around the cursor / last action". But §1's vision action path
is **CGEvent**, and the platform code is explicit that CGEvent mouse input
**moves the real cursor** (`crates/visualops-platform/src/lib.rs:748`: *"Synthetic
drag moves the real cursor; this is inherent to CGEvent mouse input."*). So:

- Acting on a vision surface **warps the user's cursor**, which then **defines the
  fovea**, which biases the next perception toward where *we* just clicked — a
  feedback loop, and a UX violation of the project's no-foreground/background
  contract (cf. the `cua-driver` "no-foreground" memory). AX path uses
  `AXPerformAction` and does **not** move the cursor; the vision path is strictly
  more intrusive.

**Corrections:**
- Decouple the fovea from the OS cursor. Anchor it to **last-action bbox** and
  **last dirty centroid**, not `CGEventGetLocation`. Internal state, not the
  shared pointer.
- Flag in the plan that vision actions are **foreground-affecting** and must be
  gated accordingly (raise risk a notch for pixel-CGEvent actions vs AX). The
  risk engine currently keys on labels only; add an axis for *action mechanism*.
- Investigate `CGEventPost` save/restore of cursor position, or `CGWarpMouseCursorPosition`
  round-trip, as partial mitigation — but document that it is never as clean as AX.

### A4. §5 stable IDs — right instinct, fragile key

The proposed key `(coarse grid cell + normalised text + role)` has two faults:

- **Grid-cell flapping**: a box near a cell boundary hops cells on ±1 px jitter →
  its key changes → id churns. Same failure mode as A2.1 (boundaries are evil).
- **Role in the key**: inferred role is itself noisy frame-to-frame (CV
  confidence wobble). Keying identity on a noisy attribute makes identity noisy.

**Correction — make matching, not hashing, the primary mechanism** (this is the
WP-D spirit applied spatially: derive identity from the *most stable* signal, not
the volatile one — cf. the `graph-stableids-ns-policy` memory where volatile
`_NS:<n>` ids are deliberately excluded):

```
carry_ids(prev_nodes, curr_nodes):
  # 1. cheap coarse bucketing only to prune candidate pairs (accel, not identity)
  # 2. score every plausible (prev, curr) pair:
  score(p, c) = w_iou * IoU(p.bbox, c.bbox)
              + w_txt * (1 - normalised_edit_distance(norm(p.text), norm(c.text)))
              + w_role * (p.role == c.role ? 1 : 0)      # tiebreaker ONLY
  # 3. greedy / Hungarian max-weight bipartite match above a threshold τ
  # 4. matched curr inherits prev.id ; unmatched curr mints a fresh synth_id
  # 5. hysteresis: a node must be unmatched for K frames before its id is retired
```

- `norm(text)` = lowercase, accent-fold, collapse whitespace, strip a small edit
  budget (the "±1 char" jitter). Reuse the same normalisation `visualops-graph`
  already does for risk keywords.
- `IoU` tolerates ±px jitter far better than grid membership.
- Role is a **tiebreaker weight**, never part of the key.
- Hysteresis (step 5) is what actually stops churn breaking diff/audit/approval —
  the plan names the symptom but not this cure.

This is **tenable**, but only as *matching with hysteresis*, not as *fuzzy
hashing*. Budget ~O(n·k) with coarse bucketing (k = candidates/bucket); n is tens
of boxes per fovea, so cost is microseconds — fits §2 trivially.

### A5. §7 hybrid fusion — under-specified in the two places that bite

"Fuse by spatial overlap; AX wins ties" omits the two real problems:

1. **Coordinate spaces don't match.** AX frames are **points, top-left-origin,
   global**. SCK captures **pixels, backing-scaled** (2× Retina). Vision returns
   **normalised [0,1], bottom-left-origin**. Overlapping an AX bbox with a vision
   bbox requires a full transform chain (see A6). The plan compares boxes as if
   they were in one space. They are not. **Correction:** define one canonical
   space (global points, top-left) and convert everything into it *before* fusion;
   make that conversion a single audited function with tests.

2. **Risk inversion on low confidence.** "Confidence-weighted risk" + "AX wins
   ties" is fine, but a *misread* low-confidence vision label is a **safety
   hole**: OCR reads "Celete" instead of "Delete" → risk engine misses the
   destructive keyword → a high-risk button is gated as low. **Correction:** risk
   must be **monotonic in uncertainty** — low confidence ⇒ *raise* the risk floor,
   never lower it. Ambiguous vision nodes default to "needs approval", not
   "probably safe". Add this as an invariant (candidate `CONTRACTS.md` entry).

Also specify the gap rule explicitly: a vision node is kept **iff** no AX node has
`IoU > θ` with it (vision fills holes; it never shadows AX). State θ and make it
configurable.

### A6. Cross-cutting gap absent from the whole plan: the coordinate transform

Nowhere does the plan handle pixel↔point↔normalised conversion, yet it is on the
path for *both* perception (OCR box → graph bbox) and action (graph bbox →
CGEvent point). The chain for an OCR observation → clickable screen point:

```
Vision normalised (x,y,w,h), bottom-left, [0,1]
  → × image pixel size            (un-normalise)
  → flip Y                        (Vision bottom-left → top-left)
  → ÷ backing scale factor        (Retina 2× → points)
  → + window origin (points)      (window-local → global)
  → global screen point for CGEvent
```

Five steps, three origin/scale conventions, one of them per-display (mixed-DPI
multi-monitor changes the scale mid-stream). This is the single richest bug
source in the whole feature and deserves its own section + golden tests. **Add it
to the plan as a first-class component**, owned alongside capture.

### A7. §2 budget — optimistic by ~2–3× against the only public number

§2 budgets OCR at 20–55 ms and §3 guesses "`.fast` ~10–50 ms on a window region".
The only public benchmark (ocrmac, **M3 Max**, full image, includes PyObjC
overhead): `.fast` **131 ms**, `.accurate` 207 ms, livetext 174 ms. In-process
Rust on a *small* region will be much less, but there is a **fixed per-request
cost** (`VNImageRequestHandler` setup + ANE dispatch) that does not shrink with
area — which is exactly why A2.1's "one request on the union region" matters and
why "one request per tile" would be fatal. **Correction:** mark §2 as
*unvalidated*; P1a must measure (a) fixed per-request floor, (b) ms/kpx slope,
(c) `.fast` vs `.accurate` on a *region*, on *this* Mac. Until then the < 100 ms
claim is a hypothesis. The plan's instinct to gate P1a on this (§8) is correct;
the table's numbers should not be quoted as if measured.

### A8. Smaller notes
- **Permissions:** SCK needs the **Screen Recording** TCC grant — *separate* from
  the Accessibility grant the AX path already requires. Two prompts, two failure
  modes. Add to onboarding/doctor.
- **Trait shape mismatch:** `Perceptor::capture(&self) -> Result<Vec<RawAxNode>>`
  is **synchronous**; SCK is **push** (delegate callbacks deliver frames on a
  dispatch queue). Resolve explicitly (see B3) — keep a running stream + a
  latest-frame cache, or one-shot `SCScreenshotManager` per call. The plan never
  states which; it changes the whole engine loop.
- **`.fast` accuracy:** `.fast` trades accuracy for speed and is worse on small
  UI glyphs / mixed scripts; combined with A5.2 (misread → risk inversion) this
  argues for `.accurate` on the *risk-bearing* fovea even if `.fast` elsewhere.

---

## PART B — Feasibility findings (the delegated §9.2)

### B1. CAPTURE — ScreenCaptureKit, window-scoped, from Rust → **feasible, mature**

- **`screencapturekit`** (doom-fish/svtlabs) — safe high-level bindings, builder
  API, **v6** line, actively maintained (crates.io updated 2026), macOS 12.3+,
  zero-copy frame delivery via **IOSurface/Metal**. Captures screen / **window** /
  app. ([crates.io](https://crates.io/crates/screencapturekit),
  [GitHub](https://github.com/doom-fish/screencapturekit-rs),
  [docs](https://doom-fish.github.io/screencapturekit-rs/screencapturekit/))
- **`objc2-screen-capture-kit`** (madsmtm/objc2 family) — lower-level, complete
  bindings incl. the `SCStreamOutput` trait (sample buffers backed by IOSurface)
  and `SCScreenshotManager`. Same objc2 type universe as `objc2-vision` →
  **zero-copy CMSampleBuffer→CVPixelBuffer handoff to OCR**.
  ([docs.rs](https://docs.rs/objc2-screen-capture-kit/latest/objc2_screen_capture_kit/),
  [SCStreamOutput](https://docs.rs/objc2-screen-capture-kit/latest/objc2_screen_capture_kit/trait.SCStreamOutput.html))

**Window by `window_id`:** yes. `getShareableContent` (called **once** at setup,
not per frame) lists `SCWindow`s; each carries `.windowID` = the CGWindowID we
already have in `Target.window_id`. Filter by it, build an `SCContentFilter` for
that one window, capture only it.
([SCWindow.windowID](https://developer.apple.com/documentation/screencapturekit/scwindow/windowid))

**One-shot vs stream.** `SCScreenshotManager.captureImage` (macOS 14+) replaces
the deprecated `CGWindowListCreateImage` and gives a single CGImage/CMSampleBuffer
on demand — clean fit for the synchronous `Perceptor` trait, but pays setup each
call. A persistent `SCStream` + latest-frame cache is the steady-state choice;
the community guidance is explicit that `getShareableContent`/stream setup is
once-at-init, not per-frame.
([SCScreenshotManager](https://developer.apple.com/documentation/screencapturekit/scscreenshotmanager),
[Nonstrict analysis](https://nonstrict.eu/blog/2023/a-look-at-screencapturekit-on-macos-sonoma/))

**Fallbacks:** `CGWindowListCreateImage` (deprecated macOS 14+ but functional,
simplest for a PoC), `CGDisplayStream` (deprecated). Both via `core-graphics`,
already a platform dep. Use SCK as primary; CG as the "it works today on older
macOS / no SCK perms" escape hatch.

**Expected per-frame latency:** no clean public number (Apple's own forum thread
on capture→callback latency is unresolved due to a clock-domain bug —
[thread/785046](https://developer.apple.com/forums/thread/785046)). Practically,
a *steady* SCK stream delivers single-window frames in the **single-digit-ms**
range (GPU/IOSurface, zero-copy); the cost that bites is **first-frame / stream
warm-up** (tens of ms, one-time), which is why a persistent stream beats repeated
one-shots. Plan's 5–15 ms for steady-state window capture is **plausible**; the
cold-start caveat (§2) is correct.

### B2. OCR — Apple Vision from Rust → **feasible in-process, no IPC needed**

- **`objc2-vision`** — Rust bindings to the Vision framework (madsmtm/objc2
  family): `VNRecognizeTextRequest`, `VNImageRequestHandler`,
  `VNRecognizedTextObservation`, recognition-level enum, CVPixelBuffer inputs.
  ([crates.io](https://crates.io/crates/objc2-vision),
  [docs.rs](https://docs.rs/objc2-vision/),
  [lib.rs](https://lib.rs/crates/objc2-vision))
- **Existence proof:** `andelf/picc` — a real Rust macOS toolkit doing
  **screenshot + Vision OCR + AX**, built on CoreGraphics/Vision/AppKit/AX **via
  objc2**. Proves the SCK/CG + Vision + AX trio coexists in one Rust process.
  ([GitHub](https://github.com/andelf/picc))

**Verdict: stay in-process via objc2-vision.** A Swift helper over stdio/IPC is
**not** needed and would *add* latency (serialize image or share IOSurface across
a process boundary + round-trip ~ms–tens of ms + a second binary to ship/sign).
The only reason to reach for a helper is if a specific newer Vision API is missing
from objc2-vision — not the case for `VNRecognizeTextRequest`. **No IPC.**

**Crates that *do* OCR for you?** Python (`ocrmac`, `macos-vision-ocr`) and CLI
wrappers exist, but for Rust the answer is "bind Vision yourself via objc2-vision"
— there is no batteries-included Rust `vision-ocr` crate; objc2-vision is the
substrate and it is enough.

**Measured latency (the number that matters):** ocrmac on **M3 Max**, **full
image**, incl. PyObjC overhead — `.fast` **131 ms ± 0.7**, `.accurate` 207 ms,
livetext 174 ms. ([ocrmac benchmark](https://github.com/straussmaximilian/ocrmac),
[recognition levels](https://developer.apple.com/documentation/vision/vnrequesttextrecognitionlevel/fast)).
These are **full-frame** and Python-laden; a small in-process region will be far
less, but a **fixed per-request floor** remains → OCR the union region once
(A2.1), never per tile. **This single benchmark is why §2 is flagged unproven.**

### B3. THE LOOP — tile + dirty-region + foveal, concretely

Design that respects A2/A3/A7 and the synchronous `Perceptor` trait:

```
Init (once):
  getShareableContent → pick SCWindow by Target.window_id
  build SCContentFilter(window); start persistent SCStream(.fast config)
  SCStreamOutput callback writes the latest CVPixelBuffer into a Mutex<LatestFrame>
  grid = window / TILE (256²); tile_hash[] = None

VisionPerceptor::capture():            # synchronous: pulls latest cached frame
  frame  = latest_frame.lock()         # zero-copy CVPixelBuffer (IOSurface)
  # 1. dirty detect (cheap, on CPU-mapped or downscaled luma)
  for each tile: h = ahash/xxhash(tile_pixels)
                 dirty[tile] = (h != tile_hash[tile]); tile_hash[tile] = h
  # 2. scroll guard (A2.2): if delta ≈ pure translation Δ, shift cached boxes by Δ,
  #    mark only the newly-revealed band dirty
  # 3. permanently-dirty guard (A2.3): tiles dirty K windows running → blacklist
  # 4. fovea = bbox(last_action) ∪ centroid(dirty)      # NOT the OS cursor (A3)
  # 5. region = snap_out( union(dirty_tiles ∩ fovea), halo=½ line-height )   # A2.1
  # 6. crop CVPixelBuffer → region ; ONE VNRecognizeTextRequest (.accurate on
  #    risk fovea, .fast elsewhere — A8) ; get [text, bbox(norm), confidence]
  # 7. transform each box: normalised→global points (A6 chain)
  # 8. role-infer (P1c) → synth RawAxNode{ ax_role, label=text, frame, source,
  #    confidence } ; geometric containment → children
  # 9. carry_ids(prev, curr) with IoU+text matching + hysteresis (A4)
  return roots                          # everything downstream unchanged (§1)
```

**Holding the §2 budget** comes from exactly two levers, both now explicit:
1. **OCR area → tiny**: union-of-dirty ∩ fovea, one request (not N tiles, not the
   window). Static chrome is hashed (µs) and skipped.
2. **OCR frequency → low**: coalesce on settle; scroll shifts cached boxes instead
   of re-OCRing; video tiles blacklisted. The worst case (everything dirty) is the
   *cold frame* §2 already concedes will exceed 100 ms.

If P1a's measured per-request floor + region slope can't fit a typical fovea under
~60 ms, the steady-state target fails — that is the **NO-GO trip-wire**.

### B4. VISION STABLE IDs — algorithm (the §5 / WP-D-spirit sketch)

Full algorithm is A4 above. Summary of the four moving parts:

1. **Coarse bucket** (acceleration only): hash boxes into a loose spatial grid to
   prune candidate pairs — *not* an identity key.
2. **Pairwise score**: `w_iou·IoU + w_txt·(1−editdist(norm_text)) + w_role·(role==)`;
   role is a tiebreaker, never a key (A4).
3. **Max-weight bipartite match** (greedy is fine at this n) above threshold τ;
   matched `curr` inherits `prev.id`; unmatched mints a fresh `synth_id` via the
   existing WP-D synthesiser (which, with no `ax_identifier`, falls to
   role+label+geometry — exactly the case the `graph-stableids-ns-policy` memory
   describes for volatile ids).
4. **Hysteresis**: retire an id only after K consecutive unmatched frames — this
   is what keeps diff/audit/approval stable under jitter, and it's the piece §5
   omits.

Reuses WP-D's normalisation and "identity from stable signal" philosophy; extends
it from attribute-space to spatial+text-space. Cost is microseconds (tens of boxes
per fovea) → fits §2.

### B5. Recommended crate stack

| Concern | Crate | Maturity | Note |
|---|---|---|---|
| Window capture (steady) | `objc2-screen-capture-kit` | **High** — madsmtm/objc2, tracks SDK | low-level but **same objc2 types as Vision → zero-copy** to OCR; preferred for the hot loop |
| Window capture (ergonomic / PoC) | `screencapturekit` (v6) | **High** — active, v6, builder API | faster to prototype; verify its frame type hands a CVPixelBuffer to objc2-vision without a copy, else use the low-level crate |
| One-shot grab / fallback | `core-graphics` (0.24, **already a dep**) | High | `CGWindowListCreateImage` PoC escape hatch; `SCScreenshotManager` via objc2 for macOS 14+ |
| OCR | `objc2-vision` | **High** — objc2 family | `VNRecognizeTextRequest`; in-process; **no IPC** |
| Hashing (tiles) | `xxhash-rust` or `ahash` | High | per-tile dirty detect; ahash is already idiomatic |
| objc2 base / blocks | `objc2`, `objc2-foundation`, `block2` | **High** — the ecosystem standard | needed to bridge SCStream delegate + Vision completion handler into Rust (channel/semaphore) |
| Actions | **none new** | — | reuse WP-F CGEvent path (`visualops-platform/src/lib.rs`) for `source != Accessibility` |

Pin all objc2-family crates to one compatible release set (they version together).
Verify exact current versions at build time — the family moves fast.

### B6. Pipeline sequence (steady-state dirty refresh)

```mermaid
sequenceDiagram
    autonumber
    participant Eng as Engine (sync)
    participant VP as VisionPerceptor
    participant SCK as SCStream (objc2-sck)
    participant Cache as LatestFrame (Mutex)
    participant V as Vision (objc2-vision, ANE)
    participant G as scene/affordance (visualops-graph, unchanged)

    Note over SCK,Cache: persistent stream, set up once; callback caches frames
    SCK-->>Cache: CVPixelBuffer (IOSurface, zero-copy)  ~single-digit ms
    Eng->>VP: capture()
    VP->>Cache: lock latest frame
    VP->>VP: tile hash + dirty diff (ahash)            1–5 ms
    VP->>VP: scroll guard / blacklist / fovea select   <1 ms
    alt dirty region non-empty
        VP->>V: ONE VNRecognizeTextRequest(region, .fast/.accurate)
        V-->>VP: [text, bbox(norm), conf]              region-dependent (PoC gate)
        VP->>VP: coord transform norm→global points    <1 ms
        VP->>VP: role infer + containment children      5–15 ms
        VP->>VP: carry_ids (IoU+text match, hysteresis) µs
    else nothing changed
        VP-->>Eng: cached RawAxNodes (no OCR)           ~0 ms
    end
    VP-->>Eng: Vec<RawAxNode> (source=Vision, conf<1)
    Eng->>G: build_scene_graph → affordances → diff     <5 ms (measured ~21µs core)
    Note over Eng,G: downstream identical to AX path (§1)
```

### B7. Latency estimate per stage (plan vs reality)

| Stage | §2 claim | Evidence | Realistic (this design) | Confidence |
|---|---|---|---|---|
| Window capture (steady) | 5–15 ms | SCK zero-copy IOSurface; no clean public ms | **3–12 ms** | Med |
| Window capture (cold/warm-up) | (one-time) | stream warm-up is the known cost | **tens of ms, once** | Med |
| Tile hash + dirty diff | 1–5 ms | ahash/xxhash on luma/downscale | **1–5 ms** | High |
| Scroll guard / blacklist / fovea | — (absent) | new, cheap | **< 2 ms** | Med |
| **OCR on dirty region (1 request)** | **20–55 ms** | ocrmac M3 Max full-img `.fast` **131 ms** → region ≪ that, but **fixed floor unknown** | **unproven — 20–80 ms; gates the whole claim** | **Low** |
| Coord transform | — (absent) | trivial arithmetic | **< 1 ms** | High |
| Role infer + children | 5–15 ms | classical CV / heuristics | **5–20 ms** | Low-Med |
| Build nodes + graph | < 5 ms | core measured ~21 µs | **< 5 ms** | High |
| Stable-id carry | (in build) | tens of boxes, IoU+edit | **< 1 ms** | High |
| **Steady-state total** | **~40–95 ms** | — | **~35–115 ms; sits *on* the line** | **Low** |

The honest read: the design is **on the boundary**, dominated by one unmeasured
term (region OCR). P1a must pin that term first; everything else fits.

### B8. Major risks

| # | Risk | Severity | Mitigation |
|---|---|---|---|
| R1 | Region-OCR floor (per-request fixed cost) too high → steady-state > 100 ms | **High** | P1a benchmark first; union-region-one-request (A2.1); `.fast` off-fovea; if floor too high, NO-GO or accept >100 ms |
| R2 | Scroll re-OCRs whole window | **High** | translation guard, shift cached boxes (A2.2) |
| R3 | CGEvent vision actions move the real cursor / break no-foreground | **High** | decouple fovea from cursor (A3); raise risk for pixel actions; cursor save/restore |
| R4 | Coordinate transform bugs (pixel/point/normalised, Retina, multi-DPI) | **High** | single audited transform fn + golden tests (A6) |
| R5 | OCR misread inverts risk (low-conf "Delete"→safe) | **High (safety)** | risk monotonic in uncertainty: low conf ⇒ raise risk floor (A5.2) |
| R6 | Affordance/risk layers emit empty graphs for vision (AX-action driven) | Med | role→action inference table in P1c (A1) |
| R7 | Permanently-dirty tiles (video) starve the loop | Med | dirty-rate blacklist (A2.3) |
| R8 | Sync `Perceptor` trait vs push SCStream | Med | persistent stream + latest-frame cache (B3/A8) |
| R9 | Screen-Recording TCC grant (separate from AX) | Low-Med | onboarding + doctor check (A8) |
| R10 | objc2 ↔ high-level crate type seam forces a frame copy | Low-Med | prefer objc2-sck+objc2-vision for zero-copy (B5) |
| R11 | `.fast` accuracy on small UI glyphs | Low-Med | `.accurate` on risk fovea (A8) |

### B9. GO / NO-GO

**GO — build P1a** (SCK window grab + objc2-vision region OCR + the measurement
harness). Rationale: all four capabilities (window capture, in-process OCR,
synthesised `RawAxNode`s, CGEvent actions) are **proven to exist in Rust today**;
the architecture reuses the existing engine cleanly; the open risks are
quantifiable, not unknown-unknowns.

**The < 100 ms steady-state claim is NOT yet GO.** It rests on one unmeasured
number (region-OCR floor) and a budget that the only public benchmark contradicts
by ~2–3× at full frame. P1a's job is precisely to turn that Low-confidence cell in
B7 into a measured one. **NO-GO trip-wire:** if region OCR on a typical fovea
(~½ window, `.fast`) cannot land under ~60 ms on the target Mac *after* warm-up,
the < 100 ms steady-state goal is not reachable with Apple Vision and the team
must either relax the target, restrict vision to non-real-time verification, or
evaluate a CoreML mobile OCR (RapidOCR/PP-OCRv4) with its own benchmark.

---

## Sources

- [screencapturekit — crates.io](https://crates.io/crates/screencapturekit)
- [doom-fish/screencapturekit-rs — GitHub](https://github.com/doom-fish/screencapturekit-rs)
- [screencapturekit — rustdoc](https://doom-fish.github.io/screencapturekit-rs/screencapturekit/)
- [objc2-screen-capture-kit — docs.rs](https://docs.rs/objc2-screen-capture-kit/latest/objc2_screen_capture_kit/)
- [SCStreamOutput — docs.rs](https://docs.rs/objc2-screen-capture-kit/latest/objc2_screen_capture_kit/trait.SCStreamOutput.html)
- [objc2-vision — crates.io](https://crates.io/crates/objc2-vision) · [docs.rs](https://docs.rs/objc2-vision/) · [lib.rs](https://lib.rs/crates/objc2-vision)
- [madsmtm/objc2 — GitHub](https://github.com/madsmtm/objc2)
- [andelf/picc — Rust objc2 screenshot+OCR+AX](https://github.com/andelf/picc)
- [ocrmac benchmark (M3 Max fast/accurate/livetext) — GitHub](https://github.com/straussmaximilian/ocrmac) · [PyPI](https://pypi.org/project/ocrmac/)
- [VNRecognizeTextRequest — Apple](https://developer.apple.com/documentation/vision/vnrecognizetextrequest) · [recognitionLevel](https://developer.apple.com/documentation/vision/vnrecognizetextrequest/recognitionlevel) · [.fast](https://developer.apple.com/documentation/vision/vnrequesttextrecognitionlevel/fast)
- [SCScreenshotManager — Apple](https://developer.apple.com/documentation/screencapturekit/scscreenshotmanager) · [SCWindow.windowID](https://developer.apple.com/documentation/screencapturekit/scwindow/windowid)
- [ScreenCaptureKit capture→callback latency thread — Apple Forums](https://developer.apple.com/forums/thread/785046)
- [A look at ScreenCaptureKit on Sonoma — Nonstrict](https://nonstrict.eu/blog/2023/a-look-at-screencapturekit-on-macos-sonoma/)
</content>
</invoke>
