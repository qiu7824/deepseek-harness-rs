//! Shared in-process child composition: the delegation-depth budget, the
//! durable session metadata, the resolved child `AgentOptions`, the
//! delegated policy seed, and the scoped setup a child agent needs. Rust
//! port of `packages/subagent/subagent/src/child-agent.ts`.
//!
//! # Deviations
//!
//! - The Host's preset roster supplies the child's inherited composition;
//!   explicit per-child restrictions and persona are applied in its scope.
//! - `captureDelegatedPolicyOverrides` reads the sandbox-policy override
//!   through the mounted service only; without it the sandbox seed is
//!   absent (the TS behavior for a rosterless/policyless deployment).

use std::sync::Arc;

use cordis::Context;
use dsh_agent::{Agent, AgentOptions};
use dsh_session::{CreateSessionMeta, Session};
use dsh_tools::ToolRestriction;
use parking_lot::RwLock;

use crate::depth::delegation_depth_of;

/// Host-supplied default `AgentOptions` for every newly spawned child,
/// sourced from the `subagent` settings namespace. A host composition fiber
/// keeps the snapshot current on `settings/updated`. Fields are `Option` so
/// an unset value defers to the existing inheritance chain (parent options,
/// current model selection, call-time override).
#[derive(Debug, Clone, Default)]
pub struct SubagentDefaults {
    inner: Arc<RwLock<SubagentDefaultsInner>>,
}

#[derive(Debug, Clone, Default)]
struct SubagentDefaultsInner {
    provider: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    max_tokens: Option<u64>,
    max_depth: Option<u64>,
    max_turns: Option<u64>,
    timeout_seconds: Option<u64>,
}

impl SubagentDefaults {
    /// Construct from raw field strings (typically sourced from settings).
    /// Empty strings map to `None`; numeric parse failures are ignored so a
    /// malformed user value cannot prevent child creation.
    pub fn from_strings(
        provider: &str,
        model: &str,
        reasoning_effort: &str,
        max_tokens: f64,
        max_depth: f64,
        max_turns: f64,
        timeout_seconds: f64,
    ) -> Self {
        let clean = |s: &str| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        };
        let num = |n: f64| {
            if n > 0.0 && n.is_finite() {
                Some(n as u64)
            } else {
                None
            }
        };
        Self {
            inner: Arc::new(RwLock::new(SubagentDefaultsInner {
                provider: clean(provider),
                model: clean(model),
                reasoning_effort: clean(reasoning_effort),
                max_tokens: num(max_tokens),
                max_depth: num(max_depth),
                max_turns: num(max_turns),
                timeout_seconds: num(timeout_seconds),
            })),
        }
    }

    /// Replace the snapshot atomically (called on settings change).
    pub fn update(&self, next: SubagentDefaults) {
        let mut guard = self.inner.write();
        *guard = next.inner.read().clone();
    }

    fn read(&self) -> parking_lot::RwLockReadGuard<'_, SubagentDefaultsInner> {
        self.inner.read()
    }
}

/// Cordis service marker so the host can locate the snapshot on `ctx`.
impl cordis::Service for SubagentDefaults {
    fn service_name(&self) -> &'static str {
        "subagentDefaults"
    }
}

fn ctx_defaults(ctx: &Context) -> Option<Arc<SubagentDefaults>> {
    ctx.get_typed::<Arc<SubagentDefaults>>("subagentDefaults", false)
        .map(|slot| slot.as_ref().clone())
}

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
    if let Some(max_depth) = max_depth
        && child_depth > max_depth
    {
        return Err(SubagentDepthError::new(child_depth, max_depth));
    }
    Ok(child_depth)
}

