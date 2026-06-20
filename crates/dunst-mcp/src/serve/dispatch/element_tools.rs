use super::*;

pub(super) fn dispatch(
    engine: &mut Engine,
    name: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    Some(match name {
        "click_element" => match arg(args, "id") {
            Some(id) => engine
                .click_element(&id, arg(args, "reasoning").as_deref())
                .map(|entry| audit_entry_value(entry, arg_bool(args, "include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            None => Err("missing 'id'".into()),
        },
        "raise_element" => match arg(args, "id") {
            Some(id) => engine
                .raise_element(&id, arg(args, "reasoning").as_deref())
                .map(|entry| audit_entry_value(entry, arg_bool(args, "include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            None => Err("missing 'id'".into()),
        },
        "pick_option" => match arg(args, "query") {
            Some(query) => engine
                .pick_option(
                    &query,
                    arg_bool(args, "visible_only").unwrap_or(true),
                    arg(args, "reasoning").as_deref(),
                )
                .map(|result| option_pick_value(result, arg_bool(args, "include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            None => Err("missing 'query'".into()),
        },
        "type_into" => match (arg(args, "id"), arg(args, "text")) {
            (Some(id), Some(text)) => engine
                .type_into(&id, &text, arg(args, "reasoning").as_deref())
                .map(|entry| audit_entry_value(entry, arg_bool(args, "include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            _ => Err("missing 'id' or 'text'".into()),
        },
        "hover_probe" => match arg(args, "id") {
            Some(id) => engine
                .hover_probe(&id)
                .map(|entry| audit_entry_value(entry, arg_bool(args, "include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            None => Err("missing 'id'".into()),
        },
        "drag_element" => match (arg(args, "source_id"), arg(args, "target_id")) {
            (Some(source_id), Some(target_id)) => engine
                .drag_element(&source_id, &target_id, arg(args, "reasoning").as_deref())
                .map(|entry| audit_entry_value(entry, arg_bool(args, "include_diff").unwrap_or(false)))
                .map_err(|e| e.to_string()),
            _ => Err("missing 'source_id' or 'target_id'".into()),
        },
        "select_file" => match arg(args, "path") {
            Some(path) => {
                let trigger = match (
                    arg(args, "trigger_id"),
                    args.get("x").and_then(Value::as_f64),
                    args.get("y").and_then(Value::as_f64),
                ) {
                    (Some(trigger_id), _, _) => {
                        Some(crate::engine::FileSelectTrigger::ElementId(trigger_id))
                    }
                    (None, Some(x), Some(y)) => {
                        Some(crate::engine::FileSelectTrigger::Point { x, y })
                    }
                    (None, None, None) => None,
                    (None, _, _) => {
                        return Some(Err(
                            "select_file requires both numeric 'x' and 'y' when using coordinates"
                                .into(),
                        ))
                    }
                };
                engine
                    .select_file(&path, trigger, arg(args, "reasoning").as_deref())
                    .map(|entry| audit_entry_value(entry, arg_bool(args, "include_diff").unwrap_or(false)))
                    .map_err(|e| e.to_string())
            }
            None => Err("missing 'path'".into()),
        },
        "approve" => match arg(args, "id") {
            Some(id) if approval_tool_enabled() => engine
                .approve(&id)
                .map(|_| json!("approved"))
                .map_err(|e| e.to_string()),
            Some(_) => Err("approve tool is disabled; set DUNST_MCP_ENABLE_APPROVE_TOOL=1 for controlled operator sessions".into()),
            None => Err("missing 'id'".into()),
        },
        "verify_state" => match (arg(args, "id"), arg(args, "field"), arg(args, "expected")) {
            (Some(id), Some(field), Some(expected)) => engine
                .verify_state(&id, &field, &expected)
                .map(|matches| json!({ "matches": matches }))
                .map_err(|e| e.to_string()),
            _ => Err("missing 'id', 'field' or 'expected'".into()),
        },
        _ => return None,
    })
}
