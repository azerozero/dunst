//! Unit tests for `read.rs` and its `epoch`/`hit_target` submodules: risk
//! flooring, menu-bar epoch exclusion, OCR-form pairing, safe-click masking, and
//! the UI-fingerprint stability/sensitivity contract.

use super::*;

#[test]
fn raw_ocr_click_risk_floors_benign_text_to_the_executor_gate() {
    // A benign OCR label (e.g. a tab title) carries no destructive keyword,
    // so assess_text returns a no-approval risk. But the only way to click
    // an OCR hit is raw pointer input, which ocr_point_risk_at always
    // approval-gates. The advertised affordance risk must match that gate.
    let benign = RiskAssessment {
        level: RiskLevel::Low,
        requires_approval: false,
        reasons: vec!["benign text".into()],
    };
    let floored = raw_ocr_click_risk(benign);
    assert_eq!(floored.level, RiskLevel::High);
    assert!(
        floored.requires_approval,
        "OCR click affordance must advertise the same approval gate the executor enforces"
    );
    assert!(
        floored
            .reasons
            .iter()
            .any(|reason| reason.contains("raw OCR input")),
        "floored risk should explain the raw-input delivery: {:?}",
        floored.reasons
    );
    assert!(
        floored.reasons.iter().any(|reason| reason == "benign text"),
        "text-derived reasons must be preserved: {:?}",
        floored.reasons
    );
}

#[test]
fn menu_bar_nodes_are_excluded_from_epoch() {
    use std::collections::BTreeMap;

    fn node(id: &str, role: Role, parent: Option<&str>) -> SceneNode {
        SceneNode {
            id: id.into(),
            role,
            ax_role: String::new(),
            label: None,
            help: None,
            value: None,
            bbox: None,
            confidence: 1.0,
            source: dunst_core::Source::Accessibility,
            enabled: true,
            focused: false,
            ax_actions: Vec::new(),
            ax_identifier: None,
            cmd_char: None,
            cmd_modifiers: None,
            cmd_virtual_key: None,
            last_seen_ms: 0,
            path: Vec::new(),
            parent: parent.map(str::to_string),
            children: Vec::new(),
        }
    }

    let mut nodes = BTreeMap::new();
    for n in [
        node("menubar", Role::MenuBar, None),
        node("mi_searchfieldaction", Role::MenuItem, Some("menubar")),
        node(
            "field_sc_search_field",
            Role::TextField,
            Some("mi_searchfieldaction"),
        ),
        node("web_area", Role::Group, None),
        node("field_page", Role::TextField, Some("web_area")),
    ] {
        nodes.insert(n.id.clone(), n);
    }
    let graph = SceneGraph {
        nodes,
        roots: vec!["menubar".into(), "web_area".into()],
        captured_at_ms: 0,
        window: dunst_core::WindowRef::default(),
    };

    // The menu bar, its items, and the Help-menu search field nested under
    // them are all excluded — their bboxes flip between displays.
    for menu_id in ["menubar", "mi_searchfieldaction", "field_sc_search_field"] {
        assert!(
            node_in_menu_bar(&graph, graph.get(menu_id).unwrap(), Some("menubar")),
            "{menu_id} must count as menu bar and stay out of the epoch"
        );
    }
    // A genuine page field remains part of the fingerprint.
    assert!(
        !node_in_menu_bar(&graph, graph.get("field_page").unwrap(), Some("menubar")),
        "a real page field must remain part of the epoch fingerprint"
    );
}

#[test]
fn page_scroll_bbox_does_not_mask_ocr_targets() {
    let page = HitTarget {
        id: "page@scroll:down".into(),
        source: "page".into(),
        role: "group",
        label: None,
        value: None,
        bbox: Some(Bbox {
            x: 0.0,
            y: 0.0,
            w: 1_000.0,
            h: 800.0,
        }),
        safe_click: None,
        confidence: 0.65,
        action_modes: Vec::new(),
        risk: RiskAssessment::low(),
    };

    assert!(
        !bbox_duplicate_of_existing(
            Bbox {
                x: 100.0,
                y: 120.0,
                w: 80.0,
                h: 20.0,
            },
            &[page],
        ),
        "page pseudo-targets are scroll surfaces, not real semantic duplicates"
    );
}

#[test]
fn ocr_form_label_maps_to_following_value_target() {
    let label = TextHit {
        text: "Titre de la réalisation O".into(),
        bbox: Bbox {
            x: 3141.0,
            y: 722.0,
            w: 138.0,
            h: 12.0,
        },
        confidence: 1.0,
    };
    let value = TextHit {
        text: "openai/gpt-5.5".into(),
        bbox: Bbox {
            x: 3150.0,
            y: 751.0,
            w: 98.0,
            h: 14.0,
        },
        confidence: 1.0,
    };

    let kind = ocr_form_label_kind(&label.text).expect("label detected");
    let candidates = [value.clone()];
    let found =
        nearest_ocr_field_value(&label, &candidates, kind).expect("following value detected");
    assert_eq!(found.text, value.text);
    assert!(ocr_form_field_x_aligned(label.bbox, value.bbox));
}

