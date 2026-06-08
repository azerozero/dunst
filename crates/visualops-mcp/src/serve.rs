//! Minimal MCP server over stdio (newline-delimited JSON-RPC 2.0).
//!
//! Dependency-light by design — the POC implements just the slice of MCP a host
//! needs: `initialize`, `tools/list`, `tools/call`, `ping`. Each tool maps onto
//! an [`Engine`] method, so the same risk-gating + audit applies whether the
//! engine is driven from the CLI demo or a real MCP client.

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

use crate::engine::Engine;
use visualops_core::SemanticAction;

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Run the stdio server loop until stdin closes.
pub fn serve(mut engine: Engine) -> i32 {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    eprintln!("visualops-mcp: stdio MCP server ready ({} tools)", tools_list().len());

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                send(&mut out, error_obj(Value::Null, -32700, &format!("parse error: {e}")));
                continue;
            }
        };
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");

        match method {
            "initialize" => send(&mut out, result_obj(id, initialize_result())),
            "notifications/initialized" => { /* notification: no reply */ }
            "ping" => send(&mut out, result_obj(id, json!({}))),
            "tools/list" => send(&mut out, result_obj(id, json!({ "tools": tools_list() }))),
            "tools/call" => {
                let resp = handle_tool_call(&mut engine, id, &req);
                send(&mut out, resp);
            }
            other => {
                if !req.get("id").is_none() {
                    send(&mut out, error_obj(id, -32601, &format!("method not found: {other}")));
                }
            }
        }
    }
    0
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "visualops-mcp", "version": env!("CARGO_PKG_VERSION") }
    })
}

fn tools_list() -> Vec<Value> {
    let str_arg = |name: &str, desc: &str| json!({ name: { "type": "string", "description": desc } });
    vec![
        tool("refresh", "Re-perceive the target window and rebuild the scene + affordance graphs.", json!({})),
        tool("get_scene_graph", "Return the current scene graph (nodes, roots, window).", json!({})),
        tool("get_affordances", "Return the affordance graph (actions + risk per element).", json!({})),
        tool(
            "find_element",
            "Find elements whose id/label/role contains the query (case-insensitive).",
            schema(json!({ "query": { "type": "string" } }), &["query"]),
        ),
        tool(
            "query_affordances",
            "List element ids that expose a given semantic action (click|type|hover|open_menu|pick|drag|...).",
            schema(str_arg("action", "semantic action"), &["action"]),
        ),
        tool(
            "click_element",
            "Click an element by id. High-risk elements return pending_approval until approve() is called.",
            schema(json!({ "id": {"type":"string"}, "reasoning": {"type":"string"} }), &["id"]),
        ),
        tool(
            "type_into",
            "Type text into a text element by id (subject to risk gating).",
            schema(json!({ "id": {"type":"string"}, "text": {"type":"string"}, "reasoning": {"type":"string"} }), &["id", "text"]),
        ),
        tool(
            "hover_probe",
            "Hover an element by id (reveals tooltips on a live target).",
            schema(json!({ "id": {"type":"string"} }), &["id"]),
        ),
        tool(
            "drag_element",
            "Drag a source element onto a target element by id (subject to risk gating). The drop point is the target's bbox centre.",
            schema(
                json!({ "source_id": {"type":"string"}, "target_id": {"type":"string"}, "reasoning": {"type":"string"} }),
                &["source_id", "target_id"],
            ),
        ),
        tool(
            "approve",
            "Whitelist a high-risk element so the next action on it proceeds.",
            schema(json!({ "id": {"type":"string"} }), &["id"]),
        ),
        tool(
            "verify_state",
            "Assert an element's field (label|value|enabled) equals an expected value.",
            schema(json!({ "id": {"type":"string"}, "field": {"type":"string"}, "expected": {"type":"string"} }), &["id", "field", "expected"]),
        ),
        tool("diff_since", "Structural diff between the previous and current scene graph.", json!({})),
        tool("export_trace", "Export the audit trail (every attempted action) as JSON.", json!({})),
    ]
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": input_schema })
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

/// Dispatch a `tools/call`. Returns a full JSON-RPC response object.
fn handle_tool_call(engine: &mut Engine, id: Value, req: &Value) -> Value {
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let arg = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_owned);

    let outcome: Result<Value, String> = match name {
        "refresh" => engine.refresh().map(|_| json!("ok")).map_err(|e| e.to_string()),
        "get_scene_graph" => Ok(serde_json::to_value(engine.scene_graph()).unwrap()),
        "get_affordances" => Ok(serde_json::to_value(engine.affordance_graph()).unwrap()),
        "find_element" => match arg("query") {
            Some(q) => Ok(serde_json::to_value(engine.find_element(&q)).unwrap()),
            None => Err("missing 'query'".into()),
        },
        "query_affordances" => match arg("action").as_deref().and_then(parse_action) {
            Some(a) => Ok(json!(engine.query_affordances(a))),
            None => Err("missing/invalid 'action'".into()),
        },
        "click_element" => match arg("id") {
            Some(eid) => engine
                .click_element(&eid, arg("reasoning").as_deref())
                .map(|e| serde_json::to_value(e).unwrap())
                .map_err(|e| e.to_string()),
            None => Err("missing 'id'".into()),
        },
        "type_into" => match (arg("id"), arg("text")) {
            (Some(eid), Some(text)) => engine
                .type_into(&eid, &text, arg("reasoning").as_deref())
                .map(|e| serde_json::to_value(e).unwrap())
                .map_err(|e| e.to_string()),
            _ => Err("missing 'id' or 'text'".into()),
        },
        "hover_probe" => match arg("id") {
            Some(eid) => engine
                .hover_probe(&eid)
                .map(|e| serde_json::to_value(e).unwrap())
                .map_err(|e| e.to_string()),
            None => Err("missing 'id'".into()),
        },
        "drag_element" => match (arg("source_id"), arg("target_id")) {
            (Some(source_id), Some(target_id)) => engine
                .drag_element(&source_id, &target_id, arg("reasoning").as_deref())
                .map(|e| serde_json::to_value(e).unwrap())
                .map_err(|e| e.to_string()),
            _ => Err("missing 'source_id' or 'target_id'".into()),
        },
        "approve" => match arg("id") {
            Some(eid) => {
                engine.approve(&eid);
                Ok(json!("approved"))
            }
            None => Err("missing 'id'".into()),
        },
        "verify_state" => match (arg("id"), arg("field"), arg("expected")) {
            (Some(eid), Some(field), Some(expected)) => engine
                .verify_state(&eid, &field, &expected)
                .map(|ok| json!({ "matches": ok }))
                .map_err(|e| e.to_string()),
            _ => Err("missing 'id', 'field' or 'expected'".into()),
        },
        "diff_since" => Ok(serde_json::to_value(engine.diff_since()).unwrap()),
        "export_trace" => engine.export_trace().map(Value::String).map_err(|e| e.to_string()),
        other => Err(format!("unknown tool: {other}")),
    };

    match outcome {
        Ok(v) => {
            let text = if v.is_string() {
                v.as_str().unwrap().to_owned()
            } else {
                serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string())
            };
            result_obj(id, json!({ "content": [{ "type": "text", "text": text }] }))
        }
        Err(msg) => result_obj(
            id,
            json!({ "content": [{ "type": "text", "text": msg }], "isError": true }),
        ),
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

fn result_obj(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_obj(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn send(out: &mut impl Write, msg: Value) {
    let _ = writeln!(out, "{msg}");
    let _ = out.flush();
}
