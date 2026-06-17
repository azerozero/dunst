> **Historical note:** This file records an earlier VisualOps-era work package or review. Current crate names, setup commands, and status live in docs/README.md, docs/ARCHITECTURE.md, and docs/CONTRACTS.md.

# WP-F — `visualops-platform`: ref retention + CGEvent Type/Drag (owner: Codex / tmux %2)

Work **only** in `crates/visualops-platform`. Do **not** touch `visualops-core`
/ `visualops-graph` / `visualops-mcp`. Do **not** run git. Keep
`cargo build -p visualops-platform` green and `examples/dump` + the live MCP
working. Do **not** change the `ActionExecutor::perform` signature (frozen).

## Frozen mini-contract for Drag (agreed with the MCP side, WP-G)
`Drag` is delivered as `perform(target, source_node, SemanticAction::Drag, Some("x,y"))`
where **`"x,y"` is the drop point in screen coordinates** (already computed by the
engine from the destination node's bbox centre). The **start** point is the
**source** element's bbox centre (`source_node.bbox`). You only implement the
mouse motion; you never resolve a second node.

## F1 [MAIN] Retain `AXUIElementRef` per node — skip the re-resolve on every action
Today `perform` does `app_element(pid)` → `resolve_window` → `find_element(window,
node)`, and `find_element` **re-walks the whole live tree** to match one element
by criteria. That is thousands of AX reads per action. Cache the retained element
during `capture` and look it up in `perform`.

- `AXUIElementRef` is `!Send`/`!Sync` and the trait is `Send + Sync`, so do **not**
  store the cache on `MacosBackend`. Use a **`thread_local!`** map inside `mod
  macos` (capture and perform run on the same thread — the single-threaded MCP
  serve loop):
  ```rust
  thread_local! {
      static AX_CACHE: RefCell<HashMap<ElementKey, AxElement>> = RefCell::new(HashMap::new());
  }
  ```
- `ElementKey` must be derivable **both** from `RawAxNode` (during `capture`) and
  from `SceneNode` (during `perform`) — they expose the same fields. Use the same
  tuple `find_element` matches on: `(ax_identifier, ax_role, label, bbox rounded
  to whole px)`. Keep it a plain hashable struct.
- During `walk_element` (capture), after you build each node, insert a **retained**
  `AxElement` (clone the existing CFRetain — the cache owns its own +1) under its
  key. **Clear the cache at the start of each `capture`** (old `AxElement`s drop →
  `CFRelease`, no leak) so it always reflects the latest tree.
- In `perform`, compute the key from `node` and look it up:
  - **hit** → act directly on the cached ref, skipping `resolve_window` /
    `find_element` entirely.
  - **miss, or the action returns `kAXErrorInvalidUIElement`/`kAXErrorCannotComplete`**
    (stale ref) → drop that entry and fall back to the **current**
    `resolve_window` + `find_element` path exactly as today.
- Net effect: one tree walk per `capture` (unchanged), O(1) per action.

**Measure:** add a timing line (reuse `VO_DUMP_TIMING` style, stderr) around a
`perform` and report action latency cached-vs-fallback in your summary.

## F2 [FEATURE] CGEvent `Type` fallback (m1)
`SemanticAction::Type` currently only does `set_string_attr(kAXValueAttribute)`,
which silently no-ops on elements that don't accept AXValue set (many native
editors). Keep the AX set-value as the **fast path**; if it fails (or the element
has no settable `AXValue`), fall back to synthesising the text via **CGEvent**
keystrokes (`CGEventKeyboardSetUnicodeString` per char, or unicode events). Focus
the element first (`set_bool_attr(kAXFocusedAttribute, true)`), then post the
events. Note the trade-off in a comment: AX set-value replaces, keystrokes append.

## F3 [FEATURE] CGEvent `Drag` (m4)
Implement the `SemanticAction::Drag` arm (currently the `other =>` error):
- Start = `source_node.bbox` centre; destination = parse `argument` as `"x,y"`
  (two f64s; error if absent/malformed — `Drag requires an "x,y" argument`).
- Synthesise: left mouse-down at start → a few `mouseDragged` steps to the
  destination → left mouse-up, via `CGEvent` (you already use `CGEvent` for
  hover). Small sleep between steps so the target registers the drag.
- This moves the real cursor (unavoidable for a synthetic drag) — note it.

## Out of scope
- No `core`/graph/mcp edits. No new public types. If you think the contract needs
  a change, **STOP and write it in your summary**.

Finish: print `cargo build -p visualops-platform`, a `dump` run (both roots), the
action-latency cached-vs-fallback numbers, and a summary of what changed.
