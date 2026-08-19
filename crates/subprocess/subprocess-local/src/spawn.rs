//! Process plumbing for the local subprocess service: detached process-tree
//! spawn with per-stream stdio dispositions, tail-keep collection with spill
//! files, tree-scoped signalling (POSIX groups; Windows taskkill), and the
//! SIGTERM→SIGKILL escalation. This layer reacts to an abort signal; callers
//! own deadlines, teardown ladders, and cause classification. Rust port of
//! `packages/subprocess/subprocess-local/src/spawn.ts`.
//!
//! # Deviations
//!
//! - The abort predicate is polled every 15 ms (the TS `AbortSignal` is an
//!   event target, which has no Rust equivalent), so abort reactions can lag
//!   by up to one tick.
//! - Spawn failures reject `spawn_subprocess` itself (Rust `Result`); the TS
//!   seam instead returns a `pid: -1` handle whose `done` promise rejects.
//! - Pipe-mode streams settle `done` at direct-child exit: Rust cannot
//!   observe "all write ends closed" on a caller-owned read end, so the TS
//!   close-bound for descendant-held pipe-mode pipes collapses into the exit
//!   boundary (collect-mode drains stay bounded by `graceMs`).
//! - Dropping the last handle/future clone before settlement kills the
//!   direct child (tokio `kill_on_drop`), where a detached Node child would
//!   survive. The owning service keeps every live handle until whole-tree
//!   exit, so managed disposal is unaffected.
//! - `taskkill` runs synchronously (`status()` instead of `spawnSync`).

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::time::Duration;

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use uuid::Uuid;

use dsh_subprocess::{
    CollectedOutput, SubprocessAbort, SubprocessCollectedOutputs, SubprocessHandle,
    SubprocessOutcome, SubprocessOutputMode, SubprocessOutputRead, SubprocessOutputReader,
    SubprocessSpawnSpec, SubprocessStdinMode, SubprocessStdio, scrubbed_parent_env,
};
use dsh_timeout::MAX_TIMER_DELAY_MS;

/// Build a child environment: explicit caller entries override the scrubbed
/// parent base using the target platform's environment-key semantics. A
/// string deliberately restores or overrides an entry; an explicit `None`
/// tombstone removes an ordinary ambient entry (TS `childEnv`).
pub fn child_env(extra: Option<&[(String, Option<String>)]>) -> Vec<(String, String)> {
    let mut env = scrubbed_parent_env();
    let Some(extra) = extra else { return env };
    #[cfg(not(windows))]
    for (key, value) in extra {
        env.retain(|(inherited, _)| inherited != key);
        if let Some(value) = value {
            env.push((key.clone(), value.clone()));
        }
    }
    #[cfg(windows)]
    for (key, value) in extra {
        let normalized = key.to_uppercase();
        env.retain(|(inherited, _)| inherited.to_uppercase() != normalized);
        if let Some(value) = value {
            env.push((key.clone(), value.clone()));
        }
    }
    env
}

/// The target platform's string name, mapped from `cfg!` facts
/// (Node's `process.platform` vocabulary).
pub fn host_platform() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        std::env::consts::OS
    }
}

/// Injectable knobs so tests can exercise spill and platform behavior
/// deterministically (TS `SpawnInternals`).
#[derive(Clone, Default)]
pub struct SpawnInternals {
    /// Directory for spill files (defaults to the OS temp dir).
    pub spill_dir: Option<PathBuf>,
    /// Windows tree-termination runner (defaults to
    /// `taskkill /PID <pid> /T /F`).
    pub taskkill: Option<Arc<dyn Fn(i32) + Send + Sync>>,
    /// Host platform override for signalling decisions.
    pub platform: Option<String>,
    /// Linux process-group member probe (defaults to `/proc` inspection).
    pub linux_group_has_live_members: Option<Arc<dyn Fn(i32) -> Option<bool> + Send + Sync>>,
}

/// Local-only synchronous final termination used by the owning service during
/// host exit and as the last fallback after failed normal disposal. It is
/// intentionally absent from the public subprocess seam (TS
/// `LocalSubprocessHandle`).
pub trait LocalSubprocessHandle: SubprocessHandle {
    /// Force-terminate the current tree synchronously without starting timers
    /// or waits.
    fn terminate_for_host_exit(&self);
}

