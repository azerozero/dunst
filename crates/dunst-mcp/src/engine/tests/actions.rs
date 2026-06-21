use super::*;

#[test]
fn low_risk_click_proceeds_and_executes() {
    let (mut eng, calls) = engine_with_counter();
    let id = id_for(&eng, "Nouvelle note");
    let entry = eng.click_element(&id, Some("create")).unwrap();
    assert_eq!(entry.result, ActionResult::Success);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn click_element_refreshes_once_when_id_is_missing_from_stale_graph() {
    let stale = raw_node(
        "AXWindow",
        Some("CVs"),
        None,
        test_bbox(0.0, 0.0, 700.0, 900.0),
        &[],
        vec![],
    );
    let fresh = raw_node(
        "AXWindow",
        Some("CVs"),
        None,
        test_bbox(0.0, 0.0, 700.0, 900.0),
        &[],
        vec![raw_node(
            "AXButton",
            Some("Importer"),
            None,
            test_bbox(300.0, 200.0, 120.0, 32.0),
            &["press"],
            vec![],
        )],
    );
    let perceptor = Box::new(SequencePerceptor::new(
        vec![vec![stale], vec![fresh]],
        WindowRef {
            pid: 1,
            window_id: 1,
            app_name: "Firefox".into(),
            title: "Collective".into(),
        },
    ));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let exec = Box::new(RecordingExecutor(calls.clone()));
    let mut eng = Engine::new(
        perceptor,
        exec,
        Target {
            pid: 1,
            window_id: 1,
        },
    )
    .unwrap();

    assert!(
        eng.scene_graph().get("btn_importer").is_none(),
        "initial graph is stale and lacks the target"
    );
    let entry = eng
        .click_element("btn_importer", Some("retry stale graph"))
        .unwrap();
    assert_eq!(entry.result, ActionResult::Success);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "btn_importer");
}

#[test]
fn find_element_prefers_exact_button_label_over_containing_help_text() {
    let mut publish = raw_node(
        "AXButton",
        Some("Publier"),
        None,
        test_bbox(520.0, 120.0, 110.0, 32.0),
        &["press"],
        vec![],
    );
    publish.enabled = false;
    let (eng, _) = engine_from_roots(
        vec![raw_node(
            "AXWindow",
            Some("Collective"),
            None,
            test_bbox(0.0, 0.0, 700.0, 500.0),
            &[],
            vec![
                raw_node(
                    "AXStaticText",
                    Some("Écrivez la description de la réalisation, les informations à publier"),
                    None,
                    test_bbox(80.0, 80.0, 500.0, 36.0),
                    &[],
                    vec![],
                ),
                publish,
            ],
        )],
        "Firefox",
        "Collective",
    );

    let matches = eng.find_element_filtered("publier", false);

    assert_eq!(
        matches.first().map(|node| node.id.as_str()),
        Some("btn_publier"),
        "exact button label should outrank explanatory text containing the query: {matches:?}"
    );
}

#[test]
fn disabled_button_click_is_unavailable() {
    let mut publish = raw_node(
        "AXButton",
        Some("Publier"),
        None,
        test_bbox(520.0, 120.0, 110.0, 32.0),
        &["press"],
        vec![],
    );
    publish.enabled = false;
    let (mut eng, calls) = engine_from_roots(
        vec![raw_node(
            "AXWindow",
            Some("Collective"),
            None,
            test_bbox(0.0, 0.0, 700.0, 500.0),
            &[],
            vec![publish],
        )],
        "Firefox",
        "Collective",
    );

    let err = eng
        .click_element("btn_publier", Some("publish disabled draft"))
        .unwrap_err();

    assert!(matches!(err, VisualOpsError::ActionUnavailable { .. }));
    assert_eq!(calls.lock().unwrap().len(), 0);
}

