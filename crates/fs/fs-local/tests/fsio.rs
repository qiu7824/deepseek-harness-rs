//! Rust port of the core subset of the TS `fsio.spec.ts` +
//! `filesystem.spec.ts` suites: resolution identity, probing, listing,
//! text/byte reads, literal edits with line-ending restoration, the
//! atomic-write publication choreography, and the provider's guarded
//! write/edit semantics.
//!
//! Deviations:
//!
//! - Windows DACL copy/secure-replacement cases ride the simplified
//!   boundaries (recorded in the port notes); the choreography cases use
//!   the injected link/rename seams.
//! - Mid-I/O abort races (TS `AbortError` translation) are covered by the
//!   pre-abort predicate checks — the Rust reads are not cancellation-
//!   driven syscalls.

use std::path::Path;
use std::sync::Arc;

use cordis::Context;
use futures::StreamExt;

use dsh_fs::{FsErrorCode, FileSystem, ResolveOptions, fs_target_key};
use dsh_fs_local::{
    Config, FsIoInternals, LocalFileSystem, LocalTarget, PathKind, apply_literal_edit,
    list_directory, normalize_line_endings, probe, read_for_edit, read_text_for_diff,
    read_whole_bytes, read_whole_text, resolve_local_target, restore_line_endings,
    stream_whole_text, write_file_atomic,
};

fn never() -> dsh_fs_local::FsAbort {
    Arc::new(|| false)
}

fn aborted() -> dsh_fs_local::FsAbort {
    Arc::new(|| true)
}

/// A per-test temp root that cleans itself up.
struct TempRoot(std::path::PathBuf);

impl TempRoot {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "dsh-fs-local-rs-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&base).expect("temp root");
        Self(base)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }

    fn write(&self, name: &str, content: &[u8]) -> String {
        let path = self.path(name);
        if let Some(parent) = Path::new(&path).parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(&path, content).expect("write");
        path
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------
// resolveLocalTarget

#[tokio::test(flavor = "current_thread")]
async fn resolves_a_relative_path_from_cwd_and_realpaths_it() {
    let temp = TempRoot::new();
    let file = temp.write("a.txt", b"hi");
    let local = resolve_local_target(temp.0.to_str().expect("cwd"), "a.txt")
        .await
        .expect("resolve");
    assert_eq!(local.display_path, std::path::absolute(&file).expect("absolute").to_string_lossy());
    assert_eq!(local.target_key.as_str(), tokio::fs::canonicalize(&file).await.expect("canonical").to_string_lossy());
}

#[tokio::test(flavor = "current_thread")]
async fn uses_the_realpathed_parent_plus_basename_when_the_file_does_not_exist() {
    let temp = TempRoot::new();
    let missing = temp.path("missing.txt");
    let before = resolve_local_target(temp.0.to_str().expect("cwd"), "missing.txt")
        .await
        .expect("resolve");
    std::fs::write(&missing, "later").expect("write");
    let after = resolve_local_target(temp.0.to_str().expect("cwd"), "missing.txt")
        .await
        .expect("resolve");
    assert_eq!(before.target_key, after.target_key);
}

#[tokio::test(flavor = "current_thread")]
async fn two_paths_to_the_same_file_via_a_symlink_share_one_target_key() {
    let temp = TempRoot::new();
    let file = temp.write("real.txt", b"hi");
    let link = temp.path("link.txt");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&file, &link).expect("symlink");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&file, &link).expect("symlink");
    let direct = resolve_local_target(temp.0.to_str().expect("cwd"), "real.txt")
        .await
        .expect("resolve");
    let via_link = resolve_local_target(temp.0.to_str().expect("cwd"), "link.txt")
        .await
        .expect("resolve");
    assert_eq!(direct.target_key, via_link.target_key);
}

