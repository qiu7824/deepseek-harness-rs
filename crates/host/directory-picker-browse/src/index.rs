//! `@deepseek-ai/dsh-host-directory-picker-browse` — Browse backend of the
//! directory-picker seam: registers `ctx.directoryPicker` with the `browse`
//! capability — one-level directory listing and child-directory creation
//! over the host filesystem. Nothing renders on the host display, so this
//! backend serves remote clients the dialog backend cannot. Policy decisions
//! (hidden entries flagged but returned, symlinks followed, whole-filesystem
//! scope) are recorded in the directory-picker seam Agent Note.
//!
//! # Deviations
//!
//! - Node's `opendir` handle is a `tokio::fs::ReadDir` iterator; dropping it
//!   closes the directory (RAII), collapsing TS's manual `close()` plus
//!   swallowed close-failure dance. The aborted scan still abandons the
//!   handle the moment the signal wins.
//! - TS `raceAbort` keeps the underlying operation running and swallows its
//!   late settlement; Rust `tokio::select!` drops the losing future at the
//!   same moment, which settles the abandoned read by cancellation instead
//!   of late-resolution. No caller-visible difference.
//! - Name ordering is Rust's bytewise `str` order. TS `localeCompare` is
//!   locale-aware (accents, numeric runs); the wire contract documents
//!   name-sorted rows and the test suite exercises ASCII names, which both
//!   orders sort identically.
//! - `dirent.file_type()` on Windows may cost one extra handle query per
//!   entry versus Node's `FindFirstFile`-backed dirents; the per-candidate
//!   work stays O(1) and the window bound keeps memory O(keep).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, arc, downcast};
use dsh_host_directory_picker::{
    AbortSignal, DirectoryEntry, DirectoryListing, DirectoryPicker,
    DirectoryPickerBrowseCapability, DirectoryPickerCapability, DirectoryPickerError,
    DirectoryPickerErrorCode, DirectoryPickerListError, register,
};
use futures::future::BoxFuture;

/// Cordis plugin name (the Rust static-registry equivalent of the TS package
/// entry name `@deepseek-ai/dsh-host-directory-picker-browse`).
pub const NAME: &str = "host-directory-picker-browse";

/// Validated plugin configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Complete-result bound of one listing level: at most this many
    /// child-directory rows (hidden rows included), with `truncated`
    /// flagging a cut level. The default follows GitHub's web UI, which
    /// truncates directory listings at 1,000 entries.
    pub max_entries: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self { max_entries: 1000 }
    }
}

impl Config {
    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "directory-picker-browse: config must be an object".to_string())?;
        let max_entries = match object.get("maxEntries") {
            None | Some(serde_json::Value::Null) => Config::default().max_entries,
            Some(serde_json::Value::Number(number)) => number
                .as_u64()
                .filter(|value| *value >= 1)
                .ok_or_else(|| {
                    "directory-picker-browse: maxEntries must be a natural number >= 1".to_string()
                })? as usize,
            Some(_) => {
                return Err(
                    "directory-picker-browse: maxEntries must be a natural number >= 1"
                        .to_string(),
                )
            }
        };
        Ok(Self { max_entries })
    }

    fn from_arcvalue(value: &ArcValue) -> Result<Self, String> {
        if let Some(config) = downcast::<Config>(value).cloned() {
            return Ok(config);
        }
        if let Some(raw) = downcast::<serde_json::Value>(value) {
            return Self::from_value(raw);
        }
        Err("directory-picker-browse: config is not an object".to_string())
    }
}