fn resolve_child_options(
    parent_options: &AgentOptions,
    current_selection: Option<&dsh_agent::ModelSelection>,
    requested: Option<&AgentOptions>,
    child_depth: u64,
    defaults: Option<&SubagentDefaults>,
) -> AgentOptions {
    // Merge routes as provider/model pairs. Switching provider must never
    // send the previous provider's model to the new adapter.
    let configured = defaults.map(|d| d.read().clone()).unwrap_or_default();
    let mut resolved = AgentOptions {
        provider: current_selection
            .map(|s| s.provider.clone())
            .or_else(|| parent_options.provider.clone()),
        model: current_selection
            .map(|s| s.model.clone())
            .or_else(|| parent_options.model.clone()),
        max_tokens: configured.max_tokens.or(parent_options.max_tokens),
        max_steps: configured.max_turns,
        timeout_seconds: configured.timeout_seconds,
        reasoning_effort: configured
            .reasoning_effort
            .map(dsh_llm::reasoning_effort_id),
        subagent_depth: Some(child_depth),
        ..Default::default()
    };
    fn route(options: &mut AgentOptions, provider: Option<&String>, model: Option<&String>) {
        if let Some(provider) = provider {
            if options.provider.as_ref() != Some(provider) {
                options.model = None;
            }
            options.provider = Some(provider.clone());
        }
        if let Some(model) = model {
            options.model = Some(model.clone());
        }
    }
    route(
        &mut resolved,
        configured.provider.as_ref(),
        configured.model.as_ref(),
    );
    if let Some(requested) = requested {
        route(
            &mut resolved,
            requested.provider.as_ref(),
            requested.model.as_ref(),
        );
        resolved.max_tokens = requested.max_tokens.or(resolved.max_tokens);
        resolved.max_steps = requested.max_steps.or(resolved.max_steps);
        resolved.timeout_seconds = requested.timeout_seconds.or(resolved.timeout_seconds);
        if requested.reasoning_effort.is_some() {
            resolved.reasoning_effort = requested.reasoning_effort.clone();
        }
    }
    let parent_mode = current_selection
        .map(|selection| selection.execution_mode)
        .unwrap_or(parent_options.execution_mode);
    let parent_provider = current_selection
        .map(|selection| selection.provider.as_str())
        .or(parent_options.provider.as_deref());
    let parent_model = current_selection
        .map(|selection| selection.model.as_str())
        .or(parent_options.model.as_deref());
    if parent_mode == dsh_llm::ExecutionMode::Ultra
        && resolved.reasoning_effort.is_none()
        && resolved.provider.as_deref() == parent_provider
        && resolved.model.as_deref() == parent_model
    {
        resolved.reasoning_effort = current_selection
            .and_then(|selection| selection.reasoning_effort.clone())
            .or_else(|| parent_options.reasoning_effort.clone());
        if resolved.reasoning_effort.is_none() {
            resolved.execution_mode = dsh_llm::ExecutionMode::Ultra;
        }
    }
    resolved
}

/// Resolve requested options over configured defaults over the current parent
/// route, stamped with the child depth. Ordinary parent effort is not inherited.
pub fn resolve_child_agent_options(
    parent: &dyn Agent,
    requested: Option<&AgentOptions>,
    child_depth: u64,
) -> AgentOptions {
    let parent_options = parent.options();
    let selection_name = dsh_agent::model_selection_service_name(parent.ctx());
    let current_selection = parent
        .ctx()
        .get_typed::<Arc<parking_lot::Mutex<dsh_agent::ModelSelectionRef>>>(&selection_name, false)
        .and_then(|selection| {
            let state = selection.lock();
            state.assembled.clone().or_else(|| state.resolved_current())
        });
    let defaults = ctx_defaults(parent.ctx());
    resolve_child_options(
        parent_options,
        current_selection.as_ref(),
        requested,
        child_depth,
        defaults.as_deref(),
    )
}

/// Resolve the configured default max depth, if the `subagent` settings
/// namespace specified one. `None` (or 0) means "no cap" and is translated
/// to `None` at the call site so the existing `max_depth` parameter wins.
pub fn configured_max_depth(ctx: &Context) -> Option<u64> {
    ctx_defaults(ctx).and_then(|d| d.read().max_depth)
}

