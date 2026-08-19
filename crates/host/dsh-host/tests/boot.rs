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
use dsh_agent::{Agent, AgentFactory, AgentOptions, AgentStatus, Inbox};
use dsh_host::{compose_host, compose_host_handle, mount_companions};
use dsh_llm::{
    ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, StreamChunk, call_id,
    create_user_message,
};
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

struct GoalToolThenTextAdapter {
    calls: std::sync::atomic::AtomicUsize,
}

impl LlmAdapter for GoalToolThenTextAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        let first = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
        let script = if first {
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "tool-call".to_string(),
                },
                StreamChunk::ToolCallDelta {
                    index: 0,
                    id: call_id("host-create-goal"),
                    name: Some("create_goal".to_string()),
                    arguments_delta: "{\"objective\":\"created by the production model loop\"}"
                        .to_string(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::ToolCall {
                        id: call_id("host-create-goal"),
                        name: "create_goal".to_string(),
                        arguments: "{\"objective\":\"created by the production model loop\"}"
                            .to_string(),
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::ToolCalls,
                    replay_state: None,
                },
            ]
        } else {
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "text".to_string(),
                },
                StreamChunk::TextDelta {
                    index: 0,
                    text: "goal created".to_string(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::Text {
                        text: "goal created".to_string(),
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::Stop,
                    replay_state: None,
                },
            ]
        };
        Box::pin(futures::stream::iter(script))
    }
}

struct PwshToolThenTextAdapter {
    calls: std::sync::atomic::AtomicUsize,
}

impl LlmAdapter for PwshToolThenTextAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        let first = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
        let script = if first {
            let arguments = serde_json::json!({
                "command": "Write-Output 'host-pwsh-e2e'",
                "description": "Print host PowerShell marker",
            })
            .to_string();
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "tool-call".to_string(),
                },
                StreamChunk::ToolCallDelta {
                    index: 0,
                    id: call_id("host-pwsh-call"),
                    name: Some("pwsh".to_string()),
                    arguments_delta: arguments.clone(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::ToolCall {
                        id: call_id("host-pwsh-call"),
                        name: "pwsh".to_string(),
                        arguments,
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::ToolCalls,
                    replay_state: None,
                },
            ]
        } else {
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "text".to_string(),
                },
                StreamChunk::TextDelta {
                    index: 0,
                    text: "pwsh completed".to_string(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::Text {
                        text: "pwsh completed".to_string(),
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::Stop,
                    replay_state: None,
                },
            ]
        };
        Box::pin(futures::stream::iter(script))
    }
}

struct SubagentToolThenTextAdapter {
    calls: std::sync::atomic::AtomicUsize,
}

impl LlmAdapter for SubagentToolThenTextAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let script = match call {
            0 => {
                let arguments = serde_json::json!({
                    "description": "Produce child marker",
                    "prompt": "Reply with exactly child-subagent-e2e"
                })
                .to_string();
                vec![
                    StreamChunk::BlockStart {
                        index: 0,
                        block_type: "tool-call".to_string(),
                    },
                    StreamChunk::ToolCallDelta {
                        index: 0,
                        id: call_id("host-subagent-call"),
                        name: Some("subagent".to_string()),
                        arguments_delta: arguments.clone(),
                    },
                    StreamChunk::BlockEnd {
                        index: 0,
                        block: ContentBlock::ToolCall {
                            id: call_id("host-subagent-call"),
                            name: "subagent".to_string(),
                            arguments,
                        },
                    },
                    StreamChunk::Finish {
                        reason: FinishReason::ToolCalls,
                        replay_state: None,
                    },
                ]
            }
            1 => vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "text".to_string(),
                },
                StreamChunk::TextDelta {
                    index: 0,
                    text: "child-subagent-e2e".to_string(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::Text {
                        text: "child-subagent-e2e".to_string(),
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::Stop,
                    replay_state: None,
                },
            ],
            _ => vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "text".to_string(),
                },
                StreamChunk::TextDelta {
                    index: 0,
                    text: "parent observed child".to_string(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::Text {
                        text: "parent observed child".to_string(),
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::Stop,
                    replay_state: None,
                },
            ],
        };
        Box::pin(futures::stream::iter(script))
    }
}

