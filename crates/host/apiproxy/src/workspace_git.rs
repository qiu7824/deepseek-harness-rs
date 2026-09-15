//! Git adoption uses literal argv, bounded output and an explicit local destination.
use dsh_subprocess::{
    SubprocessCollect, SubprocessOutputMode, SubprocessRuntime, SubprocessSpawnSpec,
    SubprocessStdinMode, SubprocessStdio,
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

type Operations = HashMap<String, (Arc<AtomicBool>, Instant, bool)>;
static OPERATIONS: LazyLock<Mutex<Operations>> = LazyLock::new(Default::default);

fn operation(id: &str, starting: bool) -> Result<Arc<AtomicBool>, String> {
    uuid::Uuid::parse_str(id).map_err(|_| "无效的克隆操作标识")?;
    let mut operations = OPERATIONS.lock().map_err(|_| "克隆操作状态不可用")?;
    operations.retain(|_, (_, at, running)| *running || at.elapsed() < Duration::from_secs(660));
    if !operations.contains_key(id) && operations.len() >= 128 {
        return Err("克隆操作过多，请稍后重试".into());
    }
    let entry = operations
        .entry(id.into())
        .or_insert_with(|| (Arc::new(AtomicBool::new(false)), Instant::now(), false));
    if starting && entry.2 {
        return Err("克隆操作正在运行".into());
    }
    if starting {
        entry.2 = true;
    }
    Ok(entry.0.clone())
}

pub(crate) fn cancel(id: &str) -> Result<(), String> {
    operation(id, false)?.store(true, Ordering::SeqCst);
    Ok(())
}

struct OperationGuard(String);
impl Drop for OperationGuard {
    fn drop(&mut self) {
        if let Ok(mut entries) = OPERATIONS.lock() {
            entries.remove(&self.0);
        }
    }
}

fn arguments(source: &str, destination: &str, branch: Option<&str>) -> Result<Vec<String>, String> {
    if source.trim().is_empty()
        || source.starts_with('-')
        || source.chars().any(char::is_control)
        || source.contains("::")
    {
        return Err("仓库地址无效；请使用 HTTPS、SSH 或本地仓库路径".into());
    }
    if let Some((scheme, rest)) = source.split_once("://") {
        if !matches!(scheme, "https" | "http" | "ssh" | "file") || rest.contains(['?', '#']) {
            return Err("仓库地址仅支持 HTTPS、SSH 或本地路径，且不能包含查询参数或片段".into());
        }
        if scheme == "ssh"
            && rest
                .split('/')
                .next()
                .and_then(|a| a.split_once('@'))
                .is_some_and(|(user, _)| user.contains(':'))
        {
            return Err("SSH 地址不能包含密码；请使用已有 SSH 密钥".into());
        }
    }
    if let Some(authority) = source
        .strip_prefix("https://")
        .or_else(|| source.strip_prefix("http://"))
    {
        if authority
            .split('/')
            .next()
            .unwrap_or_default()
            .contains('@')
        {
            return Err("仓库地址不能包含密码或访问令牌；请使用 Git 凭据管理器".into());
        }
    }
    if !Path::new(destination).is_absolute() || destination.chars().any(char::is_control) {
        return Err("请选择本机绝对目录作为克隆位置".into());
    }
    let mut args = vec![
        "-c".into(),
        "protocol.ext.allow=never".into(),
        "-c".into(),
        "credential.interactive=false".into(),
        "-c".into(),
        "core.hooksPath=/dev/null".into(),
        "clone".into(),
    ];
    if let Some(branch) = branch.filter(|b| !b.is_empty()) {
        if branch.starts_with('-') || branch.chars().any(char::is_control) {
            return Err("分支名称无效".into());
        }
        args.extend(["--branch".into(), branch.into()]);
    }
    args.extend(["--".into(), source.into(), destination.into()]);
    Ok(args)
}

pub(crate) async fn clone_repository(
    source: &str,
    destination: &str,
    branch: Option<&str>,
    operation_id: Option<&str>,
) -> Result<(), String> {
    let args = arguments(source, destination, branch)?;
    let id = operation_id
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let cancelled = operation(&id, true)?;
    let _operation = OperationGuard(id);
    if cancelled.load(Ordering::SeqCst) {
        return Err("Git 克隆已取消".into());
    }
    dsh_workspace_resources::checked_path(Path::new(destination))?;
    let parent = Path::new(destination)
        .parent()
        .filter(|p| p.is_dir())
        .ok_or("克隆目录的父目录不存在")?;
    // Refuse even empty existing directories: a failed clone must never replace user files.
    if tokio::fs::try_exists(destination)
        .await
        .map_err(|_| "无法检查克隆目录")?
    {
        return Err("克隆目录已存在，请选择新的目录；已有仓库请使用添加本地工作区".into());
    }
    // Reserve the exact destination atomically; no existing user directory is adopted.
    tokio::fs::create_dir(destination)
        .await
        .map_err(|_| "克隆目录已存在或无法创建，请选择新的目录")?;
    dsh_workspace_resources::checked_path(Path::new(destination))?;
    let runtime = dsh_subprocess_local::LocalSubprocessRuntime::new();
    let deadline = Instant::now();
    let signal = cancelled.clone();
    let collect = || {
        SubprocessOutputMode::Collect(SubprocessCollect {
            max_bytes: 256 * 1024,
            spill: None,
        })
    };
    let child = runtime
        .spawn(SubprocessSpawnSpec {
            argv: std::iter::once("git".into()).chain(args).collect(),
            cwd: parent.to_string_lossy().into_owned(),
            stdio: SubprocessStdio {
                stdin: SubprocessStdinMode::Ignore,
                stdout: collect(),
                stderr: collect(),
            },
            grace_ms: 500,
            signal: Some(Arc::new(move || {
                signal.load(Ordering::SeqCst) || deadline.elapsed() >= Duration::from_secs(600)
            })),
            env: Some(vec![
                ("GIT_TERMINAL_PROMPT".into(), Some("0".into())),
                ("GCM_INTERACTIVE".into(), Some("Never".into())),
                (
                    "GIT_SSH_COMMAND".into(),
                    Some("ssh -o BatchMode=yes -o StrictHostKeyChecking=yes".into()),
                ),
            ]),
        })
        .map_err(|_| "无法启动 Git，请确认已安装 Git；目标目录保留")?;
    let outcome = child
        .done()
        .await
        .map_err(|_| "Git 进程异常；目标目录保留")?;
    if cancelled.load(Ordering::SeqCst) {
        return Err("Git 克隆已取消，未添加工作区；部分目录保留，请检查后选择新目录重试".into());
    }
    if deadline.elapsed() >= Duration::from_secs(600) {
        return Err("Git 克隆超时，未添加工作区；部分目录保留".into());
    }
    if outcome.exit_code != Some(0) {
        return Err(
            "Git 克隆失败；请检查仓库、分支、网络及已有凭据；SSH 主机密钥须预先验证；部分目录保留"
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn destination() -> &'static str {
        if cfg!(windows) {
            r"C:\workspace\new repo"
        } else {
            "/workspace/new repo"
        }
    }
    #[test]
    fn option_and_helper_injection_never_reach_git() {
        for source in [
            "--upload-pack=bad",
            "ext::bad",
            "https://token@host/repo",
            "repo\n--bare",
            "custom://host/repo",
            "https://host/repo?token=secret",
            "ssh://user:secret@host/repo",
        ] {
            assert!(arguments(source, destination(), None).is_err());
        }
        assert!(arguments("https://host/repo", "relative", None).is_err());
        assert!(arguments("https://host/repo", destination(), Some("--bad")).is_err());
    }
    #[test]
    fn ssh_sources_and_branch_remain_literal_arguments() {
        let args = arguments(
            "git@example.test:owner/repo.git",
            destination(),
            Some("feature/test"),
        )
        .unwrap();
        assert_eq!(
            &args[args.len() - 3..],
            &["--", "git@example.test:owner/repo.git", destination()]
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--branch", "feature/test"])
        );
    }

    #[tokio::test]
    async fn cancellation_before_start_does_not_create_destination() {
        let id = uuid::Uuid::new_v4().to_string();
        cancel(&id).unwrap();
        let destination = std::env::temp_dir().join(format!("dsh-cancel-{id}"));
        assert!(
            clone_repository(
                "https://example.invalid/repo",
                &destination.to_string_lossy(),
                None,
                Some(&id)
            )
            .await
            .unwrap_err()
            .contains("取消")
        );
        assert!(!destination.exists());
    }
}
