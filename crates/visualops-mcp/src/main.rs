//! VisualOps MCP server (stdio).
//!
//! Integration owner: architect. Wires a [`Perceptor`] + the `visualops-graph`
//! pipeline + an [`ActionExecutor`] behind the MCP tool surface:
//! `get_scene_graph`, `find_element`, `query_affordances`, `click_element`,
//! `type_into`, `hover_probe`, `verify_state`, `diff_since`, `export_trace`.
//!
//! Until the MCP runtime is wired in, this binary self-checks the pipeline on
//! the bundled Notes fixture so the scaffold is runnable end-to-end as the
//! worker crates fill in.

fn main() {
    eprintln!("visualops-mcp scaffold — pipeline wiring pending (see docs/ARCHITECTURE.md).");
}
