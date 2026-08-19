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
