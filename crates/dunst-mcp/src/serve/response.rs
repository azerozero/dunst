use std::time::Instant;

use dunst_core::{
    ActionResult, AuditEntry, GraphDiff, NodeChange, SemanticAction, SessionIdentity,
};
use serde_json::{json, Value};

use crate::engine::{ModalDismissResult, OcrClickResult, OptionPickResult};

use super::{
    build_git_dirty, build_git_sha, build_time_unix, server_version_label,
    DIFF_SUMMARY_VALUE_LIMIT, SERVER_VERSION,
};

pub(super) fn add_timing_meta(
    mut result: Value,
    tool: &str,
    started: Instant,
    session: Option<&SessionIdentity>,
) -> Value {
    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
    if let Value::Object(obj) = &mut result {
        let session = session
            .and_then(|identity| serde_json::to_value(identity).ok())
            .unwrap_or(Value::Null);
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
                    "build_time_unix": build_time_unix(),
                    "session": session
                }
            }),
        );
    }
    result
}

pub(super) fn audit_entry_value(entry: AuditEntry, include_diff: bool) -> Value {
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
        if entry.result == ActionResult::Success {
            if let Some(hint) = success_action_hint(&entry) {
                obj.insert("verification_hint".into(), hint);
            }
        }
    }
    value
}

fn raw_input_target(target_id: &str) -> bool {
    target_id.starts_with("keyboard@")
        || target_id.starts_with("cursor@")
        || target_id.starts_with("wheel@")
        || target_id.starts_with("ocr@")
        || target_id.starts_with("screen@")
        || target_id.starts_with("file@")
        || target_id.starts_with("hover-reveal@")
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
            "next_step": "Check desktop_view/window_view for whether another window of the same app is frontmost. Try focus_window/open_menu again only if the target remains background-safe; use raise_element only after explicit operator approval."
        })),
        SemanticAction::Click if entry.target_id.starts_with("menubar_") => Some(json!({
            "reason": "The menu-bar item was visible in AX but pressing it did not open the menu.",
            "next_step": "Use open_menu with the exact localized label and verify the menu opened. If another window of the same app is frontmost, raise only after explicit operator approval."
        })),
        SemanticAction::Click | SemanticAction::Pick
            if entry.target_id.starts_with("mi_") && entry.result == ActionResult::Failed =>
        {
            Some(json!({
                "reason": "The target looks like a native menu item, but the action failed. If its bbox is empty or the menu is not visibly open, it is a latent AX menu item and cannot be actioned directly.",
                "next_step": "Open the parent menu visibly first, or use a context-menu item that appears in get_hit_targets/find_element with a real bbox. Do not keep retrying latent mi_* ids.",
                "verification": "the menu item should have a non-empty on-screen bbox, or the parent menu should be visibly open before click_element/pick_option"
            }))
        }
        SemanticAction::Click
            if entry.target_id.starts_with("field_") || entry.target_id.starts_with("text_") =>
        {
            Some(json!({
                "reason": "The text-field click did not produce a verified focus/caret placement.",
                "next_step": "Do not type yet. Re-read the field with find_element or verify_state focused=true. If it is still false, try focus_window and retry the element-bound click; use raise_element or raw click only after explicit operator authorization.",
                "verification": "verify_state(field='focused', expected='true') should pass before typing"
            }))
        }
        SemanticAction::Click if entry.target_id.starts_with("btn_remove_") => Some(json!({
            "reason": "The element-bound click completed at the platform layer, but the requested removal was not observed after re-perception.",
            "next_step": "Do not click save/submit. Re-read the matching labels with find_element visible_only=false and retry only if a stable element id can be resolved; otherwise cancel the edit session.",
            "verification": "the target id must disappear or the count of elements with the same remove label must decrease"
        })),
        SemanticAction::Click if entry.target_id.starts_with("chk_") => Some(json!({
            "reason": "The element-bound checkbox click completed at the platform layer, but the checkbox value did not change after re-perception.",
            "next_step": "Do not save yet. Re-read the checkbox with find_element visible_only=false. If the value is still unchanged, expose the checkbox in the viewport or retry only through a stable element id.",
            "verification": "the target checkbox value should change between 0/1 or false/true after the click"
        })),
        _ => None,
    }
}

