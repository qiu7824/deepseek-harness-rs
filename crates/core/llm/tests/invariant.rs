//! Stream-grammar invariant tests: Rust port of
//! `packages/llm/llm/tests/invariant.spec.ts` plus the companion
//! registration path.

use std::sync::Arc;

use cordis::Context;
use dsh_invariants::{InvariantConfig, InvariantRegistry};
use dsh_llm::*;
use futures::StreamExt;

fn failing_fail() -> Arc<dyn Fn(&str) + Send + Sync> {
    Arc::new(|message: &str| panic!("invariant: {message}"))
}

fn well_formed() -> Vec<StreamChunk> {
    vec![
        StreamChunk::BlockStart {
            index: 0,
            block_type: "text".to_string(),
        },
        StreamChunk::TextDelta {
            index: 0,
            text: "hi".to_string(),
        },
        StreamChunk::BlockEnd {
            index: 0,
            block: ContentBlock::Text {
                text: "hi".to_string(),
            },
        },
        StreamChunk::Usage {
            usage: TokenUsage::default(),
        },
        StreamChunk::Finish {
            reason: FinishReason::Stop,
            replay_state: None,
        },
    ]
}

/// Drive one wrapped stream to completion on a plain (non-tokio) thread;
/// returns the chunks or the rendered violation.
fn drain(chunks: Vec<StreamChunk>) -> Result<Vec<StreamChunk>, String> {
    let source: ChunkStream = Box::pin(futures::stream::iter(chunks));
    let mut wrapped = validate_stream(source, failing_fail());
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        futures::executor::block_on(async {
            let mut collected = Vec::new();
            while let Some(chunk) = wrapped.next().await {
                collected.push(chunk);
            }
            collected
        })
    }))
    .map_err(|payload| {
        payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_else(|| "listener panicked".to_string())
    })
}

#[test]
fn accepts_well_formed_streams() {
    let script = well_formed();
    assert_eq!(drain(script.clone()).expect("valid"), script);
}

#[test]
fn rejects_deltas_without_matching_open_blocks() {
    let chunks = vec![
        StreamChunk::TextDelta {
            index: 0,
            text: "x".to_string(),
        },
        StreamChunk::Finish {
            reason: FinishReason::Stop,
            replay_state: None,
        },
    ];
    let error = drain(chunks).expect_err("delta without an open block must fail");
    assert!(
        error.contains("text delta at index 0 requires an open text block, got undefined"),
        "got {error}"
    );

    let chunks = vec![
        StreamChunk::BlockStart {
            index: 0,
            block_type: "text".to_string(),
        },
        StreamChunk::ReasoningDelta {
            index: 0,
            text: "x".to_string(),
        },
        StreamChunk::Finish {
            reason: FinishReason::Stop,
            replay_state: None,
        },
    ];
    let error = drain(chunks).expect_err("type-mismatched delta must fail");
    assert!(
        error.contains("reasoning delta at index 0 requires an open reasoning block, got text"),
        "got {error}"
    );
}

#[test]
fn rejects_repeated_block_starts_and_mismatched_block_ends() {
    let chunks = vec![
        StreamChunk::BlockStart {
            index: 0,
            block_type: "text".to_string(),
        },
        StreamChunk::BlockStart {
            index: 0,
            block_type: "text".to_string(),
        },
    ];
    let error = drain(chunks).expect_err("repeated block-start must fail");
    assert!(
        error.contains("repeated block-start index 0"),
        "got {error}"
    );

    let chunks = vec![
        StreamChunk::BlockStart {
            index: 0,
            block_type: "text".to_string(),
        },
        StreamChunk::BlockEnd {
            index: 0,
            block: ContentBlock::Reasoning {
                text: String::new(),
            },
        },
    ];
    let error = drain(chunks).expect_err("mismatched block-end must fail");
    assert!(
        error.contains("block-end index 0 closes reasoning, expected text"),
        "got {error}"
    );

    let chunks = vec![StreamChunk::BlockEnd {
        index: 3,
        block: ContentBlock::Text {
            text: String::new(),
        },
    }];
    let error = drain(chunks).expect_err("unopened block-end must fail");
    assert!(
        error.contains("block-end index 3 has no open block"),
        "got {error}"
    );
}

