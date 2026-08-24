//! Path-containment mechanics for the filesystem sandbox. Rust port of
//! `packages/fs/fs-sandbox/src/containment.ts`. Canonical spellings take the
//! fast lexical path; filesystem identity supplies the conservative fallback
//! for alias-equivalent roots such as Windows 8.3 names and casing.
//!
//! # Deviations
//!
//! - The TS identity comparison uses `dev:ino`; the Rust port compares the
//!   full Unix metadata identity pair on Unix and falls back to a
//!   canonicalize-equality check on Windows (no file index in std).

use std::path::Path;

fn comparable_path(path: &str, case_sensitive: bool) -> String {
    if case_sensitive {
        path.to_string()
    } else {
        path.to_lowercase()
    }
}

fn is_lexically_under(path: &str, root: &str, case_sensitive: bool) -> bool {
    let comparable_target = comparable_path(path, case_sensitive);
    let comparable_root = comparable_path(root, case_sensitive);
    if comparable_target == comparable_root {
        return true;
    }
    let separator = std::path::MAIN_SEPARATOR.to_string();
    let prefix = if comparable_root.ends_with(std::path::MAIN_SEPARATOR) {
        comparable_root
    } else {
        format!("{comparable_root}{separator}")
    };
    comparable_target.starts_with(&prefix)
}

#[allow(dead_code)] // Retained for platform-specific containment probes.
async fn stat_if_present(path: &str) -> Result<Option<std::fs::Metadata>, String> {
    match tokio::fs::metadata(path).await {
        Ok(info) => Ok(Some(info)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(unix)]
fn same_identity(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

/// Determine whether a canonical target is a writable root or lies beneath
/// it. The lexical fast path handles normal canonical spellings. When
/// spellings differ, walk the target's existing ancestors and compare
/// filesystem identity with the root; this recognizes Windows
/// long-name/8.3 aliases and casing without weakening containment to a
/// textual approximation.
pub async fn is_path_under(
    path: &str,
    root: &str,
    case_sensitive: Option<bool>,
) -> Result<bool, String> {
    let case_sensitive = case_sensitive.unwrap_or(cfg!(not(windows)));
    if is_lexically_under(path, root, case_sensitive) {
        return Ok(true);
    }
    #[cfg(unix)]
    let root_info = {
        let Some(root_info) = stat_if_present(root).await? else {
            return Ok(false);
        };
        root_info
    };
    #[cfg(windows)]
    let canonical_root = tokio::fs::canonicalize(root).await.ok();
    let mut ancestor = path.to_string();
    loop {
        #[cfg(unix)]
        {
            if let Some(ancestor_info) = stat_if_present(&ancestor).await? {
                if same_identity(&ancestor_info, &root_info) {
                    return Ok(true);
                }
            }
        }
        #[cfg(windows)]
        {
            // Windows std exposes no file index; canonicalize equality is
            // the reliable alias identity (8.3 names and casing resolve to
            // one spelling).
            if let Ok(ancestor_canonical) = tokio::fs::canonicalize(&ancestor).await
                && canonical_root.as_ref() == Some(&ancestor_canonical)
            {
                return Ok(true);
            }
        }
        let parent = Path::new(&ancestor)
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
            .unwrap_or_else(|| ancestor.clone());
        if parent == ancestor {
            return Ok(false);
        }
        ancestor = parent;
    }
}
