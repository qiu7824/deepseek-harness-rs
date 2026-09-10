//! Adapter for generated single-request Remote methods and legacy flat RPCs.
use serde_json::Value;

pub(crate) fn normalize(method: &str, payload: Value) -> Result<Value, serde_json::Error> {
    if !matches!(
        method,
        "pluginInventory.setEnabled"
            | "messageFeedback.put"
            | "messageFeedback.list"
            | "messageFeedback.delete"
            | "sessionFeedback.record"
    ) || payload.get("args").is_none()
    {
        return Ok(payload);
    }
    let valid = payload.as_object().is_some_and(|object| object.len() == 1)
        && payload["args"].as_object().is_some_and(|args| {
            args.len() == 1 && args.get("request").is_some_and(Value::is_object)
        });
    if !valid {
        return Err(<serde_json::Error as serde::de::Error>::custom(
            "expected the generated Remote envelope {args:{request:...}} without mixed flat fields",
        ));
    }
    Ok(payload["args"]["request"].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn generated_remote_requests_and_flat_rpcs_have_identical_business_payloads() {
        for method in [
            "pluginInventory.setEnabled",
            "messageFeedback.put",
            "messageFeedback.list",
            "messageFeedback.delete",
            "sessionFeedback.record",
        ] {
            let request = json!({"entryId":"sidebar","enabled":false});
            assert_eq!(
                normalize(method, json!({"args":{"request":request}})).unwrap(),
                request
            );
            assert_eq!(normalize(method, request.clone()).unwrap(), request);
            assert!(
                normalize(
                    method,
                    json!({"args":{"request":request},"entryId":"different"})
                )
                .is_err()
            );
        }
        let scoped = json!({"args":{"agentId":"root","request":{"objective":"work"}}});
        assert_eq!(normalize("goal.create", scoped.clone()).unwrap(), scoped);
    }
}
