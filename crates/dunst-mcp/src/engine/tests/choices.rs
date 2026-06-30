use super::*;

fn choice_roots() -> Vec<dunst_core::RawAxNode> {
    vec![raw_node(
        "AXWindow",
        Some("Checkout"),
        None,
        test_bbox(0.0, 0.0, 700.0, 500.0),
        &[],
        vec![
            raw_node(
                "AXGroup",
                Some("Delivery time *"),
                None,
                test_bbox(20.0, 40.0, 300.0, 120.0),
                &[],
                vec![
                    raw_node(
                        "AXRadioButton",
                        Some("ASAP"),
                        Some("1"),
                        test_bbox(40.0, 70.0, 80.0, 24.0),
                        &["press"],
                        vec![],
                    ),
                    raw_node(
                        "AXRadioButton",
                        Some("Schedule"),
                        Some("0"),
                        test_bbox(40.0, 100.0, 120.0, 24.0),
                        &["press"],
                        vec![],
                    ),
                ],
            ),
            raw_node(
                "AXGroup",
                Some("Extras"),
                None,
                test_bbox(360.0, 40.0, 260.0, 120.0),
                &[],
                vec![
                    raw_node(
                        "AXCheckBox",
                        Some("Cutlery"),
                        Some("0"),
                        test_bbox(380.0, 70.0, 90.0, 24.0),
                        &["press"],
                        vec![],
                    ),
                    raw_node(
                        "AXCheckBox",
                        Some("Napkins"),
                        Some("1"),
                        test_bbox(380.0, 100.0, 90.0, 24.0),
                        &["press"],
                        vec![],
                    ),
                ],
            ),
            raw_node(
                "AXTextArea",
                Some("Note to courier"),
                Some(""),
                test_bbox(40.0, 210.0, 300.0, 80.0),
                &[],
                vec![],
            ),
        ],
    )]
}

#[test]
fn enumerate_classifies_radios_as_single_select_and_checkboxes_as_multi() {
    let (mut eng, _) = engine_from_roots(choice_roots(), "CheckoutApp", "Checkout");

    let model = eng
        .enumerate_choices(EnumerateOpts {
            scope: "page",
            include_latent: true,
            scroll_scan: false,
            max_scroll_pages: 1,
            limit: 100,
        })
        .unwrap();

    let single = model
        .groups
        .iter()
        .find(|group| group.kind == GroupKind::SingleSelect)
        .expect("radio group");
    assert_eq!(single.choices.len(), 2);
    assert_eq!(
        single
            .choices
            .iter()
            .filter(|choice| choice.state == SelectionState::Selected)
            .count(),
        1
    );

    let multi = model
        .groups
        .iter()
        .find(|group| group.kind == GroupKind::MultiSelect)
        .expect("checkbox group");
    assert_eq!(multi.choices.len(), 2);
    assert!(multi.choices.iter().any(|choice| choice.label == "Cutlery"));
}

#[test]
fn enumerate_marks_required_group_from_label_markers() {
    let (mut eng, _) = engine_from_roots(choice_roots(), "CheckoutApp", "Checkout");

    let model = eng
        .enumerate_choices(EnumerateOpts {
            scope: "page",
            include_latent: true,
            scroll_scan: false,
            max_scroll_pages: 1,
            limit: 100,
        })
        .unwrap();

    let delivery = model
        .groups
        .iter()
        .find(|group| group.label.as_deref() == Some("Delivery time *"))
        .expect("delivery group");
    assert_eq!(delivery.requirement, Requirement::Required);
}

#[test]
fn enumerate_ax_latent_captures_offscreen_choices_without_scroll() {
    let roots = vec![raw_node(
        "AXWindow",
        Some("Checkout"),
        None,
        test_bbox(0.0, 0.0, 700.0, 500.0),
        &[],
        vec![raw_node(
            "AXGroup",
            Some("Delivery time"),
            None,
            test_bbox(20.0, 40.0, 300.0, 120.0),
            &[],
            vec![raw_node(
                "AXRadioButton",
                Some("Tomorrow"),
                Some("0"),
                None,
                &["press"],
                vec![],
            )],
        )],
    )];
    let (mut eng, _) = engine_from_roots(roots, "CheckoutApp", "Checkout");

    let model = eng
        .enumerate_choices(EnumerateOpts {
            scope: "page",
            include_latent: true,
            scroll_scan: false,
            max_scroll_pages: 1,
            limit: 100,
        })
        .unwrap();

    assert!(model
        .groups
        .iter()
        .flat_map(|group| &group.choices)
        .any(|choice| choice.label == "Tomorrow"));
}

#[test]
fn enumerate_scroll_scan_restores_origin_and_sets_coverage_complete() {
    let (mut eng, _) = engine_from_roots(choice_roots(), "CheckoutApp", "Checkout");
    let before = eng.current_ui_epoch_fingerprint();

    let model = eng
        .enumerate_choices(EnumerateOpts {
            scope: "page",
            include_latent: true,
            scroll_scan: true,
            max_scroll_pages: 1,
            limit: 100,
        })
        .unwrap();

    assert_eq!(model.coverage, Coverage::Complete);
    assert_eq!(eng.current_ui_epoch_fingerprint(), before);
}

#[test]
fn enumerate_partial_coverage_returns_scroll_plan() {
    let (eng, _) = engine_from_roots(choice_roots(), "CheckoutApp", "Checkout");
    let mut result = eng.hit_targets(true, "page", 100, None);
    let scroll = result
        .targets
        .iter()
        .find(|target| target.id == "page@scroll:down")
        .cloned()
        .expect("scroll target");
    let ocr_choice = HitTarget {
        id: "ocr_text_delivery_window".to_string(),
        source: "ocr".to_string(),
        role: "button",
        label: Some("Delivery window".to_string()),
        value: None,
        bbox: Some(Bbox {
            x: 80.0,
            y: 320.0,
            w: 120.0,
            h: 24.0,
        }),
        safe_click: None,
        confidence: 0.8,
        action_modes: vec![HitActionMode {
            action: SemanticAction::Click,
            tool_hint: "click_near_text".to_string(),
            target_id: Some("ocr_text_delivery_window".to_string()),
            arguments: None,
            drop_targets: Vec::new(),
            risk: RiskAssessment::low(),
        }],
        risk: RiskAssessment::low(),
    };
    result.targets = vec![scroll.clone(), ocr_choice.clone()];

    let model = eng.choice_model_from_targets_for_test(&result, vec![scroll, ocr_choice], "page");

    assert_eq!(model.coverage, Coverage::Partial);
    assert_eq!(model.scroll_plan.len(), 1);
}
