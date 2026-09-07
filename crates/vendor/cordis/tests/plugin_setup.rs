use cordis::{ArcValue, Context, FiberState, Plugin, PluginError, arc};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

struct RequiresOwner {
    owner: Arc<AtomicBool>,
    applied: Arc<AtomicBool>,
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
