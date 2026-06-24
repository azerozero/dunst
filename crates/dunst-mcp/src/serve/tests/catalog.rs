use super::*;
use crate::serve::registry::TOOL_REGISTRY;

#[test]
fn tools_list_exposes_read_text_with_object_schema() {
    std::env::remove_var("DUNST_MCP_ENABLE_APPROVE_TOOL");
    let tools = tools_list();
    assert_eq!(tools.len(), 68, "tool count");
    // Every tool must declare a JSON-Schema object input (the type:object fix).
    for t in &tools {
        assert_eq!(
            t["inputSchema"]["type"], "object",
            "tool {} has a non-object inputSchema: {}",
            t["name"], t["inputSchema"]
        );
    }
    let read_text = tools
        .iter()
        .find(|t| t["name"] == "read_text")
        .expect("read_text tool present");
    assert_eq!(read_text["inputSchema"]["type"], "object");
    // `region` is optional → it must not be in `required`.
    assert_eq!(read_text["inputSchema"]["required"], json!([]));
    assert!(
        read_text["inputSchema"]["properties"]
            .get("expected_epoch")
            .is_none(),
        "read-only tools must not advertise mutation preconditions"
    );
    let click_element = tools
        .iter()
        .find(|t| t["name"] == "click_element")
        .expect("click_element tool present");
    assert_eq!(
        click_element["inputSchema"]["properties"]["expected_epoch"]["type"],
        "string"
    );
    assert_eq!(
        click_element["inputSchema"]["properties"]["fencing_token"]["type"],
        "string"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "list_launchable_apps"),
        "installed-app catalogue tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "app_info"),
        "single app info tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "list_displays"),
        "display topology tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "list_browser_tabs"),
        "browser tab listing tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "wait_for_element"),
        "async element wait tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "pick_option"),
        "popover/list/radio option helper present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "raise_element"),
        "raise action helper present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "text_snapshot"),
        "AX text snapshot tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "wait_for_text_stable"),
        "AX text stability wait tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "window_view"),
        "scoped window view tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "desktop_view"),
        "desktop topology tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "target_visibility"),
        "target visibility tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "get_hit_targets"),
        "semantic hit target tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "visual_change_probe"),
        "visual change probe tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "analyze_region_ax"),
        "region AX analysis tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "move_window_to_display"),
        "display move tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "move_app_to_display"),
        "app display move tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "arrange_windows"),
        "window arrangement tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "expose_target_window"),
        "verified target exposure tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "open_url_and_attach_tab"),
        "open URL and attach helper present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "read_text_detailed"),
        "detailed OCR diagnostics tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "find_ocr_text"),
        "OCR text search tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "click_near_text"),
        "OCR-bound click tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "detect_modal"),
        "modal detection tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "dismiss_modal"),
        "safe modal dismiss tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "extract_ocr_cards"),
        "OCR card extraction tool present"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "version"),
        "runtime version tool present"
    );
    let platform_capabilities = tools
        .iter()
        .find(|t| t["name"] == "platform_capabilities")
        .expect("platform capabilities tool present");
    assert_eq!(
        platform_capabilities["inputSchema"]["properties"]
            .as_object()
            .expect("properties object")
            .len(),
        0
    );
    assert!(
        tools.iter().any(|t| t["name"] == "select_file"),
        "native file selection tool present"
    );
}

#[test]
fn read_text_without_live_window_is_a_clean_error() {
    // An invalid window id → no live macOS window → a clean Err, never a panic.
    // (Off macOS, the stub returns the same class of error.)
    let mut e = engine_with_window(u32::MAX);

    // Direct engine call carries the "live macOS window" message.
    let err = e.read_text(None, false).unwrap_err();
    assert!(
        err.to_string().contains("live macOS window"),
        "unexpected error: {err}"
    );

    // Through the dispatcher: a well-formed isError result, not a crash.
    let resp = call(&mut e, "read_text", json!({}));
    assert!(
        is_error(&resp),
        "read_text without a live window must be isError: {resp}"
    );
    assert_eq!(resp["jsonrpc"], "2.0");

    // A malformed region is rejected before any capture is attempted.
    let bad = call(&mut e, "read_text", json!({ "region": { "x": 1.0 } }));
    assert!(is_error(&bad));
    assert!(text(&bad).contains("region"), "got: {}", text(&bad));
}