#[test]
fn type_into_waits_for_ax_value_to_settle() {
    let expected =
        "Freelance — Architecte DevSecOps & Platform Engineering | Mission Défense air-gapped";
    let window = |value: &str| {
        raw_node(
            "AXWindow",
            Some("Collective"),
            None,
            test_bbox(0.0, 0.0, 700.0, 900.0),
            &[],
            vec![raw_node(
                "AXTextField",
                Some("Titre de poste"),
                Some(value),
                test_bbox(100.0, 120.0, 568.0, 32.0),
                &["press"],
                vec![],
            )],
        )
    };
    let perceptor = Box::new(SequencePerceptor::new(
        vec![
            vec![window(
                "nce — Architecte DevSecOps & Platform Engineering | Mission Défense air-gapped",
            )],
            vec![window("Freel")],
            vec![window(expected)],
        ],
        WindowRef {
            pid: 1,
            window_id: 1,
            app_name: "Firefox".into(),
            title: "Collective".into(),
        },
    ));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let exec = Box::new(RecordingExecutor(calls.clone()));
    let mut eng = Engine::new(
        perceptor,
        exec,
        Target {
            pid: 1,
            window_id: 1,
        },
    )
    .unwrap();
    let field = id_for(&eng, "Titre de poste");

    let entry = eng
        .type_into(&field, expected, Some("wait for AX settle"))
        .unwrap();

    assert_eq!(entry.result, ActionResult::Success);
    assert_eq!(
        eng.scene_graph().get(&field).unwrap().value.as_deref(),
        Some(expected)
    );
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, field);
}

#[test]
fn high_risk_click_is_gated_then_approved() {
    let (mut eng, calls) = engine_with_counter();
    let id = id_for(&eng, "Supprimer");

    // 1. Denied pending approval — and the executor must NOT have run.
    let e1 = eng.click_element(&id, Some("delete")).unwrap();
    assert_eq!(e1.result, ActionResult::PendingApproval);
    assert_eq!(e1.risk.level, RiskLevel::High);
    assert!(e1.risk.requires_approval);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "executor must not run on a gated action"
    );

    // 2. Approve, retry — proceeds, executor called exactly once.
    eng.approve(&id).unwrap();
    let e2 = eng.click_element(&id, Some("approved")).unwrap();
    assert_eq!(e2.result, ActionResult::Success);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn remove_click_fails_when_target_count_does_not_decrease() {
    let roots = remove_tag_roots(2);
    let (mut eng, calls) =
        engine_from_sequence(vec![roots.clone(), roots], "Firefox", "Expertises");
    let id = "btn_remove_platform_engineering_2";

    eng.approve(id).unwrap();
    let entry = eng.click_element(id, Some("remove duplicate")).unwrap();

    assert_eq!(entry.result, ActionResult::Failed);
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(
        eng.find_element_filtered("remove Platform Engineering", false)
            .len(),
        2,
        "unchanged remove-label count should be treated as failed"
    );
}

#[test]
fn remove_click_succeeds_when_target_count_decreases() {
    let (mut eng, calls) = engine_from_sequence(
        vec![remove_tag_roots(2), remove_tag_roots(1)],
        "Firefox",
        "Expertises",
    );
    let id = "btn_remove_platform_engineering_2";

    eng.approve(id).unwrap();
    let entry = eng.click_element(id, Some("remove duplicate")).unwrap();

    assert_eq!(entry.result, ActionResult::Success);
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(
        eng.find_element_filtered("remove Platform Engineering", false)
            .len(),
        1
    );
}

