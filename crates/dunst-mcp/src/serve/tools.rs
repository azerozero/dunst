use super::*;

pub(super) fn tools_list() -> Vec<Value> {
    let mut tools = Vec::new();
    tools.extend(state_tools());
    tools.extend(query_tools());
    tools.extend(element_tools());
    tools.extend(batch_tools());
    tools.extend(pointer_and_chart_tools());
    tools.extend(window_app_tools());
    tools.extend(keyboard_menu_tools());
    tools.extend(approval_tools());
    tools
}

fn state_tools() -> Vec<Value> {
    vec![
        tool(
            "version",
            "Return the running dunst-mcp build identity: package version, git commit, dirty flag, build timestamp, and protocol version. Use this after restart to confirm the active server binary.",
            json!({}),
        ),
        tool(
            "platform_capabilities",
            "Return grouped OS backend capabilities for input, clipboard, perception/OCR/CV, window, and app operations. Use this before assuming macOS-only live GUI features are available.",
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
            "target_visibility",
            "Return whether the attached target window is frontmost, covered, fully covered, or missing from the desktop stack. Use before OCR/screenshot/raw clicks when multiple windows overlap.",
            json!({}),
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
    ]
}

fn query_tools() -> Vec<Value> {
    vec![
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
            "Return the affordance graph (actions + risk per element). Latent (off-screen / zero-bbox) nodes are omitted unless include_latent is true. scope can limit results to page or browser_chrome.",
            schema(json!({ "include_latent": { "type": "boolean", "description": "include latent/off-screen nodes (default false)" }, "scope": { "type": "string", "enum": ["all", "page", "browser_chrome"], "description": "filter browser chrome versus page targets (default all)" } }), &[]),
        ),
        tool(
            "get_hit_targets",
            "Return semantic UI targets with labels, roles, safe click zones, available action modes (click/type/drag/drop/scroll/read_at/etc.), risk, target_visibility, selected browser tab, and a ui_epoch fingerprint. AX targets are listed first, with page scroll pseudo-targets plus OCR text/cards and vision shapes added as supplemental targets when available. Pass previous_epoch to detect stale coordinates after window moves, resizes, tab switches, or page changes.",
            schema(
                json!({
                    "include_latent": { "type": "boolean", "description": "include latent/off-screen nodes (default false)" },
                    "scope": { "type": "string", "enum": ["all", "page", "browser_chrome"], "description": "filter browser chrome versus page targets (default page)" },
                    "limit": { "type": "integer", "description": "max semantic targets, 1-500 (default 80)" },
                    "previous_epoch": { "type": "string", "description": "previous ui_epoch.fingerprint to detect stale plans" },
                    "fresh": { "type": "boolean", "description": "ensure recent graph before reading targets (default true)" },
                    "force_refresh": { "type": "boolean", "description": "force an AX refresh even if the short TTL is still valid (default false)" }
                }),
                &[],
            ),
        ),
        tool(
            "find_element",
            "Find elements whose id/label/role contains the query (case-insensitive). Ensures a recent AX graph by default. Results are ranked with visible enabled targets first; visible_only drops off-window/latent noise. If AX has no matches, falls back to OCR/vision hit targets tagged by source.",
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
            "OCR the target window (or an optional screen-point region x,y,w,h) via Apple Vision; returns recognised text lines with screen bbox + confidence. accurate:true uses the slower, more precise recognition (default fast). content_only:true filters browser chrome/tab-strip noise and low-confidence lines.",
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
                    "accurate": { "type": "boolean", "description": "slower, higher-accuracy OCR (default false)" },
                    "content_only": { "type": "boolean", "description": "filter browser chrome/tab strip and low-confidence text (default false)" }
                }),
                &[],
            ),
        ),
        tool(
            "read_text_detailed",
            "OCR with diagnostics: returns target_visibility, warnings, recommended_next_steps, all_hits, and filtered hits. Use this before raw clicks when windows may be covered or browser chrome may pollute OCR.",
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
                    "accurate": { "type": "boolean", "description": "slower, higher-accuracy OCR (default false)" },
                    "content_only": { "type": "boolean", "description": "filter browser chrome/tab strip and low-confidence text (default false)" }
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
            "find_ocr_text",
            "Search target-window OCR for text and return ranked hits with bbox, center point, confidence, target_visibility, and warnings. Use before click_near_text instead of hand-picked coordinates.",
            schema(
                json!({
                    "query": { "type": "string", "description": "text to find in OCR output" },
                    "content_only": { "type": "boolean", "description": "filter browser chrome/tab strip and low-confidence text (default true)" },
                    "accurate": { "type": "boolean", "description": "use slower accurate OCR (default true)" },
                    "limit": { "type": "integer", "description": "max ranked hits, 1-50 (default 20)" }
                }),
                &["query"],
            ),
        ),
        tool(
            "detect_modal",
            "Detect likely modal/overlay state and safe OCR close/dismiss candidates. Use before clicking behind a popup. If no close candidate is returned, do not guess raw coordinates.",
            json!({}),
        ),
        tool(
            "extract_ocr_cards",
            "Group target-window OCR lines into card-like candidates with title, rating/reviews/eta/fee/promo when visible, bbox, and target_visibility. Use for Uber Eats-like grids when AX exposes only a root group.",
            schema(
                json!({
                    "accurate": { "type": "boolean", "description": "use slower accurate OCR (default true)" },
                    "content_only": { "type": "boolean", "description": "filter browser chrome/tab strip and low-confidence text (default true)" },
                    "limit": { "type": "integer", "description": "max cards, 1-50 (default 24)" }
                }),
                &[],
            ),
        ),
        tool(
            "query_affordances",
            "List element ids that expose a given semantic action (click|type|hover|open_menu|pick|drag|...). Latent (off-screen / zero-bbox) nodes are omitted unless include_latent is true. scope can limit results to page or browser_chrome.",
            schema(
                json!({
                    "action": { "type": "string", "description": "semantic action" },
                    "include_latent": { "type": "boolean", "description": "include latent nodes (default false)" },
                    "scope": { "type": "string", "enum": ["all", "page", "browser_chrome"], "description": "filter browser chrome versus page targets (default all)" }
                }),
                &["action"],
            ),
        ),
        tool(
            "enumerate_choices",
            "Survey the whole choice surface once and return a structured option model: groups (single-select vs multi-select vs text field), required/optional, and per-choice {id, label, coords, current state}. Default mode captures off-screen AX choices in a single pass (no scroll). scroll_scan=true performs a position-restoring scroll sweep to also assemble OCR/vision choices on virtualized or AX-sparse surfaces; it surveys without operator approval and restores the original scroll position. Pass the returned ui_epoch to apply_selections as expected_epoch.",
            schema(
                json!({
                    "scope": { "type": "string", "enum": ["page", "all", "browser_chrome"], "description": "target surface (default page)" },
                    "include_latent": { "type": "boolean", "description": "include off-screen AX choices (default true)" },
                    "scroll_scan": { "type": "boolean", "description": "position-restoring scroll sweep for OCR-only surfaces (default false)" },
                    "max_scroll_pages": { "type": "integer", "description": "scroll-sweep bound, 1-12 (default 6)" },
                    "limit": { "type": "integer", "description": "max choices, 1-500 (default 200)" },
                    "fresh": { "type": "boolean", "description": "ensure a recent graph first (default true)" },
                    "force_refresh": { "type": "boolean", "description": "force AX refresh even inside the short TTL (default false)" }
                }),
                &[],
            ),
        ),
    ]
}

