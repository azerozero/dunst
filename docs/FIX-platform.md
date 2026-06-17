> **Historical note:** This file records an earlier VisualOps-era work package or review. Current crate names, setup commands, and status live in docs/README.md, docs/ARCHITECTURE.md, and docs/CONTRACTS.md.

# FIX round 2 — `visualops-platform` (owner: Codex / tmux 8)

Triaged from `docs/review-platform.md` (Claude), `cli-audit-code`, `cli-forge-perf`.
Work **only** in `crates/visualops-platform`. Do not touch `visualops-core` /
`visualops-graph`. Do not run git. Keep `cargo build -p visualops-platform` green
and `examples/dump` working on a live window.

## P1 [MAJEUR → blocking for the long-running server] CoreFoundation leak
The backend never `CFRelease`s a single owned `AXUIElementRef`/`AXValueRef`:
`app_element` (l.132), `attr_ax_element` (l.377), `attr_ax_value` (l.500), and
every child retained in `ax_elements` (l.435) leak. Per `capture` that is the app
ref + the window + up to MAX_NODES (5000) element refs + 1–2 AXValues/node. Fine
for one-shot `dump`, but `visualops-mcp` re-captures on every action → unbounded.

**Keep the `CFRetain` in `ax_elements` (l.435) — it is correct and required**
(`find_element` derefs children off its stack *after* the parent `CFArray` is
dropped; without the retain that's a use-after-free). The bug is the missing
paired release. Fix with an RAII wrapper:

```rust
struct AxElement(AXUIElementRef);
impl Drop for AxElement { fn drop(&mut self) { if !self.0.is_null() { unsafe { CFRelease(self.0 as _) } } } }
```

- `ax_elements` → `Vec<AxElement>` (the current retain becomes the wrapper's +1);
  `walk_element`/`find_element` hold `AxElement`s that free on Drop. For an
  element you *return* (resolve_window / find_element result), `mem::forget` the
  wrapper to transfer ownership, or return the `AxElement`.
- `app_element` → return an `AxElement`; it frees at end of capture/window_ref/perform.
- `attr_ax_value` → release the `AXValueRef` after `AXValueGetValue`.
- Remove the now-used `#[allow(dead_code)]` on `release_ax_element` or delete it
  in favour of `Drop`.

Verify with `leaks`/Instruments if convenient, else reason it through.

## P2 [MINEUR, integration] Don't mask empty `value` strings
`attr_string` does `.filter(|s| !s.is_empty())` (l.364), so a genuinely empty
text field (`AXValue == ""`) becomes `value: None`. The graph diff then can't tell
"field cleared" from "no value". Fix: only drop empty strings for the **label**
derivation; keep `Some("")` for `value`.

## P3 [MINEUR, integration] Align `element_matches` label fallback with capture
`walk_element` derives label as `title → description → (value only if
AXStaticText)` (l.197), but `element_matches` (l.280) falls back to `value` for
**any** role → can resolve a different element than the one captured/risk-assessed.
Make `element_matches` use the same rule (value only for `AXStaticText`).

## P4 [PERF] Lower the AX messaging timeout
`AXUIElementSetMessagingTimeout(app, 5.0)` (l.139): on a hung target a single
attribute read blocks ~5 s, multiplied across thousands of reads. Lower to ~1.0 s
and treat a timeout as "node unavailable".

## P5 [MINEUR] `resolve_window` reads the windows attribute twice
It copies `kAXWindowsAttribute` at l.146 and again at l.163 on the fallback path.
Memoise the first read (and release the windows you don't return — covered once P1
RAII is in).

## P6 [FEATURE, enables the live risk demo] Also walk the app's `AXMenuBar`
**Important integration point** you correctly flagged: the menu bar is an
attribute of the *application* element, not a descendant of the window, so a live
`capture` of Notes currently has **no** `Supprimer`/`Éteindre`/`Forcer à quitter`
— exactly the high-risk items the demo gates on. Add an option (e.g. a second
`capture` path or include it by default) that also reads the app element's
`kAXMenuBarAttribute` and returns it as a **second root** (matching the 2-root
fixture shape). Then a live capture mirrors the fixture and the live risk demo
works end-to-end. Keep menus collapsed-safe (menu items may have no frame → None).

## Deferred (note in code comment, do NOT implement now)
- m1 `Type` CGEvent fallback, m4 real `Drag` via CGEvents — post-POC.
- A1 batch reads via `AXUIElementCopyMultipleAttributeValues` — the biggest perf
  lever, but gated on having a benchmark/baseline first. Leave a `// PERF:` note.

Finish: print build result + a `dump` run (window root + menubar root) and a
summary of what changed.