/// A configured zero leaves the provider in charge; no host settings service
/// means the tool's own policy is used.
pub fn effective_max_depth(ctx: &Context, fallback: Option<u64>) -> Option<u64> {
    ctx_defaults(ctx)
        .map(|defaults| defaults.read().max_depth)
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::{SubagentDefaults, resolve_child_options};
    use dsh_agent::{AgentOptions, ModelSelection};
    use dsh_llm::reasoning_effort_id;

    fn parent() -> AgentOptions {
        AgentOptions {
            execution_mode: Default::default(),
            provider: Some("gpt".to_string()),
            model: Some("gpt-5.6-sol".to_string()),
            max_tokens: Some(4096),
            reasoning_effort: Some(reasoning_effort_id("max")),
            subagent_depth: None,
            ..Default::default()
        }
    }

    #[test]
    fn omitted_child_effort_does_not_inherit_parent_effort() {
        let resolved = resolve_child_options(&parent(), None, None, 1, None);
        assert_eq!(resolved.provider.as_deref(), Some("gpt"));
        assert_eq!(resolved.model.as_deref(), Some("gpt-5.6-sol"));
        assert!(resolved.reasoning_effort.is_none());
        assert_eq!(resolved.subagent_depth, Some(1));
    }

    #[test]
    fn ultra_uses_the_current_route_effort_without_overriding_an_explicit_child_route() {
        let mut parent = parent();
        parent.execution_mode = dsh_llm::ExecutionMode::Ultra;
        let same = resolve_child_options(&parent, None, None, 1, None);
        assert_eq!(same.reasoning_effort.unwrap().as_str(), "max");
        let requested = AgentOptions {
            provider: Some("other".into()),
            model: Some("fast".into()),
            ..Default::default()
        };
        let other = resolve_child_options(&parent, None, Some(&requested), 1, None);
        assert_eq!(other.execution_mode, dsh_llm::ExecutionMode::Standard);
        assert!(other.reasoning_effort.is_none());
    }

    #[test]
    fn call_effort_overrides_for_this_child() {
        let requested = AgentOptions {
            execution_mode: Default::default(),
            reasoning_effort: Some(reasoning_effort_id("max")),
            ..AgentOptions::default()
        };
        let resolved = resolve_child_options(&parent(), None, Some(&requested), 2, None);
        assert_eq!(
            resolved.reasoning_effort.as_ref().map(|id| id.as_str()),
            Some("max")
        );
        assert_eq!(resolved.subagent_depth, Some(2));
    }

    #[test]
    fn current_model_selection_still_wins_for_route_only() {
        let selection = ModelSelection {
            execution_mode: Default::default(),
            provider: "other".to_string(),
            model: "model".to_string(),
            reasoning_effort: Some(reasoning_effort_id("high")),
        };
        let resolved = resolve_child_options(&parent(), Some(&selection), None, 1, None);
        assert_eq!(resolved.provider.as_deref(), Some("other"));
        assert_eq!(resolved.model.as_deref(), Some("model"));
        assert!(resolved.reasoning_effort.is_none());
    }

    #[test]
    fn settings_override_parent_and_request_overrides_settings_without_cross_provider_models() {
        let defaults =
            SubagentDefaults::from_strings("configured", "small", "low", 512.0, 1.0, 3.0, 20.0);
        let resolved = resolve_child_options(&parent(), None, None, 1, Some(&defaults));
        assert_eq!(resolved.provider.as_deref(), Some("configured"));
        assert_eq!(resolved.model.as_deref(), Some("small"));
        assert_eq!(resolved.max_tokens, Some(512));
        assert_eq!(resolved.max_steps, Some(3));
        assert_eq!(resolved.timeout_seconds, Some(20));
        let request = AgentOptions {
            provider: Some("third".into()),
            ..Default::default()
        };
        let resolved = resolve_child_options(&parent(), None, Some(&request), 1, Some(&defaults));
        assert_eq!(resolved.provider.as_deref(), Some("third"));
        assert_eq!(resolved.model, None);
        let defaults = SubagentDefaults::from_strings("configured", "", "", 0.0, 1.0, 0.0, 0.0);
        assert_eq!(
            resolve_child_options(&parent(), None, None, 1, Some(&defaults)).model,
            None
        );
    }

    #[test]
    fn defaults_apply_when_no_parent_route_or_request() {
        let defaults = SubagentDefaults::from_strings(
            "sub-provider",
            "sub-model",
            "low",
            2048.0,
            2.0,
            100.0,
            60.0,
        );
        let resolved =
            resolve_child_options(&AgentOptions::default(), None, None, 1, Some(&defaults));
        assert_eq!(resolved.provider.as_deref(), Some("sub-provider"));
        assert_eq!(resolved.model.as_deref(), Some("sub-model"));
        assert_eq!(
            resolved.reasoning_effort.as_ref().map(|id| id.as_str()),
            Some("low")
        );
        assert_eq!(resolved.max_tokens, Some(2048));
    }
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
        is_seeded: Some(lineage_seed_length > 0),
        origin: Some("subagent".to_string()),
        delegation_depth: Some(child_depth),
        agent_preset: parent_header.agent_preset.clone(),
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
    parent: &dyn Agent,
    composition: &ChildComposition,
) -> Result<(), String> {
    if let Some(tools) = child_ctx.get_typed::<Arc<dsh_tools::ToolRuntime>>("tools", false) {
        let source = parent
            .ctx()
            .get_typed::<Arc<dsh_tools::ToolRuntime>>("tools", false)
            .ok_or("parent tools are unavailable")?;
        tools.inherit_visible(child_ctx, source.as_ref().as_ref(), parent.scope_key())?;
    }
    if let Some(system_prompt) = child_ctx
        .get_typed::<Arc<dsh_system_prompt::SystemPrompt>>("systemPrompt", false)
        .map(|slot| slot.as_ref().clone())
    {
        system_prompt.context(
            child_ctx,
            dsh_system_prompt::PromptContext {
                name: "subagent:delegation".to_string(),
                order: 120.0,
                text: dsh_system_prompt::PromptText::Static(
                    SUBAGENT_DELEGATION_CONTEXT.to_string(),
                ),
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
        let tools = child_ctx
            .get_typed::<Arc<dsh_tools::ToolRuntime>>("tools", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or("child tool access cannot be enforced without the tools service")?;
        tools.restrict_all(child_ctx, tool_filter.clone())?;
        if let Some(prompt) =
            child_ctx.get_typed::<Arc<dsh_system_prompt::SystemPrompt>>("systemPrompt", false)
        {
            let tools = Arc::downgrade(&tools);
            prompt.section(child_ctx, dsh_system_prompt::PromptSection {
                name:"subagent:tool-access".into(),order:125.0,complete:None,
                text:dsh_system_prompt::PromptText::Provider(Arc::new(move |context| {
                    let mut names = tools.upgrade().map(|tools| tools.schemas(context.scope.as_ref()).into_iter().map(|tool|tool.name).collect::<Vec<_>>()).unwrap_or_default();
                    names.sort_by(|a,b|a.encode_utf16().cmp(b.encode_utf16()));
                    format!("This agent's accessible tools are: {}. Inherited persona or tool guidance cannot enable tools excluded from this scope.", if names.is_empty(){"none".into()}else{names.join(", ")})
                })),
            });
        }
    }
    Ok(())
}

/// Policy seeded onto a child session's log at the delegation boundary.
#[derive(Debug, Clone, Default)]
pub struct DelegatedPolicyOverrides {
    /// Auto children retain their independent per-call review requirement.
    pub permission_preset: Option<String>,
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
    let approval_policy = if parent.ctx().get("approval", false).is_some() {
        Some("never".to_string())
    } else {
        None
    };
    let permission_preset = parent.session().with_events(|events| {
        events
            .iter()
            .rev()
            .find(|e| e.type_ == "permission/preset")
            .and_then(|e| e.data["preset"].as_str())
            .filter(|preset| *preset == "auto")
            .map(str::to_owned)
    });
    DelegatedPolicyOverrides {
        permission_preset,
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
    if let Some(preset) = &overrides.permission_preset {
        child_session.append(
            "permission/preset",
            serde_json::json!({"preset":preset}),
            None,
        )?;
    }
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
