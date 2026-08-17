//! Package-owned invariant companion for `@deepseek-ai/dsh-tool-goal`.
//! The model adapter owns no independent state or event protocol; accepted
//! mutations are checked by the goal domain and authority behavior is tested.

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};

pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-tool-goal";
pub const NAME: &str = "tool-goal-invariant";
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
        .expect("the tool-goal invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

pub struct ToolGoalInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for ToolGoalInvariantPlugin {
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
