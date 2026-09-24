use cordis::Context;
use dsh_agent::{Agent,AgentFactory,AgentRegistry,CreateAgentOptions};
use dsh_agent_loop::AgentLoop;
use dsh_llm::{ChunkStream,ContentBlock,GenerateOptions,LlmAdapter,LlmRuntime,MessageSource};
use dsh_session::{SessionStore,session_id};
use dsh_subagent::{SubagentRuntime,SubagentStartRequest};
use dsh_subagent::continuation::{ContinuableStartSpec,SubagentFollowupOptions,SubagentInterruptAuthority};
use std::{sync::Arc,time::Duration};

struct PendingAdapter;
impl LlmAdapter for PendingAdapter {fn stream(&self,_:&GenerateOptions)->ChunkStream {Box::pin(futures::stream::pending())}}
fn spec(parent:Arc<dyn Agent>)->ContinuableStartSpec {
    ContinuableStartSpec {provider:"spawn".into(),label:"quota child".into(),signal:Arc::new(||false),request:SubagentStartRequest {
        label:Some("quota child".into()),prompt:vec![ContentBlock::Text{text:"wait".into()}],parent,signal:Arc::new(||false),agent_options:None,output_schema:None,max_depth:Some(8),tool_filter:None,persona:None,
    }}
}
fn followup()->SubagentFollowupOptions {SubagentFollowupOptions {source:MessageSource::User {rpc_id:None,client_time_zone:None},signal:Arc::new(||false),steer:false}}

