//! File-backed credentials provider over `$DSH_HOME/.credentials.yaml`,
//! layered against the environment by how much each layer is trusted:
//!
//! ```text
//! inherited process environment      (read-only, wins)
//! > $DSH_HOME/.credentials.yaml      (provider-managed, writable)
//! > <invocation cwd>/.env            (read-only fallback)
//! > $DSH_HOME/.env                   (read-only fallback)
//! ```
//!
//! Rust port of `packages/credentials/credentials-local/src/index.ts`.
//!
//! # Deviations
//!
//! - The watcher abstraction is a crate seam: the real backend is the
//!   `notify` crate (with a debounce window instead of chokidar's
//!   awaitWriteFinish); tests inject a fake to drive the event pipeline
//!   deterministically, mirroring the TS `vi.mock('chokidar')`.
//! - `refresh` warns on every failure; the TS rethrows `INVARIANT`-coded
//!   reload failures to the queue's error surface (the commit still lands
//!   before the fan-out either way).
//! - The atomic write and the writer lock come from `dsh-atomic-write`; the
//!   writer is a seam for the drain spec's gated hold point.
//! - The provider has no Cordis fiber of its own: installation registers a
//!   ctx effect that drains on fiber unload (closed → quiesce), and
//!   [`LocalCredentialProvider::close`] exposes the same drain for tests.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::Context;
use futures::future::BoxFuture;
use indexmap::IndexMap;
use parking_lot::Mutex;

use dsh_credentials::{CredentialProvider, CredentialRef, credential_ref};
use dsh_home_paths::{canonicalize_watch_path, resolve_dsh_home};
use dsh_launch_environment::{LaunchEnvironmentSource, launch_environment_of};

use crate::document::{parse_credentials_document, render_document};

/// Basename of the credentials document inside the harness home.
pub const CREDENTIALS_FILENAME: &str = ".credentials.yaml";

/// Plugin config: file location and hot-reload behavior.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Credentials document path; defaults to `.credentials.yaml` under the
    /// harness home.
    pub path: Option<String>,
    /// Harness home used when `path` is omitted; defaults to `$DSH_HOME` or
    /// `~/.dsh`.
    pub dsh_home: Option<String>,
    /// Watch the document and hot-publish external edits; defaults to true.
    pub watch: Option<bool>,
    /// Watcher write-settle window in milliseconds; defaults to 100.
    pub debounce_ms: Option<u64>,
}

/// Fully resolved provider parameters; defaulting happens here, never
/// inline (TS `ResolvedSpec`).
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedSpec {
    pub filename: String,
    pub watch: bool,
    pub debounce_ms: u64,
}

/// Resolve the runtime spec from plugin config: an explicit `path` wins,
/// otherwise the document lives at `<harness home>/.credentials.yaml`.
pub fn resolve_spec(config: &Config) -> ResolvedSpec {
    let filename = match &config.path {
        Some(path) => PathBuf::from(path),
        None => resolve_dsh_home(config.dsh_home.as_deref(), &|name: &str| {
            std::env::var(name).ok()
        })
        .join(CREDENTIALS_FILENAME),
    };
    // One platform-canonical spelling (the TS `resolve` normalizes
    // separators the same way on Windows).
    let filename = std::path::absolute(&filename).unwrap_or(filename);
    ResolvedSpec {
        filename: filename.to_string_lossy().into_owned(),
        watch: config.watch.unwrap_or(true),
        debounce_ms: config.debounce_ms.unwrap_or(100),
    }
}

/// Permission bits outside the owner; a credentials document must have none
/// of them.
#[cfg(unix)]
const GROUP_OTHER_BITS: u32 = 0o077;

