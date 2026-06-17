> **Historical note:** This file records an earlier VisualOps-era work package or review. Current crate names, setup commands, and status live in docs/README.md, docs/ARCHITECTURE.md, and docs/CONTRACTS.md.

# WP-G — `visualops-mcp`: drag_element tool + fixes (owner: Claude / tmux %3)

Work **only** in `crates/visualops-mcp`. Do **not** touch `visualops-core` /
`visualops-graph` / `visualops-platform`. Do **not** run git. Keep
`cargo test -p visualops-mcp` green and add the new tests below.

## Frozen mini-contract for Drag (agreed with the platform side, WP-F)
The platform executes `Drag` as `perform(target, source_node,
SemanticAction::Drag, Some("x,y"))` where **`"x,y"` is the drop point in screen
coordinates** = the **destination** node's bbox centre. The engine computes that
string; the platform does the mouse motion from the source's bbox centre. You do
**not** change `core` or the `ActionExecutor` trait — `SemanticAction::Drag`
already exists and the mock executor records any action.

## G1 [MAIN] `Engine::drag_element` + `drag_element` MCP tool
Add a thin wrapper that reuses the existing gated `act()` path (engine.rs) — do
**not** write a second risk/audit path.

- `Engine::drag_element(&mut self, source_id: &str, target_id: &str, reasoning:
  Option<&str>) -> Result<AuditEntry>`:
  1. Look up the **target** node in the scene graph; take `bbox` (error
     `ElementNotFound`/a clear message if the id is unknown or has no bbox —
     a drop needs a concrete point).
  2. Compute the centre: `x = bbox.x + bbox.w/2.0`, `y = bbox.y + bbox.h/2.0`.
  3. `self.act(source_id, SemanticAction::Drag, Some(&format!("{x},{y}")),
     reasoning)`. `act()` already checks the source exposes `Drag`
     (`aff.actions.contains(&Drag)` → `ActionUnavailable` otherwise), gates on
     risk, runs the executor, re-perceives, diffs, and audits. Nothing else to do.
- `serve.rs`:
  - Add the tool to `tools_list()`:
    `drag_element` — "Drag a source element onto a target element by id (subject
    to risk gating)." inputSchema: required `source_id`, `target_id`; optional
    `reasoning`.
  - Add the dispatch arm in `handle_tool_call`: read `source_id`/`target_id`,
    call `engine.drag_element(...)`, map to the same `serde_json::to_value(entry)`
    /`isError` shape as `click_element`.

## G2 [FIX] `TOOL_COUNT` off-by-one
`serve.rs` has `const TOOL_COUNT: usize = 11;` but `tools_list()` returns 12 (soon
13 with G1). The startup banner prints the wrong number. Either derive it
(`tools_list().len()`) or bump it and keep it correct. Prefer deriving it so it
can't drift again.

## G3 [TESTS] Cover the new tool
In the engine tests (engine.rs `#[cfg(test)]`, which already build a
`MockPerceptor` + recording executor):
- `drag_element` on a source node that exposes `Drag` → the recorded executor call
  is `(source_id, Drag, Some("x,y"))` with `x,y` matching the **target**'s bbox
  centre; the returned `AuditEntry` has `action == Drag` and is in the trace.
- `drag_element` with an unknown `target_id` → `Err` (no audit entry).
- a source node **without** a `Drag` affordance → `ActionUnavailable` (gating
  holds).
- If the fixture has no node exposing `Drag`, pick a node from the affordance graph
  that does (the round-2 `drag_targets_for` produces `Drag` for table cells/rows in
  `MockPerceptor::notes_fixture()`), or assert via `query_affordances(Drag)` first.

## Bonus (optional, you now have the live MCP)
This session has the `visualops` MCP server approved (`mcp__visualops__*`). Once
WP-F lands on the platform side you can drive a real drag end-to-end through it,
but that is **not** required for this WP — the mock-backed engine tests are the
deliverable.

Finish: print `cargo test -p visualops-mcp`, confirm the banner tool count is
right, and summarise what changed.
