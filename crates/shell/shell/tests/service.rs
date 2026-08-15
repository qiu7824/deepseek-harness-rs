//! Rust port of the TS `service.spec.ts` suite for `dsh-shell`: a concrete
//! executor registers as `ctx.shell`, serves the abstract API, reports no
//! default sandbox mode, and a second registration fails loud.
//!
//! # Deviations
//!
//! - `run`'s infrastructure-failure rejections collapse into
//!   `Result<ShellRunResult, String>` (the repo-wide result-channel
//!   convention); the stub resolves `Ok`.
//! - The duplicate-registration panic is contained by the fiber load chain,
//!   so `settle()` reports the generic `plugin callback panicked` error
//!   rather than the original `service "shell" has been registered` message
//!   (that message reaches the logger only).

use std::sync::Arc;

use cordis::{Context, Plugin};
use futures::future::BoxFuture;
use parking_lot::Mutex;

use dsh_shell::{
    CollectedOutput, ShellExecRequest, ShellExecSpec, ShellExecutor, ShellProcess,
    ShellProcessRead, ShellProcessStatus, ShellRunResult,
};

/// The TS `StubExecutor`: canned foreground results, a hand-built process
/// handle. The seam is TASK-FREE (`start` returns a [`ShellProcess`] handle;
/// task semantics live in `ctx.jobs`).
struct StubExecutor;

impl ShellExecutor for StubExecutor {
    fn resolve(&self, request: ShellExecRequest) -> ShellExecSpec {
        ShellExecSpec {
            command: request.command,
            workdir: request.workdir.unwrap_or_else(|| "/stub".to_string()),
            timeout_ms: request.timeout_ms.unwrap_or(1_000),
            stdout_max_bytes: request.stdout_max_bytes.unwrap_or(64_000),
            signal: request.signal,
            stdin: request.stdin,
            env: request.env,
            dsh_env: request.dsh_env,
            sandbox_policy: request.sandbox_policy,
        }
    }

    fn run(&self, spec: ShellExecSpec) -> BoxFuture<'static, Result<ShellRunResult, String>> {
        Box::pin(async move {
            Ok(ShellRunResult {
                exit_code: Some(0),
                signal: None,
                timed_out: false,
                aborted: false,
                timeout_ms: spec.timeout_ms,
                stdout: CollectedOutput {
                    text: "ok".to_string(),
                    truncated: false,
                    spill_path: None,
                },
                stderr: CollectedOutput {
                    text: String::new(),
                    truncated: false,
                    spill_path: None,
                },
                sandbox: None,
            })
        })
    }

    fn start(&self, _spec: ShellExecSpec) -> Arc<dyn ShellProcess> {
        Arc::new(StubProcess {
            status: Mutex::new(ShellProcessStatus::Running),
        })
    }
}

/// The TS hand-built `ShellProcess`.
struct StubProcess {
    status: Mutex<ShellProcessStatus>,
}

impl ShellProcess for StubProcess {
    fn status(&self) -> ShellProcessStatus {
        *self.status.lock()
    }

    fn exit_code(&self) -> Option<i32> {
        None
    }

    fn signal(&self) -> Option<String> {
        None
    }

    fn done(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn sandbox(&self) -> Option<dsh_shell::ShellSandboxInfo> {
        None
    }

    fn read_output(&self) -> ShellProcessRead {
        ShellProcessRead {
            delta: String::new(),
            lossy: false,
            stdout_spill_path: None,
            stderr_spill_path: None,
        }
    }

    fn kill(&self) -> bool {
        let mut status = self.status.lock();
        if *status != ShellProcessStatus::Running {
            return false;
        }
        *status = ShellProcessStatus::Killed;
        true
    }
}

/// The plugin form of the stub executor (the TS `ctx.plugin(StubExecutor)`).
struct StubExecutorPlugin;

#[async_trait::async_trait]
impl Plugin for StubExecutorPlugin {
    async fn apply(&self, ctx: &Context, _config: cordis::ArcValue) -> Result<(), cordis::PluginError> {
        let erased: Arc<dyn ShellExecutor> = Arc::new(StubExecutor);
        ctx.register_service(erased);
        Ok(())
    }
}

async fn boot() -> (Context, Arc<dyn ShellExecutor>) {
    let ctx = Context::root();
    let fiber = ctx.plugin(Arc::new(StubExecutorPlugin), cordis::arc(()));
    fiber.settle().await.expect("executor loads");
    let shell = ctx
        .get_typed::<Arc<dyn ShellExecutor>>("shell", false)
        .map(|slot| slot.as_ref().clone())
        .expect("shell service registered");
    (ctx, shell)
}

#[tokio::test(flavor = "current_thread")]
async fn a_concrete_subclass_registers_as_ctx_shell_and_serves_the_abstract_api() {
    let (_ctx, shell) = boot().await;

    let spec = shell.resolve(ShellExecRequest::new("echo hi"));
    assert_eq!(spec.command, "echo hi");
    assert_eq!(spec.workdir, "/stub");
    assert_eq!(spec.timeout_ms, 1_000);
    assert_eq!(spec.stdout_max_bytes, 64_000);
    assert!(spec.sandbox_policy.is_none());

    let result = shell.run(spec).await.expect("run");
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout.text, "ok");

    let proc = shell.start(ShellExecSpec {
        command: "echo hi".to_string(),
        workdir: "/stub".to_string(),
        timeout_ms: 1_000,
        stdout_max_bytes: 64_000,
        signal: None,
        stdin: None,
        env: None,
        dsh_env: None,
        sandbox_policy: None,
    });
    assert_eq!(proc.status(), ShellProcessStatus::Running);
    let read = proc.read_output();
    assert_eq!(read.delta, "");
    assert!(!read.lossy);
    assert!(proc.kill());
    assert!(!proc.kill(), "already settled → no-op");
    proc.done().await;
}

#[tokio::test(flavor = "current_thread")]
async fn reports_no_default_sandbox_mode_from_the_task_free_base_seam() {
    let (_ctx, shell) = boot().await;
    assert!(shell.sandbox_mode().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn loading_a_second_implementation_fails() {
    let ctx = Context::root();
    let fiber = ctx.plugin(Arc::new(StubExecutorPlugin), cordis::arc(()));
    fiber.settle().await.expect("first executor loads");
    // One bash service per context — the second registration panics inside
    // `apply` and fails the fiber (cordis standard duplicate-service
    // behavior).
    let second = ctx.plugin(Arc::new(StubExecutorPlugin), cordis::arc(()));
    let error = second.settle().await.err().expect("second load fails");
    assert!(error.message().contains("panicked"), "{}", error.message());
}
