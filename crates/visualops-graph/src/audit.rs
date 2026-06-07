//! Scene-graph diffing for `diff_since` and the audit trail.

use visualops_core::{GraphDiff, SceneGraph};

/// Structural diff `before -> after`, keyed by stable node ID:
/// - IDs only in `after`  -> [`NodeChange::Added`](visualops_core::NodeChange::Added)
/// - IDs only in `before` -> [`NodeChange::Removed`](visualops_core::NodeChange::Removed)
/// - IDs in both with differing `label` / `value` / `enabled` / `bbox`
///   -> one [`NodeChange::Changed`](visualops_core::NodeChange::Changed) per field.
///
/// `freshness_ms` / `last_seen_ms` are **ignored** (always differ). Output order
/// must be deterministic (iterate the `BTreeMap`).
pub fn diff(before: &SceneGraph, after: &SceneGraph) -> GraphDiff {
    let _ = (before, after);
    todo!("WP-B: deterministic structural diff, ignoring timestamps")
}
