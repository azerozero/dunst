//! Pure helpers for the UI-epoch fingerprint: bbox bucketing/hashing, transient
//! decoration detection, menu-bar exclusion, and structural-path filtering.
//!
//! Extracted verbatim from [`super`] (`read.rs`): every function here is
//! `self`-free, so grouping them keeps the epoch logic readable as one concern
//! and shrinks the former god-module. See `Engine::current_ui_epoch_fingerprint`
//! for the caller.

use std::hash::{Hash, Hasher};

use dunst_core::{AffordanceGraph, Bbox, Role, SceneGraph, SceneNode};

use crate::engine::query_support::normalize_match;
use crate::engine::scene_query::{normalized_selection_signal, option_selected_state};

use super::{BBOX_FINGERPRINT_QUANTUM, VISIBLE_FRACTION_FINGERPRINT_BUCKETS};

pub(super) fn hash_optional_bbox<H: Hasher>(bbox: Option<Bbox>, hasher: &mut H) {
    bbox.is_some().hash(hasher);
    if let Some(bbox) = bbox {
        hash_bbox(bbox, hasher);
    }
}

pub(super) fn hash_bbox<H: Hasher>(bbox: Bbox, hasher: &mut H) {
    bbox_fingerprint_bucket(bbox.x).hash(hasher);
    bbox_fingerprint_bucket(bbox.y).hash(hasher);
    bbox_fingerprint_bucket(bbox.w).hash(hasher);
    bbox_fingerprint_bucket(bbox.h).hash(hasher);
}

pub(super) fn rounded_i64(value: f64) -> i64 {
    value.round() as i64
}

pub(super) fn bbox_fingerprint_bucket(value: f64) -> i64 {
    (value / BBOX_FINGERPRINT_QUANTUM).round() as i64
}

pub(super) fn visible_fraction_bucket(value: f64) -> i64 {
    (value.clamp(0.0, 1.0) * VISIBLE_FRACTION_FINGERPRINT_BUCKETS).round() as i64
}

pub(super) fn node_epoch_hashes_id(
    affordance: Option<&dunst_core::Affordance>,
    control_state: Option<bool>,
) -> bool {
    control_state.is_some()
        || affordance.is_some_and(|affordance| {
            !affordance.actions.is_empty() || !affordance.drag_targets.is_empty()
        })
}

pub(super) fn node_epoch_control_state(node: &SceneNode) -> Option<bool> {
    match node.role {
        Role::Checkbox | Role::Radio => option_selected_state(node),
        Role::Button | Role::MenuButton | Role::MenuItem | Role::Row | Role::Cell => {
            node.value.as_deref().and_then(normalized_selection_signal)
        }
        _ if is_toggle_ax_role(&node.ax_role) => {
            node.value.as_deref().and_then(normalized_selection_signal)
        }
        _ => None,
    }
}

pub(super) fn is_toggle_ax_role(ax_role: &str) -> bool {
    ax_role.contains("Switch") || ax_role.contains("Toggle")
}

pub(super) fn transient_epoch_node_ids(
    graph: &SceneGraph,
    affordances: &AffordanceGraph,
) -> std::collections::BTreeSet<String> {
    graph
        .nodes
        .values()
        .filter(|node| {
            let affordance = affordances.affordances.get(&node.id);
            node_is_transient_epoch_decoration(node, affordance)
        })
        .map(|node| node.id.clone())
        .collect()
}

/// Whether `node` belongs to the app menu bar (a `MenuBar`/`Menu`/`MenuItem`, or
/// a control nested under one — e.g. the Help-menu `_SC_SEARCH_FIELD`). The menu
/// bar is excluded from the UI-epoch fingerprint: on a multi-display Mac it
/// follows the active screen, so its node bboxes flip between coordinate systems
/// across perceptions and would otherwise churn the epoch — wrongly rejecting a
/// just-approved raw gesture as "stale". Walks a bounded ancestry so a deep page
/// node stays cheap.
pub(super) fn node_in_menu_bar(
    graph: &SceneGraph,
    node: &SceneNode,
    menubar_root: Option<&str>,
) -> bool {
    if matches!(node.role, Role::MenuBar | Role::Menu | Role::MenuItem) {
        return true;
    }
    let mut current = node.parent.as_deref();
    for _ in 0..16 {
        let Some(parent_id) = current else {
            return false;
        };
        if menubar_root == Some(parent_id) {
            return true;
        }
        let Some(parent) = graph.get(parent_id) else {
            return false;
        };
        if matches!(parent.role, Role::MenuBar | Role::Menu | Role::MenuItem) {
            return true;
        }
        current = parent.parent.as_deref();
    }
    false
}

pub(super) fn node_is_transient_epoch_decoration(
    node: &SceneNode,
    affordance: Option<&dunst_core::Affordance>,
) -> bool {
    if !node.children.is_empty()
        || node_epoch_control_state(node).is_some()
        || affordance.is_some_and(|affordance| {
            !affordance.actions.is_empty() || !affordance.drag_targets.is_empty()
        })
    {
        return false;
    }

    let ax_role = normalize_match(&node.ax_role);
    let role_is_known_transient = [
        "axinsertionpoint",
        "axcaret",
        "axcursor",
        "axfocusring",
        "axtexthighlight",
        "axtextmarker",
        "axtextmarkerrange",
        "axselectedtextmarker",
        "axselectedtextmarkerrange",
        "axselection",
        "axselectionrange",
    ]
    .iter()
    .any(|needle| ax_role.contains(needle));
    if role_is_known_transient {
        return true;
    }

    let text_marks_transient = [
        node.label.as_deref(),
        node.value.as_deref(),
        node.help.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(normalize_match)
    .any(|text| {
        text.contains("insertion point")
            || text.contains("text cursor")
            || text.contains("caret")
            || text.contains("focus ring")
            || text.contains("selection highlight")
    });
    if text_marks_transient && node.role == Role::Unknown {
        return true;
    }

    node.role == Role::Unknown && node.bbox.is_some_and(|bbox| bbox.w <= 1.0 || bbox.h <= 1.0)
}

pub(super) fn epoch_filtered_path(
    graph: &SceneGraph,
    node: &SceneNode,
    transient_ids: &std::collections::BTreeSet<String>,
) -> Vec<usize> {
    let mut path = Vec::new();
    let mut current = Some(node.id.as_str());
    while let Some(id) = current {
        let siblings = graph
            .get(id)
            .and_then(|node| node.parent.as_deref())
            .and_then(|parent_id| graph.get(parent_id))
            .map(|parent| parent.children.as_slice())
            .unwrap_or(graph.roots.as_slice());
        let index = siblings
            .iter()
            .take_while(|sibling_id| sibling_id.as_str() != id)
            .filter(|sibling_id| !transient_ids.contains(sibling_id.as_str()))
            .count();
        path.push(index);
        current = graph.get(id).and_then(|node| node.parent.as_deref());
    }
    path.reverse();
    path
}
