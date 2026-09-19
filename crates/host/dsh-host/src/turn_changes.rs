//! Bounded before/after file observations at Agent turn boundaries.
use cordis::{Context, EventOptions, NextFn, downcast_arc};
use dsh_agent::{AgentPreStepPayload, AgentTurnStoppingPayload};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;

#[derive(Clone, Serialize, Deserialize)]
struct FileVersion {
    hash: String,
    text: Option<String>,
    reason: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
struct Snapshot {
    files: BTreeMap<String, FileVersion>,
    incomplete: bool,
}
pub(crate) struct TurnChanges {
    root: PathBuf,
    active: Mutex<BTreeMap<String, (u64, PathBuf)>>,
    capture: tokio::sync::Semaphore,
}
fn key(owner: &str) -> String {
    format!("{:x}", Sha256::digest(owner.as_bytes()))
}
async fn snapshot(root: &Path) -> Result<Snapshot, String> {
    let root = tokio::fs::canonicalize(root)
        .await
        .map_err(|e| e.to_string())?;
    let mut result = Snapshot::default();
    let mut command = tokio::process::Command::new("git");
    command
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(&root)
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
        .await
        .map_err(|_| "Workspace file enumeration timed out")?
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("此工作区尚未启用 Git，无法建立回合文件基线".into());
    }
    if output.stdout.len() > 2 * 1024 * 1024 {
        return Err("工作区文件清单超过回合审阅预算".into());
    }
    let mut total = 0usize;
    for (index, path) in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .enumerate()
    {
        if index >= 5000 {
            result.incomplete = true;
            break;
        }
        let Ok(relative) = std::str::from_utf8(path) else {
            result.incomplete = true;
            continue;
        };
        let path = root.join(relative);
        let Ok(resolved) = tokio::fs::canonicalize(&path).await else {
            continue;
        };
        if !resolved.starts_with(&root)
            || tokio::fs::symlink_metadata(&path)
                .await
                .is_ok_and(|m| m.file_type().is_symlink())
        {
            result.incomplete = true;
            continue;
        }
        let metadata = tokio::fs::metadata(&resolved)
            .await
            .map_err(|e| e.to_string())?;
        if !metadata.is_file() {
            continue;
        }
        if metadata.len() > 1024 * 1024 || total + metadata.len() as usize > 16 * 1024 * 1024 {
            result.incomplete = true;
            result.files.insert(
                relative.into(),
                FileVersion {
                    hash: String::new(),
                    text: None,
                    reason: Some("文件超过快照预算".into()),
                },
            );
            continue;
        }
        use tokio::io::AsyncReadExt;
        let mut data = Vec::new();
        tokio::fs::File::open(&resolved)
            .await
            .map_err(|e| e.to_string())?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut data)
            .await
            .map_err(|e| e.to_string())?;
        if data.len() > 1024 * 1024 {
            result.incomplete = true;
            continue;
        }
        total += data.len();
        let hash = format!("{:x}", Sha256::digest(&data));
        let text = if data.contains(&0) {
            None
        } else {
            String::from_utf8(data).ok()
        };
        let reason = text
            .is_none()
            .then(|| "二进制或非 UTF-8 文件，仅比较内容哈希".into());
        result
            .files
            .insert(relative.into(), FileVersion { hash, text, reason });
    }
    Ok(result)
}
impl TurnChanges {
    pub async fn install(ctx: &Context, root: &Path) -> Result<Arc<Self>, String> {
        let root = root.join("cache/turn-changes");
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|e| e.to_string())?;
        let mut entries = tokio::fs::read_dir(&root)
            .await
            .map_err(|e| e.to_string())?;
        while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.len() > 77
                && name.ends_with("-before.json")
                && name.as_bytes()[..64].iter().all(|c| c.is_ascii_hexdigit())
                && name.as_bytes()[64] == b'-'
                && name[65..name.len() - 12]
                    .bytes()
                    .all(|c| c.is_ascii_digit())
            {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
        let service = Arc::new(Self {
            root,
            active: Mutex::new(BTreeMap::new()),
            capture: tokio::sync::Semaphore::new(2),
        });
        let begin = service.clone();
        ctx.on(
            "agent/pre-step",
            Arc::new(move |_, args| {
                let service = begin.clone();
                let payload = args.first().and_then(downcast_arc::<AgentPreStepPayload>);
                let next = args.last().and_then(downcast_arc::<NextFn>);
                Box::pin(async move {
                    if let Some(payload) = payload {
                        if let Some(cwd) = payload.agent.session().header().cwd.clone() {
                            service
                                .begin(
                                    payload.agent.id().as_str(),
                                    payload.turn,
                                    PathBuf::from(cwd),
                                )
                                .await;
                        }
                    }
                    match next {
                        Some(next) => Some(next.call().await),
                        None => None,
                    }
                })
            }),
            EventOptions::default().global(true),
        )
        .await;
        let finish = service.clone();
        ctx.on(
            "agent/turn-finished",
            Arc::new(move |_, args| {
                let service = finish.clone();
                let payload = args
                    .first()
                    .and_then(downcast_arc::<AgentTurnStoppingPayload>);
                Box::pin(async move {
                    if let Some(payload) = payload {
                        service
                            .finish(payload.agent.id().as_str(), payload.turn)
                            .await;
                    }
                    None
                })
            }),
            EventOptions::default().global(true),
        )
        .await;
        Ok(service)
    }
    async fn begin(&self, owner: &str, turn: u64, cwd: PathBuf) {
        let mut active = self.active.lock().await;
        if active
            .get(owner)
            .is_some_and(|(existing, _)| *existing == turn)
        {
            return;
        }
        if active.len() >= 32 {
            return;
        }
        active.insert(owner.into(), (turn, cwd.clone()));
        drop(active);
        let Ok(_permit) = self.capture.acquire().await else {
            return;
        };
        let value = match snapshot(&cwd).await {
            Ok(value) => serde_json::to_value(value).unwrap_or_default(),
            Err(error) => serde_json::json!({"error":error}),
        };
        let _ = tokio::fs::write(
            self.root.join(format!("{}-{turn}-before.json", key(owner))),
            value.to_string(),
        )
        .await;
    }
    async fn finish(&self, owner: &str, turn: u64) {
        let baseline = {
            let mut active = self.active.lock().await;
            if active.get(owner).is_some_and(|(number, _)| *number == turn) {
                active.remove(owner)
            } else {
                None
            }
        };
        let Some((_, cwd)) = baseline else { return };
        let Ok(_permit) = self.capture.acquire().await else {
            return;
        };
        let before_path = self.root.join(format!("{}-{turn}-before.json", key(owner)));
        let result=async{
            let before:Snapshot=serde_json::from_slice(&tokio::fs::read(&before_path).await.map_err(|e|e.to_string())?).map_err(|_|"回合开始时未能取得完整基线")?;
            let after=snapshot(&cwd).await?;
            let paths:std::collections::BTreeSet<_>=before.files.keys().chain(after.files.keys()).collect();
            let mut files=Vec::new();let mut budget=0usize;
            for path in paths{
                let old=before.files.get(path);let new=after.files.get(path);
                if old.zip(new).is_some_and(|(a,b)|!a.hash.is_empty()&&a.hash==b.hash){continue;}
                let uncertain=old.is_some_and(|v|v.hash.is_empty())||new.is_some_and(|v|v.hash.is_empty())||(old.is_none()&&before.incomplete)||(new.is_none()&&after.incomplete);
                let old_text=old.and_then(|v|v.text.as_deref());let new_text=new.and_then(|v|v.text.as_deref());
                budget+=old_text.map_or(0,str::len)+new_text.map_or(0,str::len);
                let reason=old.and_then(|v|v.reason.clone()).or_else(||new.and_then(|v|v.reason.clone())).or_else(||uncertain.then(||"快照范围不完整，无法判定该文件变化".into())).or_else(||(budget>4*1024*1024).then(||"差异正文超过预览预算".into()));
                files.push(serde_json::json!({"path":path,"kind":if uncertain{"unavailable"}else if old.is_none(){"added"}else if new.is_none(){"deleted"}else{"modified"},"before":if reason.is_none(){old_text}else{None},"after":if reason.is_none(){new_text}else{None},"beforeHash":old.map(|v|&v.hash),"afterHash":new.map(|v|&v.hash),"reason":reason}));
            }
            Ok::<_,String>(serde_json::json!({"turn":turn,"files":files,"incomplete":before.incomplete||after.incomplete}))
        }.await;
        let value = result
            .unwrap_or_else(|error| serde_json::json!({"turn":turn,"error":error,"files":[]}));
        let _ = dsh_atomic_write::write_file_atomic(
            &self.root.join(format!("{}.json", key(owner))),
            value.to_string().as_bytes(),
            dsh_atomic_write::WriteFileAtomicOptions {
                mode: 0o600,
                dir_mode: Some(0o700),
            },
        )
        .await;
        let _ = tokio::fs::remove_file(before_path).await;
        // Review snapshots are a cache, not an unbounded second session archive.
        if let Ok(mut directory) = tokio::fs::read_dir(&self.root).await {
            let mut reports = Vec::new();
            while let Ok(Some(entry)) = directory.next_entry().await {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.len() == 69
                    && name.ends_with(".json")
                    && name.as_bytes()[..64].iter().all(|c| c.is_ascii_hexdigit())
                {
                    if let Ok(metadata) = entry.metadata().await {
                        reports.push((metadata.modified().ok(), entry.path()));
                    }
                }
            }
            reports.sort_by_key(|item| item.0);
            let remove = reports.len().saturating_sub(64);
            for (_, path) in reports.into_iter().take(remove) {
                let _ = tokio::fs::remove_file(path).await;
            }
        }
    }
    pub async fn latest(&self, owner: &str) -> Result<serde_json::Value, String> {
        let path = self.root.join(format!("{}.json", key(owner)));
        match tokio::fs::read(path).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(serde_json::json!({"files":[],"pending":true}))
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn compares_turn_boundaries_including_preexisting_uncommitted_content() {
        let root = std::env::temp_dir().join(format!("turn-review-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        assert!(
            tokio::process::Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(&workspace)
                .status()
                .await
                .unwrap()
                .success()
        );
        tokio::fs::write(workspace.join("file.txt"), "index baseline\n")
            .await
            .unwrap();
        assert!(
            tokio::process::Command::new("git")
                .args(["add", "file.txt"])
                .current_dir(&workspace)
                .status()
                .await
                .unwrap()
                .success()
        );
        tokio::fs::write(workspace.join("file.txt"), "preexisting dirty text\n")
            .await
            .unwrap();
        tokio::fs::write(workspace.join("deleted.txt"), "remove me\n")
            .await
            .unwrap();
        let ctx = Context::root();
        let service = TurnChanges::install(&ctx, &root).await.unwrap();
        service.begin("session", 7, workspace.clone()).await;
        tokio::fs::write(workspace.join("file.txt"), "new turn text\n")
            .await
            .unwrap();
        tokio::fs::remove_file(workspace.join("deleted.txt"))
            .await
            .unwrap();
        tokio::fs::write(workspace.join("new.txt"), "added\n")
            .await
            .unwrap();
        service.finish("session", 7).await;
        let result = service.latest("session").await.unwrap();
        assert_eq!(result["turn"], 7);
        assert_eq!(result["files"].as_array().unwrap().len(), 3);
        let file = result["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["path"] == "file.txt")
            .unwrap();
        assert_eq!(file["before"], "preexisting dirty text\n");
        assert_eq!(file["after"], "new turn text\n");
        assert!(service.active.lock().await.is_empty());
        ctx.fiber.dispose().await;
        drop(service);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
