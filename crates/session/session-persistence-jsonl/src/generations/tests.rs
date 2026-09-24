use super::*;
use serde_json::json;

struct OwnedChild(std::process::Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        // A failed assertion must not leave the helper holding a Session lock.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn directory() -> PathBuf {
    let root = std::env::temp_dir().join(format!("dsh-generation-tests-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    root
}
fn write(dir: &Path, name: &str, version: u64, id: &str) {
    let header = if version == 3 {
        json!({"type":"session","version":3,"id":id,"createdAt":0,"delegationDepth":0})
    } else {
        json!({"type":"session","version":version,"id":id,"createdAt":0,"isSeeded":false,"delegationDepth":0})
    };
    let bytes = format!("{header}\n").into_bytes();
    let bytes = if name.ends_with(".zstd") {
        crate::compress_zstd_frame(&bytes).unwrap()
    } else {
        bytes
    };
    std::fs::write(dir.join(name), bytes).unwrap();
}

#[test]
fn newest_supported_generation_is_selected_without_rewriting_the_older_log() {
    let dir = directory();
    write(&dir, "session.jsonl.zstd", 3, "s");
    let old = std::fs::read(dir.join("session.jsonl.zstd")).unwrap();
    assert_eq!(select_generation(&dir, "s").unwrap().unwrap().version, 3);
    write(&dir, "session.v4.jsonl", 4, "s");
    let selected = select_generation(&dir, "s").unwrap().unwrap();
    assert_eq!(selected.version, 4);
    assert_eq!(selected.compression, JsonlCompression::None);
    assert_eq!(std::fs::read(dir.join("session.jsonl.zstd")).unwrap(), old);
}

#[test]
fn newer_unknown_generations_are_not_hidden_by_a_supported_older_file() {
    for name in ["session.v5.jsonl", "session.jsonl"] {
        let dir = directory();
        write(&dir, "session.v4.jsonl.zstd", 4, "s");
        write(&dir, name, 5, "s");
        assert!(
            select_generation(&dir, "s")
                .unwrap_err()
                .contains("unsupported newer")
        );
    }
}

#[test]
fn conflicting_headers_encodings_and_duplicate_generation_claims_are_refused() {
    let dir = directory();
    write(&dir, "session.v4.jsonl", 3, "s");
    assert!(
        select_generation(&dir, "s")
            .unwrap_err()
            .contains("disagrees")
    );
    let dir = directory();
    write(&dir, "session.v4.jsonl", 4, "other");
    assert!(
        select_generation(&dir, "s")
            .unwrap_err()
            .contains("identity")
    );
    for duplicate in ["session.jsonl", "session.v4.jsonl.zstd"] {
        let dir = directory();
        write(&dir, "session.v4.jsonl", 4, "s");
        write(&dir, duplicate, 4, "s");
        assert!(
            select_generation(&dir, "s")
                .unwrap_err()
                .contains("ambiguous")
        );
    }
}

#[test]
fn backup_names_are_ignored_but_managed_generation_names_are_strict() {
    let dir = directory();
    std::fs::write(dir.join("session.jsonl.v0-backup-fixture"), b"backup").unwrap();
    assert!(select_generation(&dir, "s").unwrap().is_none());
    std::fs::write(dir.join("session.v04.jsonl"), b"not a valid claim").unwrap();
    assert!(select_generation(&dir, "s").is_err());
}

#[test]
fn writer_lease_excludes_a_second_owner_but_does_not_block_readers() {
    let dir = directory();
    write(&dir, "session.jsonl.zstd", 3, "s");
    let lease = SessionGenerationLease::acquire(&dir).unwrap();
    assert_eq!(lease.directory(), std::fs::canonicalize(&dir).unwrap());
    assert!(
        SessionGenerationLease::acquire(&dir)
            .err()
            .unwrap()
            .contains("SESSION_IN_USE")
    );
    assert!(select_generation(&dir, "s").unwrap().is_some());
    drop(lease);
    let _next = SessionGenerationLease::acquire(&dir).unwrap();
}

#[test]
fn cross_process_owner_exclusion_survives_owner_termination() {
    let dir = directory();
    let ready = dir.join("child-ready");
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "generations::tests::private_writer_owner",
            "--ignored",
            "--nocapture",
        ])
        .env("DSH_GENERATION_OWNER_DIR", &dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = OwnedChild(command.spawn().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !ready.exists() && std::time::Instant::now() < deadline {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "owner process exited before locking"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let announced = ready.exists();
    let denied = SessionGenerationLease::acquire(&dir)
        .err()
        .is_some_and(|e| e.contains("SESSION_IN_USE"));
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    assert!(
        announced && denied,
        "a live external owner must exclude admission"
    );
    let _next = SessionGenerationLease::acquire(&dir).unwrap();
    assert!(dir.join(".session-writer.lock").is_file());
}

#[test]
#[ignore = "private child entry for the cross-process ownership test"]
fn private_writer_owner() {
    let Some(path) = std::env::var_os("DSH_GENERATION_OWNER_DIR") else {
        return;
    };
    let path = PathBuf::from(path);
    let _lease = SessionGenerationLease::acquire(&path).unwrap();
    std::fs::write(path.join("child-ready"), b"locked").unwrap();
    std::thread::sleep(std::time::Duration::from_secs(60));
}
