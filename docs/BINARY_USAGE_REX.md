# Binary and Live-Session Usage REX

Return on experience for using `dunst-mcp` as a live MCP server in a real
browser automation session. The source session was a Firefox/Uber Eats workflow;
personal order details, address data, payment details, and order identifiers are
intentionally omitted from this project document.

This document complements `README.md`, which remains the canonical setup and
command reference.

## Session Shape

Observed runtime:

| Area | Value |
|------|-------|
| MCP server | `dunst` |
| Main tools used | `screenshot`, `refresh`, `find_element`, `click_element`, `click_at`, `type_keys`, `read_text` |
| Browser | Firefox |
| Target style | Live macOS window, AX-first with screenshot/OCR fallback |
| Outcome | End-to-end browser workflow completed successfully |

The important product signal: `dunst-mcp` was capable of driving a real,
stateful, multi-step web checkout flow, but the reliable workflow depended on a
few strict operator habits.

## Binary Surface

The binary is `dunst-mcp`. It exposes four operator-facing commands:

```bash
dunst-mcp demo
dunst-mcp serve
dunst-mcp doctor
dunst-mcp setup
```

Observed command roles:

| Command | Use | Notes |
|---------|-----|-------|
| `demo` | Device-free fixture run | Good first confidence check; does not need a live app. |
| `serve` | MCP stdio server | Main runtime path for Codex/Claude-style clients. |
| `doctor` | Local environment diagnostics | Good preflight; exits non-zero when macOS Accessibility is missing. |
| `setup` | Print client config snippets | Dry-run only; it does not write user config files. |

For development, the practical startup path is:

```bash
cargo build -p dunst-mcp
cargo run -p dunst-mcp -- doctor
scripts/mcp-dunst.sh
```

For MCP handshake and tool-surface checks, prefer fixture mode:

```bash
DUNST_MCP_MODE=fixture scripts/mcp-dunst.sh
```

For app-pinned live startup:

```bash
DUNST_MCP_APP="Firefox" scripts/mcp-dunst.sh
```

## Reliable Operating Loop

The safest loop after page navigation, modal changes, or checkout transitions is:

```text
refresh -> find_element -> filter visible match -> click_element -> screenshot
```

Rules that mattered in the session:

1. Call `refresh` before `find_element` after every navigation or modal state
   change.
2. Treat `screenshot` after each action as the visual source of truth.
3. Prefer `click_element` over raw coordinate `click_at` when an AX element is
   available.
4. Use AX bounding boxes as screen coordinates; do not estimate from screenshots
   unless there is no element.
5. In modals, click labels well inside the content area; raw clicks near the
   border can hit the backdrop and close the modal.
6. For off-screen options inside modals, try `click_element` directly: AX
   `scrolltovisible` plus `press` can succeed when keyboard scrolling does not.

## Tool Reliability Observed

| Tool | Reliability | Notes |
|------|-------------|-------|
| `screenshot` | High | Best truth source after each action. |
| `refresh` | Required | Avoids stale scene-graph reads after navigation. |
| `find_element` | Medium | Refreshes by default on MCP; ranked visible matches reduce browser/menu noise. |
| `click_element` | High | Preferable to coordinates; can scroll off-screen AX elements into view. |
| `click_at` | Risky | Raw coordinates can miss small targets or hit modal backdrops. |
| `scroll` / `press_key` | Mixed | `scroll(id=...)` uses a direct AX scrollbar when exposed; otherwise key scrolling remains app-dependent. |
| `read_text` | Good | Useful OCR fallback for visible state and copy. |
| `type_keys` | Good | Worked for browser text entry in the session. |
| Action diffs | Medium | Compact summaries are now default; full diffs remain available on demand. |

## Issues and Workarounds

### Stale Scene Graphs

Symptom: `find_element` returned no matches immediately after navigation or a
modal transition.

Workaround used during the session: call `refresh` first. This was reproduced
multiple times.

Implemented improvement: the MCP `find_element` tool now refreshes before
searching by default. Callers can pass `fresh=false` when they explicitly want to
search the cached graph.

### Large Scene Diffs

Symptom: action results could include huge scene-graph diffs that were too large
to read inline.

Workaround used during the session: use `screenshot` as the post-action check
and reserve raw diffs for debug artifact inspection.

Implemented improvement: action responses now return `graph_diff_summary` by
default. Pass `include_diff=true` to get the full `graph_diff`, or call
`diff_since` with `summary=true` for compact diff counts and samples.

### Modal Scrolling

Symptom: keyboard scrolling did not reliably move internal modal content.

Workaround: use `click_element` on the desired off-screen option when possible.
The AX path can scroll the element into view and press it in one action.

Implemented improvement: `scroll` accepts an optional `id`. With an `id`, the
backend resolves the element, searches that element and its AX parents for
`AXVerticalScrollBar`, and changes the scrollbar `AXValue` directly. Without an
`id`, `scroll` keeps the background Page/Home/End fallback.

