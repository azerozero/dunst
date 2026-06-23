use super::*;
use crate::serve::registry::{tool_route, ToolRoute};

mod args;
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
            Err(format!("unknown tool: {name}")),
        );
    };

    // screenshot returns an IMAGE content block, not text.
    if route == ToolRoute::Screenshot {
        return screenshot_response(engine, id, name, started, session.as_ref());
    }

    let outcome = match route {
        ToolRoute::Read => read_tools::dispatch(engine, name, &args),
        ToolRoute::Element => element_tools::dispatch(engine, name, &args),
        ToolRoute::Raw => raw_tools::dispatch(engine, name, &args),
        ToolRoute::WindowApp => window_app_tools::dispatch(engine, name, &args),
        ToolRoute::Screenshot => unreachable!("handled above"),
    }
    .unwrap_or_else(|| Err(format!("registered tool has no handler: {name}")));

    text_response(id, name, started, session.as_ref(), outcome)
}

fn screenshot_response(
    engine: &Engine,
    id: Value,
    name: &str,
    started: Instant,
    session: Option<&SessionIdentity>,
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
            ),
        ),
    }
}

fn text_response(
    id: Value,
    name: &str,
    started: Instant,
    session: Option<&SessionIdentity>,
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
            ),
        ),
    }
}