/// Reject a credentials document other OS users can read, before its
/// contents are read at all. The provider creates and replaces the file at
/// `0600`, but a hand-written or externally generated one carries whatever
/// umask produced it, and silently serving secrets out of a world-readable
/// file would make the mode the provider promises meaningless.
///
/// POSIX only: Windows has no mode to inspect, so the check is skipped
/// rather than faked.
async fn assert_owner_only(filename: &str) -> Result<(), String> {
    match tokio::fs::metadata(filename).await {
        Ok(meta) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = meta.permissions().mode();
                let offending = mode & GROUP_OTHER_BITS;
                if offending != 0 {
                    return Err(format!(
                        "credentials-local: {filename} is readable beyond its owner (mode {:o}); run \"chmod 600 {filename}\" before starting again",
                        mode & 0o777
                    ));
                }
            }
            #[cfg(not(unix))]
            let _ = meta;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // TS node's realpath reports ENOTDIR when a parent component is a
            // file; Windows Rust canonicalize reports NotFound there — check
            // explicitly so a file-as-parent is a loud misconfiguration, not
            // "no credentials yet".
            if let Some(parent) = Path::new(filename).parent()
                && let Ok(meta) = tokio::fs::metadata(parent).await
                && !meta.is_dir()
            {
                return Err(format!(
                    "ENOTDIR: cannot reach {filename}: a parent component is not a directory"
                ));
            }
            canonicalize_watch_path(Path::new(filename))
                .await
                .map_err(|error| error.to_string())?;
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn is_enoent(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

/// The durable-document writer seam (default: `dsh-atomic-write`'s
/// `write_file_atomic` at `0600`).
pub type DocumentWriter =
    Arc<dyn Fn(&Path, &[u8]) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

/// The document reader seam (default: `tokio::fs::read_to_string`); tests
/// inject read failures after the permission check.
pub type DocumentReader =
    Arc<dyn Fn(&Path) -> BoxFuture<'static, std::io::Result<String>> + Send + Sync>;

fn default_reader() -> DocumentReader {
    Arc::new(|path: &Path| {
        let path = path.to_path_buf();
        Box::pin(async move { tokio::fs::read_to_string(path).await })
    })
}

fn default_writer() -> DocumentWriter {
    Arc::new(|path: &Path, content: &[u8]| {
        let path = path.to_path_buf();
        let content = content.to_vec();
        Box::pin(async move {
            dsh_atomic_write::write_file_atomic(
                &path,
                &content,
                dsh_atomic_write::WriteFileAtomicOptions {
                    mode: 0o600,
                    dir_mode: Some(0o700),
                },
            )
            .await
            .map_err(|error| error.to_string())
        })
    })
}

/// Shared provider state swapped wholesale on every reload.
struct ProviderState {
    /// Raw text of the last read or persisted document; `None` while the
    /// file is absent. Watcher events whose content equals this cache are
    /// no-ops, which is also the self-write suppression.
    text: Option<String>,
    /// Parsed document snapshot; replaced wholesale on every reload.
    values: IndexMap<String, String>,
}

/// File-backed credentials provider (`$DSH_HOME/.credentials.yaml`).
pub struct LocalCredentialProvider {
    ctx: Context,
    spec: ResolvedSpec,
    state: Mutex<ProviderState>,
    /// Single exclusive operation chain (the TS settled promise tail):
    /// watcher reloads and line edits run one at a time in queue order, so an
    /// edit can never render from text a concurrent reload is busy replacing.
    operation_tail: Arc<tokio::sync::Mutex<()>>,
    /// Set at dispose: refuse new writes and let in-flight work no-op.
    closed: AtomicBool,
    writer: DocumentWriter,
    reader: DocumentReader,
    watcher: Mutex<Option<Arc<dyn WatchControl>>>,
    /// The installed `Arc<Self>` (filled after construction): queued
    /// operations need an owned handle to live past the enqueue call.
    self_arc: std::sync::OnceLock<Arc<LocalCredentialProvider>>,
}

impl LocalCredentialProvider {
    /// Construct, resolve the spec, load the initial document, and register
    /// as `ctx.credentials` (the TS constructor + `Service.init` collapse).
    pub fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        Self::install_with_seams(ctx, config, default_writer(), None, default_reader())
    }

    /// Install with seam-local writer, watcher factory, and reader (the
    /// spec harness's gated-write, fake-watcher, and read-failure
    /// injections).
    pub fn install_with_seams(
        ctx: &Context,
        config: Config,
        writer: DocumentWriter,
        watcher_factory: Option<WatcherFactory>,
        reader: DocumentReader,
    ) -> Result<Arc<Self>, String> {
        let spec = resolve_spec(&config);
        let provider = Arc::new(Self {
            ctx: ctx.clone(),
            spec: spec.clone(),
            state: Mutex::new(ProviderState {
                text: None,
                values: IndexMap::new(),
            }),
            operation_tail: Arc::new(tokio::sync::Mutex::new(())),
            closed: AtomicBool::new(false),
            writer,
            reader,
            watcher: Mutex::new(None),
            self_arc: std::sync::OnceLock::new(),
        });
        provider.self_arc.set(provider.clone()).ok();
        futures::executor::block_on(provider.load_initial())?;
        let erased: Arc<dyn CredentialProvider> = provider.clone();
        ctx.register_service(erased);

        if spec.watch {
            let target =
                futures::executor::block_on(canonicalize_watch_path(Path::new(&spec.filename)))
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .into_owned();
            let factory = watcher_factory.unwrap_or_else(notify_watcher_factory);
            let control = futures::executor::block_on(factory(
                target,
                provider.clone() as Arc<dyn WatchSink>,
                spec.debounce_ms,
            ))?;
            *provider.watcher.lock() = Some(control);
        }

        // Drain on fiber unload: refuse new operations, close the watcher,
        // then settle the queued ones (the TS `Service.init` disposers).
        let provider_for_effect = provider.clone();
        let _ = ctx.effect(
            "credentials-local.dispose",
            Box::pin(async move {
                Some(cordis::make_disposer(move || {
                    let provider = provider_for_effect.clone();
                    Box::pin(async move {
                        provider.drain().await;
                    })
                }))
            }),
        );
        Ok(provider)
    }

    /// The resolved spec (diagnostic surface).
    pub fn spec(&self) -> &ResolvedSpec {
        &self.spec
    }

    /// The resolved absolute document path (diagnostic surface).
    pub fn filename(&self) -> &str {
        &self.spec.filename
    }

    /// The drain performed by fiber disposal (also exposed for tests): stop
    /// accepting work, close the watcher, and settle the operation queue.
    pub async fn drain(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let control = self.watcher.lock().take();
        if let Some(control) = control {
            control.close().await;
        }
        // Acquiring the chain settles every in-flight operation.
        let _ = self.operation_tail.lock().await;
    }

    /// Opaque read of `closed`: control flow cannot narrow it across awaits.
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// The inherited-environment value for a reference, or `None` when empty
    /// or unset.
    fn inherited(&self, reference: &CredentialRef) -> Option<String> {
        let snapshot = launch_environment_of(&self.ctx);
        let entry = snapshot.get_from(reference.as_str(), &[LaunchEnvironmentSource::Process]);
        entry
            .filter(|entry| !entry.value.is_empty())
            .map(|entry| entry.value.clone())
    }

    /// The `.env` fallback for a reference — below the managed store, never
    /// above it. The invoking project ranks over the user's home file,
    /// matching the environment layering: the more specific location wins.
    fn dotenv_fallback(
        &self,
        reference: &CredentialRef,
    ) -> Option<(String, LaunchEnvironmentSource)> {
        let snapshot = launch_environment_of(&self.ctx);
        let entry = snapshot.get_from(
            reference.as_str(),
            &[
                LaunchEnvironmentSource::ProjectEnv,
                LaunchEnvironmentSource::UserEnv,
            ],
        );
        entry
            .filter(|entry| !entry.value.is_empty())
            .map(|entry| (entry.value.clone(), entry.source))
    }

    /// Reject a write the inherited environment would shadow into apparent
    /// no-effect. Only that layer can shadow a write: everything else this
    /// provider resolves ranks below the document being written.
    fn assert_unshadowed(&self, reference: &CredentialRef, verb: &str) -> Result<(), String> {
        if self.inherited(reference).is_some() {
            return Err(format!(
                "credentials-local: \"{reference}\" is supplied read-only by the launching environment, so {verb} would be shadowed; unset it in the shell you start dsh from instead"
            ));
        }
        Ok(())
    }

    /// Boot read: an absent file is an empty store; an invalid one fails the
    /// plugin's activation, because a credentials document that exists but
    /// cannot be trusted must never be treated as "no credentials stored".
    async fn load_initial(&self) -> Result<(), String> {
        assert_owner_only(&self.spec.filename).await?;
        let text = match (self.reader)(Path::new(&self.spec.filename)).await {
            Ok(text) => text,
            Err(error) if is_enoent(&error) => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        let values = parse_credentials_document(&text, &self.spec.filename)?;
        *self.state.lock() = ProviderState {
            text: Some(text),
            values,
        };
        Ok(())
    }

    /// Queue a reload; only a rejection escapes the queue's error surface.
    fn queue_refresh(&self) {
        let provider = self.self_arc.get().cloned().expect("installed");
        let provider_for_run = provider.clone();
        let run = async move {
            let _guard = provider_for_run.operation_tail.lock().await;
            provider_for_run.refresh().await
        };
        // The refresh runs detached (the TS `void this.enqueue(...)`); the
        // sink that triggered it runs inside a runtime.
        let handle = tokio::runtime::Handle::try_current();
        if let Ok(handle) = handle {
            let provider_for_log = provider.clone();
            handle.spawn(async move {
                if let Err(error) = run.await {
                    // A reload failure keeps the queue alive and surfaces as
                    // an error so one poisoned commit cannot silently end
                    // hot reloading forever.
                    provider_for_log
                        .ctx
                        .named_logger(None)
                        .error(vec![cordis::arc(format!(
                            "credentials-local: reload commit failed at {}: {error}",
                            provider_for_log.spec.filename
                        ))]);
                }
            });
        } else {
            // Outside a runtime (a bare watcher callback): drive the refresh
            // to completion on this thread.
            futures::executor::block_on(run).ok();
        }
    }

    /// Re-read the document after a watcher event. Unchanged content
    /// (including this provider's own writes) is a no-op; an unreadable
    /// document keeps the last good snapshot and warns — a live hot-reload
    /// must never take the process down.
    async fn refresh(&self) -> Result<(), String> {
        if self.is_closed() {
            return Ok(());
        }
        match self.reconcile_from_disk().await {
            Ok(()) => Ok(()),
            Err(error) => {
                self.ctx.named_logger(None).warn(vec![cordis::arc(format!(
                    "credentials-local: reload failed at {}; keeping the last good document: {error}",
                    self.spec.filename
                ))]);
                Ok(())
            }
        }
    }

    /// Compare the on-disk text against the cache and publish any difference
    /// into the seam. Absence publishes the empty store; an unreadable or
    /// invalid document throws, so each caller picks its policy — a reload
    /// warns and keeps the last good snapshot, a write fails loud rather
    /// than overwriting a document it could not understand.
    async fn reconcile_from_disk(&self) -> Result<(), String> {
        // Re-checked on every reload and before every write: an external
        // editor or a restored backup can loosen the mode after boot.
        assert_owner_only(&self.spec.filename).await?;
        let text = match (self.reader)(Path::new(&self.spec.filename)).await {
            Ok(text) => Some(text),
            Err(error) if is_enoent(&error) => None,
            Err(error) => return Err(error.to_string()),
        };
        let changed;
        {
            let mut state = self.state.lock();
            if text.as_deref() == state.text.as_deref() || self.is_closed() {
                return Ok(());
            }
            let next = match &text {
                None => IndexMap::new(),
                Some(text) => parse_credentials_document(text, &self.spec.filename)?,
            };
            changed = changed_refs(&state.values, &next);
            state.text = text;
            state.values = next;
        }
        for reference in changed {
            self.notify_updated(&self.ctx, &reference).await?;
        }
        Ok(())
    }

    /// Queue one line edit; entry checks reject early, the queue re-judges
    /// them at run time.
    async fn write(&self, reference: &CredentialRef, value: Option<&str>) -> Result<(), String> {
        let verb = if value.is_some() { "set" } else { "unset" };
        if self.is_closed() {
            return Err(format!(
                "credentials-local is disposed: cannot {verb} \"{reference}\""
            ));
        }
        self.assert_unshadowed(reference, verb)?;
        let _guard = self.operation_tail.lock().await;
        if self.is_closed() {
            return Err(format!(
                "credentials-local was disposed before the queued \"{reference}\" {verb} ran"
            ));
        }
        // Re-judged at run time: the environment may have changed while
        // queued.
        self.assert_unshadowed(reference, verb)?;
        // The writer lock's exclusive create needs the parent to exist;
        // 0700 because the harness home holds user-private data.
        let parent = Path::new(&self.spec.filename)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(&parent)
                .await
                .map_err(|error| error.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = tokio::fs::DirBuilder::new();
                builder.recursive(true);
                builder.mode(0o700);
                let _ = builder.create(&parent).await;
            }
        }
        let filename = PathBuf::from(&self.spec.filename);
        dsh_atomic_write::with_file_lock(&filename, async {
            // Read-modify-write: fold in any on-disk state this process has
            // not observed yet — an external edit still inside the watcher
            // debounce window, a change the watcher missed, or another
            // process's write — so the line edit below can never resurrect a
            // stale document.
            self.reconcile_from_disk().await?;
            let existing;
            let next_text;
            {
                let state = self.state.lock();
                existing = state.values.get(reference.as_str()).cloned();
                if value.is_none() && existing.is_none() {
                    return Ok(());
                }
                next_text = render_document(state.text.as_deref(), reference, value);
            }
            // 0600: a document holding secrets is never world-readable.
            (self.writer)(&filename, next_text.as_bytes()).await?;
            {
                let mut state = self.state.lock();
                state.text = Some(next_text);
                if let Some(value) = value {
                    state
                        .values
                        .insert(reference.as_str().to_string(), value.to_string());
                } else {
                    state.values.shift_remove(reference.as_str());
                }
            }
            // After the commit: a broken observer must never make the
            // durable write look failed (an INVARIANT failure still
            // rethrows).
            self.notify_updated(&self.ctx, reference).await
        })
        .await
        .map_err(|error| error.to_string())?
    }
}

/// Entries whose stored value changed; the parser has already proven every
/// key addressable.
fn changed_refs(
    prev: &IndexMap<String, String>,
    next: &IndexMap<String, String>,
) -> Vec<CredentialRef> {
    let mut changed = Vec::new();
    let mut keys: Vec<&String> = prev.keys().chain(next.keys()).collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        if prev.get(key) == next.get(key) {
            continue;
        }
        changed.push(credential_ref(key));
    }
    changed
}

