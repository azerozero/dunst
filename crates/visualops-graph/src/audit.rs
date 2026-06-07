//! Scene-graph diffing for `diff_since` and the audit trail.

use visualops_core::{GraphDiff, NodeChange, SceneGraph, SceneNode};

/// Structural diff `before -> after`, keyed by stable node ID:
/// - IDs only in `after`  -> [`NodeChange::Added`](visualops_core::NodeChange::Added)
/// - IDs only in `before` -> [`NodeChange::Removed`](visualops_core::NodeChange::Removed)
/// - IDs in both with differing `label` / `value` / `enabled` / `bbox`
///   -> one [`NodeChange::Changed`](visualops_core::NodeChange::Changed) per field.
///
/// `freshness_ms` / `last_seen_ms` are **ignored** (always differ). Output order
/// is deterministic: `before` is walked in `BTreeMap` key order (yielding
/// `Removed`/`Changed`), then `after`'s new IDs yield `Added`.
pub fn diff(before: &SceneGraph, after: &SceneGraph) -> GraphDiff {
    let mut changes = Vec::new();

    for (id, b) in &before.nodes {
        match after.nodes.get(id) {
            None => changes.push(NodeChange::Removed {
                id: id.clone(),
                label: b.label.clone(),
            }),
            Some(a) => collect_field_changes(id, b, a, &mut changes),
        }
    }

    for (id, a) in &after.nodes {
        if !before.nodes.contains_key(id) {
            changes.push(NodeChange::Added {
                id: id.clone(),
                label: a.label.clone(),
            });
        }
    }

    GraphDiff { changes }
}

/// Emit one [`NodeChange::Changed`] per differing semantic field. Timestamps,
/// confidence, source, focus and structural links are deliberately ignored.
fn collect_field_changes(id: &str, b: &SceneNode, a: &SceneNode, out: &mut Vec<NodeChange>) {
    if b.label != a.label {
        out.push(changed(id, "label", opt(&b.label), opt(&a.label)));
    }
    if b.value != a.value {
        out.push(changed(id, "value", opt(&b.value), opt(&a.value)));
    }
    if b.enabled != a.enabled {
        out.push(changed(id, "enabled", b.enabled.to_string(), a.enabled.to_string()));
    }
    if b.bbox != a.bbox {
        out.push(changed(id, "bbox", format!("{:?}", b.bbox), format!("{:?}", a.bbox)));
    }
}

fn changed(id: &str, field: &str, before: String, after: String) -> NodeChange {
    NodeChange::Changed {
        id: id.to_string(),
        field: field.to_string(),
        before,
        after,
    }
}

fn opt(value: &Option<String>) -> String {
    value.clone().unwrap_or_default()
}
