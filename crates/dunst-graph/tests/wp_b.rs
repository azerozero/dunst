//! WP-B done-criteria, exercised against the device-free Notes fixture.
//!
//! `MockPerceptor::notes_fixture()` replays a captured AX tree (no macOS),
//! which we flatten into a `SceneGraph`, derive affordances/risk from, and diff.

use std::collections::BTreeSet;

use dunst_core::mock::MockPerceptor;
use dunst_core::{
    NodeChange, Perceptor, RiskLevel, Role, SceneGraph, SceneNode, SemanticAction, Target,
};
use dunst_graph::{build_scene_graph, derive_affordances, diff, RiskEngine};

/// Number of `RawAxNode`s in `fixtures/notes.json` (window subtree: 11,
/// menu-bar subtree: 11).
const FIXTURE_NODE_COUNT: usize = 22;

fn build(now_ms: u64) -> SceneGraph {
    let perceptor = MockPerceptor::notes_fixture().expect("fixture loads");
    let target = Target {
        pid: 1363,
        window_id: 105,
    };
    let roots = perceptor.capture(&target).expect("capture");
    let window = perceptor.window_ref(&target).expect("window_ref");
    build_scene_graph(roots, window, now_ms)
}

fn node_by_label<'a>(graph: &'a SceneGraph, label: &str) -> &'a SceneNode {
    graph
        .nodes
        .values()
        .find(|n| n.label.as_deref() == Some(label))
        .unwrap_or_else(|| panic!("no node with label {label:?}"))
}

fn node_by_role(graph: &SceneGraph, role: Role) -> &SceneNode {
    graph
        .nodes
        .values()
        .find(|n| n.role == role)
        .unwrap_or_else(|| panic!("no node with role {role:?}"))
}

// 1. build_scene_graph produces btn_nouvelle_note (Button, "Nouvelle note", 1.0).
#[test]
fn builds_nouvelle_note_button() {
    let graph = build(1_000);
    let node = graph
        .get("btn_nouvelle_note")
        .expect("btn_nouvelle_note exists");
    assert_eq!(node.role, Role::Button);
    assert_eq!(node.label.as_deref(), Some("Nouvelle note"));
    assert_eq!(node.confidence, 1.0);
    assert_eq!(node.last_seen_ms, 1_000);
    assert_eq!(graph.captured_at_ms, 1_000);
}

// 2. All synthesised IDs are unique (every raw node survives into the map).
#[test]
fn all_ids_unique() {
    let graph = build(1_000);
    assert_eq!(
        graph.nodes.len(),
        FIXTURE_NODE_COUNT,
        "a collision would drop a node from the id-keyed map"
    );
    let unique: BTreeSet<&String> = graph.nodes.keys().collect();
    assert_eq!(unique.len(), graph.nodes.len());

    // Every parent/child link resolves, and roots are recorded in order.
    assert_eq!(graph.roots.len(), 2);
    for node in graph.nodes.values() {
        if let Some(parent) = &node.parent {
            assert!(graph.get(parent).is_some(), "dangling parent {parent}");
        }
        for child in &node.children {
            assert!(graph.get(child).is_some(), "dangling child {child}");
        }
    }
}

// 3. The text area node exposes Type in its affordance actions.
#[test]
fn text_area_exposes_type() {
    let graph = build(1_000);
    let affordances = derive_affordances(&graph, &RiskEngine::new());
    let text_area = node_by_role(&graph, Role::TextArea);
    let affordance = affordances
        .affordances
        .get(&text_area.id)
        .expect("text area affordance");
    assert!(affordance.actions.contains(&SemanticAction::Type));
    assert!(affordance.actions.contains(&SemanticAction::Focus));
    assert!(affordance.actions.contains(&SemanticAction::Scroll));
}

// 4. Risk tiers: destructive items are High + require approval; benign are Low.
#[test]
fn risk_tiers() {
    let graph = build(1_000);
    let engine = RiskEngine::new();

    for label in [
        "Supprimer",
        "Éteindre",
        "Forcer à quitter Notes",
        "Redémarrer…",
    ] {
        let assessment = engine.assess(node_by_label(&graph, label));
        assert_eq!(assessment.level, RiskLevel::High, "{label} should be High");
        assert!(
            assessment.requires_approval,
            "{label} should require approval"
        );
        assert!(
            !assessment.reasons.is_empty(),
            "{label} should record a reason"
        );
    }

    for label in ["Copier", "Nouvelle note"] {
        let assessment = engine.assess(node_by_label(&graph, label));
        assert_eq!(assessment.level, RiskLevel::Low, "{label} should be Low");
        assert!(
            !assessment.requires_approval,
            "{label} must not require approval"
        );
    }
}

// 5. The Notes AXCell/AXRow have non-empty drag_targets and a Drag action.
#[test]
fn rows_and_cells_are_draggable() {
    let graph = build(1_000);
    let affordances = derive_affordances(&graph, &RiskEngine::new());

    for role in [Role::Cell, Role::Row] {
        let node = node_by_role(&graph, role);
        let affordance = affordances.affordances.get(&node.id).unwrap();
        assert!(
            !affordance.drag_targets.is_empty(),
            "{role:?} should have drag targets"
        );
        assert!(
            affordance.actions.contains(&SemanticAction::Drag),
            "{role:?} should expose Drag"
        );
    }
}

// 6. diff: value change -> one Changed{value}; add/remove -> Added/Removed;
//    timestamp-only differences -> no changes.
#[test]
fn diff_detects_field_add_remove_and_ignores_timestamps() {
    let graph = build(1_000);

    // Identical clone: no changes.
    assert!(diff(&graph, &graph.clone()).is_empty());

    // Same fixture captured at a different instant: only timestamps differ.
    let later = build(9_999);
    assert!(
        diff(&graph, &later).is_empty(),
        "timestamp-only differences must produce no changes"
    );

    let target_id = "btn_nouvelle_note".to_string();

    // Value change -> exactly one Changed { field: "value" }.
    let mut changed_value = graph.clone();
    changed_value.nodes.get_mut(&target_id).unwrap().value = Some("hello".into());
    let result = diff(&graph, &changed_value);
    assert_eq!(result.changes.len(), 1);
    match &result.changes[0] {
        NodeChange::Changed {
            id, field, after, ..
        } => {
            assert_eq!(id, &target_id);
            assert_eq!(field, "value");
            assert_eq!(after, "hello");
        }
        other => panic!("expected Changed, got {other:?}"),
    }

    // Removed node -> exactly one Removed.
    let mut removed = graph.clone();
    removed.nodes.remove(&target_id);
    let result = diff(&graph, &removed);
    assert_eq!(result.changes.len(), 1);
    assert!(matches!(&result.changes[0], NodeChange::Removed { id, .. } if id == &target_id));

    // Added node (reverse direction) -> exactly one Added.
    let result = diff(&removed, &graph);
    assert_eq!(result.changes.len(), 1);
    assert!(matches!(&result.changes[0], NodeChange::Added { id, .. } if id == &target_id));
}
