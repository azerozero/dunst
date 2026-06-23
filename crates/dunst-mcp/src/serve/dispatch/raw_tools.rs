use super::*;
use crate::engine::OcrClickOptions;

pub(super) fn dispatch(
    engine: &mut Engine,
    name: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    Some(match name {
        "click_at" => match point(args) {
            Some((x, y)) => match engine.click_at(x, y) {
                Ok(entry) => {
                    let include_diff = arg_bool(args, "include_diff").unwrap_or(false);
                    let expected = arg(args, "expected_text");
                    if let Some(expected) = expected {
                        let found = raw_expected_text_found(engine, &expected);
                        Ok(json!({
                            "audit": audit_entry_value(entry, include_diff),
                            "expected_text": expected,
                            "expected_text_found": found,
                            "verification_hint": if found {
                                Value::Null
                            } else {
                                json!("Raw click completed, but expected_text was not visible afterward; treat it as semantically unverified.")
                            }
                        }))
                    } else {
                        Ok(audit_entry_value(entry, include_diff))
                    }
                }
                Err(err) => Err(err.to_string()),
            },
            None => Err("click_at requires numeric 'x' and 'y'".into()),
        },
        "click_near_text" => match arg(args, "query") {
            Some(query) => engine
                .click_near_text(
                    &query,
                    OcrClickOptions {
                        content_only: arg_bool(args, "content_only").unwrap_or(true),
                        accurate: arg_bool(args, "accurate").unwrap_or(true),
                        occurrence: args.get("occurrence").and_then(Value::as_u64).unwrap_or(1)
                            as usize,
                        expected_text: arg(args, "expected_text").as_deref(),
                        reasoning: arg(args, "reasoning").as_deref(),
                        offset: (
                            args.get("offset_x").and_then(Value::as_f64).unwrap_or(0.0),
                            args.get("offset_y").and_then(Value::as_f64).unwrap_or(0.0),
                        ),
                    },
                )
                .map(|result| {
                    ocr_click_value(result, arg_bool(args, "include_diff").unwrap_or(false))
                })
                .map_err(|e| e.to_string()),
            None => Err("click_near_text requires 'query'".into()),
        },
        "dismiss_modal" => engine
            .dismiss_modal(arg(args, "reasoning").as_deref())
            .map(|result| {
                modal_dismiss_value(result, arg_bool(args, "include_diff").unwrap_or(false))
            })
            .map_err(|e| e.to_string()),
        "reveal_hover_click" => match (point(args), arg(args, "query")) {
            (Some((x, y)), Some(query)) => engine
                .reveal_hover_click(
                    x,
                    y,
                    &query,
                    args.get("settle_ms").and_then(Value::as_u64).unwrap_or(250),
                    arg(args, "reasoning").as_deref(),
                )
                .map(|entry| {
                    audit_entry_value(entry, arg_bool(args, "include_diff").unwrap_or(false))
                })
                .map_err(|e| e.to_string()),
            _ => Err(
                "reveal_hover_click requires numeric 'x', numeric 'y', and string 'query'".into(),
            ),
        },
        "hover_at" => match point(args) {
            Some((x, y)) => engine
                .hover_at(x, y)
                .map(|()| json!("ok"))
                .map_err(|e| e.to_string()),
            None => Err("hover_at requires numeric 'x' and 'y'".into()),
        },
        "focus_window" => Ok(json!({ "focused": engine.focus_window() })),
        "right_click_at" => match point(args) {
            Some((x, y)) => engine
                .right_click_at(x, y)
                .map(|entry| {
                    audit_entry_value(entry, arg_bool(args, "include_diff").unwrap_or(false))
                })
                .map_err(|e| e.to_string()),
            None => Err("right_click_at requires numeric 'x' and 'y'".into()),
        },
        "double_click_at" => match point(args) {
            Some((x, y)) => engine
                .double_click_at(x, y)
                .map(|entry| {
                    audit_entry_value(entry, arg_bool(args, "include_diff").unwrap_or(false))
                })
                .map_err(|e| e.to_string()),
            None => Err("double_click_at requires numeric 'x' and 'y'".into()),
        },
        "open_menu" => match arg(args, "name") {
            Some(name) => engine
                .open_menu(&name)
                .map(|entry| {
                    audit_entry_value(entry, arg_bool(args, "include_diff").unwrap_or(false))
                })
                .map_err(|e| e.to_string()),
            None => Err("open_menu requires 'name'".into()),
        },
        "press_key" => match arg(args, "key") {
            Some(key) => engine
                .press_key(
                    &key,
                    args.get("repeat").and_then(Value::as_u64).unwrap_or(1) as usize,
                )
                .map(|entry| {
                    audit_entry_value(entry, arg_bool(args, "include_diff").unwrap_or(false))
                })
                .map_err(|e| e.to_string()),
            None => Err("missing 'key'".into()),
        },
        "type_keys" => match arg(args, "text") {
            Some(text) => engine
                .type_keys(&text)
                .map(|entry| {
                    audit_entry_value(entry, arg_bool(args, "include_diff").unwrap_or(false))
                })
                .map_err(|e| e.to_string()),
            None => Err("missing 'text'".into()),
        },
        "scroll" => engine
            .scroll(
                arg(args, "direction").as_deref().unwrap_or("down"),
                args.get("pages").and_then(Value::as_u64).unwrap_or(3) as usize,
                arg(args, "id").as_deref(),
            )
            .map(|entry| audit_entry_value(entry, arg_bool(args, "include_diff").unwrap_or(false)))
            .map_err(|e| e.to_string()),
        "scroll_at" => match point(args) {
            Some((x, y)) => engine
                .scroll_at(
                    x,
                    y,
                    arg(args, "direction").as_deref().unwrap_or("down"),
                    args.get("pages").and_then(Value::as_u64).unwrap_or(3) as usize,
                    arg_bool(args, "borrow_cursor").unwrap_or(false),
                )
                .map(|entry| {
                    audit_entry_value(entry, arg_bool(args, "include_diff").unwrap_or(false))
                })
                .map_err(|e| e.to_string()),
            None => Err("scroll_at requires numeric 'x' and 'y'".into()),
        },
        "zoom" => engine
            .zoom(arg(args, "direction").as_deref().unwrap_or("in"))
            .map(|entry| audit_entry_value(entry, arg_bool(args, "include_diff").unwrap_or(false)))
            .map_err(|e| e.to_string()),
        "hotkey" => match arg(args, "combo") {
            Some(combo) => engine
                .hotkey(&combo)
                .map(|entry| {
                    audit_entry_value(entry, arg_bool(args, "include_diff").unwrap_or(false))
                })
                .map_err(|e| e.to_string()),
            None => Err("missing 'combo'".into()),
        },
        _ => return None,
    })
}

fn point(args: &Value) -> Option<(f64, f64)> {
    Some((
        args.get("x").and_then(Value::as_f64)?,
        args.get("y").and_then(Value::as_f64)?,
    ))
}

fn raw_expected_text_found(engine: &Engine, expected: &str) -> bool {
    let needle = expected.to_lowercase();
    engine
        .read_text_detailed(None, true, true)
        .map(|result| {
            result
                .hits
                .iter()
                .any(|hit| hit.text.to_lowercase().contains(&needle))
        })
        .unwrap_or(false)
}
