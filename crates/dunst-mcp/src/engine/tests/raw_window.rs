use super::*;

#[test]
fn user_active_guard_retry_runs_once_before_returning() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_in_closure = attempts.clone();
    let result = retry_user_active_guard_after(Duration::from_millis(0), || {
            if attempts_in_closure.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(DunstError::Execution(
                    "user-active guard blocked hover_at: last keyboard/mouse input was 1 ms ago (< 150 ms)".into(),
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
fn target_visibility_reports_covered_window_and_hint() {
    let front = DesktopWindow {
        window_id: 1,
        pid: 10,
        app: "Firefox".into(),
        title: "Google".into(),
        bounds: Bbox {
            x: 0.0,
            y: 0.0,
            w: 500.0,
            h: 500.0,
        },
        on_screen: true,
        z_order: 0,
        is_frontmost: false,
        display: None,
        covered_by: Vec::new(),
        covers: Vec::new(),
    };
    let target = DesktopWindow {
        window_id: 2,
        pid: 20,
        app: "Firefox".into(),
        title: "Uber Eats".into(),
        bounds: Bbox {
            x: 50.0,
            y: 50.0,
            w: 500.0,
            h: 500.0,
        },
        on_screen: true,
        z_order: 1,
        is_frontmost: false,
        display: None,
        covered_by: Vec::new(),
        covers: Vec::new(),
    };
    let view = desktop_view_from_windows(Vec::new(), vec![target, front], None);
    let visibility = target_visibility_from_desktop(
        2,
        "Uber Eats".into(),
        Bbox {
            x: 50.0,
            y: 50.0,
            w: 500.0,
            h: 500.0,
        },
        &view,
    );

    assert_eq!(visibility.status, "covered");
    assert_eq!(visibility.covered_by[0].window_id, 1);
    assert!(visibility.visible_fraction < 1.0);
    assert!(visibility.fallback_hint.is_some());
}

#[test]
fn hit_targets_return_safe_click_zones_and_action_modes() {
    let (eng, _) = engine_with_counter();
    let new_note_id = id_for(&eng, "Nouvelle note");

    let result = eng.hit_targets(false, "all", 80, None);
    assert!(!result.ui_epoch.fingerprint.is_empty());
    assert!(!result.state_changed);

    let target = result
        .targets
        .iter()
        .find(|target| target.id == new_note_id)
        .expect("Nouvelle note should be returned as a semantic target");
    assert_eq!(target.role, "button");
    assert_eq!(target.source, "accessibility");
    assert_eq!(target.risk.level, RiskLevel::Low);
    assert!(target.action_modes.iter().any(|mode| {
        mode.action == SemanticAction::Click
            && mode.tool_hint == "click_element"
            && mode.target_id.as_deref() == Some(new_note_id.as_str())
    }));

    let bbox = target.bbox.expect("button bbox");
    let safe = target.safe_click.as_ref().expect("safe click zone");
    assert!(safe.bbox.x >= bbox.x);
    assert!(safe.bbox.y >= bbox.y);
    assert!(safe.bbox.x + safe.bbox.w <= bbox.x + bbox.w);
    assert!(safe.bbox.y + safe.bbox.h <= bbox.y + bbox.h);
    assert!(point_in_bbox(safe.center, bbox));

    for direction in ["down", "up", "top", "bottom"] {
        let target_id = format!("page@scroll:{direction}");
        let page_scroll = result
            .targets
            .iter()
            .find(|target| target.id == target_id)
            .unwrap_or_else(|| panic!("{target_id} pseudo-target should be returned"));
        assert_eq!(page_scroll.source, "page");
        assert_eq!(page_scroll.risk.level, RiskLevel::High);
        assert!(page_scroll.risk.requires_approval);
        assert!(page_scroll.action_modes.iter().any(|mode| {
            mode.action == SemanticAction::Scroll
                && mode.tool_hint == "scroll"
                && mode.target_id.as_deref() == Some(target_id.as_str())
                && mode.risk.level == RiskLevel::High
                && mode.risk.requires_approval
                && mode
                    .arguments
                    .as_ref()
                    .and_then(|args| args.get("direction"))
                    .and_then(serde_json::Value::as_str)
                    == Some(direction)
        }));
    }
}

#[test]
fn query_affordances_lists_page_scroll_pseudo_targets() {
    let (eng, _) = engine_with_counter();
    let ids = eng.query_affordances_scoped(SemanticAction::Scroll, false, "page");

    for direction in ["down", "up", "top", "bottom"] {
        let target_id = format!("page@scroll:{direction}");
        assert!(
            ids.iter().any(|id| id == &target_id),
            "missing {target_id} from scroll affordances: {ids:?}"
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn page_scroll_pseudo_target_uses_keyboard_scroll_gate() {
    let (mut eng, _) = engine_with_counter();

    let entry = eng
        .scroll("down", 2, Some("page@scroll:down"))
        .expect("pseudo page scroll should gate before platform input");

    assert_eq!(entry.result, ActionResult::PendingApproval);
    assert_eq!(entry.target_id, "keyboard@scroll:down:2");
    assert!(
        entry
            .risk
            .reasons
            .iter()
            .any(|reason| reason.contains("raw input is not bound")),
        "keyboard scroll stays explicit raw input: {:?}",
        entry.risk.reasons
    );
}

#[test]
fn hit_targets_report_stale_previous_epoch() {
    let (eng, _) = engine_with_counter();

    let current = eng.hit_targets(false, "all", 80, None);
    let unchanged = eng.hit_targets(false, "all", 80, Some(&current.ui_epoch.fingerprint));
    assert!(!unchanged.state_changed);
    assert!(unchanged.stale_reason.is_none());

    let stale = eng.hit_targets(false, "all", 80, Some("old-window-state"));
    assert!(stale.state_changed);
    assert!(stale
        .stale_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("ui_epoch changed")));
    assert!(stale
        .resume_hint
        .as_deref()
        .is_some_and(|hint| hint.contains("Discard cached coordinates")));
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
    let target_id = raw_press_key_target_id("Backspace", 1);
    let risk = Engine::raw_input_risk(Vec::new());

    let first = eng
        .gate_raw_input(
            &target_id,
            SemanticAction::KeyPress,
            Some("Backspace".into()),
            Some("raw key press"),
            risk.clone(),
        )
        .expect("first keypress should gate");
    assert_eq!(first.result, ActionResult::PendingApproval);

    eng.approve(&target_id).unwrap();
    assert!(
        eng.gate_raw_input(
            &target_id,
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
            &raw_press_key_target_id("Backspace", 2),
            SemanticAction::KeyPress,
            Some("Backspace x2".into()),
            Some("raw key press"),
            risk,
        )
        .is_none(),
        "same key should remain approved for a short event budget"
    );
}

#[test]
fn raw_key_approval_does_not_cover_other_keys() {
    let (mut eng, _) = engine_with_counter();
    let risk = Engine::raw_input_risk(Vec::new());
    let backspace = raw_press_key_target_id("Backspace", 1);
    let return_key = raw_press_key_target_id("Return", 1);

    eng.pending_gate_ids.insert(backspace.clone());
    eng.approve(&backspace).unwrap();

    assert!(
        eng.gate_raw_input(
            &backspace,
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
            &return_key,
            SemanticAction::KeyPress,
            Some("Return".into()),
            Some("raw key press"),
            risk,
        )
        .is_some(),
        "a key-specific raw grant must not cover another key"
    );
}

#[test]
fn raw_type_keys_approval_is_one_shot() {
    let (mut eng, _) = engine_with_counter();
    let first_text = "Wok THAi Brest avis Google";
    let second_text = "Osaka Brest avis Google";
    let target_id = raw_type_keys_target_id(first_text);
    let second_target_id = raw_type_keys_target_id(second_text);
    let risk = Engine::raw_input_risk(Vec::new());

    let first = eng
        .gate_raw_input(
            &target_id,
            SemanticAction::Type,
            Some(first_text.into()),
            Some("raw keyboard text into focused element"),
            risk.clone(),
        )
        .expect("first raw type should gate");
    assert_eq!(first.result, ActionResult::PendingApproval);

    eng.approve(&target_id).unwrap();
    assert!(
        eng.gate_raw_input(
            &second_target_id,
            SemanticAction::Type,
            Some(second_text.into()),
            Some("raw keyboard text into focused element"),
            risk.clone(),
        )
        .is_some(),
        "approval for one type_keys payload must not cover changed text"
    );
    assert!(
        eng.gate_raw_input(
            &raw_type_keys_target_id(first_text),
            SemanticAction::Type,
            Some(first_text.into()),
            Some("raw keyboard text into focused element"),
            risk.clone(),
        )
        .is_none(),
        "approved type_keys should pass once"
    );
    assert!(
        eng.gate_raw_input(
            &raw_type_keys_target_id(first_text),
            SemanticAction::Type,
            Some(first_text.into()),
            Some("raw keyboard text into focused element"),
            risk,
        )
        .is_some(),
        "type_keys approval should be consumed after one matching payload"
    );
}

#[test]
fn raw_paste_text_approval_is_one_shot() {
    let (mut eng, _) = engine_with_counter();
    let first_text = "grob + st4ck - IA souveraine";
    let second_text = "RFC-HIT";
    let target_id = raw_paste_text_target_id(first_text);
    let second_target_id = raw_paste_text_target_id(second_text);
    let risk = Engine::raw_input_risk(vec![
        "temporarily writes system clipboard before sending Cmd+V to the focused target".into(),
    ]);

    assert!(
        eng.gate_raw_input(
            &target_id,
            SemanticAction::Type,
            Some(first_text.into()),
            Some("raw clipboard paste into focused element"),
            risk.clone(),
        )
        .is_some(),
        "first raw paste should gate"
    );

    eng.approve(&target_id).unwrap();
    assert!(
        eng.gate_raw_input(
            &second_target_id,
            SemanticAction::Type,
            Some(second_text.into()),
            Some("raw clipboard paste into focused element"),
            risk.clone(),
        )
        .is_some(),
        "approval for one paste_text payload must not cover changed text"
    );
    assert!(
        eng.gate_raw_input(
            &raw_paste_text_target_id(first_text),
            SemanticAction::Type,
            Some(first_text.into()),
            Some("raw clipboard paste into focused element"),
            risk.clone(),
        )
        .is_none(),
        "approved paste_text should pass once"
    );
    assert!(
        eng.gate_raw_input(
            &raw_paste_text_target_id(first_text),
            SemanticAction::Type,
            Some(first_text.into()),
            Some("raw clipboard paste into focused element"),
            risk,
        )
        .is_some(),
        "paste_text approval should be consumed after one matching payload"
    );
}

#[test]
fn raw_key_approval_budget_is_event_based() {
    let (mut eng, _) = engine_with_counter();
    let risk = Engine::raw_input_risk(Vec::new());
    let target_id = raw_press_key_target_id("Backspace", 8);

    eng.pending_gate_ids.insert(target_id.clone());
    eng.approve(&target_id).unwrap();

    assert!(
        eng.gate_raw_input(
            &raw_press_key_target_id("Backspace", 8),
            SemanticAction::KeyPress,
            Some("Backspace x8".into()),
            Some("raw key press"),
            risk.clone(),
        )
        .is_none(),
        "approval should cover the approved physical key events"
    );
    assert!(
        eng.gate_raw_input(
            &raw_press_key_target_id("Backspace", 3),
            SemanticAction::KeyPress,
            Some("Backspace x3".into()),
            Some("raw key press"),
            risk,
        )
        .is_some(),
        "remaining two key events should not cover a three-key repeat"
    );
}

#[test]
fn raw_key_approval_covers_the_approved_clamped_batch() {
    let (mut eng, _) = engine_with_counter();
    let risk = Engine::raw_input_risk(Vec::new());
    let target_id = raw_press_key_target_id("Backspace", 20);

    let gated = eng
        .gate_raw_input(
            &target_id,
            SemanticAction::KeyPress,
            Some("Backspace x20".into()),
            Some("raw key press"),
            risk.clone(),
        )
        .expect("first max-size key batch should gate");
    assert_eq!(gated.result, ActionResult::PendingApproval);

    eng.approve(&target_id).unwrap();
    assert!(
        eng.gate_raw_input(
            &target_id,
            SemanticAction::KeyPress,
            Some("Backspace x20".into()),
            Some("raw key press"),
            risk,
        )
        .is_none(),
        "approving Backspace x20 must cover the exact approved batch"
    );
}

#[test]
fn valid_synthetic_raw_target_can_be_preapproved_after_pending_is_lost() {
    let (mut eng, _) = engine_with_counter();
    let risk = Engine::raw_input_risk(Vec::new());
    let target_id = "screen@820,320:click";

    eng.gate_raw_input(
        target_id,
        SemanticAction::Click,
        Some("click 820,320".into()),
        Some("raw screen click"),
        risk.clone(),
    )
    .expect("first raw click should gate");
    eng.refresh().unwrap();
    assert!(
        !eng.pending_gate_ids.contains(target_id),
        "refresh simulates losing the pending approval token"
    );

    eng.approve(target_id).unwrap();
    assert!(
        eng.gate_raw_input(
            target_id,
            SemanticAction::Click,
            Some("click 820,320".into()),
            Some("raw screen click"),
            risk,
        )
        .is_none(),
        "a syntactically valid in-window raw target should be approvable without the stale pending id"
    );
}

#[test]
fn synthetic_raw_preapproval_rejects_off_target_points() {
    let (mut eng, _) = engine_with_counter();
    let err = eng.approve("screen@9999,9999:click").unwrap_err();
    assert!(
        err.to_string().contains("outside the target window"),
        "off-window raw approval should stay rejected: {err}"
    );
}

#[test]
fn synthetic_raw_preapproval_rejects_unsupported_hotkeys() {
    let (mut eng, _) = engine_with_counter();
    let err = eng.approve("keyboard@hotkey:cmd+a").unwrap_err();
    assert!(
        err.to_string().contains("keyboard-layout sensitive"),
        "layout-sensitive hotkeys should not be preapproved: {err}"
    );

    let err = eng.approve("keyboard@hotkey:cmd+unknownkey").unwrap_err();
    assert!(
        err.to_string().contains("unsupported hotkey combo"),
        "unsupported hotkeys should be rejected clearly: {err}"
    );
}

#[test]
fn raw_point_risk_reports_sparse_ax_instead_of_backdrop_for_browser_canvas_pages() {
    let eng = browser_engine("Firefox", "Collective", None);
    let risk = eng.raw_point_risk(320.0, 240.0);

    assert!(
        risk.reasons.iter().any(
            |reason| reason.contains("AX content") && reason.contains("OCR/shape verification")
        ),
        "sparse browser AX pages should get an actionable risk reason: {:?}",
        risk.reasons
    );
    assert!(
        !risk
            .reasons
            .iter()
            .any(|reason| reason.contains("possible backdrop")),
        "sparse AX pages should not be described as a likely backdrop: {:?}",
        risk.reasons
    );
}

#[test]
fn browser_content_raw_point_reports_sparse_ax_even_when_chrome_nodes_exist() {
    let window = raw_node(
        "AXWindow",
        Some("Collective"),
        None,
        test_bbox(0.0, 0.0, 1200.0, 800.0),
        &["raise"],
        vec![
            raw_node(
                "AXButton",
                Some("Back"),
                None,
                test_bbox(12.0, 12.0, 30.0, 24.0),
                &["press"],
                vec![],
            ),
            raw_node(
                "AXButton",
                Some("Reload"),
                None,
                test_bbox(52.0, 12.0, 30.0, 24.0),
                &["press"],
                vec![],
            ),
            raw_node(
                "AXTextField",
                Some("Address"),
                Some("https://www.collective.work/profile/clement-liard"),
                test_bbox(92.0, 12.0, 460.0, 24.0),
                &[],
                vec![],
            ),
        ],
    );
    let eng = engine_from_roots(vec![window], "Firefox", "Collective").0;
    let risk = eng.raw_point_risk(400.0, 300.0);

    assert!(
        risk.reasons
            .iter()
            .any(|reason| reason.contains("AX content") && reason.contains("browser AX tree")),
        "browser content point should report sparse page AX despite chrome nodes: {:?}",
        risk.reasons
    );
    assert!(
        !risk
            .reasons
            .iter()
            .any(|reason| reason.contains("possible backdrop")),
        "browser chrome nodes should not turn sparse page content into a backdrop warning: {:?}",
        risk.reasons
    );
}

#[test]
fn hover_reveal_success_cleanup_clears_raw_grant() {
    let (mut eng, _) = engine_with_counter();
    let target_id = "hover-reveal@120,350:\"Edit\":click";
    let risk = Engine::raw_input_risk(Vec::new());

    eng.pending_gate_ids.insert(target_id.to_string());
    eng.approve(target_id).unwrap();
    assert!(
        eng.gate_raw_input(
            target_id,
            SemanticAction::Click,
            Some("hover 120,350; click visible \"Edit\"".into()),
            Some("reveal hover-only control and click it"),
            risk,
        )
        .is_none(),
        "approved hover-reveal should pass raw gate"
    );
    eng.clear_inflight_raw_approval(target_id);
    assert!(
        !eng.raw_approval_available_for_test(target_id),
        "successful nested hover-reveal click must clear the consumed raw grant"
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
fn wheel_scroll_approval_covers_same_direction_count_change() {
    let (mut eng, _) = engine_with_counter();
    let risk = Engine::raw_input_risk(Vec::new());

    let gated = eng
        .gate_raw_input(
            "wheel@scroll:down:1:820,320",
            SemanticAction::Scroll,
            Some("wheel scroll down x1".into()),
            Some("background wheel scroll"),
            risk.clone(),
        )
        .expect("first wheel scroll should gate");
    assert_eq!(gated.result, ActionResult::PendingApproval);

    eng.approve("wheel@scroll:down:1:820,320").unwrap();
    assert!(
        eng.gate_raw_input(
            "wheel@scroll:down:3:140,260",
            SemanticAction::Scroll,
            Some("wheel scroll down x3".into()),
            Some("background wheel scroll"),
            risk,
        )
        .is_none(),
        "same-direction wheel scroll should not ask again solely because count or point changed"
    );
}

#[test]
fn scroll_strategy_key_scopes_firefox_by_site_title_when_url_is_hidden() {
    let linkedin = browser_engine("Firefox", "Clément LIARD | LinkedIn", None);
    let choualbox = browser_engine("Firefox", "Choualbox", None);

    let linkedin_key = linkedin.scroll_strategy_key();
    let choualbox_key = choualbox.scroll_strategy_key();

    assert_eq!(linkedin_key.app, "firefox");
    assert_eq!(linkedin_key.page, "title:linkedin");
    assert_eq!(choualbox_key.app, "firefox");
    assert_eq!(choualbox_key.page, "title:choualbox");
    assert_ne!(
        linkedin_key, choualbox_key,
        "a learned Firefox+LinkedIn scroll fallback must not apply to another Firefox site"
    );
}

#[test]
fn scroll_strategy_key_prefers_url_host_when_available() {
    let eng = browser_engine(
        "Firefox",
        "Clément LIARD | LinkedIn",
        Some("https://www.linkedin.com/in/clement-liard/"),
    );

    let key = eng.scroll_strategy_key();

    assert_eq!(key.app, "firefox");
    assert_eq!(key.page, "host:linkedin.com");
}

#[test]
fn real_cursor_scroll_strategy_is_learned_only_after_low_signal_background() {
    let mut eng = browser_engine("Firefox", "Clément LIARD | LinkedIn", None);
    let scope = eng.scroll_strategy_key();
    let cursor_success = scroll_success_audit("cursor@scroll:down:2:2941,1224");

    eng.note_real_cursor_scroll_result(scope.clone(), &cursor_success);
    assert!(
        eng.remembered_scroll_strategy().is_none(),
        "an isolated explicit real-cursor scroll should not become the default"
    );

    let background_low_signal = scroll_success_audit("wheel@scroll:down:2:3286,941");
    eng.note_background_scroll_result(scope.clone(), &background_low_signal);
    eng.note_real_cursor_scroll_result(scope, &cursor_success);

    assert_eq!(
        eng.remembered_scroll_strategy(),
        Some(ScrollStrategy::RealCursorWheel)
    );
}

#[test]
fn attach_clears_raw_approval_grants() {
    let (mut eng, _) = engine_with_counter();
    let target_id = raw_press_key_target_id("Backspace", 1);

    eng.pending_gate_ids.insert(target_id.to_string());
    eng.approve(&target_id).unwrap();
    assert!(eng.raw_approval_available_for_test(&target_id));

    eng.attach(99, 199).unwrap();
    assert!(
        !eng.raw_approval_available_for_test(&target_id),
        "raw grants are scoped to the attached window"
    );
}

fn browser_engine(app: &str, title: &str, url: Option<&str>) -> Engine {
    let mut children = Vec::new();
    if let Some(url) = url {
        children.push(raw_node(
            "AXTextField",
            Some(url),
            Some(url),
            test_bbox(20.0, 20.0, 420.0, 24.0),
            &[],
            vec![],
        ));
    }
    let window = raw_node(
        "AXWindow",
        Some(title),
        None,
        test_bbox(0.0, 0.0, 1200.0, 800.0),
        &["raise"],
        children,
    );
    engine_from_roots(vec![window], app, title).0
}

fn scroll_success_audit(target_id: &str) -> AuditEntry {
    AuditEntry {
        ts_ms: 0,
        target_id: target_id.to_string(),
        action: SemanticAction::Scroll,
        argument: None,
        risk: Engine::raw_input_risk(Vec::new()),
        reasoning: None,
        result: ActionResult::Success,
        graph_diff: GraphDiff::default(),
        caller: None,
    }
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