struct TerminalOpenThenTextAdapter {
    calls: std::sync::atomic::AtomicUsize,
}

impl LlmAdapter for TerminalOpenThenTextAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        let first = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
        let script = if first {
            let arguments =
                serde_json::json!({ "type": "shell", "name": "model-main" }).to_string();
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "tool-call".to_string(),
                },
                StreamChunk::ToolCallDelta {
                    index: 0,
                    id: call_id("host-terminal-open"),
                    name: Some("terminal_open".to_string()),
                    arguments_delta: arguments.clone(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::ToolCall {
                        id: call_id("host-terminal-open"),
                        name: "terminal_open".to_string(),
                        arguments,
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::ToolCalls,
                    replay_state: None,
                },
            ]
        } else {
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "text".to_string(),
                },
                StreamChunk::TextDelta {
                    index: 0,
                    text: "terminal opened".to_string(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::Text {
                        text: "terminal opened".to_string(),
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::Stop,
                    replay_state: None,
                },
            ]
        };
        Box::pin(futures::stream::iter(script))
    }
}

struct TerminalRoundTripAdapter {
    calls: std::sync::atomic::AtomicUsize,
}

impl LlmAdapter for TerminalRoundTripAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let tool = match call {
            0 => Some((
                "terminal-open",
                "terminal_open",
                serde_json::json!({
                    "type": "shell",
                    "name": "roundtrip",
                }),
            )),
            1 => Some(("terminal-list", "terminal_list", serde_json::json!({}))),
            2 => Some((
                "terminal-send",
                "terminal_send",
                serde_json::json!({
                    "sessionId": "pty-1",
                    "text": "Write-Output model-terminal-roundtrip",
                    "submit": true,
                }),
            )),
            3 => Some((
                "terminal-read",
                "terminal_read",
                serde_json::json!({
                    "sessionId": "pty-1",
                    "count": 100,
                }),
            )),
            4 => Some((
                "terminal-close",
                "terminal_close",
                serde_json::json!({
                    "sessionId": "pty-1",
                }),
            )),
            _ => None,
        };
        let script = if let Some((id, name, args)) = tool {
            let id = call_id(id);
            let arguments = args.to_string();
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "tool-call".to_string(),
                },
                StreamChunk::ToolCallDelta {
                    index: 0,
                    id: id.clone(),
                    name: Some(name.to_string()),
                    arguments_delta: arguments.clone(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::ToolCall {
                        id,
                        name: name.to_string(),
                        arguments,
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::ToolCalls,
                    replay_state: None,
                },
            ]
        } else {
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "text".to_string(),
                },
                StreamChunk::TextDelta {
                    index: 0,
                    text: "terminal roundtrip complete".to_string(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::Text {
                        text: "terminal roundtrip complete".to_string(),
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::Stop,
                    replay_state: None,
                },
            ]
        };
        Box::pin(futures::stream::iter(script))
    }
}

struct TerminalSignalAdapter {
    calls: std::sync::atomic::AtomicUsize,
}

