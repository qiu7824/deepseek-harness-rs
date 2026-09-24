//! Optional per-call review for explicitly selected Auto sessions.
mod decision;
mod snapshot;
use cordis::{ArcValue, Context, Disposer, NextFn, arc, downcast_arc};
use decision::Decision;
use dsh_agent::CancellationSignal;
use dsh_llm::{
    BlockAssembler, ContentBlock, FinishReason, GenerateOptions, LlmRuntime, StreamChunk,
};
use dsh_permission_presets::{AUTO_PRESET, PermissionPresetService};
use dsh_session::SessionStore;
use dsh_tools::{PreToolDecision, RUN_CODE_NAME, ToolErrorInfo, ToolExecution};
use futures::{FutureExt, StreamExt};
use parking_lot::Mutex;
use std::{panic::AssertUnwindSafe, sync::Arc};
const POLICY: &str = include_str!("policy.txt");
struct Activity {
    accepting: bool,
    count: usize,
}
struct State {
    llm: Arc<LlmRuntime>,
    permissions: Arc<PermissionPresetService>,
    sessions: Arc<SessionStore>,
    activity: Mutex<Activity>,
    cancel: Arc<CancellationSignal>,
    idle: tokio::sync::Notify,
}
struct Lease(Arc<State>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.activity.lock().count -= 1;
        self.0.idle.notify_waiters();
    }
}
impl State {
    fn open(&self) -> bool {
        self.activity.lock().accepting
    }
    fn begin(self: &Arc<Self>) -> Option<Lease> {
        let mut a = self.activity.lock();
        if !a.accepting {
            return None;
        }
        a.count += 1;
        Some(Lease(self.clone()))
    }
    async fn drain(&self) {
        loop {
            let notified = self.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.activity.lock().count == 0 {
                return;
            }
            notified.await;
        }
    }
    async fn review(self: &Arc<Self>, exec: &ToolExecution) -> Result<Decision, String> {
        let agent = exec.agent.as_ref().ok_or("review requires an agent")?;
        let frozen = snapshot::capture(agent.as_ref(), exec)?;
        let caller = exec.signal.lock().clone();
        let state = self.clone();
        let signal: Arc<dyn Fn() -> bool + Send + Sync> =
            Arc::new(move || caller() || !state.open() || state.cancel.aborted());
        let mut stream = self.llm.stream(GenerateOptions {
            provider: frozen.provider,
            model: frozen.model,
            reasoning_effort: None,
            messages: vec![dsh_llm::create_user_message(
                vec![ContentBlock::Text { text: frozen.text }],
                dsh_llm::MessageSource::User {
                    rpc_id: None,
                    client_time_zone: None,
                },
            )],
            system: Some(POLICY.into()),
            tools: None,
            temperature: Some(0.0),
            max_tokens: None,
            stop: None,
            signal: Some(signal.clone()),
            session_id: None,
            purpose: Some("auto-review".into()),
            agent_loop_request: false,
            telemetry: None,
        });
        let mut assembler = BlockAssembler::new();
        let mut finished = false;
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(20));
        loop {
            if signal() {
                return Err("review cancelled".into());
            }
            let chunk = tokio::select! {biased;_=self.cancel.cancelled()=>return Err("review cancelled".into()),_=tick.tick()=>continue,chunk=stream.next()=>chunk};
            let Some(chunk) = chunk else {
                break;
            };
            if finished {
                return Err("review emitted data after finish".into());
            }
            if let StreamChunk::Finish { reason, .. } = &chunk {
                if *reason != FinishReason::Stop {
                    return Err("review did not stop normally".into());
                }
                finished = true;
            }
            assembler.push(&chunk);
        }
        if !finished {
            return Err("review has no terminal finish".into());
        }
        let blocks = assembler.blocks();
        let Some(ContentBlock::Text { text }) = blocks.last() else {
            return Err("review has no final JSON text".into());
        };
        if blocks[..blocks.len() - 1]
            .iter()
            .any(|b| !matches!(b, ContentBlock::Reasoning { .. }))
        {
            return Err("review must contain exactly one text block".into());
        }
        decision::parse(text)
    }
}
fn denied(exec: &ToolExecution, reason: Option<String>) -> PreToolDecision {
    PreToolDecision::DenyWithInfo {
        reason: format!(
            "Auto review rejected tool \"{}\"; its body was not executed",
            exec.name
        ),
        info: ToolErrorInfo {
            name: "AutoReviewDeniedError".into(),
            code: "AUTO_REVIEW_DENIED".into(),
        },
        meta: reason.map(|reason| serde_json::json!({"autoReview":{"reason":reason}})),
    }
}
pub struct AutoReview {
    state: Arc<State>,
    stop_listener: Disposer,
    stop_contribution: Disposer,
    stop_service: Mutex<Option<Disposer>>,
    closed: tokio::sync::OnceCell<()>,
}
impl cordis::Service for AutoReview {
    fn service_name(&self) -> &'static str {
        "autoReview"
    }
}
impl AutoReview {
    pub async fn install(ctx: &Context) -> Result<Arc<Self>, String> {
        let llm = ctx
            .get_typed::<Arc<LlmRuntime>>("llm", false)
            .ok_or("Auto review requires llm")?
            .as_ref()
            .clone();
        let permissions = ctx
            .get_typed::<Arc<PermissionPresetService>>("permissionPresets", false)
            .ok_or("Auto review requires permission presets")?
            .as_ref()
            .clone();
        let sessions = ctx
            .get_typed::<Arc<SessionStore>>("sessions", false)
            .ok_or("Auto review requires sessions")?
            .as_ref()
            .clone();
        let state = Arc::new(State {
            llm,
            permissions: permissions.clone(),
            sessions,
            activity: Mutex::new(Activity {
                accepting: true,
                count: 0,
            }),
            cancel: CancellationSignal::new(),
            idle: Default::default(),
        });
        let for_listener = state.clone();
        let listener: Arc<cordis::Listener> = Arc::new(move |_, args| {
            let state = for_listener.clone();
            Box::pin(async move {
                let exec = args
                    .first()
                    .and_then(|v| v.downcast_ref::<Arc<ToolExecution>>())
                    .cloned()
                    .expect("tool execution");
                let next =
                    downcast_arc::<NextFn>(args.last().expect("tool next")).expect("tool next");
                let Some(agent) = &exec.agent else {
                    return Some(next.call().await);
                };
                if exec.parent.is_none() && exec.name == RUN_CODE_NAME {
                    return Some(next.call().await);
                }
                if agent
                    .session()
                    .with_events(|events| state.permissions.current(events))
                    != AUTO_PRESET
                {
                    return Some(next.call().await);
                }
                let Some(_lease) = state.begin() else {
                    return Some(arc(PreToolDecision::Cancel));
                };
                let reviewed = AssertUnwindSafe(state.review(&exec)).catch_unwind().await;
                if !state.open() || state.cancel.aborted() || (exec.signal.lock().clone())() {
                    return Some(arc(PreToolDecision::Cancel));
                }
                match reviewed {
                    Ok(Ok(Decision::Allow)) => {
                        let downstream = next.call().await;
                        if !state.open() || state.cancel.aborted() || (exec.signal.lock().clone())()
                        {
                            Some(arc(PreToolDecision::Cancel))
                        } else {
                            Some(downstream)
                        }
                    }
                    Ok(Ok(Decision::Deny(reason))) => Some(arc(denied(&exec, reason))),
                    _ => Some(arc(denied(&exec, None))),
                }
            })
        });
        let stop_listener = ctx
            .on(
                "tools/pre-execute",
                listener,
                cordis::EventOptions {
                    prepend: true,
                    ..Default::default()
                },
            )
            .await;
        let admission_state = Arc::downgrade(&state);
        let stop_contribution = match permissions.register_auto(Arc::new(move || {
            if admission_state.upgrade().is_some_and(|s| s.open()) {
                Ok(())
            } else {
                Err("Auto review integration is closing".into())
            }
        })) {
            Ok(stop) => stop,
            Err(error) => {
                stop_listener().await;
                return Err(error);
            }
        };
        let review = Arc::new(Self {
            state,
            stop_listener,
            stop_contribution,
            stop_service: Mutex::new(None),
            closed: Default::default(),
        });
        *review.stop_service.lock() = Some(ctx.register_service(review.clone()));
        let owned = review.clone();
        let disposer = cordis::events::make_disposer(move || {
            let owned = owned.clone();
            Box::pin(async move {
                owned.shutdown().await;
            })
        });
        let _ = ctx.effect(
            "auto-review lifecycle",
            Box::pin(async move { Some(disposer) }),
        );
        Ok(review)
    }
    pub async fn shutdown(&self) {
        self.closed.get_or_init(||async {
            self.state.activity.lock().accepting=false;
            for session in self.state.sessions.list() {
                if session.with_events(|events|self.state.permissions.current(events))==AUTO_PRESET {
                    if let Err(error)=self.state.permissions.set(&session,"danger-full-access"){eprintln!("Auto review session transition failed; execution remains fenced: {error}");}
                }
            }
            self.state.cancel.abort();self.state.drain().await;
            (self.stop_listener)().await;(self.stop_contribution)().await;
                let stop=self.stop_service.lock().take();
                if let Some(stop)=stop {stop().await;}
        }).await;
    }
}
pub struct AutoReviewPlugin;
#[async_trait::async_trait]
impl cordis::Plugin for AutoReviewPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("experimental-auto-review")
    }
    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(["llm", "permissionPresets", "sessions", "tools"])
    }
    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), cordis::PluginError> {
        AutoReview::install(ctx)
            .await
            .map(|_| ())
            .map_err(|e| cordis::PluginError::new(arc(e)))
    }
}

#[cfg(test)]
mod tests;