#[test]
fn ui_fingerprint_ignores_volatile_free_text_and_cosmetic_jitter() {
    let base = test_engine(live_text_roots(
        "Livraison dans 12 min",
        "Maintenant",
        42.2,
        true,
    ));
    let changed_text = test_engine(live_text_roots(
        "Livraison dans 11 min",
        "Promo expire a 12:31",
        42.6,
        true,
    ));

    assert_eq!(
        test_fingerprint(&base, None, test_visibility(0.991, "visible")),
        test_fingerprint(&changed_text, None, test_visibility(0.999, "visible"))
    );
}

#[test]
fn ui_fingerprint_changes_for_structural_and_control_state_mutations() {
    let base = test_fingerprint(
        &test_engine(structural_roots(StructuralMutation::None)),
        None,
        test_visibility(1.0, "visible"),
    );

    for (mutation, reason) in [
        (StructuralMutation::AddedNode, "node added"),
        (StructuralMutation::RemovedNode, "node removed"),
        (StructuralMutation::RoleChanged, "role changed"),
        (StructuralMutation::SelectionChanged, "selection changed"),
        (StructuralMutation::EnabledChanged, "enabled changed"),
        (StructuralMutation::BboxMoved, "bbox moved meaningfully"),
    ] {
        let changed = test_fingerprint(
            &test_engine(structural_roots(mutation)),
            None,
            test_visibility(1.0, "visible"),
        );
        assert_ne!(changed, base, "{reason}");
    }
}

#[test]
fn ui_fingerprint_ignores_transient_caret_nodes_and_path_shift() {
    let base = test_fingerprint(
        &test_engine(structural_roots_with_transient_caret(false)),
        None,
        test_visibility(1.0, "visible"),
    );
    let focused = test_fingerprint(
        &test_engine(structural_roots_with_transient_caret(true)),
        None,
        test_visibility(1.0, "visible"),
    );

    assert_eq!(focused, base);
}

#[test]
fn ui_fingerprint_changes_for_tab_and_visibility_identity() {
    let engine = test_engine(structural_roots(StructuralMutation::None));
    let tab = BrowserTab {
        id: "tab-1".into(),
        title: "Restaurant".into(),
        selected: true,
        url: Some("https://example.test/restaurant".into()),
        bbox: Some(test_bbox(8.0, 8.0, 180.0, 28.0)),
    };
    let base = test_fingerprint(&engine, Some(tab.clone()), test_visibility(1.0, "visible"));

    let mut changed = tab.clone();
    changed.id = "tab-2".into();
    assert_ne!(
        test_fingerprint(&engine, Some(changed), test_visibility(1.0, "visible")),
        base,
        "tab id changed"
    );

    let mut changed = tab.clone();
    changed.url = Some("https://example.test/checkout".into());
    assert_ne!(
        test_fingerprint(&engine, Some(changed), test_visibility(1.0, "visible")),
        base,
        "tab url changed"
    );

    let mut changed = tab;
    changed.selected = false;
    assert_ne!(
        test_fingerprint(&engine, Some(changed), test_visibility(1.0, "visible")),
        base,
        "tab selected changed"
    );

    assert_ne!(
        test_fingerprint(&engine, None, test_visibility(1.0, "covered")),
        test_fingerprint(&engine, None, test_visibility(1.0, "visible")),
        "visibility status changed"
    );
}

#[derive(Clone, Copy)]
enum StructuralMutation {
    None,
    AddedNode,
    RemovedNode,
    RoleChanged,
    SelectionChanged,
    EnabledChanged,
    BboxMoved,
}

fn test_engine(roots: Vec<dunst_core::RawAxNode>) -> Engine {
    Engine::new(
        Box::new(dunst_core::mock::MockPerceptor::new(
            roots,
            WindowRef {
                pid: 1,
                window_id: 1,
                app_name: "TestApp".into(),
                title: "Checkout".into(),
            },
        )),
        Box::new(dunst_core::mock::RecordingExecutor::default()),
        Target {
            pid: 1,
            window_id: 1,
        },
    )
    .unwrap()
}

fn test_fingerprint(
    engine: &Engine,
    browser_tab: Option<BrowserTab>,
    visibility: TargetVisibility,
) -> String {
    engine.ui_fingerprint(
        test_bbox(0.0, 0.0, 700.0, 500.0),
        browser_tab.as_ref(),
        &visibility,
        engine.affordance_graph(),
    )
}

fn live_text_roots(
    volatile_label: &str,
    volatile_value: &str,
    x: f64,
    focused: bool,
) -> Vec<dunst_core::RawAxNode> {
    let mut volatile_text = raw_node(
        "AXStaticText",
        Some(volatile_label),
        Some(volatile_value),
        test_bbox_opt(x, 70.0, 160.0, 24.0),
        true,
        &[],
        vec![],
    );
    volatile_text.focused = focused;
    vec![raw_node(
        "AXWindow",
        Some("Checkout"),
        None,
        test_bbox_opt(0.0, 0.0, 700.0, 500.0),
        true,
        &[],
        vec![
            volatile_text,
            raw_node(
                "AXButton",
                Some("Add"),
                None,
                test_bbox_opt(40.0, 120.0, 90.0, 30.0),
                true,
                &["press"],
                vec![],
            ),
        ],
    )]
}

