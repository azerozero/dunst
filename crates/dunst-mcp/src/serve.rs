//! Minimal MCP server over stdio (newline-delimited JSON-RPC 2.0).
//!
//! Dependency-light by design — the POC implements just the slice of MCP a host
//! needs: `initialize`, `tools/list`, `tools/call`, `ping`. Each tool maps onto
//! an [`Engine`] method, so the same risk-gating + audit applies whether the
//! engine is driven from the CLI demo or a real MCP client.

use std::{
    any::Any,
    io::{self, BufRead, Write},
    panic::{catch_unwind, AssertUnwindSafe},
    time::{Duration, Instant},
};

use serde_json::{json, Value};

mod dispatch;
mod response;
mod tools;

use dispatch::handle_tool_call;
use response::{add_timing_meta, audit_entry_value, diff_summary_value, option_pick_value};
use tools::tools_list;

use crate::engine::{Engine, SceneView};
use dunst_core::{Bbox, SceneNode, SemanticAction};

#[cfg(test)]
use dunst_core::{ActionResult, AuditEntry, GraphDiff, NodeChange};

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const BUILD_GIT_SHA: Option<&str> = option_env!("DUNST_BUILD_GIT_SHA");
const BUILD_GIT_DIRTY: Option<&str> = option_env!("DUNST_BUILD_GIT_DIRTY");
const BUILD_TIME_UNIX: Option<&str> = option_env!("DUNST_BUILD_TIME_UNIX");
const FORCE_REFRESH_COALESCE_TTL: Duration = Duration::from_millis(300);
const FIND_ELEMENT_FORCE_REFRESH_FAST_PATH_TTL: Duration = Duration::from_millis(2_000);
const DIFF_SUMMARY_VALUE_LIMIT: usize = 160;

/// Run the stdio server loop until stdin closes.
pub fn serve(mut engine: Engine) -> i32 {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    eprintln!(
        "dunst-mcp: stdio MCP server ready (version {}, git {}, dirty {}, built {}, {} tools)",
        SERVER_VERSION,
        build_git_sha(),
        build_git_dirty(),
        build_time_unix(),
        tools_list().len()
    );

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                send(
                    &mut out,
                    error_obj(Value::Null, -32700, &format!("parse error: {e}")),
                );
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
                let resp = handle_tool_call_safely(&mut engine, id, &req);
                send(&mut out, resp);
            }
            other => {
                if req.get("id").is_some() {
                    send(
                        &mut out,
                        error_obj(id, -32601, &format!("method not found: {other}")),
                    );
                }
            }
        }
    }
    0
}

fn handle_tool_call_safely(engine: &mut Engine, id: Value, req: &Value) -> Value {
    let started = Instant::now();
    let tool_name = req
        .get("params")
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("<unknown>")
        .to_string();

    match catch_unwind(AssertUnwindSafe(|| {
        handle_tool_call(engine, id.clone(), req)
    })) {
        Ok(resp) => resp,
        Err(payload) => panic_tool_response(id, &tool_name, started, payload),
    }
}

fn panic_tool_response(
    id: Value,
    tool_name: &str,
    started: Instant,
    payload: Box<dyn Any + Send>,
) -> Value {
    let msg = panic_payload_message(payload.as_ref());
    eprintln!("dunst-mcp: recovered panic in tools/call {tool_name}: {msg}");
    result_obj(
        id,
        add_timing_meta(
            json!({
                "content": [{
                    "type": "text",
                    "text": format!("tool call panicked in {tool_name}: {msg}")
                }],
                "isError": true
            }),
            tool_name,
            started,
        ),
    )
}

fn panic_payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(msg) = payload.downcast_ref::<&str>() {
        return (*msg).to_string();
    }
    if let Some(msg) = payload.downcast_ref::<String>() {
        return msg.clone();
    }
    "<non-string panic payload>".to_string()
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "dunst",
            "version": server_version_label()
        },
        "_meta": {
            "dunst": build_info()
        }
    })
}

fn build_git_sha() -> &'static str {
    BUILD_GIT_SHA.unwrap_or("unknown")
}

fn build_git_dirty() -> &'static str {
    BUILD_GIT_DIRTY.unwrap_or("unknown")
}

fn build_time_unix() -> &'static str {
    BUILD_TIME_UNIX.unwrap_or("unknown")
}

fn server_version_label() -> String {
    format!(
        "{}+git.{}{}",
        SERVER_VERSION,
        build_git_sha(),
        if build_git_dirty() == "true" {
            ".dirty"
        } else {
            ""
        }
    )
}

fn build_info() -> Value {
    json!({
        "version": SERVER_VERSION,
        "version_label": server_version_label(),
        "git_sha": build_git_sha(),
        "git_dirty": build_git_dirty(),
        "build_time_unix": build_time_unix(),
        "protocol_version": PROTOCOL_VERSION,
    })
}