/// Liveness-poll cadence for tree-exit waits (TS `sleepTick`).
const SLEEP_TICK_MS: u64 = 15;

static SPILL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The default spill location: a private (0700 on POSIX) per-process
/// directory under the OS tmpdir, created lazily. Predictable world-readable
/// paths would let other local users read command output or pre-create
/// symlinks (TS `privateSpillDir`).
fn private_spill_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "dsh-subprocess-{}-{}",
            std::process::id(),
            Uuid::new_v4().simple()
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let _ = std::fs::DirBuilder::new().mode(0o700).create(&dir);
        }
        #[cfg(not(unix))]
        {
            let _ = std::fs::create_dir(&dir);
        }
        dir
    })
    .clone()
}

/// Collects one stream with a bounded in-memory tail. With a spill cap, on
/// first overflow a spill file is created and every chunk (including those
/// already collected) is appended there while the full stream remains within
/// the cap; without one, only the in-memory tail is ever retained (the
/// diagnostic-tail shape — a language server's stderr) (TS `OutputCollector`).
pub struct OutputCollector {
    chunks: Vec<Vec<u8>>,
    bytes: usize,
    dropped: bool,
    spill_fd: Option<std::fs::File>,
    spill_file: Option<PathBuf>,
    spill_disabled: bool,
    /// Total bytes ever pushed (not just retained).
    total: u64,
    max_bytes: usize,
    max_spill_bytes: Option<u64>,
    label: String,
    spill_dir: PathBuf,
}

impl OutputCollector {
    pub fn new(
        max_bytes: usize,
        max_spill_bytes: Option<u64>,
        label: impl Into<String>,
        spill_dir: PathBuf,
    ) -> Self {
        let spill_disabled = max_spill_bytes.is_none();
        Self {
            chunks: Vec::new(),
            bytes: 0,
            dropped: false,
            spill_fd: None,
            spill_file: None,
            spill_disabled,
            total: 0,
            max_bytes,
            max_spill_bytes,
            label: label.into(),
            spill_dir,
        }
    }

    /// Ingest one stream chunk, counting it toward the whole-stream total.
    /// On first overflow of the in-memory cap a spill file is opened (when
    /// spilling is enabled) and every chunk (already-collected ones included)
    /// is appended there from then on; the in-memory tail then drops whole
    /// chunks from its head (or the head of a single over-cap chunk) until it
    /// fits the cap again.
    pub fn push(&mut self, chunk: &[u8]) {
        self.total += chunk.len() as u64;
        let overflows = self.bytes + chunk.len() > self.max_bytes;
        if !self.spill_disabled && (overflows || self.spill_fd.is_some()) {
            self.spill_all(chunk);
        }
        self.chunks.push(chunk.to_vec());
        self.bytes += chunk.len();
        while self.bytes > self.max_bytes {
            let excess = self.bytes - self.max_bytes;
            if self.chunks[0].len() <= excess {
                // Drop the whole head chunk (length ≥ 1 is guaranteed while
                // over cap).
                self.bytes -= self.chunks.remove(0).len();
            } else {
                // Trim the head so the retained window is byte-exact at the
                // cap — a diagnostic tail must hold the LAST maxBytes
                // regardless of how the stream was chunked.
                let head = &self.chunks[0];
                let rest = head[excess..].to_vec();
                self.chunks[0] = rest;
                self.bytes -= excess;
            }
            self.dropped = true;
        }
    }