impl LlmAdapter for TerminalSignalAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let tool = match call {
            0 => Some((
                "signal-open",
                "terminal_open",
                serde_json::json!({ "type": "shell" }),
            )),
            1 => Some((
                "signal-send",
                "terminal_send",
                serde_json::json!({
                    "sessionId": "pty-1",
                    "text": "Start-Sleep -Seconds 60",
                    "submit": true,
                    "run_in_background": true,
                }),
            )),
            2 => Some((
                "signal-deliver",
                "terminal_signal",
                serde_json::json!({
                    "sessionId": "pty-1",
                    "signal": "SIGINT",
                }),
            )),
            3 => Some((
                "signal-job",
                "job_output",
                serde_json::json!({
                    "job_id": "pty-send-1",
                    "wait": true,
                    "timeout_ms": 5000,
                }),
            )),
            4 => Some((
                "signal-close",
                "terminal_close",
                serde_json::json!({ "sessionId": "pty-1" }),
            )),
            _ => None,
        };
        let script = if let Some((id, name, args)) = tool {
            let id = call_id(id);
            let arguments = args.to_string();
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "tool-call".to_string(),
                },
                StreamChunk::ToolCallDelta {
                    index: 0,
                    id: id.clone(),
                    name: Some(name.to_string()),
                    arguments_delta: arguments.clone(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::ToolCall {
                        id,
                        name: name.to_string(),
                        arguments,
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::ToolCalls,
                    replay_state: None,
                },
            ]
        } else {
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: "text".to_string(),
                },
                StreamChunk::TextDelta {
                    index: 0,
                    text: "terminal signal complete".to_string(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::Text {
                        text: "terminal signal complete".to_string(),
                    },
                },
                StreamChunk::Finish {
                    reason: FinishReason::Stop,
                    replay_state: None,
                },
            ]
        };
        Box::pin(futures::stream::iter(script))
    }
}

struct HostRoundAdapter {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl LlmAdapter for HostRoundAdapter {
    fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(futures::stream::iter(vec![
            StreamChunk::BlockStart {
                index: 0,
                block_type: "text".to_string(),
            },
            StreamChunk::TextDelta {
                index: 0,
                text: "production goal round".to_string(),
            },
            StreamChunk::BlockEnd {
                index: 0,
                block: ContentBlock::Text {
                    text: "production goal round".to_string(),
                },
            },
            StreamChunk::Finish {
                reason: FinishReason::Stop,
                replay_state: None,
            },
        ]))
    }
}

struct CommandProbeAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope_key: ScopeKey,
}

