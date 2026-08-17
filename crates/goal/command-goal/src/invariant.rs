//! Package-owned invariant companion for `@deepseek-ai/dsh-command-goal`.
//! The command adapter owns no independent event stream or projection; the
//! goal domain validates mutations and command behavior is package-tested.

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-command-goal";
pub const NAME: &str = "command-goal-invariant";
pub const INJECT: [&str; 1] = ["invariants"];

pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: None,
        install: Arc::new(|_ctx: &Context, _fail: Arc<dyn Fn(&str) + Send + Sync>| {
            Box::pin(async {})
        }),
    }
}

pub fn apply(ctx: &Context) -> cordis::Disposer {
    let registry = ctx
        .get_typed::<Arc<InvariantRegistry>>("invariants", false)
        .expect("the command-goal invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

pub struct CommandGoalInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for CommandGoalInvariantPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        apply(ctx);
        Ok(())
    }
}
