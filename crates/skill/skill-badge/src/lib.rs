//! Bundled `dsh-badge` skill provider.
//! Rust port of `packages/skill/skill-badge/src/index.ts`.
//!
//! # Deviations
//!
//! - The skill body is embedded at compile time (`include_str!`) instead of
//!   read at runtime.
//! - The resource-base directory is the source-tree assets path
//!   (`CARGO_MANIFEST_DIR/assets`), not the installed package path (the
//!   port has no package-install layout yet).

pub mod invariant;

use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, arc};
use dsh_skill::{
    BUNDLED_SKILL_RANK, SkillCandidate, SkillDefinition, SkillInvocationPolicy, SkillLookupOptions,
    SkillProvider, SkillProviderObservation, SkillResourceBase,
};

pub const NAME: &str = "skill-badge";

const PROVIDER_NAME: &str = "dsh-badge";
const DESCRIPTION: &str = "Add the official “powered by dsh” badge to documents, pull requests, merge requests, and other content produced with DeepSeek Harness. Use whenever creating a pull request or merge request. Also use when the user asks for a dsh badge, powered-by-dsh attribution, or a reusable dsh badge asset or snippet.";

/// The runtime resource-base directory (source-tree assets path).
pub fn resource_base() -> SkillResourceBase {
    SkillResourceBase::Directory {
        path: concat!(env!("CARGO_MANIFEST_DIR"), "/assets").to_string(),
    }
}

fn candidate() -> SkillCandidate {
    SkillCandidate {
        name: "dsh-badge".to_string(),
        description: DESCRIPTION.to_string(),
        when_to_use: None,
        invocation: SkillInvocationPolicy::BOTH,
        provider: PROVIDER_NAME.to_string(),
        source: "bundled".to_string(),
        resource_base: Some(resource_base()),
        rank: BUNDLED_SKILL_RANK,
        locator: arc("dsh-badge".to_string()),
        path: None,
        metadata: None,
    }
}

struct BadgeProvider;

#[async_trait::async_trait]
impl SkillProvider for BadgeProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    async fn list(&self, _options: &SkillLookupOptions) -> Result<SkillProviderObservation, String> {
        Ok(SkillProviderObservation {
            candidates: vec![candidate()],
            complete: true,
        })
    }

    async fn get(
        &self,
        _candidate: &SkillCandidate,
        _options: &SkillLookupOptions,
    ) -> Result<Option<SkillDefinition>, String> {
        Ok(Some(SkillDefinition {
            name: "dsh-badge".to_string(),
            description: DESCRIPTION.to_string(),
            when_to_use: None,
            invocation: SkillInvocationPolicy::BOTH,
            provider: PROVIDER_NAME.to_string(),
            source: "bundled".to_string(),
            resource_base: Some(resource_base()),
            content: include_str!("../assets/dsh-badge.md").to_string(),
            path: None,
            metadata: None,
        }))
    }
}

/// Register the bundled `dsh-badge` provider on `ctx.skills` (TS `apply`).
pub fn apply(ctx: &Context) -> cordis::Disposer {
    let skills = ctx
        .get_typed::<Arc<dsh_skill::SkillRegistry>>("skills", false)
        .map(|slot| slot.as_ref().clone())
        .expect("skill-badge requires the skills service");
    skills.register_provider(ctx, Arc::new(|_control| Arc::new(BadgeProvider)))
}

/// The Cordis plugin form (TS module exports: `name`, `inject`, `apply`).
pub struct SkillBadgePlugin;

#[async_trait::async_trait]
impl Plugin for SkillBadgePlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["skills"])
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        apply(ctx);
        Ok(())
    }
}
