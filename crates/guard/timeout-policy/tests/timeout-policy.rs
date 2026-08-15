//! Rust port of the core `timeout-policy.spec.ts` behaviors: delegation
//! without a budget, derived-signal dispatch, post-execute restoration,
//! TOOL_TIMEOUT replacement when this plugin's own deadline wins, upstream
//! abort preservation, and fiber disposal — driven through the real
//! `ToolRuntime` pipeline with real (short) timers instead of fake ones.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cordis::{Context, arc};
use dsh_llm::{ContentBlock, call_id};
use dsh_system_prompt::SystemPrompt;
use dsh_timeout_policy::{INJECT, NAME, TOOL_TIMEOUT, TimeoutPolicyPlugin};
use dsh_tools::{
    AbortPredicate, TOOL_ABORTED, ToolBodyError, ToolDefinition, ToolExecutionInput,
    ToolOutputDefinition, ToolRunContext, ToolRuntime,
};

fn never_abort() -> AbortPredicate {
    Arc::new(|| false)
}

fn abort_flag(flag: Arc<AtomicBool>) -> AbortPredicate {
    Arc::new(move || flag.load(Ordering::SeqCst))
}

async fn setup() -> Context {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    ToolRuntime::install(
        &ctx,
        dsh_tools::Config {
            mode: None,
            max_parallel_sub_calls: None,
        },
    )
    .expect("tools");
    let disposer = dsh_timeout_policy::apply(&ctx);
    (disposer)().await;
    ctx
}

/// A tool whose body returns the text value verbatim as its only content
/// block.
fn content_tool(
    name: &str,
    timeout_ms: Option<u64>,
    execute: Arc<
        dyn Fn(&serde_json::Value, &ToolRunContext)
            -> cordis::BoxFuture<'static, Result<serde_json::Value, ToolBodyError>>
            + Send
            + Sync,
    >,
) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: "d".to_string(),
        parameters: serde_json::json!({}),
        output: ToolOutputDefinition {
            schema: serde_json::json!({}),
            render: Arc::new(|_args, value| {
                Ok(vec![ContentBlock::Text {
                    text: value.as_str().unwrap_or_default().to_string(),
                }])
            }),
            presentation_meta: None,
        },
        timeout_ms,
        is_concurrency_safe: None,
        execute,
        finalize_content: None,
        present_call: None,
        present_result: None,
    }
}

fn input(name: &str, signal: AbortPredicate) -> ToolExecutionInput {
    ToolExecutionInput {
        call_id: call_id(&format!("c-{name}")),
        root_call_id: None,
        name: name.to_string(),
        arguments: serde_json::json!({}),
        agent: None,
        parent: None,
        signal,
    }
}

#[test]
fn exposes_the_owned_contract() {
    assert_eq!(TOOL_TIMEOUT, "TOOL_TIMEOUT");
    assert_eq!(NAME, "timeout-policy");
    assert_eq!(INJECT, ["tools"]);
    let _plugin = TimeoutPolicyPlugin;
    let _ = dsh_timeout_policy::tool_timeout_result(100);
}

#[tokio::test(flavor = "current_thread")]
async fn delegates_an_unbudgeted_tool_without_touching_the_signal() {
    let ctx = setup().await;
    let upstream_flag = Arc::new(AtomicBool::new(false));
    let seen: Arc<std::sync::Mutex<Option<AbortPredicate>>> =
        Arc::new(std::sync::Mutex::new(None));
    let seen_for_body = seen.clone();
    let tool = content_tool("probe", None, Arc::new(move |_args, run_ctx| {
        let seen = seen_for_body.clone();
        let predicate = run_ctx.execution.signal.lock().clone();
        Box::pin(async move {
            *seen.lock().expect("seen") = Some(predicate);
            Ok(serde_json::json!("ok"))
        })
    }));
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .expect("tools");
    tools.register(&ctx, tool).expect("register");
    let upstream = abort_flag(upstream_flag.clone());
    let result = tools.execute(input("probe", upstream)).await;
    assert!(!result.is_error);
    assert_eq!(result.content, vec![ContentBlock::Text { text: "ok".into() }]);
    // No policy swap for an unbudgeted tool: the body's signal still tracks
    // the upstream abort (the Rust runtime derives the body signal by fusing
    // wrapper and caller predicates — the identity differs from the TS
    // pass-through, the behavior does not).
    let seen = seen.lock().expect("seen").clone().expect("seen");
    assert!(!seen(), "unaborted while idle");
    upstream_flag.store(true, Ordering::SeqCst);
    assert!(seen(), "tracks the upstream abort");
}

