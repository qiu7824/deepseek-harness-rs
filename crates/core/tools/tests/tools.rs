//! Tool runtime pipeline tests: Rust port of
//! `packages/core/tools/tests/tools.spec.ts` (registry, pre/guard/around/
//! post pipeline, cancellation contract, and notification).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use cordis::{ArcValue, Context, EventOptions, NextFn, arc, downcast_arc};
use dsh_llm::{ContentBlock, call_id};
use dsh_tools::schema::{
    ParameterPropertySpec, ParameterSchemaSpec, StringValueSchemaSpec, ValueSchemaAnnotations,
    ValueSchemaSpec,
};
use dsh_tools::*;
use serde_json::{Value as JsonValue, json};

fn setup() -> (Context, Arc<ToolRuntime>) {
    let ctx = Context::root();
    let _ = dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
        .expect("systemPrompt");
    let runtime = ToolRuntime::install(&ctx, Config::default()).expect("tools");
    (ctx, runtime)
}

fn echo_parameters() -> JsonValue {
    let mut properties = ParameterSchemaSpec::new();
    properties.insert(
        "message".to_string(),
        ParameterPropertySpec {
            schema: ValueSchemaSpec::String(StringValueSchemaSpec {
                annotations: ValueSchemaAnnotations::default(),
                enum_: None,
                const_: None,
            }),
            required: true,
        },
    );
    parameter_schema_spec_to_json_schema(&properties).expect("parameters")
}

fn echo_tool() -> ToolDefinition {
    ToolDefinition {
        name: "echo".to_string(),
        description: "echo a message".to_string(),
        parameters: echo_parameters(),
        output: ToolOutputDefinition {
            schema: value_schema_spec_to_json_schema(&ValueSchemaSpec::String(
                StringValueSchemaSpec {
                    annotations: ValueSchemaAnnotations::default(),
                    enum_: None,
                    const_: None,
                },
            ))
            .expect("output schema"),
            render: Arc::new(|_args, value| {
                Ok(vec![ContentBlock::Text {
                    text: value.as_str().expect("string").to_string(),
                }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(|args, _run_ctx| {
            let text = args["message"].as_str().expect("message").to_string();
            Box::pin(async move { Ok(JsonValue::String(text)) })
        }),
        finalize_content: None,
        present_call: None,
        present_result: None,
    }
}

fn input(name: &str, args: JsonValue) -> ToolExecutionInput {
    ToolExecutionInput {
        call_id: call_id("c1"),
        root_call_id: None,
        name: name.to_string(),
        arguments: args,
        agent: None,
        parent: None,
        signal: Arc::new(|| false),
    }
}

fn error_text(result: &ToolExecutionResult) -> Option<&str> {
    result.error.as_ref().map(|error| error.message.as_str())
}

#[tokio::test]
async fn registers_lists_and_unregisters_tools() {
    let (ctx, runtime) = setup();
    let dispose = runtime.register(&ctx, echo_tool()).expect("register");
    assert!(runtime.get("echo", None).is_some());
    let schemas = runtime.schemas(None);
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].name, "echo");
    assert_eq!(schemas[0].parameters, echo_parameters());

    // Duplicate registration rejects (the TS NamedEntries contract).
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime
            .register(&ctx, echo_tool())
            .expect("duplicate must panic");
    }));
    assert!(outcome.is_err());

    // Cross-registry transactions need a non-panicking prepare seam so the
    // caller can roll back other prepared contributions.
    let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.prepare_register_arc(&ctx, Arc::new(echo_tool()))
    }));
    assert!(
        prepared.is_ok(),
        "prepare_register_arc must not unwind on a duplicate"
    );
    assert!(
        prepared.expect("prepare outcome").is_err(),
        "duplicate prepare must return Err"
    );

    // The reserved transport name cannot be registered.
    let mut reserved = echo_tool();
    reserved.name = "run_code".to_string();
    let error = runtime
        .register(&ctx, reserved)
        .err()
        .expect("reserved name must reject");
    assert!(error.contains("reserved"), "got {error}");

    dispose().await;
    assert!(runtime.get("echo", None).is_none());
}