    /// Open the spill file lazily and append `chunk` (and any prior chunks
    /// once) (TS `spillAll`).
    fn spill_all(&mut self, chunk: &[u8]) {
        if let Some(max) = self.max_spill_bytes {
            if self.total > max {
                self.discard_spill();
                return;
            }
        }
        if self.spill_fd.is_none() {
            // Random suffix + create_new (O_EXCL, fails on any existing path,
            // symlink or not) + owner-only mode: defeats spill-path
            // prediction and symlink planting in shared tmp dirs.
            let file_name = format!(
                "dsh-subprocess-{}-{}-{}-{}.log",
                std::process::id(),
                SPILL_COUNTER.fetch_add(1, SeqCst) + 1,
                Uuid::new_v4().simple(),
                self.label,
            );
            let path = self.spill_dir.join(file_name);
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(mut file) => {
                    for prior in &self.chunks {
                        // Deviation: a mid-stream write failure disables
                        // spilling instead of throwing out of a stream event.
                        if file.write_all(prior).is_err() {
                            self.spill_fd = None;
                            self.spill_file = None;
                            self.spill_disabled = true;
                            return;
                        }
                    }
                    self.spill_fd = Some(file);
                    self.spill_file = Some(path);
                }
                Err(_) => {
                    // Deviation: containment instead of the TS openSync throw.
                    self.spill_disabled = true;
                    return;
                }
            }
        }
        if let Some(fd) = &mut self.spill_fd {
            if fd.write_all(chunk).is_err() {
                self.discard_spill();
            }
        }
    }

    /// Stop spilling and remove the file once it can no longer hold the
    /// complete stream (TS `discardSpill`). The failed-close retry collapses
    /// into a plain drop (Rust cannot observe `close(2)` errors).
    fn discard_spill(&mut self) {
        let file = self.spill_file.take();
        self.spill_fd = None;
        self.spill_disabled = true;
        if let Some(file) = file {
            // A failed unlink leaves at most maxSpillBytes behind, never an
            // unbounded file.
            let _ = std::fs::remove_file(file);
        }
    }

    /// Incremental read in whole-stream byte coordinates: returns everything
    /// pushed since `from_byte`. When `from_byte` has already slid out of the
    /// in-memory tail window, the read is `lossy` — it returns the whole
    /// retained tail and the gap is only recoverable from the spill file.
    pub fn read_from(&self, from_byte: u64) -> SubprocessOutputRead {
        let window_start = self.total - self.bytes as u64;
        let mut buffer = Vec::with_capacity(self.bytes);
        for chunk in &self.chunks {
            buffer.extend_from_slice(chunk);
        }
        let lossy = from_byte < window_start;
        let text = if lossy {
            &buffer[..]
        } else {
            &buffer[(from_byte - window_start) as usize..]
        };
        SubprocessOutputRead {
            text: String::from_utf8_lossy(text).into_owned(),
            next_offset: self.total,
            lossy,
            spill_path: self
                .spill_file
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        }
    }

    /// Close the spill file once the stream has ended. Idempotent; the spawn
    /// path seals both collectors at settlement so reads after exit never
    /// point at a still-open file. The delayed-writeback failure arm
    /// collapses into a drop (TS `seal`).
    pub fn seal(&mut self) {
        if self.spill_fd.is_none() {
            return;
        }
        self.spill_fd = None;
    }

    /// Seal the spill file and return the final output (TS `finalize`).
    pub fn finalize(&mut self) -> CollectedOutput {
        self.seal();
        let mut buffer = Vec::with_capacity(self.bytes);
        for chunk in &self.chunks {
            buffer.extend_from_slice(chunk);
        }
        CollectedOutput {
            text: String::from_utf8_lossy(&buffer).into_owned(),
            truncated: self.dropped,
            spill_path: self
                .spill_file
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        }
    }
}

/// Offset-based access to one live collector (implements the seam's
/// [`SubprocessOutputReader`]).
pub struct CollectorReader {
    collector: Arc<Mutex<OutputCollector>>,
}

impl CollectorReader {
    pub fn new(collector: Arc<Mutex<OutputCollector>>) -> Self {
        Self { collector }
    }
}

impl SubprocessOutputReader for CollectorReader {
    fn read_from(&self, from_byte: u64) -> SubprocessOutputRead {
        self.collector.lock().read_from(from_byte)
    }
}

