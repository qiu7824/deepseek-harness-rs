//! Host-spine boot integration: the shared composition plus the binary's
//! report contract, exercised in-process.
//!
//! # Deviations
//!
//! - The composition nests `futures::executor::block_on` for synchronous
//!   installers (SQLite open, inject fibers); the tests run on a
//!   multi-threaded runtime so those nested executors can always make
//!   progress (a current-thread runtime can deadlock).

use std::sync::Arc;

use cordis::Context;
use dsh_agent::{Agent, AgentOptions, AgentStatus, Inbox};
use dsh_host::{compose_host, mount_companions};
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionId, session_id};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn raw_http(port: u16, request: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("host webserver accepts TCP");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("request written");
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.expect("response read");
    let text = String::from_utf8(bytes).expect("HTTP response is utf8");
    let (head, body) = text.split_once("\r\n\r\n").expect("HTTP response head");
    let status = head
        .split_whitespace()
        .nth(1)
        .expect("status code")
        .parse::<u16>()
        .expect("numeric status");
    (status, body.to_string())
}

async fn raw_http_prefix(port: u16, request: &str, needle: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("host webserver accepts TCP");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("request written");
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let read = stream.read(&mut chunk).await.expect("response prefix read");
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..read]);
            if String::from_utf8_lossy(&bytes).contains(needle) {
                break;
            }
        }
    })
    .await
    .expect("response prefix timed out");
    String::from_utf8(bytes).expect("HTTP response is utf8")
}

struct CommandProbeAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope_key: ScopeKey,
}

impl CommandProbeAgent {
    fn new() -> Arc<dyn Agent> {
        let id = session_id("host-goal-command-probe");
        let session = Session::create(id.clone(), None, None).expect("probe session");
        let inbox = Inbox::new(&session, Default::default()).expect("probe inbox");
        Arc::new(Self {
            id,
            session,
            inbox,
            ctx: Context::root(),
            scope_key: ScopeKey::new(),
        })
    }
}

