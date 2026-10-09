use super::*;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("artifact-index-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(p.join("workspace")).unwrap();
        Self(p)
    }
    fn workspace(&self) -> PathBuf {
        fs::canonicalize(self.0.join("workspace")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self.0.starts_with(std::env::temp_dir()));
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn fast_open_does_not_scan_or_invent_new_files_before_a_baseline() {
    let f = Fixture::new();
    let root = f.workspace();
    let service = Artifacts::new(&f.0);
    fs::write(root.join("existing.txt"), "before").unwrap();
    assert!(
        service.list_mode("owner", &root, None, false).unwrap()["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let index = service.index("owner", &root).unwrap();
    assert!(!index.lock().baseline_ready);
    assert!(index.lock().baseline.is_empty());
    service.baseline("owner", &root).unwrap();
    fs::write(root.join("existing.txt"), "after with a different size").unwrap();
    let list = service.list_mode("owner", &root, None, true).unwrap();
    assert_eq!(list["entries"][0]["change"], "modified");
    assert_eq!(list["entries"][0]["path"], "existing.txt");
}
#[test]
fn warm_index_replays_only_new_events_and_preserves_delivery_classification() {
    let f = Fixture::new();
    let root = f.workspace();
    let service = Artifacts::new(&f.0);
    fs::write(root.join("report.md"), "report").unwrap();
    let session =
        dsh_session::Session::create(session_id("artifact-index-owner"), None, None, None).unwrap();
    session
        .append(
            "deliverables/presented",
            json!({"files":[{"path":"report.md"}]}),
            None,
        )
        .unwrap();
    let first = service
        .list_mode("owner", &root, Some(&session), false)
        .unwrap();
    assert_eq!(first["entries"][0]["source"], "delivery");
    let file = service.root.join(format!("{}.json", digest(b"owner")));
    let modified = fs::metadata(&file).unwrap().modified().unwrap();
    service
        .list_mode("owner", &root, Some(&session), false)
        .unwrap();
    assert_eq!(modified, fs::metadata(&file).unwrap().modified().unwrap());
    assert_eq!(
        service.index("owner", &root).unwrap().lock().next_event_seq,
        session.seq().get()
    );
    session
        .append(
            "tool/result",
            json!({"meta":{"path":"report.md","before":"report","after":"edited"},
                "message":{"id":"result-1","role":"tool","toolCallId":"call-1",
                    "source":{"kind":"tool","callId":"call-1"},"content":[{"type":"text","text":"edited"}]}}),
            Some(dsh_session::SurfaceIntent { surface_op: dsh_session::SurfaceOp::Append, source_event_seqs: None }),
        )
        .unwrap();
    assert_eq!(
        service
            .list_mode("owner", &root, Some(&session), false)
            .unwrap()["entries"][0]["source"],
        "delivery"
    );
}
#[test]
fn legacy_baselines_are_preserved_and_unsafe_cached_paths_are_not_read() {
    let f = Fixture::new();
    let root = f.workspace();
    let service = Artifacts::new(&f.0);
    let old = stamp(&{
        let p = root.join("file.txt");
        fs::write(&p, "old").unwrap();
        p
    })
    .unwrap();
    fs::create_dir_all(&service.root).unwrap();
    persist_json(&service.root.join(format!("{}.json",digest(b"owner"))),&json!({"root":root,"baseline":{"file.txt":old},"entries":{"escape":{"path":"../private.txt","change":"created","source":"workspace","updatedAt":0}},"truncated":false})).unwrap();
    fs::write(root.join("file.txt"), "new contents").unwrap();
    service.baseline("owner", &root).unwrap();
    let rows = service.list_mode("owner", &root, None, true).unwrap();
    assert_eq!(rows["entries"].as_array().unwrap().len(), 1);
    assert_eq!(rows["entries"][0]["change"], "modified");
}
#[test]
fn one_session_lock_does_not_block_another_session_index() {
    let f = Fixture::new();
    let root = f.workspace();
    let service = Artifacts::new(&f.0);
    let index = service.index("busy", &root).unwrap();
    let _guard = index.lock();
    let other = service.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        tx.send(other.list_mode("other", &root, None, false).is_ok())
            .unwrap()
    });
    assert!(rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap());
    worker.join().unwrap();
}

#[test]
fn completed_turn_requests_one_reconciliation_for_shell_created_files() {
    let f = Fixture::new();
    let root = f.workspace();
    let service = Artifacts::new(&f.0);
    let session =
        dsh_session::Session::create(session_id("artifact-turn-owner"), None, None, None).unwrap();
    service.baseline("owner", &root).unwrap();
    fs::write(root.join("shell-output.txt"), "produced by a shell command").unwrap();
    session
        .append(
            "turn/end",
            json!({"turn":1,"reason":{"kind":"success"}}),
            None,
        )
        .unwrap();
    assert_eq!(
        service
            .list_mode("owner", &root, Some(&session), false)
            .unwrap()["refreshNeeded"],
        true
    );
    let refreshed = service
        .list_mode("owner", &root, Some(&session), true)
        .unwrap();
    assert_eq!(refreshed["refreshNeeded"], false);
    assert_eq!(refreshed["entries"][0]["path"], "shell-output.txt");
    assert_eq!(
        service
            .list_mode("owner", &root, Some(&session), false)
            .unwrap()["refreshNeeded"],
        false
    );
}

#[test]
fn a_pending_baseline_does_not_block_cached_reads_for_its_session() {
    let f = Fixture::new();
    let root = f.workspace();
    let service = Artifacts::new(&f.0);
    let index = service.index("owner", &root).unwrap();
    let gate = index.lock().baseline_gate.clone();
    let _pending = gate.lock();
    assert!(
        service.list_mode("owner", &root, None, false).unwrap()["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
