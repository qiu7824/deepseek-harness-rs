//! Durable, dependency-aware projection of named runtime facts.
use cordis::{Context, EventOptions, downcast_arc};
use dsh_llm::ContextSnapshotSection;
use dsh_session::{Session, SessionEvent};
use parking_lot::Mutex;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

#[path = "runtime_context_state.rs"]
mod state;
use state::{ProjectionState, owned_message};

/// Maintains only the facts supported by the current model-visible surface.
pub struct RuntimeContextProjection {
    retained: Arc<Mutex<ProjectionState>>,
}
impl RuntimeContextProjection {
    pub fn new(ctx: &Context, session: &Session) -> Self {
        let projection = Self::restore(session).expect("runtime context restoration");
        projection.attach(ctx, session);
        projection
    }
    pub(crate) fn restore(session: &Session) -> Result<Self, String> {
        let surface = session.surface()?.nodes;
        let visible: HashSet<_> = surface.iter().copied().collect();
        let mut messages = HashMap::new();
        let mut retained = ProjectionState::default();
        session.visit_events(0, None, |event| {
            if let Some(message) = owned_message(event) {
                retained.seen = true;
                if visible.contains(&event.seq.get()) {
                    messages.insert(event.seq.get(), message);
                }
            }
            Ok(true)
        })?;
        // Replacements retain their surface position, not durable sequence
        // order. Missing baseline/delta links force a complete next snapshot.
        for seq in surface {
            if let Some(message) = messages.remove(&seq) {
                retained.observe(seq, message);
            }
        }
        Ok(Self {
            retained: Arc::new(Mutex::new(retained)),
        })
    }
    pub(crate) fn attach(&self, ctx: &Context, session: &Session) {
        let identity = session.identity();
        let retained = self.retained.clone();
        let listener: Arc<cordis::Listener> = Arc::new(move |_, args| {
            let subject = downcast_arc::<Session>(&args[0]);
            let event = downcast_arc::<SessionEvent>(&args[1]);
            let retained = retained.clone();
            Box::pin(async move {
                let (Some(subject), Some(event)) = (subject, event) else {
                    return None;
                };
                if subject.identity() != identity {
                    return None;
                }
                let mut state = retained.lock();
                state.replace(&event);
                if let Some(message) = owned_message(&event) {
                    state.observe(event.seq.get(), message);
                }
                None
            })
        });
        let context = ctx.clone();
        std::thread::spawn(move || {
            futures::executor::block_on(context.on(
                "session/event",
                listener,
                EventOptions::default().global(true),
            ))
        })
        .join()
        .expect("session/event listener registration");
    }
    /// Full facts establish a baseline; subsequent messages carry only changed
    /// sections and explicit removals. Projection itself never commits state.
    pub fn project(
        &self,
        current: &str,
        sections: &[ContextSnapshotSection],
    ) -> Option<dsh_llm::UserMessage> {
        self.retained.lock().project(current, sections)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_llm::{ContentBlock, MessageSource, create_user_message};
    use dsh_session::{SurfaceIntent, SurfaceOp};
    fn sections(task: &str) -> Vec<ContextSnapshotSection> {
        vec![
            ContextSnapshotSection {
                name: "environment".into(),
                text: "stable runtime ".repeat(8000),
            },
            ContextSnapshotSection {
                name: "task".into(),
                text: task.into(),
            },
        ]
    }
    fn commit(
        session: &Session,
        projection: &RuntimeContextProjection,
        sections: &[ContextSnapshotSection],
    ) -> dsh_llm::UserMessage {
        let message = projection
            .project(
                &dsh_system_prompt::join_context_sections(sections),
                sections,
            )
            .unwrap();
        session
            .append(
                "user/message",
                serde_json::to_value(&message).unwrap(),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap();
        message
    }

    #[tokio::test]
    async fn deltas_replay_cold_and_a_user_message_replacement_invalidates_their_baseline() {
        let ctx = Context::root();
        let store = dsh_session::SessionStore::install(&ctx);
        let session = store.create(&ctx, None, None).await.unwrap();
        let projection = RuntimeContextProjection::new(&ctx, &session);
        let initial = sections("pending");
        let full = commit(&session, &projection, &initial);
        let next = sections("verified");
        let delta = commit(&session, &projection, &next);
        assert!(serde_json::to_value(&delta.source).unwrap()["contextDeltaVersion"] == 1);
        let rendered = delta.content[0].as_text().unwrap();
        assert!(rendered.contains("verified"));
        assert!(!rendered.contains("stable runtime"));
        assert!(rendered.len() < 500);
        assert!(full.content[0].as_text().unwrap().len() > 100000);
        assert!(
            projection
                .project(&dsh_system_prompt::join_context_sections(&next), &next)
                .is_none()
        );
        let restored = RuntimeContextProjection::restore(&archive(&session)).unwrap();
        assert!(
            restored
                .project(&dsh_system_prompt::join_context_sections(&next), &next)
                .is_none()
        );
        let baseline = session
            .events()
            .iter()
            .find(|event| event.data["id"].as_str() == Some(full.id.as_str()))
            .unwrap()
            .seq
            .get();
        let checkpoint = create_user_message(
            vec![ContentBlock::Text {
                text: "compacted earlier history".into(),
            }],
            MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        );
        session
            .append(
                "user/message",
                serde_json::to_value(checkpoint).unwrap(),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Replace {
                        start: baseline,
                        end: baseline,
                    },
                    source_event_seqs: Some(vec![baseline]),
                }),
            )
            .unwrap();
        for state in [
            projection,
            RuntimeContextProjection::restore(&archive(&session)).unwrap(),
        ] {
            let fresh = state
                .project(&dsh_system_prompt::join_context_sections(&next), &next)
                .expect("missing baseline requires complete facts");
            assert!(
                fresh.content[0]
                    .as_text()
                    .unwrap()
                    .contains("stable runtime")
            );
            assert!(fresh.content[0].as_text().unwrap().contains("verified"));
            assert!(serde_json::to_value(fresh.source).unwrap()["contextDeltaVersion"].is_null());
        }
        ctx.fiber.dispose().await;
    }