#[test]
fn tools_list_exposes_click_at_and_press_key() {
    std::env::remove_var("DUNST_MCP_ENABLE_APPROVE_TOOL");
    let tools = tools_list();
    let click = tools
        .iter()
        .find(|t| t["name"] == "click_at")
        .expect("click_at tool present");
    assert_eq!(click["inputSchema"]["type"], "object");
    assert_eq!(click["inputSchema"]["required"], json!(["x", "y"]));

    let press = tools
        .iter()
        .find(|t| t["name"] == "press_key")
        .expect("press_key tool present");
    assert_eq!(press["inputSchema"]["type"], "object");
    assert_eq!(press["inputSchema"]["required"], json!(["key"]));

    let paste = tools
        .iter()
        .find(|t| t["name"] == "paste_text")
        .expect("paste_text tool present");
    assert_eq!(paste["inputSchema"]["type"], "object");
    assert_eq!(paste["inputSchema"]["required"], json!(["text"]));

    let right_click = tools
        .iter()
        .find(|t| t["name"] == "right_click_at")
        .expect("right_click_at tool present");
    let right_click_description = right_click["description"]
        .as_str()
        .expect("right_click_at description");
    assert!(right_click_description.contains("real-cursor warp/restore"));
    assert!(right_click_description.contains("hardware cursor"));
    assert!(!right_click_description.contains("Background web via SkyLight"));

    let select_file = tools
        .iter()
        .find(|t| t["name"] == "select_file")
        .expect("select_file tool present");
    assert_eq!(select_file["inputSchema"]["type"], "object");
    assert_eq!(select_file["inputSchema"]["required"], json!(["path"]));

    let reveal_hover_click = tools
        .iter()
        .find(|t| t["name"] == "reveal_hover_click")
        .expect("reveal_hover_click tool present");
    assert_eq!(reveal_hover_click["inputSchema"]["type"], "object");
    assert_eq!(
        reveal_hover_click["inputSchema"]["required"],
        json!(["x", "y", "query"])
    );
    let reveal_description = reveal_hover_click["description"]
        .as_str()
        .expect("reveal_hover_click description");
    assert!(reveal_description.contains("already-visible target-window point"));
    assert!(reveal_description.contains("refuses covered target pixels"));
    assert!(!reveal_description.contains("raises the target window"));

    let read_at = tools
        .iter()
        .find(|t| t["name"] == "read_at")
        .expect("read_at tool present");
    assert_eq!(read_at["inputSchema"]["required"], json!(["x", "y"]));
    assert_eq!(
        read_at["inputSchema"]["properties"]["borrow_cursor"]["type"],
        "boolean"
    );

    let read_series = tools
        .iter()
        .find(|t| t["name"] == "read_series")
        .expect("read_series tool present");
    assert_eq!(
        read_series["inputSchema"]["properties"]["borrow_cursor"]["type"],
        "boolean"
    );
    assert!(
        tools.iter().all(|t| t["name"] != "approve"),
        "approve is an operator-side escape hatch and is not advertised by default"
    );

    let scroll_at = tools
        .iter()
        .find(|t| t["name"] == "scroll_at")
        .expect("scroll_at tool present");
    assert_eq!(scroll_at["inputSchema"]["required"], json!(["x", "y"]));
    assert_eq!(
        scroll_at["inputSchema"]["properties"]["borrow_cursor"]["type"],
        "boolean"
    );
}

#[test]
fn tool_registry_matches_advertised_catalog() {
    std::env::remove_var("DUNST_MCP_ENABLE_APPROVE_TOOL");
    let mut catalog: Vec<_> = tools_list()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect();
    let mut registry: Vec<_> = TOOL_REGISTRY
        .iter()
        .filter(|tool| tool.name != "approve")
        .map(|tool| tool.name.to_string())
        .collect();
    catalog.sort();
    registry.sort();

    assert_eq!(catalog, registry);
    assert!(
        TOOL_REGISTRY.iter().any(|tool| tool.name == "approve"),
        "operator-side approve tool remains registered even when hidden by default"
    );
}
