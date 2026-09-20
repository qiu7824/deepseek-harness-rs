use crate::args::Request;
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub struct Environment {
    pub values: HashMap<String, String>,
    pub reads: Vec<PathBuf>,
    pub writes: Vec<PathBuf>,
    pub denied: Vec<PathBuf>,
    pub command: Vec<String>,
}

fn allowed(name: &str) -> bool {
    matches!(
        name.to_ascii_uppercase().as_str(),
        "PATH"
            | "PATHEXT"
            | "SYSTEMROOT"
            | "WINDIR"
            | "COMSPEC"
            | "SYSTEMDRIVE"
            | "PROGRAMFILES"
            | "PROGRAMFILES(X86)"
            | "PROGRAMW6432"
            | "PROGRAMDATA"
            | "OS"
            | "PROCESSOR_ARCHITECTURE"
            | "NUMBER_OF_PROCESSORS"
            | "LIB"
            | "LIBPATH"
            | "INCLUDE"
            | "VCTOOLSINSTALLDIR"
            | "VSINSTALLDIR"
            | "WINDOWSSDKDIR"
            | "WINDOWSSDKVERSION"
            | "PSMODULEPATH"
            | "PSEXECUTIONPOLICYPREFERENCE"
            | "TERM"
            | "COLORTERM"
            | "LANG"
            | "LC_ALL"
    )
}

use crate::launcher::locate;

pub fn prepare(request: &Request) -> Result<Environment> {
    let mut values: HashMap<_, _> = std::env::vars()
        .filter(|(k, _)| allowed(k))
        .map(|(k, v)| (k.to_ascii_uppercase(), v))
        .collect();
    let key = format!(
        "{:x}",
        Sha256::digest(
            format!(
                "{}:{}",
                request.workspace.display(),
                request.session.as_deref().unwrap_or("project")
            )
            .to_lowercase()
            .as_bytes()
        )
    );
    let runtime = request.home.join("runtime").join(&key[..20]);
    let mut reads = request.reads.clone();
    let mut writes = Vec::new();
    // Read-only policy never grants writes to runtime caches. Initialization
    // by the host is separate from permissions given to the child.
    for (name, folder) in [
        ("HOME", "home"),
        ("USERPROFILE", "home"),
        ("APPDATA", "home/AppData/Roaming"),
        ("LOCALAPPDATA", "home/AppData/Local"),
        ("TEMP", "tmp"),
        ("TMP", "tmp"),
        ("CARGO_HOME", "cargo"),
        ("RUSTUP_HOME", "rustup"),
        ("NPM_CONFIG_CACHE", "npm"),
        ("PIP_CACHE_DIR", "pip"),
    ] {
        let path = runtime.join(folder);
        std::fs::create_dir_all(&path)?;
        values.insert(name.into(), path.to_string_lossy().into_owned());
    }
    reads.push(runtime.clone());
    if !request.read_only {
        writes.push(runtime);
        writes.extend(request.temps.clone());
    }
    values.insert("PYTHONUTF8".into(), "1".into());
    values.insert("PYTHONNOUSERSITE".into(), "1".into());
    values.insert("RUSTUP_AUTO_INSTALL".into(), "0".into());
    values.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
    values.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
    let mut command = request.command.clone();
    let program = command
        .first()
        .and_then(|s| Path::new(s).file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let profile = crate::toolchain::real_profile()?;
    let rust_command = matches!(program.as_str(), "cargo" | "rustc" | "rustdoc");
    let selected_executable = command.first().and_then(|name| locate(name));
    let shim = selected_executable
        .as_ref()
        .and_then(|p| p.parent())
        .is_some_and(|parent| {
            std::fs::canonicalize(parent).ok()
                == std::fs::canonicalize(profile.join(".cargo/bin")).ok()
        });
    let explicit = rust_command && shim && command.get(1).is_some_and(|s| s.starts_with('+'));
    let requested = explicit.then(|| command[1].trim_start_matches('+'));
    let selected_toolchain = if rust_command && !shim {
        Ok(selected_executable
            .as_ref()
            .and_then(|p| p.parent())
            .and_then(|bin| bin.parent())
            .filter(|root| {
                root.join("bin/rustc.exe").is_file() && root.join("bin/cargo.exe").is_file()
            })
            .map(Path::to_path_buf))
    } else {
        crate::toolchain::resolve(&profile, &request.workspace, requested)
    };
    match selected_toolchain {
        Ok(Some(toolchain)) => {
            let bin = toolchain.join("bin");
            reads.push(toolchain.clone());
            values.insert(
                "RUSTUP_TOOLCHAIN".into(),
                toolchain.to_string_lossy().into_owned(),
            );
            values.insert(
                "RUSTC".into(),
                bin.join("rustc.exe").to_string_lossy().into_owned(),
            );
            values.insert(
                "RUSTDOC".into(),
                bin.join("rustdoc.exe").to_string_lossy().into_owned(),
            );
            values.insert(
                "PATH".into(),
                format!(
                    "{};{}",
                    bin.display(),
                    values.get("PATH").cloned().unwrap_or_default()
                ),
            );
            if rust_command && shim {
                command[0] = bin
                    .join(format!("{program}.exe"))
                    .to_string_lossy()
                    .into_owned();
                if explicit {
                    command.remove(1);
                }
            }
        }
        Err(error) if matches!(program.as_str(), "cargo" | "rustc" | "rustdoc") => {
            return Err(error);
        }
        _ => {}
    }
    // Explicitly selected programs require their installation tree, not the
    // user's whole profile. Shared compiler/SDK paths remain read-only.
    crate::launcher::adapt(&mut command, &mut reads)?;
    for name in ["PATH", "LIB", "LIBPATH", "INCLUDE"] {
        if let Some(paths) = values.get(name) {
            reads.extend(std::env::split_paths(paths).filter(|p| p.is_absolute() && p.is_dir()));
        }
    }
    reads.push(request.workspace.clone());
    let mut denied = vec![request.home.join(".sandbox-secrets")];
    {
        let profile = crate::toolchain::real_profile()?;
        for path in [
            ".ssh",
            ".codex",
            ".hermes",
            ".config",
            ".aws",
            ".azure",
            ".kube",
            ".gnupg",
            ".npmrc",
            ".netrc",
            ".git-credentials",
            ".cargo/credentials",
            ".cargo/credentials.toml",
            "AppData/Local/DeepSeek Harness/.credentials.yaml",
            "AppData/Local/DeepSeek Harness/settings.json",
            "AppData/Local/DeepSeek Harness/sessions",
            "AppData/Local/DeepSeek Harness/storages",
            "AppData/Local/DeepSeek Harness/capabilities",
            "AppData/Local/DeepSeek Harness/memory",
        ] {
            let path = profile.join(path);
            if path.exists() {
                denied.push(path);
            }
        }
    }
    reads.sort();
    reads.dedup();
    Ok(Environment {
        values,
        reads,
        writes,
        denied,
        command,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excludes_credentials_and_runtime_injection_variables() {
        for key in [
            "OPENAI_API_KEY",
            "DEEPSEEK_API_KEY",
            "GITHUB_TOKEN",
            "NODE_OPTIONS",
            "PYTHONPATH",
            "RUSTC_WRAPPER",
            "GIT_SSH_COMMAND",
            "AWS_SECRET_ACCESS_KEY",
        ] {
            assert!(!allowed(key), "{key}");
        }
        assert!(allowed("Path"));
        assert!(allowed("LIB"));
    }
}
