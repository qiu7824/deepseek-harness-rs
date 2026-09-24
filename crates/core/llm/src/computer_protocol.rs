//! Native Responses computer-call vocabulary. Execution remains owned by the
//! computer controller and must retain its identity, permission and stop checks.
use serde_json::{Value, json};

pub const TOOL_NAME: &str = "computer_native";
pub const MAX_ACTIONS: usize = 64;
pub const MAX_PATH_POINTS: usize = 256;

fn fields(value: &Value, allowed: &[&str], required: &[&str]) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or("computer action must be an object")?;
    if object.keys().any(|key| !allowed.contains(&key.as_str()))
        || required.iter().any(|key| !object.contains_key(*key))
    {
        return Err("computer action has missing or unknown fields".into());
    }
    Ok(())
}
fn point(value: &Value) -> Result<(), String> {
    for key in ["x", "y"] {
        if !value[key].as_u64().is_some_and(|n| n <= 100_000) {
            return Err("computer coordinates must be integers between 0 and 100000".into());
        }
    }
    Ok(())
}
pub fn validate_actions(value: &Value) -> Result<(), String> {
    let actions = value
        .as_array()
        .ok_or("computer actions must be an array")?;
    if actions.is_empty() || actions.len() > MAX_ACTIONS {
        return Err("computer call must contain between 1 and 64 actions".into());
    }
    for action in actions {
        if let Some(keys) = action.get("keys") {
            let keys = keys.as_array().ok_or("computer keys must be an array")?;
            if keys.len() > 8
                || keys.iter().any(|key| {
                    !key.as_str().is_some_and(|key| {
                        !key.is_empty() && key.len() <= 64 && !key.chars().any(char::is_control)
                    })
                })
            {
                return Err("computer key sequence is invalid".into());
            }
        }
        match action["type"]
            .as_str()
            .ok_or("computer action type is missing")?
        {
            "click" => {
                fields(
                    action,
                    &["type", "x", "y", "button", "keys"],
                    &["type", "x", "y", "button"],
                )?;
                point(action)?;
                if !matches!(
                    action["button"].as_str(),
                    Some("left" | "right" | "middle" | "back" | "forward" | "wheel")
                ) {
                    return Err("unsupported computer mouse button".into());
                }
            }
            "double_click" | "move" => {
                fields(action, &["type", "x", "y", "keys"], &["type", "x", "y"])?;
                point(action)?;
            }
            "scroll" => {
                let keys = &["type", "x", "y", "scroll_x", "scroll_y"];
                fields(
                    action,
                    &["type", "x", "y", "scroll_x", "scroll_y", "keys"],
                    keys,
                )?;
                point(action)?;
                for key in ["scroll_x", "scroll_y"] {
                    if !action[key]
                        .as_i64()
                        .is_some_and(|n| (-100_000..=100_000).contains(&n))
                    {
                        return Err("computer scroll delta is out of bounds".into());
                    }
                }
            }
            "drag" => {
                fields(action, &["type", "path", "keys"], &["type", "path"])?;
                let path = action["path"]
                    .as_array()
                    .ok_or("computer drag path must be an array")?;
                if !(2..=MAX_PATH_POINTS).contains(&path.len()) {
                    return Err("computer drag path must contain between 2 and 256 points".into());
                }
                for p in path {
                    fields(p, &["x", "y"], &["x", "y"])?;
                    point(p)?;
                }
            }
            "keypress" => {
                fields(action, &["type", "keys"], &["type", "keys"])?;
                let keys = action["keys"]
                    .as_array()
                    .ok_or("computer keys must be an array")?;
                if keys.is_empty()
                    || keys.len() > 8
                    || keys.iter().any(|key| {
                        !key.as_str().is_some_and(|key| {
                            !key.is_empty() && key.len() <= 64 && !key.chars().any(char::is_control)
                        })
                    })
                {
                    return Err("computer key sequence is invalid".into());
                }
            }
            "type" => {
                fields(action, &["type", "text"], &["type", "text"])?;
                if !action["text"]
                    .as_str()
                    .is_some_and(|text| text.len() <= 65_536 && !text.contains('\0'))
                {
                    return Err("computer text is invalid or exceeds 64 KiB".into());
                }
            }
            "wait" | "screenshot" => {
                fields(action, &["type"], &["type"])?;
            }
            _ => return Err("unsupported native computer action".into()),
        }
    }
    Ok(())
}

