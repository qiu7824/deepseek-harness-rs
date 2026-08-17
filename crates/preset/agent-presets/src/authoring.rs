//! Copying, reading, and deleting locally authored presets.
//! Rust port of `src/authoring.ts`.
//!
//! Authoring is confined to a `user` root: the shipped `.system` set is part
//! of the deployment. The only authoring write is a whole-directory copy of
//! an existing preset.

use std::path::{Path, PathBuf};

use dsh_home_paths::expand_home_path;

use crate::metadata::{METADATA_FILE, PresetMetadata, render_preset_metadata};
use crate::preset::{AgentPreset, PresetRoot, preset_id_ok};

/// A preset id that cannot be used as a directory name under a root.
#[derive(Debug, thiserror::Error)]
#[error(
    "agent-presets: preset id {preset_id:?} must match ^[a-z0-9][a-z0-9-]*$ — the id is a directory name, so anything else could escape the preset root"
)]
pub struct InvalidPresetIdError {
    /// The rejected id.
    pub preset_id: String,
}

/// A copy target that is already occupied — a copy never overwrites.
#[derive(Debug, thiserror::Error)]
#[error(
    "agent-presets: preset \"{preset_id}\" already exists — a copy never overwrites; delete the existing preset first or choose another id"
)]
pub struct PresetExistsError {
    /// The id that is already taken.
    pub preset_id: String,
}

/// Authoring was attempted where the deployment allows none.
#[derive(Debug, thiserror::Error)]
#[error("agent-presets: preset \"{preset_id}\" cannot be written: {reason}")]
pub struct PresetNotWritableError {
    /// What the caller tried to change, for the diagnostic.
    pub preset_id: String,
    /// Why it is not writable.
    pub reason: String,
}

