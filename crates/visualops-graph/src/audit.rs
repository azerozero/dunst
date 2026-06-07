//! Scene-graph diffing for `diff_since` and the audit trail.

use visualops_core::{Bbox, GraphDiff, NodeChange, SceneGraph, SceneNode};

/// Structural diff `before -> after`, keyed by stable node ID:
/// - IDs only in `after`  -> [`NodeChange::Added`](visualops_core::NodeChange::Added)
/// - IDs only in `before` -> [`NodeChange::Removed`](visualops_core::NodeChange::Removed)
/// - IDs in both with differing `label` / `value` / `enabled` / `bbox` /
///   `parent` / `children` -> one
///   [`NodeChange::Changed`](visualops_core::NodeChange::Changed) per field (G2).
/// - a change to [`SceneGraph::roots`] -> one `Changed { id: "<graph>", field:
///   "roots" }` (G2).
///
/// **Reconciliation (G3).** Because `synth_id` is label-derived, renaming a node
/// changes its ID, which would naively surface as `Removed` + `Added`. After the
/// naive pass, an unmatched `Removed`/`Added` pair that shares
/// `(parent, role, ax_identifier)` and a close bbox is rewritten into a single
/// `Changed { field: "label", before, after }` carrying the **new** id.
///
/// `freshness_ms` / `last_seen_ms` are **ignored** (always differ). Output order
/// is deterministic: field changes (in `before` key order), then
/// reconciled-label / leftover removed, then leftover added, then roots.
pub fn diff(before: &SceneGraph, after: &SceneGraph) -> GraphDiff {
    let mut changes = Vec::new();
    let mut removed: Vec<&SceneNode> = Vec::new();

    // Pass 1: field changes for common IDs; collect IDs gone from `after`.
    for (id, b) in &before.nodes {
        match after.nodes.get(id) {
            None => removed.push(b),
            Some(a) => collect_field_changes(id, b, a, &mut changes),
        }
    }

    // Collect IDs new in `after`.
    let added: Vec<&SceneNode> = after
        .nodes
        .iter()
        .filter(|(id, _)| !before.nodes.contains_key(*id))
        .map(|(_, node)| node)
        .collect();

    // Pass 2 (G3): reconcile removed<->added that are the same element renamed.
    let mut added_consumed = vec![false; added.len()];
    for &b in &removed {
        let mut matched = None;
        for (i, &a) in added.iter().enumerate() {
            if !added_consumed[i] && reconcilable(b, a) {
                matched = Some(i);
                break;
            }
        }

        match matched {
            Some(i) => {
                added_consumed[i] = true;
                let a = added[i];
                changes.push(changed(&a.id, "label", opt(&b.label), opt(&a.label)));
            }
            None => changes.push(NodeChange::Removed {
                id: b.id.clone(),
                label: b.label.clone(),
            }),
        }
    }

    // Leftover added (genuinely new nodes).
    for (i, a) in added.iter().enumerate() {
        if !added_consumed[i] {
            changes.push(NodeChange::Added {
                id: a.id.clone(),
                label: a.label.clone(),
            });
        }
    }

    // Root ordering / membership (G2).
    if before.roots != after.roots {
        changes.push(changed(
            "<graph>",
            "roots",
            before.roots.join(","),
            after.roots.join(","),
        ));
    }

    GraphDiff { changes }
}

/// Emit one [`NodeChange::Changed`] per differing field, including the structural
/// links `parent` and `children` (G2). Timestamps, confidence, source and focus
/// are deliberately ignored.
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
    if b.parent != a.parent {
        out.push(changed(id, "parent", opt(&b.parent), opt(&a.parent)));
    }
    if b.children != a.children {
        // Children IDs are already in document order -> deterministic encoding.
        out.push(changed(id, "children", b.children.join(","), a.children.join(",")));
    }
}

/// Two nodes (one removed, one added) are the **same element renamed** when their
/// structural anchors line up: same parent, role, AX identifier and a close bbox.
/// The label is intentionally excluded — it is the field that changed.
fn reconcilable(b: &SceneNode, a: &SceneNode) -> bool {
    b.parent == a.parent
        && b.role == a.role
        && b.ax_identifier == a.ax_identifier
        && bbox_close(b.bbox, a.bbox)
}

/// Bbox proximity for reconciliation: both absent matches, both present must be
/// within 1pt on every edge, mixed presence does not match.
fn bbox_close(a: Option<Bbox>, b: Option<Bbox>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            (x.x - y.x).abs() < 1.0
                && (x.y - y.y).abs() < 1.0
                && (x.w - y.w).abs() < 1.0
                && (x.h - y.h).abs() < 1.0
        }
        _ => false,
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
