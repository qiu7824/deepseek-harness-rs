//! Rust port of the TS `service.spec.ts` suite for `dsh-code-runtime`: a
//! concrete runtime registers as `ctx.codeRuntime`, serves the abstract API,
//! reports failures as result fields, and unregisters with its fiber.
//!
//! # Deviations
//!
//! - The pre-aborted reason object collapses into the message `"aborted"`
//!   (the abort predicate carries no reason in Rust).
//! - The duplicate-registration panic is contained by the fiber load chain,
//!   so `settle()` reports the generic `plugin callback panicked` error.

use std::sync::Arc;

use cordis::{Context, Plugin};
use futures::FutureExt;
use parking_lot::Mutex;

use dsh_code_runtime::{
    CodeJsonValue, CodeRunFailure, CodeRunFailureKind, CodeRunRequest, CodeRunResult, CodeRuntime,
};

/// The TS `StubRuntime`: records requests, "executes" by invoking every
/// binding once in declaration order, and lets tests script the outcome.
struct StubRuntime {
    requests: Arc<Mutex<Vec<CodeRunRequest>>>,
    next_result: Arc<Mutex<CodeRunResult>>,
}

impl StubRuntime {
    fn new() -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            next_result: Arc::new(Mutex::new(CodeRunResult::default())),
        }
    }
}

impl CodeRuntime for StubRuntime {
    fn language(&self) -> String {
        "typescript".to_string()
    }

    fn isolation(&self) -> String {
        "in-process-stub".to_string()
    }

    fn run(
        &self,
        request: CodeRunRequest,
    ) -> futures::future::BoxFuture<'static, Result<CodeRunResult, String>> {
        let requests = self.requests.clone();
        let next_result = self.next_result.clone();
        Box::pin(async move {
            if request.signal.as_ref().is_some_and(|signal| signal()) {
                return Ok(CodeRunResult {
                    logs: Vec::new(),
                    value: None,
                    error: Some(CodeRunFailure {
                        kind: CodeRunFailureKind::Abort,
                        message: "aborted".to_string(),
                    }),
                });
            }
            for namespace in &request.bindings {
                for (_name, function) in &namespace.functions {
                    function(serde_json::json!({ "from": "stub" })).await;
                }
            }
            requests.lock().push(request);
            Ok(next_result.lock().clone())
        })
    }
}

/// The plugin form (the TS `ctx.plugin(StubRuntime)`); the concrete handle
/// rides a slot (the TS `ctx.codeRuntime as StubRuntime` cast).
struct StubRuntimePlugin {
    slot: Arc<Mutex<Option<Arc<StubRuntime>>>>,
}

#[async_trait::async_trait]
impl Plugin for StubRuntimePlugin {
    async fn apply(
        &self,
        ctx: &Context,
        _config: cordis::ArcValue,
    ) -> Result<(), cordis::PluginError> {
        let runtime = Arc::new(StubRuntime::new());
        let erased: Arc<dyn CodeRuntime> = runtime.clone();
        ctx.register_service(erased);
        *self.slot.lock() = Some(runtime);
        Ok(())
    }
}

async fn setup() -> (Context, Arc<StubRuntime>) {
    let ctx = Context::root();
    let slot = Arc::new(Mutex::new(None));
    let fiber = ctx.plugin(
        Arc::new(StubRuntimePlugin { slot: slot.clone() }),
        cordis::arc(()),
    );
    fiber.settle().await.expect("runtime loads");
    let runtime = slot.lock().take().expect("runtime installed");
    (ctx, runtime)
}

#[tokio::test(flavor = "current_thread")]
async fn registers_as_ctx_code_runtime_and_serves_the_abstract_api() {
    let (_ctx, runtime) = setup().await;
    assert_eq!(runtime.language(), "typescript");
    assert_eq!(runtime.isolation(), "in-process-stub");

    let calls: Arc<Mutex<Vec<CodeJsonValue>>> = Arc::new(Mutex::new(Vec::new()));
    let calls_for_binding = calls.clone();
    let probe: dsh_code_runtime::CodeBindingFunction = Arc::new(move |args| {
        let calls = calls_for_binding.clone();
        Box::pin(async move {
            calls.lock().push(args.clone());
            serde_json::Value::Null
        })
    });
    let result = runtime
        .run(CodeRunRequest {
            program: "return 1".to_string(),
            bindings: vec![dsh_code_runtime::CodeBindingNamespace {
                global: "tools".to_string(),
                functions: vec![("probe".to_string(), probe)],
                error_class: None,
            }],
            signal: None,
        })
        .await
        .expect("run");
    assert_eq!(result.logs, Vec::<String>::new());
    assert!(result.error.is_none());
    assert!(result.value.is_none());
    assert_eq!(*calls.lock(), vec![serde_json::json!({ "from": "stub" })]);
    assert_eq!(runtime.requests.lock().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn reports_a_failed_run_as_an_error_field_never_a_rejection() {
    let (_ctx, runtime) = setup().await;
    *runtime.next_result.lock() = CodeRunResult {
        logs: vec!["boom".to_string()],
        value: None,
        error: Some(CodeRunFailure {
            kind: CodeRunFailureKind::Exception,
            message: "boom".to_string(),
        }),
    };
    let result = runtime
        .run(CodeRunRequest {
            program: "throw new Error(\"boom\")".to_string(),
            bindings: Vec::new(),
            signal: None,
        })
        .await
        .expect("run");
    assert_eq!(
        result.error,
        Some(CodeRunFailure {
            kind: CodeRunFailureKind::Exception,
            message: "boom".to_string(),
        })
    );
    assert!(result.value.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn reports_a_pre_aborted_signal_as_an_abort_failure() {
    let (_ctx, runtime) = setup().await;
    let aborted: dsh_code_runtime::CodeAbort = Arc::new(|| true);
    let result = runtime
        .run(CodeRunRequest {
            program: "return 1".to_string(),
            bindings: Vec::new(),
            signal: Some(aborted),
        })
        .await
        .expect("run");
    assert_eq!(
        result.error,
        Some(CodeRunFailure {
            kind: CodeRunFailureKind::Abort,
            message: "aborted".to_string(),
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn is_removed_from_the_context_when_the_providing_fiber_disposes() {
    let ctx = Context::root();
    let slot = Arc::new(Mutex::new(None));
    let fiber = ctx.plugin(
        Arc::new(StubRuntimePlugin { slot: slot.clone() }),
        cordis::arc(()),
    );
    fiber.settle().await.expect("runtime loads");
    assert!(ctx.get("codeRuntime", false).is_some());

    fiber.dispose().await;
    assert!(ctx.get("codeRuntime", false).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_a_second_implementation_in_the_same_context() {
    let ctx = Context::root();
    let slot = Arc::new(Mutex::new(None));
    let fiber = ctx.plugin(
        Arc::new(StubRuntimePlugin { slot: slot.clone() }),
        cordis::arc(()),
    );
    fiber.settle().await.expect("first runtime loads");
    let second = ctx.plugin(
        Arc::new(StubRuntimePlugin { slot: slot.clone() }),
        cordis::arc(()),
    );
    let error = second.settle().await.err().expect("second load fails");
    assert!(error.message().contains("panicked"), "{}", error.message());
}
