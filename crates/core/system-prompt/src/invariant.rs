//! Package-owned prompt-assembly invariants. Rust port of
//! `packages/core/system-prompt/src/invariant.ts`.
//!
//! The TS runtime checks `typeof section.text !== 'string'` and variable
//! value types; Rust's `String`/`Option<String>` fields make those checks
//! type-enforced, so only the shape checks that can still fail remain.

use std::sync::Arc;

use cordis::{BoxFuture, Context, Disposer, EventOptions, Listener, NextFn, downcast};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

use crate::{PromptAssembly, SharedAssembly};

const PACKAGE_NAME: &str = "@deepseek-ai/dsh-system-prompt";

/// Cordis companion plugin name.
pub const NAME: &str = "system-prompt-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// Validate the authoritative assembly returned by the waterfall.
fn validate_assembly(assembly: &PromptAssembly, fail: &dyn Fn(&str)) {
    let mut section_names = std::collections::HashSet::new();
    for section in &assembly.sections {
        if section.name.is_empty() {
            fail("assembled section names must be non-empty");
        }
        if !section_names.insert(section.name.clone()) {
            fail(&format!(
                "assembled section name {} is duplicated",
                serde_json::to_string(&section.name).unwrap_or_default()
            ));
        }
    }

    let mut context_names = std::collections::HashSet::new();
    for context in &assembly.contexts {
        if context.name.is_empty() {
            fail("assembled context names must be non-empty");
        }
        if !context_names.insert(context.name.clone()) {
            fail(&format!(
                "assembled context name {} is duplicated",
                serde_json::to_string(&context.name).unwrap_or_default()
            ));
        }
    }

    for tool in &assembly.tools {
        if tool.name.is_empty() {
            fail("assembled tool names must be non-empty");
        }
    }

    for name in assembly.variables.keys() {
        if !crate::is_valid_variable_name(name) {
            fail(&format!(
                "assembled variable name {} is invalid",
                serde_json::to_string(name).unwrap_or_default()
            ));
        }
    }
}

/// Register the system-prompt invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> BoxFuture<'static, Disposer> {
    let ctx = ctx.clone();
    Box::pin(async move {
        let invariants = ctx
            .get_typed::<Arc<InvariantRegistry>>("invariants", false)
            .expect("invariants service required by system-prompt-invariant");
        invariants.register(
            &ctx,
            PACKAGE_NAME,
            InvariantInstaller {
                install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
                    let ctx = ctx.clone();
                    Box::pin(async move { install_inner(&ctx, fail).await })
                }),
                inject: None,
            },
        )
    })
}

/// Install validation around the authoritative assembly waterfall result
/// (TS `install`): a prepended global listener awaits `next()` so it sees
/// the FINAL assembly, validates it, and passes it through.
async fn install_inner(ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>) {
    let listener: Arc<Listener> = Arc::new(move |_ctx: &Context, args: Vec<cordis::ArcValue>| {
        let fail = Arc::clone(&fail);
        Box::pin(async move {
            let value = downcast::<NextFn>(&args[2])
                .expect("system-prompt/assemble next continuation")
                .call()
                .await;
            let assembled = downcast::<SharedAssembly>(&value)
                .expect("system-prompt/assemble must resolve a PromptAssembly")
                .snapshot();
            validate_assembly(&assembled, &*fail);
            Some(value)
        })
    });
    ctx.on(
        "system-prompt/assemble",
        listener,
        EventOptions::default().global(true).prepend(true),
    )
    .await;
}
