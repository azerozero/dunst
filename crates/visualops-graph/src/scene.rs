//! Raw AX tree -> normalised [`SceneGraph`].
//!
//! Responsibilities (see WP-B for detail):
//! 1. Map native `ax_role` -> [`Role`] ([`map_role`]).
//! 2. Flatten the tree into `BTreeMap<id, SceneNode>` with `parent`/`children`
//!    wired by synthesised IDs and `roots` preserved in order.
//! 3. Synthesise **stable, human-readable IDs** ([`synth_id`]): `"btn_nouvelle_note"`.
//!    Must be deterministic and collision-free within one graph.
//! 4. Set `confidence = 1.0` and `source = Source::Accessibility` for AX nodes,
//!    `last_seen_ms = now_ms`.

use visualops_core::{RawAxNode, Role, SceneGraph, SceneNode, WindowRef};

/// Map a native AX role string to a normalised [`Role`]. Unknown roles map to
/// [`Role::Unknown`] (the original string stays in `SceneNode::ax_role`).
pub fn map_role(ax_role: &str) -> Role {
    let _ = ax_role;
    todo!("WP-B: AXButton->Button, AXTextArea->TextArea, AXMenuItem->MenuItem, ... else Unknown")
}

/// Synthesise a stable, human-readable, unique-within-graph ID for a node.
/// Format: `"{role_prefix}_{slug(label)}"`, falling back to a short hash of the
/// structural path when no label exists. `used` tracks already-emitted IDs so a
/// numeric suffix can disambiguate collisions (`btn_partager`, `btn_partager_2`).
pub fn synth_id(role: Role, label: Option<&str>, path: &[usize], used: &std::collections::BTreeSet<String>) -> String {
    let _ = (role, label, path, used);
    todo!("WP-B: stable id synthesis; deterministic; collision-free via `used`")
}

/// Build the full scene graph from perceived roots.
pub fn build_scene_graph(roots: Vec<RawAxNode>, window: WindowRef, now_ms: u64) -> SceneGraph {
    let _ = (roots, window, now_ms);
    todo!("WP-B: recursive flatten + id synthesis + parent/child wiring")
}

// Helper expected by tests; WP-B may keep or inline it.
#[allow(dead_code)]
fn _node_marker(_n: &SceneNode) {}