#[tokio::test]
async fn executes_a_tool_body_and_materializes_the_success() {
    let (ctx, runtime) = setup();
    runtime.register(&ctx, echo_tool()).expect("register");

    let result = runtime
        .execute(input("echo", json!({ "message": "hi" })))
        .await;
    assert!(!result.is_error);
    assert_eq!(result.value, Some(json!("hi")));
    assert_eq!(
        result.content,
        vec![ContentBlock::Text {
            text: "hi".to_string()
        }]
    );
}

#[tokio::test]
async fn invalid_arguments_and_invalid_output_become_error_results() {
    let (ctx, runtime) = setup();
    let mut tool = echo_tool();
    let invalid_args = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&invalid_args);
    tool.execute = Arc::new(move |args, _run_ctx| {
        counter.fetch_add(1, Ordering::SeqCst);
        let text = args["message"].as_str().expect("message").to_string();
        Box::pin(async move { Ok(JsonValue::String(text)) })
    });
    runtime.register(&ctx, tool).expect("register");

    // Missing required argument: ToolArgsError-like failure (the body's own
    // panic normalizes into an error result).
    let result = runtime.execute(input("echo", json!({}))).await;
    assert!(result.is_error);
    assert!(
        error_text(&result).expect("message").contains("message"),
        "got {:?}",
        error_text(&result)
    );

    // Invalid output value (the output schema requires a string). A second
    // tool name keeps the registry's duplicate-name contract out of the way.
    let mut tool = echo_tool();
    tool.name = "echo2".to_string();
    tool.output = ToolOutputDefinition {
        schema: value_schema_spec_to_json_schema(&ValueSchemaSpec::String(StringValueSchemaSpec {
            annotations: ValueSchemaAnnotations::default(),
            enum_: None,
            const_: None,
        }))
        .expect("output schema"),
        render: Arc::new(|_args, value| {
            Ok(vec![ContentBlock::Text {
                text: value.as_str().expect("string").to_string(),
            }])
        }),
        presentation_meta: None,
    };
    tool.execute = Arc::new(|_args, _run_ctx| Box::pin(async { Ok(json!(42)) }));
    runtime.register(&ctx, tool).expect("register");

    let result = runtime
        .execute(input("echo2", json!({ "message": "hi" })))
        .await;
    assert!(result.is_error);
    assert_eq!(
        result
            .error
            .as_ref()
            .expect("error")
            .info
            .as_ref()
            .expect("info")
            .code,
        "INVALID_TOOL_OUTPUT"
    );
}

#[tokio::test]
async fn unknown_tool_reports_unknown_tool() {
    let (_ctx, runtime) = setup();
    let result = runtime.execute(input("missing", json!({}))).await;
    assert!(result.is_error);
    let info = result
        .error
        .as_ref()
        .expect("error")
        .info
        .as_ref()
        .expect("info");
    assert_eq!(info.code, "UNKNOWN_TOOL");
    assert!(
        error_text(&result)
            .expect("message")
            .contains("unknown tool \"missing\"")
    );
}

#[tokio::test]
async fn unknown_tool_child_does_not_invoke_the_panic_hook() {
    if std::env::var_os("DSH_UNKNOWN_TOOL_CHILD").is_none() {
        return;
    }
    std::panic::set_hook(Box::new(|_| eprintln!("DSH_UNKNOWN_TOOL_PANIC_HOOK")));
    let (_ctx, runtime) = setup();
    let result = runtime.execute(input("missing", json!({}))).await;
    assert_eq!(
        result
            .error
            .as_ref()
            .and_then(|error| error.info.as_ref())
            .map(|info| info.code.as_str()),
        Some("UNKNOWN_TOOL")
    );
}