/// Normalize current batched calls and legacy single-action calls. Pending
/// checks are retained as data; parsing never acknowledges or authorizes them.
pub fn parse_call(item: &Value) -> Result<(String, Value), String> {
    fields(
        item,
        &[
            "type",
            "id",
            "call_id",
            "status",
            "actions",
            "action",
            "pending_safety_checks",
        ],
        &["type", "call_id"],
    )?;
    if item["type"] != "computer_call"
        || item
            .get("status")
            .is_some_and(|status| status != "completed")
    {
        return Err("native computer call is not complete".into());
    }
    let id = item["call_id"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
        .ok_or("invalid computer call_id")?;
    if item.get("id").is_some_and(|id| {
        !id.as_str()
            .is_some_and(|id| !id.is_empty() && id.len() <= 256)
    }) {
        return Err("invalid computer item id".into());
    }
    let actions = match (item.get("actions"), item.get("action")) {
        (Some(actions), None) => actions.clone(),
        (None, Some(action)) => json!([action]),
        _ => return Err("computer call must have exactly one action representation".into()),
    };
    validate_actions(&actions)?;
    let checks = item
        .get("pending_safety_checks")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let list = checks
        .as_array()
        .filter(|checks| checks.len() <= 32)
        .ok_or("invalid computer safety checks")?;
    for check in list {
        fields(check, &["id", "code", "message"], &["id"])?;
        if !check["id"]
            .as_str()
            .is_some_and(|id| !id.is_empty() && id.len() <= 256)
        {
            return Err("invalid computer safety check id".into());
        }
        for key in ["code", "message"] {
            if check
                .get(key)
                .is_some_and(|v| !v.is_null() && !v.as_str().is_some_and(|s| s.len() <= 8192))
            {
                return Err("invalid computer safety check text".into());
            }
        }
    }
    Ok((
        id.into(),
        json!({"actions":actions,"pendingSafetyChecks":checks}),
    ))
}

/// Runtime-owned receipt, never inferred from text or model arguments.
pub const SAFETY_RECEIPT: &str = "plugin:computer-safety-receipt";

pub fn safety_receipt(
    content: &[crate::ContentBlock],
    call_id: &str,
) -> Result<Option<Value>, String> {
    let mut receipt = None;
    for block in content {
        let crate::ContentBlock::Extension(block) = block else {
            continue;
        };
        if block.type_ != SAFETY_RECEIPT {
            continue;
        }
        if receipt.is_some()
            || block.fields.len() != 2
            || block.fields.get("callId").and_then(Value::as_str) != Some(call_id)
        {
            return Err("Invalid native computer safety receipt".into());
        }
        let checks = block
            .fields
            .get("checks")
            .ok_or("Missing safety receipt checks")?;
        let (_, parsed) = parse_call(
            &json!({"type":"computer_call","call_id":call_id,"actions":[{"type":"screenshot"}],"pending_safety_checks":checks}),
        )?;
        if parsed["pendingSafetyChecks"].as_array().unwrap().is_empty() {
            return Err("Empty native computer safety receipt".into());
        }
        receipt = Some(checks.clone());
    }
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_actions_preserve_order_ids_and_unacknowledged_checks() {
        let actions = json!([
            {"type":"click","x":1,"y":2,"button":"left"},
            {"type":"double_click","x":1,"y":2},
            {"type":"move","x":3,"y":4},
            {"type":"scroll","x":3,"y":4,"scroll_x":-20,"scroll_y":200},
            {"type":"drag","path":[{"x":1,"y":2},{"x":2,"y":4},{"x":7,"y":8}]},
            {"type":"keypress","keys":["CTRL","A"]}, {"type":"type","text":"中文"},
            {"type":"wait"}, {"type":"screenshot"}
        ]);
        let checks = json!([{"id":"check-1","code":"sensitive_domain","message":"Confirm access"}]);
        let (id,args)=parse_call(&json!({"type":"computer_call","id":"item-1","call_id":"call-1","actions":actions,"status":"completed","pending_safety_checks":checks})).unwrap();
        assert_eq!(id, "call-1");
        assert_eq!(args["actions"], actions);
        assert_eq!(args["pendingSafetyChecks"], checks);
        assert!(args.get("acknowledged_safety_checks").is_none());
        assert_eq!(
            parse_call(
                &json!({"type":"computer_call","call_id":"old","action":{"type":"screenshot"}})
            )
            .unwrap()
            .1["actions"],
            json!([{"type":"screenshot"}])
        );
    }
    #[test]
    fn invalid_native_calls_fail_before_any_action_can_be_dispatched() {
        for actions in [
            json!([]),
            json!([{"type":"click","button":"left","x":-1,"y":1}]),
            json!([{"type":"move","x":1.5,"y":1}]),
            json!([{"type":"drag","path":[{"x":1,"y":1}]}]),
            json!([{"type":"type","text":"ok","target":"other"}]),
            json!([{"type":"keypress","keys":[]}]),
            json!([{"type":"screenshot","acknowledged":true}]),
            json!([{"type":"unknown"}]),
            json!(vec![json!({"type":"screenshot"}); 65]),
        ] {
            assert!(
                parse_call(&json!({"type":"computer_call","call_id":"x","actions":actions}))
                    .is_err()
            );
        }
        assert!(parse_call(&json!({"type":"computer_call","call_id":"x","status":"in_progress","actions":[{"type":"screenshot"}]})).is_err());
        assert!(parse_call(&json!({"type":"computer_call","call_id":"x","actions":[{"type":"screenshot"}],"action":{"type":"screenshot"}})).is_err());
    }
}
