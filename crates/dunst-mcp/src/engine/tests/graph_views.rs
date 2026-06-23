use super::*;

#[test]
fn unavailable_action_is_an_error() {
    let (mut eng, calls) = engine_with_counter();
    // A button has no Type affordance.
    let id = id_for(&eng, "Nouvelle note");
    let err = eng.type_into(&id, "x", None).unwrap_err();
    assert!(matches!(err, DunstError::ActionUnavailable { .. }));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn every_attempt_is_audited() {
    let (mut eng, _c) = engine_with_counter();
    let _ = eng.click_element(&id_for(&eng, "Supprimer"), None); // gated
    let _ = eng.click_element(&id_for(&eng, "Nouvelle note"), None); // ok
    assert_eq!(eng.trace().len(), 2);
}

#[test]
fn audited_attempts_include_session_identity_when_known() {
    let (mut eng, _c) = engine_with_counter();
    eng.set_session_identity(SessionIdentity {
        session_id: "dunst-test-session".into(),
        client_name: Some("codex".into()),
        client_version: Some("5.5".into()),
        agent_id: Some("collective-fixer".into()),
        parent_pid: Some(42),
        parent_process: Some("codex".into()),
    });

    let _ = eng.click_element(&id_for(&eng, "Nouvelle note"), None);
    let caller = eng
        .trace()
        .last()
        .and_then(|entry| entry.caller.as_ref())
        .expect("audit entry has session identity");

    assert_eq!(caller.session_id, "dunst-test-session");
    assert_eq!(caller.client_name.as_deref(), Some("codex"));
    assert_eq!(caller.agent_id.as_deref(), Some("collective-fixer"));
}

#[test]
fn drag_records_target_bbox_centre() {
    let (mut eng, calls) = engine_with_recorder();
    let source = non_gated_drag_source(&eng);
    let target = id_for(&eng, "Nouvelle note");

    // Expected drop point = centre of the *target* node's bbox, formatted
    // exactly as the engine formats it.
    let bbox = eng.scene_graph().get(&target).unwrap().bbox.unwrap();
    let expected = format!("{},{}", bbox.x + bbox.w / 2.0, bbox.y + bbox.h / 2.0);

    let entry = eng.drag_element(&source, &target, Some("reorder")).unwrap();

    // The executor saw exactly (source, Drag, Some("x,y")).
    let recorded = calls.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0],
        (source.clone(), SemanticAction::Drag, Some(expected))
    );

    // The audit entry describes the drag on the source and is in the trace.
    assert_eq!(entry.action, SemanticAction::Drag);
    assert_eq!(entry.target_id, source);
    assert_eq!(entry.result, ActionResult::Success);
    assert_eq!(eng.trace().len(), 1);
    assert_eq!(eng.trace()[0].action, SemanticAction::Drag);
}

#[test]
fn drag_unknown_target_is_an_error() {
    let (mut eng, calls) = engine_with_recorder();
    let source = non_gated_drag_source(&eng);

    let err = eng
        .drag_element(&source, "no_such_target", None)
        .unwrap_err();
    assert!(matches!(err, DunstError::ElementNotFound(_)));

    // No executor call, no audit entry: the failure is structural, pre-act.
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(eng.trace().len(), 0);
}

#[test]
fn drag_source_without_affordance_is_unavailable() {
    let (mut eng, calls) = engine_with_recorder();
    // A toolbar button exposes Click, never Drag; the target has a bbox.
    let source = id_for(&eng, "Nouvelle note");
    let target = id_for(&eng, "Nouvelle note");

    let err = eng.drag_element(&source, &target, None).unwrap_err();
    assert!(matches!(err, DunstError::ActionUnavailable { .. }));
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(eng.trace().len(), 0);
}

// --- WP-J / J1: get_scene_graph projection ------------------------------

