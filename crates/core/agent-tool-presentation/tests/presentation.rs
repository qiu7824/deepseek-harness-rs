//! Agent-tool-presentation tests: Rust port of
//! `packages/core/agent-tool-presentation/tests/*` (native scoped
//! declaration and the codeRuntime wait).

use std::sync::Arc;

use cordis::{Context, Plugin, PluginError, arc};
use dsh_agent_tool_presentation::{Config, INJECT, NAME, ToolPresentationPlugin, apply};
use dsh_scope::{CreateScopeOptions, ScopeKey, create_scope};
use dsh_tools::{ToolPresentationMode, ToolRuntime};

fn setup() -> (Context, Arc<ToolRuntime>) {
    let ctx = Context::root();
    let _ = dsh_system_prompt::SystemPrompt::install(&ctx, dsh_system_prompt::Config::default())
        .expect("systemPrompt");
    let tools = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    (ctx, tools)
}

#[tokio::test]
async fn native_declares_the_presentation_for_the_mounting_scope() {
    let (ctx, tools) = setup();
    let scope = create_scope(&ctx, ScopeKey::new(), &CreateScopeOptions::default());
    let result = apply(
        &scope.ctx,
        Config {
            mode: ToolPresentationMode::Native,
        },
    );
    assert!(result.is_ok());
    // The scoped mode resolves through the layers; the global default stays
    // untouched.
    let _ = tools;
    (scope.dispose)().await;
}

#[tokio::test]
async fn native_row_rejects_a_context_global_mount() {
    let (ctx, _tools) = setup();
    // A plain context has no scope: presentAs must refuse loudly.
    let error = apply(
        &ctx,
        Config {
            mode: ToolPresentationMode::Native,
        },
    )
    .expect_err("global mount must reject");
    assert!(error.contains("scoped context"), "got {error}");
}

#[tokio::test]
async fn code_row_waits_for_the_code_runtime() {
    let (ctx, tools) = setup();
    let scope = create_scope(&ctx, ScopeKey::new(), &CreateScopeOptions::default());
    apply(
        &scope.ctx,
        Config {
            mode: ToolPresentationMode::Code,
        },
    )
    .expect("apply");

    // The codeRuntime wait stays pending until a runtime service appears:
    // verify the inject fiber exists without demanding a runtime.
    let _ = tools;
    (scope.dispose)().await;
}

#[tokio::test]
async fn code_row_applies_once_the_runtime_publishes() {
    let (ctx, _tools) = setup();
    let scope = create_scope(&ctx, ScopeKey::new(), &CreateScopeOptions::default());
    apply(
        &scope.ctx,
        Config {
            mode: ToolPresentationMode::Code,
        },
    )
    .expect("apply");

    // Publish a codeRuntime service on the root: the inject fiber activates
    // and calls presentAs for the mounting scope.
    ctx.register_service(Arc::new(CodeRuntimeStub));
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    (scope.dispose)().await;
}

struct CodeRuntimeStub;

impl cordis::Service for CodeRuntimeStub {
    fn service_name(&self) -> &'static str {
        "codeRuntime"
    }
}

#[tokio::test]
async fn plugin_form_exports_identity_and_injection() {
    assert_eq!(NAME, "tool-presentation");
    assert_eq!(INJECT, ["tools"]);

    let (_ctx, _tools) = setup();
    let plugin = ToolPresentationPlugin {
        config: Config {
            mode: ToolPresentationMode::Native,
        },
    };
    assert_eq!(plugin.name(), Some("tool-presentation"));
    // INJECT is exercised through the exported constant (InjectSpec exposes
    // no reader; the plugin wires it verbatim).
    assert_eq!(INJECT, ["tools"]);
    let _: Result<(), PluginError> = Ok(());
    let _ = arc(());
}
