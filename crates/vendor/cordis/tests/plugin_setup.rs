use cordis::{ArcValue, Context, FiberState, Plugin, PluginError, arc};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

struct RequiresOwner {
    owner: Arc<AtomicBool>,
    applied: Arc<AtomicBool>,
}

#[tokio::test]
async fn panicking_effect_setup_releases_all_disposal_waiters() {
    let ctx = Context::root();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let dispose = ctx.effect("panicking setup", Box::pin(async move {
        started_tx.send(()).unwrap();
        release_rx.await.unwrap();
        panic!("controlled effect setup failure");
    }));
    started_rx.await.unwrap();
    let first = tokio::spawn(dispose());
    tokio::task::yield_now().await;
    assert!(!first.is_finished(), "teardown must await unfinished setup");
    let second = tokio::spawn(dispose());
    release_tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        first.await.unwrap();
        second.await.unwrap();
    }).await.expect("a setup panic must not strand any cleanup waiter");
}
#[async_trait::async_trait]
impl Plugin for RequiresOwner {
    async fn apply(&self, _: &Context, _: ArcValue) -> Result<(), PluginError> {
        if !self.owner.load(Ordering::SeqCst) {
            return Err(PluginError::new(arc("owner missing".to_owned())));
        }
        self.applied.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_setup_precedes_publication_and_activation() {
    let ctx = Context::root();
    let owner = Arc::new(AtomicBool::new(false));
    let applied = Arc::new(AtomicBool::new(false));
    let fiber = ctx.plugin_with_setup(
        Arc::new(RequiresOwner {
            owner: owner.clone(),
            applied: applied.clone(),
        }),
        arc(()),
        |fiber| {
            assert_eq!(fiber.state(), FiberState::Pending);
            assert!(fiber.ctx().is_some());
            assert!(!applied.load(Ordering::SeqCst));
            owner.store(true, Ordering::SeqCst);
        },
    );
    fiber.settle().await.unwrap();
    assert!(applied.load(Ordering::SeqCst));
    fiber.dispose().await;
    assert_eq!(fiber.state(), FiberState::Disposed);
    // The original convenience API continues to activate without a setup hook.
    applied.store(false, Ordering::SeqCst);
    let ordinary = ctx.plugin(
        Arc::new(RequiresOwner {
            owner,
            applied: applied.clone(),
        }),
        arc(()),
    );
    ordinary.settle().await.unwrap();
    assert!(applied.load(Ordering::SeqCst));
    ordinary.dispose().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn fast_plugin_completion_cannot_be_overwritten_by_loading() {
    let ctx = Context::root();
    for _ in 0..64 {
        let mut fibers = Vec::new();
        for _ in 0..32 {
            let applied = Arc::new(AtomicBool::new(false));
            let fiber = ctx.plugin(
                Arc::new(RequiresOwner {
                    owner: Arc::new(AtomicBool::new(true)),
                    applied: applied.clone(),
                }),
                arc(()),
            );
            fibers.push((fiber, applied));
        }
        for (fiber, applied) in fibers {
            tokio::time::timeout(std::time::Duration::from_secs(5), fiber.settle())
                .await
                .expect("completed initialization must wake its waiters")
                .unwrap();
            assert!(applied.load(Ordering::SeqCst));
            assert_eq!(fiber.state(), FiberState::Active);
            tokio::time::timeout(std::time::Duration::from_secs(5), fiber.dispose())
                .await
                .expect("completed effect setup must allow disposal");
            assert_eq!(fiber.state(), FiberState::Disposed);
        }
    }
}
