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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn canonicalize_resolves_existing_ancestor() {
        let dir = std::env::temp_dir().join(format!("dsh-home-{}", std::process::id()));
        let nested = dir.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        let canonical = canonicalize_watch_path(&nested.join("missing.txt"))
            .await
            .unwrap();
        let expected = std::fs::canonicalize(&nested).unwrap().join("missing.txt");
        assert_eq!(canonical, expected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expand_tilde_forms() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home_path("~"), home);
        assert_eq!(expand_home_path("~/x"), home.join("x"));
        assert_eq!(expand_home_path("~\\x"), home.join("x"));
        assert_eq!(expand_home_path("plain"), PathBuf::from("plain"));
    }

    #[test]
    fn resolve_precedence() {
        let env = |name: &str| -> Option<String> {
            (name == "DSH_HOME").then(|| "/env/home".to_string())
        };
        // path.resolve("/cfg") is drive-rooted on Windows; assert loosely.
        let configured = resolve_dsh_home(Some("/cfg"), &env);
        assert!(configured.ends_with(Path::new("cfg")), "got {configured:?}");
        let env_home = resolve_dsh_home(None, &env);
        assert!(
            env_home.ends_with(Path::new("env").join("home"))
                || env_home == PathBuf::from("/env/home"),
            "got {env_home:?}"
        );
        let empty_env = |_name: &str| Some("   ".to_string());
        assert_eq!(resolve_dsh_home(None, &empty_env), default_dsh_home());
        assert_eq!(dsh_home_display(&default_dsh_home()), "~/.dsh");
        assert_eq!(dsh_home_display(Path::new("/other")), "$DSH_HOME");
    }

    #[test]
    fn dsh_home_path_joins() {
        let env = |_name: &str| None::<String>;
        let path = dsh_home_path(None, &env, &["sessions", "x"]);
        assert_eq!(path, default_dsh_home().join("sessions").join("x"));
    }
}
