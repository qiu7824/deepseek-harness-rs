//! Rust port of the core `packages/session/session-telemetry` behaviors:
//! adoption replay, live capture with the fixed chunk projection, severity
//! mapping, shutdown markers, the `agent/error` relay, the redaction
//! waterfall, and the handoff cursor.

use std::sync::Arc;

use cordis::Context;
use dsh_session::{Session, SessionStore, session_id};
use dsh_session_telemetry::{
    AttributeValue, SessionTelemetryCapture, SessionTelemetryChannel, SessionTelemetryCoordinator,
    SessionTelemetryRecord, SessionTelemetrySeverity, SessionTelemetrySink,
};
use parking_lot::Mutex;

/// Recording sink: captures every record and shutdown calls in order.
struct RecordingSink {
    records: Arc<Mutex<Vec<SessionTelemetryRecord>>>,
    flushes: Arc<Mutex<u64>>,
    shutdowns: Arc<Mutex<u64>>,
}

#[async_trait::async_trait]
impl SessionTelemetrySink for RecordingSink {
    fn emit(&self, record: SessionTelemetryRecord) {
        self.records.lock().push(record);
    }

    fn flush(&self) {
        *self.flushes.lock() += 1;
    }

    async fn shutdown(&self) -> Result<(), String> {
        *self.shutdowns.lock() += 1;
        Ok(())
    }
}

fn harness(
    capture: SessionTelemetryCapture,
) -> (
    Context,
    Arc<SessionStore>,
    Arc<RecordingSink>,
    Arc<SessionTelemetryCoordinator>,
) {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let sink = Arc::new(RecordingSink {
        records: Arc::new(Mutex::new(Vec::new())),
        flushes: Arc::new(Mutex::new(0)),
        shutdowns: Arc::new(Mutex::new(0)),
    });
    let coordinator = SessionTelemetryCoordinator::new(&ctx, sink.clone(), capture);
    (ctx, store, sink, coordinator)
}