/// Terminate one Windows process tree with `taskkill /T /F`. Contained like
/// POSIX group signalling — delivery races tree exit, so an absent tree, a
/// nonzero status, or a missing taskkill binary must not break idempotent
/// teardown (TS `taskkillProcessTree`).
pub fn taskkill_process_tree(pid: i32) {
    if pid <= 0 {
        return;
    }
    // Outcome deliberately unchecked: an already-absent tree (status 128),
    // exit races, and a missing taskkill binary are as tolerable here as
    // ESRCH is for a POSIX group signal.
    use std::process::Stdio;
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Shared signalling/liveness state for one spawned tree. All methods are
/// synchronous and race-safe; the two background tasks (abort watcher,
/// tree-exit observer) plus the settlement task coordinate through it.
struct TreeShared {
    observed: AtomicBool,
    settled: AtomicBool,
    child_exited: AtomicBool,
    pid: i32,
    platform: String,
    grace_ms: u64,
    abort: Option<SubprocessAbort>,
    abort_reacted: AtomicBool,
    taskkill: Arc<dyn Fn(i32) + Send + Sync>,
    /// Linux-only group probe; unused on non-POSIX builds.
    #[cfg_attr(not(unix), allow(dead_code))]
    linux_live: Arc<dyn Fn(i32) -> Option<bool> + Send + Sync>,
    grace_timer: Mutex<Option<JoinHandle<()>>>,
    observer: OnceLock<Shared<BoxFuture<'static, ()>>>,
}

#[cfg(unix)]
fn sig_term() -> i32 {
    libc::SIGTERM
}

#[cfg(not(unix))]
fn sig_term() -> i32 {
    0
}

#[cfg(unix)]
fn sig_kill() -> i32 {
    libc::SIGKILL
}

#[cfg(not(unix))]
fn sig_kill() -> i32 {
    0
}

impl TreeShared {
    /// Whether the detached tree's root (or POSIX group) is still alive
    /// (TS `treeAlive`).
    fn tree_alive(&self) -> bool {
        if self.observed.load(SeqCst) {
            return false;
        }
        if self.pid <= 0 {
            return false;
        }
        if self.platform == "win32" {
            // Windows has no group-liveness probe; the direct child's exit is
            // the observable boundary (taskkill /T already took the tree with
            // it).
            return !self.child_exited.load(SeqCst);
        }
        #[cfg(unix)]
        {
            let rc = unsafe { libc::kill(-(self.pid), 0) };
            if rc == 0 {
                // A group containing only unreaped zombies still answers
                // kill(0), but it can execute no work. Only inspect after
                // direct-child settlement so live-process polls remain a
                // syscall rather than repeated process-table scans.
                if self.settled.load(SeqCst)
                    && self.platform == "linux"
                    && self.linux_live(self.pid) == Some(false)
                {
                    return false;
                }
                return true;
            }
            let code = std::io::Error::last_os_error().raw_os_error();
            // POSIX reports an absent group as ESRCH.
            if code == Some(libc::ESRCH) {
                return false;
            }
            // EPERM and non-POSIX negative-pid failures fall back to the
            // direct child's settlement facts.
            if code == Some(libc::EPERM) {
                return true;
            }
            !self.child_exited.load(SeqCst)
        }
        #[cfg(not(unix))]
        {
            !self.child_exited.load(SeqCst)
        }
    }

    /// Signal a detached process tree with platform-correct semantics: POSIX
    /// signals the negative process-group id and falls back to the direct
    /// child when the group is gone; Windows terminates the tree via taskkill
    /// (any signal value force-terminates) (TS `signalTree`).
    fn signal_tree(&self, sig: i32) {
        if self.platform == "win32" {
            (self.taskkill)(self.pid);
            return;
        }
        #[cfg(unix)]
        {
            if self.pid <= 0 {
                return;
            }
            unsafe {
                if libc::kill(-(self.pid), sig) != 0 {
                    // The fallback needs a live child whose group signal
                    // fails (EPERM-style); the swallow keeps teardown
                    // idempotent.
                    let _ = libc::kill(self.pid, sig);
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (self.pid, sig);
        }
    }

    /// The escalation's tier primitive (not on the handle — `terminate` is
    /// the only consumer-facing termination verb). Guards on TREE liveness,
    /// not outcome settlement (TS `kill`).
    fn kill_tier(&self, sig: i32) {
        if !self.tree_alive() {
            return;
        }
        self.signal_tree(sig);
    }

    /// Start or reuse the handle's single whole-tree exit observer. The first
    /// confirmed absence is a permanent no-more-signals boundary: it cancels
    /// a pending escalation before this process-group id can be reused (TS
    /// `observeTreeExit`).
    fn observe(self: &Arc<Self>) -> Shared<BoxFuture<'static, ()>> {
        self.observer
            .get_or_init(|| {
                let shared = self.clone();
                async move {
                    loop {
                        if !shared.tree_alive() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(SLEEP_TICK_MS)).await;
                    }
                    shared.observed.store(true, SeqCst);
                    if let Some(timer) = shared.grace_timer.lock().take() {
                        timer.abort();
                    }
                }
                .boxed()
                .shared()
            })
            .clone()
    }

    /// Begin the SIGTERM → `graceMs` → SIGKILL escalation on the process
    /// tree — the seam's only termination verb. Idempotent, a no-op once the
    /// tree is gone (TS `terminate`).
    fn terminate(self: &Arc<Self>) {
        if self.observed.load(SeqCst) || self.grace_timer.lock().is_some() {
            return;
        }
        // Observe from the first termination tier onward, even when inherited
        // pipes delay `done` and no consumer has begun its own teardown wait.
        let observed = self.observe();
        tokio::spawn(async move {
            observed.await;
        });
        if self.observed.load(SeqCst) {
            return;
        }
        self.kill_tier(sig_term());
        // The escalation must survive direct-child settlement — the leader
        // dying does not mean the tree died — so settlement does not clear
        // this timer, and kill_tier re-probes tree liveness before
        // force-killing. Self-bounds at graceMs.
        let shared = self.clone();
        let grace_ms = self.grace_ms;
        let timer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(grace_ms)).await;
            shared.kill_tier(sig_kill());
        });
        *self.grace_timer.lock() = Some(timer);
    }

    /// Synchronous final termination without starting timers or waits (TS
    /// `terminateForHostExit`).
    fn terminate_for_host_exit(&self) {
        self.kill_tier(sig_kill());
    }
}

