//! The DeepSeek Harness Host boot spine (M6): compose the ported service
//! stack — sessions, agents, system prompt, tools, JSONL persistence,
//! SQLite FTS5 session search, schedule, commands, and user questions —
//! run the package-owned invariant companions, and expose a boot report
//! with a real end-to-end durability + search probe.
//!
//! The M6 shell upgrade composes the web face on top: the webserver route
//! service, the SPA dist server (fallback seat), the directory-picker seam
//! (browse backend), the plugin inventory, and the apiproxy gateway.

use std::sync::Arc;

use axum::body::Body as WebBody;
use cordis::{ArcValue, Context, Plugin, PluginError, arc, make_disposer};
use dsh_agent::AgentRegistry;
use dsh_agent_loop::AgentLoop;
use dsh_commands::CommandRuntime;
use dsh_goal::GoalService;
use dsh_host_apiproxy::{
    AbortSignal, ApiProxyCarrier, ApiProxyDefaults, ApiProxyService, Body as CarrierBody,
    CarrierRequest, FetchHandler, FrameRequest, rpc_id, to_fetch_handler,
};
use dsh_host_directory_picker_browse::{BrowseDirectoryPicker, Config as PickerConfig};
use dsh_host_frontend_static::Config as FrontendConfig;
use dsh_host_frontend_static::apply as apply_frontend_static;
use dsh_host_plugin_inventory::PluginInventoryGateway;
use dsh_host_webserver::{
    Config as WebConfig, Host as BindHost, RouteDisposer, WebHandlerError, WebRequest, WebResponse,
    WebRoute, WebRouteKind, WebServer, WebUpgradeRoute, WebUpgraded,
};
use dsh_invariants::{InvariantConfig, InvariantRegistry};
use dsh_jobs_local::LocalJobRegistry;
use dsh_llm::LlmRuntime;
use dsh_pwsh_local::LocalPwshExecutor;
use dsh_sandbox_local::LocalSandboxProvider;
use dsh_sandbox_policy::SandboxPolicyService;
use dsh_session::{SessionStore, session_id};
use dsh_session_persistence::SessionPersistenceApi;
use dsh_session_persistence_jsonl::{JsonlCompression, JsonlConfig, JsonlSessionPersistence};
use dsh_session_query::{SessionQueryEngine, SessionSearchRequest};
use dsh_session_query_sqlite::{Config as SqliteSearchConfig, SqliteSearch};
use dsh_subprocess_local::LocalSubprocessRuntime;
use dsh_system_prompt::SystemPrompt;
use dsh_terminal::TerminalSessionService;
use dsh_tools::ToolRuntime;
use dsh_user_approval::ApprovalService;
use dsh_user_questions::UserQuestionService;
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

const MAX_API_REQUEST_BODY_BYTES: usize = 160 * 1024 * 1024;

fn decode_query_component(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let pair = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                if let Ok(byte) = u8::from_str_radix(pair, 16) {
                    decoded.push(byte);
                    index += 2;
                } else {
                    decoded.push(b'%');
                }
            }
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn parse_query(raw: &str) -> Vec<(String, String)> {
    raw.split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            (decode_query_component(key), decode_query_component(value))
        })
        .collect()
}

fn valid_authority_port(port: Option<&str>) -> bool {
    port.is_none_or(|port| !port.is_empty() && port.parse::<u16>().is_ok())
}

