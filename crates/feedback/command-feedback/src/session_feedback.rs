//! Human-authored Session remarks; recording never starts model work.
use cordis::{Context, Service};
use dsh_session::{SessionStore, session_id};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub const FEEDBACK_CATEGORIES: &[&str] = &[
    "task-result",
    "instruction-following",
    "product-interaction",
    "service-stability",
    "resource-cost",
    "security-privacy-permission",
    "other",
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionFeedbackRecordRequest {
    pub session_id: String,
    pub text: Option<String>,
    pub category: Option<String>,
    /// Optional Rust RPC extension for retries after an uncertain response.
    pub request_id: Option<String>,
}

pub struct SessionFeedbackService {
    sessions: Arc<SessionStore>,
}
impl Service for SessionFeedbackService {
    fn service_name(&self) -> &'static str {
        "sessionFeedback"
    }
}
impl SessionFeedbackService {
    pub fn install(ctx: &Context) {
        if ctx
            .get_typed::<Arc<Self>>("sessionFeedback", false)
            .is_some()
        {
            return;
        }
        if let Some(sessions) = ctx.get_typed::<Arc<SessionStore>>("sessions", false) {
            ctx.register_service(Arc::new(Self {
                sessions: sessions.as_ref().clone(),
            }));
        }
    }
    pub fn record(&self, request: &SessionFeedbackRecordRequest) -> Value {
        let fail =
            |code: &str, message: &str| json!({"ok":false,"error":{"code":code,"message":message}});
        if request
            .category
            .as_deref()
            .is_some_and(|category| !FEEDBACK_CATEGORIES.contains(&category))
        {
            return fail("invalid-feedback", "Unknown feedback category");
        }
        let text = request.text.as_deref().unwrap_or("").trim();
        if text.chars().count() > 20_000 {
            return fail("invalid-feedback", "Feedback text is too long");
        }
        if request.request_id.as_deref().is_some_and(|id| {
            id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        }) {
            return fail("invalid-feedback", "Invalid feedback request id");
        }
        let Some(session) = self.sessions.get(&session_id(&request.session_id)) else {
            return fail("session-not-found", "Session is not loaded");
        };
        let mut data = json!({});
        if !text.is_empty() {
            data["text"] = json!(text);
        }
        if let Some(category) = &request.category {
            data["category"] = json!(category);
        }
        if let Some(id) = &request.request_id {
            data["requestId"] = json!(id);
        }
        let mut conflict = false;
        let written = session.append_if("feedback/record", data.clone(), None, |events| {
            if let Some(id) = &request.request_id {
                if let Some(previous) = events.iter().rev().find(|event| {
                    event.type_ == "feedback/record"
                        && event.data["requestId"].as_str() == Some(id.as_str())
                }) {
                    conflict = previous.data != data;
                    return false;
                }
            }
            true
        });
        match written {
            Err(_) => fail("feedback-record-failed", "Could not record feedback"),
            Ok(_) if conflict => fail(
                "feedback-request-conflict",
                "This feedback request id already describes another remark",
            ),
            Ok(_) => json!({"ok":true,"value":{"recorded":true}}),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn session_remarks_are_validated_deduplicated_and_do_not_start_turns() {
        let ctx = Context::root();
        let sessions = SessionStore::install(&ctx);
        let session = sessions
            .create(&ctx, Some(session_id("feedback-fixture")), None)
            .await
            .unwrap();
        let service = SessionFeedbackService { sessions };
        let mut request = SessionFeedbackRecordRequest {
            session_id: "feedback-fixture".into(),
            text: Some("  Details  ".into()),
            category: Some("service-stability".into()),
            request_id: Some("request-1".into()),
        };
        assert_eq!(service.record(&request)["value"]["recorded"], true);
        assert_eq!(service.record(&request)["value"]["recorded"], true);
        let events = session.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].type_, "feedback/record");
        assert_eq!(events[0].data["text"], "Details");
        request.text = Some("changed".into());
        assert_eq!(
            service.record(&request)["error"]["code"],
            "feedback-request-conflict"
        );
        request.request_id = Some("request-2".into());
        request.category = Some("invalid".into());
        assert_eq!(
            service.record(&request)["error"]["code"],
            "invalid-feedback"
        );
        request.category = None;
        request.session_id = "other-session".into();
        assert_eq!(
            service.record(&request)["error"]["code"],
            "session-not-found"
        );
        assert_eq!(session.events().len(), 1);
    }
}
