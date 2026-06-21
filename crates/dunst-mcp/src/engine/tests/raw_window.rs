use super::*;

#[test]
fn user_active_guard_retry_runs_once_before_returning() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_in_closure = attempts.clone();
    let result = retry_user_active_guard_after(Duration::from_millis(0), || {
            if attempts_in_closure.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(VisualOpsError::Execution(
                    "user-active guard blocked hover_at: last keyboard/mouse input was 1 ms ago (< 300 ms)".into(),
                ))
            } else {
                Ok("ok")
            }
        })
        .unwrap();

    assert_eq!(result, "ok");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[test]
fn internal_hover_lead_point_is_clamped_to_target_window() {
    let (eng, _) = engine_with_counter();
    let window = eng.current_window_bounds();
    let (x, y) = eng.clamp_point_to_target_window(window.x - 8.0, window.y - 8.0);
    assert!(point_in_bbox((x, y), window));
    assert_eq!(x, window.x);
    assert_eq!(y, window.y);
}

#[test]
fn text_snapshot_returns_visible_ax_text_without_full_graph() {
    let (eng, _) = engine_with_counter();
    let snippets = eng.text_snapshot(Some("Corps de la note"), true, 10);
    assert_eq!(snippets.len(), 1);
    assert_eq!(snippets[0].role, "text_area");
    assert_eq!(snippets[0].text, "Corps de la note");
    assert!(snippets[0].visible);
}

#[test]
fn terminal_ocr_fallback_reads_ax_text_area_value() {
    let window = raw_node(
        "AXWindow",
        Some("iTerm2"),
        None,
        test_bbox(0.0, 0.0, 800.0, 600.0),
        &[],
        vec![raw_node(
            "AXTextArea",
            None,
            Some("cargo test\nfinished ok"),
            test_bbox(10.0, 10.0, 780.0, 560.0),
            &[],
            vec![],
        )],
    );
    let (eng, _) = engine_from_roots(vec![window], "iTerm2", "shell");

    let hits = eng.ax_terminal_text_hits(None);
    assert_eq!(
        hits.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(),
        vec!["cargo test", "finished ok"]
    );
}

#[test]
fn refresh_if_stale_reuses_recent_graph() {
    let (mut eng, _) = engine_with_counter();
    let refreshed = eng.refresh_if_stale().unwrap();
    assert!(
        !refreshed,
        "newly-created engine should still be inside read TTL"
    );
}

#[test]
fn visual_signature_comparison_counts_threshold_crossings() {
    let previous = [10, 20, 30, 40];
    let current = [10, 25, 60, 39];
    let (changed, max_delta, mean_delta) = compare_signatures(&previous, &current, 4);
    assert_eq!(changed, 2);
    assert_eq!(max_delta, 30);
    assert!((mean_delta - 9.0).abs() < f64::EPSILON);
}

#[test]
fn window_view_adds_window_geometry_without_full_graph() {
    let (eng, _) = engine_with_counter();
    let view = eng.window_view(4);
    assert_eq!(view.target.pid, 1363);
    assert_eq!(view.title, "Notes – Aucune note");
    assert!(view.window.w > 0.0);
    assert!(view.window.h > 0.0);
    assert!(view.key_elements.len() <= 4);
    assert!(view.visible_text.len() <= 4);
}

#[test]
fn desktop_view_marks_missing_display_topology_as_degraded() {
    let view = desktop_view_from_windows(
        Vec::new(),
        Vec::new(),
        Some("no valid display topology".into()),
    );
    assert!(view.degraded);
    assert_eq!(view.reason.as_deref(), Some("no valid display topology"));
    assert!(view.displays.is_empty());
    assert!(view.windows.is_empty());
}