fn is_loopback_authority(authority: &str) -> bool {
    if authority.is_empty() || authority.trim() != authority {
        return false;
    }
    if let Some(ipv6) = authority.strip_prefix('[') {
        let Some((host, suffix)) = ipv6.split_once(']') else {
            return false;
        };
        let port = if suffix.is_empty() {
            None
        } else if let Some(port) = suffix.strip_prefix(':') {
            Some(port)
        } else {
            return false;
        };
        return host == "::1" && valid_authority_port(port);
    }

    let (host, port) = match authority.split_once(':') {
        Some((host, port)) if !port.contains(':') => (host, Some(port)),
        Some(_) => return false,
        None => (authority, None),
    };
    if !valid_authority_port(port) {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let octets = host.split('.').collect::<Vec<_>>();
    octets.len() == 4
        && octets[0] == "127"
        && octets.iter().all(|octet| {
            (1..=3).contains(&octet.len())
                && octet.bytes().all(|byte| byte.is_ascii_digit())
                && octet.parse::<u8>().is_ok()
        })
}

fn canonical_authority(authority: &str, default_port: Option<u16>) -> Option<String> {
    let parsed = authority.parse::<http::uri::Authority>().ok()?;
    let host = parsed.host().to_ascii_lowercase();
    let port = parsed.port_u16().filter(|port| Some(*port) != default_port);
    Some(match port {
        Some(port) if host.contains(':') => format!("[{host}]:{port}"),
        Some(port) => format!("{host}:{port}"),
        None if host.contains(':') => format!("[{host}]"),
        None => host,
    })
}

fn origin_matches_host(origin: &str, host: &str) -> bool {
    if origin == "null" {
        return false;
    }
    let Ok(uri) = origin.parse::<http::Uri>() else {
        return false;
    };
    let default_port = match uri.scheme_str() {
        Some("http") => Some(80),
        Some("https") => Some(443),
        _ => return false,
    };
    let Some(origin_authority) = uri.authority() else {
        return false;
    };
    canonical_authority(origin_authority.as_str(), default_port)
        == canonical_authority(host, Some(80))
}

fn trusted_web_request(request: &WebRequest) -> bool {
    let host = request
        .headers()
        .get(http::header::HOST)
        .and_then(|value| value.to_str().ok());
    host.is_some_and(is_loopback_authority)
        && !request
            .headers()
            .get("sec-fetch-site")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|site| site == "cross-site")
        && !request
            .headers()
            .get(http::header::ORIGIN)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|origin| !host.is_some_and(|host| origin_matches_host(origin, host)))
}

async fn pump_websocket_downlink(
    request: WebRequest,
    socket: WebUpgraded,
    api: Arc<ApiProxyService>,
    host_stream: bool,
) -> Result<(), WebHandlerError> {
    if !trusted_web_request(&request) {
        return Err(WebHandlerError::new("forbidden"));
    }
    let mut websocket = tokio_tungstenite::WebSocketStream::from_raw_socket(
        socket,
        tokio_tungstenite::tungstenite::protocol::Role::Server,
        None,
    )
    .await;
    let signal = AbortSignal::new();
    let frame_request = FrameRequest {
        rpc_id: rpc_id(uuid::Uuid::new_v4().to_string()),
        payload: serde_json::json!({}),
    };
    let mut frames = if host_stream {
        api.events_host(frame_request, signal.clone())
    } else {
        api.events_mux(frame_request, signal.clone())
    };
    loop {
        tokio::select! {
            frame = frames.next() => {
                let Some(frame) = frame else { break; };
                let method = frame.payload.get("type").and_then(serde_json::Value::as_str).unwrap_or("stream/error");
                let wire = serde_json::json!({
                    "type": "server-request",
                    "rpcId": frame.rpc_id,
                    "method": method,
                    "payload": frame.payload,
                });
                websocket.send(Message::Text(serde_json::to_string(&wire).map_err(|error| WebHandlerError::new(error.to_string()))?.into())).await.map_err(|error| WebHandlerError::new(error.to_string()))?;
            }
            incoming = websocket.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(payload))) => {
                        websocket.send(Message::Pong(payload)).await.map_err(|error| WebHandlerError::new(error.to_string()))?;
                    }
                    Some(Ok(Message::Pong(_))) | Some(Ok(Message::Text(_))) | Some(Ok(Message::Binary(_))) | Some(Ok(Message::Frame(_))) => {
                        // Browser clients are downlink-only; ignore incidental
                        // carrier frames rather than tearing down a healthy
                        // subscription before its first host event.
                    }
                    Some(Err(_)) => break,
                }
            }
        }
    }
    signal.abort();
    Ok(())
}