/// The host account's home directory (TS `homedir()`: `USERPROFILE` on
/// Windows, `HOME` elsewhere).
pub fn home_dir() -> String {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The current platform name in Node `process.platform` vocabulary.
pub fn platform() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

/// True when the path names one fixed filesystem location regardless of
/// process state: POSIX-absolute on POSIX; on Windows only drive-qualified
/// (`C:\…`) or complete UNC (`\\server\share…`) forms. Rooted drive-less
/// forms (`\foo`, `/foo`) and incomplete UNC prefixes (`\\`, `\\server`)
/// pass `isAbsolute` yet still resolve against the process's current drive.
///
/// `platform` replaces `process.platform` for deterministic tests.
pub fn fully_qualified(path: &str, platform: &str) -> bool {
    if platform == "win32" {
        win32_fully_qualified(path)
    } else {
        // POSIX `isAbsolute`: leading `/` regardless of the host platform
        // (std Path::is_absolute speaks host semantics and would misjudge
        // POSIX paths on Windows).
        path.starts_with('/')
    }
}

/// The Windows arm: `win32.isAbsolute(path)` AND the drive/UNC regex
/// `^(?:[A-Za-z]:[\\/]|[\\/]{2}[^\\/]+[\\/]+[^\\/]+)`, parsed without a
/// regex engine.
fn win32_fully_qualified(path: &str) -> bool {
    let bytes = path.as_bytes();
    // Drive-qualified: [A-Za-z]:[\\/]
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return true;
    }
    // Complete UNC: [\\/]{2} server [\\/]+ share — server and share are one
    // or more non-separator bytes each.
    if bytes.len() >= 2
        && (bytes[0] == b'\\' || bytes[0] == b'/')
        && bytes[1] == bytes[0]
    {
        let rest = &bytes[2..];
        let mut server = 0;
        while server < rest.len() && rest[server] != b'\\' && rest[server] != b'/' {
            server += 1;
        }
        if server == 0 {
            return false;
        }
        let mut separators = server;
        while separators < rest.len()
            && (rest[separators] == b'\\' || rest[separators] == b'/')
        {
            separators += 1;
        }
        if separators == server {
            return false;
        }
        let mut share = separators;
        while share < rest.len() && rest[share] != b'\\' && rest[share] != b'/' {
            share += 1;
        }
        return share > separators;
    }
    false
}

/// One streamed listing candidate: the dirent facts a row needs, nothing
/// else retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingCandidate {
    /// Base name within the streamed level.
    pub name: String,
    /// Dirent says directory (no probe needed).
    pub is_directory: bool,
    /// Dirent says symlink (enterability needs a stat probe).
    pub is_symbolic_link: bool,
}

/// Insert a streamed candidate into the name-sorted bounded window, evicting
/// the name-largest candidate when the window exceeds `keep`. Memory over an
/// arbitrarily large level therefore stays O(keep) regardless of how many
/// children the directory holds.
///
/// Returns true when an eviction happened (the level has candidates beyond
/// the window).
pub fn bounded_insert(
    window: &mut Vec<ListingCandidate>,
    candidate: ListingCandidate,
    keep: usize,
) -> bool {
    // Full window, name at or beyond the tail: one comparison rejects, so an
    // oversized level costs O(1) per candidate past the head instead of a
    // window scan.
    if window.len() == keep
        && candidate.name.as_str() >= window.last().expect("full window has a tail").name.as_str()
    {
        return true;
    }
    // Binary insertion keeps a retained candidate at O(log keep) comparisons.
    let index = window.partition_point(|entry| entry.name < candidate.name);
    window.insert(index, candidate);
    if window.len() <= keep {
        return false;
    }
    window.pop();
    true
}

/// Await `operation`, but resolve as aborted the moment the signal fires.
/// The losing operation is dropped (its handle closes) — the Rust
/// counterpart of TS `raceAbort`'s abandoned-read swallow.
pub async fn race_abort<T>(
    operation: impl std::future::Future<Output = T>,
    signal: &AbortSignal,
) -> Result<T, ()> {
    tokio::select! {
        biased;
        _ = signal.cancelled() => Err(()),
        value = operation => Ok(value),
    }
}

/// Ancestor chain from the filesystem root to `target` inclusive — the
/// breadcrumb rows of a listing, every one a jump target. The root crumb is
/// labeled by its full path (`/`, `C:\`).
pub fn ancestry_crumbs(target: &Path) -> Vec<DirectoryEntry> {
    let mut crumbs: Vec<DirectoryEntry> = Vec::new();
    let mut current = target;
    loop {
        let display = current.to_string_lossy().into_owned();
        let name = match current.file_name() {
            Some(name) => name.to_string_lossy().into_owned(),
            None => display.clone(), // root: basename is empty, label by full path
        };
        crumbs.push(DirectoryEntry {
            name,
            path: display,
            hidden: false,
        });
        match current.parent() {
            Some(parent) if parent != current => current = parent,
            _ => break,
        }
    }
    crumbs.reverse();
    crumbs
}

