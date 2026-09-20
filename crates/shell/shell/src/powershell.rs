//! Shared host discovery. A located executable does not imply sandbox access.
use std::path::{Path, PathBuf};

fn candidates(
    program_files: Option<&Path>,
    paths: &[PathBuf],
    system_root: Option<&Path>,
) -> Vec<PathBuf> {
    let mut result = Vec::new();
    if let Some(root) = program_files.filter(|root| root.is_absolute()) {
        result.push(root.join("PowerShell/7/pwsh.exe"));
    }
    result.extend(
        paths
            .iter()
            .filter(|path| path.is_absolute())
            .take(256)
            .map(|path| path.join("pwsh.exe")),
    );
    if let Some(root) = system_root.filter(|root| root.is_absolute()) {
        result.push(root.join("System32/WindowsPowerShell/v1.0/powershell.exe"));
    }
    result
}

pub fn locate_powershell() -> Option<String> {
    let paths: Vec<_> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    let choices = if cfg!(windows) {
        candidates(
            std::env::var_os("ProgramFiles")
                .map(PathBuf::from)
                .as_deref(),
            &paths,
            std::env::var_os("SystemRoot").map(PathBuf::from).as_deref(),
        )
    } else {
        paths
            .into_iter()
            .filter(|path| path.is_absolute())
            .take(256)
            .map(|path| path.join("pwsh"))
            .collect()
    };
    choices
        .into_iter()
        .find(|path| {
            let normalized = path
                .to_string_lossy()
                .replace('\\', "/")
                .to_ascii_lowercase();
            // Execution aliases cannot be validated like ordinary installed binaries.
            !normalized.contains("/microsoft/windowsapps/") && path.is_file()
        })
        .map(|path| path.to_string_lossy().into_owned())
}

/// Windows PowerShell 5 cannot reliably read the user's policy registry from
/// AppContainer. Carry the existing effective host policy, never invent Bypass.
/// Only the protected system executable is queried; custom executables must not
/// be launched on the host as an incidental preflight.
pub fn system_execution_policy(executable: &str) -> Option<String> {
    #[cfg(windows)]
    {
        use std::{
            os::windows::process::CommandExt,
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        let system = PathBuf::from(std::env::var_os("SystemRoot")?)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let system = std::fs::canonicalize(system).ok()?;
        if std::fs::canonicalize(executable).ok()? != system {
            return None;
        }
        static CACHE: std::sync::Mutex<Option<(Instant, String)>> = std::sync::Mutex::new(None);
        let mut cache = CACHE.lock().ok()?;
        if let Some((at, value)) = cache
            .as_ref()
            .filter(|(at, _)| at.elapsed() < Duration::from_secs(60))
        {
            let _ = at;
            return Some(value.clone());
        }
        let mut command = Command::new(&system);
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Microsoft.PowerShell.Security\\Get-ExecutionPolicy",
            ])
            .env("PSModulePath", system.parent()?.join("Modules"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(0x08000000);
        let mut child = command.spawn().ok()?;
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(Some(_)) => return None,
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                _ if started.elapsed() > Duration::from_secs(3) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                _ => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        let output = child.wait_with_output().ok()?;
        let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
        if !matches!(
            value.as_str(),
            "Restricted" | "AllSigned" | "RemoteSigned" | "Unrestricted" | "Bypass"
        ) {
            return None;
        }
        *cache = Some((Instant::now(), value.clone()));
        Some(value)
    }
    #[cfg(not(windows))]
    {
        let _ = executable;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modern_install_precedes_path_and_legacy_without_relative_candidates() {
        let root = std::env::temp_dir();
        let paths = [PathBuf::from("relative"), root.join("custom")];
        let items = candidates(Some(&root), &paths, Some(&root));
        assert_eq!(
            items,
            vec![
                root.join("PowerShell/7/pwsh.exe"),
                root.join("custom/pwsh.exe"),
                root.join("System32/WindowsPowerShell/v1.0/powershell.exe")
            ]
        );
    }
}