fn success_action_hint(entry: &AuditEntry) -> Option<Value> {
    if entry.action == SemanticAction::Raise {
        return Some(json!({
            "reason": "The platform raise action returned success, but foreground/window stacking can still be stale or blocked by another same-app/frontmost window.",
            "next_step": "Call target_visibility or expose_target_window to verify that covered_by is empty before OCR, screenshots, or raw pointer input.",
            "verification": "target_visibility.status should be frontmost or visible_background with an empty covered_by list"
        }));
    }
    if entry.action == SemanticAction::Scroll
        && entry.graph_diff.changes.iter().all(low_signal_diff_change)
    {
        let next_step = if entry.target_id.starts_with("cursor@scroll:") {
            "Verify with read_text/OCR before relying on the new viewport. If OCR did not move after a real-cursor scroll, do not repeat the same point; choose an OCR text/card point inside the scrollable content, or use expose_target_window/raise_element only after explicit operator approval when foreground focus is acceptable."
        } else {
            "Verify with read_text/OCR or window_view before relying on the new viewport. If OCR did not move after background scroll, retry scroll_at at a visible OCR text/card point with borrow_cursor=true; for AX-backed panes, prefer scroll with a scrollable element id."
        };
        return Some(json!({
            "reason": "The scroll action returned success, but no meaningful AX graph movement was observed.",
            "next_step": next_step,
            "verification": "visible text, OCR, or key element positions should change before relying on the new viewport"
        }));
    }
    if entry.action == SemanticAction::Click
        && raw_input_target(&entry.target_id)
        && entry.graph_diff.changes.iter().all(low_signal_diff_change)
    {
        let next_step = if entry.target_id.starts_with("screen@") {
            "Do not repeat the same raw point. Re-map the UI with screenshot diagnostics, OCR, read_shapes, or analyze_region_ax; remember screenshots are image pixels while raw click tools expect global screen points."
        } else if entry.target_id.starts_with("hover-reveal@") {
            "Do not retry the same hover-reveal target blindly. Read the failure detail, then verify whether the target window is visible/topmost and whether the requested hover text appears with OCR before another mutating attempt."
        } else {
            "Do not assume the raw action changed UI state. Re-read the target UI and choose an element-bound or OCR-bound action before the next mutation."
        };
        return Some(json!({
            "reason": "The raw click returned success, but no meaningful AX graph change was observed afterward.",
            "next_step": next_step,
            "verification": "expected_text_found should be true, or OCR/page_state/detect_modal should show the intended state change before continuing"
        }));
    }
    if entry.action != SemanticAction::Click
        || raw_input_target(&entry.target_id)
        || entry
            .graph_diff
            .changes
            .iter()
            .any(|change| !low_signal_diff_change(change))
    {
        return None;
    }

    Some(json!({
        "reason": "The platform click returned success, but no meaningful AX graph change was observed afterward.",
        "next_step": "Treat this as unverified. Re-read the target UI before taking the next mutating step; do not assume that a modal opened, a form saved, or a disabled control changed.",
        "verification": "use page_state, find_element, verify_state, or wait_for_text_stable to confirm the intended state change"
    }))
}

pub(super) fn option_pick_value(result: OptionPickResult, include_diff: bool) -> Value {
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

pub(super) fn ocr_click_value(result: OcrClickResult, include_diff: bool) -> Value {
    let audit = audit_entry_value(result.audit, include_diff);
    json!({
        "query": result.query,
        "hit": result.hit,
        "click_point": result.click_point,
        "offset": result.offset,
        "audit": audit,
        "expected_text": result.expected_text,
        "expected_text_found": result.expected_text_found,
        "verification_hint": result.verification_hint,
    })
}

pub(super) fn modal_dismiss_value(result: ModalDismissResult, include_diff: bool) -> Value {
    let audit = audit_entry_value(result.audit, include_diff);
    json!({
        "modal_before": result.modal_before,
        "clicked": result.clicked,
        "audit": audit,
        "modal_after": result.modal_after,
        "dismissed": result.dismissed,
        "verification_hint": result.verification_hint,
    })
}

pub(super) fn diff_summary_value(diff: &GraphDiff, limit: usize) -> Value {
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
    let id = diff_change_id(change);
    if id.starts_with("mi_menuitemhit_") || id.contains("intercom") {
        return true;
    }
    matches!(
        change,
        NodeChange::Changed { id, field, .. }
            if field == "bbox"
                && (id.starts_with("grp_")
                    || id.starts_with("el_")
                    || id.starts_with("img_"))
    )
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
