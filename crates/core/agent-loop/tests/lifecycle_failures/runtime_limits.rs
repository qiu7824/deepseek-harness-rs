use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};
use std::time::Duration;
use dsh_agent::{Agent, AgentOptions};
use dsh_llm::{ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, StreamChunk, call_id};
use super::support::{harness_with_options, message, quick_tool, register_adapter, turn_end_kinds};

struct Looping(AtomicUsize);
impl LlmAdapter for Looping {
    fn stream(&self, _: &GenerateOptions) -> ChunkStream {
        let n = self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::iter(vec![
            StreamChunk::BlockStart {index:0,block_type:"tool-call".into()},
            StreamChunk::BlockEnd {index:0,block:ContentBlock::ToolCall {id:call_id(format!("call-{n}")),name:"quick".into(),arguments:"{}".into()}},
            StreamChunk::Finish {reason:FinishReason::ToolCalls,replay_state:None},
        ]))
    }
}
struct Pending;
impl LlmAdapter for Pending {
    fn stream(&self, _: &GenerateOptions) -> ChunkStream { Box::pin(futures::stream::pending()) }
}

#[tokio::test]
async fn step_budget_stops_tool_cycles_and_resets_on_followup() {
    let h = harness_with_options(AgentOptions {max_steps:Some(2),..Default::default()}).await;
    let adapter = Arc::new(Looping(AtomicUsize::new(0)));
    register_adapter(&h, adapter.clone());
    h.tools.register(&h.ctx, quick_tool(Arc::new(AtomicBool::new(false)))).unwrap();
    for expected in [2, 4] {
        h.agent.send(message("run"), dsh_agent::InboxTarget::NextTurn, true);
        tokio::time::timeout(Duration::from_secs(3), h.agent.when_idle()).await.unwrap();
        assert_eq!(adapter.0.load(Ordering::SeqCst), expected);
    }
    assert_eq!(turn_end_kinds(&h.agent), vec!["aborted", "aborted"]);
    let events=h.agent.session().events();
    assert_eq!(events.iter().filter(|e|e.type_=="step/start").count(),4);
    assert_eq!(events.iter().filter(|e|e.type_=="step/end").count(),4);
    assert!(events.iter().filter(|e|e.type_=="turn/end").all(|e|e.data.to_string().contains("maxTurns exceeded")));
}

#[tokio::test]
async fn deadline_cancels_stalled_model_and_allows_a_fresh_followup() {
    let h = harness_with_options(AgentOptions {timeout_seconds:Some(1),..Default::default()}).await;
    register_adapter(&h, Arc::new(Pending));
    for _ in 0..2 {
        h.agent.send(message("run"), dsh_agent::InboxTarget::NextTurn, true);
        tokio::time::timeout(Duration::from_secs(3), h.agent.when_idle()).await.unwrap();
    }
    assert_eq!(turn_end_kinds(&h.agent), vec!["aborted", "aborted"]);
    assert!(h.agent.session().events().iter().filter(|e|e.type_=="turn/end").all(|e|e.data.to_string().contains("timeoutSeconds exceeded")));
}


#[tokio::test]
async fn deadline_cancels_uncooperative_tool_and_preserves_step_closure() {
    use super::support::{NamedToolThenTextAdapter, hanging_tool};
    let h = harness_with_options(AgentOptions {timeout_seconds:Some(1),..Default::default()}).await;
    register_adapter(&h, Arc::new(NamedToolThenTextAdapter {name:"hang", calls:AtomicUsize::new(0)}));
    let entered=Arc::new(AtomicBool::new(false));
    let dropped=Arc::new(AtomicBool::new(false));
    h.tools.register(&h.ctx,hanging_tool(entered.clone(),dropped.clone())).unwrap();
    h.agent.followup(message("run"));
    tokio::time::timeout(Duration::from_secs(3),h.agent.when_idle()).await.unwrap();
    assert!(entered.load(Ordering::SeqCst));
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(turn_end_kinds(&h.agent),vec!["aborted"]);
    assert_eq!(h.agent.session().events().iter().filter(|e| e.type_=="step/end").count(),1);
}

#[tokio::test]
async fn deadline_interrupts_a_stalled_pre_step_hook() {
    let h = harness_with_options(AgentOptions {timeout_seconds:Some(1),..Default::default()}).await;
    let listener:Arc<cordis::Listener>=Arc::new(|_,_|Box::pin(futures::future::pending()));
    h.ctx.on("agent/pre-step",listener,Default::default()).await;
    h.agent.followup(message("run"));
    tokio::time::timeout(Duration::from_secs(3),h.agent.when_idle()).await.unwrap();
    assert_eq!(turn_end_kinds(&h.agent),vec!["aborted"]);
}
