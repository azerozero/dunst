//! Minimal MCP server over stdio (newline-delimited JSON-RPC 2.0).
//!
//! Dependency-light by design — the POC implements just the slice of MCP a host
//! needs: `initialize`, `tools/list`, `tools/call`, `ping`. Each tool maps onto
//! an [`Engine`] method, so the same risk-gating + audit applies whether the
//! engine is driven from the CLI demo or a real MCP client.

use std::{
    io::{self, BufRead, Write},
    time::{Duration, Instant},
};

use serde_json::{json, Value};

use crate::engine::{Engine, OptionPickResult, SceneView};
use dunst_core::{
    ActionResult, AuditEntry, Bbox, GraphDiff, NodeChange, SceneNode, SemanticAction,
};

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
                let resp = handle_tool_call(&mut engine, id, &req);
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

fn tools_list() -> Vec<Value> {
    let mut tools = vec![
        tool(
            "version",
            "Return the running dunst-mcp build identity: package version, git commit, dirty flag, build timestamp, and protocol version. Use this after restart to confirm the active server binary.",
            json!({}),
        ),
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
            "page_state",
            "Return a lightweight orientation snapshot: target app/window, title, likely URL, visible text snippets, and key visible elements. Ensures a recent AX graph by default; force_refresh requests a refresh but coalesces bursts inside a short TTL.",
            schema(
                json!({
                    "fresh": { "type": "boolean", "description": "ensure recent graph before reading state (default true, uses short TTL)" },
                    "force_refresh": { "type": "boolean", "description": "force an AX refresh even if the short TTL is still valid (default false)" },
                    "limit": { "type": "integer", "description": "max visible text/key elements, 1-50 (default 12)" }
                }),
                &[],
            ),
        ),
        tool(
            "text_snapshot",
            "Return AX text snippets without the full scene graph or OCR. Use this to extract visible LLM/chat/document text, or set visible_only=false plus query to inspect off-screen AX text. Ensures a recent graph by default.",
            schema(
                json!({
                    "query": { "type": "string", "description": "case-insensitive filter over id/role/text" },
                    "visible_only": { "type": "boolean", "description": "only snippets visible in the target window (default true)" },
                    "fresh": { "type": "boolean", "description": "ensure recent graph before reading text (default true, uses short TTL)" },
                    "force_refresh": { "type": "boolean", "description": "force an AX refresh even if the short TTL is still valid (default false)" },
                    "limit": { "type": "integer", "description": "max snippets, 1-500 (default 120)" }
                }),
                &[],
            ),
        ),
        tool(
            "wait_for_text_stable",
            "Wait until AX text snippets stop changing for a stable interval. Use after submitting long ChatGPT/Claude prompts instead of repeatedly polling page_state. Returns the final snippets used for the stability check.",
            schema(
                json!({
                    "query": { "type": "string", "description": "optional case-insensitive filter over id/role/text" },
                    "visible_only": { "type": "boolean", "description": "only visible snippets (default true)" },
                    "timeout_ms": { "type": "integer", "description": "maximum wait, clamped 500-120000 ms (default 30000)" },
                    "stable_ms": { "type": "integer", "description": "required unchanged duration, clamped 250-10000 ms (default 1200)" },
                    "interval_ms": { "type": "integer", "description": "poll interval, clamped 100-5000 ms (default 500)" },
                    "limit": { "type": "integer", "description": "max snippets in each snapshot, 1-500 (default 120)" }
                }),
                &[],
            ),
        ),
        tool(
            "list_browser_tabs",
            "List browser tabs exposed by the target window tab strip. Use this before clicking browser tabs; it avoids confusing page/sidebar items named like a tab. Returns visible AXRadioButton tabs with id, title, selected, url if title itself is a URL, and bbox. query filters title/id; visible_only defaults true.",
            schema(
                json!({
                    "query": { "type": "string", "description": "case-insensitive filter over tab title/id" },
                    "visible_only": { "type": "boolean", "description": "only tabs visible in the tab strip (default true)" },
                    "fresh": { "type": "boolean", "description": "ensure recent graph before reading tabs (default true, uses short TTL)" },
                    "force_refresh": { "type": "boolean", "description": "force an AX refresh even if the short TTL is still valid (default false)" }
                }),
                &[],
            ),
        ),
        tool(
            "list_displays",
            "List active displays/screens: Dunst 1-based index, CoreGraphics display_id, global bounds in screen points, native pixel resolution, scale, and main-display flag. Index 1 is the main display; others follow arrangement order.",
            json!({}),
        ),
        tool(
            "window_view",
            "Enter a compact scoped view of the target window: target app/window, owning display, window bounds, window position relative to that display, visible text, and key elements. Ensures a recent AX graph by default; avoids returning the full AX graph.",
            schema(
                json!({
                    "fresh": { "type": "boolean", "description": "ensure recent graph before reading scoped view (default true, uses short TTL)" },
                    "force_refresh": { "type": "boolean", "description": "force an AX refresh even if the short TTL is still valid (default false)" },
                    "limit": { "type": "integer", "description": "max visible text/key elements, 1-50 (default 12)" }
                }),
                &[],
            ),
        ),
        tool(
            "desktop_view",
            "Return the desktop/window topology: displays, top-level windows, front/back z_order, frontmost window, owning display, and geometric overlap lists. all:false drops fragments/shadows/off-size windows.",
            schema(json!({ "all": { "type": "boolean", "description": "include every layer-0 window incl. fragments (default false)" } }), &[]),
        ),
        tool(
            "visual_change_probe",
            "Sample a spaced luminance pixel grid over a screen region and compare it with the previous probe for the same region/grid. Can trigger a full AX refresh when pixels changed; AX cannot refresh only a rectangle.",
            schema(
                json!({
                    "region": {
                        "type": "object",
                        "description": "optional screen-point region; omit for the target window",
                        "properties": {
                            "x": { "type": "number" },
                            "y": { "type": "number" },
                            "w": { "type": "number" },
                            "h": { "type": "number" }
                        },
                        "required": ["x", "y", "w", "h"]
                    },
                    "columns": { "type": "integer", "description": "sample columns, clamped 2-128 (default 16)" },
                    "rows": { "type": "integer", "description": "sample rows, clamped 2-128 (default 12)" },
                    "threshold": { "type": "integer", "description": "per-cell luma delta threshold, 0-255 (default 12)" },
                    "refresh_on_change": { "type": "boolean", "description": "run a full AX refresh if changed (default false)" }
                }),
                &[],
            ),
        ),
        tool(
            "analyze_region_ax",
            "Analyze only a screen region through AX hit-tests on a spaced grid. Returns unique shallow AX elements under the sampled points; this is targeted region analysis, not a full AX subtree refresh.",
            schema(
                json!({
                    "region": {
                        "type": "object",
                        "description": "optional screen-point region; omit for the target window",
                        "properties": {
                            "x": { "type": "number" },
                            "y": { "type": "number" },
                            "w": { "type": "number" },
                            "h": { "type": "number" }
                        },
                        "required": ["x", "y", "w", "h"]
                    },
                    "columns": { "type": "integer", "description": "sample columns, clamped 1-64 (default 8)" },
                    "rows": { "type": "integer", "description": "sample rows, clamped 1-64 (default 6)" }
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
            "Find elements whose id/label/role contains the query (case-insensitive). Ensures a recent AX graph by default. Results are ranked with visible enabled targets first; visible_only drops off-window/latent noise.",
            schema(
                json!({
                    "query": { "type": "string" },
                    "fresh": { "type": "boolean", "description": "ensure recent graph before searching (default true, uses short TTL)" },
                    "force_refresh": { "type": "boolean", "description": "force an AX refresh even if the short TTL is still valid (default false)" },
                    "visible_only": { "type": "boolean", "description": "drop latent/off-window matches (default false)" }
                }),
                &["query"],
            ),
        ),
        tool(
            "wait_for_element",
            "Wait for an element matching query to appear or disappear, polling the AX graph until timeout. Use after submit/navigation actions to verify the UI actually changed, for example wait for Interrompre la reponse after sending ChatGPT or Claude prompts.",
            schema(
                json!({
                    "query": { "type": "string", "description": "case-insensitive id/label/role query" },
                    "visible_only": { "type": "boolean", "description": "drop latent/off-window matches (default true)" },
                    "absent": { "type": "boolean", "description": "wait for no matches instead of at least one match (default false)" },
                    "timeout_ms": { "type": "integer", "description": "maximum wait, clamped 100-30000 ms (default 5000)" },
                    "interval_ms": { "type": "integer", "description": "poll interval, clamped 50-2000 ms (default 250)" }
                }),
                &["query"],
            ),
        ),
        tool(
            "read_text",
            "OCR the target window (or an optional screen-point region x,y,w,h) via Apple Vision; returns recognised text lines with screen bbox + confidence. accurate:true uses the slower, more precise recognition (default fast).",
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
                    },
                    "accurate": { "type": "boolean", "description": "slower, higher-accuracy OCR (default false)" }
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
            "Click an element by id. High-risk elements return pending_approval until approve() is called. Action responses are compact by default; set include_diff=true for the full scene diff.",
            schema(json!({ "id": {"type":"string"}, "reasoning": {"type":"string"}, "include_diff": {"type":"boolean"} }), &["id"]),
        ),
        tool(
            "raise_element",
            "Raise an element by id when its affordance exposes the semantic raise action, typically a window root such as win_collective. Use this instead of click_element when click_element reports Click is unavailable on a window.",
            schema(json!({ "id": {"type":"string"}, "reasoning": {"type":"string"}, "include_diff": {"type":"boolean"} }), &["id"]),
        ),
        tool(
            "pick_option",
            "Pick a popover/list/radio option by visible text. Resolves static option text to the nearest clickable parent, then reports best-effort selected/closed state. High-risk targets still gate like click_element.",
            schema(
                json!({
                    "query": { "type": "string", "description": "option text to find" },
                    "visible_only": { "type": "boolean", "description": "drop latent/off-window matches (default true)" },
                    "reasoning": { "type": "string" },
                    "include_diff": { "type": "boolean" }
                }),
                &["query"],
            ),
        ),
        tool(
            "type_into",
            "Replace text in a text element by id (subject to risk gating). On macOS this uses an element-bound selected-text range plus Unicode keystrokes when possible, so React/web fields receive real input events without layout-sensitive Cmd+A.",
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
            "Click at a raw screen point (x,y) inside the target window. For OCR-driven navigation: read_text a link, then click_at its bbox centre. Raw mutating input is high-risk and requires approval. If pending_approval is not explicitly approved, switch to ui_fallback_hint: map the UI with window_view/get_affordances/find_element, then use click_element/type_into/scroll by id. Off-target points are rejected unless DUNST_MCP_ALLOW_OFF_TARGET_RAW=1. If the user-active guard blocks it, wait until the operator is idle and retry once.",
            schema(json!({ "x": {"type":"number"}, "y": {"type":"number"} }), &["x", "y"]),
        ),
        tool(
            "select_file",
            "Select a local file in a native macOS file chooser for browser upload controls. Provide path plus either trigger_id (an existing upload/dropzone/link element to real-click first) or x/y (screen point to real-click first); omit trigger_id/x/y only when the file chooser is already open. High-risk: uses System Events to click inside the target window and drive the chooser. Off-target trigger points are rejected unless DUNST_MCP_ALLOW_OFF_TARGET_RAW=1.",
            schema(
                json!({
                    "path": { "type": "string", "description": "absolute or working-directory-relative local file path to select" },
                    "trigger_id": { "type": "string", "description": "optional element id to click before selecting the file" },
                    "x": { "type": "number", "description": "optional screen x to click before selecting the file" },
                    "y": { "type": "number", "description": "optional screen y to click before selecting the file" },
                    "reasoning": { "type": "string" },
                    "include_diff": { "type": "boolean" }
                }),
                &["path"],
            ),
        ),
        tool(
            "hover_at",
            "Hover (background mouse-move, no cursor movement) at a raw screen point (x,y) inside the target window so the target reveals a hover state — e.g. a chart crosshair tooltip / value-at-cursor — then read_text it. A probe: no gating, no audit. Off-target points are rejected unless DUNST_MCP_ALLOW_OFF_TARGET_RAW=1.",
            schema(json!({ "x": {"type":"number"}, "y": {"type":"number"} }), &["x", "y"]),
        ),
        tool(
            "read_at",
            "Read the value at a screen point inside the target window. Defaults to background hover without borrowing the OS cursor; set borrow_cursor=true only when a surface requires a real cursor hover. Off-target points are rejected unless DUNST_MCP_ALLOW_OFF_TARGET_RAW=1.",
            schema(json!({ "x": {"type":"number"}, "y": {"type":"number"}, "borrow_cursor": {"type":"boolean","description":"briefly borrow and restore the OS cursor for real-hover-only surfaces (default false)"} }), &["x", "y"]),
        ),
        tool(
            "read_series",
            "Read values at SEVERAL screen points inside the target window. Defaults to background hover without borrowing the OS cursor; set borrow_cursor=true to borrow once, warp+OCR each point, then restore. points = [[x,y], ...]; returns one OCR list per point. Off-target points are rejected unless DUNST_MCP_ALLOW_OFF_TARGET_RAW=1.",
            schema(
                json!({ "points": { "type": "array", "items": { "type": "array", "items": { "type": "number" } } }, "borrow_cursor": {"type":"boolean","description":"borrow and restore the OS cursor for real-hover-only surfaces (default false)"} }),
                &["points"],
            ),
        ),
        tool(
            "scan_chart",
            "Detect → confirm rendered → traverse → series. Coarse-to-fine CV first answers whether a chart is actually rendered (not a blank plot) and where it sits; only if present does it traverse the plot at mid-height and read the value-at-cursor across it. Returns {present, focused, fill_ratio, region, samples:[{x,value,time,raw}]}. Activates the window without raising it first so a backgrounded web canvas paints.",
            schema(json!({ "samples": { "type": "integer", "description": "points across the width (2-12, default 5)" } }), &[]),
        ),
        tool(
            "focus_window",
            "Make the target window AppKit-active WITHOUT raising it or switching Spaces (SkyLight focus-without-raise) so a backgrounded web canvas (e.g. a chart) paints, without foregrounding. Returns true if the SkyLight SPIs applied.",
            json!({}),
        ),
        tool(
            "list_windows",
            "Enumerate REAL, drivable windows (sizeable + titled; tab-strip/shadow/menubar fragments dropped) — window_id, pid, app, title, bounds, on_screen — to pick a target. Pass all:true for every layer-0 window. The daemon's own discovery (no external tool).",
            schema(json!({ "all": { "type": "boolean", "description": "include every layer-0 window incl. fragments (default false)" } }), &[]),
        ),
        tool(
            "move_window_to_display",
            "Move the target window to a display from list_displays. Centers it on that display; preserve_size:true keeps the current size but clamps it inside the display, false fits inside the display with padding.",
            schema(
                json!({
                    "display": { "type": "integer", "description": "Dunst 1-based display index from list_displays" },
                    "preserve_size": { "type": "boolean", "description": "keep current size when possible (default true)" }
                }),
                &["display"],
            ),
        ),
        tool(
            "move_app_to_display",
            "Move all sizeable top-level windows for an app to a display from list_displays. Windows are centered with a small cascade offset; preserve_size:true keeps each current size when possible.",
            schema(
                json!({
                    "app": { "type": "string", "description": "running app name or substring, e.g. Firefox" },
                    "display": { "type": "integer", "description": "Dunst 1-based display index from list_displays" },
                    "preserve_size": { "type": "boolean", "description": "keep current window sizes when possible (default true)" }
                }),
                &["app", "display"],
            ),
        ),
        tool(
            "arrange_windows",
            "Reorganize selected windows on one display. Selection must be explicit via window_ids, app, or all:true. mode: grid | columns/side_by_side | rows | cascade | maximize.",
            schema(
                json!({
                    "display": { "type": "integer", "description": "Dunst 1-based display index from list_displays" },
                    "mode": { "type": "string", "enum": ["grid", "columns", "side_by_side", "rows", "cascade", "maximize"], "description": "layout mode (default grid)" },
                    "window_ids": { "type": "array", "items": { "type": "integer" }, "description": "specific window ids to arrange" },
                    "app": { "type": "string", "description": "running app name or substring" },
                    "all": { "type": "boolean", "description": "arrange all sizeable titled windows (default false)" }
                }),
                &["display"],
            ),
        ),
        tool(
            "list_apps",
            "List running GUI apps (those owning a window) — app, pid, windows (count), on_screen — coarser discovery than list_windows: which app to launch_app/attach, and whether it is already running. Optional query filters by case-insensitive name substring (doubles as \"search app\"). Sorted by window count.",
            schema(json!({ "query": { "type": "string", "description": "case-insensitive app-name substring filter (optional)" } }), &[]),
        ),
        tool(
            "list_launchable_apps",
            "List installed macOS .app bundles without launching them. Scans /Applications, /System/Applications, Utilities folders, and ~/Applications; reads Contents/Info.plist for name, bundle id, version, category, description, path, executable, and running status.",
            schema(
                json!({
                    "query": { "type": "string", "description": "case-insensitive filter over name/display name/bundle id" },
                    "limit": { "type": "integer", "description": "max results, 1-500 (default 80)" }
                }),
                &[],
            ),
        ),
        tool(
            "app_info",
            "Read one installed app's Info.plist metadata before launching it. Resolve by app display/name, bundle_id, or exact .app path.",
            schema(
                json!({
                    "app": { "type": "string", "description": "display name or bundle filename, e.g. Firefox" },
                    "bundle_id": { "type": "string", "description": "bundle identifier, e.g. org.mozilla.firefox" },
                    "path": { "type": "string", "description": "exact .app bundle path" }
                }),
                &[],
            ),
        ),
        tool(
            "attach",
            "Re-target the daemon to a window_id (from list_windows) at runtime — dynamic targeting, no fixed/hardcoded target. Re-perceives and returns the new target + scene summary.",
            schema(json!({ "window_id": { "type": "integer" } }), &["window_id"]),
        ),
        tool(
            "launch_app",
            "Launch an app WITHOUT bringing it to the foreground (open -g), optionally opening a url in it. Then list_windows + attach to drive it. Closes the last external dependency — full autonomy via the MCP alone. args: extra argv passed to the app (only applies when this call actually launches it). To read a Chromium chart in pure background, launch with args [\"--disable-features=CalculateNativeWinOcclusion\",\"--disable-renderer-backgrounding\",\"--disable-background-timer-throttling\",\"--disable-backgrounding-occluded-windows\"] so the <canvas> keeps painting while backgrounded (otherwise scan_chart sees a blank plot).",
            schema(json!({ "app": {"type":"string"}, "url": {"type":"string"}, "args": {"type":"array","items":{"type":"string"},"description":"extra argv for the app (e.g. Chromium background-paint flags)"} }), &["app"]),
        ),
        tool(
            "close_app",
            "Quit an app gracefully by name (no foreground).",
            schema(json!({ "app": {"type":"string"} }), &["app"]),
        ),
        tool(
            "screenshot",
            "Composited PNG of the target window, returned as an image — lets you SEE the pixels directly (multimodal) alongside OCR/CV. Works backgrounded.",
            json!({}),
        ),
        tool(
            "right_click_at",
            "Right-click at a raw screen point (x,y) — context menus. Background web via SkyLight (no cursor, no foreground). Raw input is high-risk; if approval is unavailable or denied, switch to ui_fallback_hint and prefer element-bound actions. If the user-active guard blocks it, wait until the operator is idle and retry once.",
            schema(json!({ "x": {"type":"number"}, "y": {"type":"number"} }), &["x", "y"]),
        ),
        tool(
            "double_click_at",
            "Double-click at a raw screen point (x,y). Background web via SkyLight. Raw input is high-risk; if approval is unavailable or denied, switch to ui_fallback_hint and prefer element-bound actions. If the user-active guard blocks it, wait until the operator is idle and retry once.",
            schema(json!({ "x": {"type":"number"}, "y": {"type":"number"} }), &["x", "y"]),
        ),
        tool(
            "open_menu",
            "Open an app menu-bar menu by name via AX (AXPress on the target app's own AXMenuBarItem); its items then appear in the scene graph. Works even when the target is NOT the frontmost app — the menu bar is exposed per-application via AX, so a backgrounded target's menu opens without raising it (verified live on backgrounded Chrome: \"Fichier\" opens with iTerm frontmost). Match the app's ACTUAL localized menu title — e.g. Chrome's View menu is \"Présentation\", not \"Affichage\" — so read the captured AXMenuBarItem labels first (get_scene_graph / find_element) instead of guessing.",
            schema(json!({ "name": {"type":"string"} }), &["name"]),
        ),
        tool(
            "press_key",
            "Press a named key on the target (e.g. \"Return\"/\"Enter\", \"Tab\", \"Escape\", arrows, \"Home\", \"End\", \"PageUp\", \"PageDown\"). Raw mutating keyboard input is high-risk: approve only after explicit operator authorization. If approval is unavailable or denied, switch to ui_fallback_hint and drive the UI through mapped element ids instead. If the user-active guard blocks it, wait until the operator is idle and retry once.",
            schema(json!({ "key": {"type":"string"} }), &["key"]),
        ),
        tool(
            "type_keys",
            "Type text into the FOCUSED element via the SkyLight auth-signed keyboard path — reaches a backgrounded/occluded window's web content. Focus the field first (prefer click_element on a field; click_at is raw). Raw mutating keyboard input is high-risk and requires approval. If approval is unavailable or denied, switch to ui_fallback_hint and use type_into on a mapped field id. If the user-active guard blocks it, wait until the operator is idle and retry once.",
            schema(json!({ "text": {"type":"string"} }), &["text"]),
        ),
        tool(
            "scroll",
            "Scroll the focused page/container. With id, uses direct AX scrollbar value changes on that element or an ancestor exposing AXVerticalScrollBar. Without id, falls back to background Page/Home/End keys. direction: down|up|top|bottom; pages: number of pages (default 3). Action responses are compact by default; set include_diff=true for the full scene diff.",
            schema(json!({ "direction": {"type":"string","enum":["down","up","top","bottom"]}, "pages": {"type":"integer"}, "id": {"type":"string","description":"optional scrollable element id; requires an AXVerticalScrollBar on the element or an ancestor"}, "include_diff": {"type":"boolean"} }), &[]),
        ),
        tool(
            "zoom",
            "Zoom the focused page in the background (Cmd =/-/0, auth-signed, reaches web). direction: in|out|reset.",
            schema(json!({ "direction": {"type":"string","enum":["in","out","reset"]} }), &[]),
        ),
        tool(
            "hotkey",
            "Send a keyboard shortcut in the background (auth-signed, reaches web): modifiers cmd|shift|opt|ctrl + a key, '+'-separated. E.g. \"cmd+l\", \"cmd+t\", \"cmd+w\". This is raw keyboard input: approve only after explicit operator authorization. If approval is unavailable or denied, do not route through browser chrome or javascript:; switch to ui_fallback_hint and use mapped element actions. Layout-sensitive text-selection shortcuts such as \"cmd+a\" are rejected; use type_into for field replacement.",
            schema(json!({ "combo": {"type":"string"} }), &["combo"]),
        ),
        tool(
            "verify_state",
            "Assert an element's field (label|value|enabled|focused) equals an expected value.",
            schema(json!({ "id": {"type":"string"}, "field": {"type":"string"}, "expected": {"type":"string"} }), &["id", "field", "expected"]),
        ),
        tool(
            "diff_since",
            "Structural diff between the previous and current scene graph. Use summary=true for a compact count/sample response.",
            schema(json!({ "summary": {"type":"boolean"}, "limit": {"type":"integer"} }), &[]),
        ),
        tool("export_trace", "Export the audit trail (every attempted action) as JSON.", json!({})),
    ];
    if approval_tool_enabled() {
        tools.push(tool(
            "approve",
            "Operator-side escape hatch: approve a gated element or raw target so the next action on it proceeds. Disabled by default; set DUNST_MCP_ENABLE_APPROVE_TOOL=1 for controlled local sessions.",
            schema(json!({ "id": {"type":"string"} }), &["id"]),
        ));
    }
    tools
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

/// Dispatch a `tools/call`. Returns a full JSON-RPC response object.
fn handle_tool_call(engine: &mut Engine, id: Value, req: &Value) -> Value {
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

fn add_timing_meta(mut result: Value, tool: &str, started: Instant) -> Value {
    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
    if let Value::Object(obj) = &mut result {
        obj.insert(
            "_meta".into(),
            json!({
                "dunst": {
                    "tool": tool,
                    "timing_ms": elapsed_ms,
                    "version": SERVER_VERSION,
                    "version_label": server_version_label(),
                    "git_sha": build_git_sha(),
                    "git_dirty": build_git_dirty(),
                    "build_time_unix": build_time_unix()
                }
            }),
        );
    }
    result
}

fn audit_entry_value(entry: AuditEntry, include_diff: bool) -> Value {
    let mut summary = diff_summary_value(&entry.graph_diff, 12);
    if typed_content_observation_relevant(&entry) {
        let observed = typed_content_change_observed(&entry);
        let exact = typed_content_exact_match(&entry);
        if let Value::Object(obj) = &mut summary {
            obj.insert("typed_content_change_observed".into(), json!(observed));
            obj.insert("typed_content_exact_match".into(), json!(exact));
        }
    }
    let mut value = serde_json::to_value(&entry).unwrap_or(Value::Null);
    if let Value::Object(obj) = &mut value {
        if !include_diff {
            obj.remove("graph_diff");
        }
        obj.insert("graph_diff_summary".into(), summary);
        if entry.result == ActionResult::PendingApproval {
            let raw_target = raw_input_target(&entry.target_id);
            obj.insert(
                "approval_hint".into(),
                json!({
                    "next_step": if raw_target {
                        "Use approve only after explicit operator authorization for this raw input. Otherwise switch to ui_fallback_hint and drive visible elements by id."
                    } else {
                        "If this element-bound action was intended and the approve tool is available, call approve with this target_id, then retry the exact same tool call once."
                    },
                    "approve_tool": "approve",
                    "approve_arguments": { "id": entry.target_id },
                    "retry_required": true
                }),
            );
            if raw_target {
                obj.insert("ui_fallback_hint".into(), raw_input_fallback_hint(&entry));
            }
        }
        if entry.result == ActionResult::Failed {
            if let Some(hint) = failed_action_hint(&entry) {
                obj.insert("failure_hint".into(), hint);
            }
        }
    }
    value
}

fn raw_input_target(target_id: &str) -> bool {
    target_id.starts_with("keyboard@") || target_id.starts_with("screen@")
}

fn raw_input_fallback_hint(entry: &AuditEntry) -> Value {
    json!({
        "mode": "ui_mapping",
        "why": "Raw keyboard/pointer input is not bound to a scene element and may affect the wrong UI surface.",
        "goal": "Map the visible UI, choose element-bound actions, verify state after each mutation, then save only after the target fields expose the intended values.",
        "recommended_sequence": [
            { "tool": "window_view", "purpose": "confirm target window, visible text, current field values, and key controls" },
            { "tool": "get_affordances", "arguments": { "include_latent": false }, "purpose": "list clickable/typeable/scrollable element ids and their risk" },
            { "tool": "find_element", "purpose": "resolve a visible label to a stable element id before acting" },
            { "tool": "click_element/type_into/pick_option/scroll", "purpose": "act only through element ids or scrollable containers" },
            { "tool": "window_view/text_snapshot/verify_state/diff_since", "purpose": "verify the UI changed as intended before the next action" }
        ],
        "avoid": [
            "do not use browser address-bar javascript: injection as a fallback",
            "do not retry raw hotkeys or raw clicks after approval is denied",
            "do not click save/submit until visible state confirms the intended values"
        ],
        "blocked_action": {
            "target_id": entry.target_id,
            "action": entry.action,
            "argument": entry.argument
        }
    })
}

fn typed_content_observation_relevant(entry: &AuditEntry) -> bool {
    entry.action == SemanticAction::Type
        && entry.argument.as_deref().is_some_and(|arg| !arg.is_empty())
        && !entry.target_id.starts_with("keyboard@")
}

fn typed_content_change_observed(entry: &AuditEntry) -> bool {
    entry.graph_diff.changes.iter().any(|change| {
        matches!(
            change,
            NodeChange::Changed { id, field, .. }
                if id == &entry.target_id && matches!(field.as_str(), "value" | "label")
        )
    })
}

fn typed_content_exact_match(entry: &AuditEntry) -> bool {
    let Some(expected) = entry.argument.as_deref() else {
        return false;
    };
    entry.graph_diff.changes.iter().any(|change| {
        matches!(
            change,
            NodeChange::Changed { id, field, after, .. }
                if id == &entry.target_id && matches!(field.as_str(), "value" | "label") && after == expected
        )
    })
}

fn failed_action_hint(entry: &AuditEntry) -> Option<Value> {
    match entry.action {
        SemanticAction::Type if !entry.target_id.starts_with("keyboard@") => Some(json!({
            "reason": "The element-bound type action completed at the platform layer, but the target element did not expose the exact requested value afterward.",
            "next_step": "Do not click save/submit. Re-read the field with find_element or text_snapshot. If the value is partial/truncated/unchanged, use an explicit operator-approved paste path or a product/API-level edit path.",
            "verification": "graph_diff_summary.typed_content_exact_match must be true before saving"
        })),
        SemanticAction::OpenMenu => Some(json!({
            "reason": "The requested menu did not expose usable menu items after the AX open-menu attempt.",
            "next_step": "Check desktop_view/window_view for whether another window of the same app is frontmost. If the target window root exposes raise, use raise_element on that window id before retrying."
        })),
        SemanticAction::Click if entry.target_id.starts_with("menubar_") => Some(json!({
            "reason": "The menu-bar item was visible in AX but pressing it did not open the menu.",
            "next_step": "Use open_menu with the exact localized label, or raise the target window first if another window of the same app is frontmost."
        })),
        SemanticAction::Click
            if entry.target_id.starts_with("field_") || entry.target_id.starts_with("text_") =>
        {
            Some(json!({
                "reason": "The text-field click did not produce a verified focus/caret placement.",
                "next_step": "Do not type yet. Re-read the field with find_element or verify_state focused=true. If it is still false, raise/focus the target window and retry the element click; use raw click only after explicit operator authorization.",
                "verification": "verify_state(field='focused', expected='true') should pass before typing"
            }))
        }
        _ => None,
    }
}

fn option_pick_value(result: OptionPickResult, include_diff: bool) -> Value {
    let audit = audit_entry_value(result.audit, include_diff);
    json!({
        "query": result.query,
        "matched_id": result.matched_id,
        "action_id": result.action_id,
        "action_role": result.action_role,
        "action": result.action,
        "selected_before": result.selected_before,
        "selected_after": result.selected_after,
        "closed_after": result.closed_after,
        "audit": audit,
    })
}

fn diff_summary_value(diff: &GraphDiff, limit: usize) -> Value {
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut changed = 0usize;
    let mut low_signal_suppressed = 0usize;
    let mut fields = serde_json::Map::new();

    for change in &diff.changes {
        if low_signal_diff_change(change) {
            low_signal_suppressed += 1;
        }
        match change {
            NodeChange::Added { .. } => added += 1,
            NodeChange::Removed { .. } => removed += 1,
            NodeChange::Changed { field, .. } => {
                changed += 1;
                let n = fields.get(field).and_then(Value::as_u64).unwrap_or(0) + 1;
                fields.insert(field.clone(), json!(n));
            }
        }
    }

    let sample = compact_diff_summary_sample(&diff.changes, limit);
    let meaningful_changes = diff.changes.len().saturating_sub(low_signal_suppressed);
    json!({
        "n_changes": diff.changes.len(),
        "meaningful_changes": meaningful_changes,
        "low_signal_suppressed": low_signal_suppressed,
        "added": added,
        "removed": removed,
        "changed": changed,
        "changed_fields": fields,
        "sample": sample,
        "truncated": meaningful_changes > limit,
    })
}

fn compact_diff_summary_sample(changes: &[NodeChange], limit: usize) -> Vec<Value> {
    changes
        .iter()
        .filter(|change| !low_signal_diff_change(change))
        .take(limit)
        .map(compact_node_change)
        .collect()
}

fn compact_node_change(change: &NodeChange) -> Value {
    match change {
        NodeChange::Added { id, label } => json!({
            "kind": "added",
            "id": id,
            "label": label.as_deref().map(truncate_diff_summary_value),
        }),
        NodeChange::Removed { id, label } => json!({
            "kind": "removed",
            "id": id,
            "label": label.as_deref().map(truncate_diff_summary_value),
        }),
        NodeChange::Changed {
            id,
            field,
            before,
            after,
        } => json!({
            "kind": "changed",
            "id": id,
            "field": field,
            "before": truncate_diff_summary_value(before),
            "after": truncate_diff_summary_value(after),
        }),
    }
}

fn low_signal_diff_change(change: &NodeChange) -> bool {
    diff_change_id(change).starts_with("mi_menuitemhit_")
}

fn diff_change_id(change: &NodeChange) -> &str {
    match change {
        NodeChange::Added { id, .. }
        | NodeChange::Removed { id, .. }
        | NodeChange::Changed { id, .. } => id,
    }
}

fn truncate_diff_summary_value(value: &str) -> String {
    let mut out = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx >= DIFF_SUMMARY_VALUE_LIMIT {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
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
    use dunst_core::mock::{MockPerceptor, RecordingExecutor};
    use dunst_core::{Perceptor, RawAxNode, Target, WindowRef};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn engine() -> Engine {
        engine_with_window(105)
    }

    fn engine_with_window(window_id: u32) -> Engine {
        let perceptor = Box::new(MockPerceptor::notes_fixture().unwrap());
        let exec = Box::<RecordingExecutor>::default();
        Engine::new(
            perceptor,
            exec,
            Target {
                pid: 1363,
                window_id,
            },
        )
        .unwrap()
    }

    fn engine_with_pid(pid: i32) -> Engine {
        let perceptor = Box::new(MockPerceptor::notes_fixture().unwrap());
        let exec = Box::<RecordingExecutor>::default();
        Engine::new(
            perceptor,
            exec,
            Target {
                pid,
                window_id: 105,
            },
        )
        .unwrap()
    }

    struct CountingPerceptor {
        roots: Vec<RawAxNode>,
        window: WindowRef,
        captures: Arc<AtomicUsize>,
    }

    impl Perceptor for CountingPerceptor {
        fn capture(&self, _target: &Target) -> dunst_core::Result<Vec<RawAxNode>> {
            self.captures.fetch_add(1, Ordering::SeqCst);
            Ok(self.roots.clone())
        }

        fn window_ref(&self, _target: &Target) -> dunst_core::Result<WindowRef> {
            Ok(self.window.clone())
        }
    }

    fn engine_with_capture_counter() -> (Engine, Arc<AtomicUsize>) {
        let roots: Vec<RawAxNode> =
            serde_json::from_str(include_str!("../../dunst-core/fixtures/notes.json")).unwrap();
        let captures = Arc::new(AtomicUsize::new(0));
        let perceptor = Box::new(CountingPerceptor {
            roots,
            window: WindowRef {
                pid: 1363,
                window_id: 105,
                app_name: "Notes".into(),
                title: "Notes – Aucune note".into(),
            },
            captures: captures.clone(),
        });
        let exec = Box::<RecordingExecutor>::default();
        let eng = Engine::new(
            perceptor,
            exec,
            Target {
                pid: 1363,
                window_id: 105,
            },
        )
        .unwrap();
        (eng, captures)
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
        resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    fn text_json(resp: &Value) -> Value {
        serde_json::from_str(&text(resp)).expect("tool text is JSON")
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
            assert!(
                t.contains(needle),
                "{tool}: error {t:?} should mention {needle:?}"
            );
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
    fn tool_call_results_include_timing_meta() {
        let mut e = engine();
        let resp = call(&mut e, "get_scene_graph", json!({ "view": "summary" }));
        assert_eq!(resp["result"]["_meta"]["dunst"]["tool"], "get_scene_graph");
        assert!(
            resp["result"]["_meta"]["dunst"]["timing_ms"]
                .as_f64()
                .is_some_and(|ms| ms >= 0.0),
            "timing_ms should be numeric: {resp}"
        );
    }

    #[test]
    fn page_state_returns_compact_orientation_payload() {
        let mut e = engine();
        let resp = call(&mut e, "page_state", json!({ "fresh": false, "limit": 4 }));
        assert!(!is_error(&resp), "page_state succeeds: {resp}");
        let state = text_json(&resp);
        assert_eq!(state["title"], "Notes – Aucune note");
        assert!(state["key_elements"].as_array().unwrap().len() <= 4);
        assert!(state["visible_text"].as_array().unwrap().len() <= 4);
    }

    #[test]
    fn text_snapshot_returns_ax_text_payload() {
        let mut e = engine();
        let resp = call(
            &mut e,
            "text_snapshot",
            json!({ "query": "Corps de la note", "fresh": false, "limit": 4 }),
        );
        assert!(!is_error(&resp), "text_snapshot succeeds: {resp}");
        let snippets = text_json(&resp);
        let snippets = snippets.as_array().expect("text_snapshot returns array");
        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0]["role"], "text_area");
    }

    #[test]
    fn window_view_returns_scoped_window_payload() {
        let mut e = engine();
        let resp = call(&mut e, "window_view", json!({ "fresh": false, "limit": 4 }));
        assert!(!is_error(&resp), "window_view succeeds: {resp}");
        let state = text_json(&resp);
        assert_eq!(state["title"], "Notes – Aucune note");
        assert!(state["window"]["w"].as_f64().unwrap() > 0.0);
        assert!(state["window"]["h"].as_f64().unwrap() > 0.0);
        assert!(state["key_elements"].as_array().unwrap().len() <= 4);
    }

    #[test]
    fn find_element_refreshes_and_can_filter_latent_matches() {
        let mut e = engine();
        let default = call(
            &mut e,
            "find_element",
            json!({ "query": "Supprimer", "fresh": false }),
        );
        assert!(!is_error(&default), "default find succeeds: {default}");
        assert!(
            !text_json(&default).as_array().unwrap().is_empty(),
            "default find keeps latent matches"
        );

        let visible_only = call(
            &mut e,
            "find_element",
            json!({ "query": "Supprimer", "visible_only": true, "fresh": false }),
        );
        assert!(
            !is_error(&visible_only),
            "visible find succeeds: {visible_only}"
        );
        assert_eq!(
            text_json(&visible_only).as_array().unwrap().len(),
            0,
            "visible_only drops collapsed/off-window matches"
        );
    }

    #[test]
    fn find_element_force_refresh_uses_recent_visible_cached_match() {
        let (mut e, captures) = engine_with_capture_counter();
        assert_eq!(
            captures.load(Ordering::SeqCst),
            1,
            "Engine::new performs the initial capture"
        );

        let resp = call(
            &mut e,
            "find_element",
            json!({
                "query": "Nouvelle note",
                "visible_only": true,
                "force_refresh": true
            }),
        );

        assert!(!is_error(&resp), "find_element succeeds: {resp}");
        assert_eq!(
            captures.load(Ordering::SeqCst),
            1,
            "a visible match in a very recent graph should not force a second AX capture"
        );
        assert!(
            !text_json(&resp).as_array().unwrap().is_empty(),
            "cached visible match returned"
        );
    }

    #[test]
    fn wait_for_element_timeout_has_single_clear_status() {
        let mut e = engine();
        let resp = call(
            &mut e,
            "wait_for_element",
            json!({
                "query": "definitely-not-present",
                "timeout_ms": 100,
                "interval_ms": 50
            }),
        );
        assert!(!is_error(&resp), "wait_for_element succeeds: {resp}");
        let body = text_json(&resp);
        assert_eq!(body["status"], "timeout");
        assert_eq!(body["condition_met"], false);
        assert_eq!(body["timed_out"], true);
        assert_eq!(body["found"], false);
        assert_eq!(body["matches"].as_array().unwrap().len(), 0);
        assert!(
            body.get("matched").is_none(),
            "legacy ambiguous field should not be returned: {body}"
        );
    }

    #[test]
    fn action_responses_are_compact_unless_full_diff_requested() {
        let mut e = engine();
        let id = text_json(&call(
            &mut e,
            "find_element",
            json!({ "query": "Nouvelle note", "fresh": false }),
        ))[0]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let compact = call(&mut e, "click_element", json!({ "id": id }));
        let compact_json = text_json(&compact);
        assert!(
            compact_json.get("graph_diff").is_none(),
            "compact response omits full graph_diff"
        );
        assert!(
            compact_json.get("graph_diff_summary").is_some(),
            "compact response carries graph_diff_summary"
        );

        let full = call(
            &mut e,
            "click_element",
            json!({ "id": id, "include_diff": true }),
        );
        let full_json = text_json(&full);
        assert!(
            full_json.get("graph_diff").is_some(),
            "include_diff restores full graph_diff"
        );
    }

    #[test]
    fn diff_summary_sample_prioritizes_useful_changes_over_browser_menu_noise() {
        let diff = GraphDiff {
            changes: vec![
                NodeChange::Changed {
                    id: "mi_menuitemhit_35".into(),
                    field: "label".into(),
                    before: "Toujours afficher".into(),
                    after: "".into(),
                },
                NodeChange::Changed {
                    id: "mi_menuitemhit_36".into(),
                    field: "label".into(),
                    before: "Afficher seulement sur la page de nouvel onglet".into(),
                    after: "".into(),
                },
                NodeChange::Added {
                    id: "btn_ajouter".into(),
                    label: Some("Ajouter".into()),
                },
            ],
        };

        let summary = diff_summary_value(&diff, 2);
        let sample = summary["sample"].as_array().unwrap();
        assert_eq!(summary["meaningful_changes"], 1);
        assert_eq!(summary["low_signal_suppressed"], 2);
        assert_eq!(sample.len(), 1);
        assert_eq!(sample[0]["id"], "btn_ajouter");
        assert!(
            sample
                .iter()
                .all(|entry| !entry["id"].as_str().unwrap().starts_with("mi_menuitemhit_")),
            "menu items should not dominate the compact diff sample: {summary}"
        );
    }

    #[test]
    fn audit_entry_full_diff_also_reports_meaningful_summary() {
        let entry = AuditEntry {
            ts_ms: 42,
            target_id: "txt_rust".into(),
            action: SemanticAction::Click,
            argument: None,
            risk: dunst_core::RiskAssessment::low(),
            reasoning: Some("select Rust".into()),
            result: ActionResult::Success,
            graph_diff: GraphDiff {
                changes: vec![NodeChange::Changed {
                    id: "mi_menuitemhit_35".into(),
                    field: "label".into(),
                    before: "".into(),
                    after: "Toujours afficher".into(),
                }],
            },
        };

        let value = audit_entry_value(entry, true);
        assert!(value.get("graph_diff").is_some());
        assert_eq!(value["graph_diff_summary"]["meaningful_changes"], 0);
        assert_eq!(value["graph_diff_summary"]["low_signal_suppressed"], 1);
        assert!(value["graph_diff_summary"]["sample"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn typed_audit_summary_reports_whether_target_value_changed() {
        let changed = AuditEntry {
            ts_ms: 42,
            target_id: "field_description".into(),
            action: SemanticAction::Type,
            argument: Some("nouvelle description".into()),
            risk: dunst_core::RiskAssessment::low(),
            reasoning: None,
            result: ActionResult::Success,
            graph_diff: GraphDiff {
                changes: vec![NodeChange::Changed {
                    id: "field_description".into(),
                    field: "value".into(),
                    before: "old".into(),
                    after: "nouvelle description".into(),
                }],
            },
        };
        let value = audit_entry_value(changed, false);
        assert_eq!(
            value["graph_diff_summary"]["typed_content_change_observed"],
            true
        );
        assert_eq!(
            value["graph_diff_summary"]["typed_content_exact_match"],
            true
        );

        let unchanged = AuditEntry {
            ts_ms: 43,
            target_id: "field_description".into(),
            action: SemanticAction::Type,
            argument: Some("nouvelle description".into()),
            risk: dunst_core::RiskAssessment::low(),
            reasoning: None,
            result: ActionResult::Success,
            graph_diff: GraphDiff {
                changes: vec![NodeChange::Changed {
                    id: "spinner".into(),
                    field: "bbox".into(),
                    before: "Some(Bbox { x: 1.0, y: 1.0, w: 1.0, h: 1.0 })".into(),
                    after: "Some(Bbox { x: 2.0, y: 2.0, w: 1.0, h: 1.0 })".into(),
                }],
            },
        };
        let value = audit_entry_value(unchanged, false);
        assert_eq!(
            value["graph_diff_summary"]["typed_content_change_observed"],
            false
        );
        assert_eq!(
            value["graph_diff_summary"]["typed_content_exact_match"],
            false
        );
    }

    #[test]
    fn typed_audit_summary_rejects_partial_target_value() {
        let partial = AuditEntry {
            ts_ms: 43,
            target_id: "field_description".into(),
            action: SemanticAction::Type,
            argument: Some("nouvelle description complete".into()),
            risk: dunst_core::RiskAssessment::low(),
            reasoning: None,
            result: ActionResult::Failed,
            graph_diff: GraphDiff {
                changes: vec![NodeChange::Changed {
                    id: "field_description".into(),
                    field: "value".into(),
                    before: "old".into(),
                    after: "uvelle description".into(),
                }],
            },
        };

        let value = audit_entry_value(partial, false);
        assert_eq!(
            value["graph_diff_summary"]["typed_content_change_observed"],
            true
        );
        assert_eq!(
            value["graph_diff_summary"]["typed_content_exact_match"],
            false
        );
    }

    #[test]
    fn failed_type_audit_includes_do_not_save_hint() {
        let entry = AuditEntry {
            ts_ms: 44,
            target_id: "field_description".into(),
            action: SemanticAction::Type,
            argument: Some("nouvelle description".into()),
            risk: dunst_core::RiskAssessment::low(),
            reasoning: None,
            result: ActionResult::Failed,
            graph_diff: GraphDiff::default(),
        };

        let value = audit_entry_value(entry, false);
        assert_eq!(
            value["graph_diff_summary"]["typed_content_change_observed"],
            false
        );
        assert_eq!(
            value["graph_diff_summary"]["typed_content_exact_match"],
            false
        );
        assert!(value["failure_hint"]["next_step"]
            .as_str()
            .unwrap()
            .contains("Do not click save"));
    }

    #[test]
    fn diff_summary_sample_truncates_large_changed_values() {
        let long_children = (0..80)
            .map(|idx| format!("grp_{idx:02}"))
            .collect::<Vec<_>>()
            .join(",");
        let diff = GraphDiff {
            changes: vec![NodeChange::Changed {
                id: "el_collective".into(),
                field: "children".into(),
                before: "".into(),
                after: long_children.clone(),
            }],
        };

        let summary = diff_summary_value(&diff, 1);
        let after = summary["sample"][0]["after"].as_str().unwrap();
        assert!(after.len() < long_children.len(), "{after}");
        assert!(after.ends_with("..."), "{after}");
    }

    #[test]
    fn tools_list_exposes_read_text_with_object_schema() {
        std::env::remove_var("DUNST_MCP_ENABLE_APPROVE_TOOL");
        let tools = tools_list();
        assert_eq!(tools.len(), 53, "tool count");
        // Every tool must declare a JSON-Schema object input (the type:object fix).
        for t in &tools {
            assert_eq!(
                t["inputSchema"]["type"], "object",
                "tool {} has a non-object inputSchema: {}",
                t["name"], t["inputSchema"]
            );
        }
        let read_text = tools
            .iter()
            .find(|t| t["name"] == "read_text")
            .expect("read_text tool present");
        assert_eq!(read_text["inputSchema"]["type"], "object");
        // `region` is optional → it must not be in `required`.
        assert_eq!(read_text["inputSchema"]["required"], json!([]));
        assert!(
            tools.iter().any(|t| t["name"] == "list_launchable_apps"),
            "installed-app catalogue tool present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "app_info"),
            "single app info tool present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "list_displays"),
            "display topology tool present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "list_browser_tabs"),
            "browser tab listing tool present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "wait_for_element"),
            "async element wait tool present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "pick_option"),
            "popover/list/radio option helper present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "raise_element"),
            "raise action helper present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "text_snapshot"),
            "AX text snapshot tool present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "wait_for_text_stable"),
            "AX text stability wait tool present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "window_view"),
            "scoped window view tool present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "desktop_view"),
            "desktop topology tool present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "visual_change_probe"),
            "visual change probe tool present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "analyze_region_ax"),
            "region AX analysis tool present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "move_window_to_display"),
            "display move tool present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "move_app_to_display"),
            "app display move tool present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "arrange_windows"),
            "window arrangement tool present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "version"),
            "runtime version tool present"
        );
        assert!(
            tools.iter().any(|t| t["name"] == "select_file"),
            "native file selection tool present"
        );
    }

    #[test]
    fn read_text_without_live_window_is_a_clean_error() {
        // An invalid window id → no live macOS window → a clean Err, never a panic.
        // (Off macOS, the stub returns the same class of error.)
        let mut e = engine_with_window(u32::MAX);

        // Direct engine call carries the "live macOS window" message.
        let err = e.read_text(None, false).unwrap_err();
        assert!(
            err.to_string().contains("live macOS window"),
            "unexpected error: {err}"
        );

        // Through the dispatcher: a well-formed isError result, not a crash.
        let resp = call(&mut e, "read_text", json!({}));
        assert!(
            is_error(&resp),
            "read_text without a live window must be isError: {resp}"
        );
        assert_eq!(resp["jsonrpc"], "2.0");

        // A malformed region is rejected before any capture is attempted.
        let bad = call(&mut e, "read_text", json!({ "region": { "x": 1.0 } }));
        assert!(is_error(&bad));
        assert!(text(&bad).contains("region"), "got: {}", text(&bad));
    }

    #[test]
    fn tools_list_exposes_click_at_and_press_key() {
        std::env::remove_var("DUNST_MCP_ENABLE_APPROVE_TOOL");
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

        let select_file = tools
            .iter()
            .find(|t| t["name"] == "select_file")
            .expect("select_file tool present");
        assert_eq!(select_file["inputSchema"]["type"], "object");
        assert_eq!(select_file["inputSchema"]["required"], json!(["path"]));

        let read_at = tools
            .iter()
            .find(|t| t["name"] == "read_at")
            .expect("read_at tool present");
        assert_eq!(read_at["inputSchema"]["required"], json!(["x", "y"]));
        assert_eq!(
            read_at["inputSchema"]["properties"]["borrow_cursor"]["type"],
            "boolean"
        );

        let read_series = tools
            .iter()
            .find(|t| t["name"] == "read_series")
            .expect("read_series tool present");
        assert_eq!(
            read_series["inputSchema"]["properties"]["borrow_cursor"]["type"],
            "boolean"
        );
        assert!(
            tools.iter().all(|t| t["name"] != "approve"),
            "approve is an operator-side escape hatch and is not advertised by default"
        );
    }

    #[test]
    fn version_tool_reports_build_identity() {
        let mut e = engine();
        let resp = call(&mut e, "version", json!({}));
        assert!(!is_error(&resp), "version succeeds: {resp}");
        let body = text_json(&resp);
        assert_eq!(body["version"], SERVER_VERSION);
        assert_eq!(body["protocol_version"], PROTOCOL_VERSION);
        assert!(body["version_label"]
            .as_str()
            .unwrap()
            .contains(SERVER_VERSION));
        assert!(body["git_sha"].is_string());
        assert!(body["build_time_unix"].is_string());
    }

    #[test]
    fn approve_tool_is_disabled_by_default() {
        std::env::remove_var("DUNST_MCP_ENABLE_APPROVE_TOOL");
        let mut e = engine();
        let resp = call(&mut e, "approve", json!({ "id": "anything" }));
        assert!(
            is_error(&resp),
            "approve must be disabled by default: {resp}"
        );
        assert!(text(&resp).contains("disabled"));
    }

    #[test]
    fn raw_pending_approval_includes_ui_mapping_fallback() {
        let entry = AuditEntry {
            ts_ms: 1,
            target_id: "keyboard@hotkey:cmd+l".into(),
            action: SemanticAction::Type,
            argument: Some("cmd+l".into()),
            risk: dunst_core::RiskAssessment {
                level: dunst_core::RiskLevel::High,
                requires_approval: true,
                reasons: vec!["raw input is not bound to a scene element".into()],
            },
            reasoning: Some("background hotkey".into()),
            result: ActionResult::PendingApproval,
            graph_diff: GraphDiff::default(),
        };

        let body = audit_entry_value(entry, false);
        assert_eq!(body["approval_hint"]["approve_tool"], "approve");
        assert_eq!(body["ui_fallback_hint"]["mode"], "ui_mapping");
        assert!(
            body["ui_fallback_hint"]["recommended_sequence"]
                .as_array()
                .unwrap()
                .iter()
                .any(|step| step["tool"] == "get_affordances"),
            "fallback should direct agents back to the affordance graph: {body}"
        );
        assert!(
            body["ui_fallback_hint"]["avoid"]
                .as_array()
                .unwrap()
                .iter()
                .any(|step| step.as_str().unwrap().contains("javascript: injection")),
            "fallback should rule out address-bar DOM injection: {body}"
        );
    }

    #[test]
    fn raw_input_tools_dispatch_and_error_cleanly() {
        // A non-existent pid: on macOS the raw CGEvent posts to nothing (no test
        // side effect); the dispatch wiring is what we assert here.
        let mut e = engine_with_pid(i32::MAX);

        // Missing required args → isError, before reaching the engine.
        assert!(
            is_error(&call(&mut e, "press_key", json!({}))),
            "press_key needs 'key'"
        );
        assert!(
            is_error(&call(&mut e, "click_at", json!({ "x": 10.0 }))),
            "click_at needs both 'x' and 'y'"
        );

        // press_key with an unknown key → a clean isError on both platforms (macOS:
        // the backend rejects the key name; non-macOS: the stub is unsupported).
        let bad = call(
            &mut e,
            "press_key",
            json!({ "key": "definitely-not-a-real-key-xyz" }),
        );
        assert!(is_error(&bad), "unknown key must be a clean isError: {bad}");
        assert_eq!(bad["jsonrpc"], "2.0");

        // click_at with in-target coords reaches the engine and returns a
        // well-formed JSON-RPC response (pending approval on macOS, isError on
        // non-macOS stub) — never a panic.
        let window = e.window_view(1).window;
        let resp = call(
            &mut e,
            "click_at",
            json!({ "x": window.x + 10.0, "y": window.y + 10.0 }),
        );
        assert_eq!(resp["jsonrpc"], "2.0");
        assert!(resp.get("result").is_some(), "well-formed response: {resp}");
        let body = text_json(&resp);
        if body["result"] == "pending_approval" {
            assert_eq!(
                body["approval_hint"]["approve_tool"], "approve",
                "pending raw input should tell agents how to approve: {body}"
            );
            assert_eq!(
                body["approval_hint"]["approve_arguments"]["id"],
                body["target_id"]
            );
            assert_eq!(body["ui_fallback_hint"]["mode"], "ui_mapping");
        }

        let missing_file = call(
            &mut e,
            "select_file",
            json!({ "path": "/definitely/not/a/real/file.pdf" }),
        );
        assert!(
            is_error(&missing_file),
            "select_file rejects missing paths before touching the OS: {missing_file}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn select_file_gates_existing_file_before_os_interaction() {
        let mut e = engine();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let resp = call(&mut e, "select_file", json!({ "path": path }));
        assert!(!is_error(&resp), "first select_file call gates: {resp}");
        let body = text_json(&resp);
        assert_eq!(body["result"], "pending_approval");
        assert_eq!(body["approval_hint"]["approve_tool"], "approve");
        assert!(
            body["risk"]["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|reason| reason
                    .as_str()
                    .unwrap()
                    .contains("selects a local file for upload")),
            "risk explains file selection: {body}"
        );
    }

    #[test]
    fn read_series_rejects_malformed_points() {
        let mut e = engine();
        let cases = [
            json!({ "points": [ [1.0, 2.0], [3.0] ] }),
            json!({ "points": [ [1.0, 2.0], ["x", 3.0] ] }),
            json!({ "points": [ [1.0, 2.0], { "x": 3.0, "y": 4.0 } ] }),
        ];
        for args in cases {
            let resp = call(&mut e, "read_series", args);
            assert!(is_error(&resp), "malformed points must fail: {resp}");
            assert!(text(&resp).contains("point"), "got: {}", text(&resp));
        }
    }
}
