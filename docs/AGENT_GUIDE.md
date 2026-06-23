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

1. `get_hit_targets`, `find_element`, `get_affordances`, or `text_snapshot` for
   AX-exposed elements.
2. `read_text_detailed`, `find_ocr_text`, or OCR targets from `get_hit_targets`
   when browser AX is sparse.
3. `click_near_text` with `expected_text` and, when supplied by
   `get_hit_targets`, `offset_x`/`offset_y` for adjacent form fields.
4. `type_into` for real AX text elements.
5. `paste_text` for focused opaque web fields when `type_keys` is unreliable.
6. Raw `click_at`, `press_key`, `type_keys`, `hotkey`, or external GUI
   automation only after explicit operator authorization and a fresh OCR or
   screenshot check.

Always re-read the field or page with OCR/AX before saving or submitting after a
raw mutation.

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
- If `user-active guard blocked` appears, wait for operator idle and retry the
  same approved action. The raw grant is restored for user-active failures.
- Do not use `javascript:` in the browser address bar as a fallback.
- Do not switch to shell `osascript` GUI driving unless the operator explicitly
  authorizes that broader fallback.

For repeated key deletion in opaque fields, prefer smaller batches plus OCR
verification. For long insertion, prefer `paste_text` over long `type_keys`
payloads.

## Live Debug Checklist

When a live MCP flow misbehaves:

1. Confirm the attached target with `target_visibility` and `window_view`.
2. Compare AX with OCR using `get_hit_targets` and `read_text_detailed`.
3. If AX is sparse, use OCR-derived targets and label-relative offsets.
4. If typing succeeds but OCR is unchanged, assume the field was not focused.
5. If the idle guard blocks repeated keys, pause and retry the same approved
   action; do not broaden to unguarded automation without explicit permission.
6. Before saving, verify the final visible text by OCR.

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
