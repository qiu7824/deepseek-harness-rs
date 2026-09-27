use super::*;
use std::io::{Seek, SeekFrom, Write};
use std::sync::Arc;

const NONCE: &str = "legacy-json-fixture";

fn fixture() -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("dsh-json-lock-{}", temp_suffix()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("settings.json");
    (root, path)
}

fn record(pid: u32, host: &str) -> Vec<u8> {
    format!(
        "{}\n",
        serde_json::json!({"pid": pid, "hostname": host, "nonce": NONCE})
    )
    .into_bytes()
}

struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_fixture(mode: &str, path: &Path, claim: Option<&Path>) -> ChildGuard {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "legacy_json_tests::process_fixture",
            "--nocapture",
        ])
        .env("DSH_JSON_LOCK_FIXTURE", mode)
        .env("DSH_JSON_LOCK_TARGET", path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(claim) = claim {
        command.env("DSH_JSON_LOCK_CLAIM", claim);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    ChildGuard(command.spawn().unwrap())
}

fn exited_pid(path: &Path) -> u32 {
    let mut child = spawn_fixture("exit", path, None);
    let pid = child.0.id();
    assert!(child.0.wait().unwrap().success());
    assert!(!legacy_owner_alive(pid).unwrap());
    pid
}

async fn ready(child: &mut ChildGuard, path: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !path.with_extension("ready").exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "fixture exited before acquiring ownership"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn process_fixture() {
    let Ok(mode) = std::env::var("DSH_JSON_LOCK_FIXTURE") else {
        return;
    };
    if mode == "exit" {
        return;
    }
    let path = PathBuf::from(std::env::var_os("DSH_JSON_LOCK_TARGET").unwrap());
    let sidecar = path.with_file_name("settings.json.lock");
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let ownership = match mode.as_str() {
            "writer" => {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&sidecar)
                    .unwrap();
                file.write_all(&record(std::process::id(), &local_hostname().unwrap()))
                    .unwrap();
                file.sync_data().unwrap();
                drop(file);
                None
            }
            "claim" => {
                let claim = PathBuf::from(std::env::var_os("DSH_JSON_LOCK_CLAIM").unwrap());
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(claim)
                    .unwrap();
                writeln!(file, "{}", std::process::id()).unwrap();
                file.sync_data().unwrap();
                drop(file);
                None
            }
            "upgrade" => {
                let mut file = open_lock_file(&sidecar).unwrap();
                file.try_lock().unwrap();
                let marker = read_lock_marker(&mut file).unwrap();
                let LockMarker::Legacy {
                    pid,
                    hostname,
                    nonce,
                    suffix_offset,
                } = lock_marker(&marker)
                else {
                    panic!("expected complete JSON legacy owner");
                };
                require_local_hostname(hostname.as_deref()).unwrap();
                assert!(!legacy_owner_alive(pid).unwrap());
                let claim = migration_claim(&migration_claim_path(&sidecar, pid, &nonce))
                    .unwrap()
                    .unwrap();
                file.seek(SeekFrom::Start(suffix_offset)).unwrap();
                file.write_all(&LOCK_PROTOCOL[..5]).unwrap();
                file.sync_data().unwrap();
                Some((file, claim))
            }
            _ => panic!("unknown fixture mode"),
        };
        fs::write(path.with_extension("ready"), "ready")
            .await
            .unwrap();
        std::future::pending::<()>().await;
        drop(ownership);
    });
}

