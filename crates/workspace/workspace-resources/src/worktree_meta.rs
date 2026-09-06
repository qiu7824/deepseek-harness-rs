//! Only the Git administrative directory recorded at copy creation is managed.
use super::*;

fn native_key(path: &Path) -> String {
    let value = path.to_string_lossy();
    #[cfg(windows)]
    {
        return value
            .trim_start_matches(r"\\?\")
            .replace('\\', "/")
            .to_ascii_lowercase();
    }
    #[cfg(not(windows))]
    {
        value.into_owned()
    }
}
fn git_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/")
}
fn admin(row: &Resource, payload: &Path) -> Result<Option<PathBuf>> {
    let Some(origin) = row
        .origin
        .as_ref()
        .filter(|origin| origin["kind"] == "git-worktree")
    else {
        return Ok(None);
    };
    let directory = PathBuf::from(
        origin["gitAdmin"]
            .as_str()
            .ok_or("执行副本缺少 Git 归属信息")?,
    );
    let common = PathBuf::from(
        origin["gitCommon"]
            .as_str()
            .ok_or("执行副本缺少 Git 对象库信息")?,
    );
    checked_path(&directory)?;
    checked_path(&common)?;
    let canonical =
        fs::canonicalize(&directory).map_err(|e| format!("Git 副本记录不可访问，材料保留：{e}"))?;
    let parent = fs::canonicalize(common.join("worktrees")).map_err(|e| e.to_string())?;
    if canonical.parent() != Some(parent.as_path()) {
        return Err("Git 副本记录越出受管范围".into());
    }
    let head = fs::read_to_string(directory.join("HEAD")).map_err(|e| e.to_string())?;
    if Some(head.trim()) != origin["gitHead"].as_str() {
        return Err("执行副本含有新的提交或分支，先保留提交后再清理".into());
    }
    for name in [
        "MERGE_HEAD",
        "REBASE_HEAD",
        "rebase-merge",
        "rebase-apply",
        "index.lock",
    ] {
        if directory.join(name).exists() {
            return Err("Git 副本仍有待恢复事务".into());
        }
    }
    let pointer = fs::read_to_string(directory.join("gitdir")).map_err(|e| e.to_string())?;
    if native_key(Path::new(pointer.trim())) != native_key(&payload.join("worktree/.git")) {
        return Err("Git 副本关联已变化，原数据保留".into());
    }
    Ok(Some(directory))
}
pub(super) fn move_payload(row: &Resource, source: &Path, target: &Path) -> Result<()> {
    let directory = admin(row, source)?;
    fs::rename(source, target).map_err(|e| e.to_string())?;
    if let Some(directory) = directory {
        let pointer = format!("{}\n", git_path(&target.join("worktree/.git")));
        if let Err(error) = atomic(&directory.join("gitdir"), pointer.as_bytes()) {
            let rollback = fs::rename(target, source);
            return Err(format!(
                "Git 副本位置保存失败：{error}；恢复结果：{rollback:?}"
            ));
        }
    }
    Ok(())
}
pub(super) fn purge(row: &Resource, payload: &Path) -> Result<()> {
    let directory = admin(row, payload)?;
    if let Some(directory) = &directory {
        tree_bytes(directory)?;
    }
    tree_bytes(payload)?;
    fs::remove_dir_all(payload).map_err(|e| e.to_string())?;
    if let Some(directory) = directory {
        fs::remove_dir_all(directory)
            .map_err(|e| format!("材料已回收，Git 副本记录清理失败：{e}"))?;
    }
    Ok(())
}
