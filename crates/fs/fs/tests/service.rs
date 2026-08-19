//! Rust port of the TS `service.spec.ts` + `invariant.spec.ts` suites for
//! the filesystem Service Definition: registration, duplicate-service
//! behavior, the branded id factories, the typed error, and the event-data
//! invariants.
//!
//! Deviations:
//!
//! - `fiber.dispose()` service removal collapses into the duplicate-
//!   registration panic (the Rust service store rejects a second
//!   registration of the same name).
//! - The `internal/dispatch` pre-hook failures are contained per listener
//!   by the Rust dispatch; the invariant tests drive the companion through
//!   `check_dispatch` and a collector fail channel instead of observing a
//!   synchronous throw.

use std::sync::Arc;

use cordis::Context;
use futures::StreamExt;
use futures::stream::BoxStream;

use dsh_fs::{
    FileSystem, FsDirEntry, FsEditOutcome, FsEditRequest, FsError, FsErrorCode, FsInfo, FsInfoType,
    FsPathInfo, FsPathInfoType, FsTarget, FsWriteIntent, FsWriteOperation, FsWriteOutcome,
    check_dispatch, fs_target_key, fs_version,
};
use dsh_sandbox::SandboxExecutionPolicy;

/// A minimal in-memory fake implementing the provider primitives.
struct FakeFileSystem {
    files: parking_lot::Mutex<std::collections::HashMap<String, String>>,
}

impl FakeFileSystem {
    fn install(ctx: &Context) -> Arc<Self> {
        let fake = Arc::new(Self {
            files: parking_lot::Mutex::new(Default::default()),
        });
        let erased: Arc<dyn FileSystem> = fake.clone();
        ctx.register_service(erased);
        fake
    }

    fn get(&self, key: &str) -> Option<String> {
        self.files.lock().get(key).cloned()
    }

    fn set(&self, key: &str, content: &str) {
        self.files
            .lock()
            .insert(key.to_string(), content.to_string());
    }
}

fn mk_target(key: &str, display_path: &str) -> FsTarget {
    FsTarget {
        target_key: fs_target_key(key),
        display_path: display_path.to_string(),
    }
}

#[async_trait::async_trait]
impl FileSystem for FakeFileSystem {
    async fn resolve(
        &self,
        path: &str,
        _opts: Option<&dsh_fs::ResolveOptions>,
    ) -> Result<FsTarget, FsError> {
        Ok(mk_target(path, path))
    }

    fn process_path(&self, target: &FsTarget) -> String {
        target.target_key.to_string()
    }

    fn file_url(&self, target: &FsTarget) -> String {
        format!("file:///{}", target.target_key)
    }

    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool {
        child.target_key == parent.target_key
            || child
                .target_key
                .as_str()
                .starts_with(&format!("{}/", parent.target_key))
    }

    async fn stat(
        &self,
        target: &FsTarget,
        _signal: Option<dsh_fs::AbortPredicate>,
    ) -> Result<Option<FsInfo>, FsError> {
        let Some(content) = self.get(target.target_key.as_str()) else {
            return Ok(None);
        };
        Ok(Some(FsInfo {
            version: fs_version("v1"),
            kind: FsInfoType::File,
            size: Some(content.len() as u64),
        }))
    }

    async fn lstat(
        &self,
        path: &str,
        _opts: Option<&dsh_fs::LstatOptions>,
        _signal: Option<dsh_fs::AbortPredicate>,
    ) -> Result<Option<FsPathInfo>, FsError> {
        let Some(content) = self.get(path) else {
            return Ok(None);
        };
        Ok(Some(FsPathInfo {
            version: fs_version("v1"),
            kind: FsPathInfoType::File,
            size: Some(content.len() as u64),
        }))
    }

    async fn read_text(
        &self,
        target: &FsTarget,
        _signal: Option<dsh_fs::AbortPredicate>,
    ) -> Result<String, FsError> {
        let Some(content) = self.get(target.target_key.as_str()) else {
            return Err(FsError::new(
                format!("not found: {}", target.display_path),
                FsErrorCode::FsNotFound,
            ));
        };
        Ok(content)
    }