#[tokio::test(flavor = "current_thread")]
async fn realpaths_the_nearest_existing_ancestor_when_intermediate_dirs_are_missing() {
    let temp = TempRoot::new();
    std::fs::create_dir_all(temp.0.join("a")).expect("existing ancestor");
    let local = resolve_local_target(temp.0.to_str().expect("cwd"), "a/b/c.txt")
        .await
        .expect("resolve");
    let expected = std::fs::canonicalize(temp.0.join("a"))
        .expect("canonical")
        .join("b")
        .join("c.txt");
    assert_eq!(local.target_key.as_str(), expected.to_string_lossy());
    assert_eq!(
        local.display_path,
        std::path::absolute(temp.0.join("a").join("b").join("c.txt"))
            .expect("absolute")
            .to_string_lossy()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_a_blank_path() {
    let temp = TempRoot::new();
    let error = resolve_local_target(temp.0.to_str().expect("cwd"), "   ")
        .await
        .err()
        .expect("rejects");
    assert_eq!(error.code, FsErrorCode::FsNotFound);
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_a_path_whose_ancestor_is_a_file_with_a_structured_error() {
    let temp = TempRoot::new();
    temp.write("afile", b"regular");
    let error = resolve_local_target(temp.0.to_str().expect("cwd"), "afile/child.txt")
        .await
        .err()
        .expect("rejects");
    assert_eq!(error.code, FsErrorCode::FsNotFound);
    assert!(error.to_string().contains("not a directory"), "{error}");
}

// ---------------------------------------------------------------------------
// probe / list

#[tokio::test(flavor = "current_thread")]
async fn probe_returns_none_for_a_missing_path_and_metadata_for_a_file() {
    let temp = TempRoot::new();
    assert_eq!(probe(&temp.path("missing")).await.expect("probe"), None);
    let file = temp.write("a.txt", b"hello");
    let info = probe(&file).await.expect("probe").expect("present");
    assert_eq!(info.kind, PathKind::File);
    assert_eq!(info.size, 5);
}

#[tokio::test(flavor = "current_thread")]
async fn list_directory_lists_direct_children_in_stable_order_without_reading_content() {
    let temp = TempRoot::new();
    let dir = temp.0.join("dir");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("b.txt"), "b").expect("b");
    std::fs::write(dir.join("a.txt"), "a").expect("a");
    std::fs::create_dir_all(dir.join("sub")).expect("sub");
    let identity = resolve_local_target(temp.0.to_str().expect("cwd"), "dir")
        .await
        .expect("resolve");
    let entries = list_directory(&identity, Some(&never())).await.expect("list");
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, vec!["a.txt", "b.txt", "sub"]);
    assert_eq!(entries[0].size, Some(1));
    assert_eq!(entries[2].kind, PathKind::Directory);
    // Child targets resolve against the parent identity.
    assert!(entries[0].target.target_key.as_str().ends_with("a.txt"));
}

// ---------------------------------------------------------------------------
// reads

#[tokio::test(flavor = "current_thread")]
async fn read_whole_text_reads_rejects_missing_directory_binary_and_invalid_utf8() {
    let temp = TempRoot::new();
    let file = temp.write("a.txt", "héllo".as_bytes());
    let local = LocalTarget {
        display_path: file.clone(),
        target_key: fs_target_key(file.clone()),
    };
    assert_eq!(read_whole_text(&local, Some(&never())).await.expect("read"), "héllo");

    let missing = LocalTarget {
        display_path: temp.path("gone"),
        target_key: fs_target_key(temp.path("gone")),
    };
    assert_eq!(
        read_whole_text(&missing, Some(&never())).await.err().expect("missing").code,
        FsErrorCode::FsNotFound
    );

    let dir = temp.0.join("dir");
    std::fs::create_dir_all(&dir).expect("dir");
    let dir_target = LocalTarget {
        display_path: dir.to_string_lossy().into_owned(),
        target_key: fs_target_key(dir.to_string_lossy().into_owned()),
    };
    assert_eq!(
        read_whole_text(&dir_target, Some(&never())).await.err().expect("directory").code,
        FsErrorCode::FsNotRegularFile
    );

    let binary = temp.write("b.bin", &[0, 1, 2]);
    let binary_target = LocalTarget {
        display_path: binary.clone(),
        target_key: fs_target_key(binary),
    };
    assert_eq!(
        read_whole_text(&binary_target, Some(&never())).await.err().expect("binary").code,
        FsErrorCode::FsNotText
    );

    let invalid = temp.write("bad.txt", &[0xff, 0xfe]);
    let invalid_target = LocalTarget {
        display_path: invalid.clone(),
        target_key: fs_target_key(invalid),
    };
    assert_eq!(
        read_whole_text(&invalid_target, Some(&never())).await.err().expect("utf8").code,
        FsErrorCode::FsNotText
    );

    // Pre-aborted signal rejects with FS_ABORTED.
    assert_eq!(
        read_whole_text(&local, Some(&aborted())).await.err().expect("aborted").code,
        FsErrorCode::FsAborted
    );
}

#[tokio::test(flavor = "current_thread")]
async fn read_whole_bytes_bounds_content_and_skips_decoding() {
    let temp = TempRoot::new();
    let raw = vec![0u8, 255u8, 1u8];
    let file = temp.write("raw.bin", &raw);
    let local = LocalTarget {
        display_path: file.clone(),
        target_key: fs_target_key(file.clone()),
    };
    let internals = FsIoInternals::default();
    assert_eq!(
        read_whole_bytes(&local, Some(&never()), 3, &internals).await.expect("read"),
        raw
    );
    assert_eq!(
        read_whole_bytes(&local, Some(&never()), 2, &internals).await.err().expect("cap").code,
        FsErrorCode::FsTooLarge
    );
}

#[tokio::test(flavor = "current_thread")]
async fn stream_whole_text_streams_the_same_decoded_text() {
    let temp = TempRoot::new();
    // Multi-chunk content with a UTF-8 boundary split.
    let content = "line one\r\nline two\n".repeat(3000) + "héllo";
    let file = temp.write("big.txt", content.as_bytes());
    let local = LocalTarget {
        display_path: file.clone(),
        target_key: fs_target_key(file),
    };
    let stream = stream_whole_text(&local, Some(&never())).await.expect("stream");
    futures::pin_mut!(stream);
    let mut collected = String::new();
    while let Some(chunk) = stream.next().await {
        collected.push_str(&chunk.expect("chunk"));
    }
    assert_eq!(collected, content);
}

// ---------------------------------------------------------------------------
// literal edits and line endings

#[test]
fn apply_literal_edit_replaces_matches_and_rejects_bad_shapes() {
    let ok = apply_literal_edit("alpha beta", "alpha", "gamma", false, "f")
        .expect("unique");
    assert_eq!(ok.0, "gamma beta");

    assert_eq!(
        apply_literal_edit("alpha beta", "missing", "x", false, "f").err().expect("zero").code,
        FsErrorCode::FsEditNotFound
    );
    assert_eq!(
        apply_literal_edit("alpha beta", "", "x", false, "f").err().expect("empty").code,
        FsErrorCode::FsEditNotFound
    );
    assert_eq!(
        apply_literal_edit("alpha alpha", "alpha", "x", false, "f").err().expect("ambiguous").code,
        FsErrorCode::FsAmbiguousEdit
    );
    let all = apply_literal_edit("alpha alpha", "alpha", "x", true, "f").expect("replace all");
    assert_eq!(all.0, "x x");
    // Matches across normalized line endings (the edit input arrives
    // LF-normalized from readForEdit).
    let crlf = apply_literal_edit(&normalize_line_endings("a\r\nb"), "a\nb", "c", false, "f")
        .expect("crlf");
    assert_eq!(crlf.0, "c");
}

#[tokio::test(flavor = "current_thread")]
async fn read_for_edit_round_trips_crlf_matching_on_lf_and_writing_back_crlf() {
    let temp = TempRoot::new();
    let file = temp.write("crlf.txt", b"line one\r\nline two\r\n");
    let (content, endings) = read_for_edit(&file, &file, Some(&never())).await.expect("read");
    assert_eq!(content, "line one\nline two\n");
    let (edited, _) = apply_literal_edit(&content, "line one\n", "replaced\n", false, &file)
        .expect("edit");
    let restored = restore_line_endings(&edited, endings);
    assert_eq!(restored, "replaced\r\nline two\r\n");
    assert_eq!(normalize_line_endings("a\r\nb"), "a\nb");
}

#[tokio::test(flavor = "current_thread")]
async fn read_text_for_diff_bounds_the_opened_file_and_returns_null_for_undiffable() {
    let temp = TempRoot::new();
    let small = temp.write("small.txt", b"line one\r\nline two");
    assert_eq!(
        read_text_for_diff(&small, 1024, Some(&never())).await.expect("basis"),
        Some("line one\nline two".to_string())
    );
    // At/above the limit → None.
    assert_eq!(read_text_for_diff(&small, 2, Some(&never())).await.expect("limit"), None);
    // Binary → None.
    let binary = temp.write("bin.bin", &[0u8, 1]);
    assert_eq!(read_text_for_diff(&binary, 1024, Some(&never())).await.expect("binary"), None);
    // Missing → None (the caller's preflight owns the structured error).
    assert_eq!(
        read_text_for_diff(&temp.path("gone"), 1024, Some(&never())).await.expect("missing"),
        None
    );
    // Pre-aborted → FS_ABORTED (cancellation still propagates).
    assert_eq!(
        read_text_for_diff(&small, 1024, Some(&aborted())).await.err().expect("aborted").code,
        FsErrorCode::FsAborted
    );
}

// ---------------------------------------------------------------------------
// atomic writes

#[tokio::test(flavor = "current_thread")]
async fn write_file_atomic_stages_privately_and_publishes() {
    let temp = TempRoot::new();
    let target = temp.path("out.txt");
    write_file_atomic(&target, "fresh", None, Some(&never()), &FsIoInternals::default(), None)
        .await
        .expect("write");
    assert_eq!(std::fs::read_to_string(&target).expect("read"), "fresh");
    // No staging litter.
    let parent = Path::new(&target).parent().expect("parent");
    let leftovers: Vec<_> = std::fs::read_dir(parent)
        .expect("dir")
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().contains(".tmpdir"))
        .collect();
    assert!(leftovers.is_empty());
    // A second write replaces.
    write_file_atomic(&target, "second", None, Some(&never()), &FsIoInternals::default(), None)
        .await
        .expect("write");
    assert_eq!(std::fs::read_to_string(&target).expect("read"), "second");
}

