use super::*;
use dsh_subprocess::{
    SubprocessAbort, SubprocessCollectedOutputs, SubprocessOutcome, SubprocessOutputRead,
    SubprocessOutputReader, SubprocessTerminalHandle, SubprocessTerminalSpawnSpec,
};
use futures::future::BoxFuture;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

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
    text: String,
    code: i32,
    hang: bool,
    killed: Arc<AtomicBool>,
}
impl SubprocessHandle for Child {
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
        SubprocessCollectedOutputs {
            stdout: Some(Arc::new(Reader(self.text.clone()))),
            stderr: None,
        }
    }
    fn done(&self) -> BoxFuture<'static, Result<SubprocessOutcome, String>> {
        let code = self.code;
        let hang = self.hang;
        Box::pin(async move {
            if hang {
                std::future::pending::<()>().await
            }
            Ok(SubprocessOutcome {
                exit_code: Some(code),
                signal: None,
            })
        })
    }
    fn terminate(&self) {
        self.killed.store(true, Ordering::SeqCst)
    }
    fn wait_for_exit(&self, _: Option<SubprocessAbort>) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
}
struct Runtime {
    missing: bool,
    version: &'static str,
    capabilities: Value,
    hang: bool,
    killed: Arc<AtomicBool>,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            missing: false,
            version: "v24.1.0",
            capabilities: json!({"permission":true,"typescriptStrip":true,"worker":true}),
            hang: false,
            killed: Arc::new(AtomicBool::new(false)),
        }
    }
}
impl SubprocessRuntime for Runtime {
    fn resolve_executable(
        &self,
        _: &str,
        _: Option<&[(String, String)]>,
        _: Option<SubprocessAbort>,
    ) -> BoxFuture<'static, Result<String, String>> {
        let missing = self.missing;
        Box::pin(async move {
            if missing {
                Err("missing".into())
            } else {
                Ok("/resolved/node".into())
            }
        })
    }
    fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<Arc<dyn SubprocessHandle>, String> {
        assert!(matches!(spec.stdio.stdin, SubprocessStdinMode::Ignore));
        assert_eq!(spec.env, Some(vec![("NODE_OPTIONS".into(), None)]));
        let version = spec.argv[1] == "--version";
        Ok(Arc::new(Child {
            text: if version {
                self.version.into()
            } else {
                self.capabilities.to_string()
            },
            code: 0,
            hang: self.hang,
            killed: self.killed.clone(),
        }))
    }
    fn spawn_terminal(
        &self,
        _: SubprocessTerminalSpawnSpec,
    ) -> BoxFuture<'static, Result<Arc<dyn SubprocessTerminalHandle>, String>> {
        Box::pin(async { Err("unexpected terminal".into()) })
    }
}

#[tokio::test]
async fn diagnostics_distinguish_missing_incompatible_invalid_and_ready() {
    let cwd = std::env::temp_dir();
    let ready = probe(Arc::new(Runtime::default()), "node", &cwd).await;
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["path"], "/resolved/node");
    assert_eq!(ready["version"], "v24.1.0");
    assert_eq!(ready["source"], "path");
    assert_eq!(ready["available"], true);
    let missing = probe(
        Arc::new(Runtime {
            missing: true,
            ..Default::default()
        }),
        "node",
        &cwd,
    )
    .await;
    assert_eq!(missing["status"], "missing");
    assert_eq!(missing["available"], false);
    let invalid = probe(
        Arc::new(Runtime {
            version: "not-node",
            ..Default::default()
        }),
        "/custom/node",
        &cwd,
    )
    .await;
    assert_eq!(invalid["status"], "error");
    assert_eq!(invalid["source"], "environment");
    let incompatible = probe(
        Arc::new(Runtime {
            version: "v20.1.0",
            capabilities: json!({"permission":true,"typescriptStrip":false,"worker":true}),
            ..Default::default()
        }),
        "node",
        &cwd,
    )
    .await;
    assert_eq!(incompatible["status"], "incompatible");
    assert_eq!(incompatible["version"], "v20.1.0");
}

#[tokio::test]
async fn timeout_terminates_probe_and_is_not_misreported_as_missing() {
    let runtime = Arc::new(Runtime {
        hang: true,
        ..Default::default()
    });
    let killed = runtime.killed.clone();
    let value = probe_with_timeout(
        runtime,
        "node",
        &std::env::temp_dir(),
        Duration::from_millis(2),
    )
    .await;
    assert_eq!(value["status"], "timeout");
    assert!(killed.load(Ordering::SeqCst));
    assert_eq!(value["available"], false);
}

#[test]
fn managed_node_precedes_path_and_selection_is_explicit() {
    let directory =
        std::env::temp_dir().join(format!("dsh-node-selection-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(directory.join("bin")).unwrap();
    assert_eq!(configured_command(&directory), "node");
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    let binary = directory.join("bin").join(name);
    std::fs::write(&binary, "fixture").unwrap();
    assert_eq!(PathBuf::from(configured_command(&directory)), binary);
    let direct = directory.join(name);
    std::fs::write(&direct, "fixture").unwrap();
    assert_eq!(PathBuf::from(configured_command(&directory)), direct);
    std::fs::remove_dir_all(directory).unwrap();
}
