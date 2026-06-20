use super::*;

#[test]
fn page_state_is_lightweight_orientation_snapshot() {
    let (eng, _) = engine_with_counter();
    let state = eng.page_state(8);
    assert_eq!(state.target.pid, 1363);
    assert_eq!(state.title, "Notes – Aucune note");
    assert!(state.key_elements.len() <= 8);
    assert!(
        state.key_elements.iter().all(|e| e.role != "menu_bar"),
        "page_state should not spend key-element budget on menu bar chrome"
    );
    assert!(
        state
            .key_elements
            .iter()
            .any(|e| e.label.as_deref() == Some("Nouvelle note")),
        "page_state should include key visible actions"
    );
}

#[test]
fn page_state_does_not_use_window_root_as_key_element() {
    let json = r#"[
          {
            "ax_role": "AXWindow",
            "label": "jarvis github - Recherche Google",
            "ax_actions": ["raise"],
            "frame": { "x": 0, "y": 32, "w": 2560, "h": 1326 }
          }
        ]"#;
    let eng = engine_from_json(json, "Zen", "jarvis github - Recherche Google");
    let state = eng.page_state(10);
    assert!(
        state.key_elements.is_empty(),
        "window root should not consume page_state key-element budget: {:?}",
        state.key_elements
    );
}

#[test]
fn page_state_drops_unlabeled_full_size_unknown_containers() {
    let window = raw_node(
        "AXWindow",
        Some("Collective"),
        None,
        test_bbox(2560.0, 440.0, 1728.0, 1000.0),
        &[],
        vec![
            raw_node(
                "AXUnknown",
                None,
                None,
                test_bbox(2560.0, 440.0, 1728.0, 1000.0),
                &["press"],
                vec![],
            ),
            raw_node(
                "AXButton",
                Some("Modifier"),
                None,
                test_bbox(3627.0, 1306.0, 81.0, 32.0),
                &["press"],
                vec![],
            ),
        ],
    );
    let (eng, _) = engine_from_roots(vec![window], "Firefox", "Collective");

    let state = eng.page_state(2);
    assert!(
        state
            .key_elements
            .iter()
            .all(|element| element.role != "unknown"),
        "unlabeled full-size unknown containers should be suppressed: {:?}",
        state.key_elements
    );
    assert!(
        state
            .key_elements
            .iter()
            .any(|element| element.label.as_deref() == Some("Modifier")),
        "real action should stay visible: {:?}",
        state.key_elements
    );
}

#[test]
fn verify_state_supports_focused_field() {
    let mut description = raw_node(
        "AXTextArea",
        Some("Description"),
        Some("Texte"),
        test_bbox(40.0, 80.0, 240.0, 120.0),
        &["press"],
        vec![],
    );
    description.focused = true;
    let window = raw_node(
        "AXWindow",
        Some("Form"),
        None,
        test_bbox(0.0, 0.0, 500.0, 400.0),
        &[],
        vec![description],
    );
    let (eng, _) = engine_from_roots(vec![window], "Browser", "Form");
    let field = id_for(&eng, "Description");

    assert!(eng.verify_state(&field, "focused", "true").unwrap());
    assert!(!eng.verify_state(&field, "focused", "false").unwrap());
}

#[test]
fn raise_element_executes_raise_affordance() {
    let window = raw_node(
        "AXWindow",
        Some("Collective"),
        None,
        test_bbox(0.0, 0.0, 500.0, 400.0),
        &["raise"],
        vec![],
    );
    let (mut eng, calls) = engine_from_roots(vec![window], "Firefox", "Collective");
    let id = id_for(&eng, "Collective");

    let pending = eng
        .raise_element(&id, Some("bring target window forward"))
        .unwrap();
    assert_eq!(pending.result, ActionResult::PendingApproval);
    assert!(pending.risk.requires_approval);
    assert_eq!(calls.lock().unwrap().len(), 0);

    eng.approve(&id).unwrap();
    let entry = eng
        .raise_element(&id, Some("approved foreground raise"))
        .unwrap();
    assert_eq!(entry.result, ActionResult::Success);
    let recorded = calls.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].0, id);
    assert_eq!(recorded[0].1, SemanticAction::Raise);
}