#[tokio::test]
async fn real_creation_nested_admission_and_cold_resume_share_resident_slots() {
    let root=std::env::temp_dir().join(format!("resident-quota-runtime-{}",uuid::Uuid::new_v4()));
    let ctx=Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx,Default::default()).unwrap();
    let llm=LlmRuntime::install(&ctx);llm.register_adapter(&ctx,vec!["quota".into()],Arc::new(PendingAdapter)).unwrap();
    dsh_tools::ToolRuntime::install(&ctx,Default::default()).unwrap();SessionStore::install(&ctx);let agents=AgentRegistry::install(&ctx);
    dsh_session_persistence_jsonl::JsonlSessionPersistence::install(&ctx,dsh_session_persistence_jsonl::JsonlConfig {root:root.to_string_lossy().into_owned(),..Default::default()}).unwrap();
    let agent_loop=AgentLoop::install(&ctx,Default::default()).unwrap();
    let parent=agent_loop.create_agent(&ctx,CreateAgentOptions {session_id:Some(session_id("quota-parent")),agent_options:Some(dsh_agent::AgentOptions {provider:Some("quota".into()),model:Some("model".into()),..Default::default()}),..Default::default()}).await.unwrap();
    let runtime=SubagentRuntime::install(&ctx);dsh_subagent_spawn_in_process::apply(&ctx,&Default::default()).unwrap();runtime.set_max_active_subagents(2).unwrap();
    let defaults=Arc::new(dsh_subagent::SubagentDefaults::from_strings("quota","model","",1024.0,8.0,3.0,30.0));
    ctx.register_service(defaults.clone());
    let first=runtime.start_continuable(spec(parent.agent.clone())).await.unwrap();
    let second=runtime.start_continuable(spec(parent.agent.clone())).await.unwrap();
    assert_eq!(runtime.start_continuable(spec(parent.agent.clone())).await.unwrap_err().code,"ACTIVATION_LIMIT_REACHED");
    let child=agents.get(&first.child_id).unwrap();
    assert_eq!(child.options().max_steps,Some(3));
    assert_eq!(child.options().timeout_seconds,Some(30));
    assert_eq!(runtime.start_continuable(spec(child)).await.unwrap_err().code,"ACTIVATION_LIMIT_REACHED");
    runtime.suppress_settlement(&first.child_id,&parent.agent).unwrap();
    runtime.interrupt(&first.child_id,&SubagentInterruptAuthority::User {parent_session_id:parent.agent.id().clone()}).unwrap();
    tokio::time::timeout(Duration::from_secs(5),async {while agents.get(&first.child_id).is_some(){tokio::time::sleep(Duration::from_millis(2)).await;}}).await.unwrap();
    runtime.set_max_active_subagents(1).unwrap();
    let content=vec![ContentBlock::Text{text:"resume".into()}];
    assert_eq!(runtime.followup(parent.agent.clone(),&first.child_id,&content,followup()).await.unwrap_err().code,"ACTIVATION_LIMIT_REACHED");
    assert!(agents.get(&first.child_id).is_none());assert!(agents.get(&second.child_id).is_some());
    defaults.update(dsh_subagent::SubagentDefaults::from_strings("changed","different","",2048.0,8.0,4.0,40.0));
    runtime.set_max_active_subagents(2).unwrap();runtime.followup(parent.agent.clone(),&first.child_id,&content,followup()).await.unwrap();
    let resumed=agents.get(&first.child_id).unwrap();
    assert_eq!(resumed.options().provider.as_deref(),Some("quota"));
    assert_eq!(resumed.options().max_steps,Some(3));
    assert_eq!(resumed.options().timeout_seconds,Some(30));
    runtime.followup(parent.agent.clone(),&first.child_id,&content,followup()).await.unwrap();
    assert_eq!(runtime.start_continuable(spec(parent.agent.clone())).await.unwrap_err().code,"ACTIVATION_LIMIT_REACHED");
    parent.agent.cancel(dsh_session::AgentCancelCause::User,None);
    tokio::time::timeout(Duration::from_secs(5),runtime.drain_continuable_descendants(&[parent.agent.clone()])).await.unwrap().unwrap();
    parent.dispose.await;for disposer in ctx.fiber.disposables.clear(){disposer().await;}
    assert!(root.canonicalize().unwrap().starts_with(std::env::temp_dir().canonicalize().unwrap()));std::fs::remove_dir_all(root).unwrap();
}
use std::sync::atomic::{AtomicUsize,Ordering};
struct CountingPending(AtomicUsize);
impl LlmAdapter for CountingPending {
    fn stream(&self,_:&GenerateOptions)->ChunkStream {self.0.fetch_add(1,Ordering::SeqCst);Box::pin(futures::stream::pending())}
}
async fn calls_at_least(adapter:&CountingPending,n:usize) {
    tokio::time::timeout(Duration::from_secs(3),async {while adapter.0.load(Ordering::SeqCst)<n {tokio::task::yield_now().await;}}).await.unwrap();
}
#[tokio::test]
async fn parallel_limit_enforces_real_turns_and_idle_followup_reacquires() {
    let ctx=Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx,Default::default()).unwrap();
    let llm=LlmRuntime::install(&ctx);let adapter=Arc::new(CountingPending(AtomicUsize::new(0)));
    llm.register_adapter(&ctx,vec!["quota".into()],adapter.clone()).unwrap();
    dsh_tools::ToolRuntime::install(&ctx,Default::default()).unwrap();SessionStore::install(&ctx);AgentRegistry::install(&ctx);
    let loops=AgentLoop::install(&ctx,Default::default()).unwrap();let runtime=SubagentRuntime::install(&ctx);runtime.set_max_parallel(1).unwrap();
    let mut children=Vec::new();
    for id in ["a","b"] {
        children.push(loops.create_agent(&ctx,CreateAgentOptions {
            session_id:Some(session_id(id)),
            meta:Some(dsh_session::CreateSessionMeta {origin:Some("subagent".into()),parent_session:Some(session_id("root")),..Default::default()}),
            agent_options:Some(dsh_agent::AgentOptions {provider:Some("quota".into()),model:Some("model".into()),..Default::default()}),..Default::default()
        }).await.unwrap());
    }
    let prompt=||dsh_llm::create_user_message(vec![ContentBlock::Text{text:"run".into()}],MessageSource::User {rpc_id:None,client_time_zone:None});
    children[0].agent.followup(prompt());calls_at_least(&adapter,1).await;
    children[1].agent.followup(prompt());tokio::time::timeout(Duration::from_secs(3),children[1].agent.when_idle()).await.unwrap();
    assert_eq!(adapter.0.load(Ordering::SeqCst),1);
    assert!(children[1].agent.session().events().iter().any(|e|e.type_=="turn/end" && e.data.to_string().contains("maxParallel exceeded")));
    runtime.set_max_parallel(2).unwrap();children[1].agent.followup(prompt());calls_at_least(&adapter,2).await;
    runtime.set_max_parallel(1).unwrap();
    assert!(children.iter().all(|c|c.agent.status()==dsh_agent::AgentStatus::Running));
    for child in &children {child.agent.cancel(dsh_session::AgentCancelCause::User,None);child.agent.when_idle().await;}
    children[1].agent.followup(prompt());calls_at_least(&adapter,3).await;
    for child in children {child.agent.cancel(dsh_session::AgentCancelCause::User,None);child.agent.when_idle().await;child.dispose.await;}
}
