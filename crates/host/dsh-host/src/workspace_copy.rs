//! Explicit, registered execution copies and version-checked delivery.
use dsh_workspace_resources::{Store, checked_path};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

fn git(
    root: &Path,
    args: &[String],
    signal: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<String, String> {
    let mut argv = vec![
        "-C".into(),
        display(root),
        "-c".into(),
        "core.hooksPath=/dev/null".into(),
    ];
    argv.extend_from_slice(args);
    futures::executor::block_on(dsh_native_command::run_native_command_bounded(
        "git",
        &argv,
        Some(signal),
        dsh_native_command::NativeCommandLimits {
            timeout: std::time::Duration::from_secs(60),
            stdout_bytes: 8 * 1024 * 1024,
            stderr_bytes: 16384,
        },
    ))
    .map(|result| result.stdout)
    .map_err(|error| format!("Git 执行副本操作失败：{error}"))
}
fn display(path: &Path) -> String {
    dsh_host_apiproxy::native_path_opener::display_native_path(&path.to_string_lossy())
}
fn safe(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = super::web_preview::safe_relative(relative)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or("输入路径必须是工作区内的普通相对路径")?;
    let path = root.join(relative);
    checked_path(&path)?;
    Ok(path)
}
fn copy(
    source: &Path,
    target: &Path,
    signal: &Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<(), String> {
    checked_path(source)?;
    checked_path(target)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let before = std::fs::metadata(source).map_err(|e| e.to_string())?;
    let mut input = std::fs::File::open(source).map_err(|e| e.to_string())?;
    let mut output = std::fs::File::create(target).map_err(|e| e.to_string())?;
    let mut buffer = [0u8; 65536];
    loop {
        if signal() {
            return Err("执行副本操作已取消".into());
        }
        let size = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if size == 0 {
            break;
        }
        output
            .write_all(&buffer[..size])
            .map_err(|e| e.to_string())?;
    }
    output.sync_all().map_err(|e| e.to_string())?;
    let after = std::fs::metadata(source).map_err(|e| e.to_string())?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err("执行输入在复制期间发生变化，请重试".into());
    }
    Ok(())
}
fn hash(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut input = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut state = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        state.update(&buffer[..count]);
    }
    Ok(format!("{:x}", state.finalize()))
}

