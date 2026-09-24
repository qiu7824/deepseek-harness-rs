use super::*;
use crate::{compress_zstd_frame, v4_artifact::validate_v4_artifact};
use serde_json::json;
use std::{
    cell::Cell,
    io::{BufRead, BufReader},
};

fn directory() -> PathBuf {
    let path = std::env::temp_dir().join(format!("v4-migration-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    path
}
fn records() -> Vec<Value> {
    vec![
        json!({"type":"session/end-seed","seq":0,"time":0,"data":{}}),
        json!({"type":"feedback/record","seq":1,"time":1,"data":{"text":"保留原会话"}}),
    ]
}
fn source(dir: &Path, compression: JsonlCompression, rows: &[Value]) -> (PathBuf, Vec<u8>) {
    let header = format!(
        "{}\n",
        json!({"type":"session","version":3,"id":"fixture","createdAt":0,"delegationDepth":0})
    );
    let body: String = rows.iter().map(|row| format!("{row}\n")).collect();
    let bytes = match compression {
        JsonlCompression::None => [header.as_bytes(), body.as_bytes()].concat(),
        JsonlCompression::Zstd => [
            compress_zstd_frame(header.as_bytes()).unwrap(),
            compress_zstd_frame(body.as_bytes()).unwrap(),
        ]
        .concat(),
    };
    let path = dir.join(format!("session{}", crate::log_suffix(compression)));
    std::fs::write(&path, &bytes).unwrap();
    (path, bytes)
}
fn assert_no_stage(dir: &Path) {
    assert!(!std::fs::read_dir(dir).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".stage")
    }));
}

#[test]
fn adjacent_migration_preserves_originals_and_is_idempotent_for_all_encodings() {
    for input in [JsonlCompression::None, JsonlCompression::Zstd] {
        for output in [JsonlCompression::None, JsonlCompression::Zstd] {
            let dir = directory();
            let (original, bytes) = source(&dir, input, &records());
            let lease = SessionGenerationLease::acquire(&dir).unwrap();
            let result = migrate_v3_to_v4(&lease, "fixture", vec![], output, &|| false).unwrap();
            assert!(result.published);
            assert_eq!(result.source_events, Some(2));
            assert_eq!(result.validation.lifecycle.event_count, 2);
            assert_eq!(std::fs::read(&original).unwrap(), bytes);
            assert_eq!(
                select_generation(&dir, "fixture").unwrap().unwrap().version,
                4
            );
            let target_bytes = std::fs::read(&result.path).unwrap();
            let again =
                migrate_v3_to_v4(&lease, "fixture", vec![], output.opposite(), &|| false).unwrap();
            assert!(!again.published);
            assert_eq!(again.path, result.path);
            assert_eq!(again.target_sha256, result.target_sha256);
            assert_eq!(std::fs::read(&again.path).unwrap(), target_bytes);
            assert_eq!(std::fs::read(&original).unwrap(), bytes);
            assert_no_stage(&dir);
        }
    }
}

#[test]
fn malformed_or_semantically_invalid_sources_leave_no_successor() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        for corrupt_frame in [false, true] {
            let dir = directory();
            let mut rows = records();
            if !corrupt_frame {
                rows.push(json!({"type":"step/end","seq":2,"time":2,"data":{"turn":1,"step":1}}));
            }
            let (original, mut bytes) = source(&dir, compression, &rows);
            if corrupt_frame {
                bytes.pop();
                std::fs::write(&original, &bytes).unwrap();
            }
            let lease = SessionGenerationLease::acquire(&dir).unwrap();
            assert!(
                migrate_v3_to_v4(&lease, "fixture", vec![], JsonlCompression::Zstd, &|| false)
                    .is_err()
            );
            assert_eq!(std::fs::read(&original).unwrap(), bytes);
            assert_eq!(
                select_generation(&dir, "fixture").unwrap().unwrap().version,
                3
            );
            assert_no_stage(&dir);
        }
    }
}

#[test]
fn cancellation_after_stage_flush_preserves_source_and_removes_only_the_private_stage() {
    let dir = directory();
    let (original, bytes) = source(&dir, JsonlCompression::Zstd, &records());
    let unrelated = dir.join("keep.txt");
    std::fs::write(&unrelated, b"keep").unwrap();
    let lease = SessionGenerationLease::acquire(&dir).unwrap();
    let saw_written_stage = Cell::new(false);
    let cancel = || {
        let written = std::fs::read_dir(&dir).unwrap().any(|entry| {
            let entry = entry.unwrap();
            entry.file_name().to_string_lossy().ends_with(".stage")
                && entry.metadata().unwrap().len() > 0
        });
        saw_written_stage.set(saw_written_stage.get() || written);
        written
    };
    assert!(
        migrate_v3_to_v4(&lease, "fixture", vec![], JsonlCompression::Zstd, &cancel)
            .unwrap_err()
            .contains("cancelled")
    );
    assert!(saw_written_stage.get());
    assert_eq!(std::fs::read(&original).unwrap(), bytes);
    assert_eq!(std::fs::read(&unrelated).unwrap(), b"keep");
    assert_eq!(
        select_generation(&dir, "fixture").unwrap().unwrap().version,
        3
    );
    assert_no_stage(&dir);
}

