use super::*;

pub(super) fn dispatch(
    engine: &mut Engine,
    name: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    Some(match name {
        "list_windows" => Ok(serde_json::to_value(
            engine.list_windows(arg_bool(args, "all").unwrap_or(false)),
        )
        .unwrap_or(Value::Null)),
        "move_window_to_display" => match args.get("display").and_then(Value::as_u64) {
            Some(display) => engine
                .move_window_to_display(
                    display as usize,
                    arg_bool(args, "preserve_size").unwrap_or(true),
                )
                .map(|view| serde_json::to_value(view).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            None => Err("move_window_to_display requires integer 'display'".into()),
        },
        "move_app_to_display" => match (
            arg(args, "app"),
            args.get("display").and_then(Value::as_u64),
        ) {
            (Some(app), Some(display)) => engine
                .move_app_to_display(
                    &app,
                    display as usize,
                    arg_bool(args, "preserve_size").unwrap_or(true),
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
                        arg(args, "mode").as_deref().unwrap_or("grid"),
                        arg(args, "app").as_deref(),
                        &window_ids,
                        arg_bool(args, "all").unwrap_or(false),
                    )
                    .map(|result| serde_json::to_value(result).unwrap_or(Value::Null))
                    .map_err(|e| e.to_string()),
                Err(err) => Err(err),
            },
            None => Err("arrange_windows requires integer 'display'".into()),
        },
        "expose_target_window" => engine
            .expose_target_window(arg_bool(args, "arrange_if_needed").unwrap_or(false))
            .map(|result| serde_json::to_value(result).unwrap_or(Value::Null))
            .map_err(|e| e.to_string()),
        "list_apps" => Ok(
            serde_json::to_value(engine.list_apps(arg(args, "query").as_deref()))
                .unwrap_or(Value::Null),
        ),
        "list_launchable_apps" => Ok(serde_json::to_value(engine.list_launchable_apps(
            arg(args, "query").as_deref(),
            args.get("limit").and_then(Value::as_u64).unwrap_or(80) as usize,
        ))
        .unwrap_or(Value::Null)),
        "app_info" => {
            let info = engine.app_info(
                arg(args, "app").as_deref(),
                arg(args, "bundle_id").as_deref(),
                arg(args, "path").as_deref(),
            );
            match info {
                Some(info) => Ok(serde_json::to_value(info).unwrap_or(Value::Null)),
                None => Err("app_info found no matching .app bundle".into()),
            }
        }
        "attach" => match args.get("window_id").and_then(Value::as_u64) {
            Some(window_id) => match engine.attach_window(window_id as u32) {
                Ok(()) => {
                    let (pid, window_id) = engine.target();
                    let graph = engine.scene_graph();
                    Ok(json!({
                        "attached": { "pid": pid, "window_id": window_id, "title": graph.window.title },
                        "n_nodes": graph.nodes.len()
                    }))
                }
                Err(err) => Err(err.to_string()),
            },
            None => Err("attach requires integer 'window_id'".into()),
        },
        "launch_app" => match arg(args, "app") {
            Some(app) => {
                let extra: Vec<String> = args
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|value| value.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(serde_json::to_value(engine.launch_app(
                    &app,
                    arg(args, "url").as_deref(),
                    &extra,
                ))
                .unwrap_or(Value::Null))
            }
            None => Err("launch_app requires 'app'".into()),
        },
        "open_url_and_attach_tab" => match (arg(args, "app"), arg(args, "url")) {
            (Some(app), Some(url)) => {
                let extra: Vec<String> = args
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|value| value.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(
                    serde_json::to_value(engine.open_url_and_attach_tab(&app, &url, &extra))
                        .unwrap_or(Value::Null),
                )
            }
            _ => Err("open_url_and_attach_tab requires 'app' and 'url'".into()),
        },
        "navigate" => match arg(args, "url") {
            Some(url) => Ok(serde_json::to_value(engine.navigate(&url)).unwrap_or(Value::Null)),
            None => Err("navigate requires 'url'".into()),
        },
        "close_app" => match arg(args, "app") {
            Some(app) => Ok(json!({ "closed": engine.close_app(&app) })),
            None => Err("close_app requires 'app'".into()),
        },
        _ => return None,
    })
}