/// The concrete local handle. Cheap to clone; every clone shares one tree.
#[derive(Clone)]
pub struct LocalHandle {
    inner: Arc<LocalHandleInner>,
}

struct LocalHandleInner {
    shared: Arc<TreeShared>,
    pid: i32,
    stdin: Mutex<Option<Box<dyn AsyncWrite + Unpin + Send>>>,
    stdout: Mutex<Option<Box<dyn AsyncRead + Unpin + Send>>>,
    stderr: Mutex<Option<Box<dyn AsyncRead + Unpin + Send>>>,
    stdout_collector: Option<Arc<Mutex<OutputCollector>>>,
    stderr_collector: Option<Arc<Mutex<OutputCollector>>>,
    done: Shared<BoxFuture<'static, Result<SubprocessOutcome, String>>>,
}

impl LocalHandle {
    /// Local-only synchronous final termination (absent from the public
    /// seam).
    pub fn terminate_for_host_exit(&self) {
        self.inner.shared.terminate_for_host_exit();
    }
}

impl SubprocessHandle for LocalHandle {
    fn pid(&self) -> i32 {
        self.inner.pid
    }

    fn stdin(&self) -> Option<Box<dyn AsyncWrite + Unpin + Send>> {
        self.inner.stdin.lock().take()
    }

    fn stdout(&self) -> Option<Box<dyn AsyncRead + Unpin + Send>> {
        self.inner.stdout.lock().take()
    }

    fn stderr(&self) -> Option<Box<dyn AsyncRead + Unpin + Send>> {
        self.inner.stderr.lock().take()
    }

    fn collected(&self) -> SubprocessCollectedOutputs {
        SubprocessCollectedOutputs {
            stdout: self.inner.stdout_collector.as_ref().map(|collector| {
                Arc::new(CollectorReader::new(collector.clone())) as Arc<dyn SubprocessOutputReader>
            }),
            stderr: self.inner.stderr_collector.as_ref().map(|collector| {
                Arc::new(CollectorReader::new(collector.clone())) as Arc<dyn SubprocessOutputReader>
            }),
        }
    }

    fn done(&self) -> BoxFuture<'static, Result<SubprocessOutcome, String>> {
        self.inner.done.clone().boxed()
    }

    fn terminate(&self) {
        self.inner.shared.terminate();
    }

    fn wait_for_exit(&self, signal: Option<SubprocessAbort>) -> BoxFuture<'static, bool> {
        let shared = self.inner.shared.clone();
        Box::pin(async move {
            let observed = shared.observe();
            if shared.observed.load(SeqCst) {
                return true;
            }
            let Some(signal) = signal else {
                observed.await;
                return true;
            };
            if signal() {
                return false;
            }
            let mut tick = tokio::time::interval(Duration::from_millis(SLEEP_TICK_MS));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = observed.clone() => return true,
                    _ = tick.tick() => {
                        if signal() {
                            return false;
                        }
                    }
                }
            }
        })
    }
}