impl Agent for CommandProbeAgent {
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
        &self.scope_key
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_host_mounts_the_goal_domain_and_human_command() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    assert!(
        ctx.get_typed::<Arc<dsh_goal::GoalService>>("goals", false)
            .is_some(),
        "the production host must expose the persisted goal domain"
    );
    let probe = CommandProbeAgent::new();
    assert!(
        spine.commands.find(&probe, "goal").is_some(),
        "the production host must register /goal"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_host_mounts_both_goal_invariant_companions() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    mount_companions(&spine);

    let goal_duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dsh_goal::invariant::apply(&ctx)
    }));
    assert!(
        goal_duplicate.is_err(),
        "goal invariant package must already be reserved"
    );

    let command_duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dsh_command_goal::invariant::apply(&ctx)
    }));
    assert!(
        command_duplicate.is_err(),
        "command-goal invariant package must already be reserved"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn composes_the_core_spine_and_boots_a_report() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    mount_companions(&spine);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    // Every core service resolves through its registered name.
    assert!(
        ctx.get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
            .is_some()
    );
    assert!(
        ctx.get_typed::<Arc<dsh_agent::AgentRegistry>>("agents", false)
            .is_some()
    );
    assert!(
        ctx.get_typed::<Arc<dsh_system_prompt::SystemPrompt>>("systemPrompt", false)
            .is_some()
    );
    assert!(
        ctx.get_typed::<Arc<dsh_tools::ToolRuntime>>("tools", false)
            .is_some()
    );
    assert!(
        ctx.get_typed::<Arc<dsh_user_approval::ApprovalService>>("approval", false)
            .is_some()
    );
    assert!(
        ctx.get_typed::<Arc<dsh_agent_presets::AgentPresets>>("agentPresets", false)
            .is_some()
    );

    let report = dsh_host::boot_report(&spine).await.expect("report");
    assert_eq!(report["session"]["id"], serde_json::json!("host-boot"));
    assert_eq!(report["session"]["seq"], serde_json::json!(1));
    assert!(
        report["services"]
            .as_array()
            .is_some_and(|items| items.len() == 13)
    );
    assert!(report["services"].as_array().is_some_and(|items| {
        items.iter().any(|name| name == "goals") && items.iter().any(|name| name == "agentPresets")
    }));
    // The durability + FTS5 probe observes both the live and the
    // persisted-only corpus; the preset roster serves the shipped presets.
    assert_eq!(
        report["probe"]["flushAcknowledged"],
        serde_json::json!(true)
    );
    assert_eq!(report["probe"]["liveSearchHits"], serde_json::json!(1));
    assert_eq!(report["probe"]["persistedSearchHits"], serde_json::json!(1));
    assert!(
        report["probe"]["presetCount"]
            .as_u64()
            .is_some_and(|count| count >= 4),
        "shipped presets discovered: {report}"
    );

    // A second report over the same composition reuses the boot session id
    // (the store rejects duplicates), so the spine reports once per process.
    let duplicate = dsh_host::boot_report(&spine).await;
    assert!(duplicate.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_created_through_the_spine_are_live() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    let session = spine
        .sessions
        .create(
            &ctx,
            Some(session_id("spine-session")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    assert!(spine.sessions.get(&session_id("spine-session")).is_some());
    assert_eq!(session.id(), &session_id("spine-session"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_respond_is_mounted_on_the_real_host_http_port() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    let body = serde_json::json!({
        "type": "client-response",
        "rpcId": "not-pending-over-http",
        "result": { "ok": true, "value": {} }
    })
    .to_string();
    let request = format!(
        "POST /api/respond HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    let (status, response) = raw_http(spine.web_server.port(), &request).await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response).expect("receipt JSON"),
        serde_json::json!({ "accepted": false, "reason": "not-pending" })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_rejects_a_rebound_host_on_the_real_http_port() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    let body = serde_json::json!({
        "type": "client-response",
        "rpcId": "rebound-host",
        "result": { "ok": true, "value": {} }
    })
    .to_string();
    let request = format!(
        "POST /api/respond HTTP/1.1\r\nHost: evil.example\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    let (status, _) = raw_http(spine.web_server.port(), &request).await;
    assert_eq!(status, 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_rejects_an_explicit_cross_site_browser_request() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    let body = serde_json::json!({
        "type": "client-response",
        "rpcId": "cross-site",
        "result": { "ok": true, "value": {} }
    })
    .to_string();
    let request = format!(
        "POST /api/respond HTTP/1.1\r\nHost: 127.0.0.1\r\nSec-Fetch-Site: cross-site\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    let (status, _) = raw_http(spine.web_server.port(), &request).await;
    assert_eq!(status, 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_rejects_a_cross_origin_browser_request() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    let body = serde_json::json!({
        "type": "client-response",
        "rpcId": "cross-origin",
        "result": { "ok": true, "value": {} }
    })
    .to_string();
    let request = format!(
        "POST /api/respond HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://evil.example\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    let (status, _) = raw_http(spine.web_server.port(), &request).await;
    assert_eq!(status, 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_accepts_a_same_origin_browser_request() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    let port = spine.web_server.port();
    let body = serde_json::json!({
        "type": "client-response",
        "rpcId": "same-origin",
        "result": { "ok": true, "value": {} }
    })
    .to_string();
    let request = format!(
        "POST /api/respond HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nSec-Fetch-Site: same-origin\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    let (status, response) = raw_http(port, &request).await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response).expect("receipt JSON"),
        serde_json::json!({ "accepted": false, "reason": "not-pending" })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_mux_streams_sse_on_the_real_host_http_port() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    let request = "GET /api/events.mux HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    let response = raw_http_prefix(spine.web_server.port(), request, ": connected").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        response
            .to_ascii_lowercase()
            .contains("content-type: text/event-stream"),
        "{response}"
    );
    assert!(response.contains(": connected"), "{response}");
}

// Keep the SessionId/Session imports referenced for parity documentation.
#[allow(dead_code)]
fn _vocabulary(_session: &Session, _id: &SessionId) {}
