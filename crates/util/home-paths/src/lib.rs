//! Shared filesystem path helpers for DeepSeek Harness user data. Rust port
//! of `@deepseek-ai/dsh-home-paths`.

use std::path::{Path, PathBuf};

use tokio::fs;

/// Directory name for the default DeepSeek Harness home under the OS home.
pub const DSH_HOME_DIR_NAME: &str = ".dsh";

/// Stable user-facing display form for the default DeepSeek Harness home.
pub const DEFAULT_DSH_HOME_DISPLAY: &str = "~/.dsh";

/// Environment variable that overrides the default DeepSeek Harness home.
pub const DSH_HOME_ENV: &str = "DSH_HOME";

fn canonical_redirect_target(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("目录必须为不含 .. 的绝对路径".into());
    }
    let mut ancestor = path.to_path_buf();
    let mut tail = Vec::new();
    while !ancestor.exists() {
        tail.push(ancestor.file_name().ok_or("目录无效")?.to_os_string());
        ancestor = ancestor.parent().ok_or("目录无效")?.to_path_buf();
    }
    let mut result = std::fs::canonicalize(&ancestor).map_err(|error| error.to_string())?;
    if !result.is_dir() {
        return Err("目录的父级不是文件夹".into());
    }
    for part in tail.into_iter().rev() {
        result.push(part);
    }
    if result.parent().is_none() {
        return Err("不能使用磁盘根目录".into());
    }
    Ok(result)
}

/// Follow the same bounded, validated storage migration chain for every entry
/// point. Redirect markers select data paths; they never establish process ownership.
pub fn resolve_redirect(root: &Path) -> Result<PathBuf, String> {
    let mut root = canonical_redirect_target(root)?;
    let mut visited = std::collections::HashSet::new();
    for _ in 0..16 {
        if !visited.insert(root.clone()) {
            return Err("数据目录重定向形成循环".into());
        }
        let path = root.join(".dsh-home-redirect.json");
        let marker: serde_json::Value = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| format!("{}: {error}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let Some(target) = marker.get("target").and_then(serde_json::Value::as_str) else {
            return Ok(root);
        };
        let next = canonical_redirect_target(Path::new(target))?;
        if !next.join("settings.json").is_file() {
            return Err("迁移后的数据目录不可用，原数据仍保留".into());
        }
        root = next;
    }
    Err("数据目录重定向层数过多".into())
}

#[cfg(test)]
mod redirect_tests {
    use super::*;

    #[test]
    fn redirects_resolve_the_actual_home_and_reject_cycles_and_missing_migrated_data() {
        let root = std::env::temp_dir().join(format!(
            "dsh-home-redirect-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("source");
        let target = root.join("target with spaces");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(source.join("settings.json"), "{}").unwrap();
        std::fs::write(target.join("settings.json"), "{}").unwrap();
        std::fs::write(
            source.join(".dsh-home-redirect.json"),
            serde_json::to_vec(&serde_json::json!({"target":target})).unwrap(),
        )
        .unwrap();
        assert_eq!(
            resolve_redirect(&source).unwrap(),
            std::fs::canonicalize(&target).unwrap()
        );
        std::fs::write(
            target.join(".dsh-home-redirect.json"),
            serde_json::to_vec(&serde_json::json!({"target":source})).unwrap(),
        )
        .unwrap();
        assert_eq!(
            resolve_redirect(&source).unwrap_err(),
            "数据目录重定向形成循环"
        );
        std::fs::remove_file(target.join(".dsh-home-redirect.json")).unwrap();
        std::fs::remove_file(target.join("settings.json")).unwrap();
        assert_eq!(
            resolve_redirect(&source).unwrap_err(),
            "迁移后的数据目录不可用，原数据仍保留"
        );
        std::fs::write(source.join(".dsh-home-redirect.json"), "{").unwrap();
        assert!(resolve_redirect(&source).is_err());
        assert!(resolve_redirect(&root.join("source/../target")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Give a native filesystem watcher one canonical spelling of a path, even
/// when its final components do not exist yet (TS `canonicalizeWatchPath`).
pub async fn canonicalize_watch_path(path: &Path) -> std::io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match fs::canonicalize(&current).await {
            Ok(canonical) => {
                if !missing.is_empty() {
                    // Prove the ancestor is an enumerable directory (the
                    // Windows file-as-parent case reports ENOENT here).
                    let mut directory = fs::read_dir(&canonical).await?;
                    directory.next_entry().await?;
                }
                let mut result = canonical;
                for segment in missing.iter().rev() {
                    result.push(segment);
                }
                return Ok(result);
            }
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(error);
                }
                let parent = current.parent().map(Path::to_path_buf);
                let Some(parent) = parent else {
                    return Err(error);
                };
                if parent == current {
                    return Err(error);
                }
                if let Some(name) = current.file_name() {
                    missing.push(name.to_os_string());
                }
                current = parent;
            }
        }
    }
}

/// Resolve the default DeepSeek Harness home (TS `defaultDshHome`).
pub fn default_dsh_home() -> PathBuf {
    #[cfg(windows)]
    if let Some(local_app_data) = dirs::data_local_dir() {
        return local_app_data.join("DeepSeek Harness");
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(DSH_HOME_DIR_NAME)
}

/// Expand supported tilde prefixes against the operating-system home
/// (TS `expandHomePath`).
pub fn expand_home_path(path: &str) -> PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        return dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(rest);
    }
    PathBuf::from(path)
}

/// Resolve the single-root DeepSeek Harness home (TS `resolveDshHome`).
///
/// Precedence, highest first: an explicit configured path, `$DSH_HOME`, then
/// `~/.dsh`. A blank `$DSH_HOME` is treated as unset.
pub fn resolve_dsh_home(configured: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let from_env = env(DSH_HOME_ENV);
    let selected = match configured {
        Some(configured) => configured.to_string(),
        None => match &from_env {
            Some(from_env) if !from_env.trim().is_empty() => from_env.clone(),
            _ => default_dsh_home().to_string_lossy().to_string(),
        },
    };
    let expanded = expand_home_path(&selected);
    if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(expanded)
    }
}

/// Join path segments onto the resolved DeepSeek Harness home
/// (TS `dshHomePath`).
pub fn dsh_home_path(
    configured: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
    segments: &[&str],
) -> PathBuf {
    let mut path = resolve_dsh_home(configured, env);
    for segment in segments {
        path.push(segment);
    }
    path
}

/// Describe a resolved harness home symbolically for user-facing display
/// (TS `dshHomeDisplay`).
pub fn dsh_home_display(resolved_home: &Path) -> String {
    if *resolved_home == default_dsh_home() {
        DEFAULT_DSH_HOME_DISPLAY.to_string()
    } else {
        format!("${DSH_HOME_ENV}")
    }
}