    async fn stream_text(
        &self,
        target: &FsTarget,
        _signal: Option<dsh_fs::AbortPredicate>,
    ) -> Result<BoxStream<'static, Result<String, FsError>>, FsError> {
        let content = self.read_text(target, None).await?;
        Ok(futures::stream::once(async move { Ok(content) }).boxed())
    }

    async fn read_bytes(
        &self,
        target: &FsTarget,
        _signal: Option<dsh_fs::AbortPredicate>,
        max_bytes: u64,
    ) -> Result<Vec<u8>, FsError> {
        let content = self.read_text(target, None).await?;
        if content.len() as u64 > max_bytes {
            return Err(FsError::new(
                format!("too large: {}", target.display_path),
                FsErrorCode::FsTooLarge,
            ));
        }
        Ok(content.into_bytes())
    }

    async fn list_dir(
        &self,
        target: &FsTarget,
        _signal: Option<dsh_fs::AbortPredicate>,
    ) -> Result<Vec<FsDirEntry>, FsError> {
        if target.target_key.as_str() != "skills" {
            return Err(FsError::new(
                format!("not a directory: {}", target.display_path),
                FsErrorCode::FsNotDirectory,
            ));
        }
        Ok(vec![FsDirEntry {
            name: "alpha.md".to_string(),
            kind: FsInfoType::File,
            target: mk_target("skills/alpha.md", "skills/alpha.md"),
            size: Some(2),
            version: Some(fs_version("v1")),
        }])
    }

    async fn write_text(
        &self,
        target: &FsTarget,
        content: &str,
        _expected: Option<&FsWriteIntent>,
        _signal: Option<dsh_fs::AbortPredicate>,
        _sandbox_policy: Option<&SandboxExecutionPolicy>,
    ) -> Result<FsWriteOutcome, FsError> {
        let before = self.get(target.target_key.as_str());
        self.set(target.target_key.as_str(), content);
        Ok(FsWriteOutcome {
            operation: if before.is_some() {
                FsWriteOperation::Update
            } else {
                FsWriteOperation::Create
            },
            version: fs_version("v2"),
            before,
            after: content.to_string(),
        })
    }

    async fn edit_text(
        &self,
        target: &FsTarget,
        edit: &FsEditRequest,
        _expected: Option<&dsh_fs::FsEditGuard>,
        _signal: Option<dsh_fs::AbortPredicate>,
        _sandbox_policy: Option<&SandboxExecutionPolicy>,
    ) -> Result<FsEditOutcome, FsError> {
        let content = self.get(target.target_key.as_str()).unwrap_or_default();
        let after = content.replace(&edit.old_string, &edit.new_string);
        self.set(target.target_key.as_str(), &after);
        Ok(FsEditOutcome {
            version: fs_version("v3"),
            before: content,
            after,
        })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn registers_as_ctx_fs_and_serves_the_primitives() {
    let ctx = Context::root();
    let fake = FakeFileSystem::install(&ctx);
    assert_eq!(fake.sandbox_mode(), None);
    fake.set("a.txt", "hi");
    let target = fake.resolve("a.txt", None).await.expect("resolve");
    assert_eq!(
        fake.stat(&target, None)
            .await
            .expect("stat")
            .expect("present")
            .kind,
        FsInfoType::File
    );
    assert_eq!(fake.read_text(&target, None).await.expect("read"), "hi");
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_a_second_implementation_of_the_same_service_name() {
    let ctx = Context::root();
    let fake = FakeFileSystem::install(&ctx);
    // The Rust service store rejects the duplicate registration.
    let erased: Arc<dyn FileSystem> = fake.clone();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ctx.register_service(erased);
    }));
    assert!(outcome.is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn stream_text_yields_the_same_text_read_text_returns() {
    let ctx = Context::root();
    let fake = FakeFileSystem::install(&ctx);
    fake.set("a.txt", "one\ntwo");
    let target = fake.resolve("a.txt", None).await.expect("resolve");
    let stream = fake.stream_text(&target, None).await.expect("stream");
    let mut streamed = String::new();
    futures::pin_mut!(stream);
    while let Some(chunk) = stream.next().await {
        streamed.push_str(&chunk.expect("chunk"));
    }
    assert_eq!(streamed, fake.read_text(&target, None).await.expect("read"));
}

#[tokio::test(flavor = "current_thread")]
async fn read_bytes_returns_raw_content_and_enforces_the_byte_cap() {
    let ctx = Context::root();
    let fake = FakeFileSystem::install(&ctx);
    fake.set("a.bin", "hi");
    let target = fake.resolve("a.bin", None).await.expect("resolve");
    assert_eq!(
        fake.read_bytes(&target, None, 2).await.expect("read"),
        b"hi"
    );
    let error = fake
        .read_bytes(&target, None, 1)
        .await
        .err()
        .expect("cap rejects");
    assert_eq!(error.code, FsErrorCode::FsTooLarge);
}

#[tokio::test(flavor = "current_thread")]
async fn list_dir_returns_child_entry_targets_without_reading_file_content() {
    let ctx = Context::root();
    let fake = FakeFileSystem::install(&ctx);
    let entries = fake
        .list_dir(&fake.resolve("skills", None).await.expect("resolve"), None)
        .await
        .expect("list");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "alpha.md");
    assert_eq!(entries[0].kind, FsInfoType::File);
    assert_eq!(entries[0].target.target_key.as_str(), "skills/alpha.md");
    assert_eq!(entries[0].size, Some(2));
    assert_eq!(entries[0].version, Some(fs_version("v1")));
}

