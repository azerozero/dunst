> **Historical note:** This file records an earlier VisualOps-era work package or review. Current crate names, setup commands, and status live in docs/README.md, docs/ARCHITECTURE.md, and docs/CONTRACTS.md.

# WP-A — `visualops-platform` (macOS AX backend)

**Owner:** Codex (tmux 8). **Crate:** `crates/visualops-platform` only.
**This is the only crate allowed to touch macOS FFI.**

Read `docs/ARCHITECTURE.md` first. Do **not** edit `visualops-core` or
`visualops-graph`. Do **not** run `git`. Keep `cargo build -p visualops-platform`
green and the example below working.

Goal: implement the real `Perceptor` (AX tree → `Vec<RawAxNode>`) and
`ActionExecutor` (semantic action → AX perform / set value) on the existing
`MacosBackend` skeleton.

## Suggested crates (add to the `[target.'cfg(target_os = "macos")']` block)

```toml
accessibility-sys   = "0.1"   # kAX* constants, AXUIElement* fns
core-foundation     = "0.10"
core-foundation-sys = "0.8"
core-graphics       = "0.24"  # CGRect/CGPoint/CGSize, CGWindowID
```

Pick alternatives if you prefer (objc2 family is fine) — just keep the public
API (`MacosBackend` implementing the two core traits) unchanged.

## Perceptor::capture

1. `AXUIElementCreateApplication(target.pid)`.
2. Resolve the window: read `kAXWindowsAttribute` (CFArray of AXUIElement). To
   match `target.window_id`, use the private
   `extern "C" { fn _AXUIElementGetWindow(e: AXUIElementRef, out: *mut u32) -> AXError; }`
   and compare to `target.window_id`. **Fallback** (if that's flaky): use
   `kAXMainWindowAttribute`, else the first window. A wrong-window fallback is
   acceptable for the POC — log it to stderr.
3. Recursively walk from the window element. Per element read:
   - `kAXRoleAttribute` (CFString) → `ax_role`
   - label: `kAXTitleAttribute`, else `kAXDescriptionAttribute`, else (for
     static text) `kAXValueAttribute` → `label`
   - `kAXHelpAttribute` → `help`
   - `kAXValueAttribute` (when a string) → `value`
   - `kAXIdentifierAttribute` → `ax_identifier`
   - `AXUIElementCopyActionNames` → `ax_actions`: strip leading `AX`, lowercase
     (`AXPress`→`press`, `AXShowMenu`→`showmenu`).
   - `kAXFrameAttribute` (AXValue, `kAXValueCGRectType`) → `frame {x,y,w,h}` in
     global screen points. If absent, use `kAXPositionAttribute` +
     `kAXSizeAttribute`. Collapsed menu items legitimately have no frame → `None`.
   - `kAXEnabledAttribute` → `enabled` (default true); `kAXFocusedAttribute` →
     `focused`.
   - `kAXChildrenAttribute` → recurse (preserve order).
4. Guard depth/'count' so a pathological tree can't hang (cap ~5000 nodes,
   depth ~40); log if capped.

Return the window element as the single root (a `Vec` of one is fine).

## Perceptor::window_ref

Return `{ pid, window_id, app_name, title }`. `app_name` from
`NSRunningApplication` or the app element's `kAXTitleAttribute`; `title` from the
window's `kAXTitleAttribute`.

## ActionExecutor::perform

The MCP layer hands you a `SceneNode` (carries `ax_role`, `label`,
`ax_identifier`, `ax_actions`). Re-walk the app/window and find the live
`AXUIElement` matching `(ax_role, label, ax_identifier)` — first match wins
(POC heuristic; fine because labels are mostly unique). Then:

| `SemanticAction` | AX call |
|------------------|---------|
| `Click`, `Pick`  | `AXUIElementPerformAction(e, kAXPressAction)` |
| `OpenMenu`       | `AXUIElementPerformAction(e, "AXShowMenu")` |
| `Raise`          | `AXUIElementPerformAction(e, kAXRaiseAction)` |
| `Focus`          | set `kAXFocusedAttribute = true` |
| `Type`           | `AXUIElementSetAttributeValue(e, kAXValueAttribute, <arg>)`; if that errors, fall back to CGEvent keystrokes |
| `Hover`          | move cursor to bbox centre via CGEvent mouse-moved (or no-op + Ok for POC) |
| others           | return `VisualOpsError::Execution(...)` |

Return `Ok(())` on success, `VisualOpsError::Execution` / `ElementNotFound`
otherwise.

## Permissions

The host terminal already has Accessibility granted, so a child process should
work. If `AXIsProcessTrusted()` is false, return a clear
`VisualOpsError::Perception("accessibility not granted")` (don't panic).

## Deliverable to prove it in isolation

Add `examples/dump.rs`:

```
cargo run -p visualops-platform --example dump -- <pid> <window_id>
```

It builds a `MacosBackend`, calls `capture`, and prints the `Vec<RawAxNode>` as
pretty JSON to stdout (use `serde_json`). I (architect) will run it on the live
Notes window (`pid` from `list_windows`) to validate and to mint new fixtures.
Add `serde_json` as a dev-dependency for the example.

## Done-criteria

- `cargo build -p visualops-platform` green.
- `examples/dump` prints a non-trivial JSON tree for a real window (Notes,
  Finder, System Settings) with roles, actions, labels and frames populated.
- `perform(Click)` on a real button (e.g. Notes "Nouvelle note") visibly acts.
- No panics on missing attributes (use Option everywhere).

Ping architect when `dump` works; I'll wire it into `visualops-mcp`.