/// One listing row for a dirent, following symlinks to directories; `None`
/// for non-directories and broken/cyclic links (skipped silently — the
/// browser shows what can be entered, and a broken link cannot).
async fn directory_row(
    parent: &Path,
    name: &str,
    is_directory: bool,
    is_symbolic_link: bool,
    signal: &AbortSignal,
) -> Result<Option<DirectoryEntry>, ()> {
    let path = parent.join(name);
    let mut enterable = is_directory;
    if !enterable && is_symbolic_link {
        // The probe races the caller too: a symlink target on a stalled
        // network filesystem must not keep a departed caller's request alive.
        match race_abort(tokio::fs::metadata(&path), signal).await? {
            Ok(metadata) => enterable = metadata.is_dir(),
            // Broken or cyclic symlink: stat is the probe, failure means
            // "not enterable".
            Err(_) => return Ok(None),
        }
    }
    if !enterable {
        return Ok(None);
    }
    // POSIX hidden convention; Windows' hidden attribute is not exposed by
    // dirents (Known Limitations). The client owns whether hidden rows show.
    Ok(Some(DirectoryEntry {
        name: name.to_string(),
        path: path.to_string_lossy().into_owned(),
        hidden: name.starts_with('.'),
    }))
}

/// The `ctx.directoryPicker` browse implementation (stable capability object
/// per service life).
pub struct BrowseDirectoryPicker {
    capability: DirectoryPickerCapability,
}

impl DirectoryPicker for BrowseDirectoryPicker {
    fn capability(&self) -> DirectoryPickerCapability {
        self.capability.clone()
    }
}

impl BrowseDirectoryPicker {
    /// Construct an unregistered backend; `install` registers it as
    /// `ctx.directoryPicker`.
    pub fn new(config: Config) -> Arc<Self> {
        let list: Arc<
            dyn Fn(
                    Option<String>,
                    AbortSignal,
                ) -> BoxFuture<'static, Result<DirectoryListing, DirectoryPickerListError>>
                + Send
                + Sync,
        > = Arc::new({
            let config = config.clone();
            move |path: Option<String>, signal: AbortSignal| {
                let config = config.clone();
                Box::pin(async move { list_directory(&config, path, &signal).await })
            }
        });
        let create_directory: Arc<
            dyn Fn(String, String) -> BoxFuture<'static, Result<String, DirectoryPickerError>>
                + Send
                + Sync,
        > = Arc::new(move |path: String, name: String| {
            Box::pin(async move { create_directory(&path, &name).await })
        });
        Arc::new(Self {
            capability: DirectoryPickerCapability::Browse(DirectoryPickerBrowseCapability::new(
                list,
                create_directory,
            )),
        })
    }

    /// Construct and register as `ctx.directoryPicker` (TS constructor +
    /// `super(ctx, 'directoryPicker')`).
    pub fn install(ctx: &Context, config: Config) -> Arc<Self> {
        let backend = Self::new(config);
        register(ctx, backend.clone());
        backend
    }
}