#[tokio::test(flavor = "current_thread")]
async fn a_budgeted_tool_keeps_its_own_result_and_sees_a_derived_signal() {
    let ctx = setup().await;
    let seen: Arc<std::sync::Mutex<Option<AbortPredicate>>> =
        Arc::new(std::sync::Mutex::new(None));
    let seen_for_body = seen.clone();
    let tool = content_tool("fast", Some(10_000), Arc::new(move |_args, run_ctx| {
        let seen = seen_for_body.clone();
        let predicate = run_ctx.execution.signal.lock().clone();
        Box::pin(async move {
            *seen.lock().expect("seen") = Some(predicate);
            Ok(serde_json::json!("ok"))
        })
    }));
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .expect("tools");
    tools.register(&ctx, tool).expect("register");
    let upstream = never_abort();
    let result = tools.execute(input("fast", upstream.clone())).await;
    assert!(!result.is_error);
    assert_eq!(result.content, vec![ContentBlock::Text { text: "ok".into() }]);
    let seen = seen.lock().expect("seen").clone().expect("seen");
    assert!(
        !Arc::ptr_eq(&seen, &upstream),
        "dispatch sees the derived deadline signal"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn restores_the_caller_signal_for_post_execute() {
    let ctx = setup().await;
    let tool = content_tool("fast", Some(10_000), Arc::new(|_args, _run_ctx| {
        Box::pin(async { Ok(serde_json::json!("ok")) })
    }));
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .expect("tools");
    tools.register(&ctx, tool).expect("register");

    let upstream = never_abort();
    let post_seen: Arc<std::sync::Mutex<Option<AbortPredicate>>> =
        Arc::new(std::sync::Mutex::new(None));
    let post_seen_for_listener = post_seen.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
        let exec = args
            .first()
            .and_then(|value| value.downcast_ref::<Arc<dsh_tools::ToolExecution>>())
            .cloned()
            .expect("exec");
        let next = cordis::downcast_arc::<cordis::NextFn>(args.last().expect("next"))
            .expect("next");
        let post_seen = post_seen_for_listener.clone();
        Box::pin(async move {
            *post_seen.lock().expect("post") = Some(exec.signal.lock().clone());
            Some(next.call().await)
        })
    });
    ctx.on("tools/post-execute", listener, Default::default()).await;

    let result = tools.execute(input("fast", upstream.clone())).await;
    assert!(!result.is_error);
    let post_seen = post_seen.lock().expect("post").clone().expect("seen");
    assert!(
        Arc::ptr_eq(&post_seen, &upstream),
        "post-execute sees the restored caller signal"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn replaces_a_cooperative_result_with_tool_timeout_when_the_deadline_wins() {
    let ctx = setup().await;
    let tool = content_tool("slow", Some(100), Arc::new(|_args, run_ctx| {
        let predicate = run_ctx.execution.signal.lock().clone();
        Box::pin(async move {
            let mut spins = 0;
            loop {
                if predicate() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
                spins += 1;
                assert!(spins < 1000, "deadline never fired");
            }
            Ok(serde_json::json!("stopped cooperatively"))
        })
    }));
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .expect("tools");
    tools.register(&ctx, tool).expect("register");
    let result = tools.execute(input("slow", never_abort())).await;
    assert!(result.is_error);
    let error = result.error.clone().expect("error");
    assert_eq!(error.message, "tool call timed out after 100ms");
    let info = error.info.expect("info");
    assert_eq!(info.name, "ToolTimeoutError");
    assert_eq!(info.code, TOOL_TIMEOUT);
    assert_eq!(
        result.content,
        vec![ContentBlock::Text {
            text: "Error: tool call timed out after 100ms".into()
        }]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn replaces_a_provider_abort_error_when_the_deadline_wins() {
    let ctx = setup().await;
    let tool = content_tool("aborter", Some(100), Arc::new(|_args, run_ctx| {
        let predicate = run_ctx.execution.signal.lock().clone();
        Box::pin(async move {
            let mut spins = 0;
            loop {
                if predicate() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
                spins += 1;
                assert!(spins < 1000, "deadline never fired");
            }
            Err(ToolBodyError::coded(
                "web fetch aborted",
                "HarnessError",
                "WEB_ABORTED",
            ))
        })
    }));
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .expect("tools");
    tools.register(&ctx, tool).expect("register");
    let result = tools.execute(input("aborter", never_abort())).await;
    assert!(result.is_error);
    let error = result.error.clone().expect("error");
    assert_eq!(error.message, "tool call timed out after 100ms");
    let info = error.info.expect("info");
    assert_eq!(info.name, "ToolTimeoutError");
    assert_eq!(info.code, TOOL_TIMEOUT);
}

#[tokio::test(flavor = "current_thread")]
async fn preserves_the_registry_aborted_result_when_the_caller_aborts_first() {
    let ctx = setup().await;
    let upstream_flag = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(AtomicBool::new(false));
    let entered_for_body = entered.clone();
    let tool = content_tool("slow", Some(100), Arc::new(move |_args, run_ctx| {
        let predicate = run_ctx.execution.signal.lock().clone();
        let entered = entered_for_body.clone();
        Box::pin(async move {
            entered.store(true, Ordering::SeqCst);
            let mut spins = 0;
            loop {
                if predicate() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
                spins += 1;
                assert!(spins < 1000, "abort never arrived");
            }
            Ok(serde_json::json!("stopped cooperatively"))
        })
    }));
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .expect("tools");
    tools.register(&ctx, tool).expect("register");
    let tools_for_spawn = tools.clone();
    let flag_for_spawn = upstream_flag.clone();
    let pending = tokio::spawn(async move {
        tools_for_spawn
            .execute(input("slow", abort_flag(flag_for_spawn)))
            .await
    });
    let mut spins = 0;
    while !entered.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(5)).await;
        spins += 1;
        assert!(spins < 1000, "tool never entered");
    }
    upstream_flag.store(true, Ordering::SeqCst);
    let result = pending.await.expect("join");
    assert!(result.is_error);
    let error = result.error.clone().expect("error");
    assert_eq!(error.message, "tool call aborted");
    let info = error.info.expect("info");
    assert_eq!(info.name, "AbortError");
    assert_eq!(info.code, TOOL_ABORTED);
}

#[tokio::test(flavor = "current_thread")]
async fn preserves_tool_timeout_when_the_deadline_wins_before_a_later_caller_abort() {
    let ctx = setup().await;
    let upstream_flag = Arc::new(AtomicBool::new(false));
    let saw_abort = Arc::new(AtomicBool::new(false));
    let saw_abort_for_body = saw_abort.clone();
    let release = Arc::new(AtomicBool::new(false));
    let release_for_body = release.clone();
    let tool = content_tool("slow-cleanup", Some(100), Arc::new(move |_args, run_ctx| {
        let predicate = run_ctx.execution.signal.lock().clone();
        let saw_abort = saw_abort_for_body.clone();
        let release = release_for_body.clone();
        Box::pin(async move {
            let mut spins = 0;
            loop {
                if predicate() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
                spins += 1;
                assert!(spins < 1000, "deadline never fired");
            }
            saw_abort.store(true, Ordering::SeqCst);
            let mut spins = 0;
            while !release.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
                spins += 1;
                assert!(spins < 1000, "release never arrived");
            }
            Ok(serde_json::json!("cleanup complete"))
        })
    }));
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .expect("tools");
    tools.register(&ctx, tool).expect("register");
    let tools_for_spawn = tools.clone();
    let flag_for_spawn = upstream_flag.clone();
    let pending = tokio::spawn(async move {
        tools_for_spawn
            .execute(input("slow-cleanup", abort_flag(flag_for_spawn)))
            .await
    });
    let mut spins = 0;
    while !saw_abort.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(5)).await;
        spins += 1;
        assert!(spins < 1000, "body never saw the abort");
    }
    upstream_flag.store(true, Ordering::SeqCst); // too late: our timer already won
    release.store(true, Ordering::SeqCst);
    let result = pending.await.expect("join");
    assert!(result.is_error);
    let error = result.error.clone().expect("error");
    assert_eq!(error.message, "tool call timed out after 100ms");
    let info = error.info.expect("info");
    assert_eq!(info.name, "ToolTimeoutError");
    assert_eq!(info.code, TOOL_TIMEOUT);
}