#[test]
fn rejects_duplicate_usage_and_open_blocks_at_clean_finish() {
    let chunks = vec![
        StreamChunk::Usage {
            usage: TokenUsage::default(),
        },
        StreamChunk::Usage {
            usage: TokenUsage::default(),
        },
        StreamChunk::Finish {
            reason: FinishReason::Stop,
            replay_state: None,
        },
    ];
    let error = drain(chunks).expect_err("duplicate usage must fail");
    assert!(
        error.contains("emitted usage more than once"),
        "got {error}"
    );

    let chunks = vec![
        StreamChunk::BlockStart {
            index: 0,
            block_type: "text".to_string(),
        },
        StreamChunk::Finish {
            reason: FinishReason::Stop,
            replay_state: None,
        },
    ];
    let error = drain(chunks).expect_err("clean finish with open blocks must fail");
    assert!(
        error.contains("finished with 1 open block(s)"),
        "got {error}"
    );

    // Error and aborted finishes may legitimately strand open blocks.
    let aborted = vec![
        StreamChunk::BlockStart {
            index: 0,
            block_type: "text".to_string(),
        },
        StreamChunk::Finish {
            reason: FinishReason::Aborted {
                failure: LlmFailure {
                    message: "stopped".to_string(),
                    code: "ABORTED".to_string(),
                    status: None,
                    provider_retry_after_ms: None,
                    request_id: None,
                },
            },
            replay_state: None,
        },
    ];
    assert!(drain(aborted).is_ok());
}

#[test]
fn rejects_missing_terminal_finish_and_chunks_after_finish() {
    let chunks = vec![StreamChunk::Usage {
        usage: TokenUsage::default(),
    }];
    let error = drain(chunks).expect_err("missing terminal finish must fail");
    assert!(
        error.contains("ended without a terminal finish chunk"),
        "got {error}"
    );

    let chunks = vec![
        StreamChunk::Finish {
            reason: FinishReason::Stop,
            replay_state: None,
        },
        StreamChunk::Usage {
            usage: TokenUsage::default(),
        },
    ];
    let error = drain(chunks).expect_err("chunks after finish must fail");
    assert!(
        error.contains("emitted usage after terminal finish"),
        "got {error}"
    );
}

#[tokio::test]
async fn companion_wraps_runtime_streams_and_reads_the_registry() {
    let ctx = Context::root();
    InvariantRegistry::new(
        &ctx,
        InvariantConfig {
            enabled: true,
            ..InvariantConfig::default()
        },
    );
    let runtime = LlmRuntime::install(&ctx);
    apply_llm_invariant(&ctx);
    // The companion installer runs in a child fiber; let it activate.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // A well-formed adapter stream passes validation, and the
    // llm/adapters-updated listener reads the registry during registration.
    struct BrokenAdapter;
    impl LlmAdapter for BrokenAdapter {
        fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
            Box::pin(futures::stream::iter(vec![
                StreamChunk::TextDelta {
                    index: 0,
                    text: "x".to_string(),
                },
                StreamChunk::Finish {
                    reason: FinishReason::Stop,
                    replay_state: None,
                },
            ]))
        }
    }
    struct WellFormedAdapter;
    impl LlmAdapter for WellFormedAdapter {
        fn stream(&self, _options: &GenerateOptions) -> ChunkStream {
            Box::pin(futures::stream::iter(well_formed()))
        }
    }

    runtime
        .register_adapter(&ctx, vec!["ok".to_string()], Arc::new(WellFormedAdapter))
        .expect("register");
    runtime
        .register_adapter(&ctx, vec!["broken".to_string()], Arc::new(BrokenAdapter))
        .expect("register");

    let stream = runtime.stream(GenerateOptions {
        provider: "ok".to_string(),
        model: "m".to_string(),
        messages: Vec::new(),
        reasoning_effort: None,
        system: None,
        tools: None,
        temperature: None,
        max_tokens: None,
        stop: None,
        signal: None,
        session_id: None,
        purpose: None,
        agent_loop_request: false,
    });
    let mut stream = stream;
    while let Some(_chunk) = stream.next().await {}

    // A grammar violation surfaces to the consumer as the invariant failure.
    let stream = runtime.stream(GenerateOptions {
        provider: "broken".to_string(),
        model: "m".to_string(),
        messages: Vec::new(),
        reasoning_effort: None,
        system: None,
        tools: None,
        temperature: None,
        max_tokens: None,
        stop: None,
        signal: None,
        session_id: None,
        purpose: None,
        agent_loop_request: false,
    });
    let mut stream = stream;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        futures::executor::block_on(async { while let Some(_chunk) = stream.next().await {} })
    }));
    let payload = outcome.expect_err("grammar violation must surface");
    let rendered = payload
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "listener panicked".to_string());
    assert!(
        rendered.contains("invariant violated by \"@deepseek-ai/dsh-llm\""),
        "got {rendered}"
    );
    assert!(rendered.contains("text delta at index 0"), "got {rendered}");
}