impl LocalSubprocessHandle for LocalHandle {
    fn terminate_for_host_exit(&self) {
        LocalHandle::terminate_for_host_exit(self);
    }
}

/// Map a tokio exit status onto the seam's exit facts. Node reports signal
/// names; Rust reports numbers, so this module maps the common ones (TS
/// `SubprocessOutcome`).
fn outcome_from_status(status: std::process::ExitStatus) -> SubprocessOutcome {
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.signal().map(|sig| match sig {
            libc::SIGHUP => "SIGHUP".to_string(),
            libc::SIGINT => "SIGINT".to_string(),
            libc::SIGQUIT => "SIGQUIT".to_string(),
            libc::SIGILL => "SIGILL".to_string(),
            libc::SIGABRT => "SIGABRT".to_string(),
            libc::SIGFPE => "SIGFPE".to_string(),
            libc::SIGKILL => "SIGKILL".to_string(),
            libc::SIGSEGV => "SIGSEGV".to_string(),
            libc::SIGPIPE => "SIGPIPE".to_string(),
            libc::SIGTERM => "SIGTERM".to_string(),
            other => format!("SIG{other}"),
        })
    };
    #[cfg(not(unix))]
    let signal = None;
    SubprocessOutcome {
        exit_code: status.code(),
        signal,
    }
}

/// Read one collect-mode pipe to EOF: phase 1 races direct-child exit (via
/// the watch) so pipe-buffer backpressure never stalls the child; phase 2
/// drains the remainder bounded by `graceMs` (the TS pipe drain timer).
fn spawn_collect_reader<R>(
    mut stream: R,
    collector: Arc<Mutex<OutputCollector>>,
    mut exit_rx: tokio::sync::watch::Receiver<bool>,
    grace_ms: u64,
) -> JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buf = vec![0u8; 64 * 1024];
        // Phase 1: read while the direct child runs.
        loop {
            if *exit_rx.borrow() {
                break;
            }
            tokio::select! {
                result = stream.read(&mut buf) => match result {
                    Ok(0) | Err(_) => return,
                    Ok(n) => collector.lock().push(&buf[..n]),
                },
                changed = exit_rx.changed() => {
                    // A closed sender means the settlement future was dropped;
                    // fall through to the bounded drain.
                    if changed.is_err() {
                        break;
                    }
                },
            }
        }
        // Phase 2: a surviving descendant that inherited a pipe must not
        // hold the outcome open indefinitely: after exit, the same bounded
        // grace that governs kills also bounds the close wait.
        let drain = async {
            loop {
                match stream.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => collector.lock().push(&buf[..n]),
                }
            }
        };
        if tokio::time::timeout(Duration::from_millis(grace_ms), drain)
            .await
            .is_err()
        {
            // The drain boundary: force-close our end so no further chunks
            // land after settlement seals the collector.
            drop(stream);
        }
    })
}