#[test]
fn compact_view_omits_heavy_fields_and_keeps_n_children() {
    let (eng, _) = engine_with_counter();
    let v = eng.scene_graph_view(SceneView::Compact, false);
    let id = id_for(&eng, "Nouvelle note");
    let node = v["nodes"].get(id.as_str()).expect("compact node present");

    // Heavy/derivable AX fields are dropped.
    for dropped in [
        "ax_role",
        "help",
        "ax_actions",
        "ax_identifier",
        "last_seen_ms",
        "children",
        "confidence",
        "source",
    ] {
        assert!(
            node.get(dropped).is_none(),
            "compact node must drop {dropped}"
        );
    }
    // Kept fields, with children collapsed to a count.
    assert!(node.get("n_children").is_some(), "n_children kept");
    assert!(node.get("bbox").is_some(), "bbox kept");
    assert_eq!(node["role"], json!("button"));
}

#[test]
fn compact_view_is_materially_smaller_than_full() {
    let (eng, _) = engine_with_counter();
    let full = eng.scene_graph_view(SceneView::Full, false);
    let compact = eng.scene_graph_view(SceneView::Compact, false);
    let full_len = serde_json::to_string(&full).unwrap().len();
    let compact_len = serde_json::to_string(&compact).unwrap().len();
    // Visible with `cargo test -- --nocapture`; the real before/after note.
    eprintln!(
            "get_scene_graph fixture size — full: {full_len} B, compact: {compact_len} B (×{:.1} lighter)",
            full_len as f64 / compact_len.max(1) as f64
        );
    assert!(
        compact_len < full_len,
        "compact ({compact_len}) must be smaller than full ({full_len})"
    );
}

#[test]
fn full_view_is_byte_identical_to_raw_scene_graph() {
    let (eng, _) = engine_with_counter();
    let v = eng.scene_graph_view(SceneView::Full, false);
    let raw = serde_json::to_value(eng.scene_graph()).unwrap();
    assert_eq!(v, raw, "full view is the unchanged escape hatch");
}

#[test]
fn summary_view_has_counts_and_roots_but_no_nodes() {
    let (eng, _) = engine_with_counter();
    let v = eng.scene_graph_view(SceneView::Summary, false);
    assert!(v.get("nodes").is_none(), "summary carries no per-node list");
    let n_nodes = v["n_nodes"].as_u64().expect("n_nodes");
    let n_actionable = v["n_actionable"].as_u64().expect("n_actionable");
    assert!(n_nodes >= 1);
    assert!(v["roots"].is_array());
    assert!(v["counts_by_role"].is_object());
    assert!(v["window"].is_object());
    assert!(n_actionable <= n_nodes, "actionable is a subset");
    assert!(
        n_actionable >= 1,
        "at least the toolbar button is actionable"
    );
}

#[test]
fn actionable_only_drops_latent_menu_items() {
    let (eng, _) = engine_with_counter();
    let supprimer = id_for(&eng, "Supprimer"); // latent AXMenuItem (no bbox)
    let nouvelle = id_for(&eng, "Nouvelle note"); // on-screen toolbar button
    let v = eng.scene_graph_view(SceneView::Compact, true);
    assert!(
        v["nodes"].get(supprimer.as_str()).is_none(),
        "latent node dropped by actionable_only"
    );
    assert!(
        v["nodes"].get(nouvelle.as_str()).is_some(),
        "on-screen node kept"
    );
}

// --- WP-J / J2: latent affordance filtering -----------------------------

#[test]
fn query_affordances_excludes_latent_by_default_but_include_latent_keeps_them() {
    let (eng, _) = engine_with_counter();
    let supprimer = id_for(&eng, "Supprimer"); // latent menu item exposing Click
    let nouvelle = id_for(&eng, "Nouvelle note"); // on-screen button

    let default = eng.query_affordances(SemanticAction::Click);
    assert!(
        !default.contains(&supprimer),
        "latent menu item filtered from default listing"
    );
    assert!(default.contains(&nouvelle), "on-screen button still listed");

    let all = eng.query_affordances_filtered(SemanticAction::Click, true);
    assert!(
        all.contains(&supprimer),
        "include_latent surfaces the latent item"
    );
    assert!(
        all.len() > default.len(),
        "include_latent is a strict superset here"
    );
}

