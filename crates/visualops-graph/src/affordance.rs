//! Scene Graph -> Affordance Graph.
//!
//! For each node: map its native `ax_actions` to [`SemanticAction`]s
//! ([`map_action`]), attach the [`RiskEngine`] assessment, and compute
//! `drag_targets` for draggable nodes (rows/cells) — for the POC, candidate
//! drop zones are sibling rows and ancestor list/table/outline containers.

use std::collections::BTreeMap;

use visualops_core::{Affordance, AffordanceGraph, Role, SceneGraph, SceneNode, SemanticAction};

use crate::risk::RiskEngine;

/// Map a native AX action verb to a semantic action. Returns `None` for verbs
/// with no agent-facing meaning (e.g. `showdefaultui`, `cancel`, `increment`).
///
/// Expected mapping: `press`/`confirm`->Click, `showmenu`->OpenMenu,
/// `pick`->Pick, `raise`->Raise; everything else -> `None`.
pub fn map_action(ax_action: &str) -> Option<SemanticAction> {
    match ax_action.to_ascii_lowercase().as_str() {
        "press" | "confirm" => Some(SemanticAction::Click),
        "showmenu" => Some(SemanticAction::OpenMenu),
        "pick" => Some(SemanticAction::Pick),
        "raise" => Some(SemanticAction::Raise),
        _ => None,
    }
}

/// Derive affordances for every node in the graph. Every node gets an entry so
/// the graph is complete; nodes with no actions are kept with empty `actions`.
pub fn derive_affordances(graph: &SceneGraph, risk: &RiskEngine) -> AffordanceGraph {
    let mut affordances: BTreeMap<String, Affordance> = BTreeMap::new();

    for (id, node) in &graph.nodes {
        let mut actions: Vec<SemanticAction> = Vec::new();

        // 1. Native AX verbs -> semantic actions (deduped, stable order).
        for verb in &node.ax_actions {
            if let Some(action) = map_action(verb) {
                push_unique(&mut actions, action);
            }
        }

        // 2. Text roles can be typed into and focused.
        if matches!(node.role, Role::TextField | Role::TextArea) {
            push_unique(&mut actions, SemanticAction::Type);
            push_unique(&mut actions, SemanticAction::Focus);
        }

        // 3. Drag targets (rows/cells only); a non-empty list exposes `Drag`.
        let drag_targets = drag_targets_for(graph, node);
        if !drag_targets.is_empty() {
            push_unique(&mut actions, SemanticAction::Drag);
        }

        affordances.insert(
            id.clone(),
            Affordance {
                id: id.clone(),
                actions,
                drag_targets,
                risk: risk.assess(node),
            },
        );
    }

    AffordanceGraph { affordances }
}

/// POC drag heuristic: only `Row`/`Cell` nodes are draggable. Candidate drop
/// zones are sibling rows plus any ancestor `List`/`Table`/`Outline`.
fn drag_targets_for(graph: &SceneGraph, node: &SceneNode) -> Vec<String> {
    if !matches!(node.role, Role::Row | Role::Cell) {
        return Vec::new();
    }

    let mut targets: Vec<String> = Vec::new();

    // Sibling rows (other children of this node's parent that are rows).
    if let Some(parent_id) = &node.parent {
        if let Some(parent) = graph.get(parent_id) {
            for sibling_id in &parent.children {
                if sibling_id == &node.id {
                    continue;
                }
                if matches!(graph.get(sibling_id).map(|s| s.role), Some(Role::Row)) {
                    push_unique_id(&mut targets, sibling_id.clone());
                }
            }
        }
    }

    // Ancestor containers (walk the parent chain to the root).
    let mut current = node.parent.clone();
    while let Some(parent_id) = current {
        let Some(parent) = graph.get(&parent_id) else {
            break;
        };
        if matches!(parent.role, Role::List | Role::Table | Role::Outline) {
            push_unique_id(&mut targets, parent_id.clone());
        }
        current = parent.parent.clone();
    }

    targets
}

fn push_unique(actions: &mut Vec<SemanticAction>, action: SemanticAction) {
    if !actions.contains(&action) {
        actions.push(action);
    }
}

fn push_unique_id(ids: &mut Vec<String>, id: String) {
    if !ids.contains(&id) {
        ids.push(id);
    }
}