    #[tokio::test]
    async fn imported_delta_metadata_cannot_stand_in_for_missing_visible_facts() {
        let ctx = Context::root();
        let store = dsh_session::SessionStore::install(&ctx);
        let session = store.create(&ctx, None, None).await.unwrap();
        let projection = RuntimeContextProjection::new(&ctx, &session);
        commit(&session, &projection, &sections("first"));
        let next = sections("second");
        let current = dsh_system_prompt::join_context_sections(&next);
        let mut delta = projection.project(&current, &next).unwrap();
        delta.content = vec![ContentBlock::Text {
            text: "incomplete imported body".into(),
        }];
        session
            .append(
                "user/message",
                serde_json::to_value(delta).unwrap(),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap();
        for state in [
            projection,
            RuntimeContextProjection::restore(&archive(&session)).unwrap(),
        ] {
            let rebuilt = state
                .project(&current, &next)
                .expect("visible facts need a full baseline");
            assert_eq!(rebuilt.content[0].as_text(), Some(current.as_str()));
        }
        ctx.fiber.dispose().await;
    }

    #[tokio::test]
    async fn section_removal_and_missing_middle_delta_are_never_silently_retained() {
        let ctx = Context::root();
        let store = dsh_session::SessionStore::install(&ctx);
        let session = store.create(&ctx, None, None).await.unwrap();
        let projection = RuntimeContextProjection::new(&ctx, &session);
        commit(&session, &projection, &sections("first"));
        let second = sections("second");
        let middle = commit(&session, &projection, &second);
        let final_sections = vec![ContextSnapshotSection {
            name: "task".into(),
            text: "third".into(),
        }];
        let removal = commit(&session, &projection, &final_sections);
        assert_eq!(
            serde_json::to_value(&removal.source).unwrap()["removedSections"],
            serde_json::json!(["environment"])
        );
        assert!(
            RuntimeContextProjection::restore(&archive(&session))
                .unwrap()
                .project(
                    &dsh_system_prompt::join_context_sections(&final_sections),
                    &final_sections
                )
                .is_none()
        );
        let seq = session
            .events()
            .iter()
            .find(|event| event.data["id"] == middle.id.as_str())
            .unwrap()
            .seq
            .get();
        let replaced = create_user_message(
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
                serde_json::to_value(replaced).unwrap(),
                Some(SurfaceIntent {
                    surface_op: SurfaceOp::Replace {
                        start: seq,
                        end: seq,
                    },
                    source_event_seqs: Some(vec![seq]),
                }),
            )
            .unwrap();
        assert!(
            projection
                .project(
                    &dsh_system_prompt::join_context_sections(&final_sections),
                    &final_sections
                )
                .is_some()
        );
        assert!(
            RuntimeContextProjection::restore(&archive(&session))
                .unwrap()
                .project(
                    &dsh_system_prompt::join_context_sections(&final_sections),
                    &final_sections
                )
                .is_some()
        );
        ctx.fiber.dispose().await;
    }

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