fn structural_roots(mutation: StructuralMutation) -> Vec<dunst_core::RawAxNode> {
    let checkbox_value = match mutation {
        StructuralMutation::SelectionChanged => Some("1"),
        _ => Some("0"),
    };
    let button_role = match mutation {
        StructuralMutation::RoleChanged => "AXStaticText",
        _ => "AXButton",
    };
    let button_enabled = !matches!(mutation, StructuralMutation::EnabledChanged);
    let button_bbox = match mutation {
        StructuralMutation::BboxMoved => test_bbox_opt(72.0, 120.0, 90.0, 30.0),
        _ => test_bbox_opt(40.0, 120.0, 90.0, 30.0),
    };
    let mut children = vec![
        raw_node(
            button_role,
            Some("Add"),
            None,
            button_bbox,
            button_enabled,
            &["press"],
            vec![],
        ),
        raw_node(
            "AXCheckBox",
            Some("Cutlery"),
            checkbox_value,
            test_bbox_opt(40.0, 170.0, 110.0, 24.0),
            true,
            &["press"],
            vec![],
        ),
    ];
    if !matches!(mutation, StructuralMutation::RemovedNode) {
        children.push(raw_node(
            "AXStaticText",
            Some("Stable footer"),
            None,
            test_bbox_opt(40.0, 220.0, 120.0, 24.0),
            true,
            &[],
            vec![],
        ));
    }
    if matches!(mutation, StructuralMutation::AddedNode) {
        children.push(raw_node(
            "AXStaticText",
            Some("New footer"),
            None,
            test_bbox_opt(40.0, 250.0, 120.0, 24.0),
            true,
            &[],
            vec![],
        ));
    }
    vec![raw_node(
        "AXWindow",
        Some("Checkout"),
        None,
        test_bbox_opt(0.0, 0.0, 700.0, 500.0),
        true,
        &[],
        children,
    )]
}

fn structural_roots_with_transient_caret(insert_caret: bool) -> Vec<dunst_core::RawAxNode> {
    let mut children = Vec::new();
    if insert_caret {
        children.push(raw_node(
            "AXInsertionPoint",
            None,
            None,
            test_bbox_opt(41.0, 126.0, 1.0, 18.0),
            true,
            &[],
            vec![],
        ));
    }
    children.extend([
        raw_node(
            "AXButton",
            Some("Add"),
            None,
            test_bbox_opt(40.0, 120.0, 90.0, 30.0),
            true,
            &["press"],
            vec![],
        ),
        raw_node(
            "AXCheckBox",
            Some("Cutlery"),
            Some("0"),
            test_bbox_opt(40.0, 170.0, 110.0, 24.0),
            true,
            &["press"],
            vec![],
        ),
        raw_node(
            "AXStaticText",
            Some("Stable footer"),
            None,
            test_bbox_opt(40.0, 220.0, 120.0, 24.0),
            true,
            &[],
            vec![],
        ),
    ]);
    vec![raw_node(
        "AXWindow",
        Some("Checkout"),
        None,
        test_bbox_opt(0.0, 0.0, 700.0, 500.0),
        true,
        &[],
        children,
    )]
}

fn raw_node(
    ax_role: &str,
    label: Option<&str>,
    value: Option<&str>,
    frame: Option<Bbox>,
    enabled: bool,
    ax_actions: &[&str],
    children: Vec<dunst_core::RawAxNode>,
) -> dunst_core::RawAxNode {
    dunst_core::RawAxNode {
        ax_role: ax_role.into(),
        label: label.map(str::to_owned),
        help: None,
        value: value.map(str::to_owned),
        ax_identifier: None,
        cmd_char: None,
        cmd_modifiers: None,
        cmd_virtual_key: None,
        ax_actions: ax_actions.iter().map(|action| action.to_string()).collect(),
        frame,
        enabled,
        focused: false,
        children,
    }
}

fn test_bbox_opt(x: f64, y: f64, w: f64, h: f64) -> Option<Bbox> {
    Some(test_bbox(x, y, w, h))
}

fn test_bbox(x: f64, y: f64, w: f64, h: f64) -> Bbox {
    Bbox { x, y, w, h }
}

fn test_visibility(visible_fraction: f64, status: &str) -> TargetVisibility {
    TargetVisibility {
        target_window_id: 1,
        target_title: "Checkout".into(),
        found_in_desktop: true,
        degraded: false,
        reason: None,
        is_frontmost: true,
        covered_by: Vec::new(),
        covers: Vec::new(),
        visible_fraction,
        status: status.into(),
        warnings: Vec::new(),
        fallback_hint: None,
    }
}
