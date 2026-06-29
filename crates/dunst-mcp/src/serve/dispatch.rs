use super::coordination::CoordinationGuard;
use super::*;
use crate::serve::registry::{tool_route, ToolRoute};

mod args;
mod batch_tools;
mod element_tools;
mod raw_tools;
mod read_tools;
mod window_app_tools;

use args::*;

/// Dispatch a `tools/call`. Returns a full JSON-RPC response object.
pub(super) fn handle_tool_call(engine: &mut Engine, id: Value, req: &Value) -> Value {
    let started = Instant::now();
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let session = engine.session_identity().cloned();

    let Some(route) = tool_route(name) else {
        return text_response(
            id,
            name,
            started,
            session.as_ref(),
            None,
            Err(format!("unknown tool: {name}")),
        );
    };

    // screenshot returns an IMAGE content block, not text.
    if route == ToolRoute::Screenshot {
        return screenshot_response(engine, id, name, started, session.as_ref(), None);
    }

    let mut coordination_meta = None;
    let mut _coordination_guard = None;
    if tool_requires_mutation_coordination(route, name, &args) {
        if let Err((message, epoch_meta)) = validate_expected_epoch(engine, name, &args) {
            merge_coordination_meta(&mut coordination_meta, "epoch", epoch_meta);
            return text_response(
                id,
                name,
                started,
                session.as_ref(),
                coordination_meta.as_ref(),
                Err(message),
            );
        }
        if let Some(session) = session.as_ref() {
            let (_, window_id) = engine.target();
            match CoordinationGuard::acquire(session, window_id, name, &args) {
                Ok(guard) => {
                    merge_coordination_meta(
                        &mut coordination_meta,
                        "mutation",
                        guard.summary_value(),
                    );
                    _coordination_guard = Some(guard);
                }
                Err(err) => {
                    merge_coordination_meta(&mut coordination_meta, "mutation", err.summary);
                    return text_response(
                        id,
                        name,
                        started,
                        Some(session),
                        coordination_meta.as_ref(),
                        Err(err.message),
                    );
                }
            }
        }
    }

    let outcome = match route {
        ToolRoute::Read => read_tools::dispatch(engine, name, &args),
        ToolRoute::Element => element_tools::dispatch(engine, name, &args),
        ToolRoute::Batch => batch_tools::dispatch(engine, name, &args),
        ToolRoute::Raw => raw_tools::dispatch(engine, name, &args),
        ToolRoute::WindowApp => window_app_tools::dispatch(engine, name, &args),
        ToolRoute::Screenshot => unreachable!("handled above"),
    }
    .unwrap_or_else(|| Err(format!("registered tool has no handler: {name}")));

    text_response(
        id,
        name,
        started,
        session.as_ref(),
        coordination_meta.as_ref(),
        outcome,
    )
}

fn screenshot_response(
    engine: &Engine,
    id: Value,
    name: &str,
    started: Instant,
    session: Option<&SessionIdentity>,
    coordination: Option<&Value>,
) -> Value {
    match engine.screenshot() {
        Some(result) => {
            let mut meta = serde_json::to_value(&result).unwrap_or(Value::Null);
            if let Value::Object(obj) = &mut meta {
                obj.remove("png_base64");
            }
            result_obj(
                id,
                add_timing_meta(
                    json!({
                        "content": [{ "type": "image", "data": result.png_base64, "mimeType": "image/png" }],
                        "diagnostics": meta
                    }),
                    name,
                    started,
                    session,
                    coordination,
                ),
            )
        }
        None => result_obj(
            id,
            add_timing_meta(
                json!({ "content": [{ "type": "text", "text": "screenshot failed" }], "isError": true }),
                name,
                started,
                session,
                coordination,
            ),
        ),
    }
}

fn text_response(
    id: Value,
    name: &str,
    started: Instant,
    session: Option<&SessionIdentity>,
    coordination: Option<&Value>,
    outcome: Result<Value, String>,
) -> Value {
    match outcome {
        Ok(value) => {
            let text = if value.is_string() {
                value.as_str().unwrap().to_owned()
            } else {
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
            };
            result_obj(
                id,
                add_timing_meta(
                    json!({ "content": [{ "type": "text", "text": text }] }),
                    name,
                    started,
                    session,
                    coordination,
                ),
            )
        }
        Err(message) => result_obj(
            id,
            add_timing_meta(
                json!({ "content": [{ "type": "text", "text": message }], "isError": true }),
                name,
                started,
                session,
                coordination,
            ),
        ),
    }
}

fn tool_requires_mutation_coordination(route: ToolRoute, name: &str, args: &Value) -> bool {
    match route {
        ToolRoute::Read => match name {
            "read_at" | "read_series" => arg_bool(args, "borrow_cursor").unwrap_or(false),
            "scan_chart" => true,
            "enumerate_choices" => arg_bool(args, "scroll_scan").unwrap_or(false),
            _ => false,
        },
        ToolRoute::Element => !matches!(name, "approve" | "verify_state"),
        ToolRoute::Batch => true,
        ToolRoute::Raw => !matches!(name, "hover_at" | "unstick_cursor"),
        ToolRoute::WindowApp => matches!(
            name,
            "move_window_to_display"
                | "move_app_to_display"
                | "arrange_windows"
                | "expose_target_window"
                | "launch_app"
                | "open_url_and_attach_tab"
                | "close_app"
        ),
        ToolRoute::Screenshot => false,
    }
}

fn validate_expected_epoch(
    engine: &mut Engine,
    tool_name: &str,
    args: &Value,
) -> Result<(), (String, Value)> {
    let Some(expected) = arg(args, "expected_epoch") else {
        return Ok(());
    };
    if expected.trim().is_empty() {
        return Ok(());
    }
    let refresh = engine.refresh_if_stale().map_err(|err| {
        let meta = json!({
            "status": "epoch_check_failed",
            "tool": tool_name,
            "expected_epoch": expected,
            "reason": err.to_string()
        });
        (format!("expected_epoch check failed: {err}"), meta)
    })?;
    let current = engine.current_ui_epoch_fingerprint();
    let matches = current == expected;
    let meta = json!({
        "status": if matches { "matched" } else { "stale" },
        "tool": tool_name,
        "expected_epoch": expected,
        "current_epoch": current,
        "refreshed": refresh
    });
    if matches {
        Ok(())
    } else {
        Err((
            "stale UI epoch: expected_epoch no longer matches the current target state; call get_hit_targets again before mutating".into(),
            meta,
        ))
    }
}

fn merge_coordination_meta(meta: &mut Option<Value>, key: &str, value: Value) {
    if meta.is_none() {
        *meta = Some(json!({}));
    }
    if let Some(Value::Object(obj)) = meta {
        obj.insert(key.into(), value);
    }
}
