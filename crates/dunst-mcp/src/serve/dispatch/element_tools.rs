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
                .map(|entry| audit_entry_with_args(entry, args))
                .map_err(|e| e.to_string()),
            None => Err("missing 'id'".into()),
        },
        "raise_element" => match arg(args, "id") {
            Some(id) => engine
                .raise_element(&id, arg(args, "reasoning").as_deref())
                .map(|entry| audit_entry_with_args(entry, args))
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
                .map(|entry| audit_entry_with_args(entry, args))
                .map_err(|e| e.to_string()),
            _ => Err("missing 'id' or 'text'".into()),
        },
        "hover_probe" => match arg(args, "id") {
            Some(id) => engine
                .hover_probe(&id)
                .map(|entry| audit_entry_with_args(entry, args))
                .map_err(|e| e.to_string()),
            None => Err("missing 'id'".into()),
        },
        "drag_element" => match (arg(args, "source_id"), arg(args, "target_id")) {
            (Some(source_id), Some(target_id)) => engine
                .drag_element(&source_id, &target_id, arg(args, "reasoning").as_deref())
                .map(|entry| audit_entry_with_args(entry, args))
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
                    .map(|entry| audit_entry_with_args(entry, args))
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
        "preauthorize" => {
            if !approval_tool_enabled() {
                Err("preauthorize is disabled; set DUNST_MCP_ENABLE_APPROVE_TOOL=1 for controlled operator sessions".into())
            } else {
                let (budget, ttl_ms) = match preauthorization_limits(args) {
                    Ok(limits) => limits,
                    Err(error) => return Some(Err(error)),
                };
                let (window_id, budget, ttl_ms) = engine.preauthorize_raw_input(budget, ttl_ms);
                Ok(json!({
                    "preauthorized": true,
                    "window_id": window_id,
                    "budget": budget,
                    "ttl_ms": ttl_ms,
                    "note": "Operator grant active for raw keyboard/pointer input in this window until budget/TTL is spent. Does not authorize batches, file selection, high-risk element actions, or unrelated external commitments. Do not renew without operator consent; revoke with revoke_preauthorization"
                }))
            }
        }
        "revoke_preauthorization" => {
            let dropped_budget = engine.raw_preauthorization_remaining().map(|(_, left, _)| left);
            Ok(json!({
                "revoked": engine.revoke_raw_preauthorization(),
                "dropped_budget": dropped_budget
            }))
        }
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

fn preauthorization_limits(args: &Value) -> Result<(usize, u64), String> {
    let bounded = |key: &str, default, min, max| match args.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .filter(|n| (min..=max).contains(n))
            .ok_or_else(|| format!("'{key}' must be an integer in {min}..={max}")),
    };
    Ok((
        bounded("budget", 20, 1, 100)? as usize,
        bounded("ttl_ms", 120_000, 1_000, 600_000)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preauthorization_rejects_invalid_limits_instead_of_granting_defaults() {
        assert_eq!(preauthorization_limits(&json!({})).unwrap(), (20, 120_000));
        assert_eq!(
            preauthorization_limits(&json!({"budget":100,"ttl_ms":600000})).unwrap(),
            (100, 600_000)
        );
        for args in [
            json!({"budget":0}),
            json!({"budget":101}),
            json!({"budget":-1}),
            json!({"budget":"20"}),
            json!({"budget":null}),
            json!({"budget":1.5}),
            json!({"ttl_ms":999}),
            json!({"ttl_ms":600001}),
        ] {
            assert!(preauthorization_limits(&args).is_err(), "{args}");
        }
    }
}