#[test]
fn get_affordances_view_filters_latent_but_keeps_it_under_include_latent() {
    let (eng, _) = engine_with_counter();
    let supprimer = id_for(&eng, "Supprimer");
    let filtered = eng.affordances_view(false);
    assert!(
        filtered["affordances"].get(supprimer.as_str()).is_none(),
        "latent omitted by default"
    );
    let all = eng.affordances_view(true);
    assert!(
        all["affordances"].get(supprimer.as_str()).is_some(),
        "include_latent keeps it"
    );
}

#[test]
fn find_element_and_gating_still_reach_latent_nodes() {
    // CRITICAL (WP-J): filtering the *listing* must NOT hide latent nodes from
    // find_element, nor stop the risk gate from acting on them by id.
    let (mut eng, calls) = engine_with_counter();
    assert!(
        !eng.find_element("Supprimer").is_empty(),
        "find_element still locates the latent item"
    );

    let supprimer = id_for(&eng, "Supprimer");
    // click_element by id reaches the gate (PendingApproval), not ActionUnavailable,
    // and the executor never runs.
    let e = eng.click_element(&supprimer, Some("delete")).unwrap();
    assert_eq!(e.result, ActionResult::PendingApproval);
    assert!(e.risk.requires_approval);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "gated action never reaches the executor"
    );
}

#[test]
fn visible_only_find_drops_latent_noise_but_default_keeps_it() {
    let (eng, _) = engine_with_counter();
    assert!(
        !eng.find_element("Supprimer").is_empty(),
        "default find still reaches latent nodes"
    );
    assert!(
        eng.find_element_filtered("Supprimer", true).is_empty(),
        "visible_only drops collapsed/off-window matches"
    );
    assert!(
        !eng.find_element_filtered("Nouvelle note", true).is_empty(),
        "visible_only keeps on-screen matches"
    );
}

#[test]
fn find_element_matches_accents_insensitively() {
    let window = raw_node(
        "AXWindow",
        Some("Profil"),
        None,
        test_bbox(0.0, 0.0, 700.0, 500.0),
        &[],
        vec![raw_node(
            "AXButton",
            Some("Éditer les expertises"),
            None,
            test_bbox(260.0, 80.0, 140.0, 36.0),
            &["press"],
            vec![],
        )],
    );
    let (eng, _) = engine_from_roots(vec![window], "Browser", "Profil");

    assert!(
        !eng.find_element_filtered("éditer", true).is_empty(),
        "accented query should match"
    );
    assert!(
        !eng.find_element_filtered("editer", true).is_empty(),
        "unaccented query should match accented UI text"
    );
}

#[test]
fn find_element_promotes_editable_control_associated_with_matching_label() {
    let window = raw_node(
        "AXWindow",
        Some("Experience"),
        None,
        test_bbox(0.0, 0.0, 700.0, 500.0),
        &[],
        vec![
            raw_node(
                "AXStaticText",
                Some("Description"),
                None,
                test_bbox(40.0, 80.0, 100.0, 20.0),
                &[],
                vec![],
            ),
            raw_node(
                "AXTextArea",
                None,
                Some("Existing body"),
                test_bbox(40.0, 108.0, 500.0, 160.0),
                &["press"],
                vec![],
            ),
        ],
    );
    let (eng, _) = engine_from_roots(vec![window], "Firefox", "Experience");

    let matches = eng.find_element_filtered("description", true);
    assert_eq!(
        matches.first().map(|node| node.role),
        Some(Role::TextArea),
        "nearest editable field should rank before the static label: {matches:?}"
    );
    assert!(
        matches.iter().any(|node| node.role == Role::StaticText),
        "the matching label remains present for orientation"
    );
}
