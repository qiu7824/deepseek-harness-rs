//! Durable workspace instructions from AGENTS.md / CLAUDE.md.

mod files;
mod render;

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

use cordis::{ArcValue, Context, Disposer, Listener, arc, downcast_arc};
use dsh_agent::{AgentPreStepPayload, PreStepDecision};
use dsh_llm::{
    AgentInstructionChange, ContentBlock, ContextForm, MessageSource, UserMessage,
    create_user_message,
};

pub use files::{InstructionFile, discover};
pub use render::{render, render_baseline};

pub const NAME: &str = "agent-instructions";

#[derive(Debug, Clone)]
pub struct Config {
    pub dsh_home: PathBuf,
    pub max_bytes: usize,
    pub max_source_bytes: u64,
}

fn visible_baseline(
    session: &dsh_session::Session,
) -> Option<(String, Vec<AgentInstructionChange>)> {
    session.events().iter().rev().find_map(|event| {
        if event.type_ != "user/message" {
            return None;
        }
        let message = serde_json::from_value::<UserMessage>(event.data.clone()).ok()?;
        let MessageSource::AgentInstructions {
            baseline,
            baseline_identity,
            changes,
            ..
        } = message.source
        else {
            return None;
        };
        if !baseline.unwrap_or(false) {
            return None;
        }
        Some((baseline_identity?, changes))
    })
}

fn baseline_identity(cwd: &std::path::Path, files: &[InstructionFile]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    cwd.hash(&mut hasher);
    for file in files {
        file.display_path.hash(&mut hasher);
        file.content.hash(&mut hasher);
    }
    format!("workspace:{:016x}", hasher.finish())
}

fn context_message(config: &Config, session: &dsh_session::Session) -> Option<UserMessage> {
    let cwd = session.header().cwd.as_deref().map(PathBuf::from)?;
    let previous = visible_baseline(session);
    let files = discover(&cwd, &config.dsh_home, config.max_source_bytes);
    let identity = baseline_identity(&cwd, &files);
    if previous.as_ref().map(|(identity, _)| identity) == Some(&identity) {
        return None;
    }
    let (text, changes) = if files.is_empty() {
        let (_, previous_changes) = previous.as_ref()?;
        let text = "<system-reminder>\nThis complete workspace instruction baseline replaces all earlier workspace instruction baselines. No workspace instructions are currently active.\n</system-reminder>".to_string();
        let changes = previous_changes
            .iter()
            .map(|change| AgentInstructionChange {
                action: "remove".to_string(),
                scope: change.scope.clone(),
                path: change.path.clone(),
                digest: None,
            })
            .collect();
        (text, changes)
    } else {
        let (text, represented) = render_baseline(&files, config.max_bytes, previous.is_some());
        if text.is_empty() || represented.is_empty() {
            return None;
        }
        let changes = represented
            .iter()
            .filter_map(|index| files.get(*index))
            .map(|file| AgentInstructionChange {
                action: if previous.is_some() { "replace" } else { "set" }.to_string(),
                scope: std::path::Path::new(&file.display_path)
                    .parent()
                    .map(|path| path.to_string_lossy().into_owned())
                    .filter(|path| !path.is_empty())
                    .unwrap_or_else(|| ".".to_string()),
                path: file.display_path.clone(),
                digest: None,
            })
            .collect();
        (text, changes)
    };
    Some(create_user_message(
        vec![ContentBlock::Text { text }],
        MessageSource::AgentInstructions {
            form: ContextForm::Instructions,
            baseline: Some(true),
            baseline_identity: Some(identity),
            changes,
        },
    ))
}

