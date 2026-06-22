# Dunst — POC

[![CI](https://github.com/azerozero/dunst/actions/workflows/ci.yml/badge.svg)](https://github.com/azerozero/dunst/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

> MCP-first macOS UI automation: choose the fastest trustworthy read path,
> then execute only verified, risk-gated actions.

A macOS daemon that turns a window into a **verifiable affordance graph** for AI
agents. Instead of `Click(x=842, y=661)`, an agent resolves a target by meaning —
a **scene-graph node** (system truth) and its **affordance** (semantic actions +
risk), two distinct objects keyed by the same stable id:

```json
// get_scene_graph (compact projection) — one node
{ "id": "btn_nouvelle_note", "role": "button", "label": "Nouvelle note",
  "bbox": { "x": 1815, "y": 391, "w": 45, "h": 52 },
  "enabled": true, "focused": false, "parent": "toolbar_692558b0", "n_children": 0 }

// get_hit_targets — the agent-facing target for that same id
{ "id": "btn_nouvelle_note", "label": "Nouvelle note", "role": "button",
  "safe_click": { "center": [1837.5, 417.0], "source": "accessibility_bbox_inset" },
  "action_modes": [{ "action": "click", "tool_hint": "click_element" }],
  "risk": { "level": "low", "requires_approval": false, "reasons": [] } }
```

(The node carries `confidence`/`source` in the `full` view; the compact projection
drops them. `risk` is the structured `RiskAssessment`, not a bare string.)

## How Dunst Reads And Acts

Dunst is used from an MCP client first. The engine starts by confirming the
target window, then walks the cheapest reliable information path before falling
back to slower or riskier surfaces. Raw screen input is the last resort, not the
default.

```mermaid
flowchart TD
    Client[MCP client] --> Call[tools/call]
    Call --> Target[Attach and verify target<br/>list_windows, attach, target_visibility]
    Target --> AXFast[Fast AX reads<br/>window_view, page_state, text_snapshot]
    AXFast --> AXQuery[Semantic targets<br/>get_hit_targets scope=page, find_element]
    AXQuery --> Targeted[Targeted probes<br/>analyze_region_ax, visual_change_probe]
    Targeted --> OCR[OCR fallback<br/>read_text_detailed, find_ocr_text, extract_ocr_cards]
    OCR --> Pixels[Pixel and chart fallback<br/>read_shapes, scan_chart, read_at]
    Pixels --> Action[Element-bound or OCR-bound action<br/>click_element, type_into, click_near_text]
    Action --> Gate[Risk gate and approval policy]
    Gate --> Verify[Refresh, diff, expected_text, audit]
```

Information ladder, from fastest and most practical to slowest:

1. **Target scope**: `list_windows`, `attach`, `target_visibility`,
   `expose_target_window`, and `list_browser_tabs` make sure Dunst is reading the
   intended window, not a covered or stale browser tab.
2. **AX semantic targets**: `get_hit_targets(scope=page)`, `window_view`,
   `page_state`, `text_snapshot`, and `find_element` are the preferred path for
   native apps and accessible web UI. `get_hit_targets` returns labels, roles,
   safe click zones, action modes, risk, selected tab, visibility, and a
   `ui_epoch` fingerprint so stale coordinates can be discarded after a move,
   resize, tab switch, or user interaction.
3. **Targeted probes**: `analyze_region_ax` and `visual_change_probe` inspect one
   region when a full AX refresh is too broad or not moving.
4. **OCR fallback**: `read_text_detailed(content_only=true)`, `find_ocr_text`,
   `extract_ocr_cards`, `detect_modal`, and `dismiss_modal` handle web canvases,
   cards, popups, and pages that expose only a root group.
5. **Pixel/chart fallback**: `read_shapes`, `scan_chart`, `read_at`, and
   `read_series` are for custom-drawn UI, charts, and hover surfaces.
6. **Raw input fallback**: `click_at`, `press_key`, `hotkey`, and `type_keys` are
   gated, audited, and should follow a visibility check plus a postcondition.

## Why The AX Slice Still Matters

The full vision path (Tile/Foveal/OCR/ScreenCaptureKit, drag and drop, replay)
is large. This POC proves the load-bearing hypothesis: the macOS Accessibility
tree is rich enough to build the first affordance graph without pixels or OCR.

Validated on Notes (pure AX, no screenshot): 427 elements, each actionable one
already carrying `role`, native `actions`, `label`/`help`, an identifier, and
risk signals in the label text (`Supprimer`, `Éteindre`, ...). Vision and OCR
are now fallbacks for non-AX surfaces, not the entrypoint.

## Workspace

| Crate                | Role                                                     |
|----------------------|----------------------------------------------------------|
| `dunst-core`     | Frozen contract: types, traits, `MockPerceptor`, fixtures|
| `dunst-graph`    | Pure logic: scene graph, affordances, risk, diff         |
| `dunst-platform` | macOS AX backend: `Perceptor` + `ActionExecutor`         |
| `dunst-vision`   | P1a spike: window capture + Apple Vision OCR + coord math |
| `dunst-mcp`      | Engine (risk gating + audit) + demo + MCP server         |

`graph` and `platform` depend only on `core`. See `docs/README.md` for the
documentation map and `docs/ARCHITECTURE.md` for the current architecture.

**Cross-platform compilation:** only `dunst-vision::coords` (pure coordinate
math) builds on any target; the rest of `dunst-vision` (capture, OCR) and all
of `dunst-platform` are `#[cfg(target_os = "macos")]`. So `cargo test` runs the
full logic/coords suite everywhere, and the macOS-only backends compile on macOS.

## Prerequisites

- macOS for live AX automation. Fixture/demo mode works without a live target.
- Rust 1.85, matching the workspace `rust-version`.
- Accessibility permission for the terminal or MCP host when using live mode.
- Screen Recording permission when using screenshot/OCR tools.

Check the local environment:

```bash
cargo run -p dunst-mcp -- doctor
```

## Run

```bash
# Device-free demo on the Notes fixture: scene -> affordance -> risk gating -> audit
cargo run -p dunst-mcp -- demo

# Build the MCP server used by Codex/Claude stdio clients
cargo build -p dunst-mcp

# Dump a live window's AX tree as JSON (find the pid/window via the MCP host)
cargo run -p dunst-platform --example dump -- <pid> <window_id>
```

The fixture demo prints a scene summary, resolves `Nouvelle note`, executes the
low-risk click, gates a destructive `Supprimer` action as `PendingApproval`, then
exports the audit trail as JSON.

Expected shape:

```text
# Dunst MCP demo — Notes (fixture, AX-only)
scene graph: 427 nodes, 1 root(s), window "Notes"
-> result=Success
-> result=PendingApproval
```

Exit-code expectations:

| Command | Success | Failure |
|---------|---------|---------|
| `dunst-mcp demo` | `0` when the fixture loads and the scripted path completes | `1` on fixture or engine initialisation failure |
| `dunst-mcp serve` | `0` when the stdio loop exits normally | `1` when an explicitly requested live target cannot be resolved |
| `dunst-mcp doctor` | `0` when the local environment is usable for live automation | `1` when Accessibility, Screen Recording, config, or platform checks fail |
| `dunst-mcp setup` | `0` after dry-run/edit output or successful apply/migrate | `1` on invalid config merge/write or clap exits non-zero for invalid arguments |

## MCP client setup

The MCP server binary is `dunst-mcp`; the server identifies itself as `dunst`.
For local clients, use `scripts/mcp-dunst.sh` as the stdio entrypoint. The
wrapper builds `target/debug/dunst-mcp` if needed, keeps stdout clean for
JSON-RPC, then starts `dunst-mcp serve --live`.

Inspect or write config snippets:

```bash
cargo run -p dunst-mcp -- setup --client codex --dry-run
cargo run -p dunst-mcp -- setup --client claude --dry-run --dev-wrapper
cargo run -p dunst-mcp -- setup --client codex --apply
cargo run -p dunst-mcp -- setup --client claude --migrate
```

Codex can load the project-local registration in `.codex/config.toml` after a
restart. Claude-style clients can use `.mcp.json`. The project-local configs use
the relative `scripts/mcp-dunst.sh` wrapper. Installed user-level configs should
prefer `dunst-mcp serve` from `PATH`.

The Codex config uses `startup_timeout_sec = 120` because the development
wrapper may need to build `target/debug/dunst-mcp` before the MCP handshake.
Installed configs that call a prebuilt `dunst-mcp` binary can use a shorter
timeout.

Use `setup --edit` to print the current file and merged result without writing,
and `setup --config PATH` for tests or non-standard client paths. A compact
device-free MCP transcript lives at `docs/fixtures/mcp-transcript.jsonl`; keep it
updated when tool names or core response shapes change.

To pin startup to an app:

```bash
DUNST_MCP_APP="Google Chrome" scripts/mcp-dunst.sh
```

App lifecycle tools:

- `list_apps` lists GUI apps that are already running.
- `list_launchable_apps` scans installed `.app` bundles without launching them.
- `app_info` reads one app's `Info.plist` metadata by name, bundle id, or path.
- `launch_app` starts an app in the background, optionally with a URL and args.
- `close_app` asks an app to quit cleanly by name.

Display/window view tools:

- `list_displays` lists active screens with Dunst's 1-based index, global bounds,
  pixel resolution, scale, and main-display flag.
- `window_view` returns a compact scoped view of the target window: owning
  display, window bounds, position relative to that display, visible text, and
  key elements without dumping the full AX graph.
- `desktop_view` returns the display/window topology with front/back `z_order`,
  frontmost window, owning display, and geometric overlap lists. If CoreGraphics
  cannot provide a real display topology, it returns `degraded:true` with a
  `reason` instead of fabricating a `0x0` display.
- `visual_change_probe` captures a screen region, samples a spaced luminance grid,
  compares it with the previous probe, and can run a full AX refresh when pixels
  changed. AX cannot refresh only one rectangle; the pixel probe is the cheap
  invalidation signal.
- `analyze_region_ax` samples a screen region with AX hit-tests and returns the
  unique shallow AX elements under that grid. macOS does not expose a direct
  subtree-by-rectangle refresh, but this is targeted AX analysis for one zone.
- `move_window_to_display` moves the target window to a display index from
  `list_displays`, centering it and preserving size by default.
- `move_app_to_display` moves all sizeable top-level windows for a running app
  to a display index from `list_displays`.
- `arrange_windows` tiles selected windows on a display as `grid`, `columns`,
  `rows`, `cascade`, or `maximize`; selection must be explicit through
  `window_ids`, `app`, or `all:true`.
- `target_visibility` reports whether the attached target is frontmost,
  visible, covered, fully covered, or missing from the desktop stack.
- `get_hit_targets` returns semantic click/type/drag targets with safe inset
  click zones, action modes, risk, selected browser tab, target visibility, and
  a `ui_epoch` fingerprint. Pass `previous_epoch` to detect that a cached plan is
  stale before clicking or dragging.
- `expose_target_window` raises the attached target and verifies whether it is
  still covered before OCR, screenshots, or raw pointer input.

Display bounds use macOS global screen points; external displays can have
negative `x`/`y` coordinates depending on Arrangement. Window moves require
Accessibility permission and can fail if an app or Space refuses AX position/size
changes.

OCR and custom-surface tools:

- `read_text` returns OCR lines; `content_only:true` filters browser chrome and
  low-confidence noise.
- `read_text_detailed` adds target-visibility diagnostics, warnings, and
  recommended next steps to the OCR result.
- `find_ocr_text` returns ranked OCR hits with bbox and center point.
- `click_near_text` clicks a selected OCR hit and can verify an `expected_text`
  postcondition afterward.
- `extract_ocr_cards` groups OCR lines into card-like candidates with title,
  rating, reviews, ETA, fee, promo, and bbox when visible.
- `detect_modal` and `dismiss_modal` handle blocking popups conservatively:
  dismissal only clicks recognized close/dismiss candidates.
- `read_shapes` and `scan_chart` cover geometric primitives and charts that AX
  and OCR do not model well.

Performance controls:

- Every MCP `tools/call` result includes `_meta.dunst.timing_ms` and
  `_meta.dunst.tool` for per-tool latency profiling.
- Read-orientation tools such as `find_element`, `page_state`, and `window_view`
  use a short AX refresh TTL by default. Pass `force_refresh:true` to bypass it.
- Mutating action paths still force a full AX refresh after execution.
- `read_text` captures only the requested screen `region` when one is provided,
  instead of capturing the whole target window and cropping later.
- `visual_change_probe` samples grayscale/luminance cells instead of keeping all
  colour channels. The main speed win is still the smaller captured region; luma
  sampling reduces comparison work and memory.
- Display and desktop topology are cached briefly; window move/arrange tools
  invalidate the desktop cache after changing geometry.
- `DUNST_AX_MAX_NODES` and `DUNST_AX_MAX_DEPTH` can lower AX traversal caps for
  very large/noisy apps.

To serve the deterministic fixture instead of a live window:

```bash
DUNST_MCP_MODE=fixture scripts/mcp-dunst.sh
```

`DUNST_MCP_ENABLE_APPROVE_TOOL=1` opt-ins to the operator-side `approve` tool for
controlled local sessions. It is not advertised by default.

Homebrew is a good later packaging target for a stable CLI, but the repo-local
wrapper is better during development: it always runs the current checkout, builds
when the debug binary is missing, and keeps Codex/Claude config pointed at the
code under test. A future formula should install the compiled `dunst-mcp` binary
and use the same `serve` entrypoint.

The `demo` narrates: resolve "Nouvelle note" by **label** → click → a destructive
`Supprimer`/`Éteindre` is **denied pending approval** → approve → proceed →
audit trail exported as JSON.

## Development

Run the same core checks as CI:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --locked
shellcheck scripts/*.sh
```

Install local Git hooks with [prek](https://github.com/j178/prek):

```bash
brew install j178/tap/prek
prek install
```

The hooks run `cargo fmt`, `clippy`, `shellcheck`, `gitleaks`, and
offline `lychee` before commits. Heavier checks (`cargo test`,
`cargo audit`, `cargo machete`) run before pushes. CI runs the online
Markdown and link checks.

Live smoke is macOS-only and requires Accessibility permission; screenshot/OCR
paths also require Screen Recording:

```bash
scripts/smoke-live.sh Notes
```

Branch policy: `main` should be protected before release distribution. In the
current repository, GitHub reports `main` as unprotected and the rulesets API
returns `403`, so this environment cannot enforce the policy directly. Until
rulesets/branch protection are available, green CI plus manual review is the
required merge gate.

## Status

POC / work in progress. Differentiator vs raw computer-use drivers: the semantic
layer — stable IDs, affordance normalisation, **risk-based approval gating**,
verify-loop and audit trail — not pixel OCR.

## License

Licensed under either MIT or Apache-2.0, at your option. See `LICENSE-MIT`,
`LICENSE-APACHE`, and `LICENSE`.
