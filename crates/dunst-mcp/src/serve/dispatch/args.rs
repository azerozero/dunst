use super::*;

pub(super) fn arg(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_owned)
}

pub(super) fn arg_bool(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
}

/// Parse an optional screen-point `region` object.
pub(super) fn parse_region(args: &Value) -> Result<Option<Bbox>, String> {
    match args.get("region") {
        None | Some(Value::Null) => Ok(None),
        Some(region) => {
            let coord = |key: &str| region.get(key).and_then(Value::as_f64);
            match (coord("x"), coord("y"), coord("w"), coord("h")) {
                (Some(x), Some(y), Some(w), Some(h)) => Ok(Some(Bbox { x, y, w, h })),
                _ => Err("region requires numeric x, y, w, h".into()),
            }
        }
    }
}

pub(super) fn parse_action(action: &str) -> Option<SemanticAction> {
    Some(match action.to_ascii_lowercase().as_str() {
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
