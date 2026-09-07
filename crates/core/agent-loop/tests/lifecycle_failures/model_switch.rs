use super::support::{harness, message, register_adapter};
use dsh_agent::{Agent, ModelSelection, ModelSelectionRef, install_model_selection};
use dsh_llm::{ChunkStream, FinishReason, GenerateOptions, LlmAdapter, MessageSource, StreamChunk};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;

struct RecordingAdapter(Arc<Mutex<Vec<GenerateOptions>>>);
impl LlmAdapter for RecordingAdapter {
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        let mut calls = self.0.lock();
        let fail_once =
            options.model == "model-b" && !calls.iter().any(|call| call.model == "model-b");
        calls.push(options.clone());
        Box::pin(futures::stream::iter([StreamChunk::Finish {
            reason: if fail_once {
                FinishReason::Error {
                    failure: dsh_llm::LlmFailure {
                        message: "transient stream failure".into(),
                        code: "TRANSPORT".into(),
                        status: None,
                        provider_retry_after_ms: None,
                        request_id: None,
                    },
                }
            } else {
                FinishReason::Stop
            },
            replay_state: Some(
                serde_json::json!({"responseModel":format!("{}-resolved",options.model)}),
            ),
        }]))
    }
}
#[tokio::test]
async fn model_notice_is_persisted_once_and_reaches_first_switched_request() {
    let harness = harness().await;
    let calls = Arc::new(Mutex::new(Vec::new()));
    register_adapter(&harness, Arc::new(RecordingAdapter(calls.clone())));
    let _retry = harness
        .ctx
        .on(
            "agent/request-error",
            Arc::new(|_, _| {
                Box::pin(async { Some(cordis::arc(Some(dsh_agent::RequestErrorAction::Retry))) })
            }),
            Default::default(),
        )
        .await;
    let selection = Arc::new(Mutex::new(ModelSelectionRef::default()));
    selection.lock().current = Some(ModelSelection {
        provider: "test".into(),
        model: "model".into(),
        reasoning_effort: None,
        execution_mode: Default::default(),
    });
    let _dispose = install_model_selection(harness.agent.ctx(), selection.clone()).await;
    for (index, model) in ["model", "model-b", "model-b"].into_iter().enumerate() {
        selection.lock().current.as_mut().unwrap().model = model.into();
        harness.agent.followup(message(&format!("turn-{index}")));
        tokio::time::timeout(Duration::from_secs(3), harness.agent.when_idle())
            .await
            .unwrap();
    }
    let notices = |messages: &[dsh_llm::Message]| {
        messages.iter().filter(|message| matches!(&message.source,MessageSource::Plugin{plugin,..} if plugin=="model-selection")).count()
    };
    let calls = calls.lock();
    assert_eq!(calls.len(), 4);
    assert_eq!(notices(&calls[0].messages), 0);
    assert_eq!(notices(&calls[1].messages), 1);
    assert_eq!(notices(&calls[2].messages), 1);
    assert_eq!(notices(&calls[3].messages), 1);
    assert!(calls.iter().all(|call| !call.model.ends_with("-resolved")));
    let events = harness.agent.session().events();
    let final_message = events
        .iter()
        .rev()
        .find(|event| event.type_ == "assistant/message")
        .unwrap();
    assert_eq!(
        final_message.data["message"]["source"]["model"],
        "model-b-resolved"
    );
    assert_eq!(
        notices(&harness.agent.session().derive_messages().unwrap()),
        1
    );
}