#[tokio::test(flavor = "current_thread")]
async fn write_file_atomic_creates_parent_directories_and_honors_pre_abort() {
    let temp = TempRoot::new();
    let target = temp.path("nested/deep/out.txt");
    write_file_atomic(&target, "x", None, Some(&never()), &FsIoInternals::default(), None)
        .await
        .expect("write");
    assert_eq!(std::fs::read_to_string(&target).expect("read"), "x");
    let other = temp.path("aborted.txt");
    let error = write_file_atomic(&other, "y", None, Some(&aborted()), &FsIoInternals::default(), None)
        .await
        .err()
        .expect("aborted");
    assert_eq!(error.code, FsErrorCode::FsAborted);
    assert!(!Path::new(&other).exists());
}

#[tokio::test(flavor = "current_thread")]
async fn guarded_create_preserves_a_competitor_through_hard_link() {
    let temp = TempRoot::new();
    let target = temp.path("guarded.txt");
    let guard = LocalTarget {
        display_path: target.clone(),
        target_key: fs_target_key(target.clone()),
    };
    write_file_atomic(&target, "mine", None, Some(&never()), &FsIoInternals::default(), Some(&guard))
        .await
        .expect("create");
    assert_eq!(std::fs::read_to_string(&target).expect("read"), "mine");
    // A competitor appears; the guarded create must reject without clobbering.
    std::fs::write(&target, "competitor").expect("competitor");
    let error = write_file_atomic(&target, "loser", None, Some(&never()), &FsIoInternals::default(), Some(&guard))
        .await
        .err()
        .expect("rejects");
    assert_eq!(error.code, FsErrorCode::FsNotObserved);
    assert_eq!(std::fs::read_to_string(&target).expect("read"), "competitor");
}

