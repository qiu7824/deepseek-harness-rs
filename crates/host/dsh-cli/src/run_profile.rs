//! Product profile dispatch over the statically shipped Rust surfaces.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cordis::Context;
use dsh_agent::AgentFactory;

use crate::profile_boot::{ComposedProfile, compose_profile};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSurface {
    Web,
    Headless,
}

pub struct RunProfileRequest {
    pub profile: String,
    pub patches: Vec<String>,
    pub args: Vec<String>,
    pub home: PathBuf,
    pub telemetry_env: Option<String>,
    /// Installed app package.json used for full Node module compatibility.
    /// Tests/static embedded callers may omit it and use shipped projections.
    pub install_anchor: Option<PathBuf>,
}

/// One launcher-owned interrupt edge. Production supplies Ctrl+C; tests can
/// inject the same one-shot lifecycle signal without depending on a console.
pub type ProfileInterrupt =
    Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'static>>;

/// Cloneable one-shot interrupt whose result is retained for every lifecycle
/// phase that needs to observe it.
#[derive(Clone)]
pub struct ProfileInterruptLatch {
    state: Arc<ProfileInterruptState>,
}

struct ProfileInterruptState {
    source: Mutex<Option<ProfileInterrupt>>,
    started: AtomicBool,
    result: tokio::sync::watch::Sender<Option<Result<(), String>>>,
}

impl ProfileInterruptLatch {
    pub fn new(source: ProfileInterrupt) -> Self {
        let (result, _) = tokio::sync::watch::channel(None);
        let latch = Self {
            state: Arc::new(ProfileInterruptState {
                source: Mutex::new(Some(source)),
                started: AtomicBool::new(false),
                result,
            }),
        };
        latch.start();
        latch
    }

    fn start(&self) {
        if self.state.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let source = self
            .state
            .source
            .lock()
            .expect("interrupt source lock")
            .take();
        let result = self.state.result.clone();
        tokio::spawn(async move {
            let outcome = match source {
                Some(source) => source.await,
                None => Err("interrupt source was unavailable".to_string()),
            };
            result.send_replace(Some(outcome));
        });
    }

    pub fn waiter(&self) -> ProfileInterrupt {
        let latch = self.clone();
        Box::pin(async move {
            latch.start();
            let mut result = latch.state.result.subscribe();
            loop {
                if let Some(outcome) = result.borrow().clone() {
                    return outcome;
                }
                result
                    .changed()
                    .await
                    .map_err(|_| "interrupt listener stopped".to_string())?;
            }
        })
    }
}

pub struct RunProfileHandle {
    pub surface: ProfileSurface,
    host: Option<dsh_host::HostSpine>,
    output: Option<String>,
}

impl RunProfileHandle {
    pub fn readiness_url(&self) -> Option<String> {
        if self.surface != ProfileSurface::Web {
            return None;
        }
        self.host.as_ref().map(|host| {
            let address = host.readiness().bound_addr;
            format!("http://127.0.0.1:{}", address.port())
        })
    }

    pub fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        match &self.host {
            Some(host) => host.shutdown().await,
            None => Ok(()),
        }
    }
}

fn enabled(row: &serde_json::Value) -> Result<bool, String> {
    match row.get("disabled") {
        None | Some(serde_json::Value::Bool(false)) => Ok(true),
        Some(serde_json::Value::Bool(true)) => Ok(false),
        Some(_) => Err(format!(
            "dsh: row {:?} uses a dynamic disabled expression that the static profile runtime cannot evaluate",
            row.get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<unknown>")
        )),
    }
}

pub fn resolve_profile_surface(composed: &ComposedProfile) -> Result<ProfileSurface, String> {
    let web = composed
        .rows
        .get("web-runtime")
        .map(enabled)
        .transpose()?
        .unwrap_or(false);
    let headless = composed
        .rows
        .get("headless-runner")
        .map(enabled)
        .transpose()?
        .unwrap_or(false);
    match (web, headless) {
        (true, false) => Ok(ProfileSurface::Web),
        (false, true) => Ok(ProfileSurface::Headless),
        (false, false) => Err(format!(
            "dsh: profile {:?} has no supported Rust surface (expected web-runtime or headless-runner)",
            composed.profile.name
        )),
        (true, true) => Err(format!(
            "dsh: profile {:?} enables both web and headless surfaces",
            composed.profile.name
        )),
    }
}

