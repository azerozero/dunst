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
        None,
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
fn tool_call_results_include_session_identity_meta() {
    let mut e = engine();
    e.set_session_identity(SessionIdentity {
        session_id: "dunst-test-session".into(),
        client_name: Some("codex".into()),
        client_version: Some("5.5".into()),
        agent_id: Some("collective-fixer".into()),
        parent_pid: Some(42),
        parent_process: Some("codex".into()),
    });

    let resp = call(&mut e, "get_scene_graph", json!({ "view": "summary" }));
    let session = &resp["result"]["_meta"]["dunst"]["session"];
    assert_eq!(session["session_id"], "dunst-test-session");
    assert_eq!(session["client_name"], "codex");
    assert_eq!(session["client_version"], "5.5");
    assert_eq!(session["agent_id"], "collective-fixer");
    assert_eq!(session["parent_pid"], 42);
    assert_eq!(session["parent_process"], "codex");
}

#[test]
fn initialize_result_includes_build_and_session_identity() {
    let session = SessionIdentity {
        session_id: "dunst-init-session".into(),
        client_name: Some("codex".into()),
        client_version: Some("5.5".into()),
        agent_id: Some("collective-fixer".into()),
        parent_pid: Some(42),
        parent_process: Some("codex".into()),
    };

    let result = initialize_result(&session);

    assert_eq!(result["_meta"]["dunst"]["version"], SERVER_VERSION);
    assert_eq!(
        result["_meta"]["dunst"]["protocol_version"],
        PROTOCOL_VERSION
    );
    assert_eq!(
        result["_meta"]["dunst"]["session"]["session_id"],
        "dunst-init-session"
    );
    assert_eq!(result["_meta"]["dunst"]["session"]["client_name"], "codex");
}

#[test]
fn initialize_client_info_parser_keeps_client_name_and_version() {
    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "clientInfo": {
                "name": "Codex",
                "version": "5.5"
            }
        }
    });

    let (name, version) = client_info_from_initialize(&req).expect("clientInfo parsed");

    assert_eq!(name.as_deref(), Some("Codex"));
    assert_eq!(version.as_deref(), Some("5.5"));
}

