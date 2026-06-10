//! Minimal MCP server over stdio (newline-delimited JSON-RPC 2.0).
//!
//! Dependency-light by design — the POC implements just the slice of MCP a host
//! needs: `initialize`, `tools/list`, `tools/call`, `ping`. Each tool maps onto
//! an [`Engine`] method, so the same risk-gating + audit applies whether the
//! engine is driven from the CLI demo or a real MCP client.

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

use crate::engine::{Engine, SceneView};
use visualops_core::{Bbox, SemanticAction};

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
                if req.get("id").is_some() {
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
    vec![
        tool("refresh", "Re-perceive the target window and rebuild the scene + affordance graphs.", json!({})),
        tool(
            "get_scene_graph",
            "Return the current scene graph. view: \"compact\" (default, light per-node projection) | \"full\" (every field) | \"summary\" (counts only, no node list). actionable_only drops off-screen/disabled nodes (compact/full).",
            schema(
                json!({
                    "view": { "type": "string", "enum": ["compact", "full", "summary"], "description": "projection, default compact" },
                    "actionable_only": { "type": "boolean", "description": "only on-screen, enabled, real-bbox nodes (compact/full)" }
                }),
                &[],
            ),
        ),
        tool(
            "get_affordances",
            "Return the affordance graph (actions + risk per element). Latent (off-screen / zero-bbox) nodes are omitted unless include_latent is true.",
            schema(json!({ "include_latent": { "type": "boolean", "description": "include latent/off-screen nodes (default false)" } }), &[]),
        ),
        tool(
            "find_element",
            "Find elements whose id/label/role contains the query (case-insensitive).",
            schema(json!({ "query": { "type": "string" } }), &["query"]),
        ),
        tool(
            "read_text",
            "OCR the target window (or an optional screen-point region x,y,w,h) via Apple Vision; returns recognised text lines with screen bbox + confidence.",
            schema(
                json!({
                    "region": {
                        "type": "object",
                        "description": "optional screen-point region; omit for the whole window",
                        "properties": {
                            "x": { "type": "number" },
                            "y": { "type": "number" },
                            "w": { "type": "number" },
                            "h": { "type": "number" }
                        },
                        "required": ["x", "y", "w", "h"]
                    }
                }),
                &[],
            ),
        ),
        tool(
            "read_shapes",
            "Detect geometric primitives (rect/bar/circle/line) in the target window via the CV layer; returns shapes with kind, screen bbox + confidence. The figures (charts, custom-drawn UI) AX and OCR miss.",
            json!({}),
        ),
        tool(
            "query_affordances",
            "List element ids that expose a given semantic action (click|type|hover|open_menu|pick|drag|...). Latent (off-screen / zero-bbox) nodes are omitted unless include_latent is true.",
            schema(
                json!({
                    "action": { "type": "string", "description": "semantic action" },
                    "include_latent": { "type": "boolean", "description": "include latent nodes (default false)" }
                }),
                &["action"],
            ),
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
            "click_at",
            "Click at a raw screen point (x,y). For OCR-driven navigation: read_text a link, then click_at its bbox centre. NOTE: not bound to an element — bypasses per-element risk gating.",
            schema(json!({ "x": {"type":"number"}, "y": {"type":"number"} }), &["x", "y"]),
        ),
        tool(
            "hover_at",
            "Hover (background mouse-move, no cursor movement) at a raw screen point (x,y) so the target reveals a hover state — e.g. a chart crosshair tooltip / value-at-cursor — then read_text it. A probe: no gating, no audit.",
            schema(json!({ "x": {"type":"number"}, "y": {"type":"number"} }), &["x", "y"]),
        ),
        tool(
            "read_at",
            "Read the value at a screen point by time-multiplexing the cursor: briefly BORROW the OS cursor (decouple the user's mouse so it can't fight the warp), warp to (x,y) to trigger a real hover (chart crosshair), OCR around it, then restore the cursor + re-couple the mouse. For non-CDP surfaces.",
            schema(json!({ "x": {"type":"number"}, "y": {"type":"number"} }), &["x", "y"]),
        ),
        tool(
            "read_series",
            "Read values at SEVERAL screen points in ONE cursor borrow — efficient for sampling a chart at intervals: decouple once, warp+OCR each point, restore once (the user's mouse freezes once, not per point). points = [[x,y], ...]; returns one OCR list per point.",
            schema(
                json!({ "points": { "type": "array", "items": { "type": "array", "items": { "type": "number" } } } }),
                &["points"],
            ),
        ),
        tool(
            "scan_chart",
            "Detect → confirm rendered → traverse → series. Coarse-to-fine CV first answers whether a chart is actually rendered (not a blank plot) and where it sits; only if present does it traverse the plot at mid-height and read the value-at-cursor across it. Returns {present, fill_ratio, region, samples:[{x,value,time,raw}]}. Honest 'present:false' over an empty plot.",
            schema(json!({ "samples": { "type": "integer", "description": "points across the width (2-12, default 5)" } }), &[]),
        ),
        tool(
            "press_key",
            "Press a named key on the target (e.g. \"Return\"/\"Enter\" to submit a typed URL, \"Tab\", \"Escape\"). Raw, ungated keyboard input.",
            schema(json!({ "key": {"type":"string"} }), &["key"]),
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

fn tool(name: &str, description: &str, mut input_schema: Value) -> Value {
    // MCP clients validate each `inputSchema` as a JSON Schema object (Claude Code
    // rejects the whole `tools/list` otherwise: "expected object"). A no-arg tool
    // passing `{}` must still declare `"type": "object"`; normalise it here so the
    // call sites stay terse and future no-arg tools can't reintroduce the bug.
    if input_schema.get("type").is_none() {
        input_schema = json!({ "type": "object", "properties": {} });
    }
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
    let arg_bool = |k: &str| args.get(k).and_then(Value::as_bool);

    let outcome: Result<Value, String> = match name {
        "refresh" => engine.refresh().map(|_| json!("ok")).map_err(|e| e.to_string()),
        "get_scene_graph" => match arg("view").as_deref().map(SceneView::parse) {
            None => Ok(engine.scene_graph_view(SceneView::Compact, arg_bool("actionable_only").unwrap_or(false))),
            Some(Some(v)) => Ok(engine.scene_graph_view(v, arg_bool("actionable_only").unwrap_or(false))),
            Some(None) => Err("invalid 'view' (expected compact|full|summary)".into()),
        },
        "get_affordances" => Ok(engine.affordances_view(arg_bool("include_latent").unwrap_or(false))),
        "find_element" => match arg("query") {
            Some(q) => Ok(serde_json::to_value(engine.find_element(&q)).unwrap_or(Value::Null)),
            None => Err("missing 'query'".into()),
        },
        "read_text" => match parse_region(&args) {
            Ok(region) => engine
                .read_text(region)
                .map(|hits| serde_json::to_value(hits).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            Err(e) => Err(e),
        },
        "read_shapes" => engine
            .read_shapes()
            .map(|shapes| serde_json::to_value(shapes).unwrap_or(Value::Null))
            .map_err(|e| e.to_string()),
        "query_affordances" => match arg("action").as_deref().and_then(parse_action) {
            Some(a) => Ok(json!(engine.query_affordances_filtered(a, arg_bool("include_latent").unwrap_or(false)))),
            None => Err("missing/invalid 'action'".into()),
        },
        "click_element" => match arg("id") {
            Some(eid) => engine
                .click_element(&eid, arg("reasoning").as_deref())
                .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            None => Err("missing 'id'".into()),
        },
        "type_into" => match (arg("id"), arg("text")) {
            (Some(eid), Some(text)) => engine
                .type_into(&eid, &text, arg("reasoning").as_deref())
                .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            _ => Err("missing 'id' or 'text'".into()),
        },
        "hover_probe" => match arg("id") {
            Some(eid) => engine
                .hover_probe(&eid)
                .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            None => Err("missing 'id'".into()),
        },
        "drag_element" => match (arg("source_id"), arg("target_id")) {
            (Some(source_id), Some(target_id)) => engine
                .drag_element(&source_id, &target_id, arg("reasoning").as_deref())
                .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            _ => Err("missing 'source_id' or 'target_id'".into()),
        },
        "click_at" => match (
            args.get("x").and_then(Value::as_f64),
            args.get("y").and_then(Value::as_f64),
        ) {
            (Some(x), Some(y)) => engine
                .click_at(x, y)
                .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            _ => Err("click_at requires numeric 'x' and 'y'".into()),
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
                .read_at(x, y)
                .map(|h| serde_json::to_value(h).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            _ => Err("read_at requires numeric 'x' and 'y'".into()),
        },
        "read_series" => match args.get("points").and_then(Value::as_array) {
            Some(arr) => {
                let pts: Vec<(f64, f64)> = arr
                    .iter()
                    .filter_map(|p| {
                        let a = p.as_array()?;
                        Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
                    })
                    .collect();
                engine
                    .read_series(&pts)
                    .map(|h| serde_json::to_value(h).unwrap_or(Value::Null))
                    .map_err(|e| e.to_string())
            }
            None => Err("read_series requires 'points': [[x,y], ...]".into()),
        },
        "scan_chart" => {
            let n = args.get("samples").and_then(Value::as_u64).unwrap_or(5) as usize;
            engine
                .scan_chart(n)
                .map(|r| serde_json::to_value(r).unwrap_or(Value::Null))
                .map_err(|e| e.to_string())
        }
        "press_key" => match arg("key") {
            Some(key) => engine
                .press_key(&key)
                .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
                .map_err(|e| e.to_string()),
            None => Err("missing 'key'".into()),
        },
        "approve" => match arg("id") {
            Some(eid) => engine
                .approve(&eid)
                .map(|_| json!("approved"))
                .map_err(|e| e.to_string()),
            None => Err("missing 'id'".into()),
        },
        "verify_state" => match (arg("id"), arg("field"), arg("expected")) {
            (Some(eid), Some(field), Some(expected)) => engine
                .verify_state(&eid, &field, &expected)
                .map(|ok| json!({ "matches": ok }))
                .map_err(|e| e.to_string()),
            _ => Err("missing 'id', 'field' or 'expected'".into()),
        },
        "diff_since" => Ok(serde_json::to_value(engine.diff_since()).unwrap_or(Value::Null)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use visualops_core::mock::{MockPerceptor, RecordingExecutor};
    use visualops_core::Target;

    fn engine() -> Engine {
        engine_with_window(105)
    }

    fn engine_with_window(window_id: u32) -> Engine {
        let perceptor = Box::new(MockPerceptor::notes_fixture().unwrap());
        let exec = Box::<RecordingExecutor>::default();
        Engine::new(perceptor, exec, Target { pid: 1363, window_id }).unwrap()
    }

    fn engine_with_pid(pid: i32) -> Engine {
        let perceptor = Box::new(MockPerceptor::notes_fixture().unwrap());
        let exec = Box::<RecordingExecutor>::default();
        Engine::new(perceptor, exec, Target { pid, window_id: 105 }).unwrap()
    }

    /// Drive `handle_tool_call` exactly as the stdio loop does.
    fn call(engine: &mut Engine, name: &str, arguments: Value) -> Value {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        });
        handle_tool_call(engine, json!(1), &req)
    }

    fn is_error(resp: &Value) -> bool {
        resp["result"]["isError"].as_bool().unwrap_or(false)
    }

    fn text(resp: &Value) -> String {
        resp["result"]["content"][0]["text"].as_str().unwrap_or("").to_string()
    }

    // Table-driven invariants of the dispatcher (audit #4): a malformed call must
    // become a clean `isError:true` JSON-RPC result, never a panic or a success.
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
            assert!(t.contains(needle), "{tool}: error {t:?} should mention {needle:?}");
            // Even on error the envelope stays well-formed JSON-RPC.
            assert_eq!(resp["jsonrpc"], "2.0");
            assert_eq!(resp["id"], json!(1));
        }
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
    fn tools_list_exposes_read_text_with_object_schema() {
        let tools = tools_list();
        // + read_at + read_series brought the set to 20.
        assert_eq!(tools.len(), 21, "tool count");
        // Every tool must declare a JSON-Schema object input (the type:object fix).
        for t in &tools {
            assert_eq!(
                t["inputSchema"]["type"], "object",
                "tool {} has a non-object inputSchema: {}", t["name"], t["inputSchema"]
            );
        }
        let read_text = tools
            .iter()
            .find(|t| t["name"] == "read_text")
            .expect("read_text tool present");
        assert_eq!(read_text["inputSchema"]["type"], "object");
        // `region` is optional → it must not be in `required`.
        assert_eq!(read_text["inputSchema"]["required"], json!([]));
    }

    #[test]
    fn read_text_without_live_window_is_a_clean_error() {
        // An invalid window id → no live macOS window → a clean Err, never a panic.
        // (Off macOS, the stub returns the same class of error.)
        let mut e = engine_with_window(u32::MAX);

        // Direct engine call carries the "live macOS window" message.
        let err = e.read_text(None).unwrap_err();
        assert!(err.to_string().contains("live macOS window"), "unexpected error: {err}");

        // Through the dispatcher: a well-formed isError result, not a crash.
        let resp = call(&mut e, "read_text", json!({}));
        assert!(is_error(&resp), "read_text without a live window must be isError: {resp}");
        assert_eq!(resp["jsonrpc"], "2.0");

        // A malformed region is rejected before any capture is attempted.
        let bad = call(&mut e, "read_text", json!({ "region": { "x": 1.0 } }));
        assert!(is_error(&bad));
        assert!(text(&bad).contains("region"), "got: {}", text(&bad));
    }

    #[test]
    fn tools_list_exposes_click_at_and_press_key() {
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
    }

    #[test]
    fn raw_input_tools_dispatch_and_error_cleanly() {
        // A non-existent pid: on macOS the raw CGEvent posts to nothing (no test
        // side effect); the dispatch wiring is what we assert here.
        let mut e = engine_with_pid(i32::MAX);

        // Missing required args → isError, before reaching the engine.
        assert!(is_error(&call(&mut e, "press_key", json!({}))), "press_key needs 'key'");
        assert!(
            is_error(&call(&mut e, "click_at", json!({ "x": 10.0 }))),
            "click_at needs both 'x' and 'y'"
        );

        // press_key with an unknown key → a clean isError on both platforms (macOS:
        // the backend rejects the key name; non-macOS: the stub is unsupported).
        let bad = call(&mut e, "press_key", json!({ "key": "definitely-not-a-real-key-xyz" }));
        assert!(is_error(&bad), "unknown key must be a clean isError: {bad}");
        assert_eq!(bad["jsonrpc"], "2.0");

        // click_at with valid coords reaches the engine and returns a well-formed
        // JSON-RPC response (success when a live window backs the pid, isError on the
        // non-macOS stub) — never a panic.
        let resp = call(&mut e, "click_at", json!({ "x": 100.0, "y": 200.0 }));
        assert_eq!(resp["jsonrpc"], "2.0");
        assert!(resp.get("result").is_some(), "well-formed response: {resp}");
    }
}