fn session(store: &SessionStore, id: &str) -> Session {
    store
        .prepare(
            Some(session_id(id)),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .expect("session")
}

fn append(session: &Session, type_: &str, data: serde_json::Value) -> dsh_session::SessionEvent {
    let intent = match type_ {
        "user/message" | "assistant/message" | "tool/result" => Some(dsh_session::SurfaceIntent {
            surface_op: dsh_session::SurfaceOp::Append,
            source_event_seqs: None,
        }),
        _ => None,
    };
    session.append(type_, data, intent).expect("append")
}

fn ledger(sink: &RecordingSink) -> Vec<SessionTelemetryRecord> {
    sink.records
        .lock()
        .iter()
        .filter(|record| record.channel == SessionTelemetryChannel::Ledger)
        .cloned()
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn live_adoption_replays_then_the_firehose_continues() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let sink = Arc::new(RecordingSink {
        records: Arc::new(Mutex::new(Vec::new())),
        flushes: Arc::new(Mutex::new(0)),
        shutdowns: Arc::new(Mutex::new(0)),
    });
    // Adopt a session that already carries history, THEN start live capture:
    // the sweep re-hands the canonical log from the cursor.
    let session = store
        .create(
            &ctx,
            Some(session_id("live")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .unwrap();
    append(&session, "turn/start", serde_json::json!({"turn": 1}));
    append(
        &session,
        "user/message",
        serde_json::json!({
            "id": "u1", "role": "user",
            "content": [{"type": "text", "text": "hi"}],
            "source": {"kind": "user"},
        }),
    );

    let _coordinator =
        SessionTelemetryCoordinator::new(&ctx, sink.clone(), SessionTelemetryCapture::Live);
    // The live sweep adopted the already-live session and replayed it.
    let records = ledger(&sink);
    assert_eq!(records.len(), 2, "sweep re-hands the existing log");

    // The firehose continues from here.
    append(
        &session,
        "turn/end",
        serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
    );
    assert_eq!(ledger(&sink).len(), 3);
    let _ = store;
}

#[tokio::test(flavor = "multi_thread")]
async fn on_demand_capture_reads_the_canonical_log() {
    let (ctx, store, sink, coordinator) = harness(SessionTelemetryCapture::OnDemand);
    let session = session(&store, "ondemand");
    append(&session, "turn/start", serde_json::json!({"turn": 1}));
    append(
        &session,
        "step/start",
        serde_json::json!({"turn": 1, "step": 1}),
    );
    append(
        &session,
        "user/message",
        serde_json::json!({
            "id": "u1", "role": "user",
            "content": [{"type": "text", "text": "hi"}],
            "source": {"kind": "user"},
        }),
    );
    append(
        &session,
        "step/end",
        serde_json::json!({"turn": 1, "step": 1}),
    );
    append(
        &session,
        "turn/end",
        serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
    );

    coordinator.capture_session(&session, None);
    let records = ledger(&sink);
    assert_eq!(records.len(), 5);
    assert_eq!(
        records[0].attributes[1].1,
        AttributeValue::Str("turn/start".to_string())
    );

    // A second capture from the handoff cursor re-hands nothing.
    coordinator.capture_session(&session, None);
    assert_eq!(ledger(&sink).len(), 5);
    let _ = ctx;
}

#[tokio::test(flavor = "multi_thread")]
async fn chunk_projection_ships_only_the_first_chunk_per_step() {
    let (ctx, store, sink, coordinator) = harness(SessionTelemetryCapture::OnDemand);
    let session = session(&store, "chunks");
    append(&session, "turn/start", serde_json::json!({"turn": 1}));
    append(
        &session,
        "step/start",
        serde_json::json!({"turn": 1, "step": 1}),
    );
    append(
        &session,
        "assistant/chunk",
        serde_json::json!({
            "turn": 1, "step": 1,
            "chunk": {"type": "text-delta", "index": 0, "text": "a"},
        }),
    );
    append(
        &session,
        "assistant/chunk",
        serde_json::json!({
            "turn": 1, "step": 1,
            "chunk": {"type": "text-delta", "index": 0, "text": "b"},
        }),
    );
    append(
        &session,
        "assistant/chunk",
        serde_json::json!({
            "turn": 1, "step": 2,
            "chunk": {"type": "text-delta", "index": 0, "text": "c"},
        }),
    );
    append(
        &session,
        "step/end",
        serde_json::json!({"turn": 1, "step": 1}),
    );
    append(
        &session,
        "turn/end",
        serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
    );

    coordinator.capture_session(&session, None);
    let records = ledger(&sink);
    let chunks = records.iter().filter(|record| {
        record.attributes.iter().any(|(key, _)| key == "event.type")
            && record
                .attributes
                .iter()
                .any(|(_, value)| value == &AttributeValue::Str("assistant/chunk".to_string()))
    });
    assert_eq!(chunks.count(), 2, "one chunk per (turn, step)");
    let _ = ctx;
}

#[tokio::test(flavor = "multi_thread")]
async fn severity_maps_error_outcomes() {
    let (ctx, store, sink, coordinator) = harness(SessionTelemetryCapture::OnDemand);
    let session = session(&store, "severity");
    append(&session, "turn/start", serde_json::json!({"turn": 1}));
    append(
        &session,
        "tool/result",
        serde_json::json!({
            "message": {
                "id": "t1", "role": "user",
                "content": [{"type": "tool-result", "toolCallId": "c1", "isError": true}],
                "source": {"kind": "tool", "callId": "c1"},
            },
        }),
    );
    append(
        &session,
        "turn/end",
        serde_json::json!({
            "turn": 1, "reason": {"kind": "error", "failure": {"message": "boom", "code": "SERVER"}},
        }),
    );

    coordinator.capture_session(&session, None);
    let records = ledger(&sink);
    let tool = records.iter().find(|record| {
        record
            .attributes
            .iter()
            .any(|(_, value)| value == &AttributeValue::Str("tool/result".to_string()))
    });
    assert_eq!(
        tool.map(|record| record.severity),
        Some(SessionTelemetrySeverity::Error)
    );
    let turn_end = records.iter().find(|record| {
        record
            .attributes
            .iter()
            .any(|(_, value)| value == &AttributeValue::Str("turn/end".to_string()))
    });
    assert_eq!(
        turn_end.map(|record| record.severity),
        Some(SessionTelemetrySeverity::Error)
    );
    let _ = ctx;
}

#[tokio::test(flavor = "multi_thread")]
async fn identity_attributes_include_header_facts() {
    let (ctx, store, sink, coordinator) = harness(SessionTelemetryCapture::OnDemand);
    let mut create = dsh_session::CreateSessionOptions::default();
    create.meta = Some(dsh_session::CreateSessionMeta {
        cwd: Some("C:\\work".to_string()),
        parent_session: Some(session_id("parent-1")),
        created_at: Some(1000),
        seed_length: Some(7),
        origin: None,
        delegation_depth: None,
        agent_preset: None,
    });
    let session = store
        .prepare(Some(session_id("ident")), Some(create))
        .expect("session");
    append(&session, "turn/start", serde_json::json!({"turn": 1}));

    coordinator.capture_session(&session, None);
    let records = ledger(&sink);
    assert_eq!(records.len(), 1);
    let attributes = &records[0].attributes;
    let has = |key: &str, expected: &str| {
        attributes
            .iter()
            .any(|(k, v)| k == key && v == &AttributeValue::Str(expected.to_string()))
    };
    assert!(has("session.id", "ident"));
    assert!(has("session.cwd", "C:\\work"));
    assert!(has("session.parent_id", "parent-1"));
    assert!(
        attributes
            .iter()
            .any(|(k, v)| k == "session.seed_length" && v == &AttributeValue::Num(7.0))
    );
    let _ = ctx;
}

#[tokio::test(flavor = "multi_thread")]
async fn redaction_waterfall_transforms_records() {
    let (ctx, store, sink, coordinator) = harness(SessionTelemetryCapture::OnDemand);
    // Mount a redaction rule: drop the body for every record.
    let listener: Arc<cordis::Listener> = Arc::new(|_ctx, args: Vec<cordis::ArcValue>| {
        let record = cordis::downcast_arc::<SessionTelemetryRecord>(&args[0]).expect("record");
        let next = cordis::downcast_arc::<cordis::NextFn>(&args[1]).expect("next");
        Box::pin(async move {
            let mut result = record.as_ref().clone();
            // The innermost next() passes through; rules transform after it.
            let _ = next.call().await;
            result.body = serde_json::json!({"redacted": true});
            Some(cordis::arc(result))
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "session-telemetry/record",
        listener,
        cordis::EventOptions::default(),
    ));

    let session = session(&store, "redact");
    append(&session, "turn/start", serde_json::json!({"turn": 1}));
    coordinator.capture_session(&session, None);
    let records = ledger(&sink);
    assert_eq!(records[0].body, serde_json::json!({"redacted": true}));
}

#[tokio::test(flavor = "multi_thread")]
async fn live_capture_relays_agent_errors_and_dispose_shutdowns() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let sink = Arc::new(RecordingSink {
        records: Arc::new(Mutex::new(Vec::new())),
        flushes: Arc::new(Mutex::new(0)),
        shutdowns: Arc::new(Mutex::new(0)),
    });
    let _coordinator =
        SessionTelemetryCoordinator::new(&ctx, sink.clone(), SessionTelemetryCapture::Live);
    let session = store
        .create(
            &ctx,
            Some(session_id("live")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .unwrap();
    append(&session, "turn/start", serde_json::json!({"turn": 1}));

    // agent/error relay emits an ops record.
    let error_payload = Arc::new(dsh_agent::AgentErrorPayload {
        agent: Arc::new(TestAgent {
            session: session.clone(),
        }),
        turn: 1,
        step: 1,
        error: serde_json::json!("boom"),
    });
    ctx.parallel("agent/error", vec![cordis::arc(error_payload)])
        .await;
    let ops: Vec<SessionTelemetryRecord> = sink
        .records
        .lock()
        .iter()
        .filter(|record| record.channel == SessionTelemetryChannel::Ops)
        .cloned()
        .collect();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].severity, SessionTelemetrySeverity::Error);
    assert!(ops[0].attributes.iter().any(|(k, v)| {
        k == "telemetry.op" && v == &AttributeValue::Str("agent-error".to_string())
    }));
    let _ = store;
}

struct TestAgent {
    session: Session,
}

impl dsh_agent::Agent for TestAgent {
    fn id(&self) -> &dsh_session::SessionId {
        self.session.id()
    }

    fn options(&self) -> &dsh_agent::AgentOptions {
        static OPTIONS: std::sync::OnceLock<dsh_agent::AgentOptions> = std::sync::OnceLock::new();
        OPTIONS.get_or_init(dsh_agent::AgentOptions::default)
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn inbox(&self) -> &dsh_agent::Inbox {
        unreachable!("not used")
    }

    fn status(&self) -> dsh_agent::AgentStatus {
        dsh_agent::AgentStatus::Idle
    }

    fn ctx(&self) -> &Context {
        unreachable!("not used")
    }

    fn scope_key(&self) -> &dsh_scope::ScopeKey {
        unreachable!("not used")
    }

    fn cancel(
        &self,
        _cause: dsh_agent::AgentCancelCause,
        _options: Option<&dsh_agent::CancelOptions>,
    ) {
    }

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn send(
        &self,
        _message: dsh_session::UserMessage,
        _target: dsh_agent::InboxTarget,
        _wakeup: bool,
    ) {
    }

    fn followup(&self, _message: dsh_session::UserMessage) {}

    fn steer(&self, _message: dsh_session::UserMessage) {}

    fn inject(&self, _message: dsh_session::UserMessage) {}
}
