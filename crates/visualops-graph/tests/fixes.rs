//! Synthetic unit tests for the FIX round-2 items (G1..G7), independent of the
//! Notes fixture where possible. Fixture-backed cases (G2/G3 reconciliation) use
//! `MockPerceptor::notes_fixture()` for realistic parent/children wiring.

use std::collections::BTreeSet;

use visualops_core::mock::MockPerceptor;
use visualops_core::{
    NodeChange, Perceptor, RawAxNode, RiskLevel, Role, SceneGraph, SemanticAction, Target, WindowRef,
};
use visualops_graph::scene::synth_id;
use visualops_graph::{build_scene_graph, derive_affordances, diff, RiskEngine};

/// Minimal `RawAxNode` builder for synthetic trees.
fn raw(role: &str, label: Option<&str>, children: Vec<RawAxNode>) -> RawAxNode {
    RawAxNode {
        ax_role: role.to_string(),
        label: label.map(str::to_string),
        help: None,
        value: None,
        ax_identifier: None,
        ax_actions: Vec::new(),
        frame: None,
        enabled: true,
        focused: false,
        children,
    }
}

fn fixture_graph() -> SceneGraph {
    let perceptor = MockPerceptor::notes_fixture().expect("fixture loads");
    let target = Target { pid: 1363, window_id: 105 };
    let roots = perceptor.capture(&target).expect("capture");
    let window = perceptor.window_ref(&target).expect("window_ref");
    build_scene_graph(roots, window, 1_000)
}

// ---------------------------------------------------------------------------
// G7 + G2-id-synthesis — synth_id: hash fallback width, empty/punct labels, collisions
// ---------------------------------------------------------------------------

#[test]
fn synth_id_hash_fallback_is_16_hex() {
    let used = BTreeSet::new();

    // No label -> hash fallback, 16 hex chars (G7).
    let id = synth_id(Role::Menu, None, &[1, 0, 0], &used);
    assert!(id.starts_with("menu_"), "got {id}");
    assert_eq!(id.len(), "menu_".len() + 16, "expected 16-hex hash: {id}");
    assert!(id["menu_".len()..].chars().all(|c| c.is_ascii_hexdigit()));

    // Empty label -> hash fallback.
    let id = synth_id(Role::TextArea, Some(""), &[0, 2], &used);
    assert!(id.starts_with("text_"));
    assert_eq!(id.len(), "text_".len() + 16);

    // Punctuation-only label (slugs to nothing) -> hash fallback.
    let id = synth_id(Role::Button, Some("!!! … —"), &[3], &used);
    assert!(id.starts_with("btn_"));
    assert_eq!(id.len(), "btn_".len() + 16);
}

#[test]
fn synth_id_resolves_collisions_with_numeric_suffix() {
    let mut used = BTreeSet::new();

    let id1 = synth_id(Role::Button, Some("Partager"), &[0, 3], &used);
    assert_eq!(id1, "btn_partager");
    used.insert(id1);

    // Same label, different path -> base collides -> `_2`.
    let id2 = synth_id(Role::Button, Some("Partager"), &[0, 4], &used);
    assert_eq!(id2, "btn_partager_2");
    used.insert(id2);

    let id3 = synth_id(Role::Button, Some("Partager"), &[1], &used);
    assert_eq!(id3, "btn_partager_3");
}

#[test]
fn synth_id_distinct_paths_distinct_hashes() {
    let used = BTreeSet::new();
    let a = synth_id(Role::Menu, None, &[1, 0, 0], &used);
    let b = synth_id(Role::Menu, None, &[1, 1, 0], &used);
    assert_ne!(a, b, "distinct structural paths must hash differently");
}

// ---------------------------------------------------------------------------
// G1 — NFD-decomposed accents fold to HIGH risk (tested through assess)
// ---------------------------------------------------------------------------

#[test]
fn risk_high_on_decomposed_accents() {
    let engine = RiskEngine::new();
    for label in ["E\u{301}teindre", "Re\u{301}initialiser"] {
        let graph = build_scene_graph(
            vec![raw("AXMenuItem", Some(label), vec![])],
            WindowRef::default(),
            0,
        );
        let node = graph.nodes.values().next().expect("one node");
        let assessment = engine.assess(node);
        assert_eq!(
            assessment.level,
            RiskLevel::High,
            "decomposed {label:?} must fold to HIGH"
        );
        assert!(assessment.requires_approval);
    }

    // NFC and NFD inputs assess identically.
    let nfc = build_scene_graph(vec![raw("AXMenuItem", Some("Éteindre"), vec![])], WindowRef::default(), 0);
    let nfd = build_scene_graph(vec![raw("AXMenuItem", Some("E\u{301}teindre"), vec![])], WindowRef::default(), 0);
    let nfc_level = engine.assess(nfc.nodes.values().next().unwrap()).level;
    let nfd_level = engine.assess(nfd.nodes.values().next().unwrap()).level;
    assert_eq!(nfc_level, nfd_level);
}

// ---------------------------------------------------------------------------
// G4 — a Cell's drag_targets contain its *sibling rows*, not its row's cells
// ---------------------------------------------------------------------------

