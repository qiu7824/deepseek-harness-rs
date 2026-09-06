use super::*;
#[test]
#[cfg(target_os = "macos")]
fn system_temp_alias_is_accepted_but_user_alias_is_not() {
    checked_path(Path::new("/var/folders")).unwrap();
    let fixture = Fixture::new();
    fs::create_dir_all(&fixture.0).unwrap();
    let link = fixture.0.join("var");
    std::os::unix::fs::symlink("/private/var", &link).unwrap();
    assert!(checked_path(&link).is_err());
    let store = Store::open(fixture.0.join("managed")).unwrap();
    assert_eq!(
        store.root(),
        fs::canonicalize(fixture.0.join("managed")).unwrap()
    );
}

#[test]
#[cfg(unix)]
fn refusing_an_unknown_directory_preserves_its_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fs::create_dir_all(&fixture.0).unwrap();
    fs::write(fixture.0.join("input.txt"), "original").unwrap();
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Store::open(&fixture.0).is_err());
    assert_eq!(
        fs::metadata(&fixture.0).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(
        fs::read_to_string(fixture.0.join("input.txt")).unwrap(),
        "original"
    );
}

#[test]
fn settings_numbers_accept_integral_schema_values_without_discarding_preferences() {
    let policy=Policy::from_json(serde_json::json!({"keepDays":9.0,"failedDays":15.0,"softLimitGib":30.0,"location":"D:/managed"})).unwrap();
    assert_eq!(policy.keep_days, 9);
    assert_eq!(policy.soft_limit_gib, 30);
    assert_eq!(policy.location, "D:/managed");
    assert!(Policy::from_json(serde_json::json!({"keepDays":2.5})).is_err());
}
pub(super) struct Fixture(pub(super) PathBuf);

#[test]
fn surviving_process_identity_protects_resources_after_lease_loss() {
    let fixture = Fixture::new();
    let store = Store::open(&fixture.0).unwrap();
    let id = {
        let lease = store
            .allocate("owner", "project", "run", "interrupted host")
            .unwrap();
        lease.attach_process(std::process::id()).unwrap();
        lease.id().to_string()
    };
    assert!(store.list_brief().unwrap()[0].busy);
    assert!(store.quarantine(&id).is_err());
    assert!(
        store
            .collect(&Policy::default(), now() + 30 * DAY, false)
            .unwrap()
            .is_empty()
    );
}
impl Fixture {
    pub(super) fn new() -> Self {
        Self(std::env::temp_dir().join(format!("dsh-resources-{}", uuid::Uuid::new_v4())))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn live_leases_candidates_and_pins_survive_collection_and_reopen() {
    let fixture = Fixture::new();
    let store = Store::open(&fixture.0).unwrap();
    let mut run = store
        .allocate("session-a", "project", "run", "测试")
        .unwrap();
    let id = run.id().to_string();
    store.write_text(&id, "log.txt", "完整日志").unwrap();
    let second = Store::open(&fixture.0).unwrap();
    assert!(second.list().unwrap()[0].busy);
    assert!(second.quarantine(&id).is_err());
    run.finish(true).unwrap();
    store.retain(&id, true, false).unwrap();
    assert!(second.quarantine(&id).is_err());
    store.retain(&id, false, true).unwrap();
    assert!(second.quarantine(&id).is_err());
    store.retain(&id, false, false).unwrap();
    second.quarantine(&id).unwrap();
    assert_eq!(second.list().unwrap()[0].state, "quarantined");
    assert_eq!(
        second.read_text(&id, "log.txt", 0, 100).unwrap(),
        "完整日志"
    );
    second.restore(&id).unwrap();
    assert!(run.path().join("log.txt").is_file());
}

#[test]
fn unknown_paths_and_escape_are_never_adopted() {
    let fixture = Fixture::new();
    fs::create_dir_all(&fixture.0).unwrap();
    fs::write(fixture.0.join("user.txt"), "input").unwrap();
    assert!(Store::open(&fixture.0).is_err());
    let store = Store::open(fixture.0.join("managed")).unwrap();
    let lease = store
        .allocate("session", "project", "script", "script")
        .unwrap();
    assert!(store.path(lease.id(), "../user.txt").is_err());
    assert!(store.path("../", "user.txt").is_err());
    assert!(store.path(lease.id(), "C:/user.txt").is_err() || !cfg!(windows));
    assert_eq!(
        fs::read_to_string(fixture.0.join("user.txt")).unwrap(),
        "input"
    );
}

#[test]
fn cache_is_reused_and_every_concurrent_lease_protects_it() {
    let fixture = Fixture::new();
    let store = Store::open(&fixture.0).unwrap();
    let mut first = store.cache("project", "toolchain-a").unwrap();
    let mut second = store.cache("project", "toolchain-a").unwrap();
    let other = store.cache("project", "toolchain-b").unwrap();
    assert_eq!(first.path(), second.path());
    assert_ne!(first.path(), other.path());
    first.finish(true).unwrap();
    assert!(store.quarantine(second.id()).is_err());
    second.finish(true).unwrap();
    store.quarantine(first.id()).unwrap();
    let mut resumed = store.cache("project", "toolchain-a").unwrap();
    assert_eq!(resumed.id(), first.id());
    assert!(resumed.path().is_dir());
    resumed.finish(true).unwrap();
}

#[test]
fn concurrent_cache_admission_and_collection_do_not_deadlock() {
    let fixture = Fixture::new();
    let store = Store::open(&fixture.0).unwrap();
    let mut initial = store.cache("project", "toolchain").unwrap();
    let id = initial.id().to_string();
    initial.finish(true).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    for collect in [false, true] {
        let store = store.clone();
        let id = id.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            for _ in 0..40 {
                if collect {
                    let _ = store.quarantine(&id);
                } else {
                    let mut lease = store.cache("project", "toolchain").unwrap();
                    lease.finish(true).unwrap();
                }
            }
            tx.send(()).unwrap();
        });
    }
    for _ in 0..2 {
        rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    }
}