#[async_trait::async_trait]
impl CredentialProvider for LocalCredentialProvider {
    async fn resolve(
        &self,
        reference: &CredentialRef,
    ) -> Option<dsh_credentials::ResolvedCredential> {
        if let Some(inherited) = self.inherited(reference) {
            return Some(dsh_credentials::ResolvedCredential {
                value: inherited,
                source: "env".to_string(),
            });
        }
        let stored = self.state.lock().values.get(reference.as_str()).cloned();
        if let Some(stored) = stored {
            return Some(dsh_credentials::ResolvedCredential {
                value: stored,
                source: "file".to_string(),
            });
        }
        let fallback = self.dotenv_fallback(reference);
        fallback.map(|(value, source)| dsh_credentials::ResolvedCredential {
            value,
            source: source.as_str().to_string(),
        })
    }

    async fn describe(&self, reference: &CredentialRef) -> dsh_credentials::CredentialInfo {
        // Only the inherited environment is unwritable: it is the one layer
        // this process cannot edit. A user `.env` value is writable in the
        // sense that matters — storing a key replaces it as the effective
        // one.
        if self.inherited(reference).is_some() {
            return dsh_credentials::CredentialInfo {
                configured: true,
                source: Some("env".to_string()),
                writable: false,
            };
        }
        let stored = self.state.lock().values.get(reference.as_str()).cloned();
        if let Some(_stored) = stored {
            return dsh_credentials::CredentialInfo {
                configured: true,
                source: Some("file".to_string()),
                writable: true,
            };
        }
        let fallback = self.dotenv_fallback(reference);
        if let Some((_, source)) = fallback {
            return dsh_credentials::CredentialInfo {
                configured: true,
                source: Some(source.as_str().to_string()),
                writable: true,
            };
        }
        dsh_credentials::CredentialInfo {
            configured: false,
            source: None,
            writable: true,
        }
    }

