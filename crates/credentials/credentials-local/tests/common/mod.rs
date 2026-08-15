//! Shared test helpers for the credentials-local suite: temp roots, boot,
//! the fake watcher registry, and the gated writer (Rust ports of the TS
//! spec harness pieces).

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cordis::Context;
use parking_lot::Mutex;

use dsh_credentials_local::{
    Config, DocumentReader, DocumentWriter, LocalCredentialProvider, WatchControl, WatchSignal,
    WatchSink, WatcherFactory,
};

/// A per-test temp root that cleans itself up.
pub struct TempRoot(pub PathBuf);

impl TempRoot {
    pub fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "dsh-credentials-local-rs-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&base).expect("temp root");
        Self(base)
    }

    pub fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Credential documents are seeded owner-only, exactly as the provider
/// creates them.
pub fn write_credentials(file: &str, text: &str) {
    std::fs::write(file, text).expect("write credentials");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600));
    }
}

/// Boot the provider over one document path; returns the ctx and the
/// provider handle.
pub fn boot(path: &str, watch: bool) -> (Context, Arc<LocalCredentialProvider>) {
    let ctx = Context::root();
    let provider = LocalCredentialProvider::install(
        &ctx,
        Config { path: Some(path.to_string()), watch: Some(watch), ..Default::default() },
    )
    .expect("boot");
    (ctx, provider)
}

/// Boot with the seam-local injections (gated writer / fake watcher / fake
/// reader).
pub fn boot_with(
    ctx: &Context,
    path: &str,
    watch: bool,
    writer: DocumentWriter,
    watcher_factory: Option<WatcherFactory>,
    reader: Option<DocumentReader>,
) -> Result<Arc<LocalCredentialProvider>, String> {
    LocalCredentialProvider::install_with_seams(
        ctx,
        Config { path: Some(path.to_string()), watch: Some(watch), ..Default::default() },
        writer,
        watcher_factory,
        reader.unwrap_or_else(default_reader),
    )
}

/// A poll-until helper (the TS `vi.waitFor` equivalent for value checks).
pub async fn wait_for<F: Fn() -> bool>(what: &str, check: F, timeout_ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        if check() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("wait_for({what}) timed out");
}

/// One fake watcher instance recorded by [`fake_watcher_factory`].
pub struct FakeWatcherInstance {
    pub target: String,
    pub debounce: u64,
    pub sink: Arc<dyn WatchSink>,
}

/// The global fake-instance registry (the TS `chokidar.__instances`).
pub fn fake_instances() -> &'static Mutex<Vec<FakeWatcherInstance>> {
    static INSTANCES: std::sync::OnceLock<Mutex<Vec<FakeWatcherInstance>>> =
        std::sync::OnceLock::new();
    INSTANCES.get_or_init(|| Mutex::new(Vec::new()))
}

/// A watcher factory recording instances and driving the sink directly.
pub fn fake_watcher_factory() -> WatcherFactory {
    Arc::new(|target: String, sink: Arc<dyn WatchSink>, debounce: u64| {
        Box::pin(async move {
            fake_instances().lock().push(FakeWatcherInstance { target, debounce, sink });
            Ok(Arc::new(FakeControl) as Arc<dyn WatchControl>)
        })
    })
}

struct FakeControl;

impl WatchControl for FakeControl {
    fn close(&self) -> futures::future::BoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

/// Send one signal through the fake watcher instance whose canonical target
/// matches this test's document path (the shared registry makes instance
/// selection per-test-path safe under parallel test threads).
pub fn emit_to(path: &str, signal: WatchSignal) {
    let instances = fake_instances().lock();
    let instance = instances
        .iter()
        .find(|instance| instance.target.trim_start_matches(r"\\?\") == path)
        .unwrap_or_else(|| panic!("no fake watcher instance for {path}"));
    instance.sink.on_signal(signal);
}

/// The injected document-reader seam (the crate's `DocumentReader`).
pub fn default_reader() -> DocumentReader {
    Arc::new(|path: &Path| {
        let path = path.to_path_buf();
        Box::pin(async move { tokio::fs::read_to_string(path).await })
    })
}

/// The default durable writer (for tests that only gate it).
pub fn real_writer() -> DocumentWriter {
    Arc::new(|path: &Path, content: &[u8]| {
        let path = path.to_path_buf();
        let content = content.to_vec();
        Box::pin(async move {
            dsh_atomic_write::write_file_atomic(
                &path,
                &content,
                dsh_atomic_write::WriteFileAtomicOptions { mode: 0o600, dir_mode: Some(0o700) },
            )
            .await
            .map_err(|error| error.to_string())
        })
    })
}

/// A writer whose first in-flight operation parks on a caller-held gate.
pub struct GatedWriter {
    gate: Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
}

impl GatedWriter {
    pub fn new() -> Self {
        Self { gate: Arc::new(tokio::sync::Mutex::new(None)) }
    }

    pub fn writer(&self) -> DocumentWriter {
        let gate = self.gate.clone();
        Arc::new(move |path: &Path, content: &[u8]| {
            let path = path.to_path_buf();
            let content = content.to_vec();
            let gate = gate.clone();
            Box::pin(async move {
                let receiver = { gate.lock().await.take() };
                if let Some(receiver) = receiver {
                    let _ = receiver.await;
                }
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

    /// Arm the gate: the next write parks until the returned sender fires.
    pub fn arm(&self) -> tokio::sync::oneshot::Sender<()> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        futures::executor::block_on(async {
            *self.gate.lock().await = Some(receiver);
        });
        sender
    }
}