#[tokio::test(flavor = "current_thread")]
async fn fiber_disposal_removes_the_listener() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).expect("system prompt");
    ToolRuntime::install(
        &ctx,
        dsh_tools::Config {
            mode: None,
            max_parallel_sub_calls: None,
        },
    )
    .expect("tools");
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .expect("tools");
    // The body reports whether its dispatch signal ever aborted within a
    // bounded window — the only stable observable of the wrapper's derived
    // deadline (the Rust runtime derives the body signal either way).
    let tool = content_tool("probe", Some(80), Arc::new(|_args, run_ctx| {
        let predicate = run_ctx.execution.signal.lock().clone();
        Box::pin(async move {
            for _ in 0..40 {
                if predicate() {
                    return Ok(serde_json::json!("aborted:true"));
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Ok(serde_json::json!("aborted:false"))
        })
    }));
    tools.register(&ctx, tool).expect("register");

    let fiber = ctx.plugin(Arc::new(TimeoutPolicyPlugin), arc(()));
    fiber.settle().await.expect("settle");
    let result = tools.execute(input("probe", never_abort())).await;
    assert!(result.is_error, "the mounted wrapper arms the deadline");
    let error = result.error.clone().expect("error");
    assert_eq!(error.message, "tool call timed out after 80ms");

    fiber.dispose().await;
    let result = tools.execute(input("probe", never_abort())).await;
    assert!(!result.is_error, "no deadline after disposal");
    assert_eq!(
        result.content,
        vec![ContentBlock::Text {
            text: "aborted:false".into()
        }]
    );
}
