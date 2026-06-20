use super::*;

// --- Audit #3: composite drag risk (max of source / drop target) ---------

/// A purpose-built fixture for the composite-drag gate: the bundled Notes
/// fixture has no node that is *both* draggable (Row/Cell) and high-risk with a
/// bbox (its high-risk items are bbox-less menu items), so we mint a tiny tree
/// with a harmless draggable row and a high-risk drop target that has a bbox.
fn composite_drag_engine() -> (Engine, Arc<Mutex<Vec<RecordedCall>>>) {
    fn raw(
        ax_role: &str,
        label: Option<&str>,
        frame: Option<Bbox>,
        ax_actions: &[&str],
        children: Vec<dunst_core::RawAxNode>,
    ) -> dunst_core::RawAxNode {
        dunst_core::RawAxNode {
            ax_role: ax_role.into(),
            label: label.map(str::to_owned),
            help: None,
            value: None,
            ax_identifier: None,
            ax_actions: ax_actions.iter().map(|s| s.to_string()).collect(),
            frame,
            enabled: true,
            focused: false,
            children,
        }
    }
    let bb = |x: f64| {
        Some(Bbox {
            x,
            y: 100.0,
            w: 50.0,
            h: 20.0,
        })
    };
    // Row under a Table → draggable (the Table is an ancestor drop container).
    let row = raw("AXRow", Some("note-a"), bb(10.0), &["press"], vec![]);
    let table = raw("AXTable", None, bb(10.0), &[], vec![row]);
    // High-risk drop target WITH a bbox (so drag_element can compute a drop).
    let danger = raw("AXButton", Some("Supprimer"), bb(200.0), &["press"], vec![]);
    let window = raw(
        "AXWindow",
        Some("W"),
        Some(Bbox {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 400.0,
        }),
        &[],
        vec![table, danger],
    );

    let calls = Arc::new(Mutex::new(Vec::new()));
    let perceptor = Box::new(MockPerceptor::new(
        vec![window],
        WindowRef {
            pid: 1,
            window_id: 1,
            app_name: "T".into(),
            title: "T".into(),
        },
    ));
    let exec = Box::new(RecordingExecutor(calls.clone()));
    let eng = Engine::new(
        perceptor,
        exec,
        Target {
            pid: 1,
            window_id: 1,
        },
    )
    .unwrap();
    (eng, calls)
}

#[test]
fn drag_onto_high_risk_target_is_gated_then_approvable() {
    let (mut eng, calls) = composite_drag_engine();
    let source = id_for(&eng, "note-a"); // low-risk draggable row
    let target = id_for(&eng, "Supprimer"); // high-risk drop target, has bbox

    // Precondition: source is harmless, target is the dangerous one.
    assert!(
        !eng.affordance_graph().affordances[&source]
            .risk
            .requires_approval
    );
    assert!(
        eng.affordance_graph().affordances[&target]
            .risk
            .requires_approval
    );

    // The gate fires on the TARGET's risk even though the source is low.
    let gated = eng
        .drag_element(&source, &target, Some("dangerous drop"))
        .unwrap();
    assert_eq!(
        gated.result,
        ActionResult::PendingApproval,
        "high-risk drop target must gate"
    );
    assert_eq!(
        gated.risk.level,
        RiskLevel::High,
        "effective risk is max(source, target)"
    );
    assert!(gated.risk.requires_approval);
    assert!(
        gated
            .risk
            .reasons
            .iter()
            .any(|r| r.contains("drop target") && r.to_lowercase().contains("supprimer")),
        "audit reason attributes the risk to the drop target: {:?}",
        gated.risk.reasons
    );
    assert!(
        calls.lock().unwrap().is_empty(),
        "gated drag never reaches the executor"
    );

    // Approving the dangerous target (its own risk is high → approve accepts it)
    // clears the composite gate for exactly one drag.
    eng.approve(&target).unwrap();
    let ok = eng
        .drag_element(&source, &target, Some("approved drop"))
        .unwrap();
    assert_eq!(ok.result, ActionResult::Success);
    let recorded = calls.lock().unwrap();
    assert_eq!(
        recorded.len(),
        1,
        "executor ran exactly once, on the source"
    );
    assert_eq!(recorded[0].0, source);
    assert_eq!(recorded[0].1, SemanticAction::Drag);
}

// --- Audit #13: a destructive *typed value* gates a low-risk field --------

