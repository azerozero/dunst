use super::*;

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

    // screenshot returns an IMAGE content block, not text.
    if name == "screenshot" {
        return screenshot_response(engine, id, name, started);
    }

    let outcome = read_tools::dispatch(engine, name, &args)
        .or_else(|| element_tools::dispatch(engine, name, &args))
        .or_else(|| raw_tools::dispatch(engine, name, &args))
        .or_else(|| window_app_tools::dispatch(engine, name, &args))
        .unwrap_or_else(|| Err(format!("unknown tool: {name}")));

    text_response(id, name, started, outcome)
}

fn screenshot_response(engine: &Engine, id: Value, name: &str, started: Instant) -> Value {
    match engine.screenshot() {
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
    }
}

fn text_response(id: Value, name: &str, started: Instant, outcome: Result<Value, String>) -> Value {
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
                ),
            )
        }
        Err(message) => result_obj(
            id,
            add_timing_meta(
                json!({ "content": [{ "type": "text", "text": message }], "isError": true }),
                name,
                started,
            ),
        ),
    }
}
