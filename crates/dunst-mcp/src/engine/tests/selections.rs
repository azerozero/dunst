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

fn search_form(value: &str) -> Vec<dunst_core::RawAxNode> {
    vec![raw_node(
        "AXWindow",
        Some("Uber Eats"),
        None,
        test_bbox(0.0, 0.0, 700.0, 500.0),
        &[],
        vec![raw_node(
            "AXSearchField",
            Some("Search"),
            Some(value),
            test_bbox(40.0, 160.0, 320.0, 36.0),
            &[],
            vec![],
        )],
    )]
}

fn plan_for(choice: &Choice, op: SelectionOp) -> SelectionPlan {
    SelectionPlan {
        steps: vec![step_for(choice, op)],
    }
}

fn set_text_plan_for(choice: &Choice, value: &str) -> SelectionPlan {
    let mut step = step_for(choice, SelectionOp::SetText);
    step.value = Some(value.to_string());
    SelectionPlan { steps: vec![step] }
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

struct TypeFailingExecutor {
    calls: Arc<Mutex<Vec<RecordedCall>>>,
}

impl ActionExecutor for TypeFailingExecutor {
    fn perform(
        &self,
        _target: &Target,
        node: &SceneNode,
        action: SemanticAction,
        argument: Option<&str>,
    ) -> dunst_core::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((node.id.clone(), action, argument.map(str::to_owned)));
        if action == SemanticAction::Type {
            Err(dunst_core::DunstError::Execution(
                "typed path failed in test".into(),
            ))
        } else {
            Ok(())
        }
    }
}

fn engine_from_sequence_with_type_failures(
    captures: Vec<Vec<dunst_core::RawAxNode>>,
) -> (Engine, Arc<Mutex<Vec<RecordedCall>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let perceptor = Box::new(SequencePerceptor::new(
        captures,
        WindowRef {
            pid: 1,
            window_id: 1,
            app_name: "Firefox".into(),
            title: "Uber Eats".into(),
        },
    ));
    let exec = Box::new(TypeFailingExecutor {
        calls: calls.clone(),
    });
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

#[test]
fn set_text_falls_back_to_focused_field_replacement_after_type_failure() {
    let expected = "sushi";
    let (mut eng, calls) = engine_from_sequence_with_type_failures(vec![
        search_form(""),
        search_form(""),
        search_form(""),
        search_form(""),
        search_form(expected),
    ]);
    eng.queue_set_field_text_results_for_test(vec![ActionResult::Success]);
    let model = eng
        .enumerate_choices(EnumerateOpts {
            scope: "page",
            include_latent: true,
            scroll_scan: false,
            max_scroll_pages: 1,
            limit: 100,
        })
        .unwrap();
    let search = choice_by_label(&model, "Search");
    let plan = set_text_plan_for(&search, expected);
    let pending = eng.apply_selections(plan.clone(), &model.ui_epoch).unwrap();
    eng.approve(&pending.batch_id).unwrap();

    let applied = eng.apply_selections(plan, &model.ui_epoch).unwrap();

    assert_eq!(applied.status, ApplyStatus::Applied);
    assert_eq!(applied.steps[0].result, StepResultStatus::Success);
    assert!(applied.verify.as_ref().unwrap().ok);
    assert_eq!(
        eng.scene_graph().get(&search.id).unwrap().value.as_deref(),
        Some(expected)
    );
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|(_, action, _)| *action == SemanticAction::Type)
            .count(),
        2
    );
    assert!(calls
        .iter()
        .any(|(_, action, _)| *action == SemanticAction::Focus));
}

#[test]
fn set_text_reports_actionable_error_when_all_paths_fail() {
    let (mut eng, _) = engine_from_sequence_with_type_failures(vec![
        search_form(""),
        search_form(""),
        search_form(""),
        search_form(""),
        search_form(""),
    ]);
    eng.queue_set_field_text_results_for_test(vec![ActionResult::Failed]);
    let model = eng
        .enumerate_choices(EnumerateOpts {
            scope: "page",
            include_latent: true,
            scroll_scan: false,
            max_scroll_pages: 1,
            limit: 100,
        })
        .unwrap();
    let search = choice_by_label(&model, "Search");
    let plan = set_text_plan_for(&search, "sushi");
    let pending = eng.apply_selections(plan.clone(), &model.ui_epoch).unwrap();
    eng.approve(&pending.batch_id).unwrap();

    let partial = eng.apply_selections(plan, &model.ui_epoch).unwrap();

    assert_eq!(partial.status, ApplyStatus::PartiallyApplied);
    assert_eq!(partial.steps[0].result, StepResultStatus::Failed);
    let error = partial.steps[0].error.as_deref().unwrap_or_default();
    assert!(error.contains("web text field could not receive input in background"));
    assert!(error.contains("try focus_window or raise the window"));
    assert!(!error.contains("actuator returned Failed"));
    let verify = partial.verify.as_ref().unwrap();
    assert!(!verify.ok);
    assert_eq!(verify.checks[0].actual_value.as_deref(), Some(""));
}
