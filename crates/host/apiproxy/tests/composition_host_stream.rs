//! Composition-layer host stream over the real fetch carrier: session
//! added/removed frames and the verbatim remote-event wrapper.

use std::sync::Arc;

use cordis::Context;
use dsh_agent::{
    Agent, AgentOptions, AgentStatus, AgentStatusPayload, CancelOptions, Inbox, InboxTarget,
};
use dsh_host_apiproxy::{
    ApiProxyDefaults, ApiProxyService, Body, CarrierRequest, to_fetch_handler,
};
use dsh_llm::UserMessage;
use dsh_scope::ScopeKey;
use dsh_session::{
    AgentCancelCause, CreateSessionMeta, CreateSessionOptions, Session, SessionId, SessionStore,
    session_id,
};

struct StatusAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope: ScopeKey,
}

impl Agent for StatusAgent {
    fn id(&self) -> &SessionId {
        &self.id
    }
    fn options(&self) -> &AgentOptions {
        static OPTIONS: std::sync::OnceLock<AgentOptions> = std::sync::OnceLock::new();
        OPTIONS.get_or_init(AgentOptions::default)
    }
    fn session(&self) -> &Session {
        &self.session
    }
    fn inbox(&self) -> &Inbox {
        &self.inbox
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Idle
    }
    fn ctx(&self) -> &Context {
        &self.ctx
    }
    fn scope_key(&self) -> &ScopeKey {
        &self.scope
    }
    fn cancel(&self, _cause: AgentCancelCause, _options: Option<&CancelOptions>) {}
    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }
    fn send(&self, _message: UserMessage, _target: InboxTarget, _wakeup: bool) {}
    fn followup(&self, _message: UserMessage) {}
    fn steer(&self, _message: UserMessage) {}
    fn inject(&self, _message: UserMessage) {}
}

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

async fn open_host(
    handler: &dsh_host_apiproxy::FetchHandler,
) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<Vec<u8>, String>> + Send>> {
    let response = handler
        .handle(CarrierRequest {
            method: http::Method::GET,
            path: "/api/events.host".to_string(),
            query: vec![],
            headers: vec![],
            body: None,
        })
        .await;
    assert_eq!(response.status(), http::StatusCode::OK);
    let Body::Stream(stream) = response.into_body() else {
        panic!("SSE answers are stream bodies");
    };
    stream
}

fn frame_payload(bytes: Vec<u8>) -> serde_json::Value {
    let text = String::from_utf8(bytes).expect("utf8");
    serde_json::from_str(
        text.lines()
            .find(|line| line.starts_with("data: "))
            .expect("data line")
            .trim_start_matches("data: "),
    )
    .expect("frame json")
}

#[test]
fn the_host_stream_forwards_session_added_and_remote_events() {
    run(async {
        let ctx = Context::root();
        let sessions = SessionStore::install(&ctx);
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        let handler = to_fetch_handler(service);

        let stream = open_host(&handler).await;
        let mut frames = Box::pin(futures::StreamExt::take(stream, 5));
        use futures::StreamExt;
        let first = frames
            .next()
            .await
            .expect("open comment")
            .expect("open stream success");
        let text = String::from_utf8(first).expect("utf8");
        assert!(text.starts_with(": connected\n\n"), "{text}");

        // Let the spawned listener registration settle (thirteen cordis
        // `on` awaits run in a spawned task; the mux stream registers one
        // and needs sixteen yields, so give this one headroom).
        for _ in 0..128 {
            tokio::task::yield_now().await;
        }

        // A session create rides host/session-added (the store announces on
        // entry).
        let _session = sessions
            .create(
                &ctx,
                Some(session_id("host-1")),
                Some(CreateSessionOptions {
                    meta: Some(CreateSessionMeta {
                        cwd: Some("D:\\proj".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            )
            .await
            .expect("session");

        let added = frames
            .next()
            .await
            .expect("session-added frame")
            .expect("session-added success");
        let payload = frame_payload(added);
        assert_eq!(payload["method"], "host/session-added");
        assert_eq!(payload["payload"]["type"], "host/session-added");
        assert_eq!(payload["payload"]["sessionId"], "host-1");
        assert_eq!(payload["payload"]["blank"], true);
        assert_eq!(payload["payload"]["cwd"], "D:\\proj");

        let agent: Arc<dyn Agent> = Arc::new(StatusAgent {
            id: _session.id().clone(),
            inbox: Inbox::new(&_session, Default::default()).expect("inbox"),
            session: _session.clone(),
            ctx: ctx.clone(),
            scope: ScopeKey::new(),
        });
        for status in [AgentStatus::Running, AgentStatus::Idle] {
            let _ = ctx.emit(
                "agent/status",
                vec![cordis::arc(AgentStatusPayload {
                    agent: agent.clone(),
                    status,
                })],
            );
            let status_frame = frames
                .next()
                .await
                .expect("status frame")
                .expect("status success");
            let payload = frame_payload(status_frame);
            assert_eq!(payload["method"], "host/session-status");
            assert_eq!(payload["payload"]["sessionId"], "host-1");
            assert_eq!(
                payload["payload"]["running"],
                status == AgentStatus::Running
            );
        }

        // An allowlisted host event rides one verbatim wrapper frame.
        let _ = ctx.emit(
            "llm/adapters-updated",
            vec![cordis::arc(
                serde_json::json!({ "provider": "deepseek-official" }),
            )],
        );
        let remote = frames
            .next()
            .await
            .expect("remote-event frame")
            .expect("remote-event success");
        let payload = frame_payload(remote);
        assert_eq!(payload["method"], "host/remote-event");
        assert_eq!(payload["payload"]["type"], "host/remote-event");
        assert_eq!(payload["payload"]["event"], "llm/adapters-updated");
        assert_eq!(
            payload["payload"]["args"][0]["provider"],
            "deepseek-official"
        );
    });
}
