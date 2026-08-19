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
            if let Some(ancestor_canonical) = tokio::fs::canonicalize(&ancestor).await.ok() {
                if canonical_root.as_ref() == Some(&ancestor_canonical) {
                    return Ok(true);
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "dsh-fssbx-containment-rs-{}-{}",
            name,
            std::process::id()
        ));
        std::fs::create_dir_all(&base).expect("root");
        base
    }

    #[tokio::test(flavor = "current_thread")]
    async fn accepts_equal_paths_descendants_and_a_filesystem_root_boundary() {
        let base = temp_root("equal");
        assert!(
            is_path_under(base.to_str().unwrap(), base.to_str().unwrap(), None)
                .await
                .expect("under")
        );
        let child = base.join("child").to_string_lossy().into_owned();
        assert!(
            is_path_under(&child, base.to_str().unwrap(), None)
                .await
                .expect("under")
        );
        let root = Path::new(base.to_str().unwrap())
            .ancestors()
            .last()
            .expect("fs root");
        assert!(
            is_path_under(base.to_str().unwrap(), root.to_str().unwrap(), None)
                .await
                .expect("under")
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn uses_case_insensitive_lexical_comparison_for_windows_style_containment() {
        let base = temp_root("case");
        let upper = base.join("child").to_string_lossy().to_uppercase();
        let lower = base.to_string_lossy().to_lowercase();
        assert!(
            is_path_under(&upper, &lower, Some(false))
                .await
                .expect("under")
        );
        assert!(
            is_path_under(
                &base.join("case-sensitive-child").to_string_lossy(),
                &base.to_string_lossy(),
                Some(true),
            )
            .await
            .expect("under")
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn recognizes_an_alias_equivalent_root_by_filesystem_identity_for_a_missing_target() {
        let base = temp_root("alias");
        let real_root = base.join("real");
        std::fs::create_dir_all(&real_root).expect("real");
        let alias_root = base.join("alias");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&real_root, &alias_root).expect("symlink");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_root, &alias_root).expect("symlink");
        let canonical = std::fs::canonicalize(&real_root).expect("canonical");
        let missing = canonical
            .join("missing")
            .join("file.txt")
            .to_string_lossy()
            .into_owned();
        assert!(
            is_path_under(&missing, alias_root.to_str().unwrap(), None)
                .await
                .expect("under")
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn denies_unrelated_and_missing_roots() {
        let base = temp_root("deny");
        let allowed = base.join("allowed");
        let outside = base.join("outside");
        std::fs::create_dir_all(&allowed).expect("allowed");
        std::fs::create_dir_all(&outside).expect("outside");
        let target = outside.join("file.txt").to_string_lossy().into_owned();
        assert!(
            !is_path_under(&target, allowed.to_str().unwrap(), None)
                .await
                .expect("under")
        );
        assert!(
            !is_path_under(&target, &base.join("missing-root").to_string_lossy(), None)
                .await
                .expect("under")
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn treats_a_regular_file_path_segment_as_a_missing_target_not_containment() {
        let base = temp_root("file-seg");
        let allowed = base.join("allowed");
        std::fs::create_dir_all(&allowed).expect("allowed");
        let blocker = base.join("blocker");
        std::fs::write(&blocker, "not a directory").expect("blocker");
        let target = blocker.join("child.txt").to_string_lossy().into_owned();
        assert!(
            !is_path_under(&target, allowed.to_str().unwrap(), None)
                .await
                .expect("under")
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