async fn bridge_api_request(request: WebRequest, handler: Arc<FetchHandler>) -> WebResponse {
    let (parts, incoming) = request.into_parts();
    let host = parts
        .headers
        .get(http::header::HOST)
        .and_then(|value| value.to_str().ok());
    let trusted_host = host.is_some_and(is_loopback_authority);
    let cross_site = parts
        .headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|site| site == "cross-site");
    let mismatched_origin = parts
        .headers
        .get(http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| !host.is_some_and(|host| origin_matches_host(origin, host)));
    if !trusted_host || cross_site || mismatched_origin {
        return http::Response::builder()
            .status(http::StatusCode::FORBIDDEN)
            .body(WebBody::from("forbidden"))
            .expect("static response");
    }
    if parts
        .headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_API_REQUEST_BODY_BYTES)
    {
        return http::Response::builder()
            .status(http::StatusCode::PAYLOAD_TOO_LARGE)
            .body(WebBody::empty())
            .expect("static response");
    }
    let bytes = match axum::body::to_bytes(WebBody::new(incoming), MAX_API_REQUEST_BODY_BYTES).await
    {
        Ok(bytes) => bytes,
        Err(_) => {
            return http::Response::builder()
                .status(http::StatusCode::PAYLOAD_TOO_LARGE)
                .body(WebBody::empty())
                .expect("static response");
        }
    };
    let query = parts.uri.query().map(parse_query).unwrap_or_default();
    let headers = parts
        .headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect();
    let response = handler
        .handle(CarrierRequest {
            method: parts.method,
            path: parts.uri.path().to_string(),
            query,
            headers,
            body: (!bytes.is_empty()).then(|| bytes.to_vec()),
        })
        .await;
    let (parts, body) = response.into_parts();
    let body = match body {
        CarrierBody::Bytes(bytes) => WebBody::from(bytes),
        CarrierBody::Stream(stream) => {
            use futures::StreamExt;
            let stream = stream.map(Ok::<Vec<u8>, std::convert::Infallible>);
            WebBody::from_stream(stream)
        }
    };
    WebResponse::from_parts(parts, body)
}

/// One booted host spine: the root context plus its registered services and
/// the disposable data directories owned by this boot.
pub struct HostSpine {
    pub ctx: Context,
    pub sessions: Arc<SessionStore>,
    pub agents: Arc<AgentRegistry>,
    pub llm: Arc<LlmRuntime>,
    pub agent_loop: Arc<AgentLoop>,
    pub tools: Arc<ToolRuntime>,
    pub system_prompt: Arc<SystemPrompt>,
    pub commands: Arc<CommandRuntime>,
    pub goals: Arc<GoalService>,
    pub questions: Arc<UserQuestionService>,
    pub approval: Arc<ApprovalService>,
    pub persistence: Arc<JsonlSessionPersistence>,
    pub search: Arc<SqliteSearch>,
    pub query: Arc<SessionQueryEngine>,
    pub web_server: Arc<WebServer>,
    pub api_proxy: Arc<ApiProxyService>,
    pub agent_presets: Arc<dsh_agent_presets::AgentPresets>,
    api_route: RouteDisposer,
    data_root: std::path::PathBuf,
    companion_fiber: parking_lot::Mutex<Option<Arc<cordis::FiberCore>>>,
    lifecycle_fiber: parking_lot::Mutex<Option<Arc<cordis::FiberCore>>>,
    shutdown_result: tokio::sync::OnceCell<Result<(), String>>,
    shutdown_requested: std::sync::atomic::AtomicBool,
    shutdown_failures: Arc<parking_lot::Mutex<Vec<String>>>,
}

/// The network coordinates published only after the host has bound its
/// listener. `bound_addr` contains the OS-selected port when port zero was
/// requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostReadiness {
    pub bound_addr: std::net::SocketAddr,
}

/// Cloneable application-facing owner used by CLI and desktop launchers.
/// Clones join the same idempotent shutdown barrier.
#[derive(Clone)]
pub struct HostHandle {
    spine: Arc<HostSpine>,
}

impl HostHandle {
    pub fn spine(&self) -> &HostSpine {
        &self.spine
    }

    pub fn readiness(&self) -> HostReadiness {
        self.spine.readiness()
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        self.spine.shutdown().await
    }
}

impl std::ops::Deref for HostHandle {
    type Target = HostSpine;

    fn deref(&self) -> &Self::Target {
        &self.spine
    }
}

impl HostSpine {
    pub fn readiness(&self) -> HostReadiness {
        HostReadiness {
            bound_addr: self.web_server.bound_addr(),
        }
    }

    pub fn data_root(&self) -> &std::path::Path {
        &self.data_root
    }

