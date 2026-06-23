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
    process::Command,
    time::{Duration, Instant},
};

use serde_json::{json, Value};

mod coordination;
mod dispatch;
mod registry;
mod response;
mod tools;

use dispatch::handle_tool_call;
use response::{
    add_timing_meta, audit_entry_value, diff_summary_value, modal_dismiss_value, ocr_click_value,
    option_pick_value,
};
use tools::tools_list;

use crate::engine::{Engine, SceneView};
use dunst_core::{Bbox, SceneNode, SemanticAction, SessionIdentity};

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
    let mut session_identity = initial_session_identity();
    engine.set_session_identity(session_identity.clone());
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    eprintln!(
        "dunst-mcp: stdio MCP server ready (session {}, agent {}, parent {}, version {}, git {}, dirty {}, built {}, {} tools)",
        session_identity.session_id,
        session_identity.agent_id.as_deref().unwrap_or("-"),
        parent_process_label(&session_identity),
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
            "initialize" => {
                if let Some((client_name, client_version)) = client_info_from_initialize(&req) {
                    session_identity.client_name = client_name;
                    session_identity.client_version = client_version;
                    engine.set_session_identity(session_identity.clone());
                    eprintln!(
                        "dunst-mcp: initialized session {} client {} {} agent {}",
                        session_identity.session_id,
                        session_identity.client_name.as_deref().unwrap_or("-"),
                        session_identity.client_version.as_deref().unwrap_or("-"),
                        session_identity.agent_id.as_deref().unwrap_or("-")
                    );
                }
                send(
                    &mut out,
                    result_obj(id, initialize_result(&session_identity)),
                );
            }
            "notifications/initialized" => { /* notification: no reply */ }
            "ping" => send(
                &mut out,
                result_obj(
                    id,
                    json!({ "_meta": { "dunst": dunst_meta(&session_identity) } }),
                ),
            ),
            "tools/list" => send(
                &mut out,
                result_obj(
                    id,
                    json!({
                        "tools": tools_list(),
                        "_meta": { "dunst": dunst_meta(&session_identity) }
                    }),
                ),
            ),
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

    let session = engine.session_identity().cloned();
    log_tool_call(&tool_name, session.as_ref());
    match catch_unwind(AssertUnwindSafe(|| {
        handle_tool_call(engine, id.clone(), req)
    })) {
        Ok(resp) => resp,
        Err(payload) => panic_tool_response(id, &tool_name, started, session.as_ref(), payload),
    }
}

fn panic_tool_response(
    id: Value,
    tool_name: &str,
    started: Instant,
    session: Option<&SessionIdentity>,
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
            session,
            None,
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

fn initialize_result(session: &SessionIdentity) -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "dunst",
            "version": server_version_label()
        },
        "_meta": {
            "dunst": dunst_meta(session)
        }
    })
}

fn dunst_meta(session: &SessionIdentity) -> Value {
    let mut dunst = build_info();
    if let Value::Object(obj) = &mut dunst {
        obj.insert("session".into(), json!(session));
    }
    dunst
}

fn initial_session_identity() -> SessionIdentity {
    let (parent_pid, parent_process) = parent_process_info();
    SessionIdentity {
        session_id: format!("dunst-{}-{}", std::process::id(), dunst_core::now_ms()),
        client_name: None,
        client_version: None,
        agent_id: env_non_empty("DUNST_MCP_AGENT_ID"),
        parent_pid,
        parent_process,
    }
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    })
}

fn parent_process_info() -> (Option<u32>, Option<String>) {
    let pid = std::process::id().to_string();
    let parent_pid = Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid])
        .output()
        .ok()
        .and_then(|output| output.status.success().then_some(output.stdout))
        .and_then(|stdout| String::from_utf8(stdout).ok())
        .and_then(|text| text.trim().parse::<u32>().ok());

    let parent_process = parent_pid
        .and_then(|ppid| {
            let ppid_arg = ppid.to_string();
            Command::new("ps")
                .args(["-o", "comm=", "-p", &ppid_arg])
                .output()
                .ok()
                .and_then(|output| output.status.success().then_some(output.stdout))
                .and_then(|stdout| String::from_utf8(stdout).ok())
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
                .map(|process| (ppid, process))
        })
        .map(|(ppid, process)| (Some(ppid), Some(process)))
        .unwrap_or((parent_pid, None));

    parent_process
}

