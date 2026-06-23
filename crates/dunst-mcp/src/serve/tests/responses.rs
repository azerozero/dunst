use super::*;

#[test]
fn action_responses_are_compact_unless_full_diff_requested() {
    let mut e = engine();
    let id = text_json(&call(
        &mut e,
        "find_element",
        json!({ "query": "Nouvelle note", "fresh": false }),
    ))[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let compact = call(&mut e, "click_element", json!({ "id": id }));
    let compact_json = text_json(&compact);
    assert!(
        compact_json.get("graph_diff").is_none(),
        "compact response omits full graph_diff"
    );
    assert!(
        compact_json.get("graph_diff_summary").is_some(),
        "compact response carries graph_diff_summary"
    );

    let full = call(
        &mut e,
        "click_element",
        json!({ "id": id, "include_diff": true }),
    );
    let full_json = text_json(&full);
    assert!(
        full_json.get("graph_diff").is_some(),
        "include_diff restores full graph_diff"
    );
}

#[test]
fn diff_summary_sample_prioritizes_useful_changes_over_browser_menu_noise() {
    let diff = GraphDiff {
        changes: vec![
            NodeChange::Changed {
                id: "mi_menuitemhit_35".into(),
                field: "label".into(),
                before: "Toujours afficher".into(),
                after: "".into(),
            },
            NodeChange::Changed {
                id: "mi_menuitemhit_36".into(),
                field: "label".into(),
                before: "Afficher seulement sur la page de nouvel onglet".into(),
                after: "".into(),
            },
            NodeChange::Added {
                id: "btn_ajouter".into(),
                label: Some("Ajouter".into()),
            },
        ],
    };

    let summary = diff_summary_value(&diff, 2);
    let sample = summary["sample"].as_array().unwrap();
    assert_eq!(summary["meaningful_changes"], 1);
    assert_eq!(summary["low_signal_suppressed"], 2);
    assert_eq!(sample.len(), 1);
    assert_eq!(sample[0]["id"], "btn_ajouter");
    assert!(
        sample
            .iter()
            .all(|entry| !entry["id"].as_str().unwrap().starts_with("mi_menuitemhit_")),
        "menu items should not dominate the compact diff sample: {summary}"
    );
}

#[test]
fn diff_summary_suppresses_intercom_and_generated_bbox_noise() {
    let diff = GraphDiff {
        changes: vec![
            NodeChange::Changed {
                id: "el_fermer_le_messenger_intercom".into(),
                field: "parent".into(),
                before: "grp_ddaf9baef4f8ba43".into(),
                after: "grp_a01b7a48660490e2".into(),
            },
            NodeChange::Changed {
                id: "grp_a450dc4b5a2179a1".into(),
                field: "bbox".into(),
                before: "Some(Bbox { x: 720.5, y: 946.0, w: 63.0, h: 63.0 })".into(),
                after: "Some(Bbox { x: 721.5, y: 947.0, w: 61.0, h: 61.0 })".into(),
            },
            NodeChange::Added {
                id: "btn_publier".into(),
                label: Some("Publier".into()),
            },
        ],
    };

    let summary = diff_summary_value(&diff, 5);
    let sample = summary["sample"].as_array().unwrap();

    assert_eq!(summary["meaningful_changes"], 1);
    assert_eq!(summary["low_signal_suppressed"], 2);
    assert_eq!(sample.len(), 1);
    assert_eq!(sample[0]["id"], "btn_publier");
}

#[test]
fn audit_entry_full_diff_also_reports_meaningful_summary() {
    let entry = AuditEntry {
        ts_ms: 42,
        target_id: "txt_rust".into(),
        action: SemanticAction::Click,
        argument: None,
        risk: dunst_core::RiskAssessment::low(),
        reasoning: Some("select Rust".into()),
        result: ActionResult::Success,
        graph_diff: GraphDiff {
            changes: vec![NodeChange::Changed {
                id: "mi_menuitemhit_35".into(),
                field: "label".into(),
                before: "".into(),
                after: "Toujours afficher".into(),
            }],
        },
        caller: None,
    };

    let value = audit_entry_value(entry, true);
    assert!(value.get("graph_diff").is_some());
    assert_eq!(value["graph_diff_summary"]["meaningful_changes"], 0);
    assert_eq!(value["graph_diff_summary"]["low_signal_suppressed"], 1);
    assert!(value["graph_diff_summary"]["sample"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn bbox_only_generated_wrapper_click_gets_verification_hint() {
    let entry = AuditEntry {
        ts_ms: 43,
        target_id: "btn_modifier".into(),
        action: SemanticAction::Click,
        argument: None,
        risk: dunst_core::RiskAssessment::low(),
        reasoning: Some("open modal".into()),
        result: ActionResult::Success,
        graph_diff: GraphDiff {
            changes: vec![NodeChange::Changed {
                id: "grp_a450dc4b5a2179a1".into(),
                field: "bbox".into(),
                before: "Some(Bbox { x: 720.5, y: 946.0, w: 63.0, h: 63.0 })".into(),
                after: "Some(Bbox { x: 721.5, y: 947.0, w: 61.0, h: 61.0 })".into(),
            }],
        },
        caller: None,
    };

    let value = audit_entry_value(entry, false);

    assert_eq!(value["graph_diff_summary"]["meaningful_changes"], 0);
    assert!(value["verification_hint"]["reason"]
        .as_str()
        .unwrap()
        .contains("no meaningful AX graph change"));
}

#[test]
fn typed_audit_summary_reports_whether_target_value_changed() {
    let changed = AuditEntry {
        ts_ms: 42,
        target_id: "field_description".into(),
        action: SemanticAction::Type,
        argument: Some("nouvelle description".into()),
        risk: dunst_core::RiskAssessment::low(),
        reasoning: None,
        result: ActionResult::Success,
        graph_diff: GraphDiff {
            changes: vec![NodeChange::Changed {
                id: "field_description".into(),
                field: "value".into(),
                before: "old".into(),
                after: "nouvelle description".into(),
            }],
        },
        caller: None,
    };
    let value = audit_entry_value(changed, false);
    assert_eq!(
        value["graph_diff_summary"]["typed_content_change_observed"],
        true
    );
    assert_eq!(
        value["graph_diff_summary"]["typed_content_exact_match"],
        true
    );

    let unchanged = AuditEntry {
        ts_ms: 43,
        target_id: "field_description".into(),
        action: SemanticAction::Type,
        argument: Some("nouvelle description".into()),
        risk: dunst_core::RiskAssessment::low(),
        reasoning: None,
        result: ActionResult::Success,
        graph_diff: GraphDiff {
            changes: vec![NodeChange::Changed {
                id: "spinner".into(),
                field: "bbox".into(),
                before: "Some(Bbox { x: 1.0, y: 1.0, w: 1.0, h: 1.0 })".into(),
                after: "Some(Bbox { x: 2.0, y: 2.0, w: 1.0, h: 1.0 })".into(),
            }],
        },
        caller: None,
    };
    let value = audit_entry_value(unchanged, false);
    assert_eq!(
        value["graph_diff_summary"]["typed_content_change_observed"],
        false
    );
    assert_eq!(
        value["graph_diff_summary"]["typed_content_exact_match"],
        false
    );
}

#[test]
fn typed_audit_summary_rejects_partial_target_value() {
    let partial = AuditEntry {
        ts_ms: 43,
        target_id: "field_description".into(),
        action: SemanticAction::Type,
        argument: Some("nouvelle description complete".into()),
        risk: dunst_core::RiskAssessment::low(),
        reasoning: None,
        result: ActionResult::Failed,
        graph_diff: GraphDiff {
            changes: vec![NodeChange::Changed {
                id: "field_description".into(),
                field: "value".into(),
                before: "old".into(),
                after: "uvelle description".into(),
            }],
        },
        caller: None,
    };

    let value = audit_entry_value(partial, false);
    assert_eq!(
        value["graph_diff_summary"]["typed_content_change_observed"],
        true
    );
    assert_eq!(
        value["graph_diff_summary"]["typed_content_exact_match"],
        false
    );
}

#[test]
fn failed_type_audit_includes_do_not_save_hint() {
    let entry = AuditEntry {
        ts_ms: 44,
        target_id: "field_description".into(),
        action: SemanticAction::Type,
        argument: Some("nouvelle description".into()),
        risk: dunst_core::RiskAssessment::low(),
        reasoning: None,
        result: ActionResult::Failed,
        graph_diff: GraphDiff::default(),
        caller: None,
    };

    let value = audit_entry_value(entry, false);
    assert_eq!(
        value["graph_diff_summary"]["typed_content_change_observed"],
        false
    );
    assert_eq!(
        value["graph_diff_summary"]["typed_content_exact_match"],
        false
    );
    assert!(value["failure_hint"]["next_step"]
        .as_str()
        .unwrap()
        .contains("Do not click save"));
}

#[test]
fn failed_checkbox_click_includes_toggle_hint() {
    let entry = AuditEntry {
        ts_ms: 45,
        target_id: "chk_devops".into(),
        action: SemanticAction::Click,
        argument: None,
        risk: dunst_core::RiskAssessment::low(),
        reasoning: None,
        result: ActionResult::Failed,
        graph_diff: GraphDiff::default(),
        caller: None,
    };

    let value = audit_entry_value(entry, false);
    assert!(value["failure_hint"]["reason"]
        .as_str()
        .unwrap()
        .contains("checkbox value did not change"));
}

#[test]
fn failed_latent_menu_item_includes_open_menu_hint() {
    let entry = AuditEntry {
        ts_ms: 46,
        target_id: "mi_selectall".into(),
        action: SemanticAction::Click,
        argument: None,
        risk: dunst_core::RiskAssessment::low(),
        reasoning: None,
        result: ActionResult::Failed,
        graph_diff: GraphDiff::default(),
        caller: None,
    };

    let value = audit_entry_value(entry, false);
    assert!(value["failure_hint"]["reason"]
        .as_str()
        .unwrap()
        .contains("latent AX menu item"));
    assert!(value["failure_hint"]["next_step"]
        .as_str()
        .unwrap()
        .contains("Do not keep retrying latent mi_* ids"));
}

#[test]
fn successful_click_without_meaningful_diff_includes_verification_hint() {
    let entry = AuditEntry {
        ts_ms: 47,
        target_id: "btn_modifier".into(),
        action: SemanticAction::Click,
        argument: None,
        risk: dunst_core::RiskAssessment::low(),
        reasoning: None,
        result: ActionResult::Success,
        graph_diff: GraphDiff::default(),
        caller: None,
    };

    let value = audit_entry_value(entry, false);
    assert!(value["verification_hint"]["reason"]
        .as_str()
        .unwrap()
        .contains("no meaningful AX graph change"));
}

#[test]
fn successful_raw_click_without_meaningful_diff_warns_not_to_retry_same_point() {
    let entry = AuditEntry {
        ts_ms: 48,
        target_id: "screen@4020,692:click".into(),
        action: SemanticAction::Click,
        argument: Some("click 4020,692".into()),
        risk: dunst_core::RiskAssessment::low(),
        reasoning: None,
        result: ActionResult::Success,
        graph_diff: GraphDiff::default(),
        caller: None,
    };

    let value = audit_entry_value(entry, false);
    assert!(value["verification_hint"]["reason"]
        .as_str()
        .unwrap()
        .contains("raw click returned success"));
    assert!(value["verification_hint"]["next_step"]
        .as_str()
        .unwrap()
        .contains("Do not repeat the same raw point"));
    assert!(value["verification_hint"]["next_step"]
        .as_str()
        .unwrap()
        .contains("screenshots are image pixels"));
}

#[test]
fn diff_summary_sample_truncates_large_changed_values() {
    let long_children = (0..80)
        .map(|idx| format!("grp_{idx:02}"))
        .collect::<Vec<_>>()
        .join(",");
    let diff = GraphDiff {
        changes: vec![NodeChange::Changed {
            id: "el_collective".into(),
            field: "children".into(),
            before: "".into(),
            after: long_children.clone(),
        }],
    };

    let summary = diff_summary_value(&diff, 1);
    let after = summary["sample"][0]["after"].as_str().unwrap();
    assert!(after.len() < long_children.len(), "{after}");
    assert!(after.ends_with("..."), "{after}");
}