    /// Stop ingress, dispose the host-owned fiber tree, drain persistence, and
    /// only then remove the temporary data root. Concurrent/repeated callers
    /// join the same result.
    pub async fn shutdown(&self) -> Result<(), String> {
        self.shutdown_requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.shutdown_result
            .get_or_init(|| async {
                self.web_server.shutdown().await;
                (self.api_route)();

                let companion_fiber = self.companion_fiber.lock().clone();
                if let Some(fiber) = companion_fiber {
                    fiber.dispose().await;
                }

                let lifecycle_fiber = self.lifecycle_fiber.lock().clone();
                match lifecycle_fiber {
                    Some(fiber) => fiber.dispose().await,
                    None => self
                        .shutdown_failures
                        .lock()
                        .push("host lifecycle fiber is missing".to_string()),
                }

                let failures = self.shutdown_failures.lock().clone();
                if !failures.is_empty() {
                    return Err(format!(
                        "host shutdown did not drain safely: {}",
                        failures.join("; ")
                    ));
                }
                match tokio::fs::remove_dir_all(&self.data_root).await {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(format!(
                        "host shutdown could not remove data root {}: {error}",
                        self.data_root.display()
                    )),
                }
            })
            .await
            .clone()
    }
}

impl Drop for HostSpine {
    fn drop(&mut self) {
        if !self
            .shutdown_requested
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            self.web_server.request_shutdown();
            (self.api_route)();
            eprintln!(
                "dsh-host dropped without shutdown().await; stop requested and data root preserved at {}",
                self.data_root.display()
            );
        }
    }
}

struct HostCompositionPlugin {
    output: Arc<parking_lot::Mutex<Option<Result<HostSpine, String>>>>,
}

#[async_trait::async_trait]
impl Plugin for HostCompositionPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("dsh-host-composition")
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        *self.output.lock() = Some(compose_host_in_fiber(ctx));
        Ok(())
    }
}

/// Compose the M6 host spine synchronously (the async service bindings
/// settle through their own fibers).
pub fn compose_host(ctx: &Context) -> Result<HostSpine, String> {
    let output = Arc::new(parking_lot::Mutex::new(None));
    let fiber = ctx.plugin(
        Arc::new(HostCompositionPlugin {
            output: Arc::clone(&output),
        }),
        arc(()),
    );
    if let Err(error) = futures::executor::block_on(fiber.settle()) {
        futures::executor::block_on(fiber.dispose());
        return Err(format!("host composition: {}", error.message()));
    }
    let result = output
        .lock()
        .take()
        .ok_or_else(|| "host composition produced no result".to_string())?;
    match result {
        Ok(spine) => {
            *spine.lifecycle_fiber.lock() = Some(fiber);
            Ok(spine)
        }
        Err(error) => {
            futures::executor::block_on(fiber.dispose());
            Err(error)
        }
    }
}

/// Compose a cloneable host owner for long-running application entrypoints.
pub fn compose_host_handle(ctx: &Context) -> Result<HostHandle, String> {
    Ok(HostHandle {
        spine: Arc::new(compose_host(ctx)?),
    })
}

