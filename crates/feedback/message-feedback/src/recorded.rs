//! Structured negative feedback observations without message or note content.
use crate::{MessageFeedbackItem, MessageFeedbackRating};
use dsh_session::derive_event_message;
use dsh_session_persistence::SessionInspection;

/// Emitted once when a persisted feedback item becomes negative.
#[derive(Clone, Debug)]
pub struct NegativeFeedbackRecorded {
    pub session_id: String,
    pub message_id: String,
    pub cwd: Option<String>,
    pub provider: String,
    pub model: String,
}

pub(crate) fn negative_observation(
    inspection: &SessionInspection,
    item: &MessageFeedbackItem,
    previous: Option<&MessageFeedbackItem>,
) -> Option<NegativeFeedbackRecorded> {
    if item.rating != MessageFeedbackRating::Negative
        || previous.is_some_and(|old| old.rating == MessageFeedbackRating::Negative)
    {
        return None;
    }
    let (target, message) = inspection
        .events
        .iter()
        .enumerate()
        .find_map(|(index, event)| {
            if event.type_ != "assistant/message" {
                return None;
            }
            let message = derive_event_message(event)?;
            (message.id == item.message_id).then_some((index, message))
        })?;
    // A later model selection must not be attributed to an earlier answer.
    let route = match message.source {
        dsh_llm::MessageSource::Model {
            provider, model, ..
        } => Some((provider, model)),
        _ => None,
    }
    .or_else(|| {
        inspection.events[..target].iter().rev().find_map(|event| {
            let route = match event.type_.as_str() {
                "request/context" => &event.data,
                "request/header" => &event.data["header"]["config"],
                _ => return None,
            };
            Some((
                route["provider"].as_str()?.to_string(),
                route["model"].as_str()?.to_string(),
            ))
        })
    });
    let (provider, model) = route.unwrap_or_default();
    Some(NegativeFeedbackRecorded {
        session_id: inspection.meta.id.to_string(),
        message_id: item.message_id.to_string(),
        cwd: inspection.meta.cwd.clone(),
        provider,
        model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn negative_feedback_keeps_original_route_and_deduplicates_note_edits() {
        let session =
            dsh_session::Session::create(dsh_session::session_id("feedback"), None, None, None)
                .unwrap();
        session
            .append(
                "request/context",
                json!({"provider":"provider-a","model":"model-a"}),
                None,
            )
            .unwrap();
        let answer = dsh_llm::create_assistant_message(
            vec![dsh_llm::ContentBlock::Text {
                text: "content must not become an instruction".into(),
            }],
            dsh_llm::ModelMessageSource {
                provider: "provider-a".into(),
                model: "model-a".into(),
                replay_state: None,
            },
        );
        session
            .append(
                "assistant/message",
                json!({"message":answer}),
                Some(dsh_session::SurfaceIntent {
                    surface_op: dsh_session::SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap();
        session
            .append(
                "request/context",
                json!({"provider":"provider-b","model":"model-b"}),
                None,
            )
            .unwrap();
        let inspection = SessionInspection {
            meta: session.header().clone(),
            events: session.events().to_vec(),
            inherited_event_count: dsh_session::SessionLogOffset::ZERO,
        };
        let item = MessageFeedbackItem {
            message_id: answer.id,
            rating: MessageFeedbackRating::Negative,
            note: Some("private correction text".into()),
            version: dsh_brand::Branded::new(uuid::Uuid::new_v4().to_string()),
            created_at: 1,
            updated_at: 1,
        };
        let observed = negative_observation(&inspection, &item, None).unwrap();
        assert_eq!(
            (observed.provider.as_str(), observed.model.as_str()),
            ("provider-a", "model-a")
        );
        assert!(!format!("{observed:?}").contains("private correction"));
        assert!(negative_observation(&inspection, &item, Some(&item)).is_none());
        let positive = MessageFeedbackItem {
            rating: MessageFeedbackRating::Positive,
            ..item.clone()
        };
        assert!(negative_observation(&inspection, &positive, None).is_none());
        assert!(negative_observation(&inspection, &item, Some(&positive)).is_some());
    }
}