/// Spawn one isolated detached process tree with the spec's per-stream stdio
/// dispositions. Runtime exits resolve `done` as
/// [`SubprocessOutcome`]; only spawn failures reject (TS
/// `spawnSubprocess`). Must be called inside a tokio runtime (the TS spawn
/// needs no runtime context, but our reader/abort/observer tasks do).
pub fn spawn_subprocess(
    spec: SubprocessSpawnSpec,
    internals: SpawnInternals,
) -> Result<LocalHandle, String> {
    if spec.grace_ms == 0 || spec.grace_ms > MAX_TIMER_DELAY_MS {
        return Err(format!(
            "subprocess graceMs must be a positive finite number no greater than {MAX_TIMER_DELAY_MS}"
        ));
    }
    let spill_dir = internals.spill_dir.unwrap_or_else(private_spill_dir);
    let platform = internals
        .platform
        .unwrap_or_else(|| host_platform().to_string());
    let taskkill: Arc<dyn Fn(i32) + Send + Sync> = internals
        .taskkill
        .unwrap_or_else(|| Arc::new(taskkill_process_tree));
    let linux_live: Arc<dyn Fn(i32) -> Option<bool> + Send + Sync> = internals
        .linux_group_has_live_members
        .unwrap_or_else(|| Arc::new(default_linux_group_live));

    if let Some(signal) = &spec.signal {
        if signal() {
            return Err("aborted before spawn: aborted".to_string());
        }
    }
    let Some(program) = spec.argv.first() else {
        return Err("invalid argv: expected a non-empty program name at argv[0]".to_string());
    };
    if program.is_empty() {
        return Err("invalid argv: expected a non-empty program name at argv[0]".to_string());
    }

    let SubprocessStdio {
        stdin: stdin_mode,
        stdout: out_mode,
        stderr: err_mode,
    } = spec.stdio;

    let env = child_env(spec.env.as_deref());

    let mut command = Command::new(program);
    command
        .args(&spec.argv[1..])
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(env)
        .stdin(match &stdin_mode {
            SubprocessStdinMode::Ignore => std::process::Stdio::null(),
            _ => std::process::Stdio::piped(),
        })
        .stdout(match &out_mode {
            SubprocessOutputMode::Inherit => std::process::Stdio::inherit(),
            _ => std::process::Stdio::piped(),
        })
        .stderr(match &err_mode {
            SubprocessOutputMode::Inherit => std::process::Stdio::inherit(),
            _ => std::process::Stdio::piped(),
        });
    // `detached` gives teardown a tree root on POSIX (its own process
    // group); Windows terminates by root pid through taskkill /T instead.
    #[cfg(unix)]
    command.process_group(0);
    // Deviation: the direct child is killed when the settlement future drops
    // (the service keeps live handles until whole-tree exit, so managed
    // disposal is unaffected).
    command.kill_on_drop(true);

    let mut child: Child = command
        .spawn()
        .map_err(|error| format!("subprocess-local: failed to spawn {program}: {error}"))?;
    let pid = child.id().map(|id| id as i32).unwrap_or(-1);

    let mut stdin_stream = child.stdin.take();
    let mut stdout_stream = child.stdout.take();
    let mut stderr_stream = child.stderr.take();

    let (exit_tx, exit_rx) = tokio::sync::watch::channel(false);

    let stdout_collector = match &out_mode {
        SubprocessOutputMode::Collect(mode) => Some(Arc::new(Mutex::new(OutputCollector::new(
            mode.max_bytes as usize,
            mode.spill.as_ref().map(|spill| spill.max_bytes),
            "stdout",
            spill_dir.clone(),
        )))),
        _ => None,
    };
    let stdout_reader = match &out_mode {
        SubprocessOutputMode::Collect(_) => match stdout_stream.take() {
            Some(stream) => stdout_collector.as_ref().map(|collector| {
                spawn_collect_reader(stream, collector.clone(), exit_rx.clone(), spec.grace_ms)
            }),
            None => None,
        },
        _ => None,
    };
    let stderr_collector = match &err_mode {
        SubprocessOutputMode::Collect(mode) => Some(Arc::new(Mutex::new(OutputCollector::new(
            mode.max_bytes as usize,
            mode.spill.as_ref().map(|spill| spill.max_bytes),
            "stderr",
            spill_dir.clone(),
        )))),
        _ => None,
    };
    let stderr_reader = match &err_mode {
        SubprocessOutputMode::Collect(_) => match stderr_stream.take() {
            Some(stream) => stderr_collector.as_ref().map(|collector| {
                spawn_collect_reader(stream, collector.clone(), exit_rx.clone(), spec.grace_ms)
            }),
            None => None,
        },
        _ => None,
    };

    let shared = Arc::new(TreeShared {
        observed: AtomicBool::new(false),
        settled: AtomicBool::new(false),
        child_exited: AtomicBool::new(false),
        pid,
        platform: platform.clone(),
        grace_ms: spec.grace_ms,
        abort: spec.signal.clone(),
        abort_reacted: AtomicBool::new(false),
        taskkill,
        linux_live,
        grace_timer: Mutex::new(None),
        observer: OnceLock::new(),
    });

    // The caller owns timeout classification; this layer only reacts to
    // abort. The predicate is polled because Rust has no abort-event target.
    if shared.abort.is_some() {
        let watcher = shared.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(SLEEP_TICK_MS)).await;
                if watcher.settled.load(SeqCst) {
                    return;
                }
                let Some(abort) = &watcher.abort else {
                    return;
                };
                if abort() {
                    // React exactly once (the TS `{ once: true }` listener).
                    if !watcher.abort_reacted.swap(true, SeqCst) {
                        watcher.terminate();
                    }
                    return;
                }
            }
        });
    }

    // Batch stdin is written and closed up front; process exit and captured
    // output remain authoritative, so write errors (EPIPE) are best-effort.
    if let SubprocessStdinMode::Data(data) = &stdin_mode {
        if let Some(mut stdin) = stdin_stream.take() {
            let data = data.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(data.as_bytes()).await;
                let _ = stdin.shutdown().await;
            });
        }
    }

    let done: Shared<BoxFuture<'static, Result<SubprocessOutcome, String>>> = {
        let shared = shared.clone();
        let stdout_collector = stdout_collector.clone();
        let stderr_collector = stderr_collector.clone();
        async move {
            let mut child = child;
            let status = child
                .wait()
                .await
                .map_err(|error| format!("subprocess-local: wait failed: {error}"))?;
            shared.child_exited.store(true, SeqCst);
            let _ = exit_tx.send(true);
            // Both readers self-bound at exit + graceMs, so settlement can
            // join them before sealing: no chunk ever lands after a seal.
            if let Some(reader) = stdout_reader {
                let _ = reader.await;
            }
            if let Some(reader) = stderr_reader {
                let _ = reader.await;
            }
            if let Some(collector) = &stdout_collector {
                collector.lock().seal();
            }
            if let Some(collector) = &stderr_collector {
                collector.lock().seal();
            }
            shared.settled.store(true, SeqCst);
            Ok(outcome_from_status(status))
        }
        .boxed()
        .shared()
    };

    // The TS child's own events drive `done` settlement with no consumer;
    // Rust futures are inert until polled, so poll from spawn time (liveness
    // probes and the abort watcher depend on the settlement flags).
    {
        let driven = done.clone();
        tokio::spawn(async move {
            let _ = driven.await;
        });
    }

    Ok(LocalHandle {
        inner: Arc::new(LocalHandleInner {
            shared,
            pid,
            stdin: Mutex::new(
                stdin_stream.map(|stream| Box::new(stream) as Box<dyn AsyncWrite + Unpin + Send>),
            ),
            stdout: Mutex::new(
                stdout_stream.map(|stream| Box::new(stream) as Box<dyn AsyncRead + Unpin + Send>),
            ),
            stderr: Mutex::new(
                stderr_stream.map(|stream| Box::new(stream) as Box<dyn AsyncRead + Unpin + Send>),
            ),
            stdout_collector,
            stderr_collector,
            done,
        }),
    })
}

