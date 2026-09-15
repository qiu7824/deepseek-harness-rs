//! Git adoption uses literal argv, bounded output and an explicit local destination.
use std::{path::Path, time::Duration};

fn arguments(source: &str, destination: &str, branch: Option<&str>) -> Result<Vec<String>, String> {
    if source.trim().is_empty()
        || source.starts_with('-')
        || source.chars().any(char::is_control)
        || source.contains("::")
    {
        return Err("仓库地址无效；请使用 HTTPS、SSH 或本地仓库路径".into());
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
) -> Result<(), String> {
    let args = arguments(source, destination, branch)?;
    // Refuse even empty existing directories: a failed clone must never replace user files.
    if tokio::fs::try_exists(destination)
        .await
        .map_err(|_| "无法检查克隆目录")?
    {
        return Err("克隆目录已存在，请选择新的目录；已有仓库请使用添加本地工作区".into());
    }
    dsh_native_command::run_native_command_bounded(
        "git",
        &args,
        None,
        dsh_native_command::NativeCommandLimits {
            timeout: Duration::from_secs(600),
            stdout_bytes: 256 * 1024,
            stderr_bytes: 256 * 1024,
        },
    )
    .await
    .map(|_| ())
    .map_err(|error| {
        format!(
            "Git 克隆失败（{}）；请检查仓库地址、访问权限和网络，失败目录保留以供检查",
            error.code.unwrap_or_else(|| "启动失败".into())
        )
    })
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
}