fn approval_tool_enabled() -> bool {
    std::env::var("DUNST_MCP_ENABLE_APPROVE_TOOL")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn wait_for_element_value(
    engine: &mut Engine,
    query: &str,
    visible_only: bool,
    absent: bool,
    timeout_ms: u64,
    interval_ms: u64,
) -> Result<Value, String> {
    let timeout = Duration::from_millis(timeout_ms.clamp(100, 30_000));
    let interval = Duration::from_millis(interval_ms.clamp(50, 2_000));
    let started = Instant::now();

    loop {
        engine.refresh_if_stale().map_err(|e| e.to_string())?;
        let matches = engine.find_element_filtered(query, visible_only);
        let condition_met = if absent {
            matches.is_empty()
        } else {
            !matches.is_empty()
        };
        let matches_value = serde_json::to_value(matches.into_iter().take(10).collect::<Vec<_>>())
            .unwrap_or(Value::Null);
        let elapsed_ms = started.elapsed().as_millis() as u64;

        let timed_out = !condition_met && started.elapsed() >= timeout;
        if condition_met || timed_out {
            let found = !matches_value
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(true);
            let status = if timed_out {
                "timeout"
            } else if absent {
                "absent"
            } else {
                "found"
            };
            return Ok(json!({
                "status": status,
                "condition_met": condition_met,
                "timed_out": timed_out,
                "found": found,
                "query": query,
                "absent": absent,
                "elapsed_ms": elapsed_ms,
                "matches": matches_value,
            }));
        }

        std::thread::sleep(interval.min(timeout.saturating_sub(started.elapsed())));
    }
}

fn wait_for_text_stable_value(
    engine: &mut Engine,
    query: Option<&str>,
    visible_only: bool,
    timeout_ms: u64,
    stable_ms: u64,
    interval_ms: u64,
    limit: usize,
) -> Result<Value, String> {
    let timeout = Duration::from_millis(timeout_ms.clamp(500, 120_000));
    let stable_window = Duration::from_millis(stable_ms.clamp(250, 10_000));
    let interval = Duration::from_millis(interval_ms.clamp(100, 5_000));
    let limit = limit.clamp(1, 500);
    let started = Instant::now();
    let mut last_signature = String::new();
    let mut last_change = Instant::now();
    let mut first = true;

    loop {
        engine.refresh().map_err(|e| e.to_string())?;
        let snippets = engine.text_snapshot(query, visible_only, limit);
        let mut signature = String::new();
        for snippet in &snippets {
            signature.push_str(&snippet.id);
            signature.push('0');
            signature.push_str(&snippet.text);
            signature.push('\n');
        }

        if first || signature != last_signature {
            first = false;
            last_signature = signature;
            last_change = Instant::now();
        }

        let elapsed = started.elapsed();
        let stable_for = last_change.elapsed();
        let stable = stable_for >= stable_window;
        if stable || elapsed >= timeout {
            return Ok(json!({
                "stable": stable,
                "elapsed_ms": elapsed.as_millis() as u64,
                "stable_for_ms": stable_for.as_millis() as u64,
                "visible_only": visible_only,
                "query": query,
                "snippets": snippets,
            }));
        }

        std::thread::sleep(interval.min(timeout.saturating_sub(elapsed)));
    }
}

fn parse_points(value: &Value) -> Result<Vec<(f64, f64)>, String> {
    let arr = value
        .as_array()
        .ok_or_else(|| "read_series requires 'points': [[x,y], ...]".to_string())?;
    let mut pts = Vec::with_capacity(arr.len());
    for (idx, point) in arr.iter().enumerate() {
        let coords = point.as_array().ok_or_else(|| {
            format!("read_series point {idx} must be an array [x,y], got {point}")
        })?;
        if coords.len() != 2 {
            return Err(format!(
                "read_series point {idx} must contain exactly two numbers"
            ));
        }
        let x = coords[0]
            .as_f64()
            .ok_or_else(|| format!("read_series point {idx} x must be numeric"))?;
        let y = coords[1]
            .as_f64()
            .ok_or_else(|| format!("read_series point {idx} y must be numeric"))?;
        pts.push((x, y));
    }
    Ok(pts)
}

fn parse_window_ids(value: Option<&Value>) -> Result<Vec<u32>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let arr = value
        .as_array()
        .ok_or_else(|| "window_ids must be an array of integers".to_string())?;
    arr.iter()
        .enumerate()
        .map(|(idx, v)| {
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| format!("window_ids[{idx}] must be a u32 integer"))
        })
        .collect()
}

fn ensure_recent_graph(engine: &mut Engine, fresh: bool, force: bool) -> Result<(), String> {
    if force {
        engine
            .refresh_if_older_than(FORCE_REFRESH_COALESCE_TTL)
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else if fresh {
        engine
            .refresh_if_stale()
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else {
        Ok(())
    }
}

fn find_matches_value(matches: Vec<&SceneNode>) -> Value {
    serde_json::to_value(matches).unwrap_or(Value::Null)
}

fn find_element_value(
    engine: &mut Engine,
    query: &str,
    visible_only: bool,
    fresh: bool,
    force: bool,
) -> Result<Value, String> {
    if force && visible_only && engine.graph_recent(FIND_ELEMENT_FORCE_REFRESH_FAST_PATH_TTL) {
        let cached_matches = engine.find_element_filtered(query, visible_only);
        if !cached_matches.is_empty() {
            return Ok(find_matches_value(cached_matches));
        }
    }

    ensure_recent_graph(engine, fresh, force)?;
    Ok(find_matches_value(
        engine.find_element_filtered(query, visible_only),
    ))
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
mod tests;
