use std::path::{Path, PathBuf};

/// PowerShell 7 loads its private .NET runtime beside pwsh.exe. Grant only a
/// recognized installation, never an arbitrary command's parent directory.
pub(crate) fn powershell_root(executable: &str) -> Option<PathBuf> {
    let executable = Path::new(executable);
    if !executable.is_absolute()
        || !executable.file_name()?.eq_ignore_ascii_case("pwsh.exe")
        || !executable.is_file()
    {
        return None;
    }
    let root = std::fs::canonicalize(executable)
        .ok()?
        .parent()?
        .to_path_buf();
    // Standard system installations already inherit AppContainer read access.
    // A normal user must not need to rewrite protected Program Files ACLs.
    if ["ProgramFiles", "ProgramFiles(x86)", "SystemRoot"]
        .iter()
        .any(|name| {
            std::env::var_os(name)
                .and_then(|path| std::fs::canonicalize(path).ok())
                .is_some_and(|system| root.starts_with(system))
        })
    {
        return None;
    }
    if safe_root(&root) && is_powershell(&root) {
        Some(root)
    } else {
        None
    }
}

pub(crate) fn safe_root(root: &Path) -> bool {
    root.parent().is_some()
        && !std::env::var_os("USERPROFILE")
            .and_then(|profile| std::fs::canonicalize(profile).ok())
            .is_some_and(|profile| profile == root)
}

pub(crate) fn is_powershell(root: &Path) -> bool {
    [
        "pwsh.exe",
        "pwsh.runtimeconfig.json",
        "System.Management.Automation.dll",
        "coreclr.dll",
    ]
    .iter()
    .all(|name| root.join(name).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_complete_runtime_but_not_arbitrary_executable_directories() {
        let root = std::env::temp_dir().join(format!(
            "dsh-runtime-layout-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let exe = root.join("pwsh.exe");
        std::fs::write(&exe, []).unwrap();
        assert!(powershell_root(exe.to_str().unwrap()).is_none());
        for name in [
            "pwsh.runtimeconfig.json",
            "System.Management.Automation.dll",
            "coreclr.dll",
        ] {
            std::fs::write(root.join(name), []).unwrap();
        }
        assert_eq!(
            powershell_root(exe.to_str().unwrap()),
            Some(std::fs::canonicalize(&root).unwrap())
        );
        assert!(powershell_root("pwsh.exe").is_none());
        assert!(powershell_root(root.join("other.exe").to_str().unwrap()).is_none());
        assert!(!safe_root(root.ancestors().last().unwrap()));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
