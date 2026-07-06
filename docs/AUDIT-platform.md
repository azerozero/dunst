> **Historical note:** This file records an earlier VisualOps-era work package or review. Current crate names, setup commands, and status live in docs/README.md, docs/ARCHITECTURE.md, and docs/CONTRACTS.md.

# AUDIT platform - visualops-platform + risk gate

Date: 2026-06-09
Scope: deep review of `crates/visualops-platform`, cross-cutting pass on `crates/visualops-mcp/src/engine.rs::act`.
Mode: audit only, no fixes applied.

## Prioritized Findings

### 1. `crates/visualops-mcp/src/serve.rs:206` - BLOCKING - `approve` is exposed as an unrestricted tool, so the gate is bypassable by the same agent

`approve` is an ordinary MCP tool that directly calls `engine.approve(&eid)` without proof of external approval, a separate capability, an operator challenge, or a role restriction. An agent that receives `PendingApproval` can simply call `approve` and then retry the action. The comment `High-risk elements return pending_approval until approve() is called` describes the mechanism, but not a real security barrier.

Proposed fix: separate approval from the agent action channel. Require a token/capability issued outside the model, bind approval to `(scene_generation, id, action, risk_hash, argument_hash)`, and reject `approve` from the same autonomous surface as `click_element`/`drag_element`.

### 2. `crates/visualops-mcp/src/engine.rs:290` - BLOCKING - approvals are persistent, unconsumed, and unvalidated

`approve(&mut self, id)` inserts any string into `BTreeSet<String>`. `act` then tests `self.approvals.contains(id)` but never consumes the approval. An approval remains valid for all future actions on the same id, after refresh, after a label/risk change, and can be pre-positioned for a nonexistent id that might appear later.

Proposed fix: make approval one-shot and consume it in `act`; validate that the id exists and that its current risk really requires approval at `approve` time; store the action, argument, scene generation, and hashed label/identifier/bbox, then invalidate on every `refresh`.

### 3. `crates/visualops-mcp/src/engine.rs:319` - BLOCKING - `drag_element` gates only the source risk, not the target risk

`drag_element` computes the bbox center of `target_id`, then calls `act(source_id, Drag, Some("x,y"))`. The `act` gate loads only the affordance and risk for `source_id`. A drag from a benign source to a destructive or irreversible target can therefore pass if the source is low-risk.

Proposed fix: evaluate composite risk for `Drag`: `max(risk(source), risk(target), risk(action+target_role+target_label))`. Approval must be bound to `(source_id, target_id, Drag, drop_point)`, not to the source alone.

### 4. `crates/visualops-platform/src/lib.rs:297` - BLOCKING - if the requested window disappears, the backend can act on `AXMainWindow` or the first process window

`resolve_window` looks for `requested_window_id`, then falls back to `AXMainWindow`, then to the first AX window. For an action, this can send a click/type/drag into another window in the same process if the targeted window was closed or replaced. This is a security and target-integrity problem.

Proposed fix: make resolution strict for `requested_window_id != 0`: if the id is not found, return a `WindowNotFound/WindowGone` error. Reserve the `AXMainWindow` fallback only for an explicitly wildcard target (`window_id == 0`) and audit it as such.

### 5. `crates/visualops-platform/src/lib.rs:71` - BLOCKING - `AX_CACHE` is thread-local global but not namespaced by `pid/window_id`

The cache stores `ElementKey -> AxElement` globally per thread. `ElementKey` contains neither `pid`, nor `window_id`, nor capture generation. Two `Engine`/targets in the same thread can pollute each other: capture B clears/fills the cache, then action A can touch an element from B if the key `(role,label,bbox,identifier)` collides.

Proposed fix: include `(pid, window_id, capture_generation, ElementKey)` in the key, or move the cache into an instance tied to the backend/target. Before any cached fast path, revalidate `_AXUIElementGetWindow(element) == target.window_id`.

### 6. `crates/visualops-mcp/src/engine.rs:343` - MAJOR - `act` executes on a potentially stale scene without pre-action refresh/revalidation

