//! Shared in-process child composition: the delegation-depth budget, the
//! durable session metadata, the resolved child `AgentOptions`, the
//! delegated policy seed, and the scoped setup a child agent needs. Rust
//! port of `packages/subagent/subagent/src/child-agent.ts`.
//!
//! # Deviations
//!
//! - `agentPresets` is not ported: `childSessionMeta` records no preset and
//!   `applyChildComposition` cannot join the parent's preset rows.
//! - `captureDelegatedPolicyOverrides` reads the sandbox-policy override
//!   through the mounted service only; without it the sandbox seed is
//!   absent (the TS behavior for a rosterless/policyless deployment).

use std::sync::Arc;

use cordis::Context;
use dsh_agent::{Agent, AgentOptions};
use dsh_session::{CreateSessionMeta, Session};
use dsh_tools::ToolRestriction;

use crate::depth::delegation_depth_of;

/// Thrown when starting a child would exceed the requested depth cap.
#[derive(Debug, Clone)]
pub struct SubagentDepthError {
    pub attempted_depth: u64,
    pub max_depth: u64,
    pub message: String,
}

impl SubagentDepthError {
    pub fn new(attempted_depth: u64, max_depth: u64) -> Self {
        Self {
            attempted_depth,
            max_depth,
            message: format!("subagent depth {attempted_depth} exceeds maxDepth {max_depth}"),
        }
    }
}

impl std::fmt::Display for SubagentDepthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SubagentDepthError {}

/// Resolve the child's delegation depth from its parent and enforce an
/// optional cap.
pub fn resolve_child_depth(
    parent: &dyn Agent,
    max_depth: Option<u64>,
) -> Result<u64, SubagentDepthError> {
    let child_depth = delegation_depth_of(parent)
        .map_err(|_| SubagentDepthError::new(u64::MAX, max_depth.unwrap_or(u64::MAX)))?
        .saturating_add(1);
    if child_depth > 9_007_199_254_740_991 {
        return Err(SubagentDepthError::new(
            child_depth,
            max_depth.unwrap_or(u64::MAX),
        ));
    }
    if let Some(max_depth) = max_depth {
        if child_depth > max_depth {
            return Err(SubagentDepthError::new(child_depth, max_depth));
        }
    }
    Ok(child_depth)
}

/// Resolve the child's `AgentOptions`: the parent's provider/model/maxTokens
/// route unless the request overrides it, stamped with the child's own
/// delegation depth.
pub fn resolve_child_agent_options(
    parent: &dyn Agent,
    requested: Option<&AgentOptions>,
    child_depth: u64,
) -> AgentOptions {
    let parent_options = parent.options();
    let mut resolved = AgentOptions {
        provider: parent_options.provider.clone(),
        model: parent_options.model.clone(),
        max_tokens: parent_options.max_tokens,
        subagent_depth: Some(child_depth),
    };
    if let Some(requested) = requested {
        if requested.provider.is_some() {
            resolved.provider = requested.provider.clone();
        }
        if requested.model.is_some() {
            resolved.model = requested.model.clone();
        }
        if requested.max_tokens.is_some() {
            resolved.max_tokens = requested.max_tokens;
        }
    }
    resolved
}

/// Build the child session's durable creation metadata.
pub fn child_session_meta(
    parent: &dyn Agent,
    child_depth: u64,
    lineage_seed_length: u64,
) -> CreateSessionMeta {
    let parent_header = parent.session().header();
    CreateSessionMeta {
        cwd: parent_header.cwd.clone(),
        parent_session: Some(parent_header.id.clone()),
        created_at: None,
        seed_length: (lineage_seed_length > 0).then_some(lineage_seed_length),
        origin: Some("subagent".to_string()),
        delegation_depth: Some(child_depth),
        agent_preset: None,
    }
}