#[test]
fn large_records_use_the_same_validated_frame_format_without_truncation() {
    let dir = directory();
    let mut rows = records();
    rows[1]["data"]["text"] = Value::String("中文🦀".repeat(100_000));
    let (original, bytes) = source(&dir, JsonlCompression::None, &rows);
    let lease = SessionGenerationLease::acquire(&dir).unwrap();
    let result =
        migrate_v3_to_v4(&lease, "fixture", vec![], JsonlCompression::Zstd, &|| false).unwrap();
    assert_eq!(result.validation.lifecycle.event_count, 2);
    assert!(result.validation.framing.plaintext_bytes > 900_000);
    assert_eq!(std::fs::read(&original).unwrap(), bytes);
    assert_eq!(
        validate_v4_artifact(&result.path, JsonlCompression::Zstd, "fixture", &|| false).unwrap(),
        result.validation
    );
}

#[test]
fn a_generation_appearing_during_conversion_is_never_overwritten() {
    let dir = directory();
    let (original, bytes) = source(&dir, JsonlCompression::None, &records());
    let lease = SessionGenerationLease::acquire(&dir).unwrap();
    let foreign = dir.join("session.v4.jsonl.zstd");
    let injected = Cell::new(false);
    let observe = || {
        if !injected.get()
            && std::fs::read_dir(&dir).unwrap().any(|entry| {
                let entry = entry.unwrap();
                entry.file_name().to_string_lossy().ends_with(".stage")
                    && entry.metadata().unwrap().len() > 0
            })
        {
            std::fs::write(&foreign, b"external artifact").unwrap();
            injected.set(true);
        }
        false
    };
    assert!(migrate_v3_to_v4(&lease, "fixture", vec![], JsonlCompression::Zstd, &observe).is_err());
    assert!(injected.get());
    assert_eq!(std::fs::read(&foreign).unwrap(), b"external artifact");
    assert_eq!(std::fs::read(&original).unwrap(), bytes);
    assert_no_stage(&dir);
}

#[cfg(windows)]
#[test]
fn source_guard_excludes_writes_and_replacement_but_allows_readers() {
    let dir = directory();
    let (path, bytes) = source(&dir, JsonlCompression::None, &records());
    let mut guard = StableGenerationSource::open(&path, &|| false).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(OpenOptions::new().write(true).open(&path).is_err());
    assert!(std::fs::rename(&path, dir.join("moved.jsonl")).is_err());
    guard.assert_unchanged(&|| false).unwrap();
    drop(guard);
    assert!(OpenOptions::new().write(true).open(&path).is_ok());
}

#[test]
fn cancellation_interrupts_the_middle_of_large_record_serialization() {
    let mut bytes = Vec::new();
    let calls = Cell::new(0);
    let cancelled = || {
        calls.set(calls.get() + 1);
        calls.get() > 8
    };
    let value = json!({"text":"x".repeat(2 * 1024 * 1024)});
    assert!(
        write_json_line(&mut bytes, &value, &cancelled)
            .unwrap_err()
            .contains("cancelled")
    );
    assert!(bytes.len() > 0 && bytes.len() < 256 * 1024);
}

#[test]
fn process_exit_before_publication_keeps_original_and_allows_retry() {
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let dir = directory();
    let (original, bytes) = source(&dir, JsonlCompression::Zstd, &records());
    let ready = dir.join("migration-stage-ready");
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "generations::migration::tests::private_migration_process",
            "--ignored",
            "--nocapture",
        ])
        .env("DSH_V4_MIGRATION_PAUSE_DIR", &dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = Child(command.spawn().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !ready.exists() && std::time::Instant::now() < deadline {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "migration child exited before its stage was ready"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        ready.exists(),
        "migration child did not reach the publication boundary"
    );
    assert!(
        SessionGenerationLease::acquire(&dir)
            .err()
            .unwrap()
            .contains("SESSION_IN_USE")
    );
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    assert_eq!(std::fs::read(&original).unwrap(), bytes);
    assert_eq!(
        select_generation(&dir, "fixture").unwrap().unwrap().version,
        3
    );
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "stage"))
        .collect();
    assert_eq!(leftovers.len(), 1);
    let lease = SessionGenerationLease::acquire(&dir).unwrap();
    let recovered =
        migrate_v3_to_v4(&lease, "fixture", vec![], JsonlCompression::Zstd, &|| false).unwrap();
    assert!(recovered.published);
    assert_eq!(std::fs::read(&original).unwrap(), bytes);
    assert!(
        leftovers[0].is_file(),
        "retry must not delete an unowned artifact"
    );
}

