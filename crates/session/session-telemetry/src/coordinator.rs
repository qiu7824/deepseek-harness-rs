//! Capture coordinator for the telemetry capability. Rust port of
//! `packages/session/session-telemetry/src/coordinator.ts`.
//!
//! # Deviations
//!
//! - The TS module-scope `WeakMap` handoff cursor and per-session chunk
//!   tracking are keyed by session identity with `session/disposed` cleanup
//!   (the established weak-semantics pattern).
//! - `structuredClone` collapses to owned `serde_json::Value` clones.
//! - `contain` logs through the context logger (`telemetry: capture step
//!   failed: …`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cordis::{Context, Disposer, EventOptions, Listener, downcast};
use dsh_agent::AgentErrorPayload;
use dsh_session::{Session, SessionEvent};
use parking_lot::Mutex;

use crate::index::{
    AttributeValue, SessionTelemetryCapture, SessionTelemetryChannel, SessionTelemetryRecord,
    SessionTelemetrySeverity, SessionTelemetrySink,
};

/// One projected record ready for backend handoff.
struct ProjectedRecord {
    record: SessionTelemetryRecord,
    /// Ledger cursor advanced only after the backend accepts this record.
    seq: Option<u64>,
}

/// Shared capture state keyed by session identity (TS module-scope
/// `WeakMap`s).
struct CaptureState {
    handoff: Mutex<HashMap<usize, u64>>,
    chunk_seen: Mutex<HashMap<usize, HashSet<String>>>,
    adopted: Mutex<HashSet<usize>>,
}

/// Install the telemetry capture side onto a context for one backend.
pub struct SessionTelemetryCoordinator {
    ctx: Context,
    backend: Arc<dyn SessionTelemetrySink>,
    state: Arc<CaptureState>,
}

impl SessionTelemetryCoordinator {
    /// Create the coordinator; live capture registers the listener set plus
    /// the `agent/error` relay, and sweeps already-live sessions. Disposal
    /// captures shutdown markers for live-adopted sessions, then awaits the
    /// backend's `shutdown()` (a failure warns instead of throwing).
    pub fn new(
        ctx: &Context,
        backend: Arc<dyn SessionTelemetrySink>,
        capture: SessionTelemetryCapture,
    ) -> Arc<Self> {
        let state = Arc::new(CaptureState {
            handoff: Mutex::new(HashMap::new()),
            chunk_seen: Mutex::new(HashMap::new()),
            adopted: Mutex::new(HashSet::new()),
        });
        let coordinator = Arc::new(Self {
            ctx: ctx.clone(),
            backend,
            state,
        });
        if capture == SessionTelemetryCapture::Live {
            coordinator.install_live_listeners(ctx);
            if let Some(sessions) =
                ctx.get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
            {
                for session in sessions.list() {
                    coordinator.adopt(&session);
                }
            }
        }
        let coordinator_for_dispose = Arc::clone(&coordinator);
        let _ = ctx.effect(
            "telemetry capture",
            Box::pin(async move {
                Some(cordis::make_disposer(move || {
                    let coordinator = Arc::clone(&coordinator_for_dispose);
                    Box::pin(async move { coordinator.dispose().await })
                }))
            }),
        );
        coordinator
    }

