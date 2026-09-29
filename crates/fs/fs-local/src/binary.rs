//! Bounded binary writes with explicit publication and policy guards.
use crate::{
    LocalFileSystem,
    fsio::{LocalTarget, PathKind, probe, write_bytes_atomic},
};
use dsh_fs::{
    AbortPredicate, FsBinaryWriteOutcome, FsError, FsErrorCode, FsTarget, FsWriteIntent,
    FsWriteOperation,
};
use dsh_sandbox::{SandboxExecutionPolicy, SandboxMode};

fn path_key(value: &str) -> String {
    let key = value.replace('\\', "/").trim_end_matches('/').to_owned();
    if cfg!(windows) {
        key.trim_start_matches("//?/").to_lowercase()
    } else {
        key
    }
}

impl LocalFileSystem {
    pub(crate) async fn write_binary(
        &self,
        target: &FsTarget,
        content: &[u8],
        expected: Option<&FsWriteIntent>,
        signal: Option<AbortPredicate>,
        policy: Option<&SandboxExecutionPolicy>,
    ) -> Result<FsBinaryWriteOutcome, FsError> {
        if content.len() > 32 * 1024 * 1024 {
            return Err(FsError::new(
                "Binary output exceeds 32 MiB",
                FsErrorCode::FsTooLarge,
            ));
        }
        if let Some(policy) = policy {
            if policy.mode == SandboxMode::ReadOnly {
                return Err(FsError::new(
                    "Binary writing requires an approved workspace write",
                    FsErrorCode::FsSandboxDenied,
                ));
            }
            if policy.mode == SandboxMode::WorkspaceWrite {
                let key = path_key(target.target_key.as_str());
                let allowed = dsh_sandbox::writable_roots(policy).iter().any(|root| {
                    let root = path_key(root);
                    key == root || key.starts_with(&format!("{root}/"))
                });
                if !allowed {
                    return Err(FsError::new(
                        "Binary output is outside the approved writable roots",
                        FsErrorCode::FsSandboxDenied,
                    ));
                }
            }
        }
        self.with_lock(target.target_key.as_str(), async {
            let existing = probe(target.target_key.as_str()).await?;
            if existing
                .as_ref()
                .is_some_and(|info| info.kind != PathKind::File)
            {
                return Err(FsError::new(
                    "Binary output must be a regular file",
                    FsErrorCode::FsNotRegularFile,
                ));
            }
            let version = match expected {
                Some(FsWriteIntent::CreateIfAbsent) if existing.is_some() => {
                    return Err(FsError::new(
                        "Output already exists; approve replacement of its current version",
                        FsErrorCode::FsNotObserved,
                    ));
                }
                Some(FsWriteIntent::ReplaceIfVersion { version }) => {
                    if existing.as_ref().map(|info| &info.version) != Some(version) {
                        return Err(FsError::new(
                            "Output changed since approval",
                            FsErrorCode::FsStaleVersion,
                        ));
                    }
                    Some(version)
                }
                _ => None,
            };
            let create =
                matches!(expected, Some(FsWriteIntent::CreateIfAbsent)).then(|| LocalTarget {
                    display_path: target.display_path.clone(),
                    target_key: target.target_key.clone(),
                });
            write_bytes_atomic(
                target.target_key.as_str(),
                content,
                existing.as_ref().map(|info| info.mode),
                signal.as_ref(),
                &self.internals,
                create.as_ref(),
                version,
            )
            .await?;
            let after = probe(target.target_key.as_str()).await?;
            Ok(FsBinaryWriteOutcome {
                operation: if existing.is_some() {
                    FsWriteOperation::Update
                } else {
                    FsWriteOperation::Create
                },
                version: self.version_after_write(after.map(|info| info.version), target),
                bytes: content.len() as u64,
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_fs::FileSystem;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[tokio::test]
    async fn dropping_a_staged_write_cleans_its_temporary_directory() {
        let root = std::env::temp_dir().join(format!("binary-drop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut fs = LocalFileSystem::build(crate::Config {
            cwd: Some(root.to_string_lossy().into()),
            diff_basis_max_bytes: None,
        })
        .unwrap();
        Arc::get_mut(&mut fs).unwrap().internals.inspect_temp =
            Some(Arc::new(|_| Box::pin(futures::future::pending())));
        let target = fs.resolve("report.docx", None).await.unwrap();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(30),
                fs.write_bytes(
                    &target,
                    b"PK fixture",
                    Some(&FsWriteIntent::CreateIfAbsent),
                    None,
                    None
                )
            )
            .await
            .is_err()
        );
        assert!(!root.join("report.docx").exists());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn bytes_preserve_binary_data_and_cannot_overwrite_without_current_version() {
        let root =
            std::env::temp_dir().join(format!("binary-publication-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let fs = LocalFileSystem::build(crate::Config {
            cwd: Some(root.to_string_lossy().into()),
            diff_basis_max_bytes: None,
        })
        .unwrap();
        let target = fs.resolve("output.docx", None).await.unwrap();
        let bytes = b"PK\0\xff\x80actual bytes";
        let written = fs
            .write_bytes(
                &target,
                bytes,
                Some(&FsWriteIntent::CreateIfAbsent),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(fs.read_bytes(&target, None, 1024).await.unwrap(), bytes);
        assert_eq!(written.bytes, bytes.len() as u64);
        assert_eq!(
            fs.write_bytes(
                &target,
                b"no",
                Some(&FsWriteIntent::CreateIfAbsent),
                None,
                None
            )
            .await
            .unwrap_err()
            .code,
            FsErrorCode::FsNotObserved
        );
        std::fs::write(root.join("output.docx"), b"external edit longer").unwrap();
        assert_eq!(
            fs.write_bytes(
                &target,
                b"no",
                Some(&FsWriteIntent::ReplaceIfVersion {
                    version: written.version
                }),
                None,
                None
            )
            .await
            .unwrap_err()
            .code,
            FsErrorCode::FsStaleVersion
        );
        assert_eq!(
            std::fs::read(root.join("output.docx")).unwrap(),
            b"external edit longer"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn cancellation_and_a_late_change_leave_the_original_file_untouched() {
        let root = std::env::temp_dir().join(format!("binary-cancel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("output.xlsx");
        std::fs::write(&path, b"original").unwrap();
        let mut fs = LocalFileSystem::build(crate::Config {
            cwd: Some(root.to_string_lossy().into()),
            diff_basis_max_bytes: None,
        })
        .unwrap();
        let changed = path.clone();
        Arc::get_mut(&mut fs).unwrap().internals.inspect_temp = Some(Arc::new(move |_| {
            let changed = changed.clone();
            Box::pin(async move {
                std::fs::write(changed, b"later external edit").map_err(|e| e.to_string())
            })
        }));
        let target = fs.resolve("output.xlsx", None).await.unwrap();
        let version = fs.stat(&target, None).await.unwrap().unwrap().version;
        assert_eq!(
            fs.write_bytes(
                &target,
                b"replacement",
                Some(&FsWriteIntent::ReplaceIfVersion { version }),
                None,
                None
            )
            .await
            .unwrap_err()
            .code,
            FsErrorCode::FsStaleVersion
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"later external edit");
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        Arc::get_mut(&mut fs).unwrap().internals.inspect_temp = Some(Arc::new(move |_| {
            flag.store(true, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }));
        let version = fs.stat(&target, None).await.unwrap().unwrap().version;
        let signal: AbortPredicate = Arc::new(move || cancelled.load(Ordering::SeqCst));
        assert_eq!(
            fs.write_bytes(
                &target,
                b"replacement",
                Some(&FsWriteIntent::ReplaceIfVersion { version }),
                Some(signal),
                None
            )
            .await
            .unwrap_err()
            .code,
            FsErrorCode::FsAborted
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"later external edit");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