#[test]
fn destructive_typed_text_gates_low_risk_field_and_is_approvable() {
    let (mut eng, calls) = engine_with_counter();
    let field = id_for(&eng, "Corps de la note"); // low-risk, typeable text area
    assert!(
        !eng.affordance_graph().affordances[&field]
            .risk
            .requires_approval,
        "the field itself is low-risk"
    );

    // Out of context, a low-risk field is NOT approvable (audit #2 still holds).
    assert!(
        eng.approve(&field).is_err(),
        "low-risk field not approvable out of context"
    );

    // A destructive payload raises the gate even though the field is harmless.
    let gated = eng
        .type_into(&field, "supprimer tout", Some("danger"))
        .unwrap();
    assert_eq!(
        gated.result,
        ActionResult::PendingApproval,
        "destructive text gates the field"
    );
    assert_eq!(
        gated.risk.level,
        RiskLevel::High,
        "effective risk = max(field, text)"
    );
    assert!(
        gated
            .risk
            .reasons
            .iter()
            .any(|r| r.contains("typed text") && r.to_lowercase().contains("supprimer")),
        "audit attributes the risk to the typed text: {:?}",
        gated.risk.reasons
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "gated type never reaches the executor"
    );

    // The field is now the subject of a pending gate → approvable; type proceeds.
    eng.approve(&field).unwrap();
    let ok = eng
        .type_into(&field, "supprimer tout", Some("approved"))
        .unwrap();
    assert_eq!(
        ok.result,
        ActionResult::Failed,
        "mock executor records the type attempt, but the unchanged fixture must fail verification"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // One-shot: a second destructive type gates again (grant consumed + refresh).
    let regated = eng.type_into(&field, "supprimer tout", None).unwrap();
    assert_eq!(regated.result, ActionResult::PendingApproval);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Benign text into the same field is never gated.
    let benign = eng.type_into(&field, "bonjour", None).unwrap();
    assert_eq!(benign.result, ActionResult::Failed);
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    // Regression: "provider" contains the French destructive keyword
    // "vider", but it is not a destructive word on token boundaries.
    let provider = eng
        .type_into(&field, "failover multi-provider", None)
        .unwrap();
    assert_eq!(provider.result, ActionResult::Failed);
    assert_eq!(provider.risk.level, RiskLevel::Low);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

// --- effective_risk in isolation (C2 refactor) --------------------------

#[test]
fn effective_risk_folds_drag_target_and_typed_text() {
    let (eng, _) = engine_with_counter();
    let low = RiskAssessment::low();
    let high = RiskAssessment {
        level: RiskLevel::High,
        requires_approval: true,
        reasons: vec!["matched keyword: supprimer".into()],
    };

    // Low source dragged onto a high-risk target → effective High, target gated.
    let co = CoTarget {
        id: "tgt".into(),
        risk: high.clone(),
    };
    let (eff, gated) = eng.effective_risk("src", SemanticAction::Drag, None, &low, Some(&co));
    assert_eq!(eff.level, RiskLevel::High);
    assert!(eff.requires_approval);
    assert_eq!(gated, vec!["tgt".to_string()]);
    assert!(eff.reasons.iter().any(|r| r.contains("drop target")));

    // Destructive text into a low-risk field → effective High, field gated.
    let (eff2, gated2) = eng.effective_risk(
        "field",
        SemanticAction::Type,
        Some("supprimer tout"),
        &low,
        None,
    );
    assert_eq!(eff2.level, RiskLevel::High);
    assert!(eff2.requires_approval);
    assert_eq!(gated2, vec!["field".to_string()]);
    assert!(eff2.reasons.iter().any(|r| r.contains("typed text")));

    // Benign: low source, no co-target, benign text → Low, no gate.
    let (eff3, gated3) = eng.effective_risk("x", SemanticAction::Click, None, &low, None);
    assert!(!eff3.requires_approval);
    assert_eq!(eff3.level, RiskLevel::Low);
    assert!(gated3.is_empty());

    // Foreground-affecting action: raising a low-risk window still gates.
    let (eff4, gated4) = eng.effective_risk("win", SemanticAction::Raise, None, &low, None);
    assert_eq!(eff4.level, RiskLevel::High);
    assert!(eff4.requires_approval);
    assert_eq!(gated4, vec!["win".to_string()]);
    assert!(eff4.reasons.iter().any(|r| r.contains("foreground")));
}