`act` clones the `SceneNode` and affordance from `current`, then executes immediately. If the UI changes between the last `refresh` and the action, the risk, action availability, drag bbox, and AX mapping may no longer match the real target. Re-perception happens after execution, too late to protect.

Proposed fix: before a non-read/probe action, revalidate the target: lightweight refresh or live AX lookup, confirm id/role/label/identifier/bbox/risk, then execute. For approved actions, invalidate if the generation or target hash changed.

### 7. `crates/visualops-mcp/src/engine.rs:385` - MAJOR - post-action `refresh()` failure is ignored

After `executor.perform`, the code does `let _ = self.refresh();` and then computes `diff_since`. If re-perception fails, the audit can still return `Success` with a false or stale diff. This weakens traceability and can hide an action that changed state.

Proposed fix: propagate the refresh error in the `AuditEntry` or introduce a `SuccessUnverified/VerificationFailed` result; do not produce a diff as if it were reliable.

### 8. `crates/visualops-platform/src/lib.rs:271` - MAJOR - the 1s AX timeout is applied only to the application, not to elements retained afterward

`AXUIElementSetMessagingTimeout(app, 1.0)` is called on the application element. The `AXUIElementRef`s extracted via attributes, arrays, cache, or fallback are not explicitly given this timeout. If the timeout is per-element, calls on children can block longer than expected.

Proposed fix: apply the timeout to every constructed `AxElement` (`app_element`, `attr_ax_element`, `ax_elements`, `retain_clone`) through an `AxElement::new_retained/non_null` helper that centralizes `AXUIElementSetMessagingTimeout`.

### 9. `crates/visualops-platform/src/lib.rs:773` - MAJOR - a partial drag can leave the target app in a "mouse down" state

`drag` posts `LeftMouseDown`, then several `LeftMouseDragged`, then `LeftMouseUp`. If `post_mouse` fails after the down, the closure returns before `LeftMouseUp`; the cursor is restored, but the target app may have received a down without an up. `CGEventPostToPid` returns no status, but event creation can fail.

Proposed fix: track `mouse_down_posted` and post a best-effort `LeftMouseUp` in cleanup/defer once a down has been emitted. Audit cleanup failure separately.

### 10. `crates/visualops-platform/src/lib.rs:827` - MAJOR - cursor restoration can move the cursor of a concurrent human user

`hover`/`drag` save the position and always call `CGDisplay::warp_mouse_cursor_position(saved)`. If the user physically moves the mouse during the ~64ms drag, the backend returns it to the old position. This is non-intrusive for the synthetic gesture, but intrusive for a concurrent human.

Proposed fix: restore only if the current position is near the synthetic trajectory or if synthetic movement was observed. Otherwise, do not warp and log `cursor_not_restored_user_moved`.

### 11. `crates/visualops-platform/src/lib.rs:715` - MAJOR - `post_to_pid` provides no acknowledgement; the backend can return `Ok` for an ignored event

Keyboard, hover, and drag paths post to the PID with `CGEventPostToPid`, which returns `void`. If the app ignores events because it is inactive, sandboxed, not key-window, or a control does not consume background events, `perform` still returns `Ok(())`. No-foreground behavior is preserved, but the audit can mark `Success` with no effect.

Proposed fix: for mutating actions, verify the expected effect through refresh/diff or a targeted AX probe. For hover/drag, return a "posted_unverified" status or add optional confirmation via graph/state change.

### 12. `crates/visualops-platform/src/lib.rs:677` - MAJOR - the `type_text` fallback can modify AX focus before posting to the PID

When `AXValue` is not settable or does not have the expected effect, the code calls `set_bool_attr(element, kAXFocusedAttribute, true)` before keystrokes. The WP treats Focus as non-intrusive, but depending on the app and control, this AX focus can change internal state, scroll, open a field, or even cause indirect activation.

Proposed fix: separate `TypeSetValue` and `TypeKeystrokes`; make the keyboard fallback opt-in by policy, revalidate that the app remained backgrounded, and explicitly audit the focus side effect.

### 13. `crates/visualops-mcp/src/engine.rs:300` - MAJOR - the risk gate ignores typed content