#[test]
fn click_element_presses_ax_actionable_button_outside_viewport() {
    let window = raw_node(
        "AXWindow",
        Some("Long modal"),
        None,
        test_bbox(0.0, 0.0, 500.0, 800.0),
        &[],
        vec![raw_node(
            "AXButton",
            Some("Sauvegarder"),
            None,
            test_bbox(40.0, 1469.0, 140.0, 36.0),
            &["press"],
            vec![],
        )],
    );
    let (mut eng, calls) = engine_from_roots(vec![window], "Browser", "Long modal");
    assert!(
        eng.find_element_filtered("Sauvegarder", true).is_empty(),
        "visible_only should still hide off-viewport controls"
    );

    let save = id_for(&eng, "Sauvegarder");
    let entry = eng.click_element(&save, Some("save long modal")).unwrap();
    assert_eq!(entry.result, ActionResult::Success);
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(calls.lock().unwrap()[0].0, save);
}

#[test]
fn page_state_filters_repeated_remove_buttons_from_key_budget() {
    let mut children = Vec::new();
    for idx in 0..100 {
        children.push(raw_node(
            "AXButton",
            Some("Remove"),
            None,
            test_bbox(20.0, 60.0 + idx as f64 * 10.0, 22.0, 8.0),
            &["press"],
            vec![],
        ));
    }
    children.push(raw_node(
        "AXButton",
        Some("Sauvegarder"),
        None,
        test_bbox(260.0, 80.0, 140.0, 36.0),
        &["press"],
        vec![],
    ));
    let window = raw_node(
        "AXWindow",
        Some("Expertises"),
        None,
        test_bbox(0.0, 0.0, 700.0, 1200.0),
        &[],
        children,
    );
    let (eng, _) = engine_from_roots(vec![window], "Browser", "Expertises");

    let state = eng.page_state(8);
    assert!(
        state
            .key_elements
            .iter()
            .all(|e| e.label.as_deref() != Some("Remove")),
        "repeated destructive buttons should not consume page_state budget: {:?}",
        state.key_elements
    );
    assert!(
        state
            .key_elements
            .iter()
            .any(|e| e.label.as_deref() == Some("Sauvegarder")),
        "useful controls must remain visible in the compact summary"
    );
}

#[test]
fn page_state_drops_tiny_technical_controls_from_key_budget() {
    let window = raw_node(
        "AXWindow",
        Some("Expertises"),
        None,
        test_bbox(0.0, 0.0, 700.0, 900.0),
        &[],
        vec![
            raw_node(
                "AXCheckBox",
                Some("Rust"),
                None,
                test_bbox(120.0, 180.0, 1.0, 1.0),
                &["press"],
                vec![],
            ),
            raw_node(
                "AXButton",
                Some("Ajouter"),
                None,
                test_bbox(260.0, 180.0, 120.0, 36.0),
                &["press"],
                vec![],
            ),
        ],
    );
    let (eng, _) = engine_from_roots(vec![window], "Firefox", "Expertises");

    let state = eng.page_state(8);
    assert!(
        state.key_elements.iter().all(|e| e.id != "chk_rust"),
        "1x1 technical checkbox should not consume page_state budget: {:?}",
        state.key_elements
    );
    assert!(
        state
            .key_elements
            .iter()
            .any(|e| e.label.as_deref() == Some("Ajouter")),
        "real visible action should remain present: {:?}",
        state.key_elements
    );
}

#[test]
fn pick_option_resolves_static_text_to_clickable_parent() {
    let window = raw_node(
        "AXWindow",
        Some("Options"),
        None,
        test_bbox(0.0, 0.0, 600.0, 500.0),
        &[],
        vec![raw_node(
            "AXGroup",
            None,
            None,
            test_bbox(40.0, 100.0, 300.0, 36.0),
            &["press"],
            vec![raw_node(
                "AXStaticText",
                Some("Disponibilité Collective"),
                None,
                test_bbox(56.0, 108.0, 220.0, 18.0),
                &[],
                vec![],
            )],
        )],
    );
    let (mut eng, calls) = engine_from_roots(vec![window], "Browser", "Options");
    let text = id_for(&eng, "Disponibilité Collective");

    let click = eng
        .click_element(&text, Some("select option text"))
        .unwrap();
    assert_eq!(click.result, ActionResult::Success);
    let clicked_id = calls.lock().unwrap()[0].0.clone();
    assert_ne!(clicked_id, text, "static text should resolve to its parent");
    assert!(clicked_id.starts_with("grp_"));

    let picked = eng
        .pick_option("Disponibilité Collective", true, Some("select option"))
        .unwrap();
    assert_eq!(picked.audit.result, ActionResult::Success);
    assert_eq!(picked.matched_id, text);
    assert_eq!(picked.action_id, clicked_id);
}