/// List one level into the name-sorted bounded window, then probe rows.
async fn list_directory(
    config: &Config,
    path: Option<String>,
    signal: &AbortSignal,
) -> Result<DirectoryListing, DirectoryPickerListError> {
    let home = home_dir();
    // The seam contract takes fully qualified paths only; resolve() would
    // silently rebase a relative or empty wire value under the host process
    // cwd (or, for rooted drive-less Windows forms, its current drive).
    if let Some(path) = &path {
        if !fully_qualified(path, platform()) {
            return Err(DirectoryPickerListError::Unreadable(
                DirectoryPickerError::new(
                    DirectoryPickerErrorCode::DirectoryUnreadable,
                    path.clone(),
                    format!("cannot list \"{path}\": not a fully qualified path"),
                ),
            ));
        }
    }
    let target = PathBuf::from(path.unwrap_or_else(|| home.clone()));

    // Stream the level (one dirent at a time) into a name-sorted window of
    // maxEntries + 1 candidates: memory stays bounded no matter how many
    // children the directory holds, the window keeps the name-sorted head,
    // and the +1 slot lets an in-window extra row prove the cut. A window
    // candidate that turns out non-enterable (broken symlink) is not
    // backfilled from beyond the window — an eviction already marks the
    // level truncated, which stays the honest answer.
    let keep = config.max_entries + 1;
    let mut window: Vec<ListingCandidate> = Vec::new();
    let mut evicted = false;

    let mut level = match race_abort(tokio::fs::read_dir(&target), signal).await {
        Err(()) => return Err(DirectoryPickerListError::Aborted),
        Ok(Ok(level)) => level,
        Ok(Err(error)) => {
            return Err(DirectoryPickerListError::Unreadable(
                DirectoryPickerError::new(
                    DirectoryPickerErrorCode::DirectoryUnreadable,
                    target.to_string_lossy().into_owned(),
                    format!(
                        "cannot list {}: {error}",
                        target.to_string_lossy().into_owned()
                    ),
                ),
            ));
        }
    };
    loop {
        let entry = match race_abort(level.next_entry(), signal).await {
            Err(()) => return Err(DirectoryPickerListError::Aborted),
            Ok(Ok(Some(entry))) => entry,
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                return Err(DirectoryPickerListError::Unreadable(
                    DirectoryPickerError::new(
                        DirectoryPickerErrorCode::DirectoryUnreadable,
                        target.to_string_lossy().into_owned(),
                        format!(
                            "cannot list {}: {error}",
                            target.to_string_lossy().into_owned()
                        ),
                    ),
                ));
            }
        };
        let file_type = match race_abort(entry.file_type(), signal).await {
            Err(()) => return Err(DirectoryPickerListError::Aborted),
            Ok(Ok(file_type)) => file_type,
            Ok(Err(error)) => {
                return Err(DirectoryPickerListError::Unreadable(
                    DirectoryPickerError::new(
                        DirectoryPickerErrorCode::DirectoryUnreadable,
                        target.to_string_lossy().into_owned(),
                        format!(
                            "cannot list {}: {error}",
                            target.to_string_lossy().into_owned()
                        ),
                    ),
                ));
            }
        };
        let is_directory = file_type.is_dir();
        let is_symbolic_link = file_type.is_symlink();
        // Only rows a browser could enter contend for the window; dirent
        // says "directory" outright, a symlink needs the later stat probe.
        if !is_directory && !is_symbolic_link {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let candidate = ListingCandidate {
            name,
            is_directory,
            is_symbolic_link,
        };
        if bounded_insert(&mut window, candidate, keep) {
            evicted = true;
        }
    }

    let mut entries: Vec<DirectoryEntry> = Vec::new();
    let mut truncated = evicted;
    for candidate in window {
        // A caller that departed between reads and probes stops before the
        // next probe (each probe's own await is raced inside directoryRow).
        if signal.aborted() {
            return Err(DirectoryPickerListError::Aborted);
        }
        let row = directory_row(
            &target,
            &candidate.name,
            candidate.is_directory,
            candidate.is_symbolic_link,
            signal,
        )
        .await
        .map_err(|()| DirectoryPickerListError::Aborted)?;
        let Some(row) = row else { continue };
        if entries.len() == config.max_entries {
            truncated = true;
            break;
        }
        entries.push(row);
    }
    Ok(DirectoryListing {
        path: target.to_string_lossy().into_owned(),
        home,
        crumbs: ancestry_crumbs(&target),
        entries,
        truncated,
    })
}

/// Create one child directory under an existing parent. `name` is a single
/// non-blank path segment (no separators, not `.`/`..`).
pub async fn create_directory(path: &str, name: &str) -> Result<String, DirectoryPickerError> {
    // Same fully-qualified fence as list: never rebase a parent under the
    // cwd or the current drive.
    if !fully_qualified(path, platform()) {
        return Err(DirectoryPickerError::new(
            DirectoryPickerErrorCode::DirectoryCreateFailed,
            path,
            format!("cannot create under \"{path}\": not a fully qualified parent path"),
        ));
    }
    let parent = PathBuf::from(path);
    // The backend owns segment validation (the wire schema also refuses
    // these, but direct service consumers must hit the same fence).
    if name.trim().is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        let target = parent.join(name);
        return Err(DirectoryPickerError::new(
            DirectoryPickerErrorCode::DirectoryCreateFailed,
            target.to_string_lossy().into_owned(),
            format!("\"{name}\" is not a single path segment"),
        ));
    }
    let target = parent.join(name);
    // Non-recursive: the parent is the directory the browser is showing, so
    // a missing parent is a real failure, not a level to invent.
    match tokio::fs::create_dir(&target).await {
        Ok(()) => Ok(target.to_string_lossy().into_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(
            DirectoryPickerError::new(
                DirectoryPickerErrorCode::DirectoryExists,
                target.to_string_lossy().into_owned(),
                format!("{} already exists", target.to_string_lossy().into_owned()),
            ),
        ),
        Err(error) => Err(DirectoryPickerError::new(
            DirectoryPickerErrorCode::DirectoryCreateFailed,
            target.to_string_lossy().into_owned(),
            format!("cannot create {}: {error}", target.to_string_lossy().into_owned()),
        )),
    }
}

/// The Cordis plugin form.
pub struct BrowseDirectoryPickerPlugin;

#[async_trait]
impl Plugin for BrowseDirectoryPickerPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new([])
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config =
            Config::from_arcvalue(&config).map_err(|error| PluginError::new(arc(error)))?;
        BrowseDirectoryPicker::install(ctx, config);
        Ok(())
    }
}