    async fn set(&self, reference: &CredentialRef, value: &str) -> Result<(), String> {
        if value.is_empty() {
            return Err(format!(
                "credentials-local: an empty value cannot be stored for \"{reference}\"; use unset"
            ));
        }
        self.write(reference, Some(value)).await
    }

    async fn unset(&self, reference: &CredentialRef) -> Result<(), String> {
        self.write(reference, None).await
    }
}

// ---------------------------------------------------------------------------
// watcher seam

/// Signals a watcher backend delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchSignal {
    Changed,
    Ready,
}

/// The provider-side surface a watcher backend drives (queue refreshes).
pub trait WatchSink: Send + Sync {
    fn on_signal(&self, signal: WatchSignal);
    fn on_error(&self, message: String);
}

impl WatchSink for LocalCredentialProvider {
    fn on_signal(&self, signal: WatchSignal) {
        if self.is_closed() {
            return;
        }
        match signal {
            WatchSignal::Changed => self.queue_refresh(),
            // The initial load raced the watcher's own setup: one reconcile
            // at ready closes the gap (the TS watcher `ready` handler).
            WatchSignal::Ready => self.queue_refresh(),
        }
    }

    fn on_error(&self, message: String) {
        self.ctx.named_logger(None).warn(vec![cordis::arc(format!(
            "credentials-local: watcher error on {}: {message}",
            self.spec.filename
        ))]);
    }
}

