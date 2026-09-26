use dsh_session::{SessionEvent, session_id};
use dsh_session_persistence::{HistoryWindowSink, SessionPersistenceApi, SessionReadWindowRequest};
use dsh_session_persistence_jsonl::{
    JsonlCompression, JsonlConfig, JsonlSessionPersistence, log_path,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountedAllocator;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
fn allocated(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
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

struct CompletedStreamSink {
    first_seq: u64,
    count: usize,
    inspected: usize,
    pushed: usize,
    output: Vec<SessionEvent>,
}
impl HistoryWindowSink for CompletedStreamSink {
    fn inspect(&mut self, event: &SessionEvent) -> Result<(), String> {
        assert_eq!(event.seq.get(), self.first_seq + self.inspected as u64);
        self.inspected += 1;
        Ok(())
    }
    fn push(&mut self, mut event: SessionEvent) -> Result<(), String> {
        assert_eq!(self.inspected, self.count);
        assert_eq!(event.seq.get(), self.first_seq + self.pushed as u64);
        self.pushed += 1;
        if event.type_ == "assistant/message" {
            event.source_event_seqs = None;
            self.output.push(event);
        }
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<(Vec<SessionEvent>, bool), String> {
        assert_eq!(self.pushed, self.count);
        Ok((self.output, false))
    }
}

#[tokio::test(flavor = "current_thread")]
async fn metadata_scan_and_typed_references_do_not_inflate_a_large_stream() {
    const CHUNKS: u64 = 131_073;
    let root = std::env::temp_dir().join(format!("dsh-native-peak-{}", uuid::Uuid::new_v4()));
    let id = session_id("native-peak");
    let root_string = root.to_string_lossy().into_owned();
    let path = log_path(&root_string, None, &id, JsonlCompression::None);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let mut file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        writeln!(file, "{{\"type\":\"session\",\"version\":4,\"id\":\"native-peak\",\"createdAt\":0,\"isSeeded\":false,\"delegationDepth\":0}}").unwrap();
        for seq in 0..CHUNKS {
            writeln!(
                file,
                "{{\"type\":\"assistant/chunk\",\"seq\":{seq},\"time\":0,\"data\":{{\"turn\":1}}}}"
            )
            .unwrap();
        }
        writeln!(file, "{{\"type\":\"assistant/message\",\"seq\":{CHUNKS},\"time\":0,\"data\":{{}},\"surfaceOp\":\"append\",\"sourceEventSeqs\":[[0,{}]]}}", CHUNKS - 1).unwrap();
        for (seq, kind) in [
            (CHUNKS + 1, "user/message"),
            (CHUNKS + 2, "assistant/message"),
        ] {
            writeln!(file, "{{\"type\":\"{kind}\",\"seq\":{seq},\"time\":0,\"data\":{{}},\"surfaceOp\":\"append\"}}").unwrap();
        }
    }
    let ctx = cordis::Context::root();
    let backend = JsonlSessionPersistence::install(
        &ctx,
        JsonlConfig {
            root: root_string,
            compression: JsonlCompression::None,
            ..Default::default()
        },
    )
    .unwrap();
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let tail = backend
        .read_window(
            &id,
            SessionReadWindowRequest {
                before_seq: None,
                max_messages: 2,
                max_events: 65_536,
            },
        )
        .await
        .unwrap();
    let tail_peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    assert_eq!(
        tail.events
            .iter()
            .map(|event| event.seq.get())
            .collect::<Vec<_>>(),
        vec![CHUNKS + 1, CHUNKS + 2]
    );
    assert!(tail.has_more);
    assert!(
        tail_peak < 5 * 1024 * 1024,
        "metadata-only scan inflated events or provenance: {tail_peak}"
    );
    drop(tail);

    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let previous = backend
        .read_window(
            &id,
            SessionReadWindowRequest {
                before_seq: Some(CHUNKS + 1),
                max_messages: 1,
                max_events: 4,
            },
        )
        .await
        .unwrap();
    let previous_peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    assert_eq!(previous.events.len(), 4);
    let sources = previous
        .events
        .last()
        .unwrap()
        .source_event_seqs
        .as_ref()
        .unwrap();
    assert_eq!(sources.len(), CHUNKS as usize);
    assert!(
        sources
            .iter()
            .enumerate()
            .all(|(index, seq)| *seq == index as u64)
    );
    assert!(previous.has_more);
    assert!(
        previous_peak < 3 * 1024 * 1024,
        "typed provenance retained a duplicate JSON array: {previous_peak}"
    );
    println!("native history peak bytes: tail={tail_peak}, previous={previous_peak}");
    drop(previous);
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let projected = backend
        .read_window_with_sink(
            &id,
            SessionReadWindowRequest {
                before_seq: Some(CHUNKS + 1),
                max_messages: 1,
                max_events: 65_536,
            },
            Box::new(CompletedStreamSink {
                first_seq: CHUNKS - 65_535,
                count: 65_536,
                inspected: 0,
                pushed: 0,
                output: Vec::new(),
            }),
        )
        .await
        .unwrap();
    let projected_peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    assert_eq!(projected.events.len(), 1);
    assert_eq!(projected.events[0].seq.get(), CHUNKS);
    assert!(projected.has_more);
    assert!(
        projected_peak < 5 * 1024 * 1024,
        "streaming projection retained its raw 65K event page: {projected_peak}"
    );
    println!("projected 65K-event history peak bytes: {projected_peak}");
    drop(projected);
    ctx.fiber.dispose().await;
    std::fs::remove_dir_all(root).unwrap();
}