#[test]
fn cell_drag_targets_include_sibling_rows() {
    let table = raw(
        "AXTable",
        None,
        vec![
            raw("AXRow", None, vec![raw("AXCell", Some("Alpha"), vec![])]),
            raw("AXRow", None, vec![raw("AXCell", Some("Beta"), vec![])]),
        ],
    );
    let graph = build_scene_graph(vec![table], WindowRef::default(), 0);
    let affordances = derive_affordances(&graph, &RiskEngine::new());

    let cell_a = graph.get("cell_alpha").expect("cell Alpha");
    let cell_b = graph.get("cell_beta").expect("cell Beta");
    let row_a = cell_a.parent.clone().expect("Alpha has a row");
    let row_b = cell_b.parent.clone().expect("Beta has a row");
    let table_id = graph
        .nodes
        .values()
        .find(|n| n.role == Role::Table)
        .unwrap()
        .id
        .clone();
    assert_ne!(row_a, row_b);

    let ta = affordances.affordances.get(&cell_a.id).unwrap();
    let tb = affordances.affordances.get(&cell_b.id).unwrap();

    // Each cell targets the *other* row (its sibling), never its own row.
    assert!(ta.drag_targets.contains(&row_b), "Alpha -> sibling row B: {:?}", ta.drag_targets);
    assert!(!ta.drag_targets.contains(&row_a), "Alpha must not target its own row");
    assert!(ta.drag_targets.contains(&table_id), "Alpha -> ancestor table");
    assert!(ta.actions.contains(&SemanticAction::Drag));

    assert!(tb.drag_targets.contains(&row_a), "Beta -> sibling row A: {:?}", tb.drag_targets);
    assert!(!tb.drag_targets.contains(&row_b), "Beta must not target its own row");
    assert!(tb.actions.contains(&SemanticAction::Drag));

    // No duplicate targets (G6 dedup).
    let unique: BTreeSet<_> = ta.drag_targets.iter().collect();
    assert_eq!(unique.len(), ta.drag_targets.len());
}

// ---------------------------------------------------------------------------
// G2 — diff reports parent / children / roots changes
// ---------------------------------------------------------------------------

#[test]
fn diff_reports_parent_change() {
    let g1 = fixture_graph();
    let mut g2 = g1.clone();
    g2.nodes.get_mut("btn_nouvelle_note").unwrap().parent = Some("grp_elsewhere".into());

    let d = diff(&g1, &g2);
    assert_eq!(d.changes.len(), 1, "{:?}", d.changes);
    assert!(matches!(
        &d.changes[0],
        NodeChange::Changed { id, field, .. } if id == "btn_nouvelle_note" && field == "parent"
    ));
}

#[test]
fn diff_reports_children_change() {
    let g1 = fixture_graph();
    let toolbar_id = g1
        .nodes
        .values()
        .find(|n| n.role == Role::Toolbar)
        .unwrap()
        .id
        .clone();

    let mut g2 = g1.clone();
    g2.nodes.get_mut(&toolbar_id).unwrap().children.push("phantom".into());

    let d = diff(&g1, &g2);
    assert_eq!(d.changes.len(), 1, "{:?}", d.changes);
    assert!(matches!(
        &d.changes[0],
        NodeChange::Changed { id, field, .. } if *id == toolbar_id && field == "children"
    ));
}

#[test]
fn diff_reports_roots_change() {
    let g1 = fixture_graph();
    let mut g2 = g1.clone();
    g2.roots.push("extra_root".into());

    let d = diff(&g1, &g2);
    assert_eq!(d.changes.len(), 1, "{:?}", d.changes);
    assert!(matches!(
        &d.changes[0],
        NodeChange::Changed { id, field, .. } if id == "<graph>" && field == "roots"
    ));
}

// ---------------------------------------------------------------------------
// G3 — a label rename (which changes the ID) reconciles into Changed{label}
// ---------------------------------------------------------------------------

#[test]
fn diff_reconciles_label_rename_as_changed() {
    let g1 = fixture_graph();
    let mut g2 = g1.clone();

    let old_id = "btn_nouvelle_note";
    let new_id = "btn_note_renommee";

    // Simulate a rebuild after the label changed: the ID changes, and the
    // parent's children list is rewired to the new ID.
    let mut node = g2.nodes.remove(old_id).expect("node exists");
    let parent_id = node.parent.clone().expect("has parent");
    node.id = new_id.to_string();
    node.label = Some("Note renommée".to_string());
    for child in g2.nodes.get_mut(&parent_id).unwrap().children.iter_mut() {
        if child == old_id {
            *child = new_id.to_string();
        }
    }
    g2.nodes.insert(new_id.to_string(), node);

    let d = diff(&g1, &g2);

    // The rename must NOT surface as Add/Remove.
    assert!(
        !d.changes
            .iter()
            .any(|c| matches!(c, NodeChange::Added { .. } | NodeChange::Removed { .. })),
        "rename leaked as Add/Remove: {:?}",
        d.changes
    );

    // Exactly one Changed{field:"label"} carrying the new ID and before/after.
    let label_changes: Vec<_> = d
        .changes
        .iter()
        .filter(|c| matches!(c, NodeChange::Changed { field, .. } if field == "label"))
        .collect();
    assert_eq!(label_changes.len(), 1, "{:?}", d.changes);
    match label_changes[0] {
        NodeChange::Changed { id, before, after, .. } => {
            assert_eq!(id, new_id);
            assert_eq!(before, "Nouvelle note");
            assert_eq!(after, "Note renommée");
        }
        _ => unreachable!(),
    }
}
