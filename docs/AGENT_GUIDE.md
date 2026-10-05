# Agent Operating Guide

This guide is the versioned operating path for automation agents working on
Dunst MCP. It complements `docs/CODE_NAVIGATION.md` and `docs/CONTRACTS.md`;
those files remain the source of truth for code ownership and behavioural
invariants.

## Reading Order

Start with these files, in order:

1. `README.md` for the product surface and setup.
2. `docs/CODE_NAVIGATION.md` for module boundaries and edit zones.
3. `docs/CONTRACTS.md` for invariants that require tests when changed.
4. `CONTRIBUTING.md` for validation and commit rules.
5. `docs/BINARY_USAGE_REX.md` for live MCP lessons and known operational gaps.

Do not create project-local `CLAUDE.md`, `llms.txt`, or `llms-full.txt` unless an
operator explicitly asks for those marker files. Keep agent guidance in normal
project docs.

## Command Rules

Use the RTK wrapper for shell commands in this workspace:

```bash
rtk cargo fmt --check
rtk cargo test
rtk cargo clippy --all-targets -- -D warnings
rtk cargo build --release -p dunst-mcp
```

For live binary validation after a successful release build:

```bash
rtk install -m 0755 target/release/dunst-mcp /Users/ludwig/.cargo/bin/dunst-mcp
rtk /Users/ludwig/.cargo/bin/dunst-mcp --version
```

Installing outside the workspace requires operator approval in managed
sandboxes. Do not overwrite unrelated local files or revert user changes.

## MCP Action Order

Use the safest available interaction layer first:

1. `platform_capabilities` when you need to know whether the current backend can
   use background input, cursor borrowing, clipboard text, OCR/CV, window ops, or
   app/file-chooser operations.
2. `get_hit_targets`, `find_element`, `get_affordances`, or `text_snapshot` for
   AX-exposed elements.
3. `read_text_detailed`, `find_ocr_text`, or OCR targets from `get_hit_targets`
   when browser AX is sparse.
4. `click_near_text` with `expected_text` and, when supplied by
   `get_hit_targets`, `offset_x`/`offset_y` for adjacent form fields.
5. `type_into` for real AX text elements.
6. `paste_text` for focused opaque web fields when `type_keys` is unreliable.
7. Raw `click_at`, `press_key`, `type_keys`, `hotkey`, or external GUI
   automation only after explicit operator authorization and a fresh OCR or
   screenshot check.

Always re-read the field or page with OCR/AX before saving or submitting after a
raw mutation.

## Batch A Multi-Field Choice Page

Use `enumerate_choices` before filling choice-heavy forms, modals, or checkout
pages:

1. Call `enumerate_choices` with default `include_latent=true`. Use
   `scroll_scan=true` only for virtualized or AX-sparse surfaces that need an OCR
   survey; it is mutation-coordinated, not approval-gated.
2. Build one `apply_selections.plan.steps[]` from returned `Choice.id` values.
   Include `label` for reflow fallback; use `op: "select"`, `"deselect"`, or
   `"set_text"`.
3. Pass `enumerate_choices.ui_epoch` as `expected_epoch`.
4. The first `apply_selections` call returns `status: "pending_approval"` and a
   single `batch_id`; surface the preview to the operator and approve that id
   once.
5. Re-call `apply_selections` with the same plan. Inspect `steps`, `rescans`,
   and the consolidated `verify` block. If the result is `partially_applied`,
   re-run `enumerate_choices` and apply only the remaining choices.

## Firefox And Sparse AX

Firefox can expose only window chrome while the page itself is readable by OCR.
Expected symptoms:

- `list_browser_tabs` may have no AX radio-button tabs.
- `window_view.visible_text` and `page_state.visible_text` may be empty from AX.
- Web inputs may not appear as `AXTextField` or `AXTextArea`.
- `type_keys` can report input success while the visible field remains unchanged
  if the field focus was not actually inside the web input.

Current mitigations:

- `list_browser_tabs` falls back to the target window title for sparse browser
  windows.
- `page_state` falls back to content OCR when browser AX text is empty.
- `get_hit_targets` adds OCR-derived form-field targets for labels followed by
  visible values, such as `Titre de la réalisation` and `Description`.
- `click_near_text` accepts label-relative offsets, so agents can focus a field
  using a verified OCR label instead of a hand-picked coordinate.
- `paste_text` performs clipboard set, Cmd+V, and clipboard restore as one
  audited MCP action. It restores previous plain-text clipboard content; rich
  clipboard formats may not survive.

