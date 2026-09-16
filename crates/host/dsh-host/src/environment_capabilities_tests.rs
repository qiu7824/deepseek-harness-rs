use super::*;
use dsh_subprocess::{
    SubprocessAbort, SubprocessCollectedOutputs, SubprocessOutcome, SubprocessOutputRead,
    SubprocessOutputReader, SubprocessTerminalHandle, SubprocessTerminalSpawnSpec,
};
use futures::future::BoxFuture;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Reader(String);
impl SubprocessOutputReader for Reader {
    fn read_from(&self, _: u64) -> SubprocessOutputRead {
        SubprocessOutputRead {
            text: self.0.clone(),
            next_offset: 0,
            lossy: false,
            spill_path: None,
        }
    }
}
struct Child {
    killed: Arc<AtomicBool>,
    hang: bool,
}
impl SubprocessHandle for Child {
    fn stdin(&self) -> Option<Box<dyn tokio::io::AsyncWrite + Unpin + Send>> {
        None
    }
    fn stdout(&self) -> Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>> {
        None
    }
    fn stderr(&self) -> Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>> {
        None
    }
    fn collected(&self) -> SubprocessCollectedOutputs {
        SubprocessCollectedOutputs {
            stdout: Some(Arc::new(Reader("git version 2.50.0".into()))),
            stderr: None,
        }
    }
    fn done(&self) -> BoxFuture<'static, Result<SubprocessOutcome, String>> {
        let hang = self.hang;
        Box::pin(async move {
            if hang {
                std::future::pending::<()>().await;
            }
            tokio::task::yield_now().await;
            Ok(SubprocessOutcome {
                exit_code: Some(0),
                signal: None,
            })
        })
    }
    fn terminate(&self) {
        self.killed.store(true, Ordering::SeqCst);
    }
    fn wait_for_exit(&self, _: Option<SubprocessAbort>) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
}
struct Runtime {
    path: String,
    count: AtomicUsize,
    missing: bool,
    hang: bool,
    killed: Arc<AtomicBool>,
}
impl SubprocessRuntime for Runtime {
    fn resolve_executable(
        &self,
        _: &str,
        _: Option<&[(String, String)]>,
        _: Option<SubprocessAbort>,
    ) -> BoxFuture<'static, Result<String, String>> {
        self.count.fetch_add(1, Ordering::SeqCst);
        let path = self.path.clone();
        let missing = self.missing;
        Box::pin(async move {
            if missing {
                Err("missing".into())
            } else {
                Ok(path)
            }
        })
    }
    fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<Arc<dyn SubprocessHandle>, String> {
        assert_eq!(spec.argv[1], "--version");
        Ok(Arc::new(Child {
            killed: self.killed.clone(),
            hang: self.hang,
        }))
    }
    fn spawn_terminal(
        &self,
        _: SubprocessTerminalSpawnSpec,
    ) -> BoxFuture<'static, Result<Arc<dyn SubprocessTerminalHandle>, String>> {
        Box::pin(async { Err("unused".into()) })
    }
}
struct Fixture {
    root: PathBuf,
    paths: Arc<RuntimePaths>,
    runtime: Arc<Runtime>,
}
impl Fixture {
    fn new(missing: bool, hang: bool) -> Self {
        let root = std::env::temp_dir().join(format!("dsh-capability-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let executable = root.join("fake-git");
        std::fs::write(&executable, "binary").unwrap();
        let paths = RuntimePaths::prepare_with_install_anchor(&root, None).unwrap();
        let runtime = Arc::new(Runtime {
            path: executable.to_string_lossy().into_owned(),
            count: AtomicUsize::new(0),
            missing,
            hang,
            killed: Arc::new(AtomicBool::new(false)),
        });
        Self {
            root,
            paths,
            runtime,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.paths.release();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn single_flight_persistence_and_binary_changes() {
    let fixture = Fixture::new(false, false);
    let cache = EnvironmentCapabilities::new(fixture.runtime.clone(), fixture.paths.clone());
    let (first, second) = tokio::join!(
        cache.inspect("git", false, Arc::new(|| false)),
        cache.inspect("git", false, Arc::new(|| false))
    );
    assert_eq!(first.unwrap()["status"], "ready");
    assert_eq!(second.unwrap()["cacheHit"], true);
    assert_eq!(fixture.runtime.count.load(Ordering::SeqCst), 1);
    let resumed = EnvironmentCapabilities::new(fixture.runtime.clone(), fixture.paths.clone());
    assert_eq!(
        resumed
            .inspect("git", false, Arc::new(|| false))
            .await
            .unwrap()["cacheHit"],
        true
    );
    std::fs::write(&fixture.runtime.path, "new longer binary").unwrap();
    assert_eq!(
        resumed
            .inspect("git", false, Arc::new(|| false))
            .await
            .unwrap()["cacheHit"],
        false
    );
    assert_eq!(fixture.runtime.count.load(Ordering::SeqCst), 2);
    assert_eq!(
        resumed
            .inspect("git", true, Arc::new(|| false))
            .await
            .unwrap()["cacheHit"],
        false
    );
}

#[tokio::test]
async fn negative_cache_expires_and_corrupt_storage_recovers() {
    let fixture = Fixture::new(true, false);
    let cache = EnvironmentCapabilities::new(fixture.runtime.clone(), fixture.paths.clone());
    assert_eq!(
        cache
            .inspect("git", false, Arc::new(|| false))
            .await
            .unwrap()["status"],
        "missing"
    );
    assert_eq!(
        cache
            .inspect("git", false, Arc::new(|| false))
            .await
            .unwrap()["cacheHit"],
        true
    );
    cache.records.lock().get_mut("git").unwrap().checked_at = now() - NEGATIVE_TTL;
    assert_eq!(
        cache
            .inspect("git", false, Arc::new(|| false))
            .await
            .unwrap()["cacheHit"],
        false
    );
    std::fs::write(&cache.cache_path, "{invalid").unwrap();
    let recovered = EnvironmentCapabilities::new(fixture.runtime.clone(), fixture.paths.clone());
    assert!(recovered.records.lock().is_empty());
    assert!(
        recovered
            .inspect("git", false, Arc::new(|| false))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn cancellation_terminates_process_and_does_not_cache_failure() {
    let fixture = Fixture::new(false, true);
    let cache = EnvironmentCapabilities::new(fixture.runtime.clone(), fixture.paths.clone());
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let request = cache.inspect("git", false, Arc::new(move || flag.load(Ordering::SeqCst)));
    let cancel = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancelled.store(true, Ordering::SeqCst);
    };
    let (result, _) = tokio::join!(request, cancel);
    assert!(result.is_err());
    assert!(fixture.runtime.killed.load(Ordering::SeqCst));
    assert!(cache.records.lock().is_empty());
}

#[tokio::test]
async fn timeout_is_bounded_and_negative_result_is_reused() {
    let fixture = Fixture::new(false, true);
    let cache = EnvironmentCapabilities::new(fixture.runtime.clone(), fixture.paths.clone());
    assert_eq!(
        cache
            .inspect("git", false, Arc::new(|| false))
            .await
            .unwrap()["status"],
        "timeout"
    );
    assert!(fixture.runtime.killed.load(Ordering::SeqCst));
    assert_eq!(
        cache
            .inspect("git", false, Arc::new(|| false))
            .await
            .unwrap()["cacheHit"],
        true
    );
}

#[tokio::test]
async fn simultaneous_refresh_requests_share_one_probe() {
    let fixture = Fixture::new(false, false);
    let cache = EnvironmentCapabilities::new(fixture.runtime.clone(), fixture.paths.clone());
    cache
        .inspect("git", false, Arc::new(|| false))
        .await
        .unwrap();
    let (first, second) = tokio::join!(
        cache.inspect("git", true, Arc::new(|| false)),
        cache.inspect("git", true, Arc::new(|| false))
    );
    assert_eq!(first.unwrap()["cacheHit"], false);
    assert_eq!(second.unwrap()["cacheHit"], true);
    assert_eq!(fixture.runtime.count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
#[ignore = "requires installed Git; explicit local integration check"]
async fn real_host_git_probe_reuses_verified_path_after_restart() {
    let fixture = Fixture::new(false, false);
    let ctx = Context::root();
    let runtime = dsh_subprocess_local::LocalSubprocessRuntime::install(&ctx);
    let cache = EnvironmentCapabilities::new(runtime.clone(), fixture.paths.clone());
    let first = cache
        .inspect("git", false, Arc::new(|| false))
        .await
        .unwrap();
    assert_eq!(first["status"], "ready", "{first}");
    assert!(
        first["version"]
            .as_str()
            .unwrap()
            .starts_with("git version")
    );
    let restored = EnvironmentCapabilities::new(runtime, fixture.paths.clone());
    assert_eq!(
        restored
            .inspect("git", false, Arc::new(|| false))
            .await
            .unwrap()["cacheHit"],
        true
    );
}

#[test]
fn environment_clock_and_file_identity_invalidate_records() {
    let record = Record {
        check_id: String::new(),
        environment: "one".into(),
        checked_at: 100,
        identity: None,
        dependencies: BTreeMap::new(),
        result: json!({"status":"missing","path":null}),
    };
    assert!(record.valid("one", 120));
    assert!(!record.valid("two", 120));
    assert!(!record.valid("one", 99));
    assert!(!record.valid("one", 160));
}
