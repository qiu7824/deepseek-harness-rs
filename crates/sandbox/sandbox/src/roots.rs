//! The writable-root derivation shared by every enforcement dialect that
//! expresses a mode as a canonical allow-list: `workspace-write` means "the
//! workspace root plus the platform temp areas", and this module is that
//! meaning's one home. Rust port of
//! `packages/sandbox/sandbox/src/roots.ts`. The Seatbelt profile
//! (`dsh-sandbox-local`) and the in-process filesystem fence
//! (`dsh-fs-sandbox`) both derive their allow-list here, so "the write tool
//! cannot write /tmp but bash can" asymmetries cannot arise between them.

use crate::index::SandboxExecutionPolicy;

/// Resolve a granted root to the path the enforcement layer actually
/// compares: canonical (symlinks resolved), because both Seatbelt filters
/// and the fs fence's containment check match resolved paths — `/tmp` IS
/// `/private/tmp` on darwin, and an as-spelled grant would match nothing.
///
/// Returns the canonical path, or the spelling as-is when resolution fails
/// (a missing root matches nothing until it exists — the conservative
/// outcome; inventing a fallback would grant a path the caller never named).
pub fn canonical_path(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|resolved| resolved.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string())
}

/// The roots one confined execution may WRITE under — the mode's meaning as
/// a canonical, deduplicated allow-list. `read-only` allows nothing;
/// `workspace-write` allows the policy's workspace root, the host `/tmp`, and
/// the per-user platform temp dir (`env::temp_dir()` — the real temp area
/// for mkstemp-family tools; omitting it would deny what the mode promises).
pub fn writable_roots(policy: &SandboxExecutionPolicy) -> Vec<String> {
    if policy.mode.as_str() != "workspace-write" {
        return Vec::new();
    }
    let candidates = [
        policy.workspace_root.clone(),
        "/tmp".to_string(),
        std::env::temp_dir().to_string_lossy().into_owned(),
    ];
    let mut roots: Vec<String> = Vec::new();
    for candidate in candidates {
        let canonical = canonical_path(&candidate);
        if !roots.contains(&canonical) {
            roots.push(canonical);
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{SandboxExecutionPolicy, SandboxMode};

    #[test]
    fn canonical_path_resolves_symlinks_and_keeps_missing_spellings() {
        let dir = std::env::temp_dir().join(format!("dsh-roots-rs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create");
        let resolved = canonical_path(&dir.to_string_lossy());
        assert_eq!(resolved, std::fs::canonicalize(&dir).expect("canonical").to_string_lossy());
        assert_eq!(
            canonical_path("/does/not/exist/anywhere-xyz"),
            "/does/not/exist/anywhere-xyz"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writable_roots_follow_the_mode() {
        let read_only = SandboxExecutionPolicy {
            mode: SandboxMode::ReadOnly,
            workspace_root: std::env::current_dir().expect("cwd").to_string_lossy().into_owned(),
            session_id: None,
        };
        assert!(writable_roots(&read_only).is_empty());

        let ws = std::env::temp_dir().join(format!("dsh-ws-rs-{}", std::process::id()));
        std::fs::create_dir_all(&ws).expect("create");
        let workspace_write = SandboxExecutionPolicy {
            mode: SandboxMode::WorkspaceWrite,
            workspace_root: ws.to_string_lossy().into_owned(),
            session_id: None,
        };
        let roots = writable_roots(&workspace_write);
        assert!(roots.contains(&std::fs::canonicalize(&ws).expect("canonical").to_string_lossy().into_owned()));
        assert!(roots.contains(&canonical_path("/tmp")));
        assert!(roots.contains(
            &std::fs::canonicalize(std::env::temp_dir())
                .expect("canonical")
                .to_string_lossy()
                .into_owned()
        ));
        // Deduplicated after canonicalization (/tmp and the platform temp
        // dir may coincide).
        let unique: std::collections::HashSet<&String> = roots.iter().collect();
        assert_eq!(unique.len(), roots.len());
        let _ = std::fs::remove_dir_all(&ws);
    }
}