#[test]
fn pick_option_reads_french_selected_state_after_normalization() {
    let window = raw_node(
        "AXWindow",
        Some("Options"),
        None,
        test_bbox(0.0, 0.0, 600.0, 500.0),
        &[],
        vec![raw_node(
            "AXGroup",
            None,
            Some("Sélectionné"),
            test_bbox(40.0, 100.0, 300.0, 36.0),
            &["press"],
            vec![raw_node(
                "AXStaticText",
                Some("Disponibilité Collective"),
                None,
                test_bbox(56.0, 108.0, 220.0, 18.0),
                &[],
                vec![],
            )],
        )],
    );
    let (mut eng, _) = engine_from_roots(vec![window], "Browser", "Options");

    let picked = eng
        .pick_option("Disponibilité Collective", true, Some("select option"))
        .unwrap();
    assert_eq!(picked.selected_before, Some(true));
    assert_eq!(picked.selected_after, Some(true));
}

#[test]
fn parent_resolution_does_not_bypass_high_risk_static_text() {
    let window = raw_node(
        "AXWindow",
        Some("Options"),
        None,
        test_bbox(0.0, 0.0, 600.0, 500.0),
        &[],
        vec![raw_node(
            "AXGroup",
            None,
            None,
            test_bbox(40.0, 100.0, 300.0, 36.0),
            &["press"],
            vec![raw_node(
                "AXStaticText",
                Some("Remove expertise"),
                None,
                test_bbox(56.0, 108.0, 220.0, 18.0),
                &[],
                vec![],
            )],
        )],
    );
    let (mut eng, calls) = engine_from_roots(vec![window], "Browser", "Options");
    let text = id_for(&eng, "Remove expertise");

    let err = eng
        .click_element(&text, Some("remove via text"))
        .unwrap_err();
    assert!(matches!(err, VisualOpsError::ActionUnavailable { .. }));
    assert!(
        calls.lock().unwrap().is_empty(),
        "high-risk static text must not execute through an unlabeled parent"
    );
}

#[test]
fn text_snapshot_filters_browser_chrome_but_keeps_page_text() {
    let json = r#"[
          {
            "ax_role": "AXWindow",
            "label": "Claude",
            "frame": { "x": 100, "y": 100, "w": 1000, "h": 800 },
            "children": [
              {
                "ax_role": "AXRadioButton",
                "label": "Premortem tab",
                "ax_actions": ["press"],
                "frame": { "x": 120, "y": 112, "w": 220, "h": 36 },
                "children": [
                  {
                    "ax_role": "AXStaticText",
                    "label": "Premortem tab",
                    "frame": { "x": 150, "y": 122, "w": 120, "h": 16 }
                  }
                ]
              },
              {
                "ax_role": "AXButton",
                "label": "Actualiser",
                "ax_actions": ["press"],
                "frame": { "x": 130, "y": 175, "w": 36, "h": 36 }
              },
              {
                "ax_role": "AXGroup",
                "frame": { "x": 120, "y": 230, "w": 850, "h": 620 },
                "children": [
                  {
                    "ax_role": "AXStaticText",
                    "label": "Final verdict: NO-GO until warm-up is done",
                    "frame": { "x": 150, "y": 260, "w": 420, "h": 22 }
                  }
                ]
              }
            ]
          }
        ]"#;
    let eng = engine_from_json(json, "Firefox", "Premortem - Claude");

    let snippets = eng.text_snapshot(None, true, 20);
    assert!(snippets.iter().any(|s| s.text.contains("Final verdict")));
    assert!(snippets.iter().all(|s| s.text != "Premortem tab"));

    let state = eng.page_state(20);
    assert!(state
        .visible_text
        .iter()
        .any(|s| s.contains("Final verdict")));
    assert!(state.visible_text.iter().all(|s| s != "Premortem tab"));
    assert!(state
        .key_elements
        .iter()
        .all(|e| e.label.as_deref() != Some("Actualiser")));
}

