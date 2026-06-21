# Dunst UI Remediation Plan

Date: 2026-06-21

Scope: failures observed while driving the Collective Work profile editor through
Dunst only, without browser cookie/API fallbacks.

## P0 - Raw Input Ergonomics

- Add scoped raw approvals so one operator-approved keyboard action can cover a
  short burst of the same safe repetition, such as several `Backspace` presses.
- Keep raw pointer clicks exact by default, but track them as pointer actions
  rather than generic typing.
- Add `press_key.repeat` so agents do not issue parallel keypress tool calls for
  simple repeated edits.
- Label raw audit actions correctly: `key_press`, `hotkey`, and `scroll` must not
  serialize as `type`.
- Retry the user-active guard internally once, then return a clear failure only
  when the target is still busy.

Status: implemented. The raw grant policy is now typed by scope
(`KeyPress`, `ScrollDirection`, `Hotkey`, `Exact`) and has regression tests for
same-key event budgets, one-shot text entry, changed text payload isolation,
cross-key isolation, hover-reveal cleanup, attach invalidation, and user-active
retry restoration.

## P0 - Setup And Contract Correctness

- Fix installed `dunst-mcp setup` snippets so MCP clients run `dunst-mcp serve`
  instead of launching the fixture demo.
- Add CLI tests for Codex and Claude setup snippets, including the repo wrapper
  case where `scripts/mcp-dunst.sh` owns its own `serve --live` arguments.
- Keep `docs/CONTRACTS.md` explicit about the difference between one-shot
  element approvals and scoped raw input grants.
- Add deterministic tests for raw grant expiry and restore-on-user-active-guard
  behavior before splitting the grant policy further.

Status: installed setup snippets and CLI tests are implemented. `doctor` now
parses `.mcp.json` and `.codex/config.toml`, verifies the `dunst` command, and
returns a non-zero status when an installed config would start `dunst-mcp`
without `serve`.

## P1 - Reliable Web Scrolling

- Add a wheel-based `scroll_at` path for browser content and use it as the
  default when no AX scrollable id is available.
- Expose page-level pseudo scroll targets when a browser page is visibly
  scrollable even if AX does not expose `AXVerticalScrollBar`.
- Return `visual_changed` beside `graph_diff_summary` so a successful scroll with
  no visible movement is treated as unverified.
- Batch repeated key operations through `press_key.repeat` from the tool layer,
  not by issuing parallel raw keyboard calls.

Status: partial. Successful scrolls with no meaningful AX movement now return a
verification hint; `press_key.repeat` is implemented. Wheel-based scrolling and
page-level pseudo scroll targets remain open.

## P1 - Browser Chrome Versus Page Scope

- Split affordance/query output into `browser_chrome` and `page` scopes so
  `get_affordances` does not drown the page in Firefox toolbar controls.
- Add browser find-bar support: detect, type into, and close Firefox's find bar
  without raw coordinate guessing.
- Add an `open_url_and_attach_tab` flow that opens a URL, selects the matching
  tab, and refreshes the target state.
- Add stale-target detection for SPA/tab navigation: when URL or browser tab
  state changes but the page graph remains the previous view, surface that as a
  targeting problem instead of continuing raw probing.

Status: partial. `AXComboBox`/`AXSearchField` browser fields now map to
typeable text fields, fixing Google/address-bar `type_into` failures.
`window_view` and `page_state` expose the selected browser tab, and `launch_app`
returns matching windows plus a verification hint. Explicit tab selection now
wins over stale window-title matching. A fully atomic `open_url_and_attach_tab`
flow remains open.

## P1 - Engine And Approval Architecture

- Phase-split `Engine::act` into prepare, gate, execute, observe, verify, and
  audit phases without changing the externally visible `ActionResult` contract.
- Centralize grant policy as typed variants: element, contextual synthetic, and
  raw scoped grants.
- Extract post-action verification for removal, checkbox, and typed-value
  postconditions so retry and refresh behavior is testable outside the main
  action function.
- Rename or split `is_raw_input_target_id`; it currently names only
  `keyboard@`/`screen@`, while the gate also handles `file@` and
  `hover-reveal@` synthetic targets.

Status: implemented. `Engine::act` is split into prepare, gate, execute,
consume, observe, and verify helpers; synthetic approval detection is separate
from reusable raw approval scopes.

## P2 - OCR Guided UI Actions

- Add helpers for OCR-relative actions, for example `click_near_text("Expérience
  de travail", right=...)`, instead of hand-picked coordinates.
- Add a combined AX/OCR search that returns visible text hits, AX matches,
  confidence, and bbox in one ranked list.
- When AX says a target exists but OCR/screenshot do not confirm it, report a
  stale or chrome-only state instead of asking the agent to keep probing.

## P2 - Agent Playbook

- Document a "full Dunst" mode: no browser cookies, no product API calls, and no
  shell credential extraction unless the user explicitly changes scope.
- After two failed raw scroll attempts, switch strategy: direct PageUp/PageDown,
  wheel scroll, find-in-page, or OCR-relative click. Do not repeat the same
  unsuccessful path.
- Batch simple text-editing operations and verify the visible field after the
  batch before saving.

## P2 - Documentation, CI, And Repo Gates

- Fix README/toolchain drift whenever `Cargo.toml` changes `rust-version`.
- Add hotspot diagrams for `Engine::act`, MCP tool dispatch ownership, macOS
  backend routing, and perception/cache lifecycle.
- Add a root `CONTRIBUTING.md` covering local hooks, contract/test update rules,
  and commit workflow.
- Decide branch protection or ruleset strategy for `main`; GitHub currently
  reports `main` as unprotected, with CI green but not enforceably required.
- Add CI coverage and dependency/security gates once the fast docs/Rust gates
  stay stable.

Status: partial. CI now has dedicated `Security audit` and `Coverage gate`
jobs with pinned cargo tools. Main branch protection/ruleset enforcement was
attempted through the GitHub API, but GitHub returned `403` because branch
protection and rulesets require GitHub Pro or a public repository for this
private repo.
