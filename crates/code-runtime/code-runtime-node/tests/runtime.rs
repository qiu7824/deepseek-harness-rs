use std::sync::Arc;

use cordis::Context;
use dsh_code_runtime::{
    CodeBindingNamespace, CodeRunFailureKind, CodeRunRequest, CodeRunResult, CodeRuntime,
};

fn payload_bytes(result: &CodeRunResult) -> usize {
    let error = result
        .error
        .as_ref()
        .map(|error| serde_json::json!({ "kind": error.kind.as_str(), "message": error.message }));
    serde_json::to_vec(&serde_json::json!({
        "value": result.value,
        "logs": result.logs,
        "error": error,
    }))
    .expect("result payload is JSON")
    .len()
}
use dsh_code_runtime_node::{Config, NodeCodeRuntime};
use dsh_subprocess_local::LocalSubprocessRuntime;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_node_worker_runs_typescript_and_round_trips_a_rust_binding() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let runtime = NodeCodeRuntime::install(&ctx, Config::default()).expect("install runtime");
    let echo: dsh_code_runtime::CodeBindingFunction =
        Arc::new(|args| Box::pin(async move { serde_json::json!({ "echoed": args }) }));

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        runtime.run(CodeRunRequest {
            program: "interface Input { n: number }\nconst value: Input = { n: 21 };\nconst reply = await tools.echo(value);\nconsole.log('bound', reply.echoed.n);\nreturn reply.echoed.n * 2;".to_string(),
            bindings: vec![CodeBindingNamespace {
                global: "tools".to_string(),
                functions: vec![("echo".to_string(), echo)],
                error_class: Some(dsh_code_runtime::CodeBindingErrorClass {
                    name: "ToolCallError".to_string(),
                    member_name_property: "toolName".to_string(),
                }),
            }],
            signal: None,
        }),
    )
    .await
    .expect("node runtime remains bounded")
    .expect("service contract");

    assert_eq!(result.value, Some(serde_json::json!(42)));
    assert_eq!(result.logs, vec!["bound 21"]);
    assert!(result.error.is_none(), "{:?}", result.error);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_code_cannot_reach_node_fs_through_process_builtins() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let runtime = NodeCodeRuntime::install(&ctx, Config::default()).expect("install runtime");
    let marker =
        std::env::temp_dir().join(format!("dsh-code-runtime-escape-{}", uuid::Uuid::new_v4()));
    let program = format!(
        "const fs = process.getBuiltinModule('fs'); fs.writeFileSync({}, 'escaped'); return true;",
        serde_json::to_string(&marker.to_string_lossy()).expect("marker path JSON")
    );

    let result = runtime
        .run(CodeRunRequest {
            program,
            bindings: Vec::new(),
            signal: None,
        })
        .await
        .expect("service contract");

    let escaped = marker.exists();
    let _ = std::fs::remove_file(&marker);
    assert!(!escaped, "model code wrote outside its binding authority");
    assert_eq!(
        result.error.as_ref().map(|error| error.kind),
        Some(CodeRunFailureKind::Exception),
        "{result:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_code_cannot_observe_the_parent_environment() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let runtime = NodeCodeRuntime::install(&ctx, Config::default()).expect("install runtime");

    let result = runtime
        .run(CodeRunRequest {
            program: "const ambient = globalThis.constructor.constructor('return process')(); return ambient.env.HOME ?? ambient.env.USERPROFILE ?? null;".to_string(),
            bindings: Vec::new(),
            signal: None,
        })
        .await
        .expect("service contract");

    assert_eq!(
        result.error.as_ref().map(|error| error.kind),
        Some(CodeRunFailureKind::Exception),
        "{result:?}"
    );
    assert!(
        result
            .error
            .as_ref()
            .is_some_and(|error| error.message.contains("process is not defined")),
        "{:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn synchronous_loop_stops_at_the_compute_budget() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let runtime = NodeCodeRuntime::install(
        &ctx,
        Config {
            compute_ms: 150,
            max_wall_ms: 5_000,
            ..Config::default()
        },
    )
    .expect("install runtime");
    let fallback = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fallback_for_timer = fallback.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        fallback_for_timer.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let signal: dsh_code_runtime::CodeAbort =
        Arc::new(move || fallback.load(std::sync::atomic::Ordering::SeqCst));

    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        runtime.run(CodeRunRequest {
            program: "for (;;) {}".to_string(),
            bindings: Vec::new(),
            signal: Some(signal),
        }),
    )
    .await
    .expect("compute timeout remains bounded")
    .expect("service contract");

    assert_eq!(
        result.error.as_ref().map(|error| error.kind),
        Some(dsh_code_runtime::CodeRunFailureKind::Timeout)
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_for_a_slow_rust_binding_does_not_consume_compute_budget() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let runtime = NodeCodeRuntime::install(
        &ctx,
        Config {
            compute_ms: 150,
            max_wall_ms: 2_000,
            ..Config::default()
        },
    )
    .expect("install runtime");
    let slow: dsh_code_runtime::CodeBindingFunction = Arc::new(|_| {
        Box::pin(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            serde_json::json!("slow-done")
        })
    });

    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        runtime.run(CodeRunRequest {
            program: "return await tools.slow({})".to_string(),
            bindings: vec![CodeBindingNamespace {
                global: "tools".to_string(),
                functions: vec![("slow".to_string(), slow)],
                error_class: None,
            }],
            signal: None,
        }),
    )
    .await
    .expect("slow binding remains bounded")
    .expect("service contract");

    assert_eq!(result.value, Some(serde_json::json!("slow-done")));
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(started.elapsed() >= std::time::Duration::from_millis(500));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_abort_stops_a_synchronous_loop_as_abort() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let runtime = NodeCodeRuntime::install(
        &ctx,
        Config {
            compute_ms: 30_000,
            max_wall_ms: 30_000,
            ..Config::default()
        },
    )
    .expect("install runtime");
    let aborted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let timer_flag = aborted.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        timer_flag.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let signal: dsh_code_runtime::CodeAbort =
        Arc::new(move || aborted.load(std::sync::atomic::Ordering::SeqCst));

    let started = std::time::Instant::now();
    let result = runtime
        .run(CodeRunRequest {
            program: "for (;;) {}".to_string(),
            bindings: Vec::new(),
            signal: Some(signal),
        })
        .await
        .expect("service contract");

    assert_eq!(
        result.error.as_ref().map(|error| error.kind),
        Some(dsh_code_runtime::CodeRunFailureKind::Abort)
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn never_settling_binding_stops_at_the_wall_deadline() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let runtime = NodeCodeRuntime::install(
        &ctx,
        Config {
            compute_ms: 30_000,
            max_wall_ms: 250,
            ..Config::default()
        },
    )
    .expect("install runtime");
    let never: dsh_code_runtime::CodeBindingFunction =
        Arc::new(|_| Box::pin(async move { std::future::pending::<serde_json::Value>().await }));

    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        runtime.run(CodeRunRequest {
            program: "await tools.never({}); return 1".to_string(),
            bindings: vec![CodeBindingNamespace {
                global: "tools".to_string(),
                functions: vec![("never".to_string(), never)],
                error_class: None,
            }],
            signal: None,
        }),
    )
    .await
    .expect("wall timeout remains bounded")
    .expect("service contract");

    assert_eq!(
        result.error.as_ref().map(|error| error.kind),
        Some(dsh_code_runtime::CodeRunFailureKind::Timeout)
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unawaited_pending_binding_does_not_pause_busy_compute_accounting() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let runtime = NodeCodeRuntime::install(
        &ctx,
        Config {
            compute_ms: 150,
            max_wall_ms: 30_000,
            ..Config::default()
        },
    )
    .expect("install runtime");
    let never: dsh_code_runtime::CodeBindingFunction =
        Arc::new(|_| Box::pin(async move { std::future::pending::<serde_json::Value>().await }));

    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        runtime.run(CodeRunRequest {
            program: "tools.never({}); for (;;) {}".to_string(),
            bindings: vec![CodeBindingNamespace {
                global: "tools".to_string(),
                functions: vec![("never".to_string(), never)],
                error_class: None,
            }],
            signal: None,
        }),
    )
    .await
    .expect("unawaited binding must not mask compute timeout")
    .expect("service contract");

    assert_eq!(
        result.error.as_ref().map(|error| error.kind),
        Some(CodeRunFailureKind::Timeout)
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logs_and_value_share_one_exact_serialized_output_budget() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let max_output_bytes = 128;
    let runtime = NodeCodeRuntime::install(
        &ctx,
        Config {
            max_output_bytes,
            ..Config::default()
        },
    )
    .expect("install runtime");

    let result = runtime
        .run(CodeRunRequest {
            program: "console.log('é'.repeat(80)); return '界'.repeat(80);".to_string(),
            bindings: Vec::new(),
            signal: None,
        })
        .await
        .expect("service contract");

    assert_eq!(
        result.error.as_ref().map(|error| error.kind),
        Some(CodeRunFailureKind::OutputLimit)
    );
    assert!(result.value.is_none());
    assert!(result.logs.is_empty());
    assert!(
        payload_bytes(&result) <= max_output_bytes as usize,
        "serialized payload was {} bytes: {result:?}",
        payload_bytes(&result)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_json_completion_value_is_rejected_without_lossy_coercion() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let runtime = NodeCodeRuntime::install(&ctx, Config::default()).expect("install runtime");

    let result = runtime
        .run(CodeRunRequest {
            program: "return { callable: () => 1 };".to_string(),
            bindings: Vec::new(),
            signal: None,
        })
        .await
        .expect("service contract");

    assert_eq!(
        result.error.as_ref().map(|error| error.kind),
        Some(CodeRunFailureKind::InvalidOutput)
    );
    assert!(result.value.is_none());
    assert!(
        result
            .error
            .as_ref()
            .is_some_and(|error| error.message.contains("$.callable")),
        "{:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_erasable_typescript_resolves_as_a_program_exception() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let runtime = NodeCodeRuntime::install(&ctx, Config::default()).expect("install runtime");

    let result = runtime
        .run(CodeRunRequest {
            program: "enum Direction { Up, Down } return Direction.Up;".to_string(),
            bindings: Vec::new(),
            signal: None,
        })
        .await
        .expect("service contract");

    assert_eq!(
        result.error.as_ref().map(|error| error.kind),
        Some(CodeRunFailureKind::Exception)
    );
    assert!(result.value.is_none());
    assert!(
        result.error.as_ref().is_some_and(|error| {
            error.message.contains("enum") || error.message.contains("TypeScript")
        }),
        "{:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_worker_heap_oom_is_worker_exit_and_followup_is_healthy() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let runtime = NodeCodeRuntime::install(
        &ctx,
        Config {
            compute_ms: 30_000,
            max_wall_ms: 10_000,
            max_old_generation_size_mb: 16,
            ..Config::default()
        },
    )
    .expect("install runtime");

    let oom = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        runtime.run(CodeRunRequest {
            program: "const hog = []; for (let i = 0; i < 80; i++) hog.push(new Array(250_000).fill(i)); return hog.length;".to_string(),
            bindings: Vec::new(),
            signal: None,
        }),
    )
    .await
    .expect("OOM remains bounded")
    .expect("service contract");
    assert_eq!(
        oom.error.as_ref().map(|error| error.kind),
        Some(CodeRunFailureKind::WorkerExit)
    );

    let followup = runtime
        .run(CodeRunRequest {
            program: "return 'healthy-after-oom';".to_string(),
            bindings: Vec::new(),
            signal: None,
        })
        .await
        .expect("service contract");
    assert_eq!(followup.value, Some(serde_json::json!("healthy-after-oom")));
    assert!(followup.error.is_none(), "{:?}", followup.error);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispose_aborts_active_run_and_rejects_followup() {
    let ctx = Context::root();
    let _subprocess = LocalSubprocessRuntime::install(&ctx);
    let runtime = NodeCodeRuntime::install(&ctx, Config::default()).expect("install runtime");
    let run = tokio::spawn(runtime.run(CodeRunRequest {
        program: "for (;;) {}".to_string(),
        bindings: Vec::new(),
        signal: None,
    }));
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    tokio::time::timeout(std::time::Duration::from_secs(3), runtime.dispose())
        .await
        .expect("dispose awaits the process tree");
    let result = run.await.expect("run task").expect("service contract");
    assert_eq!(
        result.error.as_ref().map(|error| error.kind),
        Some(CodeRunFailureKind::Abort)
    );
    let rejected = runtime
        .run(CodeRunRequest {
            program: "return 1;".to_string(),
            bindings: Vec::new(),
            signal: None,
        })
        .await
        .expect_err("disposed runtime rejects admission");
    assert!(rejected.contains("disposed"), "{rejected}");
}