#[test]
fn desktop_view_renumbers_z_order_after_filtering() {
    let front = DesktopWindow {
        window_id: 1,
        pid: 10,
        app: "Finder".into(),
        title: "front".into(),
        bounds: Bbox {
            x: 0.0,
            y: 0.0,
            w: 500.0,
            h: 500.0,
        },
        on_screen: true,
        z_order: 7,
        is_frontmost: false,
        display: None,
        covered_by: Vec::new(),
        covers: Vec::new(),
    };
    let back = DesktopWindow {
        window_id: 2,
        pid: 20,
        app: "Obsidian".into(),
        title: "back".into(),
        bounds: Bbox {
            x: 50.0,
            y: 50.0,
            w: 500.0,
            h: 500.0,
        },
        on_screen: true,
        z_order: 9,
        is_frontmost: false,
        display: None,
        covered_by: Vec::new(),
        covers: Vec::new(),
    };

    let view = desktop_view_from_windows(Vec::new(), vec![back, front], None);
    assert_eq!(view.frontmost.as_ref().unwrap().window_id, 1);
    assert_eq!(view.frontmost.as_ref().unwrap().z_order, 0);
    assert!(view.frontmost.as_ref().unwrap().is_frontmost);
    assert_eq!(view.windows[1].z_order, 1);
    assert_eq!(view.windows[0].covers, vec![2]);
    assert_eq!(view.windows[1].covered_by, vec![1]);
}

#[test]
fn raw_point_risk_flags_possible_backdrop_clicks() {
    let (eng, _) = engine_with_counter();
    let risk = eng.raw_point_risk(10_000.0, 10_000.0);
    assert_eq!(risk.level, RiskLevel::High);
    assert!(
        risk.reasons
            .iter()
            .any(|r| r.contains("outside the target window")),
        "risk reasons should flag off-window raw points: {:?}",
        risk.reasons
    );
}

#[test]
fn raw_point_guard_rejects_off_target_points() {
    let old = std::env::var("DUNST_MCP_ALLOW_OFF_TARGET_RAW").ok();
    std::env::remove_var("DUNST_MCP_ALLOW_OFF_TARGET_RAW");
    let (eng, _) = engine_with_counter();
    let err = eng
        .ensure_point_in_target_window(10_000.0, 10_000.0, "click")
        .unwrap_err()
        .to_string();
    if let Some(value) = old {
        std::env::set_var("DUNST_MCP_ALLOW_OFF_TARGET_RAW", value);
    }
    assert!(
        err.contains("outside the target window"),
        "off-target raw coordinates should fail clearly: {err}"
    );
}

#[test]
fn raw_key_approval_allows_short_repeated_same_key_burst() {
    let (mut eng, _) = engine_with_counter();
    let target_id = "keyboard@press:Backspace";
    let risk = Engine::raw_input_risk(Vec::new());

    let first = eng
        .gate_raw_input(
            target_id,
            SemanticAction::KeyPress,
            Some("Backspace".into()),
            Some("raw key press"),
            risk.clone(),
        )
        .expect("first keypress should gate");
    assert_eq!(first.result, ActionResult::PendingApproval);

    eng.approve(target_id).unwrap();
    assert!(
        eng.gate_raw_input(
            target_id,
            SemanticAction::KeyPress,
            Some("Backspace".into()),
            Some("raw key press"),
            risk.clone(),
        )
        .is_none(),
        "approved key should pass"
    );
    assert!(
        eng.gate_raw_input(
            target_id,
            SemanticAction::KeyPress,
            Some("Backspace".into()),
            Some("raw key press"),
            risk,
        )
        .is_none(),
        "same key should remain approved for a short burst"
    );
}

#[test]
fn raw_scroll_approval_covers_same_direction_count_change() {
    let (mut eng, _) = engine_with_counter();
    let risk = Engine::raw_input_risk(Vec::new());

    let gated = eng
        .gate_raw_input(
            "keyboard@scroll:up:1",
            SemanticAction::Scroll,
            Some("scroll up x1".into()),
            Some("background web scroll"),
            risk.clone(),
        )
        .expect("first scroll should gate");
    assert_eq!(gated.result, ActionResult::PendingApproval);

    eng.approve("keyboard@scroll:up:1").unwrap();
    assert!(
        eng.gate_raw_input(
            "keyboard@scroll:up:2",
            SemanticAction::Scroll,
            Some("scroll up x2".into()),
            Some("background web scroll"),
            risk,
        )
        .is_none(),
        "same-direction scroll should not ask for another approval solely because pages changed"
    );
}