## Raw Input Rules

Raw pointer and keyboard tools are high-risk. Keep these rules:

- Prefer OCR-bound actions over screen-coordinate actions.
- Include `expected_text` when clicking or focusing by OCR.
- Treat `visible_background` as usable for background SkyLight input, but verify
  target visibility before raw pointer or real-cursor actions.
- Real-cursor actions such as `right_click_at`, `scroll_at(borrow_cursor=true)`,
  `read_at(borrow_cursor=true)`, and `reveal_hover_click` require visible target
  pixels at the requested point. They should restore the cursor and must not
  raise the target window as a side effect.
- If `user-active guard blocked` appears, wait for operator idle and retry the
  same approved action. The raw grant is restored for user-active failures.
- Do not use `javascript:` in the browser address bar as a fallback.
- Do not switch to shell `osascript` GUI driving unless the operator explicitly
  authorizes that broader fallback.

For repeated key deletion in opaque fields, prefer smaller batches plus OCR
verification. For long insertion, prefer `paste_text` over long `type_keys`
payloads.

## Session Provenance

Each MCP server process has a `SessionIdentity`:

- `session_id` is generated when the server starts.
- `client_name` and `client_version` come from MCP `initialize.clientInfo` when
  the client sends them.
- `agent_id` comes from `DUNST_MCP_AGENT_ID` when the operator wants a stable
  human-readable label for the agent.
- `parent_pid` and `parent_process` are best-effort process ancestry hints.

The identity appears in `_meta.dunst.session`, every audited `AuditEntry.caller`
record when known, and stderr `tools/call` logs. Treat it as provenance only: it
does not authenticate a client and does not replace the approval gate.

## Platform Capability Groups

Do not infer support from `target_os` in MCP-facing code. Query
`platform_capabilities` and branch on the grouped surface:

- `input`: AX actions, background pointer/keyboard/hotkeys, focus without raise,
  real cursor borrowing, and menu-bar actions.
- `clipboard`: plain-text read/write and whether rich formats are preserved.
- `perception`: AX tree, screenshots, OCR, CV shapes, and chart scanning.
- `windows`: listing, visibility, move/resize, arrange, and expose operations.
- `apps`: running-app listing, launch/open URL/close, installed app metadata, and
  native file chooser support.

OS-specific implementation belongs in `dunst-platform` or `dunst-vision`. MCP
dispatch should reason in these capabilities and tool-level contracts, which
keeps the macOS backend replaceable by Linux/Windows backends later.

For multi-session work, keep the current design pattern:

- Multiple readers are acceptable.
- UI mutation is single-writer. Mutating MCP tools take a global mutation lock
  before executing pointer, keyboard, focus, clipboard, window, or launch actions.
- The active target window also gets a TTL lease owned by `SessionIdentity`.
  Another session mutating the same `window_id` fails with a clear
  `window_lease_blocked` coordination result until the lease expires.
- The lease returns a `fencing_token` in `_meta.dunst.coordination.mutation`.
  Pass it on later mutating calls when you want stale lease ownership to be
  refused explicitly.
- Pass `expected_epoch` from `get_hit_targets.ui_epoch.fingerprint` on mutating
  calls when you are acting from a cached UI plan. Dunst refuses the mutation if
  the current window/tab/visibility/actionable graph fingerprint changed.
- Do not bypass Dunst with direct `osascript`/external GUI automation while a
  Dunst lease is active; the MCP coordinator cannot serialize tools it never
  sees.

## Live Debug Checklist

When a live MCP flow misbehaves:

1. Confirm the attached target with `target_visibility` and `window_view`.
2. Check `_meta.dunst.session` or stderr logs to identify which MCP session is
   issuing calls.
3. Check `_meta.dunst.coordination` for `window_lease_blocked`,
   `fencing_token_mismatch`, or stale `expected_epoch` before retrying.
4. Compare AX with OCR using `get_hit_targets` and `read_text_detailed`.
5. If AX is sparse, use OCR-derived targets and label-relative offsets.
6. If typing succeeds but OCR is unchanged, assume the field was not focused.
7. If the idle guard blocks repeated keys, pause and retry the same approved
   action; do not broaden to unguarded automation without explicit permission.
8. Before saving, verify the final visible text by OCR.

## Validation Before Handoff

Run the closest checks for the touched layer. For changes to MCP schema,
dispatch, raw input, or perception fallbacks, run at least:

```bash
rtk cargo fmt --check
rtk cargo test -p dunst-mcp
rtk cargo test -p dunst-platform
```

Run full workspace tests and clippy before pushing when time permits:

```bash
rtk cargo test
rtk cargo clippy --all-targets -- -D warnings
```

## MCP Tool Reference (all 76 tools)

The catalog is authoritative in `crates/dunst-mcp/src/serve/tools.rs`; a live
client sees it via `tools/list`. **76 tools are registered; 73 are advertised by
default** — the 3 operator-approval tools are gated behind
`DUNST_MCP_ENABLE_APPROVE_TOOL=1`. Grouped by family (family = `*_tools()` in
`tools.rs`).

### Orientation & state (`state_tools`, 13)

- `version` — running build identity (package version, git commit, dirty flag, timestamp, protocol).
- `platform_capabilities` — grouped OS backend capabilities (input, clipboard, perception/OCR/CV, windows, apps).
- `refresh` — re-perceive the target window and rebuild the scene + affordance graphs.
- `get_scene_graph` — the current scene graph (`compact` | `full` | `summary`; `actionable_only`).
- `page_state` — lightweight orientation snapshot: app/window, title, likely URL, visible text, key elements.
- `text_snapshot` — AX text snippets without the full scene graph or OCR.
- `wait_for_text_stable` — wait until AX text snippets stop changing for a stable interval.
- `list_browser_tabs` — browser tabs exposed by the target window tab strip.
- `list_displays` — active displays: 1-based index, `display_id`, global bounds, scale, main flag.
- `window_view` — compact scoped view of the target window (owning display, bounds, text, key elements).
- `desktop_view` — desktop/window topology: displays, windows, `z_order`, frontmost, overlaps (`degraded:true` when CG topology is missing).
- `target_visibility` — whether the target is frontmost, covered, fully covered, or missing.
- `visual_change_probe` — spaced luminance pixel grid vs the previous probe; refreshes AX only when pixels changed.

### Query & perception (`query_tools`, 14)

- `analyze_region_ax` — AX hit-tests on a spaced grid over one screen region.
- `get_affordances` — affordance graph (actions + risk per element); `include_latent`, `scope`.
- `get_hit_targets` — semantic UI targets: labels, roles, safe click zones, action modes, risk, `ui_epoch` fingerprint.
- `find_element` — elements whose id/label/role contains a query (case-insensitive).
- `wait_for_element` — poll the AX graph until an element matching a query appears or disappears.
- `read_text` — OCR the target window (or a region) via Apple Vision; `content_only` filters chrome/noise.
- `read_text_detailed` — OCR plus target-visibility diagnostics, warnings, and recommended next steps.
- `read_shapes` — CV geometric primitives (rect/bar/circle/line) in the target window.
- `read_zones` — nest flat vision output (shapes + OCR + controls) into a containment tree.
- `find_ocr_text` — ranked target-window OCR hits with bbox, center point, confidence.
- `detect_modal` — likely modal/overlay state and safe OCR close/dismiss candidates.
- `extract_ocr_cards` — group OCR lines into card candidates (title/rating/reviews/eta/fee/promo).
- `query_affordances` — element ids exposing a given semantic action (`click|type|hover|open_menu|pick|drag`).
- `enumerate_choices` — structured choice model (groups, required/optional, per-choice state) for batch fills.

### Element actions (`element_tools`, 11)

- `click_element` — click an element by id.
- `raise_element` — raise an element by id (typically a window root).
- `pick_option` — pick a popover/list/radio option by visible text.
- `type_into` — replace text in a text element by id (risk-gated).
- `hover_probe` — hover an element by id to reveal tooltips on a live target.
- `drag_element` — drag a source element onto a target element by id (risk-gated).
- `click_at` — click a raw screen point inside the target window.
- `click_near_text` — OCR, pick a ranked text hit, click its center or a bounded offset; verify `expected_text`.
- `dismiss_modal` — dismiss a modal only via an OCR-detected close/dismiss candidate.
- `reveal_hover_click` — briefly borrow the real cursor on a visible point to reveal hover-only controls.
- `select_file` — pick a local file in the native file chooser for browser upload controls.

### Batch (`batch_tools`, 1)

- `apply_selections` — apply a whole choice plan as one batch behind a single operator approval.

### Pointer & charts (`pointer_and_chart_tools`, 6)

