//! `@deepseek-ai/dsh-host-directory-picker` — Service definition for the
//! `ctx.directoryPicker` capability seam: how the web-GUI host lets an
//! operator select a workspace directory. Backends differ in interaction
//! shape, not just mechanism, so the service exposes a discriminated
//! capability instead of one method set: a `native` backend opens one OS
//! chooser on the host's display, while a `browse` backend serves
//! listing/creation primitives for an in-app browser (and thereby works for
//! remote clients no OS dialog can reach). Consumers switch on
//! `capability().kind`; the union is merge-extensible, and the documented
//! default for an unknown kind is to hide the picking affordance rather than
//! fail.
//!
//! # Deviations
//!
//! - TS's merge-extensible capability map is a Rust enum: adding a new
//!   interaction shape is a compile-time change for every consumer, and
//!   consumers match with a fallback arm that hides the picking affordance
//!   (the documented unknown-kind default).
//! - The TS `AbortSignal` parameter collapses to the seam-owned
//!   [`AbortSignal`] (a clonable, abortable, awaitable flag); backends race
//!   it against their blocking work.
//! - TS browse primitives throw [`DirectoryPickerError`]; the Rust
//!   equivalents return `Result<_, DirectoryPickerError>`.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::{Context, Disposer, Service};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

/// The service name under which a backend registers (`ctx.directoryPicker`).
pub const SERVICE_NAME: &str = "directoryPicker";

/// Clonable, abortable cancellation flag standing in for TS `AbortSignal`
/// (caller/connection lifetime). Backends race it against blocking work; a
/// stalled scan must not outlive a disconnected caller.
#[derive(Clone, Default)]
pub struct AbortSignal {
    inner: Arc<AbortSignalInner>,
}

struct AbortSignalInner {
    aborted: AtomicBool,
    notify: tokio::sync::Notify,
}

impl Default for AbortSignalInner {
    fn default() -> Self {
        Self {
            aborted: AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        }
    }
}

impl AbortSignal {
    /// A live, unaborted signal.
    pub fn new() -> Self {
        Self::default()
    }

    /// Trigger the signal; waiters wake and `aborted()` reports true.
    pub fn abort(&self) {
        self.inner.aborted.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    /// Whether the signal has been triggered.
    pub fn aborted(&self) -> bool {
        self.inner.aborted.load(Ordering::SeqCst)
    }

    /// Resolve once the signal aborts (immediately when already aborted).
    pub async fn cancelled(&self) {
        loop {
            if self.aborted() {
                return;
            }
            self.inner.notify.notified().await;
        }
    }
}

/// The native interaction: one OS directory chooser on the host display.
#[derive(Clone)]
pub struct DirectoryPickerNativeCapability {
    /// Open the chooser and wait for the operator. `signal` is the
    /// caller/connection lifetime; abort terminates the chooser. Resolves to
    /// the chosen absolute path, or `None` when the operator cancels.
    pub pick: Arc<dyn Fn(AbortSignal) -> BoxFuture<'static, Option<String>> + Send + Sync>,
}

impl DirectoryPickerNativeCapability {
    pub fn new(
        pick: Arc<dyn Fn(AbortSignal) -> BoxFuture<'static, Option<String>> + Send + Sync>,
    ) -> Self {
        Self { pick }
    }
}

/// One directory row: a listing child or a breadcrumb ancestor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryEntry {
    /// Base name shown in a browser row (a root crumb carries its full path).
    pub name: String,
    /// Absolute host path — clients never join path segments themselves.
    pub path: String,
    /// Hidden by the host platform's convention (dot-prefixed on POSIX); the
    /// client owns whether to show it.
    pub hidden: bool,
}

/// One directory level plus its ancestry, as a browse backend reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryListing {
    /// Absolute path of the listed directory.
    pub path: String,
    /// The host account's home directory (breadcrumb "Home" rooting).
    pub home: String,
    /// Ancestor chain from the filesystem root to the listed directory
    /// inclusive; every crumb is a jump target (crumb `hidden` is always
    /// false).
    pub crumbs: Vec<DirectoryEntry>,
    /// Direct child directories, name-sorted; symlinks to directories
    /// included.
    pub entries: Vec<DirectoryEntry>,
    /// True when the backend cut `entries` at its complete-result bound: the
    /// level has more child directories than reported, and the missing rows
    /// are the name-sorted tail (hidden rows count toward the bound).
    pub truncated: bool,
}

/// Outcome vocabulary of a browse `list`: an abort is the caller's own
/// reason (TS rejects with the signal's reason), while a listing failure is
/// the typed [`DirectoryPickerError`]. The two never collapse — consumers
/// must not dress a departed caller as an unreadable directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryPickerListError {
    /// The caller's signal aborted the scan.
    Aborted,
    /// The target is not fully qualified or cannot be listed.
    Unreadable(DirectoryPickerError),
}