#[test]
fn interrupted_runs_keep_failure_retention_and_recovery_window() {
    let fixture = Fixture::new();
    let store = Store::open(&fixture.0).unwrap();
    let id = {
        let run = store.allocate("s", "p", "run", "crash").unwrap();
        run.id().to_string()
    };
    let started = store.get(&id).unwrap().updated_at;
    assert!(
        store
            .collect(&Policy::default(), started + 8 * DAY, false)
            .unwrap()
            .is_empty()
    );
    store
        .collect(&Policy::default(), started + 15 * DAY, false)
        .unwrap();
    assert_eq!(store.list().unwrap()[0].state, "quarantined");
    let quarantined = store.get(&id).unwrap().updated_at;
    store
        .collect(&Policy::default(), quarantined + 2 * DAY, false)
        .unwrap();
    assert!(
        store
            .path(&id, "log.txt")
            .unwrap()
            .parent()
            .unwrap()
            .exists()
    );
    store
        .collect(&Policy::default(), quarantined + 4 * DAY, false)
        .unwrap();
    assert_eq!(store.get(&id).unwrap().state, "reclaimed");
}

#[test]
#[cfg(unix)]
fn planted_symlink_blocks_collection_without_touching_target() {
    let fixture = Fixture::new();
    let store = Store::open(fixture.0.join("managed")).unwrap();
    let outside = fixture.0.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("input"), "original").unwrap();
    let mut run = store.allocate("s", "p", "run", "test").unwrap();
    std::os::unix::fs::symlink(&outside, run.path().join("escape")).unwrap();
    run.finish(true).unwrap();
    assert!(store.quarantine(run.id()).is_err());
    assert_eq!(
        fs::read_to_string(outside.join("input")).unwrap(),
        "original"
    );
}
