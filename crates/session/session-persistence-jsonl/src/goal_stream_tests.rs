use super::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::Write;

thread_local! { static LARGEST_SCAN_ALLOCATION: Cell<Option<usize>> = const { Cell::new(None) }; }
struct ScanAllocator;
fn allocation(size: usize) {
    let _ = LARGEST_SCAN_ALLOCATION.try_with(|value| {
        if let Some(prior) = value.get() {
            value.set(Some(prior.max(size)));
        }
    });
}
unsafe impl GlobalAlloc for ScanAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocation(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        allocation(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        allocation(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: ScanAllocator = ScanAllocator;

fn event(seq: u64, kind: &str, data: serde_json::Value) -> SessionEvent {
    SessionEvent {
        type_: kind.into(),
        seq: dsh_session::SessionSeq::new(seq).unwrap(),
        time: 1,
        data,
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}
fn change(seq: u64, revision: u64, objective: &str) -> SessionEvent {
    event(
        seq,
        "goal/change",
        serde_json::json!({"kind":"goal/change","version":1,"operation":if revision==1{"create"}else{"edit"},"goal":{"id":"goal-1","revision":revision,"objective":objective,"phase":"active","maxGoalRounds":8},"roundsStarted":0,"createdAt":1,"updatedAt":revision}),
    )
}
fn visitor() -> (Arc<Mutex<Vec<SessionEvent>>>, NonpackedEventVisitor) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let output = seen.clone();
    (
        seen,
        Arc::new(move |events| {
            output.lock().extend_from_slice(events);
            Ok(true)
        }),
    )
}
fn with_allocation_limit(run: impl FnOnce() -> Result<(), String>) -> (Result<(), String>, usize) {
    LARGEST_SCAN_ALLOCATION.with(|value| value.set(Some(0)));
    let result = run();
    let peak = LARGEST_SCAN_ALLOCATION.with(|value| value.replace(None).unwrap());
    (result, peak)
}

#[test]
fn bounded_goal_reader_skips_large_message_content_and_packed_payloads() {
    let large = "x".repeat(8 * 1024 * 1024);
    let goal = change(3, 1, "A");
    let bytes = format!(
        "{{\"type\":\"session\",\"version\":3}}\n{{\"type\":\"assistant/message\",\"seq\":0,\"time\":1,\"data\":{{\"message\":\"{large}\"}}}}\n{{\"type\":\"text-chunks\",\"seq0\":1,\"time0\":1,\"data\":[\"{large}\"]}}\n{{\"type\":\"user/message\",\"seq\":2,\"time\":1,\"data\":{{\"content\":[\"{large}\"],\"source\":{{\"kind\":\"goal\",\"goalId\":\"goal-1\",\"revision\":1,\"round\":1}}}}}}\n{}\n",
        serde_json::to_string(&goal).unwrap()
    );
    let (seen, visit) = visitor();
    let (result, largest) =
        with_allocation_limit(|| goal_stream::visit_reader(bytes.as_bytes(), &visit));
    result.unwrap();
    assert!(
        largest < 512 * 1024,
        "unrelated data caused a large allocation: {largest}"
    );
    let seen = seen.lock();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].type_, "user/message");
    assert!(seen[0].data.get("content").is_none());
    assert_eq!(seen[0].data["source"]["goalId"], "goal-1");
    assert_eq!(seen[1], goal);
}

#[test]
fn bounded_goal_reader_refuses_large_goal_data_noncanonical_order_and_torn_tail() {
    let huge = serde_json::to_string(&change(0, 1, &"x".repeat(2 * 1024 * 1024))).unwrap();
    let (_, visit) = visitor();
    let (result, largest) =
        with_allocation_limit(|| goal_stream::visit_reader(huge.as_bytes(), &visit));
    assert!(result.unwrap_err().contains("byte limit"));
    assert!(
        largest < 512 * 1024,
        "goal cap was checked after allocation: {largest}"
    );
    let noncanonical = format!(
        "{{\"data\":\"{}\",\"type\":\"assistant/message\",\"seq\":0,\"time\":1}}",
        "x".repeat(2 * 1024 * 1024)
    );
    let (result, largest) =
        with_allocation_limit(|| goal_stream::visit_reader(noncanonical.as_bytes(), &visit));
    assert!(result.unwrap_err().contains("type-first"));
    assert!(largest < 512 * 1024);
    let torn = format!(
        "{}\n{{\n",
        serde_json::to_string(&change(0, 1, "A")).unwrap()
    );
    assert!(
        goal_stream::visit_reader(torn.as_bytes(), &visit).is_err(),
        "a torn record cannot be mistaken for clean EOF"
    );
}

#[test]
fn bounded_goal_reader_early_stop_does_not_parse_the_next_record() {
    let bytes = format!(
        "{}\n{{broken",
        serde_json::to_string(&change(0, 1, "A")).unwrap()
    );
    let visit: NonpackedEventVisitor = Arc::new(|events| {
        assert_eq!(events[0].type_, "goal/change");
        Ok(false)
    });
    goal_stream::visit_reader(bytes.as_bytes(), &visit).unwrap();
}

fn write_log(root: &Path, id: &SessionId, compression: JsonlCompression) -> PathBuf {
    let header = SessionHeader {
        version: dsh_session::SESSION_FORMAT_VERSION,
        id: id.clone(),
        created_at: 1,
        cwd: Some("C:/goal-scan".into()),
        parent_session: None,
        is_seeded: false,
        origin: None,
        delegation_depth: None,
        agent_preset: Some("standard".into()),
    };
    let path = log_path(
        &root.to_string_lossy(),
        header.cwd.as_deref(),
        id,
        compression,
    );
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let header = serde_json::to_string(&crate::format::to_header_line(&header, None).unwrap())
        .unwrap()
        + "\n";
    let body = event_lines(
        &[
            change(0, 1, "A"),
            event(
                1,
                "assistant/message",
                serde_json::json!({"content":"x".repeat(4*1024*1024)}),
            ),
            change(2, 2, "B"),
        ],
        true,
    ) + "\n";
    let mut file = std::fs::File::create(&path).unwrap();
    for bytes in [header.as_bytes(), body.as_bytes()] {
        match compression {
            JsonlCompression::None => file.write_all(bytes).unwrap(),
            JsonlCompression::Zstd => file
                .write_all(&compress_zstd_frame(bytes).unwrap())
                .unwrap(),
        }
    }
    path
}

fn cleanup(root: &Path) {
    let resolved = std::fs::canonicalize(root).unwrap();
    let parent = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    assert_eq!(resolved.parent(), Some(parent.as_path()));
    assert!(
        resolved
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("dsh-goal-scan-")
    );
    std::fs::remove_dir_all(resolved).unwrap();
}

#[tokio::test]
async fn bounded_goal_backend_handles_plain_and_multiframe_zstd_without_materialization() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let root = std::env::temp_dir().join(format!("dsh-goal-scan-{}", uuid::Uuid::new_v4()));
        let id = dsh_session::session_id("scan-target");
        let path = write_log(&root, &id, compression);
        let before = std::fs::read(&path).unwrap();
        let ctx = Context::root();
        let backend = JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root: root.to_string_lossy().into(),
                compression,
                ..Default::default()
            },
        )
        .unwrap();
        let (seen, visit) = visitor();
        SessionPersistenceApi::visit_goal_events_bounded(backend.as_ref(), &id, visit)
            .await
            .unwrap();
        let seen = seen.lock();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0], change(0, 1, "A"));
        assert_eq!(seen[1], change(2, 2, "B"));
        drop(seen);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "goal reads must not migrate or rewrite the source"
        );
        assert!(
            ctx.get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
                .is_none(),
            "cold goal scanning does not compose or materialize a session store"
        );
        ctx.fiber.dispose().await;
        cleanup(&root);
    }
}

#[test]
fn bounded_goal_zstd_reader_rejects_frames_above_the_writer_window() {
    let root = std::env::temp_dir().join(format!("dsh-goal-scan-{}", uuid::Uuid::new_v4()));
    let id = dsh_session::session_id("large-window");
    let path = write_log(&root, &id, JsonlCompression::Zstd);
    let mut bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes[5], 0x58);
    bytes[5] = 0x60;
    std::fs::write(&path, bytes).unwrap();
    let (_, visit) = visitor();
    assert!(goal_stream::visit_path(&path, JsonlCompression::Zstd, visit).is_err());
    cleanup(&root);
}
