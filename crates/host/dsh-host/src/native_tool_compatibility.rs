//! Protocol compatibility is not proof of model capability or account entitlement.
use serde_json::Value;

pub(crate) fn status(profile: Option<&Value>) -> &'static str {
    let Some(profile) = profile else {
        return "unknown";
    };
    if let Some(auth) = profile["authProvider"].as_str() {
        return if auth == "openai-codex" {
            "compatible"
        } else {
            "unsupported"
        };
    }
    match profile["api"].as_str() {
        Some("openai-completions" | "openai-responses") => "compatible",
        Some(_) => "unsupported",
        None => "unknown",
    }
}

pub(crate) fn ensure_supported(route: &str, state: &str) -> Result<(), String> {
    if state == "unsupported" {
        return Err(format!(
            "NATIVE_TOOL_UNSUPPORTED: 连接 {route} 不支持此图像/托管搜索接口；普通对话和工具调用仍可使用。请在设置→模型→任务分工选择兼容连接。该操作尚未发出请求；相同连接重试无效。"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn subscribed_chat_routes_do_not_inherit_native_tools_from_wire_format() {
        for auth in ["devin", "windsurf", "claude-code"] {
            assert_eq!(
                status(Some(&json!({"authProvider":auth,"api":"openai-responses"}))),
                "unsupported"
            );
        }
        assert_eq!(
            status(Some(
                &json!({"authProvider":"openai-codex","api":"openai-responses"})
            )),
            "compatible"
        );
    }
    #[test]
    fn unknown_profiles_remain_unknown_and_known_protocol_mismatches_fail_early() {
        assert_eq!(status(None), "unknown");
        assert_eq!(status(Some(&json!({}))), "unknown");
        for api in ["openai-completions", "openai-responses"] {
            assert_eq!(status(Some(&json!({"api":api}))), "compatible");
        }
        let state = status(Some(&json!({"api":"anthropic-messages"})));
        assert_eq!(state, "unsupported");
        assert!(
            ensure_supported("route", state)
                .unwrap_err()
                .starts_with("NATIVE_TOOL_UNSUPPORTED:")
        );
        assert!(ensure_supported("unknown", "unknown").is_ok());
    }
}
