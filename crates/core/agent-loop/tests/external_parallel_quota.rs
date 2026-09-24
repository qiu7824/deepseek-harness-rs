use cordis::Context;
use dsh_agent::{AgentFactory, AgentRegistry, CreateAgentOptions};
use dsh_agent_loop::AgentLoop;
use dsh_llm::{ChunkStream, GenerateOptions, LlmAdapter, LlmRuntime};
use dsh_session::{SessionStore, session_id};
use dsh_subagent::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

struct Pending(AtomicUsize);
impl LlmAdapter for Pending {
    fn stream(&self, _: &GenerateOptions) -> ChunkStream {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::pending())
    }
}
struct Run {
    id: dsh_session::SessionId,
    complete: AtomicBool,
    allow_disposal: AtomicBool,
    disposing: AtomicBool,
    fail_disposal: AtomicBool,
    disposed: AtomicBool,
}
async fn until(predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
#[async_trait::async_trait]
impl SubagentRun for Run {
    fn id(&self) -> &dsh_session::SessionId {
        &self.id
    }
    fn local_agent(&self) -> Option<Arc<dyn dsh_agent::Agent>> {
        None
    }
    async fn result(&self) -> Result<SubagentResult, String> {
        until(|| self.complete.load(Ordering::SeqCst)).await;
        Ok(SubagentResult {
            output: vec![],
            structured: None,
            stop_reason: SubagentStopReason::Completed,
        })
    }
    async fn dispose(&self) -> Result<(), String> {
        self.disposing.store(true, Ordering::SeqCst);
        until(|| self.allow_disposal.load(Ordering::SeqCst)).await;
        if self.fail_disposal.load(Ordering::SeqCst) {
            return Err("process exit unconfirmed".into());
        }
        self.disposed.store(true, Ordering::SeqCst);
        Ok(())
    }
}
struct Provider {
    starts: AtomicUsize,
    fail_start: AtomicBool,
    runs: parking_lot::Mutex<Vec<Arc<Run>>>,
}
#[async_trait::async_trait]
impl SubagentProvider for Provider {
    fn name(&self) -> &str {
        "external-fixture"
    }
    fn capabilities(&self) -> SubagentCapabilities {
        Default::default()
    }
    fn inherits_parent_context(&self) -> bool {
        false
    }
    async fn start(
        &self,
        _: ResolvedSubagentStartRequest,
    ) -> Result<Arc<dyn SubagentRun>, SubagentError> {
        let n = self.starts.fetch_add(1, Ordering::SeqCst);
        if self.fail_start.swap(false, Ordering::SeqCst) {
            return Err(SubagentError::new("START_FAILED", "fixture startup failed"));
        }
        let run = Arc::new(Run {
            id: session_id(format!("external-{n}")),
            complete: AtomicBool::new(false),
            allow_disposal: AtomicBool::new(false),
            disposing: AtomicBool::new(false),
            fail_disposal: AtomicBool::new(false),
            disposed: AtomicBool::new(false),
        });
        self.runs.lock().push(run.clone());
        Ok(run)
    }
}
fn request(parent: Arc<dyn dsh_agent::Agent>) -> SubagentStartRequest {
    SubagentStartRequest {
        parent,
        label: None,
        prompt: vec![],
        signal: Arc::new(|| false),
        agent_options: None,
        output_schema: None,
        max_depth: None,
        tool_filter: None,
        persona: None,
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_start_and_disposal_share_local_root_quota_and_failed_start_releases() {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    let llm = LlmRuntime::install(&ctx);
    let adapter = Arc::new(Pending(AtomicUsize::new(0)));
    llm.register_adapter(&ctx, vec!["quota".into()], adapter.clone())
        .unwrap();
    dsh_tools::ToolRuntime::install(&ctx, Default::default()).unwrap();
    SessionStore::install(&ctx);
    AgentRegistry::install(&ctx);
    let loops = AgentLoop::install(&ctx, Default::default()).unwrap();
    let parent = loops
        .create_agent(
            &ctx,
            CreateAgentOptions {
                session_id: Some(session_id("root")),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let child = loops
        .create_agent(
            &ctx,
            CreateAgentOptions {
                session_id: Some(session_id("child")),
                meta: Some(dsh_session::CreateSessionMeta {
                    origin: Some("subagent".into()),
                    parent_session: Some(parent.agent.id().clone()),
                    ..Default::default()
                }),
                agent_options: Some(dsh_agent::AgentOptions {
                    provider: Some("quota".into()),
                    model: Some("model".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let runtime = SubagentRuntime::install(&ctx);
    runtime.set_max_parallel(1).unwrap();
    let provider = Arc::new(Provider {
        starts: AtomicUsize::new(0),
        fail_start: AtomicBool::new(false),
        runs: Default::default(),
    });
    runtime.register_provider(&ctx, provider.clone()).unwrap();
    let first = runtime
        .start(provider.name(), request(parent.agent.clone()))
        .await
        .unwrap();
    let denied = runtime
        .start(provider.name(), request(child.agent.clone()))
        .await
        .err()
        .unwrap();
    assert_eq!(denied.code, "PARALLEL_LIMIT_REACHED");
    assert_eq!(provider.starts.load(Ordering::SeqCst), 1);
    let prompt = || {
        dsh_llm::create_user_message(
            vec![dsh_llm::ContentBlock::Text {
                text: "wait".into(),
            }],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        )
    };
    child.agent.followup(prompt());
    child.agent.when_idle().await;
    assert_eq!(adapter.0.load(Ordering::SeqCst), 0);
    runtime.set_max_parallel(2).unwrap();
    child.agent.followup(prompt());
    until(|| adapter.0.load(Ordering::SeqCst) == 1).await;
    runtime.set_max_parallel(1).unwrap();
    child
        .agent
        .cancel(dsh_session::AgentCancelCause::User, None);
    child.agent.when_idle().await;
    let run = provider.runs.lock()[0].clone();
    run.complete.store(true, Ordering::SeqCst);
    until(|| run.disposing.load(Ordering::SeqCst)).await;
    assert!(
        runtime
            .start(provider.name(), request(parent.agent.clone()))
            .await
            .is_err()
    );
    run.allow_disposal.store(true, Ordering::SeqCst);
    first.result().await.unwrap();
    assert!(run.disposed.load(Ordering::SeqCst));
    first.dispose().await.unwrap();
    provider.fail_start.store(true, Ordering::SeqCst);
    assert_eq!(
        runtime
            .start(provider.name(), request(parent.agent.clone()))
            .await
            .err()
            .unwrap()
            .code,
        "START_FAILED"
    );
    let second = runtime
        .start(provider.name(), request(parent.agent.clone()))
        .await
        .unwrap();
    let run = provider.runs.lock()[1].clone();
    drop(second);
    run.complete.store(true, Ordering::SeqCst);
    run.allow_disposal.store(true, Ordering::SeqCst);
    until(|| run.disposed.load(Ordering::SeqCst)).await;
    // Observer completion releases the slot even after the business handle is dropped.
    let third = loop {
        match runtime
            .start(provider.name(), request(parent.agent.clone()))
            .await
        {
            Ok(run) => break run,
            Err(error) if error.code == "PARALLEL_LIMIT_REACHED" => tokio::task::yield_now().await,
            Err(error) => panic!("{}", error.message),
        }
    };
    let run = provider.runs.lock()[2].clone();
    run.complete.store(true, Ordering::SeqCst);
    run.allow_disposal.store(true, Ordering::SeqCst);
    run.fail_disposal.store(true, Ordering::SeqCst);
    assert!(third.result().await.is_err());
    drop(third);
    assert_eq!(
        runtime
            .start(provider.name(), request(parent.agent.clone()))
            .await
            .err()
            .unwrap()
            .code,
        "PARALLEL_LIMIT_REACHED"
    );
    child.dispose.await;
    parent.dispose.await;
}