impl CommandProbeAgent {
    fn boxed() -> Arc<dyn Agent> {
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
    let probe = CommandProbeAgent::boxed();
    assert!(
        spine.commands.find(&probe, "goal").is_some(),
        "the production host must register /goal"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_host_mounts_goal_tools_and_the_model_driver() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");

    assert!(
        ctx.get("llm", false).is_some(),
        "production host must expose llm"
    );
    assert!(
        ctx.get("agentLoop", false).is_some(),
        "production host must expose the model agent driver"
    );
    for name in ["get_goal", "create_goal", "update_goal"] {
        assert!(
            spine.tools.get(name, None).is_some(),
            "production host must register {name}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_host_automatically_drives_an_armed_goal_round() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    spine
        .llm
        .register_adapter(
            &ctx,
            vec!["test".to_string()],
            Arc::new(HostRoundAdapter {
                calls: calls.clone(),
            }),
        )
        .expect("adapter");
    let handle = spine
        .agent_loop
        .create_agent(
            &ctx,
            dsh_agent::CreateAgentOptions {
                session_id: Some(session_id("host-automatic-goal-round")),
                agent_options: Some(AgentOptions {
                    provider: Some("test".to_string()),
                    model: Some("model".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .expect("create agent");
    spine
        .goals
        .create(
            &handle.agent,
            dsh_goal::CreateGoalRequest {
                objective: "run through the production host".to_string(),
                max_goal_rounds: Some(1),
            },
        )
        .expect("create goal");

    let blocked = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let goal = spine
                .goals
                .get(&handle.agent)
                .expect("goal read")
                .expect("goal");
            if goal.phase == dsh_goal::GoalPhase::Blocked {
                break goal;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        let goal = spine
            .goals
            .get(&handle.agent)
            .expect("goal read")
            .expect("goal");
        panic!(
            "production host did not drive the armed goal: phase={}, activation={:?}, rounds={}, calls={}",
            goal.phase.as_str(),
            goal.activation,
            goal.rounds_started,
            calls.load(std::sync::atomic::Ordering::SeqCst)
        )
    });

    assert_eq!(blocked.rounds_started, 1);
    assert_eq!(
        blocked
            .blocked_reason
            .as_ref()
            .map(|reason| reason.code.as_str()),
        Some("round-limit")
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let events = handle.agent.session().events();
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.type_ == "user/message"
                    && event.data["source"]["kind"] == "goal"
                    && event.data["source"]["round"] == 1
            })
            .count(),
        1
    );
    handle.dispose.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_composition_child_leaves_no_data_root() {
    if std::env::var_os("DSH_HOST_FAILED_COMPOSE_CHILD").is_none() {
        return;
    }
    let temp_root = std::env::temp_dir();
    let before: std::collections::HashSet<_> = std::fs::read_dir(&temp_root)
        .expect("read isolated temp before")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| name.to_string_lossy().starts_with("dsh-host-"))
        .collect();

    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default())
        .expect("preinstall conflicting system prompt");
    let _error = compose_host(&ctx).err().expect("composition must fail");

    let after: Vec<_> = std::fs::read_dir(&temp_root)
        .expect("read isolated temp after")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| name.to_string_lossy().starts_with("dsh-host-") && !before.contains(name))
        .collect();
    assert!(
        after.is_empty(),
        "failed composition leaked data roots: {after:?}"
    );
}

#[test]
fn failed_composition_removes_the_data_root_before_returning() {
    let temp_root = std::env::temp_dir().join(format!(
        "dsh-host-failed-compose-parent-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&temp_root).expect("create isolated temp");
    let output = std::process::Command::new(std::env::current_exe().expect("current test exe"))
        .args([
            "--exact",
            "failed_composition_child_leaves_no_data_root",
            "--nocapture",
        ])
        .env("DSH_HOST_FAILED_COMPOSE_CHILD", "1")
        .env("TEMP", &temp_root)
        .env("TMP", &temp_root)
        .output()
        .expect("run failed-composition child");
    let _ = std::fs::remove_dir_all(&temp_root);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_host_registers_the_deepseek_provider() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    assert!(
        spine
            .llm
            .list_providers()
            .iter()
            .any(|provider| provider.id == "deepseek-official"),
        "the shipped Host must expose its default provider route"
    );
    spine.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_model_loop_executes_create_goal_durably() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    spine
        .llm
        .register_adapter(
            &ctx,
            vec!["test".to_string()],
            Arc::new(GoalToolThenTextAdapter {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
        )
        .expect("adapter");
    let handle = spine
        .agent_loop
        .create_agent(
            &ctx,
            dsh_agent::CreateAgentOptions {
                session_id: Some(session_id("host-model-goal")),
                agent_options: Some(AgentOptions {
                    provider: Some("test".to_string()),
                    model: Some("model".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .expect("create agent");
    handle.agent.followup(create_user_message(
        vec![ContentBlock::Text {
            text: "Create the goal.".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ));
    handle.agent.when_idle().await;

    let goal = spine
        .goals
        .get(&handle.agent)
        .expect("goal service")
        .expect("model-created goal");
    assert_eq!(goal.objective, "created by the production model loop");
    assert_eq!(goal.revision, 1);
    assert_eq!(goal.phase, dsh_goal::GoalPhase::Active);
    let events = handle.agent.session().events();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.type_ == "goal/change")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.type_ == "tool/call")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.type_ == "tool/result")
            .count(),
        1
    );
    handle.dispose.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_model_loop_runs_and_disposes_a_spawn_subagent() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    spine
        .llm
        .register_adapter(
            &ctx,
            vec!["test-subagent".to_string()],
            Arc::new(SubagentToolThenTextAdapter {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
        )
        .expect("adapter");
    let handle = spine
        .agent_loop
        .create_agent(
            &ctx,
            dsh_agent::CreateAgentOptions {
                session_id: Some(session_id("host-model-subagent")),
                agent_options: Some(AgentOptions {
                    provider: Some("test-subagent".to_string()),
                    model: Some("model".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .expect("create parent agent");
    handle.agent.followup(create_user_message(
        vec![ContentBlock::Text {
            text: "Delegate the marker task.".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ));
    handle.agent.when_idle().await;

    let results: Vec<_> = handle
        .agent
        .session()
        .events()
        .iter()
        .filter(|event| event.type_ == "tool/result")
        .map(|event| event.data.clone())
        .collect();
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(
        results[0].get("error").is_none(),
        "spawn subagent tool failed: {:?}",
        results[0]
    );
    assert!(
        results[0].to_string().contains("child-subagent-e2e"),
        "child output missing from durable parent result: {:?}",
        results[0]
    );
    assert_eq!(
        spine.agents.list().len(),
        1,
        "foreground child must be disposed before the parent settles"
    );
    handle.dispose.await;
    spine.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_model_loop_executes_real_pwsh_durably() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    spine
        .llm
        .register_adapter(
            &ctx,
            vec!["test".to_string()],
            Arc::new(PwshToolThenTextAdapter {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
        )
        .expect("adapter");
    let handle = spine
        .agent_loop
        .create_agent(
            &ctx,
            dsh_agent::CreateAgentOptions {
                session_id: Some(session_id("host-model-pwsh")),
                agent_options: Some(AgentOptions {
                    provider: Some("test".to_string()),
                    model: Some("model".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .expect("create agent");
    dsh_sandbox_policy::set_sandbox_mode(
        handle.agent.session(),
        dsh_sandbox::SandboxMode::DangerFullAccess,
    )
    .expect("explicit test-only unrestricted session mode");
    handle.agent.followup(create_user_message(
        vec![ContentBlock::Text {
            text: "Run the real PowerShell tool.".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ));
    handle.agent.when_idle().await;

    let result_events: Vec<serde_json::Value> = handle
        .agent
        .session()
        .events()
        .iter()
        .filter(|event| event.type_ == "tool/result")
        .map(|event| event.data.clone())
        .collect();
    handle.dispose.await;
    spine.shutdown().await.expect("shutdown");

    assert_eq!(result_events.len(), 1, "{result_events:?}");
    assert!(
        result_events[0].get("error").is_none(),
        "pwsh tool failed: {:?}",
        result_events[0]
    );
    assert!(
        result_events[0].to_string().contains("host-pwsh-e2e"),
        "real stdout missing from durable tool result: {:?}",
        result_events[0]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_model_loop_opens_and_owner_disposes_a_terminal() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    spine
        .llm
        .register_adapter(
            &ctx,
            vec!["test-terminal".to_string()],
            Arc::new(TerminalOpenThenTextAdapter {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
        )
        .expect("adapter");
    let handle = spine
        .agent_loop
        .create_agent(
            &ctx,
            dsh_agent::CreateAgentOptions {
                session_id: Some(session_id("host-model-terminal")),
                agent_options: Some(AgentOptions {
                    provider: Some("test-terminal".to_string()),
                    model: Some("model".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .expect("create agent");
    dsh_sandbox_policy::set_sandbox_mode(
        handle.agent.session(),
        dsh_sandbox::SandboxMode::DangerFullAccess,
    )
    .expect("explicit test-only unrestricted session mode");
    handle.agent.followup(create_user_message(
        vec![ContentBlock::Text {
            text: "Open a persistent terminal.".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ));
    handle.agent.when_idle().await;
    let terminals = ctx
        .get_typed::<Arc<dsh_terminal::TerminalSessionService>>("terminals", false)
        .map(|slot| slot.as_ref().clone())
        .expect("terminals service");
    let results: Vec<_> = handle
        .agent
        .session()
        .events()
        .iter()
        .filter(|event| event.type_ == "tool/result")
        .map(|event| event.data.clone())
        .collect();
    let live_before_dispose = terminals.list(&handle.agent);
    handle.dispose.await;
    let live_after_dispose = terminals.list(&handle.agent);
    spine.shutdown().await.expect("shutdown");

    assert_eq!(results.len(), 1, "{results:?}");
    assert!(results[0].get("error").is_none(), "{results:?}");
    assert!(results[0].to_string().contains("pty-"), "{results:?}");
    assert_eq!(live_before_dispose.len(), 1);
    assert!(live_after_dispose.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_model_loop_roundtrips_terminal_tools() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    spine
        .llm
        .register_adapter(
            &ctx,
            vec!["test-terminal-roundtrip".to_string()],
            Arc::new(TerminalRoundTripAdapter {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
        )
        .expect("adapter");
    let handle = spine
        .agent_loop
        .create_agent(
            &ctx,
            dsh_agent::CreateAgentOptions {
                session_id: Some(session_id("host-model-terminal-roundtrip")),
                agent_options: Some(AgentOptions {
                    provider: Some("test-terminal-roundtrip".to_string()),
                    model: Some("model".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .expect("create agent");
    dsh_sandbox_policy::set_sandbox_mode(
        handle.agent.session(),
        dsh_sandbox::SandboxMode::DangerFullAccess,
    )
    .expect("explicit test-only unrestricted session mode");
    handle.agent.followup(create_user_message(
        vec![ContentBlock::Text {
            text: "Roundtrip terminal tools.".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ));
    handle.agent.when_idle().await;
    let terminals = ctx
        .get_typed::<Arc<dsh_terminal::TerminalSessionService>>("terminals", false)
        .map(|slot| slot.as_ref().clone())
        .expect("terminals service");
    let results: Vec<_> = handle
        .agent
        .session()
        .events()
        .iter()
        .filter(|event| event.type_ == "tool/result")
        .map(|event| event.data.clone())
        .collect();
    let live = terminals.list(&handle.agent);
    handle.dispose.await;
    spine.shutdown().await.expect("shutdown");

    assert_eq!(results.len(), 5, "{results:?}");
    assert!(
        results.iter().all(|result| result.get("error").is_none()),
        "{results:?}"
    );
    assert!(
        results
            .iter()
            .any(|result| result.to_string().contains("model-terminal-roundtrip")),
        "{results:?}"
    );
    assert!(live.is_empty(), "terminal_close did not remove the session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_model_loop_backgrounds_and_signals_terminal_work() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    spine
        .llm
        .register_adapter(
            &ctx,
            vec!["test-terminal-signal".to_string()],
            Arc::new(TerminalSignalAdapter {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
        )
        .expect("adapter");
    let handle = spine
        .agent_loop
        .create_agent(
            &ctx,
            dsh_agent::CreateAgentOptions {
                session_id: Some(session_id("host-model-terminal-signal")),
                agent_options: Some(AgentOptions {
                    provider: Some("test-terminal-signal".to_string()),
                    model: Some("model".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .expect("create agent");
    dsh_sandbox_policy::set_sandbox_mode(
        handle.agent.session(),
        dsh_sandbox::SandboxMode::DangerFullAccess,
    )
    .expect("explicit test-only unrestricted session mode");
    handle.agent.followup(create_user_message(
        vec![ContentBlock::Text {
            text: "Run and interrupt terminal work.".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ));
    tokio::time::timeout(std::time::Duration::from_secs(15), handle.agent.when_idle())
        .await
        .expect("terminal signal model round deadline");
    let results: Vec<_> = handle
        .agent
        .session()
        .events()
        .iter()
        .filter(|event| event.type_ == "tool/result")
        .map(|event| event.data.clone())
        .collect();
    handle.dispose.await;
    spine.shutdown().await.expect("shutdown");

    assert_eq!(results.len(), 5, "{results:?}");
    assert!(
        results.iter().all(|result| result.get("error").is_none()),
        "{results:?}"
    );
    assert!(
        results
            .iter()
            .any(|result| result.to_string().contains("pty-send-1")),
        "background terminal job missing: {results:?}"
    );
    assert!(
        results
            .iter()
            .any(|result| result.to_string().contains("delivered")),
        "terminal signal missing: {results:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_host_runs_a_persistent_terminal_session() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    let handle = spine
        .agent_loop
        .create_agent(
            &ctx,
            dsh_agent::CreateAgentOptions {
                session_id: Some(session_id("host-terminal-e2e")),
                ..Default::default()
            },
        )
        .await
        .expect("create agent");
    dsh_sandbox_policy::set_sandbox_mode(
        handle.agent.session(),
        dsh_sandbox::SandboxMode::DangerFullAccess,
    )
    .expect("explicit test-only unrestricted session mode");
    let terminals = ctx
        .get_typed::<Arc<dsh_terminal::TerminalSessionService>>("terminals", false)
        .map(|slot| slot.as_ref().clone())
        .expect("terminals service");
    let created = match terminals.spawn(
        handle.agent.clone(),
        dsh_terminal::TerminalSpawnRequest {
            type_: "shell".to_string(),
            name: Some("main".to_string()),
            cwd: None,
        },
        None,
    ) {
        Ok(spawn) => spawn.await,
        Err(error) => Err(error),
    };
    let created = created.expect("production shell PTY backend");
    let operation = terminals
        .start_send(
            &handle.agent,
            &created.session_id,
            dsh_terminal::TerminalSendRequest {
                text: "Write-Output host-terminal-e2e".to_string(),
                submit: true,
                signal: None,
            },
        )
        .expect("start terminal send");
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), operation.done())
        .await
        .expect("terminal send deadline");
    let read = terminals
        .read(
            &handle.agent,
            &created.session_id,
            dsh_terminal::TerminalReadRequest {
                offset: None,
                count: Some(100),
            },
        )
        .expect("terminal read");
    terminals
        .kill(
            &handle.agent,
            &created.session_id,
            "test complete".to_string(),
        )
        .expect("terminal kill")
        .await
        .expect("terminal cleanup");
    handle.dispose.await;
    spine.shutdown().await.expect("shutdown");
    assert_eq!(created.type_, "shell");
    assert!(
        result.viewport.contains("host-terminal-e2e"),
        "terminal viewport missing output: {:?}",
        result.viewport
    );
    assert!(
        read.text.contains("host-terminal-e2e"),
        "terminal scrollback missing output: {:?}",
        read.text
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_host_mounts_goal_and_agent_loop_invariant_companions() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    mount_companions(&spine).expect("mount companions");

    let goal_duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dsh_goal::invariant::apply(&ctx)
    }));
    assert!(
        goal_duplicate.is_err(),
        "goal invariant package must already be reserved"
    );

    let round_driver_duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dsh_goal_round_driver::invariant::apply(&ctx)
    }));
    assert!(
        round_driver_duplicate.is_err(),
        "goal-round-driver invariant package must already be reserved"
    );

    let command_duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dsh_command_goal::invariant::apply(&ctx)
    }));
    assert!(
        command_duplicate.is_err(),
        "command-goal invariant package must already be reserved"
    );

    let tool_duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dsh_tool_goal::invariant::apply(&ctx)
    }));
    assert!(
        tool_duplicate.is_err(),
        "tool-goal invariant package must already be reserved"
    );

    let loop_duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dsh_agent_loop::apply_agent_loop_invariant(&ctx)
    }));
    assert!(
        loop_duplicate.is_err(),
        "agent-loop invariant package must already be reserved"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn composes_the_core_spine_and_boots_a_report() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    mount_companions(&spine).expect("mount companions");
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
    let services = report["services"].as_array().expect("services array");
    let unique: std::collections::HashSet<_> = services
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert_eq!(
        unique.len(),
        services.len(),
        "duplicate or non-string service: {report}"
    );
    for required in [
        "sessions",
        "agents",
        "goals",
        "llm",
        "tools",
        "agentLoop",
        "agentPresets",
    ] {
        assert!(unique.contains(required), "missing {required}: {report}");
    }
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
    spine.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_host_reports_the_execution_stack() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    let report = dsh_host::boot_report(&spine).await.expect("report");
    let services = report["services"].as_array().expect("services array");
    let missing: Vec<&str> = [
        "subprocess",
        "shell",
        "jobs",
        "terminals",
        "sandbox",
        "sandboxPolicy",
    ]
    .into_iter()
    .filter(|required| !services.iter().any(|service| service == *required))
    .collect();
    spine.shutdown().await.expect("shutdown");
    assert!(
        missing.is_empty(),
        "production Host is missing {missing:?}: {report}"
    );
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
async fn explicit_shutdown_flushes_before_removing_data_and_stops_the_listener_idempotently() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    let readiness = spine.readiness();
    assert!(readiness.bound_addr.port() > 0);
    assert_eq!(readiness.bound_addr.port(), spine.web_server.port());

    let session = spine
        .sessions
        .create(
            &ctx,
            Some(session_id("shutdown-durability")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .expect("create");
    session
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("append");

    let data_root = spine.data_root().to_path_buf();
    assert!(data_root.exists());
    let root_was_present_at_flush = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = root_was_present_at_flush.clone();
    let observed_root = data_root.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, _args| {
        observed.store(observed_root.exists(), std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { None })
    });
    let release_observer = ctx
        .on("session/flush", listener, cordis::EventOptions::default())
        .await;

    spine.shutdown().await.expect("first shutdown");
    assert!(
        root_was_present_at_flush.load(std::sync::atomic::Ordering::SeqCst),
        "shutdown must flush while the owned data root still exists"
    );
    assert!(
        !data_root.exists(),
        "owned data root is removed only after persistence drains"
    );
    let reconnect = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        tokio::net::TcpStream::connect(readiness.bound_addr),
    )
    .await;
    assert!(
        !matches!(reconnect, Ok(Ok(_))),
        "listener must not accept new connections after shutdown: {reconnect:?}"
    );

    spine
        .shutdown()
        .await
        .expect("repeated shutdown is idempotent");
    release_observer().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mounted_companions_shutdown_completes_within_two_seconds() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    mount_companions(&spine).expect("mount companions");
    mount_companions(&spine).expect("repeated mount is idempotent");

    tokio::time::timeout(std::time::Duration::from_secs(2), spine.shutdown())
        .await
        .expect("mounted companion shutdown must not hang")
        .expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mounted_companions_shutdown_after_real_http_requests_is_bounded() {
    let ctx = Context::root();
    let spine = compose_host(&ctx).expect("compose");
    mount_companions(&spine).expect("mount companions");
    let port = spine.web_server.port();

    let body = serde_json::json!({
        "type": "client-response",
        "rpcId": "shutdown-after-http",
        "result": { "ok": true, "value": {} }
    })
    .to_string();
    let request = format!(
        "POST /api/respond HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    let (api_status, _) = raw_http(port, &request).await;
    assert_eq!(api_status, 200);

    tokio::time::timeout(std::time::Duration::from_secs(2), spine.shutdown())
        .await
        .expect("shutdown after real HTTP requests must not hang")
        .expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cloneable_host_handles_join_one_shutdown_barrier() {
    let ctx = Context::root();
    let host = compose_host_handle(&ctx).expect("compose handle");
    let clone = host.clone();
    let readiness = host.readiness();
    let data_root = host.data_root().to_path_buf();
    assert_eq!(readiness.bound_addr.port(), host.web_server.port());

    let (left, right) = tokio::join!(host.shutdown(), clone.shutdown());
    left.expect("first caller");
    right.expect("second caller");
    assert!(!data_root.exists());
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
