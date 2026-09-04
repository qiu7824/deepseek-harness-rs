//! Stage every requested copy before publishing paths; rollback published
//! directories if any later step fails, and commit the home redirect last.
use super::{ACTIVE, LOCK, REDIRECT, canonical_target, copy_tree, write_json};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

fn reserve_root(root: &Path) -> Result<File, String> {
    let parent = root.parent().ok_or("数据目录无父级")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let identity = root.to_string_lossy().to_string();
    let identity = if cfg!(windows) {
        identity.to_lowercase()
    } else {
        identity
    };
    let name = format!(".dsh-lock-{:x}.lock", Sha256::digest(identity.as_bytes()));
    let lease = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(parent.join(name))
        .map_err(|e| e.to_string())?;
    lease
        .try_lock()
        .map_err(|_| "该数据目录正在使用，请先关闭其它 Harness 实例".to_string())?;
    Ok(lease)
}

fn finish_lock(root: &Path, reservation: File) -> Result<Vec<File>, String> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(LOCK))
        .map_err(|e| e.to_string())?;
    lock.try_lock()
        .map_err(|_| "该数据目录正在使用，请先关闭其它 Harness 实例".to_string())?;
    Ok(vec![reservation, lock])
}

pub(super) fn acquire_lock(root: &Path) -> Result<Vec<File>, String> {
    finish_lock(root, reserve_root(root)?)
}

pub(super) fn desired_paths(
    value: &Value,
    active: &BTreeMap<String, PathBuf>,
) -> Result<BTreeMap<String, PathBuf>, String> {
    let mut paths = active.clone();
    for name in active.keys() {
        if let Some(value) = value.get(name) {
            let path = value.as_str().ok_or("目录必须为文本")?;
            paths.insert(name.clone(), canonical_target(Path::new(path))?);
        }
    }
    let old = &active["dataDirectory"];
    let new = paths["dataDirectory"].clone();
    if old != &new {
        for (name, path) in &mut paths {
            if name != "dataDirectory"
                && let Ok(relative) = path.strip_prefix(old)
            {
                *path = new.join(relative);
            }
        }
    }
    Ok(paths)
}

fn ensure_empty(source: &Path, target: &Path) -> Result<(), String> {
    if source == target {
        return Ok(());
    }
    if source.starts_with(target) || target.starts_with(source) {
        return Err("源目录与目标目录不能互相包含".into());
    }
    if target.exists()
        && fs::read_dir(target)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
    {
        return Err(format!("目标目录必须为空：{}", target.display()));
    }
    Ok(())
}

pub(super) fn validate_targets(
    paths: &BTreeMap<String, PathBuf>,
    active: &BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    let data = &paths["dataDirectory"];
    for (name, target) in paths {
        let source = &active[name];
        if name != "dataDirectory" && (target == data || data.starts_with(target)) {
            return Err("用途目录不能等于或包含数据根目录".into());
        }
        ensure_empty(source, target)?;
        if name == "dataDirectory" {
            continue;
        }
        for (other, other_target) in paths {
            if other != "dataDirectory"
                && other != name
                && (target.starts_with(other_target) || other_target.starts_with(target))
            {
                return Err("缓存、运行环境与测试目录不能相同或互相包含".into());
            }
        }
        if target != source {
            for (other, other_source) in active {
                if other != "dataDirectory"
                    && other != name
                    && (target.starts_with(other_source) || other_source.starts_with(target))
                {
                    return Err("目标目录不能与其它用途的当前目录重叠".into());
                }
            }
        }
    }
    Ok(())
}

struct StagedCopy {
    target: PathBuf,
    stage: PathBuf,
    target_existed: bool,
    published: bool,
    committed: bool,
}