#[test]
fn attach_clears_raw_approval_grants() {
    let (mut eng, _) = engine_with_counter();
    let target_id = "keyboard@press:Backspace";

    eng.pending_gate_ids.insert(target_id.to_string());
    eng.approve(target_id).unwrap();
    assert!(eng.raw_approval_available_for_test(target_id));

    eng.attach(99, 199).unwrap();
    assert!(
        !eng.raw_approval_available_for_test(target_id),
        "raw grants are scoped to the attached window"
    );
}

#[test]
fn raw_region_guard_rejects_off_target_regions() {
    let old = std::env::var("DUNST_MCP_ALLOW_OFF_TARGET_RAW").ok();
    std::env::remove_var("DUNST_MCP_ALLOW_OFF_TARGET_RAW");
    let (eng, _) = engine_with_counter();
    let err = eng
        .ensure_region_in_target_window(
            Bbox {
                x: 10_000.0,
                y: 10_000.0,
                w: 100.0,
                h: 100.0,
            },
            "read_text",
        )
        .unwrap_err()
        .to_string();
    if let Some(value) = old {
        std::env::set_var("DUNST_MCP_ALLOW_OFF_TARGET_RAW", value);
    }
    assert!(
        err.contains("outside the target window"),
        "off-target regions should fail clearly: {err}"
    );
}

#[test]
fn top_level_menu_opener_listed_but_deep_submenu_item_filtered() {
    let (mut eng, calls) = engine_with_counter();
    // "Édition" is a top-level menu opener: direct child of the menubar root,
    // bbox null. "Supprimer" is a deep item under a closed Menu, bbox null.
    let edition = id_for(&eng, "Édition");
    let supprimer = id_for(&eng, "Supprimer");

    // Both are geometrically latent (no bbox) — only structure differs.
    assert!(eng.scene_graph().get(&edition).unwrap().bbox.is_none());
    assert!(eng.scene_graph().get(&supprimer).unwrap().bbox.is_none());

    // The exemption is STRUCTURAL, not role-based: Édition's parent IS the
    // menubar root; Supprimer's parent is a closed Menu, not the root.
    let menubar_root = eng
        .scene_graph()
        .roots
        .iter()
        .find(|id| {
            eng.scene_graph()
                .get(id)
                .map(|n| n.role == Role::MenuBar)
                .unwrap_or(false)
        })
        .cloned()
        .expect("menubar root in roots");
    assert_eq!(
        eng.scene_graph().get(&edition).unwrap().parent.as_deref(),
        Some(menubar_root.as_str()),
        "Édition sits directly under the menubar root"
    );
    assert_ne!(
        eng.scene_graph().get(&supprimer).unwrap().parent.as_deref(),
        Some(menubar_root.as_str()),
        "Supprimer sits under a closed Menu, not the menubar root"
    );

    // query_affordances("click"): the opener is listed, the deep item is not.
    let click = eng.query_affordances(SemanticAction::Click);
    assert!(
        click.contains(&edition),
        "top-level menu opener listed despite null bbox"
    );
    assert!(
        !click.contains(&supprimer),
        "deep submenu item stays filtered (phantom)"
    );

    // include_latent brings back the deep phantom too (superset).
    let all = eng.query_affordances_filtered(SemanticAction::Click, true);
    assert!(all.contains(&edition));
    assert!(all.contains(&supprimer));

    // get_affordances mirrors the same exemption.
    let aff = eng.affordances_view(false);
    assert!(
        aff["affordances"].get(edition.as_str()).is_some(),
        "opener kept in get_affordances"
    );
    assert!(
        aff["affordances"].get(supprimer.as_str()).is_none(),
        "deep item omitted in get_affordances"
    );

    // find_element still locates both; the gate still acts on the deep item by id.
    assert!(!eng.find_element("Édition").is_empty());
    assert!(!eng.find_element("Supprimer").is_empty());
    let gated = eng.click_element(&supprimer, Some("delete")).unwrap();
    assert_eq!(gated.result, ActionResult::PendingApproval);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "exemption never opens the gate"
    );
}