    fn install_live_listeners(self: &Arc<Self>, ctx: &Context) {
        let created_coordinator = Arc::clone(self);
        let created: Arc<Listener> = Arc::new(move |_ctx, args| {
            let session = downcast::<Session>(&args[0]).cloned().expect("session arg");
            let coordinator = Arc::clone(&created_coordinator);
            Box::pin(async move {
                coordinator.adopt(&session);
                None
            })
        });
        let _ = futures::executor::block_on(ctx.on(
            "session/created",
            created,
            EventOptions::default(),
        ));

        let disposed_coordinator = Arc::clone(self);
        let disposed: Arc<Listener> = Arc::new(move |_ctx, args| {
            let session = downcast::<Session>(&args[0]).cloned().expect("session arg");
            let coordinator = Arc::clone(&disposed_coordinator);
            Box::pin(async move {
                coordinator.contain(|| {
                    if !coordinator.state.adopted.lock().remove(&session.identity()) {
                        return;
                    }
                    coordinator.deliver(
                        &session,
                        ProjectedRecord {
                            record: coordinator.redact(shutdown_record(&session)),
                            seq: None,
                        },
                    );
                });
                None
            })
        });
        let _ = futures::executor::block_on(ctx.on(
            "session/disposed",
            disposed,
            EventOptions::default(),
        ));

        let event_coordinator = Arc::clone(self);
        let event: Arc<Listener> = Arc::new(move |_ctx, args| {
            let session = downcast::<Session>(&args[0]).cloned().expect("session arg");
            let event = downcast::<SessionEvent>(&args[1])
                .cloned()
                .expect("event arg");
            let coordinator = Arc::clone(&event_coordinator);
            Box::pin(async move {
                coordinator.contain(|| coordinator.capture_event(&session, &event));
                None
            })
        });
        let _ =
            futures::executor::block_on(ctx.on("session/event", event, EventOptions::default()));

        let flush_coordinator = Arc::clone(self);
        let flush: Arc<Listener> = Arc::new(move |_ctx, args| {
            let session = downcast::<Session>(&args[0]).cloned().expect("session arg");
            let coordinator = Arc::clone(&flush_coordinator);
            Box::pin(async move {
                coordinator.contain(|| coordinator.hint_flush(&session));
                None
            })
        });
        let _ =
            futures::executor::block_on(ctx.on("session/flush", flush, EventOptions::default()));

        let error_coordinator = Arc::clone(self);
        let error: Arc<Listener> = Arc::new(move |_ctx, args| {
            let payload = cordis::downcast_arc::<Arc<AgentErrorPayload>>(&args[0])
                .expect("agent/error payload");
            let coordinator = Arc::clone(&error_coordinator);
            Box::pin(async move {
                coordinator.contain(|| {
                    coordinator.relay_agent_error(
                        &payload.agent,
                        payload.turn,
                        payload.step,
                        &payload.error,
                    )
                });
                None
            })
        });
        let _ = futures::executor::block_on(ctx.on("agent/error", error, EventOptions::default()));
    }

    async fn dispose(&self) {
        // Sessions still adopted here are alive through whole-application
        // teardown; capture the marker before the backend quiesces.
        let adopted: Vec<usize> = self.state.adopted.lock().iter().copied().collect();
        // Recover sessions from the store by identity is not possible; the
        // adopted set carries identities, so find the sessions from the
        // store list.
        if let Some(sessions) = self
            .ctx
            .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
        {
            for session in sessions.list() {
                if adopted.contains(&session.identity()) {
                    self.contain(|| {
                        self.deliver(
                            &session,
                            ProjectedRecord {
                                record: self.redact(shutdown_record(&session)),
                                seq: None,
                            },
                        );
                    });
                }
            }
        }
        if let Err(error) = self.backend.shutdown().await {
            self.ctx
                .named_logger(Some("telemetry"))
                .warn(vec![cordis::arc(format!(
                    "backend shutdown failed: {error}"
                ))]);
        }
    }

    /// Project and hand over the canonical session-log suffix after the
    /// handoff cursor, optionally stopping at an inclusive sequence
    /// boundary (TS `captureSession`).
    pub fn capture_session(&self, session: &Session, through_seq: Option<u64>) {
        let cursor = self
            .state
            .handoff
            .lock()
            .get(&session.identity())
            .map(|seq| *seq as i64)
            .unwrap_or(session.first_live_seq() as i64 - 1);
        let events = session.events();
        for event in events.iter() {
            if through_seq.is_some_and(|through| event.seq > through) {
                break;
            }
            self.contain(|| {
                if event.seq as i64 <= cursor {
                    self.track(session, event);
                } else {
                    self.capture_event(session, event);
                }
            });
        }
    }

    /// Adopt a session: replay its log THROUGH the projection from the
    /// handoff cursor, then rely on the firehose (TS `adopt`).
    pub fn adopt(&self, session: &Session) {
        if !self.state.adopted.lock().insert(session.identity()) {
            return;
        }
        self.capture_session(session, None);
    }

