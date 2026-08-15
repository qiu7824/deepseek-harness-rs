//! Rust port of the TS `service.spec.ts` suite for the subprocess seam: a
//! stub concrete service exercises the abstract API, the duplicate-service
//! behavior, and the parent-environment scrub.
//!
//! Deviations:
//!
//! - `fiber.dispose()` service removal collapses into the duplicate-
//!   registration panic (the Rust service store rejects a second
//!   registration of the same name).
//! - The scrub probe uses per-test unique environment names (`set_var` is
//!   unsafe in Rust 2024) instead of fixed names.

use std::sync::Arc;

use cordis::Context;
use futures::future::BoxFuture;
use futures::stream::BoxStream;

use dsh_subprocess::{
    CollectedOutput, SubprocessCollectedOutputs, SubprocessHandle, SubprocessOutcome,
    SubprocessOutputMode, SubprocessOutputRead, SubprocessOutputReader, SubprocessRuntime,
    SubprocessSpawnSpec, SubprocessTerminalForeground, SubprocessTerminalHandle,
    SubprocessTerminalSignal, SubprocessTerminalSpawnSpec, scrubbed_parent_env,
};

/// Minimal concrete service: a hand-built handle. The seam is spawn-only —
/// defaulting, shell semantics, and deadlines belong to callers.
struct StubSubprocessRuntime;

struct StubReader;

impl SubprocessOutputReader for StubReader {
    fn read_from(&self, _from_byte: u64) -> SubprocessOutputRead {
        SubprocessOutputRead {
            text: String::new(),
            next_offset: 0,
            lossy: false,
            spill_path: None,
        }
    }
}

struct StubHandle;

impl SubprocessHandle for StubHandle {
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
            stdout: Some(Arc::new(StubReader)),
            stderr: None,
        }
    }

    fn done(&self) -> BoxFuture<'static, Result<SubprocessOutcome, String>> {
        Box::pin(async { Ok(SubprocessOutcome { exit_code: Some(0), signal: None }) })
    }

    fn terminate(&self) {}

    fn wait_for_exit(&self, _signal: Option<dsh_subprocess::SubprocessAbort>) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
}

struct StubTerminalHandle;

impl SubprocessTerminalHandle for StubTerminalHandle {
    fn pid(&self) -> u32 {
        1
    }

    fn output(&self) -> BoxStream<'static, Vec<u8>> {
        Box::pin(futures::stream::empty())
    }

    fn done(&self) -> BoxFuture<'static, Result<SubprocessOutcome, String>> {
        Box::pin(async { Ok(SubprocessOutcome { exit_code: Some(0), signal: None }) })
    }

    fn write(&self, _data: &str) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn inspect_foreground(&self) -> BoxFuture<'static, Result<Option<SubprocessTerminalForeground>, String>> {
        Box::pin(async {
            Ok(Some(SubprocessTerminalForeground { process_group_id: 1, input_waiting: true }))
        })
    }

    fn signal_foreground(&self, _signal: SubprocessTerminalSignal) -> BoxFuture<'static, Result<u32, String>> {
        Box::pin(async { Ok(1) })
    }

    fn terminate(&self) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

impl SubprocessRuntime for StubSubprocessRuntime {
    fn resolve_executable(
        &self,
        command: &str,
        _env: Option<&[(String, String)]>,
        _signal: Option<dsh_subprocess::SubprocessAbort>,
    ) -> BoxFuture<'static, Result<String, String>> {
        let resolved = format!("/bin/{command}");
        Box::pin(async move { Ok(resolved) })
    }

    fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<Arc<dyn SubprocessHandle>, String> {
        let _ = spec;
        Ok(Arc::new(StubHandle))
    }

    fn spawn_terminal(
        &self,
        spec: SubprocessTerminalSpawnSpec,
    ) -> BoxFuture<'static, Result<Arc<dyn SubprocessTerminalHandle>, String>> {
        let _ = spec;
        Box::pin(async { Ok(Arc::new(StubTerminalHandle) as Arc<dyn SubprocessTerminalHandle>) })
    }
}