pub fn apply(ctx: &Context, config: Config) -> Disposer {
    let config = Arc::new(config);
    let listener: Arc<Listener> = Arc::new(move |_dispatch_ctx, args: Vec<ArcValue>| {
        let config = Arc::clone(&config);
        Box::pin(async move {
            let payload = args
                .first()
                .and_then(|value| value.downcast_ref::<AgentPreStepPayload>())
                .cloned()
                .expect("agent/pre-step payload");
            let next = downcast_arc::<cordis::NextFn>(args.last().expect("agent/pre-step next"))
                .expect("agent/pre-step next");
            let decision_value = next.call().await;
            let decision = downcast_arc::<PreStepDecision>(&decision_value)
                .expect("agent/pre-step decision")
                .as_ref()
                .clone();
            let PreStepDecision::Enter { mut messages } = decision else {
                return Some(decision_value);
            };
            if let Some(context) = context_message(&config, payload.agent.session()) {
                let last_claimed = messages
                    .iter()
                    .enumerate()
                    .filter(|(_, entered)| {
                        payload
                            .messages
                            .iter()
                            .any(|claimed| claimed.id == entered.id)
                    })
                    .map(|(index, _)| index)
                    .next_back();
                messages.insert(last_claimed.map_or(0, |index| index + 1), context);
            }
            Some(arc(PreStepDecision::Enter { messages }))
        })
    });
    futures::executor::block_on(ctx.on("agent/pre-step", listener, Default::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_source_uses_the_frontend_contract() {
        let source = MessageSource::AgentInstructions {
            form: ContextForm::Instructions,
            baseline: Some(true),
            baseline_identity: Some("x".into()),
            changes: vec![AgentInstructionChange {
                action: "set".into(),
                scope: ".".into(),
                path: "AGENTS.md".into(),
                digest: None,
            }],
        };
        let json = serde_json::to_value(source).unwrap();
        assert_eq!(json["kind"], "agent-instructions");
        assert_eq!(json["changes"][0]["path"], "AGENTS.md");
    }

    #[test]
    fn baseline_message_is_durable_and_not_repeated_after_restore() {
        let root = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        std::fs::write(root.path().join("AGENTS.md"), "repo rule").unwrap();
        let id = dsh_session::session_id("instructions-test");
        let header = dsh_session::SessionHeader {
            version: dsh_session::SESSION_FORMAT_VERSION,
            id: id.clone(),
            created_at: 0,
            cwd: Some(root.path().to_string_lossy().into_owned()),
            parent_session: None,
            seed_length: None,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        };
        let session = dsh_session::Session::create(id, None, Some(&header)).unwrap();
        let config = Config {
            dsh_home: home.path().to_path_buf(),
            max_bytes: 65_536,
            max_source_bytes: 1_048_576,
        };
        let message = context_message(&config, &session).expect("baseline");
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["source"]["kind"], "agent-instructions");
        assert_eq!(json["source"]["changes"][0]["path"], "AGENTS.md");
        assert!(
            json["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("repo rule")
        );
        session
            .append(
                "user/message",
                serde_json::to_value(message).unwrap(),
                Some(dsh_session::SurfaceIntent {
                    surface_op: dsh_session::SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap();
        assert!(context_message(&config, &session).is_none());
        std::fs::write(root.path().join("AGENTS.md"), "updated rule").unwrap();
        let replacement = context_message(&config, &session).expect("replacement");
        let replacement = serde_json::to_value(replacement).unwrap();
        assert_eq!(replacement["source"]["changes"][0]["action"], "replace");
        assert!(
            replacement["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("replaces all earlier workspace instruction baselines")
        );
        let replacement_message: UserMessage = serde_json::from_value(replacement).unwrap();
        session
            .append(
                "user/message",
                serde_json::to_value(replacement_message).unwrap(),
                Some(dsh_session::SurfaceIntent {
                    surface_op: dsh_session::SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap();
        std::fs::remove_file(root.path().join("AGENTS.md")).unwrap();
        let removal =
            serde_json::to_value(context_message(&config, &session).expect("removal")).unwrap();
        assert_eq!(removal["source"]["changes"][0]["action"], "remove");
        assert!(
            removal["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("No workspace instructions are currently active")
        );
    }
}