// ---------------------------------------------------------------------------
// the provider

fn boot(temp: &TempRoot) -> Arc<LocalFileSystem> {
    let ctx = Context::root();
    LocalFileSystem::install(
        &ctx,
        Config {
            cwd: Some(temp.0.to_string_lossy().into_owned()),
            diff_basis_max_bytes: None,
        },
    )
    .expect("install")
}

#[tokio::test(flavor = "current_thread")]
async fn provider_resolves_reads_writes_and_edits() {
    let temp = TempRoot::new();
    let backend = boot(&temp);
    let target = backend.resolve("a.txt", None).await.expect("resolve");
    let created = backend
        .write_text(&target, "hello world", None, None, None)
        .await
        .expect("write");
    assert_eq!(created.before, None);
    assert_eq!(created.after, "hello world");
    assert_eq!(backend.read_text(&target, None).await.expect("read"), "hello world");

    // Guarded replace at the produced version.
    let replaced = backend
        .write_text(&target, "replaced", Some(&dsh_fs::FsWriteIntent::ReplaceIfVersion { version: created.version.clone() }), None, None)
        .await
        .expect("replace");
    assert_eq!(replaced.before.as_deref(), Some("hello world"));
    assert_eq!(replaced.after, "replaced");

    // Stale guard rejects.
    let stale = backend
        .write_text(&target, "stale", Some(&dsh_fs::FsWriteIntent::ReplaceIfVersion { version: created.version.clone() }), None, None)
        .await
        .err()
        .expect("stale");
    assert_eq!(stale.code, FsErrorCode::FsStaleVersion);

    // createIfAbsent onto an existing file rejects.
    let overwrite = backend
        .write_text(&target, "blind", Some(&dsh_fs::FsWriteIntent::CreateIfAbsent), None, None)
        .await
        .err()
        .expect("blind overwrite rejects");
    assert_eq!(overwrite.code, FsErrorCode::FsNotObserved);

    // Literal edit at the fresh version.
    let current = backend.stat(&target, None).await.expect("stat").expect("present");
    let edited = backend
        .edit_text(
            &target,
            &dsh_fs::FsEditRequest { old_string: "replaced".to_string(), new_string: "edited".to_string(), replace_all: false },
            Some(&dsh_fs::FsEditGuard { version: current.version.clone() }),
            None,
            None,
        )
        .await
        .expect("edit");
    assert_eq!(edited.before, "replaced");
    assert_eq!(edited.after, "edited");
    assert_eq!(backend.read_text(&target, None).await.expect("read"), "edited");
}

