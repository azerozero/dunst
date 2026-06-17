> **Historical note:** This file records an earlier VisualOps-era work package or review. Current crate names, setup commands, and status live in docs/README.md, docs/ARCHITECTURE.md, and docs/CONTRACTS.md.

# WP-K — actions without moving the cursor or fronting the window (owner: Codex / tmux %2)

Work **only** in `crates/visualops-platform`. Do **not** touch core / graph / mcp
/ vision. Do **not** change `ActionExecutor::perform`'s signature. No git. Keep
`cargo build -p visualops-platform` green and `examples/dump` working.

The product promise vs raw computer-use drivers is **non-intrusive** action: the
agent acts in the **background** — no window raised to the foreground, no visible
mouse-cursor movement. Today this holds for the AX path but **not** the CGEvent
path. Close that gap.

## Current state (confirm + document)
- `Click | Pick | OpenMenu | Focus` → `AXUIElementPerformAction` / set-attribute:
  **already** no cursor move, no fronting. Add a short comment asserting this.
- `Raise` → `AXRaiseAction`: fronts the window **by design** (that's its job) —
  leave it, just note it's the one intentional exception.
- `Hover` and `Drag` → `CGEvent…post(CGEventTapLocation::HID)`: **global** —
  these **move the real cursor** (`hover` MouseMoved; `drag` down/drag/up). The
  `Type` CGEvent fallback uses HID too (keyboard doesn't move the cursor but still
  shouldn't front).

## K1 — deliver mouse events to the target pid, not the global HID tap
- Thread `target.pid` into `hover`, `drag` (and the keyboard fallback in
  `type_text`) — `perform` already has `target: &Target`.
- Replace `event.post(CGEventTapLocation::HID)` with **`event.post_to_pid(pid)`**
  (`CGEventPostToPid`) so the event is delivered into the app's event stream
  without driving the global cursor / activating the app. (This is how background
  drivers dispatch without focus steal.)

## K2 — belt-and-braces: restore the cursor if it still moves
`post_to_pid` *should* leave the visible cursor where it was. If empirically it
still nudges, wrap the gesture: read the current location
(`CGEvent::new(source)?.location()` or `CGEventSourceGetPixelPositionFromBeforeEvent`),
do the synthetic gesture, then `CGWarpMouseCursorPosition(saved)` to put it back —
so any movement is imperceptible. Don't disassociate the mouse unless required.

## K3 — verify empirically (the user is watching the screen)
Drive a real `drag` and `hover` against Notes (via `examples/dump` flow or a tiny
example) and confirm: (a) the **visible cursor does not jump**, (b) **Notes does
not come to the foreground**. Report what you observed. If a fully invisible
synthetic drag isn't achievable with `post_to_pid`+warp, say so honestly and state
the residual.

## Out of scope
No new public types; no change to the `Drag` "x,y" contract; no core/mcp edits. If
you think the contract needs a change, STOP and write it in your summary.

Finish: `cargo build -p visualops-platform`, the empirical observation (cursor
still? window stayed background?), and a summary of what changed.