Remaining limitation: web/custom modals only benefit from direct AX scrolling
when the browser exposes the modal container or an ancestor as an AX element with
`AXVerticalScrollBar`. For off-screen menu options, `click_element` can still be
the better path when AX can scroll the target into view.

### Raw Coordinate Clicks

Symptom: small coordinate misses could close modals or hit neighboring controls.

Workaround: prefer `click_element`; if raw clicking is required, click inside
the label/content body, not near the modal edge.

Implemented improvement: raw clicks are already high-risk gated, and the risk
reasons now flag points outside visible scene elements as possible
backdrop/blank-area clicks.

`modal-aware` would mean a stricter version of this: detect that a modal/dialog
is currently open, identify its rectangle, and specifically warn when a raw click
falls outside that modal but inside the surrounding window. The current check is
geometry-only; it does not yet prove that the blank area is a modal backdrop.

### Noisy Search Results

Symptom: `find_element` could include browser chrome/menu/history items in
addition to the intended page target.

Workaround used during the session: filter matches by visible bounding box,
enabled state, role, and whether the bbox falls inside the target window.

Implemented improvement: `find_element` ranks visible, enabled targets first and
accepts `visible_only=true` to drop latent/off-window matches.

### Stale Debug Binary

During this REX cycle, `target/debug/dunst-mcp --help` initially did not list
`setup` even though the source code implemented it. Rebuilding fixed the
mismatch:

```bash
cargo build -p dunst-mcp
```

Operational rule: after CLI source changes, test via
`cargo run -p dunst-mcp -- ...` or rebuild before invoking
`target/debug/dunst-mcp` directly.

## Recommended Helper

A workflow-level `safe_click` helper would have prevented most misses observed
in the session:

```text
safe_click(query, expected_role):
  refresh
  matches = find_element(query)
  keep matches with:
    bbox inside target window
    enabled == true
    role matches expected_role when supplied
    confidence >= 0.9 when confidence is present
  click_element(best_match.id)
  screenshot
```

This can live first as a client-side recipe. If it proves stable, promote it to
a first-class MCP tool.

## Follow-Up Product Backlog

Implemented from this REX:

1. `find_element` refreshes before searching by default on the MCP surface.
2. `find_element` ranks visible, enabled targets first.
3. `find_element` accepts `visible_only=true` to drop latent/off-window matches.
4. Action responses are compact by default and expose `graph_diff_summary`
   instead of the full scene diff.
5. Full action diffs remain available through `include_diff=true`.
6. `diff_since` accepts `summary=true` for compact diff counts and samples.
7. `scroll` accepts an optional scrollable `id` and uses direct AX scrollbar
   value changes when the app exposes `AXVerticalScrollBar`.
8. Raw click risk reasons flag likely backdrop/blank-area points.
9. `page_state` returns title, likely URL, visible text, and key elements.
10. CLI help smoke tests assert the implemented subcommands/options stay listed.
11. README documents expected exit codes for `demo`, `serve`, `doctor`, and
    `setup`.
12. `list_displays` exposes active screens with index, bounds, pixel resolution,
    scale, and main-display flag.
13. `window_view` gives a compact scoped view of the target window/display
    without returning the full AX graph.
14. `move_window_to_display` moves the target window between displays by index.
15. `move_app_to_display` moves all sizeable top-level windows for a running app
    between displays by index.
16. `desktop_view` reports display/window topology, front/back order, frontmost
    window, and geometric overlaps.
17. `arrange_windows` reorganizes an explicit window selection on one display as
    grid, columns, rows, cascade, or maximize.
18. `desktop_view` marks missing/invalid CoreGraphics display topology as
    `degraded:true` with a `reason` instead of returning a silent `0x0` display.
19. MCP `tools/call` responses expose `_meta.dunst.timing_ms` for per-tool latency
    profiling.
20. Read-orientation tools can reuse a recent AX graph through a short TTL while
    mutating actions still force a post-action refresh.
21. `read_text(region=...)` captures only the requested screen rectangle.
22. Display/desktop topology has a short TTL cache, invalidated after window
    move/arrange operations.
23. AX traversal caps are tunable with `DUNST_AX_MAX_NODES` and
    `DUNST_AX_MAX_DEPTH`.
24. `visual_change_probe` samples a spaced luminance grid over a screen region,
    compares it with the previous probe, and can run a full AX refresh only when
    pixels changed.
25. `analyze_region_ax` samples a screen region with AX hit-tests and returns the
    unique shallow AX elements under that grid, giving targeted zone analysis
    without a full AX tree walk.

Remaining backlog:

1. Make backdrop detection modal-aware instead of geometry-only if the AX tree
   reliably exposes modal/dialog roles in target apps.
2. Add wrapper-specific exit-code docs for shell validation failures.
