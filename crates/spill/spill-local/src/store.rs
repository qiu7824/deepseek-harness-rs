//! Cordis-free storage mechanics for the local spill backend: private
//! session-scoped directory selection, safe-name derivation, path-traversal
//! protection, and the exclusive owner-only write. Rust port of
//! `packages/spill/spill-local/src/store.ts` — kept out of the service class
//! (like `dsh-bash-local`'s `run.ts`) so the filesystem behavior is
//! unit-testable without a `ctx` and without the OS temp dir.
//!
//! # Deviations
//!
//! - The random filename prefix (TS `randomBytes(6).toString('hex')`) is the
//!   first 12 hex digits of a v4 UUID — 48 cryptographically random bits,
//!   byte-for-byte equivalent entropy and spelling.
//! - POSIX permission bits (`0700` dir, `0600` file) are applied under
//!   `#[cfg(unix)]` only, mirroring the TS spec's `win32` skip.

use std::sync::OnceLock;

use sha2::{Digest, Sha256};

/// The default spill root: a private (0700) per-process directory under the
/// OS tmpdir, created lazily. Predictable world-readable paths would let
/// other local users read spilled tool output or pre-create symlinks;
/// an unpredictable suffix gives the `mkdtemp` equivalent.
pub fn private_root() -> &'static str {
    static DEFAULT_ROOT: OnceLock<String> = OnceLock::new();
    DEFAULT_ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!(
            "dsh-spill-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        ));
        std::fs::create_dir(&root)
            .unwrap_or_else(|error| panic!("cannot create private spill root '{root:?}': {error}"));
        root.to_string_lossy().into_owned()
    })
}

/// Encode an arbitrary string as one safe path segment, injectively over ALL
/// JS (UTF-16) strings — the TS `encodeSegment`. Each UTF-16 code unit is
/// kept literal (`[A-Za-z0-9._-]`, minus `~`) or escaped as `~XXXX`; `~` is
/// itself escaped, so the mapping is reversible and distinct inputs never
/// collide. The whole-segment tokens `.`/`..` are escaped so they can never
/// traverse. An empty string encodes to `~` (never an empty segment).
pub fn encode_segment(raw: &str) -> String {
    if raw.is_empty() {
        return "~".to_string();
    }
    if raw == "." {
        return "~002E".to_string();
    }
    if raw == ".." {
        return "~002E~002E".to_string();
    }
    let mut out = String::new();
    for unit in raw.encode_utf16() {
        let ch = char::from_u32(u32::from(unit)).expect("utf-16 unit");
        if ch != '~' && (ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')) {
            out.push(ch);
        } else {
            out.push('~');
            out.push_str(&format!("{unit:04X}"));
        }
    }
    out
}

/// The session-scoped directory: `<root>/session-<hash(sessionId)>`, a short
/// stable sha256 hash (the TS `sessionDir`).
pub fn session_dir(root: &str, session_id: &str) -> String {
    let hash = hex_prefix(&Sha256::digest(session_id.as_bytes()), 12);
    std::path::Path::new(root)
        .join(format!("session-{hash}"))
        .to_string_lossy()
        .into_owned()
}

fn hex_prefix(bytes: &[u8], chars: usize) -> String {
    let mut out = String::with_capacity(chars);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
        if out.len() >= chars {
            break;
        }
    }
    out.truncate(chars);
    out
}

/// Options for [`save_text_file`] — the resolved root and the request fields
/// the store needs (TS `SaveTextOptions`).
#[derive(Debug, Clone)]
pub struct SaveTextOptions {
    /// The spill root directory (configured or the lazy private default).
    pub root: String,
    /// The owning session id (scopes the directory).
    pub session_id: String,
    /// Caller-suggested base name; sanitized to one safe segment before use.
    pub suggested_name: String,
    /// The full text to persist.
    pub content: String,
}

/// A written spill file (TS `SavedText`).
#[derive(Debug, Clone, PartialEq)]
pub struct SavedText {
    pub path: String,
    pub bytes: u64,
}

#[cfg(unix)]
fn set_dir_mode(dir: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

#[cfg(unix)]
fn set_file_mode(file: &tokio::fs::File) {
    use std::os::unix::fs::PermissionsExt;
    let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600)).await;
}

/// Write `content` to a fresh file under the session-scoped directory and
/// return its path + byte length. The filename is a random hex prefix plus
/// the sanitized `suggested_name`, so it is unpredictable (defeats symlink
/// planting in a shared root) AND stays readable. The open is exclusive +
/// owner-only (TS `'wx', 0o600`): it fails on any existing path — symlink or
/// not — so a pre-planted target cannot redirect the write.
pub async fn save_text_file(options: SaveTextOptions) -> Result<SavedText, std::io::Error> {
    let dir = session_dir(&options.root, &options.session_id);
    tokio::fs::create_dir_all(&dir).await?;
    #[cfg(unix)]
    set_dir_mode(std::path::Path::new(&dir));
    let safe_name = encode_segment(&options.suggested_name);
    let prefix = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    let path = std::path::Path::new(&dir).join(format!("{prefix}-{safe_name}"));
    let bytes = options.content.len() as u64;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await?;
    #[cfg(unix)]
    set_file_mode(&file);
    tokio::io::AsyncWriteExt::write_all(&mut file, options.content.as_bytes()).await?;
    // Flush before returning: the caller reads the artifact back (and other
    // processes may list it) as soon as `save_text` resolves.
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    Ok(SavedText { path: path.to_string_lossy().into_owned(), bytes })
}
