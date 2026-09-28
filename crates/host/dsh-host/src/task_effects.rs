//! Classify declared observation tools, never infer safety from arbitrary code.
use serde_json::Value;

pub(crate) fn read_only(name: &str, arguments: &Value) -> bool {
    matches!(
        name,
        "read"
            | "read_file"
            | "read_image"
            | "read_video"
            | "office_render"
            | "list_directory"
            | "glob"
            | "grep"
            | "environment_probe"
            | "environment_validate"
            | "consult_model"
            | "web_search"
            | "web_fetch"
            | "job_output"
            | "job_list"
    ) || name == "agent_team" && matches!(arguments["action"].as_str(), Some("status" | "wait"))
        || name == "workspace_scratch"
            && matches!(
                arguments["action"].as_str(),
                Some("read" | "list" | "inspect")
            )
        || name == "computer_use"
            && matches!(
                arguments["action"].as_str(),
                Some(
                    "status"
                        | "capture"
                        | "list_sessions"
                        | "list_windows"
                        | "list_tabs"
                        | "ax_state"
                )
            )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn failed_browser_observation_is_not_an_unknown_write() {
        for action in [
            "status",
            "capture",
            "list_sessions",
            "list_windows",
            "list_tabs",
            "ax_state",
        ] {
            assert!(read_only(
                "computer_use",
                &json!({"action":action,"sessionId":"not-started"})
            ));
        }
    }
    #[test]
    fn mutations_and_unrecognized_actions_remain_writes() {
        for action in [
            "start",
            "navigate",
            "click",
            "type",
            "close",
            "video_frame",
            "unknown",
        ] {
            assert!(!read_only("computer_use", &json!({"action":action})));
        }
        assert!(!read_only("computer_use", &json!({})));
        assert!(!read_only(
            "computer_use_js",
            &json!({"code":"return document.title"})
        ));
        assert!(!read_only("execute_native", &json!({"program":"echo"})));
        assert!(!read_only(
            "workspace_scratch",
            &json!({"action":"promote"})
        ));
    }
}
