use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("artifact-restore-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.0
                .canonicalize()
                .unwrap()
                .starts_with(std::env::temp_dir().canonicalize().unwrap())
        );
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn restore_copy_crash_child() {
    let Some(root) = std::env::var_os("DSH_ARTIFACT_RESTORE_CRASH_FIXTURE") else {
        return;
    };
    let root = PathBuf::from(root);
    let source = root.join("payload");
    let calls = AtomicUsize::new(0);
    restore_new(
        &source,
        &root.join("recovered.docx"),
        &file_digest(&source).unwrap(),
        &|| {
            // Exit with a partially written private staging file; Drop does not run.
            if calls.fetch_add(1, Ordering::SeqCst) == 2 {
                std::process::exit(77);
            }
            false
        },
        &|| Ok(()),
    )
    .unwrap();
    panic!("crash checkpoint was not reached");
}

#[test]
fn process_interruption_never_publishes_partial_recovery_and_retry_succeeds() {
    let fixture = Fixture::new();
    let source = fixture.0.join("payload");
    let target = fixture.0.join("recovered.docx");
    let bytes = vec![0x5au8; 192 * 1024];
    fs::write(&source, &bytes).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            "artifacts::restore_tests::restore_copy_crash_child",
            "--nocapture",
        ])
        .env("DSH_ARTIFACT_RESTORE_CRASH_FIXTURE", &fixture.0);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        child.creation_flags(0x08000000);
    }
    let output = child.output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(77),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !target.exists(),
        "an interrupted restore may not reserve the final filename with partial bytes"
    );
    let staged: Vec<_> = fs::read_dir(&fixture.0)
        .unwrap()
        .map(Result::unwrap)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".dsh-restore-")
        })
        .collect();
    assert_eq!(staged.len(), 1);
    assert!(
        staged[0].metadata().unwrap().len() > 0
            && staged[0].metadata().unwrap().len() < bytes.len() as u64
    );
    assert_eq!(fs::read(&source).unwrap(), bytes);
    restore_new(
        &source,
        &target,
        &file_digest(&source).unwrap(),
        &|| false,
        &|| Ok(()),
    )
    .unwrap();
    assert_eq!(fs::read(&target).unwrap(), bytes);
    assert_eq!(
        fs::read(&source).unwrap(),
        bytes,
        "recovery material remains available"
    );
}

#[test]
fn a_competing_target_is_never_overwritten_and_failed_staging_is_removed() {
    let fixture = Fixture::new();
    let source = fixture.0.join("payload");
    let target = fixture.0.join("recovered.xlsx");
    fs::write(&source, b"complete recovery payload").unwrap();
    let result = restore_new(
        &source,
        &target,
        &file_digest(&source).unwrap(),
        &|| false,
        &|| {
            fs::write(&target, b"new file from another operation")
                .map_err(|error| error.to_string())
        },
    );
    assert!(result.is_err());
    assert_eq!(
        fs::read(&target).unwrap(),
        b"new file from another operation"
    );
    assert_eq!(fs::read(&source).unwrap(), b"complete recovery payload");
    assert_eq!(
        fs::read_dir(&fixture.0).unwrap().count(),
        2,
        "failed publication cleans its unique staging file"
    );
}

#[test]
fn changed_recovery_payload_is_not_published() {
    let fixture = Fixture::new();
    let source = fixture.0.join("payload");
    let target = fixture.0.join("recovered.docx");
    fs::write(&source, b"approved bytes").unwrap();
    let expected = file_digest(&source).unwrap();
    fs::write(&source, b"changed bytes").unwrap();
    assert!(restore_new(&source, &target, &expected, &|| false, &|| Ok(())).is_err());
    assert!(!target.exists());
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
}
