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
