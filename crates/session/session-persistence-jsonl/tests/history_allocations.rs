use dsh_session::{
    SESSION_FORMAT_VERSION, SessionEvent, SessionHeader, SessionSeq, SurfaceOp, session_id,
};
use dsh_session_persistence::{SessionPersistenceApi, SessionReadWindowRequest};
use dsh_session_persistence_jsonl::{JsonlCompression, JsonlConfig, JsonlSessionPersistence};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! { static ALLOCATED: Cell<Option<usize>> = const { Cell::new(None) }; }
struct MeasuredAllocator;
fn record(size: usize) {
    let _ = ALLOCATED.try_with(|value| {
        if let Some(total) = value.get() {
            value.set(Some(total.saturating_add(size)));
        }
    });
}
unsafe impl GlobalAlloc for MeasuredAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record(layout.size());
        }
        pointer
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let pointer = unsafe { System.realloc(pointer, layout, size) };
        if !pointer.is_null() {
            record(size);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: MeasuredAllocator = MeasuredAllocator;

#[tokio::test(flavor = "current_thread")]
async fn small_history_page_allocations_do_not_scale_with_scan_ceiling() {
    let root = std::env::temp_dir().join(format!("dsh-history-alloc-{}", uuid::Uuid::new_v4()));
    let ctx = cordis::Context::root();
    let backend = JsonlSessionPersistence::install(
        &ctx,
        JsonlConfig {
            root: root.to_string_lossy().into_owned(),
            compression: JsonlCompression::Zstd,
            ..Default::default()
        },
    )
    .unwrap();
    let id = session_id("allocation-budget");
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
    let mut events = vec![];
    for seq in 0..8 {
        let message = seq == 0 || seq == 7;
        events.push(SessionEvent{seq:SessionSeq::new(seq).unwrap(),time:seq as i64,type_:if seq==0{"user/message"}else if seq==7{"assistant/message"}else{"assistant/chunk"}.into(),
            data:if seq==0{serde_json::json!({"id":"u","role":"user","source":{"kind":"user"},"content":[]})}else if seq==7{serde_json::json!({"turn":1,"step":1,"message":{"id":"a","role":"assistant","source":{"kind":"model","provider":"mock","model":"test"},"content":[]}})}else{serde_json::json!({"turn":1,"step":1,"chunk":{"type":"text-delta","index":0,"text":"hello"}})},
            surface_op:message.then_some(SurfaceOp::Append),source_event_seqs:None,ignorable:None});
    }
    backend.append(&id, &events).await.unwrap();
    let mut measured = vec![];
    for max_events in [64, 65536] {
        // Warm filesystem/header checks before measuring the synchronous decode allocations.
        let request = SessionReadWindowRequest {
            before_seq: None,
            max_messages: 2,
            max_events,
        };
        backend.read_window(&id, request).await.unwrap();
        ALLOCATED.with(|value| value.set(Some(0)));
        let window = backend.read_window(&id, request).await;
        let bytes = ALLOCATED.with(|value| value.replace(None).unwrap());
        assert_eq!(window.unwrap().events, events);
        measured.push(bytes);
    }
    for dispose in ctx.fiber.disposables.clear() {
        dispose().await;
    }
    tokio::fs::remove_dir_all(&root).await.unwrap();
    println!(
        "history allocation bytes: small_ceiling={}, large_ceiling={}",
        measured[0], measured[1]
    );
    assert!(
        measured[1] <= measured[0] * 2 + 256 * 1024,
        "a tiny page eagerly allocates its safety ceiling: {measured:?}"
    );
}