    /// Feed the chunk projection without handing off — the ≤cursor half of
    /// re-adoption.
    fn track(&self, session: &Session, event: &SessionEvent) {
        if event.type_ == "assistant/chunk" {
            let key = format!(
                "{}:{}",
                event.data.get("turn").and_then(|v| v.as_u64()).unwrap_or(0),
                event.data.get("step").and_then(|v| v.as_u64()).unwrap_or(0)
            );
            self.seen(session).insert(key);
        }
    }

    /// Project, redact, and hand one event to the backend.
    fn capture_event(&self, session: &Session, event: &SessionEvent) {
        if event.type_ == "assistant/chunk" {
            let key = format!(
                "{}:{}",
                event.data.get("turn").and_then(|v| v.as_u64()).unwrap_or(0),
                event.data.get("step").and_then(|v| v.as_u64()).unwrap_or(0)
            );
            let mut seen = self.seen(session);
            // Fixed chunk projection: only the first chunk of each
            // (turn, step) ships.
            if !seen.insert(key) {
                return;
            }
        }
        let record = self.redact(SessionTelemetryRecord {
            channel: SessionTelemetryChannel::Ledger,
            time: event.time,
            severity: severity_of(event),
            attributes: identity_of(session, event),
            body: event.data.clone(),
        });
        self.deliver(
            session,
            ProjectedRecord {
                record,
                seq: Some(event.seq),
            },
        );
    }

    /// Run the `session-telemetry/record` waterfall at capture time (TS
    /// `redact`). The innermost `next` passes the record through unchanged.
    ///
    /// Deviation: the waterfall is asynchronous in the Rust cordis port, so
    /// the synchronous capture contract drives it on a dedicated thread
    /// (the listeners' futures are polled with `block_on` there). A
    /// thread-pool-backed executor is a later optimization.
    fn redact(&self, record: SessionTelemetryRecord) -> SessionTelemetryRecord {
        let ctx = self.ctx.clone();
        let fallback = record.clone();
        let record_for_thread = record.clone();
        let handled = std::thread::spawn(move || {
            let args = vec![cordis::arc(record_for_thread)];
            let result = futures::executor::block_on(ctx.waterfall(
                "session-telemetry/record",
                args,
                Box::pin(async move { cordis::arc(fallback) }),
            ));
            cordis::downcast_arc::<SessionTelemetryRecord>(&result)
                .map(|record| record.as_ref().clone())
        })
        .join()
        .unwrap_or(None);
        handled.unwrap_or(record)
    }

    /// Hand one redacted record to the backend, then advance the cursor.
    fn deliver(&self, session: &Session, pending: ProjectedRecord) {
        self.backend.emit(pending.record);
        if let Some(seq) = pending.seq {
            self.state.handoff.lock().insert(session.identity(), seq);
        }
    }

    /// Forward the turn-end boundary to the backend's optional flush hint.
    fn hint_flush(&self, session: &Session) {
        if self.state.adopted.lock().contains(&session.identity()) {
            self.backend.flush();
        }
    }

    /// Relay one `agent/error` bus emission as an operational record.
    fn relay_agent_error(
        &self,
        agent: &Arc<dyn dsh_agent::Agent>,
        turn: u64,
        step: u64,
        error: &serde_json::Value,
    ) {
        let detail = error_detail(error);
        let record = self.redact(SessionTelemetryRecord {
            channel: SessionTelemetryChannel::Ops,
            time: now_ms(),
            severity: SessionTelemetrySeverity::Error,
            attributes: vec![
                (
                    "telemetry.op".to_string(),
                    AttributeValue::Str("agent-error".to_string()),
                ),
                (
                    "session.id".to_string(),
                    AttributeValue::Str(agent.session().id().as_str().to_string()),
                ),
                (
                    "agent.id".to_string(),
                    AttributeValue::Str(agent.id().as_str().to_string()),
                ),
                (
                    "error.name".to_string(),
                    AttributeValue::Str(detail.0.clone()),
                ),
                ("turn".to_string(), AttributeValue::Num(turn as f64)),
                ("step".to_string(), AttributeValue::Num(step as f64)),
            ],
            body: serde_json::json!({"name": detail.0, "message": detail.1}),
        });
        self.deliver(agent.session(), ProjectedRecord { record, seq: None });
    }

