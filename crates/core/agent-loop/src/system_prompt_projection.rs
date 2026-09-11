//! Route-aware system prompt updates over the durable surface.
use dsh_llm::{ContentBlock, Message, MessageSource, Role, create_message};
use dsh_session::{Session, SurfaceIntent, SurfaceOp};

pub(crate) struct SystemPromptCommit {
    pub message: Message,
    pub intent: SurfaceIntent,
    pub prefix: bool,
}

fn message(text: &str) -> Message {
    create_message(
        Role::System,
        if text.is_empty() {
            Vec::new()
        } else {
            vec![ContentBlock::Text { text: text.into() }]
        },
        MessageSource::Plugin {
            plugin: "@deepseek-ai/dsh-system-prompt".into(),
            form: None,
            sections: None,
            summary: None,
            compaction_id: None,
            source_command_id: None,
        },
    )
}
fn replace(seq: u64, text: &str) -> SystemPromptCommit {
    SystemPromptCommit {
        message: message(text),
        intent: SurfaceIntent {
            surface_op: SurfaceOp::Replace {
                start: seq,
                end: seq,
            },
            source_event_seqs: Some(vec![seq]),
        },
        prefix: false,
    }
}

/// Only a continuing, capable route appends nonempty changes. Clearing,
/// capability loss or a new series empties all active tails before fixing the
/// head, so returning to a capable route cannot revive superseded instructions.
pub(crate) fn project(
    session: &Session,
    rendered: &str,
    in_history: bool,
    starts_series: bool,
) -> Result<Vec<SystemPromptCommit>, String> {
    let surface = session.surface()?;
    let nodes = session.with_events(|events| -> Result<Vec<(u64, Option<String>)>, String> {
        let mut nodes = Vec::new();
        for seq in &surface.nodes {
            let Some(event) = events
                .get(*seq as usize)
                .filter(|event| event.type_ == "system/message")
            else {
                continue;
            };
            let system: Message = serde_json::from_value(event.data["message"].clone())
                .map_err(|error| format!("invalid system node at {seq}: {error}"))?;
            if system.role != Role::System {
                return Err(format!("system node at {seq} has a non-system role"));
            }
            let text = match system.content.as_slice() {
                [] => Some(String::new()),
                [ContentBlock::Text { text }] => Some(text.clone()),
                _ => None,
            };
            nodes.push((*seq, text));
        }
        Ok(nodes)
    })?;
    let Some((head, head_text)) = nodes.first() else {
        return Ok(vec![SystemPromptCommit {
            message: message(rendered),
            intent: SurfaceIntent {
                surface_op: SurfaceOp::Append,
                source_event_seqs: None,
            },
            prefix: true,
        }]);
    };
    if !in_history || starts_series || rendered.is_empty() {
        let mut commits = nodes
            .iter()
            .skip(1)
            .filter(|(_, text)| text.as_deref() != Some(""))
            .map(|(seq, _)| replace(*seq, ""))
            .collect::<Vec<_>>();
        if head_text.as_deref() != Some(rendered) {
            commits.push(replace(*head, rendered));
        }
        return Ok(commits);
    }
    let latest = nodes
        .iter()
        .rev()
        .find(|(_, text)| text.as_deref() != Some(""))
        .unwrap_or(&nodes[0]);
    if latest.1.as_deref() == Some(rendered) {
        return Ok(Vec::new());
    }
    Ok(vec![SystemPromptCommit {
        message: message(rendered),
        intent: SurfaceIntent {
            surface_op: SurfaceOp::Append,
            source_event_seqs: None,
        },
        prefix: false,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn commit(session: &Session, text: &str, capable: bool, series: bool) {
        for update in project(session, text, capable, series).unwrap() {
            session
                .append(
                    "system/message",
                    json!({"turn":1,"step":1,"prefix":update.prefix,"message":update.message}),
                    Some(update.intent),
                )
                .unwrap();
        }
    }
    fn texts(session: &Session) -> Vec<String> {
        session
            .derive_messages()
            .unwrap()
            .iter()
            .filter(|message| message.role == Role::System)
            .flat_map(|message| message.content.iter())
            .filter_map(|block| block.as_text().map(str::to_string))
            .collect()
    }
    #[tokio::test]
    async fn append_preserves_prefix_and_normalization_cannot_revive_old_instructions() {
        let ctx = cordis::Context::root();
        let store = dsh_session::SessionStore::install(&ctx);
        let session = store.create(&ctx, None, None).await.unwrap();
        commit(&session, "A", true, true);
        let prefix = session.events()[0].clone();
        commit(&session, "B", true, false);
        assert_eq!(session.events()[0], prefix);
        assert_eq!(texts(&session), ["A", "B"]);
        assert!(project(&session, "B", true, false).unwrap().is_empty());
        commit(&session, "C", true, false);
        assert_eq!(texts(&session), ["A", "B", "C"]);
        commit(&session, "C", false, false);
        assert_eq!(texts(&session), ["C"]);
        assert!(project(&session, "C", false, false).unwrap().is_empty());
        commit(&session, "", true, false);
        assert!(texts(&session).is_empty());
        commit(&session, "D", true, true);
        assert_eq!(texts(&session), ["D"]);
        commit(&session, "E", true, false);
        assert_eq!(texts(&session), ["D", "E"]);
        commit(&session, "E", true, true);
        assert_eq!(texts(&session), ["E"]);
    }
}
