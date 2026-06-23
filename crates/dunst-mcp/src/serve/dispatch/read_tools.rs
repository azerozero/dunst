use super::*;

pub(super) fn dispatch(
    engine: &mut Engine,
    name: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    dispatch_state_tools(engine, name, args)
        .or_else(|| dispatch_snapshot_tools(engine, name, args))
        .or_else(|| dispatch_wait_tools(engine, name, args))
        .or_else(|| dispatch_vision_tools(engine, name, args))
        .or_else(|| dispatch_probe_tools(engine, name, args))
        .or_else(|| dispatch_trace_tools(engine, name, args))
}

fn dispatch_state_tools(
    engine: &mut Engine,
    name: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    Some(match name {
        "version" => Ok(build_info()),
        "platform_capabilities" => Ok(
            serde_json::to_value(dunst_platform::platform_capabilities()).unwrap_or(Value::Null),
        ),
        "refresh" => engine
            .refresh()
            .map(|_| json!("ok"))
            .map_err(|e| e.to_string()),
        "get_scene_graph" => {
            match arg(args, "view").as_deref().map(SceneView::parse) {
                None => Ok(engine.scene_graph_view(
                    SceneView::Compact,
                    arg_bool(args, "actionable_only").unwrap_or(false),
                )),
                Some(Some(view)) => Ok(engine
                    .scene_graph_view(view, arg_bool(args, "actionable_only").unwrap_or(false))),
                Some(None) => Err("invalid 'view' (expected compact|full|summary)".into()),
            }
        }
        "list_displays" => Ok(serde_json::to_value(engine.list_displays()).unwrap_or(Value::Null)),
        "get_affordances" => Ok(engine.affordances_view_scoped(
            arg_bool(args, "include_latent").unwrap_or(false),
            arg(args, "scope").as_deref().unwrap_or("all"),
        )),
        "query_affordances" => match arg(args, "action").as_deref().and_then(parse_action) {
            Some(action) => Ok(json!(engine.query_affordances_scoped(
                action,
                arg_bool(args, "include_latent").unwrap_or(false),
                arg(args, "scope").as_deref().unwrap_or("all"),
            ))),
            None => Err("missing/invalid 'action'".into()),
        },
        _ => return None,
    })
}

fn dispatch_snapshot_tools(
    engine: &mut Engine,
    name: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    Some(match name {
        "page_state" => {
            if let Err(err) = ensure_recent_graph(
                engine,
                arg_bool(args, "fresh").unwrap_or(true),
                arg_bool(args, "force_refresh").unwrap_or(false),
            ) {
                Err(err)
            } else {
                Ok(serde_json::to_value(
                    engine.page_state(
                        args.get("limit").and_then(Value::as_u64).unwrap_or(12) as usize
                    ),
                )
                .unwrap_or(Value::Null))
            }
        }
        "text_snapshot" => {
            if let Err(err) = ensure_recent_graph(
                engine,
                arg_bool(args, "fresh").unwrap_or(true),
                arg_bool(args, "force_refresh").unwrap_or(false),
            ) {
                Err(err)
            } else {
                let query = arg(args, "query");
                Ok(serde_json::to_value(engine.text_snapshot(
                    query.as_deref(),
                    arg_bool(args, "visible_only").unwrap_or(true),
                    args.get("limit").and_then(Value::as_u64).unwrap_or(120) as usize,
                ))
                .unwrap_or(Value::Null))
            }
        }
        "list_browser_tabs" => {
            if let Err(err) = ensure_recent_graph(
                engine,
                arg_bool(args, "fresh").unwrap_or(true),
                arg_bool(args, "force_refresh").unwrap_or(false),
            ) {
                Err(err)
            } else {
                Ok(serde_json::to_value(engine.list_browser_tabs(
                    arg(args, "query").as_deref(),
                    arg_bool(args, "visible_only").unwrap_or(true),
                ))
                .unwrap_or(Value::Null))
            }
        }
        "get_hit_targets" => {
            if let Err(err) = ensure_recent_graph(
                engine,
                arg_bool(args, "fresh").unwrap_or(true),
                arg_bool(args, "force_refresh").unwrap_or(false),
            ) {
                Err(err)
            } else {
                Ok(serde_json::to_value(engine.hit_targets(
                    arg_bool(args, "include_latent").unwrap_or(false),
                    arg(args, "scope").as_deref().unwrap_or("page"),
                    args.get("limit").and_then(Value::as_u64).unwrap_or(80) as usize,
                    arg(args, "previous_epoch").as_deref(),
                ))
                .unwrap_or(Value::Null))
            }
        }
        "window_view" => {
            if let Err(err) = ensure_recent_graph(
                engine,
                arg_bool(args, "fresh").unwrap_or(true),
                arg_bool(args, "force_refresh").unwrap_or(false),
            ) {
                Err(err)
            } else {
                Ok(serde_json::to_value(
                    engine.window_view(
                        args.get("limit").and_then(Value::as_u64).unwrap_or(12) as usize
                    ),
                )
                .unwrap_or(Value::Null))
            }
        }
        "desktop_view" => Ok(serde_json::to_value(
            engine.desktop_view(arg_bool(args, "all").unwrap_or(false)),
        )
        .unwrap_or(Value::Null)),
        "target_visibility" => {
            Ok(serde_json::to_value(engine.target_visibility()).unwrap_or(Value::Null))
        }
        "find_element" => match arg(args, "query") {
            Some(query) => find_element_value(
                engine,
                &query,
                arg_bool(args, "visible_only").unwrap_or(false),
                arg_bool(args, "fresh").unwrap_or(true),
                arg_bool(args, "force_refresh").unwrap_or(false),
            ),
            None => Err("missing 'query'".into()),
        },
        _ => return None,
    })
}