fn element_tools() -> Vec<Value> {
    vec![
        tool(
            "click_element",
            "Click an element by id. High-risk elements return pending_approval until approve() is called. Action responses are compact by default; set include_diff=true for the full scene diff.",
            schema(json!({ "id": {"type":"string"}, "reasoning": {"type":"string"}, "include_diff": {"type":"boolean"} }), &["id"]),
        ),
        tool(
            "raise_element",
            "Raise an element by id when its affordance exposes the semantic raise action, typically a window root such as win_collective. Foreground-affecting: this is high-risk and requires explicit operator approval; prefer focus_window or element-bound actions when possible.",
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
            "Click at a raw screen point (x,y) inside the target window. Prefer click_near_text or click_element whenever possible. Raw mutating input is high-risk and requires approval. If pending_approval is not explicitly approved, switch to ui_fallback_hint: map the UI with window_view/get_affordances/find_element/find_ocr_text, then use element-bound or OCR-bound actions. Off-target points are rejected unless DUNST_MCP_ALLOW_OFF_TARGET_RAW=1. If the user-active guard blocks it, wait until the operator is idle and retry once.",
            schema(json!({ "x": {"type":"number"}, "y": {"type":"number"}, "expected_text": {"type":"string", "description":"optional text expected to be visible after the raw click"}, "include_diff": {"type":"boolean"} }), &["x", "y"]),
        ),
        tool(
            "click_near_text",
            "OCR the target window, choose a ranked text hit by query, click its bbox centre or a bounded offset from that centre, and optionally verify expected_text afterward. This is still raw input and approval-gated, but safer than manual click_at because the returned audit includes the OCR hit and any offset used.",
            schema(
                json!({
                    "query": {"type":"string", "description":"visible OCR text to click"},
                    "occurrence": {"type":"integer", "description":"1-based ranked hit to click (default 1)"},
                    "offset_x": {"type":"number", "description":"optional x offset in screen points from the OCR bbox centre, clamped to +/-1000; use for adjacent form fields"},
                    "offset_y": {"type":"number", "description":"optional y offset in screen points from the OCR bbox centre, clamped to +/-1000; use for adjacent form fields"},
                    "expected_text": {"type":"string", "description":"optional text that should be visible after the click"},
                    "content_only": {"type":"boolean", "description":"filter browser chrome/tab strip and low-confidence text (default true)"},
                    "accurate": {"type":"boolean", "description":"use slower accurate OCR (default true)"},
                    "reasoning": {"type":"string"},
                    "include_diff": {"type":"boolean"}
                }),
                &["query"],
            ),
        ),
        tool(
            "dismiss_modal",
            "Safely dismiss a modal only by clicking an OCR-detected close/dismiss candidate. Refuses to click guessed modal corners or backdrop areas. High-risk and approval-gated because it uses raw pointer input.",
            schema(json!({ "reasoning": {"type":"string"}, "include_diff": {"type":"boolean"} }), &[]),
        ),
        tool(
            "reveal_hover_click",
            "Temporarily borrow the real cursor on an already-visible target-window point to reveal hover-only controls, click the first visible element matching query by AX id, then restore the cursor. Use for UIs where edit/delete buttons appear only while the real mouse hovers a card. High-risk: requires approval, mutates UI, and refuses covered target pixels. settle_ms is clamped to 50-1500 ms (default 250).",
            schema(
                json!({
                    "x": {"type":"number", "description":"screen x inside the target window/card to hover"},
                    "y": {"type":"number", "description":"screen y inside the target window/card to hover"},
                    "query": {"type":"string", "description":"visible button/element text to click after hover reveal, e.g. Modifier cette expérience"},
                    "settle_ms": {"type":"integer", "description":"hover/render wait before AX refresh; clamped 50-1500 ms, default 250"},
                    "reasoning": {"type":"string"},
                    "include_diff": {"type":"boolean"}
                }),
                &["x", "y", "query"],
            ),
        ),
        tool(
            "select_file",
            "Select a local file in the native platform file chooser for browser upload controls. Provide path plus either trigger_id (an existing upload/dropzone/link element to real-click first) or x/y (screen point to real-click first); omit trigger_id/x/y only when the file chooser is already open. High-risk: may real-click inside the target window and drive a native chooser through the platform backend. Off-target trigger points are rejected unless DUNST_MCP_ALLOW_OFF_TARGET_RAW=1.",
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
    ]
}

fn batch_tools() -> Vec<Value> {
    vec![tool(
        "apply_selections",
        "Apply a whole choice plan as ONE batch behind a single operator approval. Build the plan from enumerate_choices and pass that ui_epoch as expected_epoch. The batch refuses a stale plan up front, re-scans ONLY when the UI epoch fingerprint changes mid-batch (progressive disclosure / reflow) and re-resolves the remaining steps, then runs a single consolidated verify. First call returns status=pending_approval with a per-step preview incl. risk; an operator approves the returned batch_id once, then re-call with the same plan to execute.",
        schema(
            json!({
                "expected_epoch": { "type": "string", "description": "ui_epoch.fingerprint the plan was built from (required)" },
                "plan": {
                    "type": "object",
                    "properties": {
                        "steps": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "group_id": { "type": "string" },
                                    "choice_id": { "type": "string", "description": "Choice.id from enumerate_choices" },
                                    "label": { "type": "string", "description": "re-resolution fallback after reflow" },
                                    "op": { "type": "string", "enum": ["select", "deselect", "set_text"] },
                                    "value": { "type": "string", "description": "text for set_text" },
                                    "expected_after": {
                                        "type": "object",
                                        "properties": {
                                            "state": { "type": "string", "enum": ["selected", "unselected"] },
                                            "value": { "type": "string" }
                                        }
                                    }
                                },
                                "required": ["choice_id", "op"]
                            }
                        }
                    },
                    "required": ["steps"]
                },
                "include_diff": { "type": "boolean" }
            }),
            &["expected_epoch", "plan"],
        ),
    )]
}

