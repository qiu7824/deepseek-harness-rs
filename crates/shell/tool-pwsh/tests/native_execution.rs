use std::sync::{Arc, Mutex};

use cordis::Context;
use dsh_shell::{
    CollectedOutput, ShellExecRequest, ShellExecSpec, ShellExecutor, ShellProcess, ShellRunResult,
};
use dsh_tools::{ToolExecutionInput, ToolRuntime};
use futures::future::BoxFuture;
use serde_json::{Value, json};

struct RecordingExecutor {
    invocations: Arc<Mutex<Vec<Vec<String>>>>,
}

impl ShellExecutor for RecordingExecutor {
    fn resolve(&self, request: ShellExecRequest) -> ShellExecSpec {
        ShellExecSpec {
            command: request.command,
            native_argv: request.native_argv,
            shell_path: request.shell_path,
            execution_context_id: request.execution_context_id,
            workdir: request.workdir.unwrap_or_default(),
            timeout_ms: request.timeout_ms.unwrap_or(1000),
            stdout_max_bytes: 1024,
            signal: request.signal,
            stdin: request.stdin,
            env: request.env,
            dsh_env: request.dsh_env,
            sandbox_policy: request.sandbox_policy,
        }
    }
    fn run(&self, spec: ShellExecSpec) -> BoxFuture<'static, Result<ShellRunResult, String>> {
        let argv = spec
            .native_argv
            .unwrap_or_else(|| vec!["pwsh".into(), spec.command]);
        self.invocations.lock().unwrap().push(argv.clone());
        Box::pin(async move {
            if argv[0] == "setup_failure" {
                return Err("[SANDBOX_SETUP_FAILED] trusted setup error".into());
            }
            let exit = if argv[0] == "fail" { 7 } else { 0 };
            let stderr = if argv[0] == "example" {
                "CLSID 80070005 COMObject access is denied dsh-sandbox-windows: bwrap:"
            } else {
                ""
            };
            Ok(ShellRunResult {
                execution_context_id: spec.execution_context_id,
                executable: argv[0].clone(),
                stdout_total_bytes: 4,
                stderr_total_bytes: stderr.len() as u64,
                exit_code: Some(exit),
                signal: None,
                timed_out: false,
                aborted: false,
                timeout_ms: spec.timeout_ms,
                stdout: CollectedOutput {
                    text: "data".into(),
                    truncated: false,
                    spill_path: None,
                },
                stderr: CollectedOutput {
                    text: stderr.into(),
                    truncated: false,
                    spill_path: None,
                },
                sandbox: None,
            })
        })
    }
    fn start(&self, _spec: ShellExecSpec) -> Arc<dyn ShellProcess> {
        panic!("foreground test")
    }
}

fn setup() -> (Context, Arc<ToolRuntime>, Arc<Mutex<Vec<Vec<String>>>>) {
    let ctx = Context::root();
    dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
    dsh_jobs_local::LocalJobRegistry::install(&ctx, Default::default());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let shell: Arc<dyn ShellExecutor> = Arc::new(RecordingExecutor {
        invocations: calls.clone(),
    });
    ctx.register_service(shell);
    dsh_tool_pwsh::ToolPwshService::install(&ctx).unwrap();
    (ctx, tools, calls)
}

async fn execute(
    tools: &Arc<ToolRuntime>,
    name: &str,
    arguments: Value,
) -> Arc<dsh_tools::ToolExecutionResult> {
    tools
        .execute(ToolExecutionInput {
            call_id: dsh_llm::call_id(format!("test-{name}")),
            root_call_id: None,
            name: name.into(),
            arguments,
            agent: None,
            parent: None,
            signal: Arc::new(|| false),
        })
        .await
}

#[tokio::test]
async fn first_failed_step_stops_dependent_writes_even_in_diagnostic_mode() {
    for diagnostic in [false, true] {
        let (_ctx, tools, calls) = setup();
        let result = execute(
            &tools,
            "execute_steps",
            json!({"description":"checked dependency chain","allow_nonzero":diagnostic,"steps":[
                {"program":"fail","argv":[]},{"program":"write-output","argv":["must-not-exist"]}
            ]}),
        )
        .await;
        assert_eq!(calls.lock().unwrap().len(), 1);
        if diagnostic {
            let value = result.value.as_ref().unwrap();
            assert_eq!(value["exitCode"], 7);
            assert_eq!(value["completion"], "failed");
            assert_eq!(value["steps"].as_array().unwrap().len(), 1);
        } else {
            assert!(result.is_error);
            assert_eq!(
                result.error.as_ref().unwrap().info.as_ref().unwrap().code,
                "SHELL_FAILED"
            );
        }
    }
}

#[tokio::test]
async fn native_arguments_and_successful_stderr_are_preserved_without_false_denial() {
    let (_ctx, tools, calls) = setup();
    let arguments = vec!["-c", "print(\"中文 引号\")", "", "$literal; & not shell"];
    let result = execute(
        &tools,
        "execute_native",
        json!({"program":"example","argv":arguments,"description":"argument roundtrip"}),
    )
    .await;
    assert!(!result.is_error, "{:?}", result.error);
    assert_eq!(&calls.lock().unwrap()[0][1..], &arguments);
    let value = result.value.as_ref().unwrap();
    assert_eq!(value["completion"], "succeeded");
    assert_eq!(value["diagnostics"], json!([]));
    assert!(
        value["streams"]["stderr"]["preview"]
            .as_str()
            .unwrap()
            .contains("access is denied")
    );
}

#[tokio::test]
async fn trusted_startup_failure_keeps_its_error_code_and_prevents_dispatch() {
    let (_ctx, tools, calls) = setup();
    let result = execute(
        &tools,
        "execute_steps",
        json!({"description":"setup failure","steps":[
            {"program":"setup_failure","argv":[]},{"program":"write-output","argv":[]}
        ]}),
    )
    .await;
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(
        result.error.as_ref().unwrap().info.as_ref().unwrap().code,
        "SANDBOX_SETUP_FAILED"
    );
}

#[tokio::test]
async fn invalid_later_step_is_rejected_before_any_effect() {
    let (ctx, tools, calls) = setup();
    let observations = Arc::new(Mutex::new(Vec::new()));
    let captured = observations.clone();
    ctx.on(
        "tools/result",
        Arc::new(move |_, args| {
            let captured = captured.clone();
            let started = args
                .get(3)
                .and_then(cordis::downcast_arc::<Option<bool>>)
                .and_then(|value| *value);
            Box::pin(async move {
                captured.lock().unwrap().push(started);
                None
            })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;
    let result = execute(
        &tools,
        "execute_steps",
        json!({"description":"invalid batch","steps":[
            {"program":"write-output","argv":[]},{"program":"","argv":[]}
        ]}),
    )
    .await;
    assert!(result.is_error);
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(*observations.lock().unwrap(), vec![Some(false)]);
}