// DeepSeek's resolver seam intentionally preserves the core LlmError shape;
// boxing it here would make the Host adapter closure incompatible with the
// shared runtime contract.
#[allow(clippy::result_large_err)]
fn compose_host_in_fiber(ctx: &Context) -> Result<HostSpine, String> {
    // Package-owned invariant companions run first so every later append is
    // validated.
    let _invariants = InvariantRegistry::new(
        ctx,
        InvariantConfig {
            enabled: true,
            package_allowlist: vec![],
            package_blocklist: vec![],
        },
    );
    let data_root = std::env::temp_dir().join(format!("dsh-host-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&data_root).map_err(|error| format!("data root: {error}"))?;
    // Own the temporary root immediately. This is the first composition
    // effect, so reverse teardown closes every subsequently installed service
    // before attempting removal. A completed HostSpine takes ownership and
    // removes the root from its explicit shutdown barrier instead.
    let data_root_transferred = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let root_for_cleanup = data_root.clone();
    let transferred_for_cleanup = Arc::clone(&data_root_transferred);
    let cleanup_logger = ctx.named_logger(Some("dsh-host"));
    let _ = ctx.effect(
        "host.data-root",
        Box::pin(async move {
            Some(make_disposer(move || {
                let root = root_for_cleanup.clone();
                let transferred = Arc::clone(&transferred_for_cleanup);
                let logger = cleanup_logger.clone();
                Box::pin(async move {
                    if !transferred.load(std::sync::atomic::Ordering::SeqCst) {
                        match tokio::fs::remove_dir_all(&root).await {
                            Ok(()) => {}
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(error) => logger.error(vec![arc(format!(
                                "failed host composition could not remove data root {}: {error}",
                                root.display()
                            ))]),
                        }
                    }
                })
            }))
        }),
    );
    let sessions_root = data_root.join("sessions");
    let search_path = data_root.join("search.db");

    let sessions = SessionStore::install(ctx);
    // Persistence and the derived search index are installed before active
    // agent fibers. Cordis disposes effects in reverse order, so agent work is
    // quiescent before the durability barrier and backend close run.
    let persistence = JsonlSessionPersistence::install(
        ctx,
        JsonlConfig {
            root: sessions_root.to_string_lossy().to_string(),
            pack_chunks: true,
            compression: JsonlCompression::Zstd,
            prepared_session_cache_size: 5,
            write_batch_max_delay_ms: 200,
        },
    )
    .map_err(|error| format!("sessionPersistence: {error}"))?;
    let search = SqliteSearch::install(
        ctx,
        &SqliteSearchConfig {
            path: search_path.to_string_lossy().to_string(),
            ..Default::default()
        },
    )
    .map_err(|error| format!("sessionQuery: {}", error.message))?;
    let query = ctx
        .get_typed::<Arc<SessionQueryEngine>>("sessionQuery", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "sessionQuery service missing".to_string())?;
    let shutdown_failures = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let sessions_for_shutdown = Arc::clone(&sessions);
    let failures_for_shutdown = Arc::clone(&shutdown_failures);
    let _ = ctx.effect(
        "host.persistence-drain-barrier",
        Box::pin(async move {
            Some(make_disposer(move || {
                let sessions = Arc::clone(&sessions_for_shutdown);
                let failures = Arc::clone(&failures_for_shutdown);
                Box::pin(async move {
                    for session in sessions.list() {
                        if let Err(error) = sessions.flush(&session).await {
                            failures.lock().push(format!(
                                "session {} flush failed: {error}",
                                session.id().as_str()
                            ));
                        }
                    }
                })
            }))
        }),
    );

    let agents = AgentRegistry::install(ctx);
    // Execution resources are owner-fiber services. Install the process root
    // first; reverse Cordis teardown then closes terminals/jobs before the
    // subprocess provider drains any remaining trees. The model-code runtime
    // is installed only after its fail-closed OS sandbox is available.
    let _subprocess = LocalSubprocessRuntime::install(ctx);
    let _sandbox = LocalSandboxProvider::install(ctx, Default::default());
    let _sandbox_policy = SandboxPolicyService::install(
        ctx,
        dsh_sandbox_policy::Config {
            mode: Some(dsh_sandbox::SandboxMode::WorkspaceWrite),
            workspace_root: None,
        },
    );
    let _code_runtime = dsh_code_runtime_node::NodeCodeRuntime::install(
        ctx,
        dsh_code_runtime_node::Config {
            require_os_sandbox: true,
            ..Default::default()
        },
    )
    .map_err(|error| format!("code-runtime-node: {error}"))?;
    let _jobs = LocalJobRegistry::install(ctx, Default::default());
    let _terminals = TerminalSessionService::install(ctx);
    let _terminal_shell = dsh_terminal_bash::ShellTerminalBackend::install(ctx, Default::default())
        .map_err(|error| format!("terminal-bash: {error}"))?;
    let _shell = LocalPwshExecutor::install(ctx, Default::default());
    let llm = LlmRuntime::install(ctx);
    let deepseek_options =
        dsh_llm_deepseek::resolve_adapter_options(&dsh_llm_deepseek::DeepSeekConfig {
            base_url: std::env::var("DSH_DEEPSEEK_BASE_URL").ok(),
            ..Default::default()
        })
        .map_err(|error| format!("llm-deepseek: {}", error.failure.message))?;
    let deepseek_key = std::env::var(dsh_llm_deepseek::DEFAULT_API_KEY_ENV).ok();
    let deepseek_adapter = Arc::new(dsh_llm_deepseek::DeepSeekAdapter::new(
        dsh_llm_deepseek::DeepSeekAdapterOptions {
            options: Arc::new(move || Ok(deepseek_options.clone())),
            resolve_api_key: Arc::new(move |_snapshot| {
                let key = deepseek_key.clone();
                Box::pin(async move { Ok(key) })
            }),
        },
    ));
    let _deepseek_registration = dsh_llm_deepseek::apply(ctx, &llm, deepseek_adapter)
        .map_err(|error| format!("llm-deepseek: {}", error.failure.message))?;
    let system_prompt = SystemPrompt::install(ctx, dsh_system_prompt::Config::default())
        .map_err(|error| format!("systemPrompt: {error}"))?;
    let tools = ToolRuntime::install(
        ctx,
        dsh_tools::Config {
            mode: None,
            max_parallel_sub_calls: None,
        },
    )
    .map_err(|error| format!("tools: {error}"))?;
    futures::executor::block_on(dsh_tool_jobs::ToolJobsService::install(
        ctx,
        Default::default(),
    ))
    .map_err(|error| format!("tool-jobs: {error}"))?;
    dsh_tool_pwsh::ToolPwshService::install(ctx).map_err(|error| format!("tool-pwsh: {error}"))?;
    dsh_tool_terminal::ToolTerminalService::install(ctx)
        .map_err(|error| format!("tool-terminal: {error}"))?;
    let _subagents = dsh_subagent::SubagentRuntime::install(ctx);
    dsh_subagent_spawn_in_process::apply(ctx, &Default::default())
        .map_err(|error| format!("subagent-spawn: {}", error.message))?;
    dsh_tool_subagent::apply(
        ctx,
        &dsh_tool_subagent::Config {
            provider: "spawn".to_string(),
            ..Default::default()
        },
    )
    .map_err(|error| format!("tool-subagent: {error}"))?;
    let workflow = dsh_workflow_node::NodeWorkflowEngine::install(ctx, Default::default())
        .map_err(|error| format!("workflow-node: {error}"))?;
    let workflow_service: Arc<dyn dsh_workflow::WorkflowEngine> = workflow.clone();
    ctx.register_service(workflow_service);
    let workflow_teardown = workflow.clone();
    let _ = ctx.effect(
        "workflow node teardown",
        Box::pin(async move {
            Some(make_disposer(move || {
                let workflow = workflow_teardown.clone();
                Box::pin(async move { workflow.dispose().await })
            }))
        }),
    );
    dsh_tool_workflow::apply(ctx).map_err(|error| format!("tool-workflow: {error}"))?;
    dsh_tool_ralph::apply(ctx, &Default::default())
        .map_err(|error| format!("tool-ralph: {error}"))?;
    let agent_loop = AgentLoop::install(ctx, dsh_agent_loop::Config::default())
        .map_err(|error| format!("agentLoop: {error}"))?;
    let commands = CommandRuntime::install(ctx);
    let goals = GoalService::install(ctx, dsh_goal::Config::default());
    let _goal_round_driver =
        dsh_goal_round_driver::apply(ctx).map_err(|error| format!("goal-round-driver: {error}"))?;
    let _goal_command =
        dsh_command_goal::apply(ctx).map_err(|error| format!("command-goal: {error}"))?;
    let _goal_tools = dsh_tool_goal::apply(ctx, &dsh_tool_goal::Config::default())
        .map_err(|error| format!("tool-goal: {error}"))?;
    let questions = UserQuestionService::install(ctx);
    let approval = ApprovalService::install(ctx, dsh_user_approval::Config::default());
    dsh_schedule::apply(ctx);
    // ---- M6 shell: the web face over the spine ----
    // The loader service anchors the plugin inventory and profile
    // composition (the Rust static registry serves empty for now).
    let loader = futures::executor::block_on(dsh_cordis_loader::LoaderService::new(ctx));
    ctx.register_service(loader);
    // The agent-presets roster: the shipped presets beside this app's
    // config plus the harness-home user root the service appends itself.
    // Anchored to the manifest, not the process cwd (tests and launchers
    // run from different directories; TS anchors to the package location
    // the same way).
    let shipped_preset_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../config/agent-presets")
        .to_string_lossy()
        .into_owned();
    let agent_presets = dsh_agent_presets::AgentPresets::install(
        ctx,
        dsh_agent_presets::Config {
            default: "standard".to_string(),
            roots: vec![dsh_agent_presets::PresetRoot {
                path: shipped_preset_root,
                trust: dsh_agent_presets::PresetTrust::System,
            }],
            include_user_root: true,
        },
        dsh_agent_presets::process_env(),
    )
    .map_err(|error| format!("agentPresets: {error}"))?;
    // The webserver binds an OS-assigned port (the report publishes it).
    let web_server = futures::executor::block_on(WebServer::install(
        ctx,
        WebConfig {
            host: BindHost::Loopback,
            port: 0,
        },
    ))
    .map_err(|error| format!("webserver: {error}"))?;
    // The SPA dist server claims the fallback seat.
    let dist_index = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../web/dist/index.html")
        .to_string_lossy()
        .into_owned();
    let _ = apply_frontend_static(ctx, FrontendConfig { dist_index })
        .map_err(|error| format!("frontend-static: {error}"))?;
    let mut boot_payload: serde_json::Value =
        serde_json::from_str(include_str!("../../../../web/dist/plugins/manifest.json"))
            .expect("web plugin manifest");
    let object = boot_payload
        .as_object_mut()
        .expect("web plugin manifest object");
    object.insert("apiBase".to_string(), serde_json::json!("/api"));
    object.insert(
        "provider".to_string(),
        serde_json::json!("deepseek-official"),
    );
    object.insert("model".to_string(), serde_json::json!("deepseek-chat"));
    let boot_script = format!(
        "<script>window.__DSH_BOOT__={};</script>",
        serde_json::to_string(&boot_payload).expect("web boot payload")
    );
    let _ = web_server.tap_index(Arc::new(move |html| {
        html.replacen("</head>", &format!("{boot_script}</head>"), 1)
    }));
    // The directory-picker seam serves the browse interaction.
    BrowseDirectoryPicker::install(ctx, PickerConfig::default());
    // The plugin inventory projects the loader tree (no loader composed in
    // this spine yet; the gateway installs and serves an empty catalog).
    let _ = PluginInventoryGateway::install(ctx);
    // The apiproxy gateway wires the 52-RPC surface onto the spine.
    let api_proxy = ApiProxyService::install(
        ctx,
        ApiProxyDefaults {
            default_model_selection: Arc::new(|| dsh_host_apiproxy::ModelSelection {
                provider: "deepseek-official".to_string(),
                model: "deepseek-chat".to_string(),
                reasoning_effort: None,
            }),
            ..Default::default()
        },
    );
    let fetch_handler = Arc::new(to_fetch_handler(api_proxy.clone()));
    let api_route = web_server.register(WebRoute {
        kind: WebRouteKind::Prefix,
        path: "/api".to_string(),
        handler: Arc::new(move |request| {
            let fetch_handler = Arc::clone(&fetch_handler);
            Box::pin(async move { Ok(bridge_api_request(request, fetch_handler).await) })
        }),
    });
    let mux_api = api_proxy.clone();
    let _ = web_server.register_upgrade(WebUpgradeRoute {
        path: "/api/events.mux".to_string(),
        handler: Arc::new(move |request, socket| {
            let api = mux_api.clone();
            Box::pin(async move { pump_websocket_downlink(request, socket, api, false).await })
        }),
    });
    let host_api = api_proxy.clone();
    let _ = web_server.register_upgrade(WebUpgradeRoute {
        path: "/api/events.host".to_string(),
        handler: Arc::new(move |request, socket| {
            let api = host_api.clone();
            Box::pin(async move { pump_websocket_downlink(request, socket, api, true).await })
        }),
    });
    data_root_transferred.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(HostSpine {
        ctx: ctx.clone(),
        sessions,
        agents,
        llm,
        agent_loop,
        tools,
        system_prompt,
        commands,
        goals,
        questions,
        approval,
        persistence,
        search,
        query,
        web_server,
        api_proxy,
        agent_presets,
        api_route,
        data_root,
        companion_fiber: parking_lot::Mutex::new(None),
        lifecycle_fiber: parking_lot::Mutex::new(None),
        shutdown_result: tokio::sync::OnceCell::new(),
        shutdown_requested: std::sync::atomic::AtomicBool::new(false),
        shutdown_failures,
    })
}

/// The service inventory plus a real durability-and-search probe — the
/// observable boot report shared by the binary and the integration test.
pub async fn boot_report(spine: &HostSpine) -> Result<serde_json::Value, String> {
    // Live path: a store-attached session, a user message, and a durability
    // flush through the JSONL coordinator.
    let session = spine
        .sessions
        .create(
            &spine.ctx,
            Some(session_id("host-boot")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .map_err(|error| format!("session create: {error}"))?;
    let starter = dsh_llm::create_user_message(
        vec![dsh_llm::ContentBlock::Text {
            text: "host boot live needle".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    session
        .append(
            "user/message",
            serde_json::to_value(&starter).map_err(|error| error.to_string())?,
            Some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .map_err(|error| format!("append: {error}"))?;
    let flushed = spine
        .sessions
        .flush(&session)
        .await
        .map_err(|error| format!("flush: {error}"))?;

    // Persisted-only path: an independent durable log the search index must
    // reconcile through the erased persistence service.
    let durable_header = dsh_session::SessionHeader {
        version: dsh_session::SESSION_FORMAT_VERSION,
        id: session_id("host-persisted"),
        created_at: 1,
        cwd: None,
        parent_session: None,
        seed_length: None,
        origin: None,
        delegation_depth: None,
        agent_preset: None,
    };
    let durable_event = dsh_session::SessionEvent {
        type_: "user/message".to_string(),
        seq: 0,
        time: 1,
        data: serde_json::to_value(dsh_llm::create_user_message(
            vec![dsh_llm::ContentBlock::Text {
                text: "host persisted needle".to_string(),
            }],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        ))
        .expect("message"),
        ignorable: None,
        surface_op: Some(dsh_session::SurfaceOp::Append),
        source_event_seqs: None,
    };
    spine
        .persistence
        .create(durable_header.clone())
        .await
        .map_err(|error| format!("persisted create: {error}"))?;
    spine
        .persistence
        .append(&durable_header.id, &[durable_event])
        .await
        .map_err(|error| format!("persisted append: {error}"))?;
    let snapshots = spine
        .persistence
        .list_snapshots()
        .await
        .map_err(|error| format!("snapshots: {error}"))?;

    // The FTS5 index must find both the live and the persisted log.
    let live_hits = spine
        .query
        .search_sessions(
            &SessionSearchRequest {
                query: "live needle".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .map_err(|error| format!("live search: {}", error.message))?;
    let persisted_hits = spine
        .query
        .search_sessions(
            &SessionSearchRequest {
                query: "persisted needle".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .map_err(|error| format!("persisted search: {}", error.message))?;

    // The agent-presets roster serves the shipped presets beside this app's
    // config (a live discovery read proves the mount).
    let roster = spine
        .agent_presets
        .list()
        .await
        .map_err(|error| format!("preset roster: {error}"))?;

    Ok(serde_json::json!({
        "services": [
            "invariants",
            "sessions",
            "agents",
            "llm",
            "systemPrompt",
            "tools",
            "agentLoop",
            "commands",
            "goals",
            "userQuestions",
            "approval",
            "sessionPersistence",
            "sessionQuery",
            "schedule",
            "agentPresets",
            "subprocess",
            "sandbox",
            "sandboxPolicy",
            "jobs",
            "terminals",
            "shell",
            "subagents",
            "codeRuntime",
            "workflowEngine",
        ],
        "session": {
            "id": session.id().as_str(),
            "seq": session.seq(),
            "toolCount": spine.tools.schemas(None).len(),
        },
        "probe": {
            "flushAcknowledged": flushed,
            "persistedSnapshotCount": snapshots.len(),
            "liveSearchHits": live_hits.items.len(),
            "persistedSearchHits": persisted_hits.items.len(),
            "presetCount": roster.len(),
        },
    }))
}

struct HostCompanionsPlugin;

#[async_trait::async_trait]
impl Plugin for HostCompanionsPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("dsh-host-companions")
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        let _ = dsh_session::invariant::apply(ctx).await;
        let _goal = dsh_goal::invariant::apply(ctx);
        let _goal_round_driver = dsh_goal_round_driver::invariant::apply(ctx);
        let _command_goal = dsh_command_goal::invariant::apply(ctx);
        let _tool_goal = dsh_tool_goal::invariant::apply(ctx);
        let _agent_loop = dsh_agent_loop::apply_agent_loop_invariant(ctx);
        let _schedule = dsh_schedule::invariant::apply(ctx);
        let _query_sqlite = dsh_session_query_sqlite::invariant::apply(ctx);
        let _ = ctx.plugin(Arc::new(dsh_llm::LlmInvariantPlugin), arc(()));
        Ok(())
    }
}

/// Mount package-owned invariant companions in an independently disposable
/// child fiber. Repeated calls join the same settled fiber.
pub fn mount_companions(spine: &HostSpine) -> Result<(), String> {
    if spine
        .shutdown_requested
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err("cannot mount host companions after shutdown has started".to_string());
    }
    let fiber = {
        let mut slot = spine.companion_fiber.lock();
        slot.get_or_insert_with(|| spine.ctx.plugin(Arc::new(HostCompanionsPlugin), arc(())))
            .clone()
    };
    futures::executor::block_on(fiber.settle())
        .map_err(|error| format!("host companions: {}", error.message()))
}

// Re-exported anchors for compositions.
pub use dsh_agent::AgentRegistry as AgentRegistryType;
pub use dsh_session::SessionStore as SessionStoreType;
