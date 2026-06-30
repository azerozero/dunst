use super::*;

fn checkbox_form(cutlery: &str, napkins: &str) -> Vec<dunst_core::RawAxNode> {
    vec![raw_node(
        "AXWindow",
        Some("Checkout"),
        None,
        test_bbox(0.0, 0.0, 700.0, 500.0),
        &[],
        vec![raw_node(
            "AXGroup",
            Some("Extras"),
            None,
            test_bbox(20.0, 40.0, 300.0, 120.0),
            &[],
            vec![
                raw_node(
                    "AXCheckBox",
                    Some("Cutlery"),
                    Some(cutlery),
                    test_bbox(40.0, 70.0, 90.0, 24.0),
                    &["press"],
                    vec![],
                ),
                raw_node(
                    "AXCheckBox",
                    Some("Napkins"),
                    Some(napkins),
                    test_bbox(40.0, 100.0, 90.0, 24.0),
                    &["press"],
                    vec![],
                ),
            ],
        )],
    )]
}

fn schedule_roots(time_visible: bool, time_selected: &str) -> Vec<dunst_core::RawAxNode> {
    let schedule_value = if time_visible { "1" } else { "0" };
    let mut groups = vec![raw_node(
        "AXGroup",
        Some("Delivery time"),
        None,
        test_bbox(20.0, 40.0, 300.0, 100.0),
        &[],
        vec![raw_node(
            "AXRadioButton",
            Some("Schedule"),
            Some(schedule_value),
            test_bbox(40.0, 70.0, 120.0, 24.0),
            &["press"],
            vec![],
        )],
    )];
    if time_visible {
        groups.push(raw_node(
            "AXGroup",
            Some("Time"),
            None,
            test_bbox(20.0, 150.0, 300.0, 100.0),
            &[],
            vec![raw_node(
                "AXCheckBox",
                Some("7 PM"),
                Some(time_selected),
                test_bbox(40.0, 180.0, 120.0, 24.0),
                &["press"],
                vec![],
            )],
        ));
    }
    vec![raw_node(
        "AXWindow",
        Some("Checkout"),
        None,
        test_bbox(0.0, 0.0, 700.0, 500.0),
        &[],
        groups,
    )]
}

fn plan_for(choice: &Choice, op: SelectionOp) -> SelectionPlan {
    SelectionPlan {
        steps: vec![step_for(choice, op)],
    }
}

fn step_for(choice: &Choice, op: SelectionOp) -> SelectionStep {
    SelectionStep {
        group_id: Some(choice.group_id.clone()),
        choice_id: choice.id.clone(),
        label: Some(choice.label.clone()),
        op,
        value: None,
        expected_after: None,
        bbox: choice.bbox,
    }
}

fn choice_by_label(model: &ChoiceModel, label: &str) -> Choice {
    model
        .groups
        .iter()
        .flat_map(|group| &group.choices)
        .find(|choice| choice.label == label)
        .unwrap_or_else(|| panic!("missing choice {label:?}"))
        .clone()
}

#[test]
fn apply_selections_first_call_is_pending_with_per_step_risk_preview() {
    let (mut eng, _) = engine_from_roots(checkbox_form("0", "0"), "CheckoutApp", "Checkout");
    let model = eng
        .enumerate_choices(EnumerateOpts {
            scope: "page",
            include_latent: true,
            scroll_scan: false,
            max_scroll_pages: 1,
            limit: 100,
        })
        .unwrap();
    let cutlery = choice_by_label(&model, "Cutlery");

    let outcome = eng
        .apply_selections(plan_for(&cutlery, SelectionOp::Select), &model.ui_epoch)
        .unwrap();

    assert_eq!(outcome.status, ApplyStatus::PendingApproval);
    assert!(outcome.batch_id.starts_with("batch@selections:"));
    assert_eq!(outcome.pending_preview.len(), 1);
    assert_eq!(outcome.pending_preview[0].risk, RiskLevel::Low);
}

#[test]
fn apply_selections_single_approval_executes_whole_batch() {
    let (mut eng, calls) = engine_from_sequence(
        vec![checkbox_form("0", "0"), checkbox_form("1", "0")],
        "CheckoutApp",
        "Checkout",
    );
    let model = eng
        .enumerate_choices(EnumerateOpts {
            scope: "page",
            include_latent: true,
            scroll_scan: false,
            max_scroll_pages: 1,
            limit: 100,
        })
        .unwrap();
    let cutlery = choice_by_label(&model, "Cutlery");
    let plan = plan_for(&cutlery, SelectionOp::Select);
    let pending = eng.apply_selections(plan.clone(), &model.ui_epoch).unwrap();

    eng.approve(&pending.batch_id).unwrap();
    let applied = eng.apply_selections(plan, &model.ui_epoch).unwrap();

    assert_eq!(applied.status, ApplyStatus::Applied);
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(applied.steps[0].result, StepResultStatus::Success);
    assert!(applied.verify.as_ref().unwrap().ok);
}

#[test]
fn apply_selections_batch_grant_is_one_shot_resists_second_batch() {
    let (mut eng, _) = engine_from_sequence(
        vec![checkbox_form("0", "0"), checkbox_form("1", "0")],
        "CheckoutApp",
        "Checkout",
    );
    let model = eng
        .enumerate_choices(EnumerateOpts {
            scope: "page",
            include_latent: true,
            scroll_scan: false,
            max_scroll_pages: 1,
            limit: 100,
        })
        .unwrap();
    let cutlery = choice_by_label(&model, "Cutlery");
    let plan = plan_for(&cutlery, SelectionOp::Select);
    let pending = eng.apply_selections(plan.clone(), &model.ui_epoch).unwrap();
    eng.approve(&pending.batch_id).unwrap();
    assert_eq!(
        eng.apply_selections(plan.clone(), &model.ui_epoch)
            .unwrap()
            .status,
        ApplyStatus::Applied
    );

    let second = eng
        .apply_selections(plan, &eng.current_ui_epoch_fingerprint())
        .unwrap();
    assert_eq!(second.status, ApplyStatus::PendingApproval);
}