impl StagedCopy {
    fn prepare(source: &Path, target: &Path) -> Result<Self, String> {
        ensure_empty(source, target)?;
        let parent = target.parent().ok_or("目标目录无父级")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let stage = parent.join(format!(".dsh-migration-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&stage).map_err(|e| e.to_string())?;
        let copy = Self {
            target: target.into(),
            stage,
            target_existed: target.exists(),
            published: false,
            committed: false,
        };
        if source.exists() {
            copy_tree(source, &copy.stage)?;
        }
        Ok(copy)
    }
    fn publish(&mut self) -> Result<(), String> {
        // Removal succeeds only if the chosen target is still an empty folder.
        if self.target.exists() {
            fs::remove_dir(&self.target).map_err(|e| format!("目标目录已变化：{e}"))?;
        }
        fs::rename(&self.stage, &self.target)
            .map_err(|e| format!("发布迁移目录 {} 失败：{e}", self.target.display()))?;
        self.published = true;
        Ok(())
    }
}

impl Drop for StagedCopy {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // If rollback cannot rename the tree, retain it for recovery. Never
        // recursively remove a published target selected by the user.
        if self.published && fs::rename(&self.target, &self.stage).is_err() {
            return;
        }
        if self.target_existed && !self.target.exists() {
            let _ = fs::create_dir(&self.target);
        }
        if self.stage.parent() == self.target.parent()
            && self
                .stage
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".dsh-migration-"))
        {
            let _ = fs::remove_dir_all(&self.stage);
        }
    }
}

pub(super) fn migrate(
    active: &BTreeMap<String, PathBuf>,
    paths: &BTreeMap<String, PathBuf>,
    settings: &Value,
) -> Result<Option<Vec<File>>, String> {
    validate_targets(paths, active)?;
    let old_root = &active["dataDirectory"];
    let root = &paths["dataDirectory"];
    let moving_root = old_root != root;
    // Reserve the final path through a sibling lock, so no open file inside
    // the staging directory prevents its rename on Windows.
    let reservation = if moving_root {
        Some(reserve_root(root)?)
    } else {
        None
    };
    let mut copies = Vec::new();
    if moving_root {
        copies.push(StagedCopy::prepare(old_root, root)?);
    }
    for name in ["cacheDirectory", "environmentDirectory", "testDirectory"] {
        let source = &active[name];
        let target = &paths[name];
        if source == target {
            continue;
        }
        if moving_root && target.starts_with(root) {
            let destination = copies[0]
                .stage
                .join(target.strip_prefix(root).map_err(|e| e.to_string())?);
            let inherited = source
                .strip_prefix(old_root)
                .ok()
                .is_some_and(|relative| root.join(relative) == *target);
            if !inherited {
                if destination.exists()
                    && fs::read_dir(&destination)
                        .map_err(|e| e.to_string())?
                        .next()
                        .is_some()
                {
                    return Err(format!("目标目录与已有数据重叠：{}", target.display()));
                }
                if source.exists() {
                    copy_tree(source, &destination)?;
                }
            }
            fs::create_dir_all(&destination).map_err(|e| e.to_string())?;
        } else {
            copies.push(StagedCopy::prepare(source, target)?);
        }
    }
    let mut saved = settings.clone();
    saved["storage-paths"] = json!(paths);
    if moving_root {
        let stage_root = &copies[0].stage;
        write_json(&stage_root.join("settings.json"), &saved)?;
        write_json(&stage_root.join(ACTIVE), &json!(paths))?;
    }
    for copy in &mut copies {
        copy.publish()?;
    }
    let new_lock = reservation
        .map(|reservation| finish_lock(root, reservation))
        .transpose()?;
    if moving_root {
        write_json(&old_root.join(REDIRECT), &json!({"target":root}))?;
    } else {
        for path in paths.values() {
            fs::create_dir_all(path).map_err(|e| e.to_string())?;
        }
        write_json(&root.join("settings.json"), &saved)?;
        write_json(&root.join(ACTIVE), &json!(paths))?;
    }
    for copy in &mut copies {
        copy.committed = true;
    }
    Ok(new_lock)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_late_publish_failure_rolls_back_earlier_targets() {
        let base = std::env::temp_dir().join(format!("dsh-copy-{}", uuid::Uuid::new_v4()));
        let source = base.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("data"), "keep").unwrap();
        let first_target = base.join("first");
        let second_target = base.join("second");
        {
            let mut first = StagedCopy::prepare(&source, &first_target).unwrap();
            let mut second = StagedCopy::prepare(&source, &second_target).unwrap();
            first.publish().unwrap();
            fs::create_dir(&second_target).unwrap();
            fs::write(second_target.join("external"), "unrelated").unwrap();
            assert!(second.publish().is_err());
        }
        assert!(!first_target.exists());
        assert_eq!(fs::read_to_string(source.join("data")).unwrap(), "keep");
        assert_eq!(
            fs::read_to_string(second_target.join("external")).unwrap(),
            "unrelated"
        );
        fs::remove_dir_all(base).unwrap();
    }
}