#[test]
fn checkbox_click_fails_when_value_does_not_change() {
    let roots = checkbox_roots("0");
    let (mut eng, calls) =
        engine_from_sequence(vec![roots.clone(), roots], "Firefox", "Expertises");
    let id = id_for(&eng, "DevOps");

    let entry = eng.click_element(&id, Some("toggle checkbox")).unwrap();

    assert_eq!(entry.result, ActionResult::Failed);
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[test]
fn checkbox_click_succeeds_when_value_changes() {
    let (mut eng, calls) = engine_from_sequence(
        vec![checkbox_roots("0"), checkbox_roots("1")],
        "Firefox",
        "Expertises",
    );
    let id = id_for(&eng, "DevOps");

    let entry = eng.click_element(&id, Some("toggle checkbox")).unwrap();

    assert_eq!(entry.result, ActionResult::Success);
    assert_eq!(calls.lock().unwrap().len(), 1);
}

// --- Audit #2: validated, one-shot, refresh-invalidated approvals --------

#[test]
fn approval_is_one_shot_consumed_by_act() {
    let (mut eng, calls) = engine_with_counter();
    let id = id_for(&eng, "Supprimer");

    eng.approve(&id).unwrap();
    assert_eq!(
        eng.click_element(&id, Some("1st")).unwrap().result,
        ActionResult::Success
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // The grant authorised exactly one action: a second high-risk click on the
    // same element (re-resolved after the post-action refresh) gates again.
    let id2 = id_for(&eng, "Supprimer");
    let e2 = eng.click_element(&id2, Some("2nd")).unwrap();
    assert_eq!(
        e2.result,
        ActionResult::PendingApproval,
        "grant must not survive one use"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "no second execution without re-approval"
    );
}

#[test]
fn approval_is_invalidated_by_refresh() {
    let (mut eng, calls) = engine_with_counter();
    let id = id_for(&eng, "Supprimer");

    eng.approve(&id).unwrap();
    eng.refresh().unwrap(); // scene re-perceived → the grant must be dropped

    let id2 = id_for(&eng, "Supprimer");
    let e = eng.click_element(&id2, Some("after refresh")).unwrap();
    assert_eq!(
        e.result,
        ActionResult::PendingApproval,
        "refresh invalidates approvals"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "executor never ran");
}

#[test]
fn approve_rejects_unknown_and_non_gated_ids() {
    let (mut eng, calls) = engine_with_counter();

    // Unknown id → ElementNotFound; nothing is stored.
    let err = eng.approve("no_such_id").unwrap_err();
    assert!(matches!(err, VisualOpsError::ElementNotFound(_)));

    // A low-risk element (toolbar button) is not gated → error, nothing stored.
    let low = id_for(&eng, "Nouvelle note");
    assert!(
        eng.approve(&low).is_err(),
        "approving a non-gated id is rejected"
    );

    // And because the bogus grants were rejected, the high-risk gate is intact:
    // "Supprimer" is still PendingApproval (no spurious approval leaked).
    let supprimer = id_for(&eng, "Supprimer");
    let e = eng.click_element(&supprimer, None).unwrap();
    assert_eq!(e.result, ActionResult::PendingApproval);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn raw_input_gate_requires_pending_synthetic_approval() {
    let (mut eng, _) = engine_with_counter();

    let pending = eng
        .gate_raw_input(
            "screen@10,20:click",
            SemanticAction::Click,
            Some("click 10,20".to_string()),
            Some("raw screen click"),
            Engine::raw_input_risk(Vec::new()),
        )
        .expect("unapproved raw input must gate");
    assert_eq!(pending.result, ActionResult::PendingApproval);
    assert_eq!(pending.risk.level, RiskLevel::High);
    assert!(pending.risk.requires_approval);

    eng.approve("screen@10,20:click").unwrap();
    assert!(
        eng.gate_raw_input(
            "screen@10,20:click",
            SemanticAction::Click,
            Some("click 10,20".to_string()),
            Some("raw screen click"),
            Engine::raw_input_risk(Vec::new()),
        )
        .is_none(),
        "approved pending raw target should pass the gate once"
    );

    eng.approvals.remove("screen@10,20:click");
    let err = eng.approve("screen@10,20:other").unwrap_err();
    assert!(matches!(err, VisualOpsError::ElementNotFound(_)));
}

#[test]
fn raw_user_active_failure_preserves_approval_for_retry() {
    let (mut eng, _) = engine_with_counter();
    let target = "keyboard@scroll:down:2";

    eng.pending_gate_ids.insert(target.to_string());
    eng.approve(target).unwrap();
    assert!(eng.raw_approval_available_for_test(target));
    assert!(
        eng.gate_raw_input(
            target,
            SemanticAction::Scroll,
            Some("scroll down x2".to_string()),
            Some("background web scroll"),
            Engine::raw_input_risk(Vec::new()),
        )
        .is_none(),
        "approved scroll should pass the raw gate"
    );

    let outcome = Err(VisualOpsError::Execution(
            "user-active guard blocked background key: last keyboard/mouse input was 244 ms ago (< 300 ms)".into(),
        ));
    let err = eng
        .audit_raw_input(
            target.to_string(),
            SemanticAction::Scroll,
            Some("scroll down x2".to_string()),
            Some("background web scroll"),
            Engine::raw_input_risk(Vec::new()),
            outcome,
        )
        .unwrap_err();
    assert!(err.to_string().contains("user-active guard blocked"));
    assert!(
        eng.raw_approval_available_for_test(target),
        "user-active guard should not consume an already approved raw action"
    );
}