/// The scoped composition a child agent's creation window applies.
#[derive(Debug, Clone, Default)]
pub struct ChildComposition {
    /// Per-child persona shadowing the deployment persona.
    pub persona: Option<String>,
    /// Per-child tool scoping.
    pub tool_filter: Option<ToolRestriction>,
}

/// Model-facing delegation-scope statement for every in-process child.
pub const SUBAGENT_DELEGATION_CONTEXT: &str = "You are a delegated subagent: your permission scope was fixed when you were started and cannot be widened from inside this session — operations that require approval are rejected automatically. When the task needs access beyond that scope, do not retry the denied operation; state the limitation in your reply so the delegating agent can handle it.";

/// Compose one child inside its creation window: register the fixed
/// delegation-scope statement, then apply the child's own shadowing persona
/// section and tool restriction.
pub fn apply_child_composition(
    child_ctx: &Context,
    _parent: &dyn Agent,
    composition: &ChildComposition,
) {
    if let Some(system_prompt) = child_ctx
        .get_typed::<Arc<dsh_system_prompt::SystemPrompt>>("systemPrompt", false)
        .map(|slot| slot.as_ref().clone())
    {
        system_prompt.context(
            child_ctx,
            dsh_system_prompt::PromptContext {
                name: "subagent:delegation".to_string(),
                order: 120.0,
                text: dsh_system_prompt::PromptText::Static(SUBAGENT_DELEGATION_CONTEXT.to_string()),
            },
        );
        if let Some(persona) = &composition.persona {
            system_prompt.section(
                child_ctx,
                dsh_system_prompt::PromptSection {
                    name: "deployment:persona".to_string(),
                    order: 0.0,
                    text: dsh_system_prompt::PromptText::Static(persona.clone()),
                    complete: None,
                },
            );
        }
    }
    if let Some(tool_filter) = &composition.tool_filter {
        if let Some(tools) = child_ctx
            .get_typed::<Arc<dsh_tools::ToolRuntime>>("tools", false)
            .map(|slot| slot.as_ref().clone())
        {
            let _ = tools.restrict(child_ctx, tool_filter.clone());
        }
    }
}

/// Policy seeded onto a child session's log at the delegation boundary.
#[derive(Debug, Clone, Default)]
pub struct DelegatedPolicyOverrides {
    /// The parent session's explicit sandbox-mode override, or `None`
    /// without one.
    pub sandbox_mode: Option<dsh_sandbox::SandboxMode>,
    /// `'never'` whenever the approval capability is composed, `None`
    /// otherwise.
    pub approval_policy: Option<String>,
}

/// Capture the policy to seed into one delegation.
pub fn capture_delegated_policy_overrides(parent: &dyn Agent) -> DelegatedPolicyOverrides {
    let sandbox_mode = parent
        .ctx()
        .get_typed::<Arc<dsh_sandbox_policy::SandboxPolicyService>>("sandboxPolicy", false)
        .map(|slot| slot.as_ref().clone())
        .and_then(|policy| policy.override_of(parent.session()));
    let approval_policy = if parent
        .ctx()
        .get("approval", false)
        .is_some()
    {
        Some("never".to_string())
    } else {
        None
    };
    DelegatedPolicyOverrides {
        sandbox_mode,
        approval_policy,
    }
}

/// Append the captured delegation policy onto the child's own log as
/// `source: 'delegation'` events inside the unpublished creation window.
pub fn append_delegated_policy_overrides(
    child_session: &Session,
    overrides: &DelegatedPolicyOverrides,
) -> Result<(), String> {
    if let Some(mode) = &overrides.sandbox_mode {
        child_session.append(
            "sandbox/mode",
            serde_json::json!({ "mode": mode.as_str(), "source": "delegation" }),
            None,
        )?;
    }
    if let Some(policy) = &overrides.approval_policy {
        child_session.append(
            "approval/policy",
            serde_json::json!({ "policy": policy, "source": "delegation" }),
            None,
        )?;
    }
    Ok(())
}