#[tokio::test(flavor = "current_thread")]
async fn stat_returns_none_for_an_absent_target() {
    let ctx = Context::root();
    let fake = FakeFileSystem::install(&ctx);
    assert_eq!(
        fake.stat(
            &fake.resolve("missing.txt", None).await.expect("resolve"),
            None
        )
        .await
        .expect("stat"),
        None
    );
}

#[tokio::test(flavor = "current_thread")]
async fn lstat_returns_path_metadata_before_resolving_a_target() {
    let ctx = Context::root();
    let fake = FakeFileSystem::install(&ctx);
    fake.set("a.txt", "hi");
    assert_eq!(
        fake.lstat("a.txt", None, None).await.expect("lstat"),
        Some(FsPathInfo {
            version: fs_version("v1"),
            kind: FsPathInfoType::File,
            size: Some(2)
        })
    );
    assert_eq!(
        fake.lstat("missing.txt", None, None).await.expect("lstat"),
        None
    );
}

#[test]
fn branded_id_factories_brand_a_string_at_compile_time_identity_at_runtime() {
    assert_eq!(fs_target_key("k").to_string(), "k");
    assert_eq!(fs_version("v").to_string(), "v");
}

#[test]
fn fs_error_carries_a_stable_code() {
    let error = FsError::new("nope", FsErrorCode::FsNotFound);
    assert_eq!(error.code, FsErrorCode::FsNotFound);
    assert_eq!(error.error.code, "FS_NOT_FOUND");
}

#[test]
fn fs_error_chains_an_underlying_cause() {
    let root = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "EACCES");
    let error = FsError::with_cause("cannot read", FsErrorCode::FsAborted, Box::new(root));
    assert!(error.cause().is_some());
    assert_eq!(error.code, FsErrorCode::FsAborted);
}

// ---------------------------------------------------------------------------
// event-data invariants

#[test]
fn accepts_decision_and_observation_events_with_usable_identities() {
    let failures: Arc<parking_lot::Mutex<Vec<String>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let fail = {
        let failures = failures.clone();
        move |message: &str| {
            failures.lock().push(message.to_string());
        }
    };
    let present = dsh_fs::FsObservation::Present {
        version: fs_version("v1"),
    };
    let absent = dsh_fs::FsObservation::Absent;
    let t = mk_target("file:1", "file.txt");

    check_dispatch("fs/write-intent", &[cordis::arc(t.clone())], &fail);
    check_dispatch("fs/edit-intent", &[cordis::arc(t.clone())], &fail);
    check_dispatch(
        "fs/observed",
        &[cordis::arc(t.clone()), cordis::arc(present)],
        &fail,
    );
    check_dispatch(
        "fs/observed",
        &[cordis::arc(t.clone()), cordis::arc(absent)],
        &fail,
    );
    check_dispatch("tools/change", &[], &fail);
    assert!(failures.lock().is_empty());
}

#[test]
fn rejects_empty_target_and_version_identities() {
    let failures: Arc<parking_lot::Mutex<Vec<String>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let fail = {
        let failures = failures.clone();
        move |message: &str| {
            failures.lock().push(message.to_string());
        }
    };

    check_dispatch(
        "fs/observed",
        &[
            cordis::arc(mk_target("", "file.txt")),
            cordis::arc(dsh_fs::FsObservation::Present {
                version: fs_version("v1"),
            }),
        ],
        &fail,
    );
    assert_eq!(failures.lock().len(), 1);
    assert!(
        failures.lock()[0].contains("targetKey must be non-empty"),
        "{}",
        failures.lock()[0]
    );
    failures.lock().clear();

    check_dispatch(
        "fs/observed",
        &[
            cordis::arc(mk_target("file:1", "")),
            cordis::arc(dsh_fs::FsObservation::Present {
                version: fs_version("v1"),
            }),
        ],
        &fail,
    );
    assert_eq!(failures.lock().len(), 1);
    assert!(
        failures.lock()[0].contains("displayPath must be non-empty"),
        "{}",
        failures.lock()[0]
    );
    failures.lock().clear();

    check_dispatch(
        "fs/observed",
        &[
            cordis::arc(mk_target("file:1", "file.txt")),
            cordis::arc(dsh_fs::FsObservation::Present {
                version: fs_version(""),
            }),
        ],
        &fail,
    );
    assert_eq!(failures.lock().len(), 1);
    assert!(
        failures.lock()[0].contains("present version must be non-empty"),
        "{}",
        failures.lock()[0]
    );
}
