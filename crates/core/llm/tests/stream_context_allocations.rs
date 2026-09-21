use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use cordis::Context;
use dsh_llm::{
    ChunkStream, ContentBlock, FinishReason, GenerateOptions, LlmAdapter, LlmRuntime,
    MessageSource, StreamChunk,
};
use futures::StreamExt;

static LIVE: AtomicUsize = AtomicUsize::new(0);
thread_local! { static ALLOCATED: Cell<Option<usize>> = const { Cell::new(None) }; }
struct CountedAllocator;
fn allocated(size: usize) {
    LIVE.fetch_add(size, Ordering::Relaxed);
    let _ = ALLOCATED.try_with(|counter| {
        if let Some(total) = counter.get() {
            counter.set(Some(total.saturating_add(size)));
        }
    });
}
unsafe impl GlobalAlloc for CountedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let pointer = unsafe { System.realloc(pointer, layout, size) };
        if !pointer.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            allocated(size);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountedAllocator = CountedAllocator;

struct DeltaAdapter;
impl LlmAdapter for DeltaAdapter {
    fn stream(&self, options: &GenerateOptions) -> ChunkStream {
        assert!(
            matches!(&options.messages[0].content[0], ContentBlock::Text { text } if text.len() == 1024 * 1024)
        );
        Box::pin(futures::stream::iter(
            (0..64)
                .map(|_| StreamChunk::TextDelta {
                    index: 0,
                    text: "delta".into(),
                })
                .chain(std::iter::once(StreamChunk::Finish {
                    reason: FinishReason::Stop,
                    replay_state: None,
                })),
        ))
    }
}

struct PanickingAdapter;
impl LlmAdapter for PanickingAdapter {
    fn stream(&self, _: &GenerateOptions) -> ChunkStream {
        Box::pin(futures::stream::unfold(0, |index| async move {
            assert!(index == 0, "controlled iteration failure");
            Some((
                StreamChunk::TextDelta {
                    index: 0,
                    text: "first".into(),
                },
                index + 1,
            ))
        }))
    }
}

fn options(provider: &str) -> GenerateOptions {
    GenerateOptions {
        provider: provider.into(),
        model: "fixture".into(),
        reasoning_effort: None,
        messages: vec![dsh_llm::create_user_message(
            vec![ContentBlock::Text {
                text: "x".repeat(1024 * 1024),
            }],
            MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        )],
        system: None,
        tools: None,
        temperature: None,
        max_tokens: None,
        stop: None,
        signal: None,
        session_id: None,
        purpose: None,
        agent_loop_request: false,
        telemetry: None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn stream_deltas_do_not_clone_or_retain_the_dispatched_context() {
    let ctx = Context::root();
    let runtime = LlmRuntime::install(&ctx);
    runtime
        .register_adapter(
            &ctx,
            vec!["allocation-fixture".into()],
            Arc::new(DeltaAdapter),
        )
        .unwrap();
    let baseline = LIVE.load(Ordering::Relaxed);
    let mut stream = runtime.stream(options("allocation-fixture"));
    assert!(matches!(
        stream.next().await,
        Some(StreamChunk::TextDelta { .. })
    ));
    let retained = LIVE.load(Ordering::Relaxed).saturating_sub(baseline);
    assert!(
        retained < 256 * 1024,
        "runtime retained the complete context after adapter dispatch: {retained}"
    );
    ALLOCATED.with(|counter| counter.set(Some(0)));
    let mut chunks = 1;
    while let Some(chunk) = stream.next().await {
        assert!(matches!(
            chunk,
            StreamChunk::TextDelta { .. }
                | StreamChunk::Finish {
                    reason: FinishReason::Stop,
                    ..
                }
        ));
        chunks += 1;
    }
    let allocated = ALLOCATED.with(|counter| counter.replace(None).unwrap());
    assert_eq!(chunks, 65);
    assert!(
        allocated < 256 * 1024,
        "stream deltas copied context instead of advancing the adapter: {allocated}"
    );
    println!(
        "stream context bytes: retained={retained}, remaining_delta_allocations={allocated}, chunks={chunks}"
    );

    runtime
        .register_adapter(
            &ctx,
            vec!["panic-fixture".into()],
            Arc::new(PanickingAdapter),
        )
        .unwrap();
    for aborted in [false, true] {
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let mut request = options("panic-fixture");
        request.signal = Some(Arc::new(move || signal.load(Ordering::Acquire)));
        let mut stream = runtime.stream(request);
        assert!(matches!(
            stream.next().await,
            Some(StreamChunk::TextDelta { .. })
        ));
        cancelled.store(aborted, Ordering::Release);
        let Some(StreamChunk::Finish { reason, .. }) = stream.next().await else {
            panic!("iteration failure must finish the stream")
        };
        if aborted {
            assert!(matches!(reason, FinishReason::Aborted { .. }));
        } else {
            assert!(matches!(reason, FinishReason::Error { .. }));
        }
        assert!(stream.next().await.is_none());
    }
    ctx.fiber.dispose().await;
}
