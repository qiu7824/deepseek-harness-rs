use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionFile {
    pub absolute_path: PathBuf,
    pub display_path: String,
    pub content: String,
}

fn is_file(path: &Path, max_source_bytes: u64) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.len() <= max_source_bytes)
        .unwrap_or(false)
}

fn project_root(cwd: &Path) -> PathBuf {
    let mut current = cwd.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return current;
        }
        let Some(parent) = current.parent() else {
            return cwd.to_path_buf();
        };
        current = parent.to_path_buf();
    }
}

fn push_candidate(
    files: &mut Vec<InstructionFile>,
    seen: &mut std::collections::HashSet<PathBuf>,
    path: PathBuf,
    display_path: String,
    max_source_bytes: u64,
) {
    let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    if !seen.insert(canonical) || !is_file(&path, max_source_bytes) {
        return;
    }
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };
    files.push(InstructionFile {
        absolute_path: path,
        display_path,
        content,
    });
}

pub fn discover(cwd: &Path, dsh_home: &Path, max_source_bytes: u64) -> Vec<InstructionFile> {
    let mut files = Vec::new();
    let mut seen = std::collections::HashSet::new();
    push_candidate(
        &mut files,
        &mut seen,
        dsh_home.join("AGENTS.md"),
        "$DSH_HOME/AGENTS.md".to_string(),
        max_source_bytes,
    );
    let root = project_root(cwd);
    let mut directories = Vec::new();
    let mut current = cwd.to_path_buf();
    loop {
        directories.push(current.clone());
        if current == root {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent.to_path_buf();
    }
    directories.reverse();
    for directory in directories {
        let relative = directory.strip_prefix(&root).unwrap_or(&directory);
        for name in [
            "AGENTS.md",
            "CLAUDE.md",
            "AGENTS.local.md",
            "CLAUDE.local.md",
        ] {
            let display = relative.join(name).to_string_lossy().into_owned();
            push_candidate(
                &mut files,
                &mut seen,
                directory.join(name),
                display,
                max_source_bytes,
            );
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_global_then_root_to_leaf_candidates() {
        let root = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        let leaf = root.path().join("pkg");
        std::fs::create_dir(&leaf).unwrap();
        std::fs::write(home.path().join("AGENTS.md"), "global").unwrap();
        std::fs::write(root.path().join("AGENTS.md"), "root").unwrap();
        std::fs::write(leaf.join("CLAUDE.md"), "leaf").unwrap();
        let files = discover(&leaf, home.path(), 1_048_576);
        assert_eq!(
            files
                .iter()
                .map(|file| file.display_path.as_str())
                .collect::<Vec<_>>(),
            ["$DSH_HOME/AGENTS.md", "AGENTS.md", "pkg\\CLAUDE.md"]
        );
    }
}
