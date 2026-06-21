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

## P1 - Reliable Web Scrolling

- Add a wheel-based `scroll_at` path for browser content and use it as the
  default when no AX scrollable id is available.
- Expose page-level pseudo scroll targets when a browser page is visibly
  scrollable even if AX does not expose `AXVerticalScrollBar`.
- Return `visual_changed` beside `graph_diff_summary` so a successful scroll with
  no visible movement is treated as unverified.

## P1 - Browser Chrome Versus Page Scope

- Split affordance/query output into `browser_chrome` and `page` scopes so
  `get_affordances` does not drown the page in Firefox toolbar controls.
- Add browser find-bar support: detect, type into, and close Firefox's find bar
  without raw coordinate guessing.
- Add an `open_url_and_attach_tab` flow that opens a URL, selects the matching
  tab, and refreshes the target state.

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