fn dispatch_wait_tools(
    engine: &mut Engine,
    name: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    Some(match name {
        "wait_for_element" => match arg(args, "query") {
            Some(query) => wait_for_element_value(
                engine,
                &query,
                arg_bool(args, "visible_only").unwrap_or(true),
                arg_bool(args, "absent").unwrap_or(false),
                args.get("timeout_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(5_000),
                args.get("interval_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(250),
            ),
            None => Err("missing query".into()),
        },
        "wait_for_text_stable" => {
            let query = arg(args, "query");
            wait_for_text_stable_value(
                engine,
                query.as_deref(),
                arg_bool(args, "visible_only").unwrap_or(true),
                args.get("timeout_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(30_000),
                args.get("stable_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(1_200),
                args.get("interval_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(500),
                args.get("limit").and_then(Value::as_u64).unwrap_or(120) as usize,
            )
        }
        _ => return None,
    })
}

fn dispatch_vision_tools(
    engine: &mut Engine,
    name: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    Some(match name {
        "read_text" => {
            let region = match parse_region(args) {
                Ok(region) => region,
                Err(err) => return Some(Err(err)),
            };
            engine
                .read_text_detailed(
                    region,
                    arg_bool(args, "accurate").unwrap_or(false),
                    arg_bool(args, "content_only").unwrap_or(false),
                )
                .map(|result| serde_json::to_value(result.hits).unwrap_or(Value::Null))
                .map_err(|e| e.to_string())
        }
        "read_text_detailed" => {
            let region = match parse_region(args) {
                Ok(region) => region,
                Err(err) => return Some(Err(err)),
            };
            engine
                .read_text_detailed(
                    region,
                    arg_bool(args, "accurate").unwrap_or(false),
                    arg_bool(args, "content_only").unwrap_or(false),
                )
                .map(|hits| serde_json::to_value(hits).unwrap_or(Value::Null))
                .map_err(|e| e.to_string())
        }
        "read_shapes" => engine
            .read_shapes()
            .map(|shapes| serde_json::to_value(shapes).unwrap_or(Value::Null))
            .map_err(|e| e.to_string()),
        "find_ocr_text" => match arg(args, "query") {
            Some(query) => engine
                .find_ocr_text(
                    &query,
                    arg_bool(args, "content_only").unwrap_or(true),
                    arg_bool(args, "accurate").unwrap_or(true),
                    args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize,
                )
                .map(|result| serde_json::to_value(result).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            None => Err("find_ocr_text requires 'query'".into()),
        },
        "detect_modal" => engine
            .detect_modal()
            .map(|result| serde_json::to_value(result).unwrap_or(Value::Null))
            .map_err(|e| e.to_string()),
        "extract_ocr_cards" => engine
            .extract_ocr_cards(
                arg_bool(args, "accurate").unwrap_or(true),
                arg_bool(args, "content_only").unwrap_or(true),
                args.get("limit").and_then(Value::as_u64).unwrap_or(24) as usize,
            )
            .map(|result| serde_json::to_value(result).unwrap_or(Value::Null))
            .map_err(|e| e.to_string()),
        "read_at" => match (
            args.get("x").and_then(Value::as_f64),
            args.get("y").and_then(Value::as_f64),
        ) {
            (Some(x), Some(y)) => engine
                .read_at(x, y, arg_bool(args, "borrow_cursor").unwrap_or(false))
                .map(|hits| serde_json::to_value(hits).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            _ => Err("read_at requires numeric 'x' and 'y'".into()),
        },
        "read_series" => match args.get("points").map(parse_points) {
            Some(Ok(points)) => engine
                .read_series(&points, arg_bool(args, "borrow_cursor").unwrap_or(false))
                .map(|hits| serde_json::to_value(hits).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            Some(Err(err)) => Err(err),
            None => Err("read_series requires 'points': [[x,y], ...]".into()),
        },
        "scan_chart" => {
            let samples = args.get("samples").and_then(Value::as_u64).unwrap_or(5) as usize;
            engine
                .scan_chart(samples)
                .map(|result| serde_json::to_value(result).unwrap_or(Value::Null))
                .map_err(|e| e.to_string())
        }
        _ => return None,
    })
}

fn dispatch_probe_tools(
    engine: &mut Engine,
    name: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    Some(match name {
        "visual_change_probe" => {
            let region = match parse_region(args) {
                Ok(region) => region,
                Err(err) => return Some(Err(err)),
            };
            engine
                .visual_change_probe(
                    region,
                    args.get("columns").and_then(Value::as_u64).unwrap_or(16) as usize,
                    args.get("rows").and_then(Value::as_u64).unwrap_or(12) as usize,
                    args.get("threshold")
                        .and_then(Value::as_u64)
                        .unwrap_or(12)
                        .min(255) as u8,
                    arg_bool(args, "refresh_on_change").unwrap_or(false),
                )
                .map(|probe| serde_json::to_value(probe).unwrap_or(Value::Null))
                .map_err(|e| e.to_string())
        }
        "analyze_region_ax" => {
            let region = match parse_region(args) {
                Ok(region) => region,
                Err(err) => return Some(Err(err)),
            };
            Ok(serde_json::to_value(engine.analyze_region_ax(
                region,
                args.get("columns").and_then(Value::as_u64).unwrap_or(8) as usize,
                args.get("rows").and_then(Value::as_u64).unwrap_or(6) as usize,
            ))
            .unwrap_or(Value::Null))
        }
        _ => return None,
    })
}

fn dispatch_trace_tools(
    engine: &mut Engine,
    name: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    Some(match name {
        "diff_since" => {
            let diff = engine.diff_since();
            if arg_bool(args, "summary").unwrap_or(false) {
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
        _ => return None,
    })
}
