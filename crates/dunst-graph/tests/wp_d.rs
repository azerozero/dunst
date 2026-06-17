//! WP-D done-criteria (D3): `ax_identifier`-stable id synthesis.
//!
//! Covers the three D3 cases:
//! 1. a node **with** a developer-assigned `ax_identifier` derives its id from it
//!    and keeps that id across a label rename, so `diff` yields exactly one
//!    `Changed{label}` (no Add/Remove);
//! 2. a node **without** a usable identifier keeps **exactly** today's
//!    label-slug / path-hash id — locked by a snapshot of the Notes fixture
//!    (incl. `btn_nouvelle_note`);
//! 3. two siblings sharing one `ax_identifier` disambiguate with a `_2` suffix.
//!
//! Policy note: AppKit auto-generates ordinal `_NS:<n>` identifiers that are not
//! stable across launches/versions, so they are **excluded** from id synthesis
//! and such nodes fall back to the label scheme unchanged (e.g. `mi_copier`, not
//! `mi_ns_573`). Only genuinely developer-assigned identifiers drive ids.

use std::collections::BTreeSet;

use dunst_core::mock::MockPerceptor;
use dunst_core::{NodeChange, Perceptor, RawAxNode, Role, SceneGraph, Target, WindowRef};
use dunst_graph::scene::synth_id;
use dunst_graph::{build_scene_graph, diff};

/// `RawAxNode` builder that can set an `ax_identifier`.
fn raw(
    role: &str,
    label: Option<&str>,
    ax_identifier: Option<&str>,
    children: Vec<RawAxNode>,
) -> RawAxNode {
    RawAxNode {
        ax_role: role.to_string(),
        label: label.map(str::to_string),
        help: None,
        value: None,
        ax_identifier: ax_identifier.map(str::to_string),
        ax_actions: Vec::new(),
        frame: None,
        enabled: true,
        focused: false,
        children,
    }
}

fn fixture_graph() -> SceneGraph {
    let perceptor = MockPerceptor::notes_fixture().expect("fixture loads");
    let target = Target {
        pid: 1363,
        window_id: 105,
    };
    let roots = perceptor.capture(&target).expect("capture");
    let window = perceptor.window_ref(&target).expect("window_ref");
    build_scene_graph(roots, window, 1_000)
}

// ---------------------------------------------------------------------------
// D1 unit — synth_id source priority: stable identifier > label > path-hash,
// with the AppKit `_NS:<n>` auto-pattern excluded.
// ---------------------------------------------------------------------------

#[test]
fn synth_id_prefers_developer_identifier() {
    let used = BTreeSet::new();

    // Developer-assigned identifier wins over the label (and keeps the role
    // prefix + slugging so the id stays glanceable).
    assert_eq!(
        synth_id(Role::Button, Some("Save"), Some("save-btn"), &[0], &used),
        "btn_save_btn"
    );

    // A real selector-style identifier is honoured.
    assert_eq!(
        synth_id(
            Role::MenuItem,
            Some("Forcer à quitter"),
            Some("_forceQuitRequested:"),
            &[7],
            &used
        ),
        "mi_forcequitrequested"
    );
}

#[test]
fn synth_id_ignores_appkit_auto_identifiers() {
    let used = BTreeSet::new();

    // `_NS:<digits>` is AppKit-auto -> excluded -> falls back to the label.
    assert_eq!(
        synth_id(Role::MenuItem, Some("Copier"), Some("_NS:573"), &[0], &used),
        "mi_copier"
    );
    assert_eq!(
        synth_id(
            Role::Window,
            Some("Notes – Aucune note"),
            Some("_NS:6"),
            &[0],
            &used
        ),
        "win_notes_aucune_note"
    );

    // Empty / punctuation-only identifiers are unusable -> fall back to label.
    assert_eq!(
        synth_id(Role::Button, Some("Save"), Some(""), &[0], &used),
        "btn_save"
    );
    assert_eq!(
        synth_id(Role::Button, Some("Save"), Some("###"), &[0], &used),
        "btn_save"
    );

    // `_NS:` with a non-numeric tail is NOT the auto pattern -> treated as a real
    // identifier (guards the digit-only check).
    assert_eq!(
        synth_id(Role::Button, Some("Save"), Some("_NS:custom"), &[0], &used),
        "btn_ns_custom"
    );

    // No label and only an excluded identifier -> path-hash fallback (unchanged).
    let id = synth_id(Role::Menu, None, Some("_NS:332"), &[1], &used);
    assert!(id.starts_with("menu_"));
    assert_eq!(
        id.len(),
        "menu_".len() + 16,
        "expected 16-hex path hash: {id}"
    );
}

