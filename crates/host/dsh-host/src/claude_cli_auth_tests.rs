use super::*;
use dsh_subprocess::{
    SubprocessAbort, SubprocessCollectedOutputs, SubprocessOutcome, SubprocessOutputRead,
    SubprocessOutputReader, SubprocessTerminalHandle, SubprocessTerminalSpawnSpec,
};
use futures::future::BoxFuture;

struct Reader(String);
impl SubprocessOutputReader for Reader {
    fn read_from(&self, _: u64) -> SubprocessOutputRead {
        SubprocessOutputRead {
            text: self.0.clone(),
            next_offset: self.0.len() as u64,
            lossy: false,
            spill_path: None,
        }
    }
}
struct Handle {
    login: bool,
    terminated: Arc<AtomicBool>,
    complete: Arc<AtomicBool>,
    signed_in: Arc<AtomicBool>,
}
impl SubprocessHandle for Handle {
    fn pid(&self) -> i32 {
        1
    }
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
        SubprocessCollectedOutputs {stdout:Some(Arc::new(Reader(json!({"loggedIn":self.signed_in.load(Ordering::SeqCst),"secret":"never-forward-this-field"}).to_string()))),stderr:None}
    }
    fn done(&self) -> BoxFuture<'static, Result<SubprocessOutcome, String>> {
        let login = self.login;
        let terminated = self.terminated.clone();
        let complete = self.complete.clone();
        let signed_in = self.signed_in.clone();
        Box::pin(async move {
            if !login {
                return Ok(SubprocessOutcome {
                    exit_code: Some(if signed_in.load(Ordering::SeqCst) {
                        0
                    } else {
                        1
                    }),
                    signal: None,
                });
            }
            loop {
                if terminated.load(Ordering::SeqCst) {
                    return Ok(SubprocessOutcome {
                        exit_code: Some(1),
                        signal: None,
                    });
                }
                if complete.load(Ordering::SeqCst) {
                    signed_in.store(true, Ordering::SeqCst);
                    return Ok(SubprocessOutcome {
                        exit_code: Some(0),
                        signal: None,
                    });
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
    }
    fn terminate(&self) {
        self.terminated.store(true, Ordering::SeqCst)
    }
    fn wait_for_exit(&self, _: Option<SubprocessAbort>) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
}
#[derive(Default)]
struct Runtime {
    missing: bool,
    calls: parking_lot::Mutex<Vec<Vec<String>>>,
    terminated: Arc<AtomicBool>,
    complete: Arc<AtomicBool>,
    signed_in: Arc<AtomicBool>,
}
impl SubprocessRuntime for Runtime {
    fn resolve_executable(
        &self,
        command: &str,
        _: Option<&[(String, String)]>,
        _: Option<SubprocessAbort>,
    ) -> BoxFuture<'static, Result<String, String>> {
        assert_eq!(command, "claude");
        let missing = self.missing;
        Box::pin(async move {
            if missing {
                Err("not found".into())
            } else {
                Ok(if cfg!(windows) {
                    "C:/official/claude.exe"
                } else {
                    "/official/claude"
                }
                .into())
            }
        })
    }
    fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<Arc<dyn SubprocessHandle>, String> {
        assert_eq!(spec.argv[1], "auth");
        assert!(matches!(spec.argv[2].as_str(), "status" | "login"));
        assert!(matches!(spec.stdio.stdin, SubprocessStdinMode::Ignore));
        assert!(spec.env.is_none());
        let login = spec.argv[2] == "login";
        if login {
            assert!(matches!(
                spec.stdio.stdout,
                SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: 1,
                    spill: None
                })
            ));
        }
        self.calls.lock().push(spec.argv);
        Ok(Arc::new(Handle {
            login,
            terminated: self.terminated.clone(),
            complete: self.complete.clone(),
            signed_in: self.signed_in.clone(),
        }))
    }
    fn spawn_terminal(
        &self,
        _: SubprocessTerminalSpawnSpec,
    ) -> BoxFuture<'static, Result<Arc<dyn SubprocessTerminalHandle>, String>> {
        Box::pin(async { Err("no terminal allowed".into()) })
    }
}
fn manager(runtime: Arc<Runtime>) -> Arc<ClaudeCliAuth> {
    ClaudeCliAuth::new(runtime, std::env::temp_dir().to_string_lossy().into())
}

#[tokio::test]
async fn missing_cli_is_explicit_and_status_is_read_only_without_output_leaks() {
    let runtime = Arc::new(Runtime {
        missing: true,
        ..Default::default()
    });
    let auth = manager(runtime.clone());
    let status = auth.status().await;
    assert_eq!(status["installed"], false);
    assert_eq!(status["scope"], "subagent");
    assert_eq!(status["installUrl"], INSTALL_URL);
    assert!(runtime.calls.lock().is_empty());
    let runtime = Arc::new(Runtime::default());
    runtime.signed_in.store(true, Ordering::SeqCst);
    let auth = manager(runtime.clone());
    let status = auth.status().await;
    assert_eq!(status["signedIn"], true);
    assert_eq!(runtime.calls.lock().len(), 1);
    assert_eq!(runtime.calls.lock()[0][2], "status");
    assert!(!status.to_string().contains("never-forward"));
}

#[tokio::test]
async fn repeated_login_clicks_share_one_process_and_cancel_terminates_it() {
    let runtime = Arc::new(Runtime::default());
    let auth = manager(runtime.clone());
    let first = auth.start().await.unwrap();
    let second = auth.start().await.unwrap();
    assert_eq!(first["attempt"], second["attempt"]);
    assert_eq!(runtime.calls.lock().len(), 1);
    let id = first["attempt"].as_str().unwrap();
    assert!(id.starts_with(PREFIX));
    let cancel = auth.cancel(id).await.unwrap();
    assert_eq!(cancel["status"], "cancelled");
    assert!(runtime.terminated.load(Ordering::SeqCst));
    assert_eq!(auth.poll(id).await.unwrap()["status"], "cancelled");
}

#[tokio::test]
async fn terminal_dependent_login_is_bounded_and_late_completion_is_reported() {
    let runtime = Arc::new(Runtime::default());
    let mut auth = manager(runtime.clone());
    Arc::get_mut(&mut auth).unwrap().login_timeout = Duration::from_millis(3);
    let result = auth.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        auth.poll(result["attempt"].as_str().unwrap())
            .await
            .unwrap_err()
            .contains("超时")
    );
    assert!(runtime.terminated.load(Ordering::SeqCst));
    let runtime = Arc::new(Runtime::default());
    let auth = manager(runtime.clone());
    let result = auth.start().await.unwrap();
    runtime.signed_in.store(true, Ordering::SeqCst);
    assert_eq!(
        auth.cancel(result["attempt"].as_str().unwrap())
            .await
            .unwrap()["status"],
        "complete"
    );
}
