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
    message.source.plugin_name() == Some(SOURCE)
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
        let projection = Self::restore(session).expect("runtime context restoration");
        projection.attach(ctx, session);
        projection
    }

    pub(crate) fn restore(session: &Session) -> Result<Self, String> {
        let surface_nodes: std::collections::HashSet<u64> =
            session.surface()?.nodes.into_iter().collect();
        let mut retained: Option<Option<Retained>> = None;
        session.visit_events(0, None, |event| {
            if event.type_ != "user/message" {
                return Ok(true);
            }
            let source = serde_json::from_value::<MessageSource>(event.data["source"].clone());
            if !source.is_ok_and(|source| source.plugin_name() == Some(SOURCE)) {
                return Ok(true);
            }
            let Ok(message) = serde_json::from_value::<dsh_llm::UserMessage>(event.data.clone())
            else {
                return Ok(true);
            };
            if retained.is_none() {
                retained = Some(None);
            }
            if surface_nodes.contains(&event.seq.get()) {
                retained = Some(Some(Retained {
                    seq: event.seq.get(),
                    text: text_of(&message),
                }));
            }
            Ok(true)
        })?;
        Ok(Self {
            retained: Arc::new(Mutex::new(retained)),
        })
    }

    pub(crate) fn attach(&self, ctx: &Context, session: &Session) {
        let session_identity = session.identity();
        let retained_cell = Arc::clone(&self.retained);
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

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_session::{SurfaceIntent, SurfaceOp};

    fn snapshot(session: &Session, text: &str) -> u64 {
        let projection = RuntimeContextProjection::restore(session).unwrap();
        let message = projection.project(text, &[]).unwrap();
        session
            .append(
                "user/message",
                serde_json::to_value(message).unwrap(),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap()
            .seq
            .get()
    }

    fn archive(session: &Session) -> Session {
        let mut builder =
            dsh_session::event_archive::EventArchiveBuilder::new(&std::env::temp_dir()).unwrap();
        session
            .visit_events(0, None, |event| {
                builder.push(event)?;
                Ok(true)
            })
            .unwrap();
        Session::from_event_archive(
            session.id().clone(),
            builder.finish().unwrap(),
            session.header(),
            session.inherited_event_count(),
            vec![],
        )
        .unwrap()
    }

    #[test]
    fn archived_runtime_context_keeps_the_latest_snapshot_still_on_the_surface() {
        let session = Session::create(
            dsh_session::session_id("runtime-context-archive"),
            None,
            None,
            None,
        )
        .unwrap();
        let kept = snapshot(&session, "kept context");
        let shadowed = snapshot(&session, "shadowed context");
        let replacement = create_user_message(
            vec![ContentBlock::Text {
                text: "checkpoint".into(),
            }],
            MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        );
        session
            .append(
                "user/message",
                serde_json::to_value(&replacement).unwrap(),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Replace {
                        start: shadowed,
                        end: shadowed,
                    },
                    source_event_seqs: Some(vec![shadowed]),
                }),
            )
            .unwrap();
        let restored = archive(&session);
        let projection = RuntimeContextProjection::restore(&restored).unwrap();
        assert!(projection.project("kept context", &[]).is_none());
        assert!(projection.project("shadowed context", &[]).is_some());
        session
            .append(
                "user/message",
                serde_json::to_value(replacement).unwrap(),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Replace {
                        start: kept,
                        end: kept,
                    },
                    source_event_seqs: Some(vec![kept]),
                }),
            )
            .unwrap();
        let cleared = RuntimeContextProjection::restore(&archive(&session)).unwrap();
        assert!(
            cleared.project("", &[]).is_some(),
            "an overwritten snapshot needs an explicit clear notice"
        );
    }
}
