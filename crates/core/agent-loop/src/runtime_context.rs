//! Durable projection state for dynamic runtime context. Rust port of
//! `packages/core/agent-loop/src/runtime-context.ts`.
//!
//! # Deviations
//!
//! - The retained cell uses a nested `Option` (`None` = no snapshot ever
//!   existed; `Some(None)` = none retained), matching the TS
//!   `{seq,text} | null | undefined` ternary.

use std::sync::Arc;

use cordis::{Context, EventOptions, downcast_arc};
use dsh_llm::{
    ContentBlock, ContextForm, ContextSnapshotSection, MessageSource, create_user_message,
};
use dsh_session::{Session, SessionEvent, is_replacement_surface_event};
use parking_lot::Mutex;

const SOURCE: &str = "@deepseek-ai/dsh-system-prompt";
const CLEARED: &str =
    "Current runtime context: none. Earlier runtime-context snapshots no longer apply.";

fn is_owned(message: &dsh_llm::UserMessage) -> bool {
    matches!(&message.source, MessageSource::Plugin { plugin, .. } if plugin == SOURCE)
}

fn text_of(message: &dsh_llm::UserMessage) -> Option<String> {
    if message.content.len() != 1 {
        return None;
    }
    match &message.content[0] {
        ContentBlock::Text { text } => Some(text.clone()),
        _ => None,
    }
}

/// One retained snapshot's identity and text.
#[derive(Debug, Clone)]
struct Retained {
    seq: u64,
    text: Option<String>,
}

/// Tracks the last retained runtime-context snapshot without owning its
/// commit.
pub struct RuntimeContextProjection {
    /// `None` means no snapshot ever existed; `Some(None)` means none is
    /// retained.
    retained: Arc<Mutex<Option<Option<Retained>>>>,
}

impl RuntimeContextProjection {
    /// Restore projection state once, then follow authoritative session
    /// events.
    pub fn new(ctx: &Context, session: &Session) -> Self {
        let initial = {
            let events = session.events();
            let surface_nodes: Vec<u64> = session
                .surface()
                .map(|surface| surface.nodes)
                .unwrap_or_default();
            let mut retained: Option<Option<Retained>> = None;
            for index in (0..events.len()).rev() {
                let event = &events[index];
                if event.type_ != "user/message" {
                    continue;
                }
                let Ok(message) =
                    serde_json::from_value::<dsh_llm::UserMessage>(event.data.clone())
                else {
                    continue;
                };
                if !is_owned(&message) {
                    continue;
                }
                if retained.is_none() {
                    retained = Some(None);
                }
                if surface_nodes.contains(&event.seq.get()) {
                    retained = Some(Some(Retained {
                        seq: event.seq.get(),
                        text: text_of(&message),
                    }));
                    break;
                }
            }
            retained
        };
        let projection = Self {
            retained: Arc::new(Mutex::new(initial)),
        };
        let session_identity = session.identity();
        let retained_cell = Arc::clone(&projection.retained);
        let listener: Arc<cordis::Listener> = Arc::new(move |_listener_ctx, args| {
            let subject = downcast_arc::<Session>(&args[0]).map(|arc| arc.as_ref().clone());
            let event = downcast_arc::<SessionEvent>(&args[1]).map(|arc| arc.as_ref().clone());
            let retained = Arc::clone(&retained_cell);
            Box::pin(async move {
                let (Some(subject), Some(event)) = (subject, event) else {
                    return None;
                };
                if subject.identity() != session_identity {
                    return None;
                }
                if event.type_ == "user/message" {
                    if let Ok(message) =
                        serde_json::from_value::<dsh_llm::UserMessage>(event.data.clone())
                    {
                        if is_owned(&message) {
                            *retained.lock() = Some(Some(Retained {
                                seq: event.seq.get(),
                                text: text_of(&message),
                            }));
                        }
                    }
                } else if is_replacement_surface_event(&event)
                    && event.source_event_seqs.as_ref().is_some_and(|seqs| {
                        retained
                            .lock()
                            .as_ref()
                            .and_then(|retained| retained.as_ref())
                            .is_some_and(|retained| seqs.contains(&retained.seq))
                    })
                {
                    *retained.lock() = Some(None);
                }
                None
            })
        });
        // The listener registers through the caller context (the TS `ctx.on`
        // is synchronous); drive the async registration on a dedicated thread.
        let ctx_for_listener = ctx.clone();
        std::thread::spawn(move || {
            futures::executor::block_on(ctx_for_listener.on(
                "session/event",
                listener,
                EventOptions::default().global(true),
            ));
        })
        .join()
        .expect("session/event listener registration");
        projection
    }

    /// Create an uncommitted snapshot only when the retained value differs.
    pub fn project(
        &self,
        current: &str,
        sections: &[ContextSnapshotSection],
    ) -> Option<dsh_llm::UserMessage> {
        let retained = self.retained.lock();
        if retained.as_ref().is_none() && current.is_empty() {
            return None;
        }
        let snapshot = if current.is_empty() {
            CLEARED.to_string()
        } else {
            current.to_string()
        };
        if retained
            .as_ref()
            .and_then(|retained| retained.as_ref())
            .and_then(|retained| retained.text.as_deref())
            == Some(snapshot.as_str())
        {
            return None;
        }
        let source = if sections.is_empty() {
            MessageSource::Plugin {
                plugin: SOURCE.to_string(),
                form: None,
                sections: None,
                summary: None,
                compaction_id: None,
                source_command_id: None,
            }
        } else {
            MessageSource::Plugin {
                plugin: SOURCE.to_string(),
                form: Some(ContextForm::Snapshot),
                sections: Some(sections.to_vec()),
                summary: None,
                compaction_id: None,
                source_command_id: None,
            }
        };
        Some(create_user_message(
            vec![ContentBlock::Text { text: snapshot }],
            source,
        ))
    }
}