#[test]
fn mutating_tool_rejects_stale_expected_epoch() {
    set_test_coordination_dir();
    let mut e = engine_with_window(unique_window_id());
    e.set_session_identity(test_session("epoch-a"));
    let id = text_json(&call(
        &mut e,
        "find_element",
        json!({ "query": "Nouvelle note", "fresh": false }),
    ))[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = call(
        &mut e,
        "click_element",
        json!({ "id": id, "expected_epoch": "stale-epoch" }),
    );

    assert!(is_error(&resp), "stale epoch must refuse mutation: {resp}");
    assert!(text(&resp).contains("stale UI epoch"));
    assert_eq!(
        resp["result"]["_meta"]["dunst"]["coordination"]["epoch"]["status"],
        "stale"
    );
}

#[test]
fn stale_expected_epoch_refuses_apply_selections() {
    set_test_coordination_dir();
    let mut e = engine_with_window(unique_window_id());
    e.set_session_identity(test_session("batch-epoch-a"));

    let resp = call(
        &mut e,
        "apply_selections",
        json!({
            "expected_epoch": "stale-epoch",
            "plan": {
                "steps": [
                    { "choice_id": "chk_cutlery", "op": "select", "label": "Cutlery" }
                ]
            }
        }),
    );

    assert!(
        is_error(&resp),
        "stale epoch must refuse apply_selections: {resp}"
    );
    assert!(text(&resp).contains("stale UI epoch"));
    assert_eq!(
        resp["result"]["_meta"]["dunst"]["coordination"]["epoch"]["status"],
        "stale"
    );
}

#[test]
fn mutating_tool_adds_window_lease_and_fencing_meta() {
    set_test_coordination_dir();
    let mut e = engine_with_window(unique_window_id());
    e.set_session_identity(test_session("lease-a"));
    let id = text_json(&call(
        &mut e,
        "find_element",
        json!({ "query": "Nouvelle note", "fresh": false }),
    ))[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = call(&mut e, "click_element", json!({ "id": id }));

    assert!(
        !is_error(&resp),
        "first mutation should acquire lease: {resp}"
    );
    let mutation = &resp["result"]["_meta"]["dunst"]["coordination"]["mutation"];
    assert_eq!(mutation["status"], "lease_acquired");
    assert_eq!(mutation["owner"]["session_id"], "lease-a");
    assert!(mutation["fencing_token"]
        .as_str()
        .is_some_and(|token| !token.is_empty()));
}

#[test]
fn active_window_lease_blocks_other_session() {
    set_test_coordination_dir();
    let window_id = unique_window_id();
    let mut first = engine_with_window(window_id);
    first.set_session_identity(test_session("lease-owner"));
    let id = text_json(&call(
        &mut first,
        "find_element",
        json!({ "query": "Nouvelle note", "fresh": false }),
    ))[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let acquired = call(&mut first, "click_element", json!({ "id": id }));
    assert!(!is_error(&acquired), "owner acquires lease: {acquired}");

    let mut second = engine_with_window(window_id);
    second.set_session_identity(test_session("lease-contender"));
    let id = text_json(&call(
        &mut second,
        "find_element",
        json!({ "query": "Nouvelle note", "fresh": false }),
    ))[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let blocked = call(&mut second, "click_element", json!({ "id": id }));

    assert!(
        is_error(&blocked),
        "other session must be blocked: {blocked}"
    );
    let mutation = &blocked["result"]["_meta"]["dunst"]["coordination"]["mutation"];
    assert_eq!(mutation["status"], "window_lease_blocked");
    assert_eq!(mutation["blocked_by"]["session_id"], "lease-owner");
}

#[test]
fn stale_fencing_token_is_rejected_for_same_session() {
    set_test_coordination_dir();
    let mut e = engine_with_window(unique_window_id());
    e.set_session_identity(test_session("fence-a"));
    let id = text_json(&call(
        &mut e,
        "find_element",
        json!({ "query": "Nouvelle note", "fresh": false }),
    ))[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let acquired = call(&mut e, "click_element", json!({ "id": id.clone() }));
    assert!(
        !is_error(&acquired),
        "first call acquires lease: {acquired}"
    );

    let stale = call(
        &mut e,
        "click_element",
        json!({ "id": id, "fencing_token": "old-token" }),
    );

    assert!(is_error(&stale), "stale fencing token must fail: {stale}");
    assert_eq!(
        stale["result"]["_meta"]["dunst"]["coordination"]["mutation"]["status"],
        "fencing_token_mismatch"
    );
}

fn test_session(session_id: &str) -> SessionIdentity {
    SessionIdentity {
        session_id: session_id.into(),
        client_name: Some("codex".into()),
        client_version: Some("5.5".into()),
        agent_id: Some(session_id.into()),
        parent_pid: Some(std::process::id()),
        parent_process: Some("cargo-test".into()),
    }
}

fn set_test_coordination_dir() {
    std::env::set_var(
        "DUNST_MCP_COORDINATION_DIR",
        format!("/tmp/dunst-mcp-tests-{}", std::process::id()),
    );
}

fn unique_window_id() -> u32 {
    static NEXT: AtomicUsize = AtomicUsize::new(700_000);
    NEXT.fetch_add(1, Ordering::SeqCst) as u32
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

#[test]
fn wait_for_text_stable_reports_empty_ax_text_diagnostic() {
    let mut e = engine();
    let resp = call(
        &mut e,
        "wait_for_text_stable",
        json!({
            "query": "definitely-not-present",
            "timeout_ms": 500,
            "stable_ms": 250,
            "interval_ms": 100,
            "limit": 4
        }),
    );

    assert!(!is_error(&resp), "wait_for_text_stable succeeds: {resp}");
    let body = text_json(&resp);
    assert_eq!(body["empty"], true);
    assert_eq!(body["snippets"].as_array().unwrap().len(), 0);
    assert!(
        body["diagnostic"]
            .as_str()
            .unwrap()
            .contains("no visible AX text snippets"),
        "diagnostic should explain empty text: {body}"
    );
    assert!(
        body["fallback_hint"]
            .as_str()
            .unwrap()
            .contains("read_text"),
        "fallback should point to OCR when AX text is empty: {body}"
    );
}