fn stub(ctx: &Context) -> Arc<dyn SubprocessRuntime> {
    let stub = Arc::new(StubSubprocessRuntime);
    let erased: Arc<dyn SubprocessRuntime> = stub.clone();
    ctx.register_service(erased);
    stub
}

#[test]
fn a_concrete_service_registers_as_ctx_subprocess_and_serves_the_abstract_api() {
    let ctx = Context::root();
    let stub = stub(&ctx);
    let handle = stub
        .spawn(SubprocessSpawnSpec {
            argv: vec!["true".to_string()],
            cwd: "/stub".to_string(),
            stdio: dsh_subprocess::SubprocessStdio {
                stdin: dsh_subprocess::SubprocessStdinMode::Ignore,
                stdout: SubprocessOutputMode::Collect(dsh_subprocess::SubprocessCollect {
                    max_bytes: 1,
                    spill: None,
                }),
                stderr: SubprocessOutputMode::Inherit,
            },
            grace_ms: 1,
            signal: None,
            env: None,
        })
        .expect("spawn");
    assert_eq!(handle.pid(), 1);
    assert_eq!(
        handle.collected().stdout.expect("reader").read_from(0),
        SubprocessOutputRead { text: String::new(), next_offset: 0, lossy: false, spill_path: None }
    );
    handle.terminate();
}

#[tokio::test(flavor = "current_thread")]
async fn the_handle_outcome_and_wait_resolve() {
    let ctx = Context::root();
    let stub = stub(&ctx);
    let handle = stub
        .spawn(SubprocessSpawnSpec {
            argv: vec!["true".to_string()],
            cwd: "/stub".to_string(),
            stdio: dsh_subprocess::SubprocessStdio {
                stdin: dsh_subprocess::SubprocessStdinMode::Ignore,
                stdout: SubprocessOutputMode::Collect(dsh_subprocess::SubprocessCollect {
                    max_bytes: 1,
                    spill: None,
                }),
                stderr: SubprocessOutputMode::Inherit,
            },
            grace_ms: 1,
            signal: None,
            env: None,
        })
        .expect("spawn");
    assert_eq!(handle.wait_for_exit(None).await, true);
    let outcome = handle.done().await.expect("outcome");
    assert_eq!(outcome.exit_code, Some(0));
    let _ = CollectedOutput { text: String::new(), truncated: false, spill_path: None };
}

#[test]
fn loading_a_second_implementation_throws() {
    let ctx = Context::root();
    let stub = stub(&ctx);
    let erased: Arc<dyn SubprocessRuntime> = stub.clone();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ctx.register_service(erased);
    }));
    assert!(outcome.is_err());
}

#[test]
fn scrubbed_parent_env_drops_credential_shaped_and_dsh_names_but_keeps_path() {
    let probe = format!("SCRUB_PROBE_{}", std::process::id());
    // SAFETY: test-process-local names, removed before the test ends.
    unsafe {
        std::env::set_var(format!("DSH_SCRUB_PROBE_{}", std::process::id()), "stale");
        std::env::set_var(format!("dsh_scrub_probe_lower_{}", std::process::id()), "stale");
        std::env::set_var(format!("{probe}_TOKEN"), "secret");
        std::env::set_var(format!("{probe}_PASSWORD"), "secret");
        std::env::set_var(&probe, "visible");
    }
    let env = scrubbed_parent_env();
    let key_of = |name: &str| env.iter().any(|(key, _)| key.eq_ignore_ascii_case(name));
    assert!(!key_of(&format!("DSH_SCRUB_PROBE_{}", std::process::id())));
    assert!(!key_of(&format!("dsh_scrub_probe_lower_{}", std::process::id())));
    assert!(!key_of(&format!("{probe}_TOKEN")));
    assert!(!key_of(&format!("{probe}_PASSWORD")));
    assert!(key_of(&probe));
    assert!(key_of("PATH"));
    unsafe {
        std::env::remove_var(format!("DSH_SCRUB_PROBE_{}", std::process::id()));
        std::env::remove_var(format!("dsh_scrub_probe_lower_{}", std::process::id()));
        std::env::remove_var(format!("{probe}_TOKEN"));
        std::env::remove_var(format!("{probe}_PASSWORD"));
        std::env::remove_var(&probe);
    }
}