#[test]
fn text_snapshot_filters_web_app_navigation_chrome() {
    let json = r#"[
          {
            "ax_role": "AXWindow",
            "label": "Collective",
            "frame": { "x": 2560, "y": 440, "w": 1728, "h": 1000 },
            "children": [
              {
                "ax_role": "AXStaticText",
                "label": "Accueil",
                "frame": { "x": 2612, "y": 536, "w": 49, "h": 18 }
              },
              {
                "ax_role": "AXStaticText",
                "label": "www.collective.work/profile/clement-liard",
                "frame": { "x": 2888, "y": 557, "w": 237, "h": 15 }
              },
              {
                "ax_role": "AXStaticText",
                "label": "Connect",
                "frame": { "x": 2576, "y": 577, "w": 49, "h": 15 }
              },
              {
                "ax_role": "AXButton",
                "label": "copy",
                "ax_actions": ["press"],
                "frame": { "x": 3133, "y": 556, "w": 16, "h": 16 }
              },
              {
                "ax_role": "AXButton",
                "label": "Open Intercom Messenger",
                "ax_actions": ["press"],
                "frame": { "x": 4196, "y": 1350, "w": 48, "h": 48 }
              },
              {
                "ax_role": "AXButton",
                "label": "Modifier les informations principales",
                "ax_actions": ["press"],
                "frame": { "x": 3181, "y": 669, "w": 32, "h": 32 }
              },
              {
                "ax_role": "AXStaticText",
                "label": "Freelance Architecte DevSecOps & IA souveraine",
                "frame": { "x": 3343, "y": 721, "w": 500, "h": 22 }
              }
            ]
          }
        ]"#;
    let eng = engine_from_json(json, "Firefox", "Collective");

    let snippets = eng.text_snapshot(None, true, 20);
    let texts: Vec<&str> = snippets.iter().map(|s| s.text.as_str()).collect();
    assert!(texts
        .iter()
        .any(|text| text.contains("Freelance Architecte DevSecOps")));
    assert!(!texts.contains(&"Accueil"));
    assert!(!texts.contains(&"Connect"));
    assert!(!texts
        .iter()
        .any(|text| text.contains("collective.work/profile")));

    let state = eng.page_state(20);
    assert!(state
        .visible_text
        .iter()
        .any(|text| text.contains("Freelance Architecte DevSecOps")));
    assert!(state.visible_text.iter().all(|text| text != "Accueil"));
    assert!(state
        .key_elements
        .iter()
        .all(|element| element.label.as_deref() != Some("copy")));
    assert!(state
        .key_elements
        .iter()
        .all(|element| { element.label.as_deref() != Some("Open Intercom Messenger") }));
    assert!(state.key_elements.iter().any(|element| {
        element.label.as_deref() == Some("Modifier les informations principales")
    }));
}

#[test]
fn text_snapshot_query_matches_whole_words_not_substrings_inside_words() {
    let window = raw_node(
        "AXWindow",
        Some("Expertises"),
        None,
        test_bbox(0.0, 0.0, 800.0, 600.0),
        &[],
        vec![
            raw_node(
                "AXStaticText",
                Some("Zero Trust"),
                None,
                test_bbox(20.0, 180.0, 120.0, 20.0),
                &[],
                vec![],
            ),
            raw_node(
                "AXStaticText",
                Some("Rust"),
                None,
                test_bbox(20.0, 120.0, 80.0, 20.0),
                &[],
                vec![],
            ),
        ],
    );
    let (eng, _) = engine_from_roots(vec![window], "Firefox", "Expertises");

    let rust = eng.text_snapshot(Some("Rust"), false, 10);
    assert_eq!(rust.len(), 1);
    assert_eq!(rust[0].text, "Rust");

    let trust = eng.text_snapshot(Some("Trust"), false, 10);
    assert_eq!(trust.len(), 1);
    assert_eq!(trust[0].text, "Zero Trust");
}
