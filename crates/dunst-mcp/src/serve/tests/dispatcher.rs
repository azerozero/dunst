use super::*;

#[test]
fn dispatcher_rejects_malformed_calls() {
    let mut e = engine();
    // (tool, arguments, substring expected in the error text)
    let cases: &[(&str, Value, &str)] = &[
        ("no_such_tool", json!({}), "unknown tool"),
        ("find_element", json!({}), "query"), // required arg missing
        ("click_element", json!({}), "id"),   // required arg missing
        ("type_into", json!({ "id": "x" }), "text"), // partial args
        ("get_scene_graph", json!({ "view": "banana" }), "view"), // invalid enum
        ("query_affordances", json!({ "action": "nope" }), "action"), // invalid enum
    ];
    for (tool, args, needle) in cases {
        let resp = call(&mut e, tool, args.clone());
        assert!(is_error(&resp), "{tool} with {args} must be isError");
        let t = text(&resp);
        assert!(
            t.contains(needle),
            "{tool}: error {t:?} should mention {needle:?}"
        );
        // Even on error the envelope stays well-formed JSON-RPC.
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], json!(1));
    }
}

#[test]
fn tool_panic_becomes_mcp_error_response() {
    let resp = panic_tool_response(
        json!(7),
        "click_element",
        Instant::now(),
        Box::new("simulated panic"),
    );

    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], json!(7));
    assert!(is_error(&resp), "panic response must be isError: {resp}");
    assert!(text(&resp).contains("tool call panicked in click_element"));
    assert_eq!(resp["result"]["_meta"]["dunst"]["tool"], "click_element");
}

#[test]
fn dispatcher_accepts_a_well_formed_call() {
    // Anchor: a valid call returns content and is NOT flagged as an error.
    let mut e = engine();
    let resp = call(&mut e, "get_scene_graph", json!({ "view": "summary" }));
    assert!(!is_error(&resp), "valid call must not be isError: {resp}");
    assert!(resp["result"]["content"][0]["text"].is_string());
}

#[test]
fn tool_call_results_include_timing_meta() {
    let mut e = engine();
    let resp = call(&mut e, "get_scene_graph", json!({ "view": "summary" }));
    assert_eq!(resp["result"]["_meta"]["dunst"]["tool"], "get_scene_graph");
    assert!(
        resp["result"]["_meta"]["dunst"]["timing_ms"]
            .as_f64()
            .is_some_and(|ms| ms >= 0.0),
        "timing_ms should be numeric: {resp}"
    );
}

#[test]
fn page_state_returns_compact_orientation_payload() {
    let mut e = engine();
    let resp = call(&mut e, "page_state", json!({ "fresh": false, "limit": 4 }));
    assert!(!is_error(&resp), "page_state succeeds: {resp}");
    let state = text_json(&resp);
    assert_eq!(state["title"], "Notes – Aucune note");
    assert!(state["key_elements"].as_array().unwrap().len() <= 4);
    assert!(state["visible_text"].as_array().unwrap().len() <= 4);
}

#[test]
fn text_snapshot_returns_ax_text_payload() {
    let mut e = engine();
    let resp = call(
        &mut e,
        "text_snapshot",
        json!({ "query": "Corps de la note", "fresh": false, "limit": 4 }),
    );
    assert!(!is_error(&resp), "text_snapshot succeeds: {resp}");
    let snippets = text_json(&resp);
    let snippets = snippets.as_array().expect("text_snapshot returns array");
    assert_eq!(snippets.len(), 1);
    assert_eq!(snippets[0]["role"], "text_area");
}

#[test]
fn window_view_returns_scoped_window_payload() {
    let mut e = engine();
    let resp = call(&mut e, "window_view", json!({ "fresh": false, "limit": 4 }));
    assert!(!is_error(&resp), "window_view succeeds: {resp}");
    let state = text_json(&resp);
    assert_eq!(state["title"], "Notes – Aucune note");
    assert!(state["window"]["w"].as_f64().unwrap() > 0.0);
    assert!(state["window"]["h"].as_f64().unwrap() > 0.0);
    assert!(state["key_elements"].as_array().unwrap().len() <= 4);
}

#[test]
fn find_element_refreshes_and_can_filter_latent_matches() {
    let mut e = engine();
    let default = call(
        &mut e,
        "find_element",
        json!({ "query": "Supprimer", "fresh": false }),
    );
    assert!(!is_error(&default), "default find succeeds: {default}");
    assert!(
        !text_json(&default).as_array().unwrap().is_empty(),
        "default find keeps latent matches"
    );

    let visible_only = call(
        &mut e,
        "find_element",
        json!({ "query": "Supprimer", "visible_only": true, "fresh": false }),
    );
    assert!(
        !is_error(&visible_only),
        "visible find succeeds: {visible_only}"
    );
    assert_eq!(
        text_json(&visible_only).as_array().unwrap().len(),
        0,
        "visible_only drops collapsed/off-window matches"
    );
}

#[test]
fn find_element_force_refresh_uses_recent_visible_cached_match() {
    let (mut e, captures) = engine_with_capture_counter();
    assert_eq!(
        captures.load(Ordering::SeqCst),
        1,
        "Engine::new performs the initial capture"
    );

    let resp = call(
        &mut e,
        "find_element",
        json!({
            "query": "Nouvelle note",
            "visible_only": true,
            "force_refresh": true
        }),
    );

    assert!(!is_error(&resp), "find_element succeeds: {resp}");
    assert_eq!(
        captures.load(Ordering::SeqCst),
        1,
        "a visible match in a very recent graph should not force a second AX capture"
    );
    assert!(
        !text_json(&resp).as_array().unwrap().is_empty(),
        "cached visible match returned"
    );
}

#[test]
fn wait_for_element_timeout_has_single_clear_status() {
    let mut e = engine();
    let resp = call(
        &mut e,
        "wait_for_element",
        json!({
            "query": "definitely-not-present",
            "timeout_ms": 100,
            "interval_ms": 50
        }),
    );
    assert!(!is_error(&resp), "wait_for_element succeeds: {resp}");
    let body = text_json(&resp);
    assert_eq!(body["status"], "timeout");
    assert_eq!(body["condition_met"], false);
    assert_eq!(body["timed_out"], true);
    assert_eq!(body["found"], false);
    assert_eq!(body["matches"].as_array().unwrap().len(), 0);
    assert!(
        body.get("matched").is_none(),
        "legacy ambiguous field should not be returned: {body}"
    );
}