#[test]
#[ignore = "private child process entry for the interrupted-migration test"]
fn private_migration_process() {
    let Some(dir) = std::env::var_os("DSH_V4_MIGRATION_PAUSE_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let ready = dir.join("migration-stage-ready");
    let lease = SessionGenerationLease::acquire(&dir).unwrap();
    let pause = || {
        let written = std::fs::read_dir(&dir).unwrap().any(|entry| {
            let entry = entry.unwrap();
            entry.file_name().to_string_lossy().ends_with(".stage")
                && entry.metadata().unwrap().len() > 0
        });
        if written {
            std::fs::write(&ready, b"flushed").unwrap();
            std::thread::sleep(std::time::Duration::from_secs(60));
            return true;
        }
        false
    };
    let _ = migrate_v3_to_v4(&lease, "fixture", vec![], JsonlCompression::Zstd, &pause);
}

#[test]
#[ignore = "requires explicitly prepared private V3 corpus and expected V4 logical rows"]
fn private_corpus_compressed_generation_publication() {
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("DSH_V4_CORPUS_MANIFEST").expect("corpus manifest"))
            .unwrap(),
    )
    .unwrap();
    let expected: Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("DSH_V4_NATIVE_REPORT").expect("native report")).unwrap(),
    )
    .unwrap();
    let output =
        PathBuf::from(std::env::var_os("DSH_V4_GENERATION_REPORT").expect("generation report"));
    let root = output
        .parent()
        .unwrap()
        .join(format!("v4-generations-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let mut reports = Vec::new();
    for (index, fixture) in manifest["fixtures"].as_array().unwrap().iter().enumerate() {
        let dir = root.join(index.to_string());
        std::fs::create_dir(&dir).unwrap();
        let original = dir.join("session.jsonl");
        std::fs::copy(fixture["path"].as_str().unwrap(), &original).unwrap();
        let id = fixture["id"].as_str().unwrap();
        let expected = expected["fixtures"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .unwrap();
        let lease = SessionGenerationLease::acquire(&dir).unwrap();
        let result = migrate_v3_to_v4(
            &lease,
            id,
            fixture["children"].as_array().unwrap().clone(),
            JsonlCompression::Zstd,
            &|| false,
        )
        .unwrap();
        let mut expected_rows =
            BufReader::new(File::open(expected["logicalRows"].as_str().unwrap()).unwrap()).lines();
        let mut scanner = None;
        let mut file = File::open(&result.path).unwrap();
        visit_artifact(
            &mut file,
            JsonlCompression::Zstd,
            &|| false,
            |part| match part {
                ArtifactPart::Header(line) => {
                    scanner = Some(dsh_session::format_v4::V4LogScanner::new(
                        line,
                        dsh_session::format_v4::V4Recovery::Strict,
                        V4Vocabulary::default(),
                    )?);
                    Ok(())
                }
                ArtifactPart::Body(bytes) => scanner.as_mut().unwrap().write(bytes, |row| {
                    let expected: Value = serde_json::from_str(
                        &expected_rows
                            .next()
                            .ok_or("unexpected decoded row")?
                            .map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?;
                    if row != expected {
                        return Err(format!("logical mismatch at seq {}", row["seq"]));
                    }
                    Ok(())
                }),
            },
        )
        .unwrap();
        let scan = scanner.unwrap().finish().unwrap();
        assert!(expected_rows.next().is_none());
        let mut original_guard = StableGenerationSource::open(&original, &|| false).unwrap();
        original_guard.assert_unchanged(&|| false).unwrap();
        assert_eq!(Some(original_guard.sha256()), result.source_sha256);
        assert_eq!(
            scan.decoded.event_count,
            expected["targetEvents"].as_u64().unwrap()
        );
        let repeated = migrate_v3_to_v4(
            &lease,
            id,
            fixture["children"].as_array().unwrap().clone(),
            JsonlCompression::Zstd,
            &|| false,
        )
        .unwrap();
        assert!(!repeated.published);
        assert_eq!(repeated.target_sha256, result.target_sha256);
        reports.push(json!({"id":id,"path":result.path,"sourceSha256":result.source_sha256,"targetSha256":result.target_sha256,"events":scan.decoded.event_count,"frames":result.validation.framing.frames,"compressedBytes":result.validation.framing.physical_bytes,"plaintextBytes":result.validation.framing.plaintext_bytes,"logicalValuesEqual":true,"sourcePreserved":true,"idempotencePassed":true}));
    }
    std::fs::write(output, serde_json::to_vec_pretty(&json!({"fixtures":reports,"privateCopyGenerationPublished":true,"installedSessionsModified":false,"runtimeActivated":false})).unwrap()).unwrap();
}
