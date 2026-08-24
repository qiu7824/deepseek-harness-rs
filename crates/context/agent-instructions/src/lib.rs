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
