use cordis::{ArcValue, Context, Plugin, PluginError, arc};
use dsh_cordis_loader::{EntryOptions, LoaderCore, LoaderService};
use serde_json::json;
use std::sync::Arc;

struct Probe(Arc<LoaderCore>);
#[async_trait::async_trait]
impl Plugin for Probe {
    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let entry = self
            .0
            .entry_of(&ctx.fiber)
            .ok_or_else(|| PluginError::new(arc("entry missing at activation".to_owned())))?;
        if !entry
            .fiber
            .lock()
            .as_ref()
            .is_some_and(|fiber| Arc::ptr_eq(fiber, &ctx.fiber))
        {
            return Err(PluginError::new(arc(
                "current fiber missing at activation".to_owned()
            )));
        }
        if cordis::downcast::<serde_json::Value>(&config).is_some_and(|value| value["fail"] == true)
        {
            return Err(PluginError::new(arc("fixture startup failure".to_owned())));
        }
        Ok(())
    }
}
fn group(fail: bool) -> EntryOptions {
    serde_json::from_value(json!({"id":"planning","name":"cordis:group","group":true,"isolate":{"planning":true},"config":[
        {"id":"first","name":"probe","config":{}},
        {"id":"second","name":"probe","config":{"fail":fail}}
    ]})).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn group_failure_cleans_partial_entries_and_replacement_ignores_late_disposal() {
    let ctx = Context::root();
    let loader = LoaderService::new(&ctx).await;
    loader
        .core
        .register("probe", Arc::new(Probe(loader.core.clone())));
    let root = loader.tree.root_group();
    assert!(root.create(group(true)).await.is_err());
    assert!(
        loader.tree.entries().is_empty(),
        "failed initialization retained a child entry"
    );
    assert!(loader.core.entries_by_fiber.lock().is_empty());
    assert!(loader.core.carrier_fibers.lock().is_empty());
    root.create(group(false)).await.unwrap();
    let entry = loader.tree.resolve("planning").unwrap();
    let old = entry.fiber.lock().clone().unwrap();
    // The candidate fails after its first child activates; rollback must make
    // exactly one new usable group with the preceding composition.
    assert!(root.create(group(true)).await.is_err());
    let restored = entry.fiber.lock().clone().unwrap();
    assert!(!Arc::ptr_eq(&old, &restored));
    restored.settle().await.unwrap();
    assert_eq!(loader.core.entries_by_fiber.lock().len(), 3);
    ctx.events
        .parallel(None, "internal/plugin", vec![arc(old.clone())])
        .await;
    assert!(
        entry
            .fiber
            .lock()
            .as_ref()
            .is_some_and(|fiber| Arc::ptr_eq(fiber, &restored))
    );
    assert!(loader.core.entry_of(&old).is_none());
    assert!(loader.core.entry_of(&restored).is_some());
    assert!(!entry.disabled().unwrap());
    root.stop().await.unwrap();
    assert!(loader.tree.entries().is_empty());
    assert!(loader.core.entries_by_fiber.lock().is_empty());
    assert!(loader.core.carrier_fibers.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn config_hooks_observe_the_entry_before_plugin_apply() {
    let ctx = Context::root();
    let loader = LoaderService::new(&ctx).await;
    loader
        .core
        .register("probe", Arc::new(Probe(loader.core.clone())));
    let core = loader.core.clone();
    let _hook = ctx
        .on(
            "internal/config",
            Arc::new(move |ctx, args| {
                let core = core.clone();
                let fiber = ctx.fiber.clone();
                Box::pin(async move {
                    let next = cordis::downcast::<cordis::NextFn>(args.last().unwrap()).unwrap();
                    assert!(
                        core.entry_of(&fiber).is_some(),
                        "config ran before ownership was installed"
                    );
                    Some(next.call().await)
                })
            }),
            cordis::EventOptions::default().global(true).prepend(true),
        )
        .await;
    for _ in 0..12 {
        loader.tree.root_group().create(group(false)).await.unwrap();
    }
    loader.tree.root_group().stop().await.unwrap();
    assert!(loader.core.entries_by_fiber.lock().is_empty());
}
