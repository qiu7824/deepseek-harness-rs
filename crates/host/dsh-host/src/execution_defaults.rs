//! Durable defaults for automatically located host runtimes.
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Default, Deserialize, Serialize)]
struct File {
    version: u32,
    paths: BTreeMap<String, String>,
}
pub(super) struct Defaults {
    path: PathBuf,
    state: Mutex<File>,
}
impl Defaults {
    pub fn load(root: &Path) -> Self {
        let path = root.join("execution-resolved-defaults-v1.json");
        let state = std::fs::metadata(&path)
            .ok()
            .filter(|m| m.len() <= 128 * 1024)
            .and_then(|_| std::fs::read(&path).ok())
            .and_then(|bytes| serde_json::from_slice::<File>(&bytes).ok())
            .filter(|file| file.version == 1 && file.paths.len() <= 32)
            .unwrap_or_default();
        Self {
            path,
            state: Mutex::new(state),
        }
    }
    pub fn select(
        &self,
        name: &str,
        locate: impl FnOnce() -> Option<String>,
    ) -> Result<Option<String>, String> {
        let mut state = self.state.lock();
        if let Some(path) = state
            .paths
            .get(name)
            .filter(|path| dsh_shell::powershell::automatic_candidate_allowed(Path::new(path)))
        {
            return Ok(Some(path.clone()));
        }
        let selected = locate().filter(|path| {
            Path::new(path).is_file()
                && dsh_shell::powershell::automatic_candidate_allowed(Path::new(path))
        });
        if selected.is_none() && !state.paths.contains_key(name) {
            return Ok(None);
        }
        let mut next = state.clone();
        if let Some(path) = &selected {
            next.paths.insert(name.into(), path.clone());
        } else {
            next.paths.remove(name);
        }
        next.version = 1;
        std::fs::create_dir_all(self.path.parent().unwrap()).map_err(|e| e.to_string())?;
        let temp = self
            .path
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let save = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(&serde_json::to_vec(&next)?)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temp, &self.path)
        })();
        if let Err(error) = save {
            let _ = std::fs::remove_file(&temp);
            return Err(format!("无法持久保存自动运行程序选择：{error}"));
        }
        *state = next;
        Ok(selected)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_and_path_reordering_preserve_the_first_selection() {
        let root = std::env::temp_dir().join(format!("dsh-auto-defaults-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first/pwsh.exe");
        let later = root.join("later/pwsh.exe");
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::create_dir_all(later.parent().unwrap()).unwrap();
        std::fs::write(&first, "first").unwrap();
        std::fs::write(&later, "later").unwrap();
        let select = Defaults::load(&root);
        assert_eq!(
            select
                .select("powershell", || Some(first.to_string_lossy().into_owned()))
                .unwrap(),
            Some(first.to_string_lossy().into_owned())
        );
        std::fs::remove_file(&first).unwrap();
        let restored = Defaults::load(&root);
        assert_eq!(
            restored
                .select("powershell", || Some(later.to_string_lossy().into_owned()))
                .unwrap(),
            Some(first.to_string_lossy().into_owned()),
            "a missing pinned executable is not silently replaced either"
        );
        let foreign = root.join(".codex/cache/bash.exe");
        std::fs::create_dir_all(foreign.parent().unwrap()).unwrap();
        std::fs::write(&foreign, "foreign executable fixture").unwrap();
        assert!(
            restored
                .select("bash", || Some(foreign.to_string_lossy().into_owned()))
                .unwrap()
                .is_none()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
