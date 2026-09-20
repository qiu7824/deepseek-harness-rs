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