- `hover_at` — background mouse-move (no cursor movement) at a raw screen point.
- `read_at` — read the value at a screen point inside the target window.
- `read_series` — read values at several screen points.
- `scan_chart` — detect → confirm rendered → traverse → series.
- `focus_window` — make the target AppKit-active without raising it (SkyLight focus-without-raise).
- `unstick_cursor` — recover a stuck OS cursor (e.g. a frozen I-beam) after driving a background window.

### Windows & apps (`window_app_tools`, 14)

- `list_windows` — enumerate real, drivable windows (sizeable + titled).
- `move_window_to_display` — move the target window to a display from `list_displays`.
- `move_app_to_display` — move all sizeable top-level windows for an app to a display.
- `arrange_windows` — reorganize selected windows on one display (grid/columns/rows/cascade/maximize).
- `expose_target_window` — try to make the target actually visible, then verify with `desktop_view`.
- `list_apps` — running GUI apps that own a window (app, pid, window count, on_screen).
- `list_launchable_apps` — installed `.app` bundles without launching them.
- `app_info` — one installed app's `Info.plist` metadata before launching.
- `attach` — re-target the daemon to a `window_id` at runtime.
- `launch_app` — launch an app in the background, optionally opening a URL/args.
- `open_url_and_attach_tab` — open a URL, attach to the best browser window, report the selected tab.
- `navigate` — load a URL in the attached browser window and re-verify.
- `close_app` — quit an app gracefully by name (no foreground).
- `screenshot` — composited PNG of the target window (multimodal: see the pixels directly).

### Keyboard, menus & audit (`keyboard_menu_tools`, 14)

- `right_click_at` — real-cursor context-click at a raw screen point.
- `double_click_at` — double-click at a raw screen point.
- `open_menu` — open an app menu-bar menu by name via AX.
- `press_key` — press a named key on the target; optional `repeat` for simple repeated edits.
- `type_keys` — type into the focused element via the SkyLight auth-signed keyboard path.
- `set_field_text` — clear the focused field and set it to text in one step.
- `paste_text` — paste text into the focused element by temporarily replacing the clipboard (Cmd+V).
- `scroll` — scroll the focused page/container (AX scrollbar, or background keys / learned fallback).
- `scroll_at` — wheel-scroll at a concrete screen point.
- `zoom` — zoom the focused page in the background (Cmd =/-/0, auth-signed).
- `hotkey` — send a background keyboard shortcut (modifiers + key, auth-signed).
- `verify_state` — assert an element field (`label|value|enabled|focused`) equals an expected value.
- `diff_since` — structural diff between the previous and current scene graph.
- `export_trace` — export the audit trail.

### Operator approvals (`approval_tools`, 3 — gated behind `DUNST_MCP_ENABLE_APPROVE_TOOL`)

- `approve` — approve a gated element or raw target so the next action on it proceeds.
- `preauthorize` — pre-authorize raw input in the attached window for a bounded flow.
- `revoke_preauthorization` — drop any active raw-input pre-authorization immediately.

#### Avoid per-keystroke approvals

Prefer `type_into(id, text)`: it replaces a mapped field and verifies the value,
including an empty replacement or an already-correct value. A low-risk field and
payload need no raw-input approval. The keyboard fallback requires verified focus
and an empty field or a verified full-field selection; it refuses unsafe appends.

If raw input is necessary, ask the operator once for the task, attached window,
raw-input scope, action budget and duration. After that grant, call
`preauthorize({"budget":20,"ttl_ms":120000})` once, perform the authorized flow,
then call `revoke_preauthorization`. Maximums are 100 calls and ten minutes;
changing the attachment clears the grant. Expiry is not permission to renew it.
Batch selection, file selection and high-risk element actions keep their own gates.

Raw input can submit forms or operate browser chrome. A window grant is **not** a
site restriction, draft-only mode or permission for unrelated sends/payments.
The MCP host must require human confirmation for `approve` and `preauthorize`;
the server cannot authenticate a human through an ordinary MCP tool call.
Leave these tools disabled if the host cannot enforce that boundary. Never enable
them autonomously to escape a pending approval.

#### Focus and uncertain clicks

`focus_window` and keyboard/background event fallbacks can move keyboard focus,
including away from a sibling window. They do not change monitor arrangement.
Do not equate background delivery with focus preservation or claim no window was
raised without observing it.

Clicks poll for delayed AX effects without replaying the action. A timeout or a
geometry-only diff does not prove failure. Check the current page before retrying,
especially for Send, payment and deletion. The action audit spans the state before
execution through the final observation, not just the last animation frame.