`type_into` gates only the target element risk. The text `argument` is never evaluated. On a low-risk but semantically dangerous field (terminal, command field, admin prompt, URL, search with shortcuts), destructive content can pass without approval.

Proposed fix: integrate `argument` into risk evaluation for `Type`, with policies by role/app: shell commands, AppleScript, sensitive URLs, destructive keywords, secrets, or multiline text with implicit enter.

### 14. `crates/visualops-graph/src/risk.rs:95` - MAJOR - risk classification is based only on label/help/identifier heuristics

Risk depends on keywords in label/help/identifier. A destructive element with no recognized keyword, icon-only UI, localization outside FR/EN, or app-renamed label will be low-risk. This is acceptable for a POC, not for a "non-bypassable" gate.

Proposed fix: add structural and contextual signals: menu item role under system menus, native action/identifier allow/deny lists, app bundle, position in "File/Edit" menus, OS confirmation, diff history, and per-action policies.

### 15. `crates/visualops-platform/src/lib.rs:561` - MAJOR - the cached fast path does not revalidate that the retained element still matches the `SceneNode`

`cached_element(key)` returns a retained `AXUIElementRef` and `perform_on_element` uses it directly. The only protection is fallback on stale errors (`kAXErrorInvalidUIElement`/`CannotComplete`). An AX element that is still valid but semantically different can receive the action.

Proposed fix: before a cached action, recompute `element_key(element)` and compare it to `key`, check `_AXUIElementGetWindow`, and ideally compare a minimal `(role,label,identifier,bbox,enabled)` hash against the current snapshot.

### 16. `crates/visualops-platform/src/lib.rs:516` - MINOR - `find_element` can fall back to a key collision

The live-search fallback uses `element_key(element) == ElementKey::from_scene(wanted)`. The key is useful but not unique: many elements can share a role, empty label, absent identifier, and rounded bbox. The first DFS match wins.

Proposed fix: enrich `ElementKey` with parent path/stable id, sibling index, window id, and/or multi-criteria scoring instead of strict equality on a small set of fields.

### 17. `crates/visualops-platform/src/lib.rs:134` - MINOR - global `clear_cache()` on every capture can invalidate concurrent actions

The cache is thread-local, but `capture` clears the entire cache for that thread regardless of target. In synchronous multi-engine use, a capture for one target can degrade or misdirect another target's fast path.

Proposed fix: namespace the cache by target/generation, or attach it to the `MacosBackend` instance instead of a `thread_local`.

### 18. `crates/visualops-platform/src/lib.rs:841` - MINOR - AX attribute errors are silently collapsed to `None`

`attr_value` returns `None` for every AX error. This simplifies capture, but hides the difference between absent attribute, timeout, permission, stale element, and system error. Capture can produce an incomplete graph without a reliable signal beyond `MAX_NODES/MAX_DEPTH`.

Proposed fix: in debug or metrics mode, count errors by AX code and expose them on stderr/trace; treat some codes (`CannotComplete`, timeout) as explicit degraded capture.

## FFI Points Without Blocking Findings

- `AxElement::retain_clone`/`Drop` (`crates/visualops-platform/src/lib.rs:111`, `:119`) correctly balance `CFRetain`/`CFRelease` for non-null refs.
- `attr_ax_element` (`crates/visualops-platform/src/lib.rs:873`) correctly transfers ownership of the create-rule `CFType` to `AxElement` through `mem::forget` after type-checking.
- `ax_elements` (`crates/visualops-platform/src/lib.rs:921`) explicitly retains borrowed values from a `CFArray` before constructing `AxElement`.
- `BatchValues` (`crates/visualops-platform/src/lib.rs:368`) keeps the `CFArray` alive while borrowed `CFTypeRef`s are used; no visible use-after-free on this path.
- `retain_core_graphics_image` does not exist in `visualops-platform`; the risk previously tied to this conversion is not applicable to this crate.

## Summary Verdict

The FFI backend is generally careful about retain/release, but system safety rests on invariants that are not encoded: global AX cache, permissive window resolution, stale scene, and overly broad approval. The risk gate is not non-bypassable today: an agent can call `approve`, a drag can bypass the target risk, and an approval persists without being consumed.