pub async fn run_profile(request: RunProfileRequest) -> Result<RunProfileHandle, String> {
    run_profile_with_interrupt(request, None).await
}

pub async fn run_profile_with_interrupt(
    request: RunProfileRequest,
    interrupt: Option<ProfileInterrupt>,
) -> Result<RunProfileHandle, String> {
    let composed = match request.install_anchor.as_deref() {
        Some(anchor) => crate::profile_boot::compose_profile_with_install_anchor(
            &request.profile,
            &request.patches,
            &request.home,
            request.telemetry_env.as_deref(),
            anchor,
        )?,
        None => compose_profile(
            &request.profile,
            &request.patches,
            &request.home,
            request.telemetry_env.as_deref(),
        )?,
    };
    let surface = resolve_profile_surface(&composed)?;
    match surface {
        ProfileSurface::Web => {
            let port = parse_web_port(&request.args)?;
            let ctx = Context::root();
            let host = dsh_host::compose_persistent_host_at_port(
                &ctx,
                request.home.clone(),
                Some(&request.profile),
                port,
            )?;
            let companions = dsh_host::mount_companions(&host);
            let (host, ()) = own_host_result(host, companions).await?;
            Ok(RunProfileHandle {
                surface,
                host: Some(host),
                output: None,
            })
        }
        ProfileSurface::Headless => run_headless(request.args, request.home, interrupt).await,
    }
}

fn parse_web_port(args: &[String]) -> Result<u16, String> {
    match args {
        [] => Ok(3080),
        [flag, value] if flag == "--port" => value.parse::<u16>().map_err(|_| {
            format!("dsh: --port must be an integer from 0 through 65535, got {value:?}")
        }),
        _ => Err(format!(
            "dsh: the Rust web surface accepts only --port <port>, got {args:?}"
        )),
    }
}

