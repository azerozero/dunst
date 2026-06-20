//! Scene Graph -> Affordance Graph.
//!
//! For each node: map its native `ax_actions` to [`SemanticAction`]s
//! ([`map_action`]), attach the [`RiskEngine`] assessment, and compute
//! `drag_targets` for draggable nodes (rows/cells) — for the POC, candidate
//! drop zones are sibling rows and ancestor list/table/outline containers.

use std::collections::{BTreeMap, BTreeSet};

use dunst_core::{Affordance, AffordanceGraph, Role, SceneGraph, SceneNode, SemanticAction};

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
        let mut drag_targets: Vec<String> = Vec::new();

        if node.enabled {
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

            // 2b. Scrollable roles / native AX scroll areas can be scrolled directly
            // through the platform backend when the app exposes an AX scrollbar.
            if matches!(
                node.role,
                Role::List | Role::Table | Role::Outline | Role::TextArea
            ) || node.ax_role == "AXScrollArea"
            {
                push_unique(&mut actions, SemanticAction::Scroll);
            }

            // 3. Drag targets (rows/cells only); a non-empty list exposes `Drag`.
            drag_targets = drag_targets_for(graph, node);
            if !drag_targets.is_empty() {
                push_unique(&mut actions, SemanticAction::Drag);
            }
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
/// zones are the sibling rows of the node's *row context* plus any ancestor
/// `List`/`Table`/`Outline`.
///
/// G4: for a `Cell`, the direct parent is its `Row`, so the row's siblings are
/// other cells — not rows. We first climb to the nearest ancestor `Row` and
/// enumerate *that* row's parent's other `Row` children. For a `Row` the node is
/// its own row context.
///
/// G6: membership is tracked with a `BTreeSet` (O(log n)) instead of repeated
/// `Vec::contains` (O(n²)); insertion order into the returned `Vec` stays
/// deterministic (sibling rows first, then ancestors).
fn drag_targets_for(graph: &SceneGraph, node: &SceneNode) -> Vec<String> {
    if !matches!(node.role, Role::Row | Role::Cell) {
        return Vec::new();
    }

    let mut targets: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    // Resolve the row context: the row itself, or (for a cell) the nearest
    // ancestor row.
    let row = match node.role {
        Role::Row => Some(node),
        _ => nearest_ancestor_row(graph, node),
    };

    // Sibling rows: the other `Row` children of the row context's parent.
    if let Some(row) = row {
        if let Some(parent_id) = &row.parent {
            if let Some(parent) = graph.get(parent_id) {
                for sibling_id in &parent.children {
                    if sibling_id == &row.id {
                        continue;
                    }
                    if matches!(graph.get(sibling_id).map(|s| s.role), Some(Role::Row)) {
                        push_unique_id(&mut targets, &mut seen, sibling_id.clone());
                    }
                }
            }
        }
    }

    // Ancestor containers (walk the parent chain from the node to the root).
    let mut current = node.parent.clone();
    while let Some(parent_id) = current {
        let Some(parent) = graph.get(&parent_id) else {
            break;
        };
        if matches!(parent.role, Role::List | Role::Table | Role::Outline) {
            push_unique_id(&mut targets, &mut seen, parent_id.clone());
        }
        current = parent.parent.clone();
    }

    targets
}

/// Climb the parent chain until the nearest [`Role::Row`] ancestor (the row a
/// cell belongs to). Returns `None` if there is none.
fn nearest_ancestor_row<'a>(graph: &'a SceneGraph, node: &SceneNode) -> Option<&'a SceneNode> {
    let mut current = node.parent.clone();
    while let Some(parent_id) = current {
        let parent = graph.get(&parent_id)?;
        if parent.role == Role::Row {
            return Some(parent);
        }
        current = parent.parent.clone();
    }
    None
}

fn push_unique(actions: &mut Vec<SemanticAction>, action: SemanticAction) {
    if !actions.contains(&action) {
        actions.push(action);
    }
}

fn push_unique_id(ids: &mut Vec<String>, seen: &mut BTreeSet<String>, id: String) {
    if seen.insert(id.clone()) {
        ids.push(id);
    }
}
