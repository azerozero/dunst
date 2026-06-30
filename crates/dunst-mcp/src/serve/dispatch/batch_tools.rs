use super::*;

pub(super) fn dispatch(
    engine: &mut Engine,
    name: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    Some(match name {
        "apply_selections" => {
            let expected_epoch = match arg(args, "expected_epoch") {
                Some(epoch) => epoch,
                None => return Some(Err("apply_selections requires 'expected_epoch'".into())),
            };
            let plan = match args.get("plan") {
                Some(plan) => {
                    match serde_json::from_value::<crate::engine::SelectionPlan>(plan.clone()) {
                        Ok(plan) => plan,
                        Err(err) => {
                            return Some(Err(format!("apply_selections has invalid 'plan': {err}")))
                        }
                    }
                }
                None => return Some(Err("apply_selections requires 'plan'".into())),
            };
            engine
                .apply_selections(plan, &expected_epoch)
                .map(|outcome| serde_json::to_value(outcome).unwrap_or(Value::Null))
                .map_err(|e| e.to_string())
        }
        _ => return None,
    })
}
