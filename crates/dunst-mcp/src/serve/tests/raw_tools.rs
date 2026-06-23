use super::*;

#[test]
fn version_tool_reports_build_identity() {
    let mut e = engine();
    let resp = call(&mut e, "version", json!({}));
    assert!(!is_error(&resp), "version succeeds: {resp}");
    let body = text_json(&resp);
    assert_eq!(body["version"], SERVER_VERSION);
    assert_eq!(body["protocol_version"], PROTOCOL_VERSION);
    assert!(body["version_label"]
        .as_str()
        .unwrap()
        .contains(SERVER_VERSION));
    assert!(body["git_sha"].is_string());
    assert!(body["build_time_unix"].is_string());
}

#[test]
fn approve_tool_is_disabled_by_default() {
    std::env::remove_var("DUNST_MCP_ENABLE_APPROVE_TOOL");
    let mut e = engine();
    let resp = call(&mut e, "approve", json!({ "id": "anything" }));
    assert!(
        is_error(&resp),
        "approve must be disabled by default: {resp}"
    );
    assert!(text(&resp).contains("disabled"));
}

#[test]
fn raw_pending_approval_includes_ui_mapping_fallback() {
    let entry = AuditEntry {
        ts_ms: 1,
        target_id: "keyboard@hotkey:cmd+l".into(),
        action: SemanticAction::Type,
        argument: Some("cmd+l".into()),
        risk: dunst_core::RiskAssessment {
            level: dunst_core::RiskLevel::High,
            requires_approval: true,
            reasons: vec!["raw input is not bound to a scene element".into()],
        },
        reasoning: Some("background hotkey".into()),
        result: ActionResult::PendingApproval,
        graph_diff: GraphDiff::default(),
        caller: None,
    };

    let body = audit_entry_value(entry, false);
    assert_eq!(body["approval_hint"]["approve_tool"], "approve");
    assert_eq!(body["ui_fallback_hint"]["mode"], "ui_mapping");
    assert!(
        body["ui_fallback_hint"]["recommended_sequence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["tool"] == "get_affordances"),
        "fallback should direct agents back to the affordance graph: {body}"
    );
    assert!(
        body["ui_fallback_hint"]["avoid"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step.as_str().unwrap().contains("javascript: injection")),
        "fallback should rule out address-bar DOM injection: {body}"
    );
}

#[test]
fn raw_input_tools_dispatch_and_error_cleanly() {
    // A non-existent pid: on macOS the raw CGEvent posts to nothing (no test
    // side effect); the dispatch wiring is what we assert here.
    let mut e = engine_with_pid(i32::MAX);

    // Missing required args → isError, before reaching the engine.
    assert!(
        is_error(&call(&mut e, "press_key", json!({}))),
        "press_key needs 'key'"
    );
    assert!(
        is_error(&call(&mut e, "click_at", json!({ "x": 10.0 }))),
        "click_at needs both 'x' and 'y'"
    );

    // press_key with an unknown key → a clean isError on both platforms (macOS:
    // the backend rejects the key name; non-macOS: the stub is unsupported).
    let bad = call(
        &mut e,
        "press_key",
        json!({ "key": "definitely-not-a-real-key-xyz" }),
    );
    assert!(is_error(&bad), "unknown key must be a clean isError: {bad}");
    assert_eq!(bad["jsonrpc"], "2.0");

    // click_at with in-target coords reaches the engine and returns a
    // well-formed JSON-RPC response (pending approval on macOS, isError on
    // non-macOS stub) — never a panic.
    let window = e.window_view(1).window;
    let resp = call(
        &mut e,
        "click_at",
        json!({ "x": window.x + 10.0, "y": window.y + 10.0 }),
    );
    assert_eq!(resp["jsonrpc"], "2.0");
    assert!(resp.get("result").is_some(), "well-formed response: {resp}");
    let body = text_json(&resp);
    if body["result"] == "pending_approval" {
        assert_eq!(
            body["approval_hint"]["approve_tool"], "approve",
            "pending raw input should tell agents how to approve: {body}"
        );
        assert_eq!(
            body["approval_hint"]["approve_arguments"]["id"],
            body["target_id"]
        );
        assert_eq!(body["ui_fallback_hint"]["mode"], "ui_mapping");
    }

    let missing_file = call(
        &mut e,
        "select_file",
        json!({ "path": "/definitely/not/a/real/file.pdf" }),
    );
    assert!(
        is_error(&missing_file),
        "select_file rejects missing paths before touching the OS: {missing_file}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn select_file_gates_existing_file_before_os_interaction() {
    let mut e = engine();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let resp = call(&mut e, "select_file", json!({ "path": path }));
    assert!(!is_error(&resp), "first select_file call gates: {resp}");
    let body = text_json(&resp);
    assert_eq!(body["result"], "pending_approval");
    assert_eq!(body["approval_hint"]["approve_tool"], "approve");
    assert!(
        body["risk"]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason
                .as_str()
                .unwrap()
                .contains("selects a local file for upload")),
        "risk explains file selection: {body}"
    );
}

#[test]
fn read_series_rejects_malformed_points() {
    let mut e = engine();
    let cases = [
        json!({ "points": [ [1.0, 2.0], [3.0] ] }),
        json!({ "points": [ [1.0, 2.0], ["x", 3.0] ] }),
        json!({ "points": [ [1.0, 2.0], { "x": 3.0, "y": 4.0 } ] }),
    ];
    for args in cases {
        let resp = call(&mut e, "read_series", args);
        assert!(is_error(&resp), "malformed points must fail: {resp}");
        assert!(text(&resp).contains("point"), "got: {}", text(&resp));
    }
}