async fn run_headless(
    args: Vec<String>,
    home: PathBuf,
    interrupt: Option<ProfileInterrupt>,
) -> Result<RunProfileHandle, String> {
    let [task] = args.as_slice() else {
        return Err("dsh: headless requires exactly one non-empty task argument".to_string());
    };
    if task.trim().is_empty() {
        return Err("dsh: headless requires exactly one non-empty task argument".to_string());
    }

    let ctx = Context::root();
    let host = dsh_host::compose_persistent_host_at(&ctx, home, Some("headless"))?;
    let companions = dsh_host::mount_companions(&host);
    let (host, ()) = own_host_result(host, companions).await?;

    let model = std::env::var("DSH_DEEPSEEK_MODEL").unwrap_or_else(|_| "deepseek-chat".to_string());
    let create_result = host
        .agent_loop
        .create_agent(
            &host.ctx,
            dsh_agent::CreateAgentOptions {
                meta: Some(dsh_session::CreateSessionMeta {
                    cwd: Some(
                        std::env::current_dir()
                            .map_err(|error| format!("dsh: headless cwd: {error}"))?
                            .to_string_lossy()
                            .into_owned(),
                    ),
                    ..Default::default()
                }),
                agent_options: Some(dsh_agent::AgentOptions {
                    provider: Some(dsh_llm_deepseek::PROVIDER.to_string()),
                    model: Some(model),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .map_err(|error| format!("dsh: headless agent: {error}"));
    let (host, handle) = own_host_result(host, create_result).await?;
    handle.agent.followup(dsh_llm::create_user_message(
        vec![dsh_llm::ContentBlock::Text { text: task.clone() }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ));
    let interrupt_result = match interrupt {
        Some(mut interrupt) => {
            tokio::select! {
                biased;
                result = &mut interrupt => Some(result),
                _ = handle.agent.when_idle() => None,
            }
        }
        None => {
            handle.agent.when_idle().await;
            None
        }
    };

    let session = handle.agent.session().clone();
    if let Some(interrupt_result) = interrupt_result {
        handle.agent.cancel(
            dsh_agent::AgentCancelCause::User,
            Some(&dsh_agent::CancelOptions { keep_inbox: false }),
        );
        handle.agent.when_idle().await;
        let flush_error = host
            .sessions
            .flush(&session)
            .await
            .err()
            .map(|error| format!("session flush failed: {error}"));
        handle.dispose.await;
        let shutdown_error = host.shutdown().await.err();
        let mut error = match interrupt_result {
            Ok(()) => "dsh: headless interrupted".to_string(),
            Err(error) => format!("dsh: headless interrupt failed: {error}"),
        };
        if let Some(flush_error) = flush_error {
            error.push_str(&format!("; {flush_error}"));
        }
        if let Some(shutdown_error) = shutdown_error {
            error.push_str(&format!("; shutdown failed: {shutdown_error}"));
        }
        return Err(error);
    }

    let events = session.events();
    let outcome = headless_outcome(&events);
    let flush_result = host
        .sessions
        .flush(&session)
        .await
        .map_err(|error| format!("dsh: headless session flush failed: {error}"));
    handle.dispose.await;

    match (outcome, flush_result) {
        (Ok(output), Ok(_)) => Ok(RunProfileHandle {
            surface: ProfileSurface::Headless,
            host: Some(host),
            output: Some(output),
        }),
        (outcome, flush_result) => {
            let error = outcome
                .err()
                .or_else(|| flush_result.err())
                .expect("failed outcome");
            let shutdown = host.shutdown().await;
            match shutdown {
                Ok(()) => Err(error),
                Err(shutdown) => Err(format!("{error}; shutdown failed: {shutdown}")),
            }
        }
    }
}

async fn own_host_result<T>(
    host: dsh_host::HostSpine,
    result: Result<T, String>,
) -> Result<(dsh_host::HostSpine, T), String> {
    match result {
        Ok(value) => Ok((host, value)),
        Err(error) => match host.shutdown().await {
            Ok(()) => Err(error),
            Err(shutdown) => Err(format!("{error}; shutdown failed: {shutdown}")),
        },
    }
}

fn headless_outcome(events: &[dsh_session::SessionEvent]) -> Result<String, String> {
    let Some(terminal_event) = events.iter().rev().find(|event| event.type_ == "turn/end") else {
        return Err("dsh: headless model produced no terminal result".to_string());
    };
    let terminal_turn = terminal_event
        .data
        .get("turn")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "dsh: headless model produced an invalid terminal turn".to_string())?;
    let terminal = terminal_event
        .data
        .get("reason")
        .and_then(|value| serde_json::from_value::<dsh_session::TurnEndReason>(value.clone()).ok());
    let output = events
        .iter()
        .rev()
        .find(|event| {
            event.type_ == "assistant/message"
                && event.data.get("turn").and_then(serde_json::Value::as_u64) == Some(terminal_turn)
        })
        .and_then(|event| event.data.get("message"))
        .and_then(|value| serde_json::from_value::<dsh_llm::Message>(value.clone()).ok())
        .map(|message| {
            message
                .content
                .into_iter()
                .filter_map(|block| match block {
                    dsh_llm::ContentBlock::Text { text } => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .filter(|text| !text.is_empty());
    match (terminal, output) {
        (Some(dsh_session::TurnEndReason::Completed), Some(output)) => Ok(output),
        (Some(dsh_session::TurnEndReason::Error { error }), _) => Err(format!(
            "dsh: headless model failed [{}] {}",
            error.code, error.message
        )),
        (Some(reason), _) => Err(format!(
            "dsh: headless turn ended without assistant text ({})",
            reason.kind()
        )),
        (None, _) => Err("dsh: headless model produced no terminal result".to_string()),
    }
}
