//! Path canonicalization for workspace identity. Rust port of
//! `packages/workspace/workspace/src/paths.ts`.

/// Canonicalize a directory path via `fs.realpath` (TS
/// `realpathNormalize`): trailing slashes, `..` segments, and symlinks are
/// all resolved. This is the ONE uniqueness canon of the package. A path
/// that does not exist rejects with the original error.
pub async fn realpath_normalize(path: &str) -> std::io::Result<String> {
    Ok(tokio::fs::canonicalize(path)
        .await?
        .to_string_lossy()
        .to_string())
}

#[cfg(all(test, windows))]
mod tests {
    use super::realpath_normalize;

    #[tokio::test]
    async fn realpath_preserves_unicode_workspace_names() {
        let root = std::env::temp_dir().join(format!(
            "dsh-workspace-unicode-工程-😀-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create unicode workspace");

        let normalized = realpath_normalize(root.to_str().expect("Windows path is Unicode"))
            .await
            .expect("normalize unicode workspace");

        assert!(
            normalized.contains("工程-😀"),
            "normalized path: {normalized}"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