    /// Lazily create the per-session first-chunk tracking set.
    fn seen(&self, session: &Session) -> parking_lot::MappedMutexGuard<'_, HashSet<String>> {
        let identity = session.identity();
        let mut guard = self.state.chunk_seen.lock();
        if !guard.contains_key(&identity) {
            guard.insert(identity, HashSet::new());
        }
        parking_lot::MutexGuard::map(guard, |map| map.get_mut(&identity).expect("inserted"))
    }

    /// Run one capture-side step with its exception contained.
    fn contain(&self, step: impl FnOnce()) {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(step));
        if let Err(payload) = outcome {
            self.ctx
                .named_logger(Some("telemetry"))
                .warn(vec![cordis::arc(format!(
                    "capture step failed: {}",
                    render_panic(payload)
                ))]);
        }
    }
}

/// Build the per-session clean-exit marker.
fn shutdown_record(session: &Session) -> SessionTelemetryRecord {
    SessionTelemetryRecord {
        channel: SessionTelemetryChannel::Ops,
        time: now_ms(),
        severity: SessionTelemetrySeverity::Info,
        attributes: vec![
            (
                "telemetry.op".to_string(),
                AttributeValue::Str("shutdown".to_string()),
            ),
            (
                "session.id".to_string(),
                AttributeValue::Str(session.id().as_str().to_string()),
            ),
        ],
        body: serde_json::json!({"op": "shutdown"}),
    }
}

/// Map an event's own outcome flag to the pre-baked alerting severity.
fn severity_of(event: &SessionEvent) -> SessionTelemetrySeverity {
    match event.type_.as_str() {
        "tool/result" => {
            let is_error = event
                .data
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(|content| content.get(0))
                .and_then(|block| block.get("isError"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if is_error {
                SessionTelemetrySeverity::Error
            } else {
                SessionTelemetrySeverity::Info
            }
        }
        "turn/end" => {
            let is_error = event
                .data
                .get("reason")
                .and_then(|reason| reason.get("kind"))
                .and_then(|kind| kind.as_str())
                == Some("error");
            if is_error {
                SessionTelemetrySeverity::Error
            } else {
                SessionTelemetrySeverity::Info
            }
        }
        _ => SessionTelemetrySeverity::Info,
    }
}

/// Normalize an arbitrary error value into the stable operational-record
/// shape.
fn error_detail(error: &serde_json::Value) -> (String, String) {
    match error {
        serde_json::Value::String(message) => ("Error".to_string(), message.clone()),
        other => ("Error".to_string(), other.to_string()),
    }
}

/// Build the minimal identity attributes.
fn identity_of(session: &Session, event: &SessionEvent) -> Vec<(String, AttributeValue)> {
    let mut attributes = vec![
        (
            "session.id".to_string(),
            AttributeValue::Str(session.id().as_str().to_string()),
        ),
        (
            "event.type".to_string(),
            AttributeValue::Str(event.type_.clone()),
        ),
        (
            "event.seq".to_string(),
            AttributeValue::Num(event.seq as f64),
        ),
    ];
    let header = session.header();
    if let Some(cwd) = &header.cwd {
        attributes.push(("session.cwd".to_string(), AttributeValue::Str(cwd.clone())));
    }
    if let Some(parent) = &header.parent_session {
        attributes.push((
            "session.parent_id".to_string(),
            AttributeValue::Str(parent.as_str().to_string()),
        ));
    }
    if let Some(seed_length) = header.seed_length {
        attributes.push((
            "session.seed_length".to_string(),
            AttributeValue::Num(seed_length as f64),
        ));
    }
    attributes
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn render_panic(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return message.to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic".to_string()
}

#[allow(dead_code)]
fn _unused_disposer(_: Disposer) {}