#[test]
fn unknown_tool_is_a_structured_failure_without_a_panic_hook() {
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "unknown_tool_child_does_not_invoke_the_panic_hook",
            "--nocapture",
        ])
        .env("DSH_UNKNOWN_TOOL_CHILD", "1")
        .output()
        .expect("run unknown-tool child");
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("DSH_UNKNOWN_TOOL_PANIC_HOOK"),
        "unknown tool invoked panic hook: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn pre_execute_listeners_gate_the_dispatch() {
    let (ctx, runtime) = setup();
    let body_runs = Arc::new(AtomicU32::new(0));
    let mut tool = echo_tool();
    let counter = Arc::clone(&body_runs);
    tool.execute = Arc::new(move |args, _run_ctx| {
        counter.fetch_add(1, Ordering::SeqCst);
        let text = args["message"].as_str().expect("message").to_string();
        Box::pin(async move { Ok(JsonValue::String(text)) })
    });
    runtime.register(&ctx, tool).expect("register");

    let listener: Arc<cordis::Listener> = Arc::new(|_ctx, args| {
        let next = downcast_arc::<NextFn>(&args[1]).expect("next").clone();
        Box::pin(async move {
            let _ = next.call().await;
            Some(arc(PreToolDecision::Deny {
                reason: "policy said no".to_string(),
            }))
        })
    });
    ctx.on(
        "tools/pre-execute",
        listener,
        EventOptions::default().global(true),
    )
    .await;

    let result = runtime
        .execute(input("echo", json!({ "message": "hi" })))
        .await;
    assert!(result.is_error);
    assert_eq!(error_text(&result), Some("policy said no"));
    assert_eq!(
        result.content,
        vec![ContentBlock::Text {
            text: "Error: policy said no".to_string()
        }]
    );
    assert_eq!(body_runs.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn guards_deny_monotonically() {
    let (ctx, runtime) = setup();
    runtime.register(&ctx, echo_tool()).expect("register");
    let guard: ToolGuard =
        Arc::new(|exec| (exec.name == "echo").then(|| "guard denies echo".to_string()));
    runtime.guard(&ctx, guard).expect("guard");

    let result = runtime
        .execute(input("echo", json!({ "message": "hi" })))
        .await;
    assert!(result.is_error);
    assert_eq!(error_text(&result), Some("guard denies echo"));
}

#[tokio::test]
async fn post_execute_can_block_and_replace() {
    let (ctx, runtime) = setup();
    runtime.register(&ctx, echo_tool()).expect("register");

    // Block turns corrective feedback into an error result.
    let blocker: Arc<cordis::Listener> = Arc::new(|_ctx, args| {
        let next = downcast_arc::<NextFn>(&args[2]).expect("next").clone();
        Box::pin(async move {
            let _ = next.call().await;
            Some(arc(PostToolDecision::Block {
                feedback: vec![ContentBlock::Text {
                    text: "please redo".to_string(),
                }],
                additional_contexts: None,
            }))
        })
    });
    ctx.on(
        "tools/post-execute",
        blocker,
        EventOptions::default().global(true),
    )
    .await;
    let result = runtime
        .execute(input("echo", json!({ "message": "hi" })))
        .await;
    assert!(result.is_error);
    assert_eq!(
        result.content,
        vec![ContentBlock::Text {
            text: "please redo".to_string()
        }]
    );
    assert_eq!(error_text(&result), Some("please redo"));

    // A fresh runtime: accept with replacement content.
    let (ctx, runtime) = setup();
    runtime.register(&ctx, echo_tool()).expect("register");
    let replacer: Arc<cordis::Listener> = Arc::new(|_ctx, args| {
        let next = downcast_arc::<NextFn>(&args[2]).expect("next").clone();
        Box::pin(async move {
            let _ = next.call().await;
            Some(arc(PostToolDecision::Accept {
                content: Some(vec![ContentBlock::Text {
                    text: "replaced".to_string(),
                }]),
                value: None,
                additional_contexts: None,
            }))
        })
    });
    ctx.on(
        "tools/post-execute",
        replacer,
        EventOptions::default().global(true),
    )
    .await;
    let result = runtime
        .execute(input("echo", json!({ "message": "hi" })))
        .await;
    assert!(!result.is_error);
    assert_eq!(
        result.content,
        vec![ContentBlock::Text {
            text: "replaced".to_string()
        }]
    );
    assert_eq!(result.value, Some(json!("hi")));
}

#[tokio::test]
async fn finalize_content_transforms_every_outcome() {
    let (ctx, runtime) = setup();
    let mut tool = echo_tool();
    tool.finalize_content = Some(Arc::new(|_exec, _result| {
        Some(vec![ContentBlock::Text {
            text: "finalized".to_string(),
        }])
    }));
    runtime.register(&ctx, tool).expect("register");

    let result = runtime
        .execute(input("echo", json!({ "message": "hi" })))
        .await;
    assert!(!result.is_error);
    assert_eq!(
        result.content,
        vec![ContentBlock::Text {
            text: "finalized".to_string()
        }]
    );
}

#[tokio::test]
async fn defer_context_and_conclude_turn_ride_the_result() {
    let (ctx, runtime) = setup();
    let mut tool = echo_tool();
    tool.execute = Arc::new(|args, run_ctx| {
        let text = args["message"].as_str().expect("message").to_string();
        run_ctx.defer_context(dsh_llm::create_user_message(
            vec![ContentBlock::Text {
                text: "deferred note".to_string(),
            }],
            dsh_llm::MessageSource::Plugin {
                plugin: "test".to_string(),
                form: None,
                sections: None,
                summary: None,
                compaction_id: None,
                source_command_id: None,
            },
        ));
        run_ctx.conclude_turn();
        Box::pin(async move { Ok(JsonValue::String(text)) })
    });
    runtime.register(&ctx, tool).expect("register");

    let result = runtime
        .execute(input("echo", json!({ "message": "hi" })))
        .await;
    assert!(!result.is_error);
    assert!(result.concludes_turn);
    assert_eq!(result.additional_contexts.len(), 1);
    assert_eq!(
        result.additional_contexts[0].content,
        vec![ContentBlock::Text {
            text: "deferred note".to_string()
        }]
    );
}

#[tokio::test]
async fn cancellation_before_dispatch_skips_the_body() {
    let (ctx, runtime) = setup();
    let body_runs = Arc::new(AtomicU32::new(0));
    let mut tool = echo_tool();
    let counter = Arc::clone(&body_runs);
    tool.execute = Arc::new(move |args, _run_ctx| {
        counter.fetch_add(1, Ordering::SeqCst);
        let text = args["message"].as_str().expect("message").to_string();
        Box::pin(async move { Ok(JsonValue::String(text)) })
    });
    runtime.register(&ctx, tool).expect("register");

    let mut aborted = input("echo", json!({ "message": "hi" }));
    aborted.signal = Arc::new(|| true);
    let result = runtime.execute(aborted).await;
    assert!(result.is_error);
    let info = result
        .error
        .as_ref()
        .expect("error")
        .info
        .as_ref()
        .expect("info");
    assert_eq!(info.code, "ABORTED_BEFORE_DISPATCH");
    assert_eq!(body_runs.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancellation_after_body_invocation_reports_aborted() {
    let (ctx, runtime) = setup();
    let aborted_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut tool = echo_tool();
    let flag = Arc::clone(&aborted_flag);
    tool.execute = Arc::new(move |args, _run_ctx| {
        let text = args["message"].as_str().expect("message").to_string();
        let flag = Arc::clone(&flag);
        Box::pin(async move {
            flag.store(true, Ordering::SeqCst);
            Ok(JsonValue::String(text))
        })
    });
    runtime.register(&ctx, tool).expect("register");

    let mut exec = input("echo", json!({ "message": "hi" }));
    let flag = Arc::clone(&aborted_flag);
    exec.signal = Arc::new(move || flag.load(Ordering::SeqCst));
    let result = runtime.execute(exec).await;
    assert!(result.is_error);
    let info = result
        .error
        .as_ref()
        .expect("error")
        .info
        .as_ref()
        .expect("info");
    assert_eq!(info.code, "ABORTED");
    assert!(
        error_text(&result)
            .expect("message")
            .contains("tool call aborted")
    );
}

#[tokio::test]
async fn execution_mode_classifies_concurrency_safety() {
    let (ctx, runtime) = setup();
    let mut tool = echo_tool();
    tool.is_concurrency_safe = Some(Arc::new(|args| args["message"].as_str() == Some("safe")));
    runtime.register(&ctx, tool).expect("register");

    assert_eq!(
        runtime.execution_mode(&input("echo", json!({ "message": "safe" }))),
        ToolExecutionMode::Parallel
    );
    assert_eq!(
        runtime.execution_mode(&input("echo", json!({ "message": "unsafe" }))),
        ToolExecutionMode::Exclusive
    );
    assert_eq!(
        runtime.execution_mode(&input("missing", json!({}))),
        ToolExecutionMode::Exclusive
    );
}

#[tokio::test]
async fn tools_result_event_fires_with_the_final_outcome() {
    let (ctx, runtime) = setup();
    runtime.register(&ctx, echo_tool()).expect("register");
    let observed: Arc<parking_lot::Mutex<Vec<Arc<ToolExecutionResult>>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let results = Arc::clone(&observed);
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args: Vec<ArcValue>| {
        let results = Arc::clone(&results);
        Box::pin(async move {
            if let Some(result) = downcast_arc::<Arc<ToolExecutionResult>>(&args[1]) {
                results.lock().push(result.as_ref().clone());
            }
            None
        })
    });
    ctx.on(
        "tools/result",
        listener,
        EventOptions::default().global(true),
    )
    .await;

    runtime
        .execute(input("echo", json!({ "message": "hi" })))
        .await;
    let observed = observed.lock();
    assert_eq!(observed.len(), 1);
    assert!(!observed[0].is_error);
    assert_eq!(observed[0].value, Some(json!("hi")));
}

#[tokio::test]
async fn tools_change_fires_on_registration() {
    let (ctx, runtime) = setup();
    let fired = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&fired);
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, _args| {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            None
        })
    });
    ctx.on(
        "tools/change",
        listener,
        EventOptions::default().global(true),
    )
    .await;

    let dispose = runtime.register(&ctx, echo_tool()).expect("register");
    dispose().await;
    // The registration and its disposal each notify (emit is
    // fire-and-forget; drain the spawned tasks with a bounded wait).
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert!(
        fired.load(Ordering::SeqCst) >= 2,
        "got {}",
        fired.load(Ordering::SeqCst)
    );
}

