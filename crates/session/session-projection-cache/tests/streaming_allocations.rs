use cordis::{Context, arc, downcast};
use dsh_session::{SessionEvent, SessionHeader, session_id};
use dsh_session_persistence_jsonl::{
    JsonlCompression, JsonlConfig, JsonlSessionPersistence, compress_zstd_frame, event_lines,
    log_path, to_header_line,
};
use dsh_session_projection::{ProjectionDefinition, SessionProjectionRegistry};
use dsh_session_projection_cache::{Config, SessionProjectionCache};
use dsh_storage::Storage;
use dsh_storage_domain::{DomainFacility, DomainFacilityConfig};
use dsh_storage_json::JsonStorageBackend;
use serde_json::{Value, json};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Counted;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
fn add(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
}
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            add(l.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            add(l.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(p, l, size) };
        if !p.is_null() {
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
            add(size);
        }
        p
    }
}
#[global_allocator]
static ALLOCATOR: Counted = Counted;

#[tokio::test(flavor = "current_thread")]
async fn large_cold_projection_matches_its_count_without_whole_history_retention() {
    const EVENTS: u64 = 50_000;
    let root = std::env::temp_dir().join(format!(
        "projection-allocation-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let sessions = root.join("sessions");
    let storage_root = root.join("storages");
    let id = session_id("large-cold");
    let meta: SessionHeader =
        serde_json::from_value(json!({"id":id,"version":3,"createdAt":0,"isSeeded":false}))
            .unwrap();
    let path = log_path(
        &sessions.to_string_lossy(),
        None,
        &id,
        JsonlCompression::Zstd,
    );
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut file = std::fs::File::create(&path).unwrap();
    let header = format!(
        "{}\n",
        serde_json::to_string(&to_header_line(&meta, None).unwrap()).unwrap()
    );
    file.write_all(&compress_zstd_frame(header.as_bytes()).unwrap())
        .unwrap();
    for start in (0..EVENTS).step_by(256) {
        let rows:Vec<SessionEvent>=(start..(start+256).min(EVENTS)).map(|seq|serde_json::from_value(json!({"seq":seq,"time":seq,"type":"assistant/chunk","data":{"turn":1,"step":1,"chunk":{"type":"text-delta","index":0,"text":"small delta"}}})).unwrap()).collect();
        let body = event_lines(&rows, true) + "\n";
        file.write_all(&compress_zstd_frame(body.as_bytes()).unwrap())
            .unwrap();
    }
    drop(file);
    let ctx = Context::root();
    let registry = SessionProjectionRegistry::install(&ctx);
    let applied = Arc::new(AtomicUsize::new(0));
    let applied_events = applied.clone();
    registry
        .register(
            &ctx,
            ProjectionDefinition {
                key: "count".into(),
                state_version: 1,
                init: Arc::new(|_| arc(json!(0))),
                apply: Arc::new(move |state, _| {
                    applied_events.fetch_add(1, Ordering::Relaxed);
                    arc(json!(
                        downcast::<Value>(state).unwrap().as_u64().unwrap() + 1
                    ))
                }),
                view: Arc::new(|state| state.clone()),
                schema: Arc::new(|v| Ok(downcast::<Value>(v).unwrap().clone())),
            },
        )
        .unwrap();
    let persistence = JsonlSessionPersistence::install(
        &ctx,
        JsonlConfig {
            root: sessions.to_string_lossy().into_owned(),
            ..Default::default()
        },
    )
    .unwrap();
    let storage = Storage::install(&ctx);
    let backend = JsonStorageBackend::new(storage_root.to_string_lossy());
    let _registration = storage.backend.register("json", backend).unwrap();
    let facility = DomainFacility::install(
        &ctx,
        DomainFacilityConfig {
            backend: "json".into(),
            routes: Default::default(),
        },
    )
    .unwrap();
    let cache = SessionProjectionCache::install(
        &ctx,
        Config {
            write_every_events: 64,
            write_interval_ms: 1000,
        },
        &facility,
        persistence,
    )
    .unwrap();
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let (cold, concurrent) = tokio::join!(cache.cold_snapshot(&id), cache.cold_snapshot(&id));
    let cold = cold.unwrap();
    assert_eq!(concurrent.unwrap(), cold);
    assert_eq!(applied.load(Ordering::Relaxed), EVENTS as usize);
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    assert_eq!(cold.values["count"], EVENTS);
    assert_eq!(cold.as_of_seq, EVENTS as i64 - 1);
    println!("cold projection: events={EVENTS}, peak additional Rust allocation={peak}");
    assert!(
        peak < 8 * 1024 * 1024,
        "cold projection retained historical payloads: {peak}"
    );
    assert_eq!(cache.cold_snapshot(&id).await.unwrap(), cold);
    for dispose in ctx.fiber.disposables.clear() {
        dispose().await;
    }
    std::fs::remove_dir_all(root).unwrap();
}