fn pointer_and_chart_tools() -> Vec<Value> {
    vec![
        tool(
            "hover_at",
            "Hover (background mouse-move, no cursor movement) at a raw screen point (x,y) inside the target window so the target reveals a hover state — e.g. a chart crosshair tooltip / value-at-cursor — then read_text it. This does not move the real OS cursor; if a web UI only creates controls on real mouse hover and the target is covered, first check desktop_view.covered_by and make the target visible/raised or use read_at/read_series with borrow_cursor=true on an exposed point. A probe: no gating, no audit. Off-target points are rejected unless DUNST_MCP_ALLOW_OFF_TARGET_RAW=1.",
            schema(json!({ "x": {"type":"number"}, "y": {"type":"number"} }), &["x", "y"]),
        ),
        tool(
            "read_at",
            "Read the value at a screen point inside the target window. Defaults to background hover without borrowing the OS cursor; set borrow_cursor=true only when a surface requires a real cursor hover. With borrow_cursor=true, the composited screen is read at the cursor, so the target must be the visible/topmost window under that point; if OCR returns another app or hover-only buttons do not appear, inspect desktop_view.covered_by and expose/raise/arrange the target before retrying. Off-target points are rejected unless DUNST_MCP_ALLOW_OFF_TARGET_RAW=1.",
            schema(json!({ "x": {"type":"number"}, "y": {"type":"number"}, "borrow_cursor": {"type":"boolean","description":"briefly borrow and restore the OS cursor for real-hover-only surfaces; requires the target to be visible/topmost under the point (default false)"} }), &["x", "y"]),
        ),
        tool(
            "read_series",
            "Read values at SEVERAL screen points inside the target window. Defaults to background hover without borrowing the OS cursor; set borrow_cursor=true to borrow once, warp+OCR each point, then restore. With borrow_cursor=true, every point must be visibly occupied by the target window, not a covering app. points = [[x,y], ...]; returns one OCR list per point. Off-target points are rejected unless DUNST_MCP_ALLOW_OFF_TARGET_RAW=1.",
            schema(
                json!({ "points": { "type": "array", "items": { "type": "array", "items": { "type": "number" } } }, "borrow_cursor": {"type":"boolean","description":"borrow and restore the OS cursor for real-hover-only surfaces; requires visible/topmost target pixels at each point (default false)"} }),
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
            "unstick_cursor",
            "Recover a stuck OS cursor (e.g. a lingering I-beam) left by driving a backgrounded window with synthetic input on macOS — a known macOS focus bug, not fixable in-shape. Briefly opens and closes the Apple menu: a menu-bar focus cycle that makes the window server re-evaluate the cursor; no menu item is ever selected (open + close only). The cursor flashes to the top-left and back. Call after a batch of clicks/scrolls or whenever the cursor looks frozen. Not approval-gated. No-op off macOS.",
            json!({}),
        ),
    ]
}

fn window_app_tools() -> Vec<Value> {
    vec![
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
            "expose_target_window",
            "Try to make the attached target window actually visible, then verify with desktop_view. If the raise action is gated, returns raise_audit with pending_approval; approve that target_id and retry. arrange_if_needed:true may arrange the target plus covering windows in columns after a successful raise.",
            schema(json!({ "arrange_if_needed": { "type": "boolean", "description": "arrange target plus covering windows if raise leaves it covered (default false)" } }), &[]),
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
            "Launch an app without bringing it to the foreground when the platform backend supports that, optionally opening a url in it. Returns launched plus matching_windows, the current target, and a verification_hint because browsers may reuse another window/tab; after URL opens, call refresh + list_browser_tabs/window_view and attach if needed before acting. args: extra argv passed to the app (only applies when this call actually launches it). To read a Chromium chart in pure background, launch with args [\"--disable-features=CalculateNativeWinOcclusion\",\"--disable-renderer-backgrounding\",\"--disable-background-timer-throttling\",\"--disable-backgrounding-occluded-windows\"] so the <canvas> keeps painting while backgrounded (otherwise scan_chart sees a blank plot).",
            schema(json!({ "app": {"type":"string"}, "url": {"type":"string"}, "args": {"type":"array","items":{"type":"string"},"description":"extra argv for the app (e.g. Chromium background-paint flags)"} }), &["app"]),
        ),
        tool(
            "open_url_and_attach_tab",
            "Open a URL in an app, then attach Dunst to the best matching browser window and report whether the selected tab/window/page URL verifies against the URL, including verified_by when a signal matched. Use this instead of launch_app + manual tab guessing for browser navigation.",
            schema(json!({ "app": {"type":"string"}, "url": {"type":"string"}, "args": {"type":"array","items":{"type":"string"},"description":"extra argv passed when launching the app"} }), &["app", "url"]),
        ),
        tool(
            "navigate",
            "Load a URL in the ATTACHED browser window and re-verify. Use this to drive a backgrounded browser to a new page: it always forces a fresh load (never re-selects a stale existing tab that merely matches the URL, the way open_url_and_attach_tab can), and it does not rely on the address bar — background keystrokes can't reach browser chrome (a typed URL falls through to the page as in-page shortcuts). Returns the same attach/verify result as open_url_and_attach_tab.",
            schema(json!({ "url": {"type":"string"} }), &["url"]),
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
    ]
}

fn keyboard_menu_tools() -> Vec<Value> {
    vec![
        tool(
            "right_click_at",
            "Right-click at a raw screen point (x,y) — context menus. Uses a brief real-cursor warp/restore on visible target pixels because macOS positions context menus from the hardware cursor. Raw input is high-risk; if approval is unavailable or denied, switch to ui_fallback_hint and prefer element-bound actions. If the user-active guard blocks it, wait until the operator is idle and retry once.",
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
            "Press a named key on the target (e.g. \"Return\"/\"Enter\", \"Tab\", \"Escape\", arrows, \"Home\", \"End\", \"PageUp\", \"PageDown\"). Optional repeat batches simple repeated edits such as Backspace. Raw mutating keyboard input is high-risk: approve only after explicit operator authorization. If approval is unavailable or denied, switch to ui_fallback_hint and drive the UI through mapped element ids instead. If the user-active guard blocks it, wait until the operator is idle and retry once.",
            schema(json!({ "key": {"type":"string"}, "repeat": {"type":"integer"} }), &["key"]),
        ),
        tool(
            "type_keys",
            "Type text into the FOCUSED element via the SkyLight auth-signed keyboard path — reaches a backgrounded/occluded window's web content. Focus the field first (prefer click_element on a field; click_at is raw). Raw mutating keyboard input is high-risk and requires approval. If approval is unavailable or denied, switch to ui_fallback_hint and use type_into on a mapped field id. If the user-active guard blocks it, wait until the operator is idle and retry once.",
            schema(json!({ "text": {"type":"string"} }), &["text"]),
        ),
        tool(
            "set_field_text",
            "Clear the FOCUSED field and set it to text in one robust step. Sets the app's AXFocusedUIElement value directly (AX select-all-replace, with a keyboard fallback) — use this instead of clearing with raw End/Backspace/double-click + type_keys, which gives erratic cursor results on backgrounded web forms (e.g. it garbles to 'copullntents'). Focus the field first (click it). Raw mutating input is high-risk and requires approval.",
            schema(json!({ "text": {"type":"string"} }), &["text"]),
        ),
        tool(
            "paste_text",
            "Paste text into the FOCUSED element by temporarily replacing the system clipboard, sending Cmd+V to the target window, then restoring the previous plain-text clipboard by default. Use for opaque browser fields where type_keys is unreliable after focusing and verifying the field. Raw mutating keyboard input is high-risk and requires approval; rich clipboard formats may not survive, and OCR/page_state verification is required before saving.",
            schema(json!({ "text": {"type":"string"}, "restore_clipboard": {"type":"boolean","description":"restore the previous plain-text clipboard after pasting; defaults true"}, "include_diff": {"type":"boolean"} }), &["text"]),
        ),
        tool(
            "scroll",
            "Scroll the focused page/container. With a real AX id, uses direct AX scrollbar value changes on that element or an ancestor exposing AXVerticalScrollBar. With a page@scroll:* pseudo-target from get_hit_targets, uses background Page/Home/End keys and remains raw-approval-gated, unless this session has learned a validated app/page fallback such as real-cursor wheel scrolling for Firefox+LinkedIn. Without id, uses the same page path. direction: down|up|top|bottom; pages: number of pages (default 3). Action responses are compact by default; set include_diff=true for the full scene diff.",
            schema(json!({ "direction": {"type":"string","enum":["down","up","top","bottom"]}, "pages": {"type":"integer"}, "id": {"type":"string","description":"optional scrollable element id; requires an AXVerticalScrollBar on the element or an ancestor"}, "include_diff": {"type":"boolean"} }), &[]),
        ),
        tool(
            "scroll_at",
            "Wheel-scroll at a concrete screen point inside the target window. By default uses the SkyLight background wheel path, which does not move the OS cursor. Set borrow_cursor=true only when the target surface requires a real cursor wheel gesture; the point must be visibly occupied by the target window and the cursor is restored afterward. Use when a point-specific scroll container is needed and AX/page-key scrolling did not work. direction: down|up|top|bottom; pages defaults to 3. Raw wheel input is approval-gated.",
            schema(json!({ "x": {"type":"number"}, "y": {"type":"number"}, "direction": {"type":"string","enum":["down","up","top","bottom"]}, "pages": {"type":"integer"}, "borrow_cursor": {"type":"boolean","description":"briefly move and restore the real OS cursor, then post a global wheel event; requires visible target pixels at the point (default false)"}, "include_diff": {"type":"boolean"} }), &["x", "y"]),
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
    ]
}

fn approval_tools() -> Vec<Value> {
    if approval_tool_enabled() {
        vec![tool(
            "approve",
            "Operator-side escape hatch: approve a gated element or raw target so the next action on it proceeds. Disabled by default; set DUNST_MCP_ENABLE_APPROVE_TOOL=1 for controlled local sessions.",
            schema(json!({ "id": {"type":"string"} }), &["id"]),
        )]
    } else {
        Vec::new()
    }
}