#[tokio::test]
async fn execute_wrapper_signal_replacement_still_fuses_the_caller() {
    let (ctx, runtime) = setup();
    runtime.register(&ctx, echo_tool()).expect("register");
    let aborted_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&aborted_flag);

    // The wrapper replaces the dispatch signal; the caller flips after the
    // body starts. Fusion must still see the CALLER's cancellation.
    let mut exec = input("echo", json!({ "message": "hi" }));
    let caller_flag = Arc::clone(&aborted_flag);
    exec.signal = Arc::new(move || caller_flag.load(Ordering::SeqCst));

    let wrapper: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
        let next = downcast_arc::<NextFn>(&args[1]).expect("next").clone();
        let flag = Arc::clone(&flag);
        Box::pin(async move {
            let exec = args[0].clone();
            let result = next.call().await;
            // Replace the visible signal with an always-false predicate: the
            // fused body must still observe the caller's flag.
            if let Some(execution) = downcast_arc::<Arc<ToolExecution>>(&exec) {
                *execution.signal.lock() = Arc::new(|| false);
                flag.store(true, Ordering::SeqCst);
            }
            Some(result)
        })
    });
    ctx.on(
        "tools/execute",
        wrapper,
        EventOptions::default().global(true),
    )
    .await;

    let result = runtime.execute(exec).await;
    assert!(result.is_error);
    let info = result
        .error
        .as_ref()
        .expect("error")
        .info
        .as_ref()
        .expect("info");
    assert_eq!(info.code, "ABORTED");
}
