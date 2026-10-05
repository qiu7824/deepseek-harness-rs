use cordis::{ArcValue, Context, FiberState, Plugin, PluginError, arc};
use dsh_cordis_loader::{EntryOptions, EntryTree, LoaderCore, LoaderService};
use serde_json::json;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;

struct Parent;
#[async_trait::async_trait]
impl Plugin for Parent {
    async fn apply(&self, _: &Context, _: ArcValue) -> Result<(), PluginError> {
        Ok(())
    }
}

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

#[tokio::test(flavor = "current_thread")]
async fn parent_disposal_releases_entry_ownership_before_returning_from_publication() {
    let ctx = Context::root();
    let loader = LoaderService::new(&ctx).await;
    loader
        .core
        .register("probe", Arc::new(Probe(loader.core.clone())));
    let parent = ctx.plugin(Arc::new(Parent), arc(()));
    parent.settle().await.unwrap();
    let tree = EntryTree::new(parent.ctx().unwrap(), loader.core.clone());
    tree.root_group().create(group(false)).await.unwrap();
    let fibers = tree
        .entries()
        .into_iter()
        .map(|entry| entry.fiber.lock().clone().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(fibers.len(), 3);
    assert_eq!(loader.core.carrier_fibers.lock().len(), 1);
    let keys = fibers
        .iter()
        .map(|fiber| Arc::as_ptr(fiber) as usize)
        .collect::<Vec<_>>();
    let core = loader.core.clone();
    let publications = Arc::new(AtomicUsize::new(0));
    let observed = publications.clone();
    // This listener follows the loader's listener. Its construction runs in
    // the disposing call, before any detached listener future can be polled.
    let observer = ctx
        .on(
            "internal/plugin",
            Arc::new(move |_, args| {
                if let Some(fiber) = args
                    .first()
                    .and_then(cordis::downcast::<Arc<cordis::FiberCore>>)
                {
                    let key = Arc::as_ptr(fiber) as usize;
                    if fiber.uid_value().is_none() && keys.contains(&key) {
                        assert!(
                            core.entry_of(fiber).is_none(),
                            "disposed entry ownership must be released during publication"
                        );
                        assert!(!core.carrier_fibers.lock().contains(&key));
                        observed.fetch_add(1, Ordering::SeqCst);
                    }
                }
                Box::pin(async { None })
            }),
            cordis::EventOptions::default().global(true),
        )
        .await;
    parent.dispose().await;
    assert_eq!(parent.state(), FiberState::Disposed);
    assert_eq!(publications.load(Ordering::SeqCst), 3);
    assert!(fibers.iter().all(|fiber| fiber.uid_value().is_none()));
    assert!(loader.core.entries_by_fiber.lock().is_empty());
    assert!(loader.core.carrier_fibers.lock().is_empty());
    tree.root_group().stop().await.unwrap();
    observer().await;
}

#[tokio::test(flavor = "current_thread")]
async fn reload_preserves_entry_ownership_and_disposal_releases_it_before_waiting_for_unload() {
    let ctx = Context::root();
    let loader = LoaderService::new(&ctx).await;
    loader
        .core
        .register("probe", Arc::new(Probe(loader.core.clone())));
    loader.tree.root_group().create(group(false)).await.unwrap();
    let entry = loader.tree.resolve("planning").unwrap();
    let fiber = entry.fiber.lock().clone().unwrap();
    let key = Arc::as_ptr(&fiber) as usize;
    let config = entry.options.lock().config.clone().unwrap();
    fiber.restart().await.unwrap();
    fiber.update(arc(config), true).await.unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
    assert!(
        loader
            .core
            .entry_of(&fiber)
            .is_some_and(|owner| Arc::ptr_eq(&owner, &entry))
    );
    assert!(loader.core.carrier_fibers.lock().contains(&key));
    assert_eq!(entry.options.lock().disabled, None);
    let mut disposal = Box::pin(fiber.dispose());
    // Poll real disposal through UID removal and publication, stopping at its
    // drain barrier. On this runtime, neither the queued unload nor detached
    // event futures can run until this poll returns.
    std::future::poll_fn(|cx| {
        assert!(matches!(disposal.as_mut().poll(cx), Poll::Pending));
        assert!(
            loader.core.entry_of(&fiber).is_none(),
            "self-disposal retained ownership while waiting for unload"
        );
        assert!(!loader.core.carrier_fibers.lock().contains(&key));
        assert!(entry.fiber.lock().is_none());
        assert_eq!(entry.options.lock().disabled, Some(json!(true)));
        Poll::Ready(())
    })
    .await;
    disposal.await;
    assert!(entry.fiber.lock().is_none());
    assert_eq!(entry.options.lock().disabled, Some(json!(true)));
    assert!(loader.core.entries_by_fiber.lock().is_empty());
    assert!(loader.core.carrier_fibers.lock().is_empty());
    loader.tree.root_group().stop().await.unwrap();
}
