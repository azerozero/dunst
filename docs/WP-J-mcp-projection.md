> **Historical note:** This file records an earlier VisualOps-era work package or review. Current crate names, setup commands, and status live in docs/README.md, docs/ARCHITECTURE.md, and docs/CONTRACTS.md.

# WP-J — get_scene_graph projection + latent affordance filter (owner: Claude / tmux %3)

Work **only** in `crates/visualops-mcp` (engine.rs + serve.rs). Do **not** touch
core / graph / platform / vision (no contract changes). No git. Keep
`cargo test -p visualops-mcp` green and add tests.

Both fixes come from the live E2E review of a real Claude client driving Notes.

## J1 — `get_scene_graph` is too heavy (516 nodes ≈ 340 KB / 14k lines)
A real MCP client can't take the full dump inline. Add a **projection**:
- `get_scene_graph` gains optional args: `view` = `"compact"` (default) | `"full"`
  | `"summary"`, and `actionable_only` (bool, default false).
  - **compact**: per node keep only `{id, role, label, value?, bbox, enabled,
    focused, parent, n_children}` — drop the heavy/derivable fields (`ax_role`
    string, `help`, `ax_actions`, `ax_identifier`, `last_seen_ms`, the full
    `children` vec → just a count). This should cut the payload by ~5–10×.
  - **summary**: no per-node list — return `{n_nodes, roots, counts_by_role,
    n_actionable, window}`. A cheap overview the agent can use before drilling in.
  - **full**: today's behaviour, unchanged (escape hatch).
  - `actionable_only`: with compact/full, include only nodes that pass the J2
    actionability test (on-screen, enabled, real bbox).
- Wire the args through serve.rs (tool inputSchema — remember `type:object`) and an
  `Engine::scene_graph_view(view, actionable_only)` method. Don't break the
  existing no-arg call path (default compact is fine, but keep `full` reachable).

## J2 — `query_affordances` / `get_affordances` over-promise (~380 "click")
~355 of those are collapsed-menu `mi_*` items at bbox `(0,0)`/off-window
(`y≈1440`) — only clickable after their parent menu opens. Filter them so the
agent isn't handed phantom targets.
- Define **latent / non-actionable** = bbox is `None` **or** zero area **or**
  positioned outside the window bounds (e.g. origin `(0,0)` with size 0, or `y`
  beyond the window height). Compute it in the engine from the node's bbox +
  the window rect — **no core/graph change**.
- `query_affordances` and `get_affordances` **filter out latent nodes by default**,
  with an opt-in `include_latent` (bool) to get everything.
- **CRITICAL — do not break two paths:**
  1. `find_element` must still find latent nodes by label (so an agent can still
     locate "Éteindre").
  2. The risk gate: `click_element` by id on a latent menu item must still work
     (the gate demo clicks `mi_shutdownnowrequested` found via `find_element`,
     not via `query_affordances`). So filter the **listing**, not the graph.

## Tests
- compact view omits the heavy fields and is materially smaller than full
  (assert a known node lacks `ax_actions`/`help` and has `n_children`).
- summary returns counts + roots, no node list.
- `query_affordances("click")` on the Notes fixture excludes a zero/None-bbox node
  but `include_latent=true` includes it; `find_element` still finds it.
- existing gating tests stay green.

Finish: `cargo test -p visualops-mcp`, a before/after size note for get_scene_graph
on the fixture, and a summary.