// ---------------------------------------------------------------------------
// D3.1 — an identifier-backed node keeps its id across a rename; diff -> one
// Changed{label}, no Add/Remove. (D2: reconciliation is not involved.)
// ---------------------------------------------------------------------------

#[test]
fn identifier_backed_id_is_stable_across_rename() {
    let before = build_scene_graph(
        vec![raw(
            "AXButton",
            Some("Enregistrer"),
            Some("save-btn"),
            vec![],
        )],
        WindowRef::default(),
        1_000,
    );
    let after = build_scene_graph(
        vec![raw(
            "AXButton",
            Some("Sauvegardé"),
            Some("save-btn"),
            vec![],
        )],
        WindowRef::default(),
        2_000,
    );

    // Same id on both sides: derived from the identifier, not the (changed) label.
    assert!(
        before.get("btn_save_btn").is_some(),
        "id should derive from identifier"
    );
    assert_eq!(
        before.roots, after.roots,
        "root id must be unchanged by the rename"
    );
    assert!(after.get("btn_save_btn").is_some());

    let d = diff(&before, &after);

    // No Add/Remove: the id was stable, so this is a plain field change.
    assert!(
        !d.changes
            .iter()
            .any(|c| matches!(c, NodeChange::Added { .. } | NodeChange::Removed { .. })),
        "stable id must not surface as Add/Remove: {:?}",
        d.changes
    );

    // Exactly one Changed{field:"label"} carrying the stable id.
    assert_eq!(d.changes.len(), 1, "{:?}", d.changes);
    match &d.changes[0] {
        NodeChange::Changed {
            id,
            field,
            before,
            after,
        } => {
            assert_eq!(id, "btn_save_btn");
            assert_eq!(field, "label");
            assert_eq!(before, "Enregistrer");
            assert_eq!(after, "Sauvegardé");
        }
        other => panic!("expected Changed{{label}}, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// D3.3 — two siblings sharing one ax_identifier -> `_2` suffix.
// ---------------------------------------------------------------------------

#[test]
fn shared_identifier_collides_with_numeric_suffix() {
    let graph = build_scene_graph(
        vec![raw(
            "AXToolbar",
            None,
            None,
            vec![
                raw("AXButton", Some("First"), Some("dup"), vec![]),
                raw("AXButton", Some("Second"), Some("dup"), vec![]),
            ],
        )],
        WindowRef::default(),
        0,
    );

    assert!(
        graph.get("btn_dup").is_some(),
        "first sibling takes the base id"
    );
    assert!(
        graph.get("btn_dup_2").is_some(),
        "second sibling disambiguates with _2"
    );
    // Both survived into the id-keyed map (no silent drop).
    assert_eq!(
        graph
            .nodes
            .values()
            .filter(|n| n.role == Role::Button)
            .count(),
        2
    );
}

// ---------------------------------------------------------------------------
// D3.2 — Notes-fixture ids: label-derived ids are byte-identical to today, and
// only genuinely developer-assigned identifiers change an id. Locks the set.
// ---------------------------------------------------------------------------

/// Split `id` into (prefix, rest) at the first `_`; a path-hash id is exactly
/// `{prefix}_{16 hex}` (no further `_`), which lets us snapshot the human-readable
/// ids without pinning the structural hashes.
fn is_path_hash_id(id: &str) -> bool {
    match id.split_once('_') {
        Some((_, rest)) => rest.len() == 16 && rest.bytes().all(|b| b.is_ascii_hexdigit()),
        None => false,
    }
}

#[test]
fn notes_fixture_label_derived_ids_are_unchanged() {
    let graph = fixture_graph();

    // The label-derived ids that existed before WP-D must be byte-identical.
    // `_NS:<n>`-identified nodes are included here precisely because the AppKit
    // auto-pattern is excluded, so they keep their human-readable label id.
    for id in [
        "win_notes_aucune_note", // window, ax_identifier "_NS:6" (excluded)
        "btn_ajouter_un_dossier",
        "btn_nouvelle_note", // the locked id from WP-D
        "mbtn_multimedia",
        "btn_partager",
        "btn_rechercher",
        "outline_dossiers", // ax_identifier "_NS:50" (excluded)
        "cell_notes",
        "menubar_edition",
        "menubar_apple",
        "mi_copier",    // ax_identifier "_NS:573" (excluded)
        "mi_coller",    // ax_identifier "_NS:581" (excluded)
        "mi_supprimer", // ax_identifier "_NS:598" (excluded)
    ] {
        assert!(
            graph.get(id).is_some(),
            "label-derived id {id:?} must still be produced"
        );
    }

    // The AppKit-auto ids must NOT have leaked into synthesis.
    for leaked in [
        "mi_ns_573",
        "mi_ns_581",
        "mi_ns_598",
        "win_ns_6",
        "outline_ns_50",
    ] {
        assert!(
            graph.get(leaked).is_none(),
            "AppKit `_NS:` id {leaked:?} must not be used"
        );
    }
}

#[test]
fn notes_fixture_developer_identifier_ids_are_stable() {
    let graph = fixture_graph();

    // The four nodes with genuinely developer-assigned identifiers now derive
    // their id from the identifier (stable by construction across renames).
    for (new_id, old_label_id) in [
        ("text_note_body_text_view", "text_corps_de_la_note"), // "Note Body Text View"
        ("mi_forcequitrequested", "mi_forcer_a_quitter_notes"), // "_forceQuitRequested:"
        ("mi_restartrequested", "mi_redemarrer"),              // "_restartRequested:"
        ("mi_shutdownnowrequested", "mi_eteindre"),            // "_shutDownNowRequested:"
    ] {
        assert!(
            graph.get(new_id).is_some(),
            "identifier-derived id {new_id:?} expected"
        );
        assert!(
            graph.get(old_label_id).is_none(),
            "old label id {old_label_id:?} must be gone"
        );
    }
}

/// Full snapshot of the human-readable (non-path-hash) ids of the Notes fixture.
/// Acts as a regression lock for the entire id scheme under WP-D.
#[test]
fn notes_fixture_named_id_snapshot() {
    let graph = fixture_graph();

    let mut named: Vec<&str> = graph
        .nodes
        .keys()
        .map(String::as_str)
        .filter(|id| !is_path_hash_id(id))
        .collect();
    named.sort_unstable();

    let expected = [
        "btn_ajouter_un_dossier",
        "btn_nouvelle_note",
        "btn_partager",
        "btn_rechercher",
        "cell_notes",
        "mbtn_multimedia",
        "menubar_apple",
        "menubar_edition",
        "mi_coller",
        "mi_copier",
        "mi_forcequitrequested",
        "mi_restartrequested",
        "mi_shutdownnowrequested",
        "mi_supprimer",
        "outline_dossiers",
        "text_note_body_text_view",
        "win_notes_aucune_note",
    ];
    assert_eq!(named, expected, "Notes-fixture named-id snapshot drifted");

    // The remaining nodes are label-less containers keyed by path hash. Their
    // count is fixed (toolbar, row, menu-bar, two menus); each is `prefix_<16hex>`
    // with a prefix drawn from the expected container set.
    let hashed: Vec<&str> = graph
        .nodes
        .keys()
        .map(String::as_str)
        .filter(|id| is_path_hash_id(id))
        .collect();
    assert_eq!(hashed.len(), 5, "expected 5 path-hash ids, got {hashed:?}");
    for id in &hashed {
        let prefix = id.split_once('_').unwrap().0;
        assert!(
            matches!(prefix, "toolbar" | "row" | "menubar" | "menu"),
            "unexpected path-hash prefix in {id:?}"
        );
    }
}
