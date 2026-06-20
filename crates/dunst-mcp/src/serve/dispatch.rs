use super::*;

/// Dispatch a `tools/call`. Returns a full JSON-RPC response object.
pub(super) fn handle_tool_call(engine: &mut Engine, id: Value, req: &Value) -> Value {
    let started = Instant::now();
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let arg = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_owned);
    let arg_bool = |k: &str| args.get(k).and_then(Value::as_bool);

    // screenshot returns an IMAGE content block, not text — handle it directly.
    if name == "screenshot" {
        return match engine.screenshot() {
            Some(b64) => result_obj(
                id,
                add_timing_meta(
                    json!({ "content": [{ "type": "image", "data": b64, "mimeType": "image/png" }] }),
                    name,
                    started,
                ),
            ),
            None => result_obj(
                id,
                add_timing_meta(
                    json!({ "content": [{ "type": "text", "text": "screenshot failed" }], "isError": true }),
                    name,
                    started,
                ),
            ),
        };
    }

    let outcome: Result<Value, String> = match name {
        "version" => Ok(build_info()),
        "refresh" => engine
            .refresh()
            .map(|_| json!("ok"))
            .map_err(|e| e.to_string()),
        "get_scene_graph" => match arg("view").as_deref().map(SceneView::parse) {
            None => Ok(engine.scene_graph_view(
                SceneView::Compact,
                arg_bool("actionable_only").unwrap_or(false),
            )),
            Some(Some(v)) => {
                Ok(engine.scene_graph_view(v, arg_bool("actionable_only").unwrap_or(false)))
            }
            Some(None) => Err("invalid 'view' (expected compact|full|summary)".into()),
        },
        "page_state" => {
            if let Err(e) = ensure_recent_graph(
                engine,
                arg_bool("fresh").unwrap_or(true),
                arg_bool("force_refresh").unwrap_or(false),
            ) {
                Err(e)
            } else {
                Ok(serde_json::to_value(engine.page_state(
                    args.get("limit").and_then(Value::as_u64).unwrap_or(12) as usize,
                ))
                .unwrap_or(Value::Null))
            }
        }
        "text_snapshot" => {
            if let Err(e) = ensure_recent_graph(
                engine,
                arg_bool("fresh").unwrap_or(true),
                arg_bool("force_refresh").unwrap_or(false),
            ) {
                Err(e)
            } else {
                let query = arg("query");
                Ok(serde_json::to_value(engine.text_snapshot(
                    query.as_deref(),
                    arg_bool("visible_only").unwrap_or(true),
                    args.get("limit").and_then(Value::as_u64).unwrap_or(120) as usize,
                ))
                .unwrap_or(Value::Null))
            }
        }
        "list_displays" => {
            Ok(serde_json::to_value(engine.list_displays()).unwrap_or(Value::Null))
        }
        "list_browser_tabs" => {
            if let Err(e) = ensure_recent_graph(
                engine,
                arg_bool("fresh").unwrap_or(true),
                arg_bool("force_refresh").unwrap_or(false),
            ) {
                Err(e)
            } else {
                Ok(serde_json::to_value(engine.list_browser_tabs(
                    arg("query").as_deref(),
                    arg_bool("visible_only").unwrap_or(true),
                ))
                .unwrap_or(Value::Null))
            }
        }
        "window_view" => {
            if let Err(e) = ensure_recent_graph(
                engine,
                arg_bool("fresh").unwrap_or(true),
                arg_bool("force_refresh").unwrap_or(false),
            ) {
                Err(e)
            } else {
                Ok(serde_json::to_value(engine.window_view(
                    args.get("limit").and_then(Value::as_u64).unwrap_or(12) as usize,
                ))
                .unwrap_or(Value::Null))
            }
        }
        "desktop_view" => {
            Ok(serde_json::to_value(engine.desktop_view(arg_bool("all").unwrap_or(false)))
                .unwrap_or(Value::Null))
        }
        "visual_change_probe" => match parse_region(&args) {
            Ok(region) => engine
                .visual_change_probe(
                    region,
                    args.get("columns").and_then(Value::as_u64).unwrap_or(16) as usize,
                    args.get("rows").and_then(Value::as_u64).unwrap_or(12) as usize,
                    args.get("threshold")
                        .and_then(Value::as_u64)
                        .unwrap_or(12)
                        .min(255) as u8,
                    arg_bool("refresh_on_change").unwrap_or(false),
                )
                .map(|probe| serde_json::to_value(probe).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            Err(e) => Err(e),
        },
        "analyze_region_ax" => match parse_region(&args) {
            Ok(region) => Ok(serde_json::to_value(engine.analyze_region_ax(
                region,
                args.get("columns").and_then(Value::as_u64).unwrap_or(8) as usize,
                args.get("rows").and_then(Value::as_u64).unwrap_or(6) as usize,
            ))
            .unwrap_or(Value::Null)),
            Err(e) => Err(e),
        },
        "get_affordances" => {
            Ok(engine.affordances_view(arg_bool("include_latent").unwrap_or(false)))
        }
        "find_element" => match arg("query") {
            Some(q) => find_element_value(
                engine,
                &q,
                arg_bool("visible_only").unwrap_or(false),
                arg_bool("fresh").unwrap_or(true),
                arg_bool("force_refresh").unwrap_or(false),
            ),
            None => Err("missing 'query'".into()),
        },
        "wait_for_element" => match arg("query") {
            Some(q) => wait_for_element_value(
                engine,
                &q,
                arg_bool("visible_only").unwrap_or(true),
                arg_bool("absent").unwrap_or(false),
                args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(5_000),
                args.get("interval_ms").and_then(Value::as_u64).unwrap_or(250),
            ),
            None => Err("missing query".into()),
        },
        "wait_for_text_stable" => {
            let query = arg("query");
            wait_for_text_stable_value(
                engine,
                query.as_deref(),
                arg_bool("visible_only").unwrap_or(true),
                args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(30_000),
                args.get("stable_ms").and_then(Value::as_u64).unwrap_or(1_200),
                args.get("interval_ms").and_then(Value::as_u64).unwrap_or(500),
                args.get("limit").and_then(Value::as_u64).unwrap_or(120) as usize,
            )
        },
        "read_text" => match parse_region(&args) {
            Ok(region) => engine
                .read_text(region, arg_bool("accurate").unwrap_or(false))
                .map(|hits| serde_json::to_value(hits).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            Err(e) => Err(e),
        },
        "read_shapes" => engine
            .read_shapes()
            .map(|shapes| serde_json::to_value(shapes).unwrap_or(Value::Null))
            .map_err(|e| e.to_string()),
        "query_affordances" => match arg("action").as_deref().and_then(parse_action) {
            Some(a) => Ok(json!(engine.query_affordances_filtered(
                a,
                arg_bool("include_latent").unwrap_or(false)
            ))),
            None => Err("missing/invalid 'action'".into()),
        },
        "click_element" => match arg("id") {
            Some(eid) => engine
                .click_element(&eid, arg("reasoning").as_deref())
                .map(|e| audit_entry_value(e, arg_bool("include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            None => Err("missing 'id'".into()),
        },
        "raise_element" => match arg("id") {
            Some(eid) => engine
                .raise_element(&eid, arg("reasoning").as_deref())
                .map(|e| audit_entry_value(e, arg_bool("include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            None => Err("missing 'id'".into()),
        },
        "pick_option" => match arg("query") {
            Some(query) => engine
                .pick_option(
                    &query,
                    arg_bool("visible_only").unwrap_or(true),
                    arg("reasoning").as_deref(),
                )
                .map(|result| option_pick_value(result, arg_bool("include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            None => Err("missing 'query'".into()),
        },
        "type_into" => match (arg("id"), arg("text")) {
            (Some(eid), Some(text)) => engine
                .type_into(&eid, &text, arg("reasoning").as_deref())
                .map(|e| audit_entry_value(e, arg_bool("include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            _ => Err("missing 'id' or 'text'".into()),
        },
        "hover_probe" => match arg("id") {
            Some(eid) => engine
                .hover_probe(&eid)
                .map(|e| audit_entry_value(e, arg_bool("include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            None => Err("missing 'id'".into()),
        },
        "drag_element" => match (arg("source_id"), arg("target_id")) {
            (Some(source_id), Some(target_id)) => engine
                .drag_element(&source_id, &target_id, arg("reasoning").as_deref())
                .map(|e| audit_entry_value(e, arg_bool("include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            _ => Err("missing 'source_id' or 'target_id'".into()),
        },
        "click_at" => match (
            args.get("x").and_then(Value::as_f64),
            args.get("y").and_then(Value::as_f64),
        ) {
            (Some(x), Some(y)) => engine
                .click_at(x, y)
                .map(|e| audit_entry_value(e, arg_bool("include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            _ => Err("click_at requires numeric 'x' and 'y'".into()),
        },
        "reveal_hover_click" => match (
            args.get("x").and_then(Value::as_f64),
            args.get("y").and_then(Value::as_f64),
            arg("query"),
        ) {
            (Some(x), Some(y), Some(query)) => engine
                .reveal_hover_click(
                    x,
                    y,
                    &query,
                    args.get("settle_ms").and_then(Value::as_u64).unwrap_or(250),
                    arg("reasoning").as_deref(),
                )
                .map(|e| audit_entry_value(e, arg_bool("include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            _ => Err("reveal_hover_click requires numeric 'x', numeric 'y', and string 'query'"
                .into()),
        },
        "select_file" => match arg("path") {
            Some(path) => {
                let click_point = match (
                    arg("trigger_id"),
                    args.get("x").and_then(Value::as_f64),
                    args.get("y").and_then(Value::as_f64),
                ) {
                    (Some(trigger_id), _, _) => Some(crate::engine::FileSelectTrigger::ElementId(
                        trigger_id,
                    )),
                    (None, Some(x), Some(y)) => {
                        Some(crate::engine::FileSelectTrigger::Point { x, y })
                    }
                    (None, None, None) => None,
                    (None, _, _) => return result_obj(
                        id,
                        add_timing_meta(
                            json!({
                                "content": [{ "type": "text", "text": "select_file requires both numeric 'x' and 'y' when using coordinates" }],
                                "isError": true
                            }),
                            name,
                            started,
                        ),
                    ),
                };
                engine
                    .select_file(&path, click_point, arg("reasoning").as_deref())
                    .map(|e| audit_entry_value(e, arg_bool("include_diff").unwrap_or(false)))
                    .map_err(|e| e.to_string())
            }
            None => Err("missing 'path'".into()),
        },
        "hover_at" => match (
            args.get("x").and_then(Value::as_f64),
            args.get("y").and_then(Value::as_f64),
        ) {
            (Some(x), Some(y)) => engine
                .hover_at(x, y)
                .map(|()| json!("ok"))
                .map_err(|e| e.to_string()),
            _ => Err("hover_at requires numeric 'x' and 'y'".into()),
        },
        "read_at" => match (
            args.get("x").and_then(Value::as_f64),
            args.get("y").and_then(Value::as_f64),
        ) {
            (Some(x), Some(y)) => engine
                .read_at(x, y, arg_bool("borrow_cursor").unwrap_or(false))
                .map(|h| serde_json::to_value(h).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            _ => Err("read_at requires numeric 'x' and 'y'".into()),
        },
        "read_series" => match args.get("points").map(parse_points) {
            Some(Ok(pts)) => {
                engine
                    .read_series(&pts, arg_bool("borrow_cursor").unwrap_or(false))
                    .map(|h| serde_json::to_value(h).unwrap_or(Value::Null))
                    .map_err(|e| e.to_string())
            }
            Some(Err(e)) => Err(e),
            None => Err("read_series requires 'points': [[x,y], ...]".into()),
        },
        "scan_chart" => {
            let n = args.get("samples").and_then(Value::as_u64).unwrap_or(5) as usize;
            engine
                .scan_chart(n)
                .map(|r| serde_json::to_value(r).unwrap_or(Value::Null))
                .map_err(|e| e.to_string())
        }
        "focus_window" => Ok(json!({ "focused": engine.focus_window() })),
        "list_windows" => Ok(serde_json::to_value(
            engine.list_windows(arg_bool("all").unwrap_or(false)),
        )
        .unwrap_or(Value::Null)),
        "move_window_to_display" => match args.get("display").and_then(Value::as_u64) {
            Some(display) => engine
                .move_window_to_display(display as usize, arg_bool("preserve_size").unwrap_or(true))
                .map(|view| serde_json::to_value(view).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            None => Err("move_window_to_display requires integer 'display'".into()),
        },
        "move_app_to_display" => match (arg("app"), args.get("display").and_then(Value::as_u64)) {
            (Some(app), Some(display)) => engine
                .move_app_to_display(
                    &app,
                    display as usize,
                    arg_bool("preserve_size").unwrap_or(true),
                )
                .map(|result| serde_json::to_value(result).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            _ => Err("move_app_to_display requires 'app' and integer 'display'".into()),
        },
        "arrange_windows" => match args.get("display").and_then(Value::as_u64) {
            Some(display) => match parse_window_ids(args.get("window_ids")) {
                Ok(window_ids) => engine
                    .arrange_windows(
                        display as usize,
                        arg("mode").as_deref().unwrap_or("grid"),
                        arg("app").as_deref(),
                        &window_ids,
                        arg_bool("all").unwrap_or(false),
                    )
                    .map(|result| serde_json::to_value(result).unwrap_or(Value::Null))
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e),
            },
            None => Err("arrange_windows requires integer 'display'".into()),
        },
        "list_apps" => Ok(
            serde_json::to_value(engine.list_apps(arg("query").as_deref())).unwrap_or(Value::Null),
        ),
        "list_launchable_apps" => Ok(serde_json::to_value(engine.list_launchable_apps(
            arg("query").as_deref(),
            args.get("limit").and_then(Value::as_u64).unwrap_or(80) as usize,
        ))
        .unwrap_or(Value::Null)),
        "app_info" => {
            let info = engine.app_info(
                arg("app").as_deref(),
                arg("bundle_id").as_deref(),
                arg("path").as_deref(),
            );
            match info {
                Some(info) => Ok(serde_json::to_value(info).unwrap_or(Value::Null)),
                None => Err("app_info found no matching .app bundle".into()),
            }
        }
        "attach" => match args.get("window_id").and_then(Value::as_u64) {
            Some(wid) => match engine.attach_window(wid as u32) {
                Ok(()) => {
                    let (tpid, twin) = engine.target();
                    let g = engine.scene_graph();
                    Ok(json!({
                        "attached": { "pid": tpid, "window_id": twin, "title": g.window.title },
                        "n_nodes": g.nodes.len()
                    }))
                }
                Err(e) => Err(e.to_string()),
            },
            None => Err("attach requires integer 'window_id'".into()),
        },
        "launch_app" => match arg("app") {
            Some(app) => {
                let extra: Vec<String> = args
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(json!({ "launched": engine.launch_app(&app, arg("url").as_deref(), &extra) }))
            }
            None => Err("launch_app requires 'app'".into()),
        },
        "close_app" => match arg("app") {
            Some(app) => Ok(json!({ "closed": engine.close_app(&app) })),
            None => Err("close_app requires 'app'".into()),
        },
        "right_click_at" => match (
            args.get("x").and_then(Value::as_f64),
            args.get("y").and_then(Value::as_f64),
        ) {
            (Some(x), Some(y)) => engine
                .right_click_at(x, y)
                .map(|e| audit_entry_value(e, arg_bool("include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            _ => Err("right_click_at requires numeric 'x' and 'y'".into()),
        },
        "double_click_at" => match (
            args.get("x").and_then(Value::as_f64),
            args.get("y").and_then(Value::as_f64),
        ) {
            (Some(x), Some(y)) => engine
                .double_click_at(x, y)
                .map(|e| audit_entry_value(e, arg_bool("include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            _ => Err("double_click_at requires numeric 'x' and 'y'".into()),
        },
        "open_menu" => match arg("name") {
            Some(name) => engine
                .open_menu(&name)
                .map(|e| audit_entry_value(e, arg_bool("include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            None => Err("open_menu requires 'name'".into()),
        },
        "press_key" => match arg("key") {
            Some(key) => engine
                .press_key(&key)
                .map(|e| audit_entry_value(e, arg_bool("include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            None => Err("missing 'key'".into()),
        },
        "type_keys" => match arg("text") {
            Some(text) => engine
                .type_keys(&text)
                .map(|e| audit_entry_value(e, arg_bool("include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            None => Err("missing 'text'".into()),
        },
        "scroll" => engine
            .scroll(
                arg("direction").as_deref().unwrap_or("down"),
                args.get("pages").and_then(Value::as_u64).unwrap_or(3) as usize,
                arg("id").as_deref(),
            )
            .map(|e| audit_entry_value(e, arg_bool("include_diff").unwrap_or(false)))
            .map_err(|e| e.to_string()),
        "zoom" => engine
            .zoom(arg("direction").as_deref().unwrap_or("in"))
            .map(|e| audit_entry_value(e, arg_bool("include_diff").unwrap_or(false)))
            .map_err(|e| e.to_string()),
        "hotkey" => match arg("combo") {
            Some(combo) => engine
                .hotkey(&combo)
                .map(|e| audit_entry_value(e, arg_bool("include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            None => Err("missing 'combo'".into()),
        },
        "approve" => match arg("id") {
            Some(eid) if approval_tool_enabled() => engine
                .approve(&eid)
                .map(|_| json!("approved"))
                .map_err(|e| e.to_string()),
            Some(_) => Err("approve tool is disabled; set DUNST_MCP_ENABLE_APPROVE_TOOL=1 for controlled operator sessions".into()),
            None => Err("missing 'id'".into()),
        },
        "verify_state" => match (arg("id"), arg("field"), arg("expected")) {
            (Some(eid), Some(field), Some(expected)) => engine
                .verify_state(&eid, &field, &expected)
                .map(|ok| json!({ "matches": ok }))
                .map_err(|e| e.to_string()),
            _ => Err("missing 'id', 'field' or 'expected'".into()),
        },
        "diff_since" => {
            let diff = engine.diff_since();
            if arg_bool("summary").unwrap_or(false) {
                Ok(diff_summary_value(
                    &diff,
                    args.get("limit").and_then(Value::as_u64).unwrap_or(12) as usize,
                ))
            } else {
                Ok(serde_json::to_value(diff).unwrap_or(Value::Null))
            }
        }
        "export_trace" => engine
            .export_trace()
            .map(Value::String)
            .map_err(|e| e.to_string()),
        other => Err(format!("unknown tool: {other}")),
    };

    match outcome {
        Ok(v) => {
            let text = if v.is_string() {
                v.as_str().unwrap().to_owned()
            } else {
                serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string())
            };
            result_obj(
                id,
                add_timing_meta(
                    json!({ "content": [{ "type": "text", "text": text }] }),
                    name,
                    started,
                ),
            )
        }
        Err(msg) => result_obj(
            id,
            add_timing_meta(
                json!({ "content": [{ "type": "text", "text": msg }], "isError": true }),
                name,
                started,
            ),
        ),
    }
}

/// Parse the optional `region` argument of `read_text` into a screen-point
/// [`Bbox`]. Absent or `null` → `None` (OCR the whole window); when present, all of
/// `x, y, w, h` are required and must be numbers.
fn parse_region(args: &Value) -> Result<Option<Bbox>, String> {
    match args.get("region") {
        None | Some(Value::Null) => Ok(None),
        Some(r) => {
            let f = |k: &str| r.get(k).and_then(Value::as_f64);
            match (f("x"), f("y"), f("w"), f("h")) {
                (Some(x), Some(y), Some(w), Some(h)) => Ok(Some(Bbox { x, y, w, h })),
                _ => Err("region requires numeric x, y, w, h".into()),
            }
        }
    }
}

fn parse_action(s: &str) -> Option<SemanticAction> {
    Some(match s.to_ascii_lowercase().as_str() {
        "click" => SemanticAction::Click,
        "hover" => SemanticAction::Hover,
        "type" => SemanticAction::Type,
        "open_menu" => SemanticAction::OpenMenu,
        "pick" => SemanticAction::Pick,
        "toggle" => SemanticAction::Toggle,
        "scroll" => SemanticAction::Scroll,
        "drag" => SemanticAction::Drag,
        "raise" => SemanticAction::Raise,
        "focus" => SemanticAction::Focus,
        _ => return None,
    })
}