/// A running watcher's teardown handle.
pub trait WatchControl: Send + Sync {
    fn close(&self) -> BoxFuture<'static, ()>;
}

/// Builds a watcher backend for one canonical document path, driving `sink`
/// with the configured debounce window.
pub type WatcherFactory = Arc<
    dyn Fn(
            String,
            Arc<dyn WatchSink>,
            u64,
        ) -> BoxFuture<'static, Result<Arc<dyn WatchControl>, String>>
        + Send
        + Sync,
>;

/// The real backend: `notify` with a write-settle debounce window.
pub fn notify_watcher_factory() -> WatcherFactory {
    Arc::new(|target: String, sink: Arc<dyn WatchSink>, debounce: u64| {
        Box::pin(async move {
            use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WatchSignal>();
            let target_for_watch = target.clone();
            let watcher = tokio::task::spawn_blocking(move || {
                let mut watcher = RecommendedWatcher::new(
                    move |_event: notify::Result<notify::Event>| {
                        let _ = tx.send(WatchSignal::Changed);
                    },
                    notify::Config::default(),
                )
                .map_err(|error| error.to_string())?;
                watcher
                    .watch(Path::new(&target_for_watch), RecursiveMode::NonRecursive)
                    .map_err(|error| error.to_string())?;
                Ok::<_, String>(watcher)
            })
            .await
            .map_err(|error| error.to_string())??;
            // Debounced refresh task: coalesce bursts into one reload after
            // the write-settle window (the TS awaitWriteFinish equivalent).
            let sink_for_task = sink.clone();
            let target_for_task = target.clone();
            tokio::spawn(async move {
                while let Some(_signal) = rx.recv().await {
                    let sink = sink_for_task.clone();
                    let target = target_for_task.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(debounce)).await;
                        if tokio::fs::metadata(&target).await.is_ok() {
                            sink.on_signal(WatchSignal::Changed);
                        }
                    });
                }
            });
            // Reconcile once at startup: a change written between the
            // initial load and the watcher becoming active never fires an
            // event (the TS `ready` handler).
            sink.on_signal(WatchSignal::Ready);
            let control = Arc::new(NotifyControl {
                _watcher: parking_lot::Mutex::new(Some(watcher)),
            }) as Arc<dyn WatchControl>;
            Ok(control)
        })
    })
}

struct NotifyControl {
    _watcher: parking_lot::Mutex<Option<notify::RecommendedWatcher>>,
}

impl WatchControl for NotifyControl {
    fn close(&self) -> BoxFuture<'static, ()> {
        // Dropping the watcher stops its event delivery; the debounce
        // channel closes and the task exits. The drop itself is synchronous.
        self._watcher.lock().take();
        Box::pin(async {})
    }
}