/// Default Linux group-liveness probe: `false` means the group contains only
/// zombie/dead entries; `None` means the process table could not prove either
/// outcome (TS `linuxProcessGroupHasLiveMembers`).
pub fn default_linux_group_live(process_group_id: i32) -> Option<bool> {
    let entries = std::fs::read_dir("/proc").ok()?;
    let mut matched = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Some(stat) = parse_proc_stat(&name) else {
            continue;
        };
        if stat.pgrp != process_group_id {
            continue;
        }
        matched = true;
        if !matches!(stat.state, 'Z' | 'X' | 'x') {
            return Some(true);
        }
    }
    if matched { Some(false) } else { None }
}

struct ProcStat {
    pgrp: i32,
    state: char,
}

/// Parse the fields used from Linux `/proc/<pid>/stat`, including the
/// parenthesized comm text (TS `parseProcStat`).
fn parse_proc_stat(pid: &str) -> Option<ProcStat> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if open == 0 || close <= open {
        return None;
    }
    let _parsed_pid: i32 = text[..open].trim().parse().ok()?;
    let rest: Vec<&str> = text[close + 2..].split_whitespace().collect();
    let state = rest.first()?.chars().next()?;
    if rest.len() <= 19 {
        return None;
    }
    let pgrp: i32 = rest.get(2)?.parse().ok()?;
    Some(ProcStat { pgrp, state })
}
