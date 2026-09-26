use dsh_session::{
    SESSION_FORMAT_VERSION, SessionEvent, SessionHeader, SessionSeq, SessionStore, session_id,
};
use dsh_session_persistence::SessionPersistenceApi;
use dsh_session_persistence_jsonl::{JsonlCompression, JsonlConfig, JsonlSessionPersistence};
use std::alloc::{GlobalAlloc, Layout, System};
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

#[tokio::test(flavor = "current_thread")]
async fn prepared_resume_archives_complete_history_and_preserves_inspection_prefix() {
    const ROWS: usize = 32;
    const ROW_BYTES: usize = 128 * 1024;
    const PAYLOAD: usize = ROWS * ROW_BYTES;
    let root = std::env::temp_dir().join(format!("dsh-prepared-alloc-{}", uuid::Uuid::new_v4()));
    let ctx = cordis::Context::root();
    SessionStore::install(&ctx);
    let backend = JsonlSessionPersistence::install(
        &ctx,
        JsonlConfig {
            root: root.to_string_lossy().into_owned(),
            compression: JsonlCompression::Zstd,
            prepared_session_cache_size: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let id = session_id("prepared-owned-history");
    backend
        .create(
            SessionHeader {
                version: SESSION_FORMAT_VERSION,
                id: id.clone(),
                created_at: 1,
                cwd: None,
                parent_session: None,
                is_seeded: false,
                origin: None,
                delegation_depth: None,
                agent_preset: None,
            },
            None,
        )
        .await
        .unwrap();
    // Separate compressed frames make whole-log plaintext inflation visible.
    for seq in 0..ROWS {
        backend
            .append(
                &id,
                &[SessionEvent {
                    seq: SessionSeq::new(seq as u64).unwrap(),
                    time: seq as i64,
                    type_: "tools/discovery".into(),
                    data: serde_json::json!({"text": "x".repeat(ROW_BYTES)}),
                    surface_op: None,
                    source_event_seqs: None,
                    ignorable: None,
                }],
            )
            .await
            .unwrap();
    }
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let prepared = backend.prepare(&id).await.unwrap();
    let retained = LIVE.load(Ordering::Relaxed).saturating_sub(baseline);
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    println!("prepared history bytes: payload={PAYLOAD}, retained={retained}, peak={peak}");
    assert!(
        retained <= 256 * 1024,
        "prepared resume retained historical payloads instead of its index: {retained}"
    );
    assert!(
        peak <= ROW_BYTES * 8,
        "cold resume exceeded its per-event working memory: {peak}"
    );
    assert_eq!(prepared.session.first_live_seq().get(), ROWS as u64);
    assert_eq!(prepared.session.seq().get(), ROWS as u64 + 1);
    let mut visited = 0;
    prepared
        .session
        .visit_events(0, Some(ROWS as u64), |event| {
            assert_eq!(event.seq.get(), visited);
            assert_eq!(event.data["text"].as_str().unwrap().len(), ROW_BYTES);
            visited += 1;
            Ok(true)
        })
        .unwrap();
    assert_eq!(visited, ROWS as u64);
    drop(prepared);
    let inspection = backend.inspect(&id).await.unwrap();
    assert_eq!(inspection.events.len(), ROWS);
    assert!(
        inspection
            .events
            .iter()
            .all(|event| event.type_ == "tools/discovery"
                && event.data["text"].as_str().unwrap().len() == ROW_BYTES)
    );
    ctx.fiber.dispose().await;
    std::fs::remove_dir_all(root).unwrap();
}