impl fmt::Display for DirectoryPickerListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Aborted => write!(f, "listing aborted by the caller"),
            Self::Unreadable(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for DirectoryPickerListError {}

/// The browse interaction: listing/creation primitives an in-app browser
/// drives one level at a time. Works for remote clients — nothing renders on
/// the host display.
#[derive(Clone)]
pub struct DirectoryPickerBrowseCapability {
    /// List one directory level. `None` lists the home directory. `signal`
    /// is the caller lifetime; abort stops the scan (a stalled network
    /// directory must not outlive a disconnected caller) and resolves with
    /// [`DirectoryPickerListError::Aborted`].
    pub list: Arc<
        dyn Fn(
                Option<String>,
                AbortSignal,
            )
                -> BoxFuture<'static, Result<DirectoryListing, DirectoryPickerListError>>
            + Send
            + Sync,
    >,
    /// Create one child directory under an existing parent. `name` is a
    /// single non-blank path segment (no separators, not `.`/`..`). Resolves
    /// to the created directory's absolute path.
    pub create_directory: Arc<
        dyn Fn(String, String) -> BoxFuture<'static, Result<String, DirectoryPickerError>>
            + Send
            + Sync,
    >,
}

impl DirectoryPickerBrowseCapability {
    pub fn new(
        list: Arc<
            dyn Fn(
                    Option<String>,
                    AbortSignal,
                )
                    -> BoxFuture<'static, Result<DirectoryListing, DirectoryPickerListError>>
                + Send
                + Sync,
        >,
        create_directory: Arc<
            dyn Fn(String, String) -> BoxFuture<'static, Result<String, DirectoryPickerError>>
                + Send
                + Sync,
        >,
    ) -> Self {
        Self {
            list,
            create_directory,
        }
    }
}

/// Union of interaction shapes a backend can provide. The TS counterpart is
/// a merge-extensible map keyed by capability kind; a new backend
/// declaration-merges its shape there instead of editing this package. The
/// Rust enum is the same vocabulary as a closed set — consumers must match
/// every arm, with `_ => hide` as the documented unknown-kind default.
#[derive(Clone)]
pub enum DirectoryPickerCapability {
    Native(DirectoryPickerNativeCapability),
    Browse(DirectoryPickerBrowseCapability),
}

impl DirectoryPickerCapability {
    /// The capability kind literal (`native` | `browse`).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Native(_) => "native",
            Self::Browse(_) => "browse",
        }
    }
}

/// Closed failure vocabulary of the browse primitives (mirrored onto the
/// wire by consumers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DirectoryPickerErrorCode {
    /// The target is not fully qualified or cannot be listed.
    DirectoryUnreadable,
    /// The child directory already exists.
    DirectoryExists,
    /// The parent is not fully qualified, or any other creation failure.
    DirectoryCreateFailed,
}

impl DirectoryPickerErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DirectoryUnreadable => "directory-unreadable",
            Self::DirectoryExists => "directory-exists",
            Self::DirectoryCreateFailed => "directory-create-failed",
        }
    }
}

/// Typed failure of the browse primitives so consumers can map business
/// codes without string matching (TS `DirectoryPickerError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryPickerError {
    /// Closed business code of the failure.
    pub code: DirectoryPickerErrorCode,
    /// The absolute path the failure is about.
    pub path: String,
    /// Operator-facing description.
    pub message: String,
}

impl DirectoryPickerError {
    pub fn new(
        code: DirectoryPickerErrorCode,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for DirectoryPickerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for DirectoryPickerError {}

/// Abstract directory-picking service. Implement [`DirectoryPicker::capability`]
/// and register the instance with [`register`] — it exposes as
/// `ctx.directoryPicker` (one implementation per context; a second
/// registration fails with cordis' standard duplicate-service behavior). The
/// capability object must be stable for the service lifetime: consumers may
/// capture it across calls.
pub trait DirectoryPicker: Send + Sync + 'static {
    /// The backend's interaction capability.
    fn capability(&self) -> DirectoryPickerCapability;
}

impl Service for dyn DirectoryPicker {
    fn service_name(&self) -> &'static str {
        SERVICE_NAME
    }
}

/// Register a backend as `ctx.directoryPicker` (the TS `super(ctx,
/// 'directoryPicker')`). The returned disposer unregisters with the owning
/// fiber.
pub fn register(ctx: &Context, picker: Arc<dyn DirectoryPicker>) -> Disposer {
    ctx.register_service(picker)
}
