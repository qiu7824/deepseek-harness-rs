use crate::{Config, LocalFileSystem};
use dsh_fs::{FileSystem, FsEditGuard, FsEditRequest, FsErrorCode, FsWriteIntent};
use std::{path::PathBuf, sync::Arc};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("text-publication-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn filesystem(&self) -> Arc<LocalFileSystem> {
        LocalFileSystem::build(Config {
            cwd: Some(self.0.to_string_lossy().into_owned()),
            diff_basis_max_bytes: None,
        })
        .unwrap()
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
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[tokio::test]
async fn text_write_and_edit_preserve_external_changes_during_staging() {
    for edit in [false, true] {
        for guarded in [false, true] {
            let fixture = Fixture::new();
            let path = fixture.0.join("source.txt");
            std::fs::write(&path, b"original content").unwrap();
            let mut fs = fixture.filesystem();
            let changed = path.clone();
            Arc::get_mut(&mut fs).unwrap().internals.inspect_temp = Some(Arc::new(move |_| {
                let changed = changed.clone();
                Box::pin(async move {
                    std::fs::write(changed, b"late change from another editor")
                        .map_err(|e| e.to_string())
                })
            }));
            let target = fs.resolve("source.txt", None).await.unwrap();
            let version = fs.stat(&target, None).await.unwrap().unwrap().version;
            let failure = if edit {
                let guard = FsEditGuard { version };
                fs.edit_text(
                    &target,
                    &FsEditRequest {
                        old_string: "original".into(),
                        new_string: "replacement".into(),
                        replace_all: false,
                    },
                    guarded.then_some(&guard),
                    None,
                    None,
                )
                .await
                .unwrap_err()
            } else {
                let intent = FsWriteIntent::ReplaceIfVersion { version };
                fs.write_text(
                    &target,
                    "replacement content",
                    guarded.then_some(&intent),
                    None,
                    None,
                )
                .await
                .unwrap_err()
            };
            assert_eq!(
                failure.code,
                FsErrorCode::FsStaleVersion,
                "edit={edit}, guarded={guarded}"
            );
            assert_eq!(
                std::fs::read(&path).unwrap(),
                b"late change from another editor"
            );
            assert_eq!(
                std::fs::read_dir(&fixture.0).unwrap().count(),
                1,
                "failed publication leaves no staging directory"
            );
        }
    }
}

#[tokio::test]
async fn text_create_if_absent_still_refuses_a_late_competing_file() {
    let fixture = Fixture::new();
    let path = fixture.0.join("new.txt");
    let mut fs = fixture.filesystem();
    let target = fs.resolve("new.txt", None).await.unwrap();
    Arc::get_mut(&mut fs).unwrap().internals.inspect_temp = Some(Arc::new(move |_| {
        let path = path.clone();
        Box::pin(
            async move { std::fs::write(path, b"competing new file").map_err(|e| e.to_string()) },
        )
    }));
    let failure = fs
        .write_text(
            &target,
            "do not publish",
            Some(&FsWriteIntent::CreateIfAbsent),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(failure.code, FsErrorCode::FsNotObserved);
    assert_eq!(
        std::fs::read(fixture.0.join("new.txt")).unwrap(),
        b"competing new file"
    );
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
}