/// Every authoring failure, keeping the three TS error classes distinct.
#[derive(Debug, thiserror::Error)]
pub enum AuthoringError {
    #[error(transparent)]
    InvalidId(#[from] InvalidPresetIdError),
    #[error(transparent)]
    Exists(#[from] PresetExistsError),
    #[error(transparent)]
    NotWritable(#[from] PresetNotWritableError),
    /// I/O failure inside the copy itself (the TS promise rejection path).
    #[error("{0}")]
    Io(String),
}

/// Convert a root path to an absolute scan/write directory
/// (TS `resolve(expandHomePath(path))`).
pub fn absolute_root_dir(root_path: &str) -> PathBuf {
    let expanded = expand_home_path(root_path);
    if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(expanded)
    }
}

/// The root locally authored presets are written to: the absolute path of
/// the first `user` root (TS `writableRoot`).
pub fn writable_root(roots: &[PresetRoot]) -> Result<PathBuf, PresetNotWritableError> {
    let root = roots
        .iter()
        .find(|candidate| candidate.trust == crate::preset::PresetTrust::User)
        .ok_or_else(|| PresetNotWritableError {
            preset_id: String::new(),
            reason: "this deployment configures no user-writable preset root".to_string(),
        })?;
    Ok(absolute_root_dir(&root.path))
}

/// Read one preset's composition text (TS `readComposition`).
pub async fn read_composition(preset: &AgentPreset) -> Result<String, String> {
    tokio::fs::read_to_string(&preset.path)
        .await
        .map_err(|error| {
            format!(
                "agent-presets: cannot read preset \"{}\": {error}",
                preset.id
            )
        })
}

/// Whether anything occupies the path (TS `occupied`).
async fn occupied(path: &Path) -> bool {
    tokio::fs::metadata(path).await.is_ok()
}

/// Copy one directory tree recursively, dereferencing symlinks, refusing to
/// overwrite (TS `cp` with `dereference: true, force: false,
/// errorOnExist: true`).
async fn copy_tree_deref(source: &Path, target: &Path) -> Result<(), String> {
    let mut children = tokio::fs::read_dir(source)
        .await
        .map_err(|error| format!("cannot copy {}: {error}", source.display()))?;
    while let Some(child) = children
        .next_entry()
        .await
        .map_err(|error| format!("cannot copy {}: {error}", source.display()))?
    {
        let file_type = match child.file_type().await {
            Ok(file_type) => file_type,
            Err(error) => return Err(format!("cannot copy {}: {error}", source.display())),
        };
        let name = child.file_name();
        let from = child.path();
        let to = target.join(&name);
        if file_type.is_symlink() {
            // Dereference: copy the target's content, not the link.
            let resolved = tokio::fs::read_link(&from)
                .await
                .map_err(|error| format!("cannot copy {}: {error}", from.display()))?;
            let meta = tokio::fs::metadata(&resolved)
                .await
                .map_err(|error| format!("cannot copy {}: {error}", from.display()))?;
            if meta.is_dir() {
                tokio::fs::create_dir(&to)
                    .await
                    .map_err(|error| format!("cannot copy {}: {error}", to.display()))?;
                Box::pin(copy_tree_deref(&resolved, &to)).await?;
            } else {
                tokio::fs::copy(&resolved, &to)
                    .await
                    .map_err(|error| format!("cannot copy {}: {error}", to.display()))?;
            }
        } else if file_type.is_dir() {
            tokio::fs::create_dir(&to)
                .await
                .map_err(|error| format!("cannot copy {}: {error}", to.display()))?;
            Box::pin(copy_tree_deref(&from, &to)).await?;
        } else {
            tokio::fs::copy(&from, &to)
                .await
                .map_err(|error| format!("cannot copy {}: {error}", to.display()))?;
        }
    }
    Ok(())
}

/// Re-tighten a copied tree to owner-only (TS `tightenModes`). A shipped
/// preset is world-readable in its install; the copy carries the same weight
/// as the settings document beside it. A file's owner-execute bit survives.
///
/// Deviation: `chmod` has no owner-mode semantics on Windows; the
/// tightening runs only on Unix (matching the TS comment that Windows
/// exposes no POSIX owner-execute bit).
#[cfg(unix)]
async fn tighten_modes(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .await
        .map_err(|error| format!("cannot tighten {}: {error}", dir.display()))?;
    let mut children = tokio::fs::read_dir(dir)
        .await
        .map_err(|error| format!("cannot tighten {}: {error}", dir.display()))?;
    while let Some(entry) = children
        .next_entry()
        .await
        .map_err(|error| format!("cannot tighten {}: {error}", dir.display()))?
    {
        let path = entry.path();
        let file_type = match entry.file_type().await {
            Ok(file_type) => file_type,
            Err(error) => {
                return Err(format!("cannot tighten {}: {error}", path.display()));
            }
        };
        if file_type.is_dir() {
            Box::pin(tighten_modes(&path)).await?;
        } else {
            let mode = tokio::fs::metadata(&path)
                .await
                .map_err(|error| format!("cannot tighten {}: {error}", path.display()))?
                .permissions()
                .mode();
            let target = if mode & 0o100 == 0 { 0o600 } else { 0o700 };
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(target))
                .await
                .map_err(|error| format!("cannot tighten {}: {error}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
async fn tighten_modes(_dir: &Path) -> Result<(), String> {
    Ok(())
}

/// Create a preset by copying an existing one's whole directory
/// (TS `copyComposition`). The copy carries everything the source directory
/// holds — composition, metadata, skill directories, assets. The copied
/// metadata is then rewritten: the source's description is kept, but its
/// name and roster `order` are not. A failed copy leaves nothing.
pub async fn copy_composition(
    roots: &[PresetRoot],
    source: &AgentPreset,
    id: &str,
    name: Option<&str>,
) -> Result<String, AuthoringError> {
    if !preset_id_ok(id) {
        return Err(InvalidPresetIdError {
            preset_id: id.to_string(),
        }
        .into());
    }
    let root = writable_root(roots)?;
    let dir = root.join(id);
    // The roster check upstream only sees discovered presets; a directory
    // with no composition file still occupies the name.
    if occupied(&dir).await {
        return Err(PresetExistsError {
            preset_id: id.to_string(),
        }
        .into());
    }
    let source_dir = Path::new(&source.path)
        .parent()
        .ok_or_else(|| {
            AuthoringError::Io("source preset path has no parent directory".to_string())
        })?
        .to_path_buf();
    let result = (async {
        tokio::fs::create_dir(&dir)
            .await
            .map_err(|error| format!("cannot copy preset: {error}"))?;
        copy_tree_deref(&source_dir, &dir).await?;
        tighten_modes(&dir).await?;
        let rendered = render_preset_metadata(&PresetMetadata {
            name: name.map(str::to_string),
            description: source.description.clone(),
            order: None,
        });
        let metadata_path = dir.join(METADATA_FILE);
        match rendered {
            None => {
                let _ = tokio::fs::remove_file(&metadata_path).await;
            }
            Some(rendered) => {
                dsh_atomic_write::write_file_atomic(
                    &metadata_path,
                    rendered.as_bytes(),
                    dsh_atomic_write::WriteFileAtomicOptions {
                        mode: 0o600,
                        dir_mode: Some(0o700),
                    },
                )
                .await
                .map_err(|error| format!("cannot copy preset: {error}"))?;
            }
        }
        Ok::<(), String>(())
    })
    .await;
    if result.is_err() {
        // A half-copied directory would be invisible to discovery at best;
        // a failed copy leaves nothing.
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
    result
        .map(|()| dir.to_string_lossy().to_string())
        .map_err(AuthoringError::Io)
}

/// Delete a locally authored preset (TS `deleteComposition`). A shipped
/// preset is refused. A preset a live session mounted is NOT refused — the
/// composition was read at creation and is never re-read.
pub async fn delete_composition(
    roots: &[PresetRoot],
    preset: &AgentPreset,
) -> Result<(), PresetNotWritableError> {
    if preset.trust != crate::preset::PresetTrust::User {
        return Err(PresetNotWritableError {
            preset_id: preset.id.clone(),
            reason: "it ships with the deployment".to_string(),
        });
    }
    let root = writable_root(roots)?;
    let dir = root.join(&preset.id);
    // Belt and braces over the id pattern: the resolved directory must still
    // be the one the writable root owns, whatever discovery reported.
    let path = Path::new(&preset.path);
    if !path.is_absolute() || !path.starts_with(&dir) {
        return Err(PresetNotWritableError {
            preset_id: preset.id.clone(),
            reason: "it does not live under the writable preset root".to_string(),
        });
    }
    tokio::fs::remove_dir_all(&dir)
        .await
        .map_err(|error| PresetNotWritableError {
            preset_id: preset.id.clone(),
            reason: format!("{error}"),
        })
}
