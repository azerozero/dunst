//! Scene Graph -> Affordance Graph.
//!
//! For each node: map its native `ax_actions` to [`SemanticAction`]s
//! ([`map_action`]), attach the [`RiskEngine`] assessment, and compute
//! `drag_targets` for draggable nodes (rows/cells/cards) — for the POC,
//! candidate drop zones are sibling rows / list/outline containers.

use visualops_core::{AffordanceGraph, SceneGraph, SemanticAction};

use crate::risk::RiskEngine;

/// Map a native AX action verb to a semantic action. Returns `None` for verbs
/// with no agent-facing meaning (e.g. `showdefaultui`).
///
/// Expected mapping: `press`->Click, `showmenu`->OpenMenu, `pick`->Pick,
/// `raise`->Raise, AXTextArea/AXTextField -> also expose Type, etc.
pub fn map_action(ax_action: &str) -> Option<SemanticAction> {
    let _ = ax_action;
    todo!("WP-B: ax verb -> SemanticAction")
}

/// Derive affordances for every node in the graph.
pub fn derive_affordances(graph: &SceneGraph, risk: &RiskEngine) -> AffordanceGraph {
    let _ = (graph, risk);
    todo!("WP-B: per-node actions + Type for text roles + drag_targets + risk")
}