fn parent_process_label(identity: &SessionIdentity) -> String {
    match (identity.parent_pid, identity.parent_process.as_deref()) {
        (Some(pid), Some(process)) => format!("{pid}:{process}"),
        (Some(pid), None) => pid.to_string(),
        _ => "-".into(),
    }
}

fn log_tool_call(tool_name: &str, session: Option<&SessionIdentity>) {
    match session {
        Some(identity) => eprintln!(
            "dunst-mcp: tools/call session {} client {} {} agent {} tool {}",
            identity.session_id,
            identity.client_name.as_deref().unwrap_or("-"),
            identity.client_version.as_deref().unwrap_or("-"),
            identity.agent_id.as_deref().unwrap_or("-"),
            tool_name
        ),
        None => eprintln!("dunst-mcp: tools/call session - client - - agent - tool {tool_name}"),
    }
}

fn client_info_from_initialize(req: &Value) -> Option<(Option<String>, Option<String>)> {
    let client_info = req.get("params")?.get("clientInfo")?;
    let name = client_info
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let version = client_info
        .get("version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    (name.is_some() || version.is_some()).then_some((name, version))
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
            let empty = snippets.is_empty();
            let diagnostic = if empty && visible_only {
                Some("no visible AX text snippets matched; the target may be browser chrome, canvas-rendered, stale, or hidden from accessibility")
            } else if empty {
                Some("no AX text snippets matched; the target may be canvas-rendered, stale, or hidden from accessibility")
            } else {
                None
            };
            return Ok(json!({
                "stable": stable,
                "elapsed_ms": elapsed.as_millis() as u64,
                "stable_for_ms": stable_for.as_millis() as u64,
                "visible_only": visible_only,
                "query": query,
                "empty": empty,
                "diagnostic": diagnostic,
                "fallback_hint": empty.then_some("use window_view/list_browser_tabs to confirm target scope, then read_text or screenshot for pixel/OCR verification"),
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
    if tool_accepts_mutation_preconditions(name) {
        add_mutation_preconditions(&mut input_schema);
    }
    json!({ "name": name, "description": description, "inputSchema": input_schema })
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

fn add_mutation_preconditions(input_schema: &mut Value) {
    let Some(properties) = input_schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    properties.entry("expected_epoch").or_insert_with(|| {
        json!({
            "type": "string",
            "description": "optional ui_epoch.fingerprint from get_hit_targets; mutating tools refuse if the current UI epoch differs"
        })
    });
    properties.entry("fencing_token").or_insert_with(|| {
        json!({
            "type": "string",
            "description": "optional active window-lease token from _meta.dunst.coordination.mutation.fencing_token; stale tokens are refused"
        })
    });
}

fn tool_accepts_mutation_preconditions(name: &str) -> bool {
    matches!(
        name,
        "click_element"
            | "raise_element"
            | "pick_option"
            | "type_into"
            | "hover_probe"
            | "drag_element"
            | "select_file"
            | "click_at"
            | "click_near_text"
            | "dismiss_modal"
            | "reveal_hover_click"
            | "read_at"
            | "read_series"
            | "scan_chart"
            | "focus_window"
            | "right_click_at"
            | "double_click_at"
            | "open_menu"
            | "press_key"
            | "type_keys"
            | "paste_text"
            | "scroll"
            | "scroll_at"
            | "zoom"
            | "hotkey"
            | "move_window_to_display"
            | "move_app_to_display"
            | "arrange_windows"
            | "expose_target_window"
            | "launch_app"
            | "open_url_and_attach_tab"
            | "close_app"
    )
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