#[tokio::test]
async fn live_json_writer_is_preserved_until_exit_then_migrated() {
    let (root, path) = fixture();
    let sidecar = root.join("settings.json.lock");
    let mut child = spawn_fixture("writer", &path, None);
    ready(&mut child, &path).await;
    let original = fs::read(&sidecar).await.unwrap();
    let error = with_file_lock(&path, async { panic!("entered a live legacy writer") })
        .await
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert_eq!(fs::read(&sidecar).await.unwrap(), original);
    drop(child);
    with_file_lock(&path, async { fs::write(&path, "complete settings").await })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        fs::read(&sidecar).await.unwrap(),
        [original.as_slice(), LOCK_PROTOCOL].concat()
    );
    assert_eq!(
        fs::read_to_string(&path).await.unwrap(),
        "complete settings"
    );
    with_file_lock(&path, async {}).await.unwrap();
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn json_migration_serializes_writers_without_replacing_the_inode() {
    let (root, path) = fixture();
    let sidecar = root.join("settings.json.lock");
    let pid = exited_pid(&path);
    let original = record(pid, &local_hostname().unwrap());
    fs::write(&sidecar, &original).await.unwrap();
    let witness = root.join("original-inode");
    std::fs::hard_link(&sidecar, &witness).unwrap();
    let entered = Arc::new(AtomicU64::new(0));
    let completed = Arc::new(AtomicU64::new(0));
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let path = path.clone();
        let entered = entered.clone();
        let completed = completed.clone();
        tasks.push(tokio::spawn(async move {
            with_file_lock(&path, async {
                assert_eq!(entered.fetch_add(1, Ordering::SeqCst), 0);
                tokio::time::sleep(Duration::from_millis(5)).await;
                completed.fetch_add(1, Ordering::SeqCst);
                assert_eq!(entered.fetch_sub(1, Ordering::SeqCst), 1);
            })
            .await
            .unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(completed.load(Ordering::SeqCst), 8);
    let migrated = [original.as_slice(), LOCK_PROTOCOL].concat();
    assert_eq!(fs::read(&sidecar).await.unwrap(), migrated);
    assert_eq!(fs::read(&witness).await.unwrap(), migrated);
    assert_eq!(
        fs::read(migration_claim_path(&sidecar, pid, NONCE))
            .await
            .unwrap(),
        LOCK_PROTOCOL
    );
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn live_legacy_takeover_claim_blocks_migration_until_its_owner_exits() {
    let (root, path) = fixture();
    let sidecar = root.join("settings.json.lock");
    let pid = exited_pid(&path);
    let original = record(pid, &local_hostname().unwrap());
    fs::write(&sidecar, &original).await.unwrap();
    let claim = migration_claim_path(&sidecar, pid, NONCE);
    let mut child = spawn_fixture("claim", &path, Some(&claim));
    ready(&mut child, &path).await;
    let old_claim = fs::read(&claim).await.unwrap();
    let error = with_file_lock(&path, async { panic!("entered an active legacy takeover") })
        .await
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert_eq!(fs::read(&sidecar).await.unwrap(), original);
    assert_eq!(fs::read(&claim).await.unwrap(), old_claim);
    drop(child);
    with_file_lock(&path, async {}).await.unwrap();
    assert_eq!(
        fs::read(&sidecar).await.unwrap(),
        [original.as_slice(), LOCK_PROTOCOL].concat()
    );
    assert_eq!(
        fs::read(&claim).await.unwrap(),
        [old_claim.as_slice(), LOCK_PROTOCOL].concat()
    );
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn interrupted_json_migration_keeps_the_owner_line_and_releases_os_ownership() {
    let (root, path) = fixture();
    let sidecar = root.join("settings.json.lock");
    let original = record(exited_pid(&path), &local_hostname().unwrap());
    fs::write(&sidecar, &original).await.unwrap();
    let mut child = spawn_fixture("upgrade", &path, None);
    ready(&mut child, &path).await;
    let partial = [original.as_slice(), &LOCK_PROTOCOL[..5]].concat();
    let error = with_file_lock(&path, async {
        panic!("entered another migration's OS lock")
    })
    .await
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    drop(child);
    assert_eq!(fs::read(&sidecar).await.unwrap(), partial);
    with_file_lock(&path, async {}).await.unwrap();
    assert_eq!(
        fs::read(&sidecar).await.unwrap(),
        [original.as_slice(), LOCK_PROTOCOL].concat()
    );
    fs::remove_dir_all(root).await.unwrap();
}

#[test]
fn migration_rechecks_inode_and_owner_bytes_after_acquiring_the_claim() {
    let (root, path) = fixture();
    let sidecar = root.join("settings.json.lock");
    let pid = exited_pid(&path);
    let original = record(pid, &local_hostname().unwrap());
    std::fs::write(&sidecar, &original).unwrap();
    let mut old = open_lock_file(&sidecar).unwrap();
    old.try_lock().unwrap();
    let retired = root.join("retired.lock");
    std::fs::rename(&sidecar, &retired).unwrap();
    std::fs::write(&sidecar, &original).unwrap();
    assert!(
        !migrate_legacy_marker(
            &mut old,
            &sidecar,
            &original,
            pid,
            NONCE,
            original.len() as u64
        )
        .unwrap()
    );
    assert_eq!(std::fs::read(&sidecar).unwrap(), original);
    drop(old);
    assert_eq!(std::fs::read(&retired).unwrap(), original);

    let mut current = open_lock_file(&sidecar).unwrap();
    current.try_lock().unwrap();
    let replacement = record(std::process::id(), &local_hostname().unwrap());
    current.set_len(0).unwrap();
    current.write_all(&replacement).unwrap();
    current.sync_data().unwrap();
    assert!(
        !migrate_legacy_marker(
            &mut current,
            &sidecar,
            &original,
            pid,
            NONCE,
            original.len() as u64
        )
        .unwrap()
    );
    drop(current);
    assert_eq!(std::fs::read(&sidecar).unwrap(), replacement);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn foreign_and_incomplete_json_records_are_preserved_with_errors() {
    let (root, path) = fixture();
    let sidecar = root.join("settings.json.lock");
    let pid = exited_pid(&path);
    let foreign = format!("{}-foreign", local_hostname().unwrap());
    let complete = record(pid, &local_hostname().unwrap());
    for original in [
        record(pid, &foreign),
        complete[..complete.len() - 1].to_vec(),
        format!("{{\"pid\":{pid},\"hostname\":\"\",\"nonce\":\"n\"}}\n").into_bytes(),
    ] {
        fs::write(&sidecar, &original).await.unwrap();
        let error = with_file_lock(&path, async { panic!("ambiguous owner was overwritten") })
            .await
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("cannot safely recover"));
        assert_eq!(fs::read(&sidecar).await.unwrap(), original);
    }
    fs::remove_dir_all(root).await.unwrap();
}

#[test]
fn json_records_require_complete_unambiguous_owner_fields() {
    for invalid in [
        "{\"pid\":1,\"hostname\":\"host\"}\n",
        "{\"pid\":1,\"hostname\":\"host\",\"nonce\":null}\n",
        "{\"pid\":1,\"hostname\":\"host\",\"nonce\":\"\"}\n",
        "{\"pid\":0,\"hostname\":\"host\",\"nonce\":\"n\"}\n",
        "{\"pid\":1,\"pid\":2,\"hostname\":\"host\",\"nonce\":\"n\"}\n",
        "{\"pid\":1,\"hostname\":\"host\",\"nonce\":\"n\",\"version\":9}\n",
    ] {
        assert!(matches!(
            lock_marker(invalid.as_bytes()),
            LockMarker::Unknown
        ));
    }
    let valid = record(42, "host");
    assert!(matches!(
        lock_marker(&valid),
        LockMarker::Legacy { pid: 42, .. }
    ));
    for length in [1, 5, LOCK_PROTOCOL.len() - 1] {
        assert!(matches!(
            lock_marker(&[valid.as_slice(), &LOCK_PROTOCOL[..length]].concat()),
            LockMarker::Legacy { pid: 42, .. }
        ));
    }
    assert!(matches!(
        lock_marker(&[valid.as_slice(), LOCK_PROTOCOL].concat()),
        LockMarker::Native
    ));
}
