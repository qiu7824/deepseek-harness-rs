use std::sync::Arc;

use cordis::Context;
use dsh_cordis_loader::{EntryOptions, LoaderService};
use serde_json::json;

#[tokio::test]
async fn loader_resolves_and_mounts_compaction_preset_plugins() {
    let ctx = Context::root();
    dsh_session::SessionStore::install(&ctx);
    dsh_token_meter::TokenMeter::install(&ctx, Default::default());
    dsh_llm::LlmRuntime::install(&ctx);
    dsh_commands::CommandRuntime::install(&ctx);
    let fiber = ctx.plugin(dsh_cordis_loader::plugin(), cordis::arc(()));
    fiber.settle().await.unwrap();
    let loader = ctx
        .get_typed::<Arc<LoaderService>>("loader", true)
        .unwrap()
        .as_ref()
        .clone();
    loader.core.register(
        "@deepseek-ai/dsh-compaction-basic",
        dsh_compaction::basic::plugin(),
    );
    loader.core.register(
        "@deepseek-ai/dsh-command-compact",
        dsh_command_compact::plugin(),
    );
    loader.core.register(
        "@deepseek-ai/dsh-compaction-tool-result-pruner",
        dsh_compaction_tool_result_pruner::plugin(),
    );
    for (name, config) in [
        (
            "@deepseek-ai/dsh-compaction-tool-result-pruner",
            json!({ "thresholdChars": 100, "headChars": 20, "tailChars": 10 }),
        ),
        (
            "@deepseek-ai/dsh-compaction-basic",
            json!({ "maxTokens": 512, "auto": false }),
        ),
        ("@deepseek-ai/dsh-command-compact", json!({})),
    ] {
        loader
            .tree
            .create(
                EntryOptions {
                    name: name.into(),
                    config: Some(config),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap();
    }
    loader.tree.await_ready().await.unwrap();
    assert!(ctx.get("toolResultPruner", true).is_some());
    assert!(ctx.get("compaction", true).is_some());
}