#[test]
fn apply_selections_rescans_only_when_fingerprint_changes() {
    let (mut eng, _) = engine_from_sequence(
        vec![
            checkbox_form("0", "0"),
            checkbox_form("1", "0"),
            checkbox_form("1", "1"),
        ],
        "CheckoutApp",
        "Checkout",
    );
    let model = eng
        .enumerate_choices(EnumerateOpts {
            scope: "page",
            include_latent: true,
            scroll_scan: false,
            max_scroll_pages: 1,
            limit: 100,
        })
        .unwrap();
    let cutlery = choice_by_label(&model, "Cutlery");
    let napkins = choice_by_label(&model, "Napkins");
    let plan = SelectionPlan {
        steps: vec![
            step_for(&cutlery, SelectionOp::Select),
            step_for(&napkins, SelectionOp::Select),
        ],
    };
    let pending = eng.apply_selections(plan.clone(), &model.ui_epoch).unwrap();
    eng.approve(&pending.batch_id).unwrap();

    let applied = eng.apply_selections(plan, &model.ui_epoch).unwrap();

    assert_eq!(applied.status, ApplyStatus::Applied);
    assert_eq!(applied.rescans, 0, "value-only changes are not reflow");
}

#[test]
fn apply_selections_reflow_reresolves_remaining_steps_by_label() {
    let (mut eng, _) = engine_from_sequence(
        vec![
            schedule_roots(false, "0"),
            schedule_roots(true, "0"),
            schedule_roots(true, "1"),
        ],
        "CheckoutApp",
        "Checkout",
    );
    let model = eng
        .enumerate_choices(EnumerateOpts {
            scope: "page",
            include_latent: true,
            scroll_scan: false,
            max_scroll_pages: 1,
            limit: 100,
        })
        .unwrap();
    let schedule = choice_by_label(&model, "Schedule");
    let plan = SelectionPlan {
        steps: vec![
            step_for(&schedule, SelectionOp::Select),
            SelectionStep {
                group_id: Some(schedule.group_id.clone()),
                choice_id: "stale_time_choice".to_string(),
                label: Some("7 PM".to_string()),
                op: SelectionOp::Select,
                value: None,
                expected_after: None,
                bbox: None,
            },
        ],
    };
    let pending = eng.apply_selections(plan.clone(), &model.ui_epoch).unwrap();
    eng.approve(&pending.batch_id).unwrap();

    let applied = eng.apply_selections(plan, &model.ui_epoch).unwrap();

    assert_eq!(applied.status, ApplyStatus::Applied);
    assert_eq!(applied.rescans, 1);
    assert_eq!(
        applied.steps[1].resolved_by,
        Some(ResolvedBy::LabelAfterRescan)
    );
}

#[test]
fn apply_selections_budget_exhaustion_degrades_to_partial() {
    let (mut eng, _) = engine_from_roots(checkbox_form("0", "0"), "CheckoutApp", "Checkout");
    let epoch = eng.current_ui_epoch_fingerprint();
    let plan = SelectionPlan {
        steps: vec![SelectionStep {
            group_id: None,
            choice_id: "future_choice".to_string(),
            label: Some("Future choice".to_string()),
            op: SelectionOp::Select,
            value: None,
            expected_after: None,
            bbox: None,
        }],
    };
    let pending = eng.apply_selections(plan.clone(), &epoch).unwrap();
    eng.approve(&pending.batch_id).unwrap();

    let partial = eng.apply_selections(plan, &epoch).unwrap();

    assert_eq!(partial.status, ApplyStatus::PartiallyApplied);
    assert_eq!(partial.remaining_steps.len(), 1);
}

#[test]
fn apply_selections_single_consolidated_verify_reports_per_check() {
    let (mut eng, _) = engine_from_sequence(
        vec![
            checkbox_form("0", "0"),
            checkbox_form("1", "0"),
            checkbox_form("1", "1"),
        ],
        "CheckoutApp",
        "Checkout",
    );
    let model = eng
        .enumerate_choices(EnumerateOpts {
            scope: "page",
            include_latent: true,
            scroll_scan: false,
            max_scroll_pages: 1,
            limit: 100,
        })
        .unwrap();
    let cutlery = choice_by_label(&model, "Cutlery");
    let napkins = choice_by_label(&model, "Napkins");
    let plan = SelectionPlan {
        steps: vec![
            step_for(&cutlery, SelectionOp::Select),
            step_for(&napkins, SelectionOp::Select),
        ],
    };
    let pending = eng.apply_selections(plan.clone(), &model.ui_epoch).unwrap();
    eng.approve(&pending.batch_id).unwrap();

    let applied = eng.apply_selections(plan, &model.ui_epoch).unwrap();

    let verify = applied.verify.expect("verify result");
    assert_eq!(verify.checks.len(), 2);
}

#[test]
fn apply_selections_rejects_forged_batch_id_via_validate_synthetic() {
    let (mut eng, _) = engine_from_roots(checkbox_form("0", "0"), "CheckoutApp", "Checkout");

    let err = eng
        .approve("batch@selections:not-a-hex-value:2")
        .unwrap_err();

    assert!(err.to_string().contains("valid batch selection"));
}