pub fn prepare(
    store: &Arc<Store>,
    owner: &str,
    project: &str,
    files: Option<&Value>,
    signal: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<Value, String> {
    let root = std::fs::canonicalize(project).map_err(|e| e.to_string())?;
    let mut lease = store.allocate(owner, &root.to_string_lossy(), "copy", "执行副本")?;
    let id = lease.id().to_string();
    let worktree = lease.path().join("worktree");
    store.retain(&id, true, true)?;
    let is_root = git(
        &root,
        &["rev-parse".into(), "--show-toplevel".into()],
        signal.clone(),
    )
    .ok()
    .and_then(|path| std::fs::canonicalize(path.trim()).ok())
    .is_some_and(|path| path == root);
    let mut inputs = Vec::<String>::new();
    if is_root {
        git(
            &root,
            &[
                "-c".into(),
                "core.symlinks=false".into(),
                "worktree".into(),
                "add".into(),
                "--detach".into(),
                display(&worktree),
                "HEAD".into(),
            ],
            signal.clone(),
        )?;
        let admin = git(
            &worktree,
            &["rev-parse".into(), "--absolute-git-dir".into()],
            signal.clone(),
        )?;
        let common = git(
            &root,
            &[
                "rev-parse".into(),
                "--path-format=absolute".into(),
                "--git-common-dir".into(),
            ],
            signal.clone(),
        )?;
        let head = git(
            &worktree,
            &["rev-parse".into(), "HEAD".into()],
            signal.clone(),
        )?;
        store.set_origin(&id,json!({"kind":"git-worktree","gitAdmin":admin.trim(),"gitCommon":common.trim(),"gitHead":head.trim(),"worktree":"worktree"}))?;
        for args in [
            vec![
                "diff".into(),
                "--name-only".into(),
                "--no-renames".into(),
                "-z".into(),
                "HEAD".into(),
            ],
            vec![
                "ls-files".into(),
                "--others".into(),
                "--exclude-standard".into(),
                "-z".into(),
            ],
        ] {
            inputs.extend(
                git(&root, &args, signal.clone())?
                    .split('\0')
                    .filter(|path| !path.is_empty())
                    .map(str::to_string),
            );
        }
    } else {
        std::fs::create_dir_all(&worktree).map_err(|e| e.to_string())?;
        if files.is_none() {
            let mut directories = vec![root.clone()];
            let mut bytes = 0u64;
            while let Some(directory) = directories.pop() {
                for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
                    if signal() {
                        return Err("执行副本操作已取消".into());
                    }
                    let entry = entry.map_err(|e| e.to_string())?;
                    let path = entry.path();
                    let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                    if [
                        ".git",
                        "node_modules",
                        "target",
                        "dist",
                        "build",
                        ".runtime",
                        ".venv",
                        "__pycache__",
                        ".cache",
                    ]
                    .contains(&name.as_str())
                        || checked_path(&path).is_err()
                    {
                        continue;
                    }
                    let relative = path
                        .strip_prefix(&root)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .replace('\\', "/");
                    if super::web_preview::safe_relative(&relative).is_none() {
                        continue;
                    }
                    let metadata = entry.metadata().map_err(|e| e.to_string())?;
                    if metadata.is_dir() {
                        directories.push(path)
                    } else if metadata.is_file() {
                        inputs.push(relative);
                        bytes = bytes.saturating_add(metadata.len());
                    }
                    if inputs.len() > 20000 || bytes > 2 * 1024 * 1024 * 1024 {
                        return Err("执行输入较多，请用 files 限定本次所需文件".into());
                    }
                }
            }
        }
    }
    if let Some(files) = files {
        let files = files.as_array().ok_or("files 必须是路径数组")?;
        inputs.extend(
            files
                .iter()
                .map(|path| {
                    path.as_str()
                        .map(str::to_string)
                        .ok_or("输入路径格式无效".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    inputs.sort();
    inputs.dedup();
    if inputs.len() > 20000 {
        return Err("执行副本输入超过 20000 个文件".into());
    }
    let mut copied = 0;
    for relative in inputs {
        let source = safe(&root, &relative)?;
        let target = safe(&worktree, &relative)?;
        if source.is_file() {
            copy(&source, &target, &signal)?;
            copied += 1;
        } else if !source.exists() && target.is_file() {
            std::fs::remove_file(&target).map_err(|e| e.to_string())?;
        }
    }
    lease.finish(true)?;
    store.retain(&id, false, true)?;
    Ok(
        json!({"id":id,"workdir":display(&worktree),"project":display(&root),"gitWorktree":is_root,"overlaidFiles":copied,"protected":true}),
    )
}

pub fn inspect(project: &str, target: &str) -> Result<Value, String> {
    let root = std::fs::canonicalize(project).map_err(|e| e.to_string())?;
    let path = safe(&root, target)?;
    if !path.exists() {
        return Ok(json!({"path":display(&path),"sha256":Value::Null,"exists":false}));
    }
    Ok(json!({"path":display(&path),"sha256":hash(&path)?,"exists":true}))
}

pub fn promote(
    store: &Store,
    id: &str,
    relative: &str,
    project: &str,
    target: &str,
    expected: &Value,
    signal: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<Value, String> {
    let root = std::fs::canonicalize(project).map_err(|e| e.to_string())?;
    let destination = safe(&root, target)?;
    let source = store.path(id, relative)?;
    let matches = |path: &Path| -> Result<bool, String> {
        Ok(if expected.is_null() {
            !path.exists()
        } else {
            expected
                .as_str()
                .is_some_and(|expected| hash(path).is_ok_and(|current| current == expected))
        })
    };
    if !matches(&destination)? {
        return Err("交付目标已变化，请先重新检查版本".into());
    }
    let temp = destination.with_file_name(format!(".dsh-delivery-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        copy(&source, &temp, &signal)?;
        let checksum = hash(&temp)?;
        if !matches(&destination)? {
            return Err("交付期间目标被修改，候选产物保留".into());
        }
        if expected.is_null() {
            // Link only the copied snapshot in the destination directory, then
            // unlink its temporary name. The execution source has a separate inode.
            std::fs::hard_link(&temp, &destination)
                .map_err(|e| format!("无法原子新建交付文件，候选产物保留：{e}"))?;
        } else {
            std::fs::rename(&temp, &destination).map_err(|e| e.to_string())?;
        }
        Ok(json!({"path":display(&destination),"sha256":checksum,"sourceId":id}))
    })();
    let _ = std::fs::remove_file(&temp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> PathBuf {
        let path = std::env::temp_dir().join(format!("dsh-copy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn worktree_copy_preserves_local_edits_and_recovers_its_git_link() {
        let root = fixture();
        let project = root.join("project");
        std::fs::create_dir(&project).unwrap();
        let signal: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(|| false);
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@invalid"],
        ] {
            git(
                &project,
                &args.into_iter().map(str::to_string).collect::<Vec<_>>(),
                signal.clone(),
            )
            .unwrap();
        }
        std::fs::write(project.join("source.txt"), "committed").unwrap();
        git(
            &project,
            &["add".into(), "source.txt".into()],
            signal.clone(),
        )
        .unwrap();
        git(
            &project,
            &["commit".into(), "-qm".into(), "base".into()],
            signal.clone(),
        )
        .unwrap();
        std::fs::write(project.join("source.txt"), "local edit").unwrap();
        std::fs::write(project.join("input.txt"), "untracked input").unwrap();
        let store = Store::open(root.join("managed")).unwrap();
        let value = prepare(
            &store,
            "owner",
            &project.to_string_lossy(),
            None,
            signal.clone(),
        )
        .unwrap();
        let id = value["id"].as_str().unwrap();
        let copy = PathBuf::from(value["workdir"].as_str().unwrap());
        assert_eq!(
            std::fs::read_to_string(copy.join("source.txt")).unwrap(),
            "local edit"
        );
        assert_eq!(
            std::fs::read_to_string(copy.join("input.txt")).unwrap(),
            "untracked input"
        );
        std::fs::write(copy.join("source.txt"), "candidate").unwrap();
        assert_eq!(
            std::fs::read_to_string(project.join("source.txt")).unwrap(),
            "local edit"
        );
        let expected = inspect(&project.to_string_lossy(), "source.txt").unwrap()["sha256"].clone();
        promote(
            &store,
            id,
            "worktree/source.txt",
            &project.to_string_lossy(),
            "source.txt",
            &expected,
            signal.clone(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(project.join("source.txt")).unwrap(),
            "candidate"
        );
        assert!(
            promote(
                &store,
                id,
                "worktree/source.txt",
                &project.to_string_lossy(),
                "source.txt",
                &expected,
                signal.clone()
            )
            .is_err()
        );
        store.retain(id, false, false).unwrap();
        let admin_path = PathBuf::from(
            store.get(id).unwrap().origin.unwrap()["gitAdmin"]
                .as_str()
                .unwrap(),
        );
        let pointer = admin_path.join("gitdir");
        let original_pointer = std::fs::read(&pointer).unwrap();
        std::fs::write(&pointer, display(&project.join("source.txt"))).unwrap();
        assert!(
            store.quarantine(id).is_err(),
            "a different existing file is not the owned Git pointer"
        );
        assert!(copy.join("source.txt").is_file());
        std::fs::write(&pointer, original_pointer).unwrap();
        store.quarantine(id).unwrap();
        let recovered = store.path(id, "worktree").unwrap();
        assert!(
            git(
                &recovered,
                &["status".into(), "--porcelain".into()],
                signal.clone()
            )
            .is_ok()
        );
        store.restore(id).unwrap();
        assert!(
            git(
                &copy,
                &["status".into(), "--porcelain".into()],
                signal.clone()
            )
            .is_ok()
        );
        let admin = PathBuf::from(
            store.get(id).unwrap().origin.unwrap()["gitAdmin"]
                .as_str()
                .unwrap(),
        );
        store.quarantine(id).unwrap();
        store
            .collect(
                &Default::default(),
                dsh_workspace_resources::now() + 4 * 86400,
                false,
            )
            .unwrap();
        assert!(!admin.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn plain_copy_excludes_build_cache_and_new_delivery_does_not_alias_source() {
        let root = fixture();
        let project = root.join("project");
        std::fs::create_dir_all(project.join("node_modules")).unwrap();
        std::fs::write(project.join("node_modules/unused"), "cache").unwrap();
        std::fs::write(project.join("input.txt"), "input").unwrap();
        let store = Store::open(root.join("managed")).unwrap();
        let signal: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(|| false);
        let value = prepare(
            &store,
            "owner",
            &project.to_string_lossy(),
            None,
            signal.clone(),
        )
        .unwrap();
        let id = value["id"].as_str().unwrap();
        let copy = PathBuf::from(value["workdir"].as_str().unwrap());
        assert!(!copy.join("node_modules").exists());
        promote(
            &store,
            id,
            "worktree/input.txt",
            &project.to_string_lossy(),
            "output.txt",
            &Value::Null,
            signal.clone(),
        )
        .unwrap();
        std::fs::write(copy.join("input.txt"), "changed later").unwrap();
        assert_eq!(
            std::fs::read_to_string(project.join("output.txt")).unwrap(),
            "input"
        );
        assert!(
            promote(
                &store,
                id,
                "worktree/input.txt",
                &project.to_string_lossy(),
                "output.txt",
                &Value::Null,
                signal
            )
            .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