#[tokio::test(flavor = "current_thread")]
async fn provider_rejects_directories_and_honors_opts_cwd() {
    let temp = TempRoot::new();
    std::fs::create_dir_all(temp.0.join("sub")).expect("sub");
    temp.write("sub/inner.txt", b"inner");
    let backend = boot(&temp);
    // opts.cwd wins over config.cwd for relative paths.
    let opts = ResolveOptions { cwd: Some(temp.path("sub")), signal: None };
    let target = backend.resolve("inner.txt", Some(&opts)).await.expect("resolve");
    assert_eq!(backend.read_text(&target, None).await.expect("read"), "inner");

    // A directory target rejects writes.
    let dir_target = backend.resolve("sub", None).await.expect("resolve");
    let error = backend.write_text(&dir_target, "x", None, None, None).await.err().expect("dir rejects");
    assert_eq!(error.code, FsErrorCode::FsNotRegularFile);
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_guarded_writes_serialize_one_wins_the_other_is_stale() {
    let temp = TempRoot::new();
    let backend = boot(&temp);
    let target = backend.resolve("race.txt", None).await.expect("resolve");
    let created = backend
        .write_text(&target, "v1", Some(&dsh_fs::FsWriteIntent::CreateIfAbsent), None, None)
        .await
        .expect("create");
    let version = created.version.clone();
    let guard = dsh_fs::FsWriteIntent::ReplaceIfVersion { version };
    let (left, right) = tokio::join!(
        backend.write_text(&target, "left", Some(&guard), None, None),
        backend.write_text(&target, "right", Some(&guard), None, None),
    );
    let outcomes = [left, right];
    let wins = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
    let stale = outcomes.iter().filter(|outcome| {
        outcome.as_ref().err().is_some_and(|error| error.code == FsErrorCode::FsStaleVersion)
    }).count();
    assert_eq!(wins, 1);
    assert_eq!(stale, 1);
}
