//! Find an actual Python installation, skipping Windows App Execution Aliases.
use std::path::{Path, PathBuf};

fn select(
    candidates: impl IntoIterator<Item = PathBuf>,
    profile: Option<&Path>,
) -> Option<PathBuf> {
    candidates
        .into_iter()
        .filter(|root| root.is_absolute())
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .find_map(|root| {
            if profile
                .and_then(|path| std::fs::canonicalize(path).ok())
                .is_some_and(|path| path == root)
            {
                return None;
            }
            let executable = root.join("python.exe");
            let dll = std::fs::read_dir(&root)
                .ok()?
                .filter_map(Result::ok)
                .any(|entry| {
                    let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                    name.starts_with("python") && name.ends_with(".dll") && entry.path().is_file()
                });
            (executable.is_file() && root.join("Lib").join("os.py").is_file() && dll).then(|| {
                PathBuf::from(crate::display_workspace_path(&executable.to_string_lossy()))
            })
        })
}
pub(super) fn command(environment: &Path) -> Option<PathBuf> {
    let mut candidates = vec![environment.join("python"), environment.to_path_buf()];
    candidates.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let profile = std::env::var_os("USERPROFILE").map(PathBuf::from);
    select(candidates, profile.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skips_aliases_and_profile_root_without_executing_any_candidate() {
        let root =
            std::env::temp_dir().join(format!("dsh-python-discovery-{}", uuid::Uuid::new_v4()));
        let alias = root.join("alias");
        let install = root.join("python");
        std::fs::create_dir_all(&alias).unwrap();
        std::fs::write(alias.join("python.exe"), b"alias").unwrap();
        std::fs::create_dir_all(install.join("Lib")).unwrap();
        for name in ["python.exe", "python312.dll", "Lib/os.py"] {
            std::fs::write(install.join(name), b"fixture").unwrap();
        }
        assert_eq!(
            select([alias.clone(), install.clone()], None),
            Some(PathBuf::from(crate::display_workspace_path(
                &std::fs::canonicalize(&install)
                    .unwrap()
                    .join("python.exe")
                    .to_string_lossy()
            )))
        );
        assert!(select([alias, install.clone()], Some(&install)).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
