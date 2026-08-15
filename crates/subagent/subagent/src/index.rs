//! Service Definition for the subagent capability seam (`ctx.subagents`): a
//! named-provider registry plus a capability-validating asynchronous start
//! API. Rust port of `packages/subagent/subagent/src/index.ts` (service
//! core).
//!
//! # Deviations
//!
//! - The continuation manager, activation setup registry, child/descendant
//!   listing, and session projections are not ported yet: continuable
//!   operations reject with `CONTINUATION_UNAVAILABLE` and listings with
//!   `UNSUPPORTED_CAPABILITY`.
//! - Provider registration is effect-scoped via the caller context; the
//!   registry service itself is a plain installable struct.
//! - `assertObjectJsonSchema` validation on `outputSchema` is enforced by
//!   the tools crate only when a schema runtime is mounted; the service
//!   checks the object root shape.

use std::collections::HashMap;
use std::sync::Arc;

use cordis::{ArcValue, Context, Disposer, InjectSpec, Plugin, PluginError};

use crate::descriptor::{snapshot_subagent_descriptor, SubagentDescriptorData};
use crate::error::SubagentError;
use crate::lifecycle::{LifecycleEdge, emit_lifecycle_edge, observe_run};
use crate::types::{
    ContinuableCreateRequest, ContinuableCreateSpec, ResolvedSubagentStartRequest,
    SubagentProvider, SubagentRun, SubagentStartRequest,
};

/// Named provider registry with one-shot runs (TS `SubagentRuntime`).
#[derive(Clone)]
pub struct SubagentRuntime {
    pub ctx: Context,
    providers: Arc<parking_lot::Mutex<HashMap<String, Arc<dyn SubagentProvider>>>>,
}

impl SubagentRuntime {
    /// Register the `subagents` service.
    pub fn install(ctx: &Context) -> Arc<Self> {
        let runtime = Arc::new(Self {
            ctx: ctx.clone(),
            providers: Arc::new(parking_lot::Mutex::new(HashMap::new())),
        });
        ctx.register_service(runtime.clone());
        runtime
    }

    /// Register a provider under its name (TS `registerProvider`).
    pub fn register_provider(
        &self,
        caller: &Context,
        provider: Arc<dyn SubagentProvider>,
    ) -> Result<Disposer, SubagentError> {
        let name = provider.name().to_string();
        {
            let mut providers = self.providers.lock();
            if providers.contains_key(&name) {
                return Err(SubagentError::new(
                    "DUPLICATE_PROVIDER",
                    format!("a subagent provider named \"{name}\" is already registered"),
                ));
            }
            providers.insert(name.clone(), provider.clone());
        }
        self.ctx
            .emit("subagent/provider-added", vec![cordis::arc(provider.clone())]);
        let disposer: Disposer = cordis::events::make_disposer({
            let runtime = self.clone();
            let name_for_dispose = name.clone();
            move || {
                let runtime = runtime.clone();
                let name = name_for_dispose.clone();
                Box::pin(async move {
                    if runtime.providers.lock().remove(&name).is_some() {
                        emit_lifecycle_edge(
                            &runtime.ctx,
                            LifecycleEdge::ProviderRemoved(name),
                        );
                    }
                })
            }
        });
        // Effect-scope the registration on the caller fiber (HMR safe).
        let effect_disposer = disposer.clone();
        let _ = caller.effect(
            "subagents.registerProvider()",
            Box::pin(async move { Some(effect_disposer) }),
        );
        Ok(disposer)
    }

    /// Look up a provider by name.
    pub fn get_provider(&self, name: &str) -> Option<Arc<dyn SubagentProvider>> {
        self.providers.lock().get(name).cloned()
    }

    /// List registered provider names in insertion order.
    pub fn list(&self) -> Vec<String> {
        self.providers.lock().keys().cloned().collect()
    }

    /// Look up a provider for dispatch or fail loud.
    fn expect_provider(&self, name: &str) -> Result<Arc<dyn SubagentProvider>, SubagentError> {
        self.get_provider(name).ok_or_else(|| {
            SubagentError::new("NO_PROVIDER", format!("no subagent provider registered for \"{name}\""))
        })
    }

    /// Reject the first requested capability that the provider lacks.
    fn assert_capabilities(
        &self,
        provider: &dyn SubagentProvider,
        request: &SubagentStartRequest,
    ) -> Result<(), SubagentError> {
        let capabilities = provider.capabilities();
        let needs: Vec<(bool, &str)> = vec![
            (request.output_schema.is_some(), "outputSchema"),
            (request.max_depth.is_some(), "depthLimit"),
            (request.tool_filter.is_some(), "toolFilter"),
            (request.persona.is_some(), "persona"),
        ];
        for (when, cap) in needs {
            let supported = match cap {
                "outputSchema" => capabilities.output_schema,
                "depthLimit" => capabilities.depth_limit,
                "toolFilter" => capabilities.tool_filter,
                "persona" => capabilities.persona,
                _ => false,
            };
            if when && !supported {
                return Err(SubagentError::new(
                    "UNSUPPORTED_CAPABILITY",
                    format!(
                        "subagent provider \"{}\" does not support the \"{cap}\" capability",
                        provider.name()
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Establish a published child on the named provider.
    pub async fn start(
        &self,
        name: &str,
        request: SubagentStartRequest,
    ) -> Result<Arc<dyn SubagentRun>, SubagentError> {
        let provider = self.expect_provider(name)?;
        self.assert_capabilities(provider.as_ref(), &request)?;
        crate::depth::assert_subagent_max_depth(request.max_depth)
            .map_err(|message| SubagentError::new("INVALID_MAX_DEPTH", message))?;
        if let Some(schema) = &request.output_schema {
            if !schema.is_object() {
                return Err(SubagentError::new(
                    "INVALID_OUTPUT_SCHEMA",
                    "subagent outputSchema must be an object-rooted JSON Schema",
                ));
            }
        }
        let descriptor = snapshot_subagent_descriptor(&SubagentDescriptorData::OneShot {
            version: crate::descriptor::SUBAGENT_DESCRIPTOR_VERSION,
            provider: name.to_string(),
            label: request.label.clone(),
        })
        .map_err(|message| SubagentError::new("INVALID_DESCRIPTOR", message))?;
        let parent = request.parent.clone();
        let resolved = ResolvedSubagentStartRequest {
            request,
            descriptor,
        };
        let run = provider.start(resolved).await?;
        Ok(observe_run(&self.ctx, name, parent, run))
    }

    /// Resolve one provider's detached continuable-creation contribution.
    pub async fn prepare_continuable(
        &self,
        name: &str,
        request: ContinuableCreateRequest,
    ) -> Result<ContinuableCreateSpec, SubagentError> {
        let provider = self.expect_provider(name)?;
        provider.prepare_continuable(request).await
    }

    /// Continuable operations are unavailable until the continuation manager
    /// is ported.
    pub fn require_continuations(&self) -> Result<(), SubagentError> {
        Err(SubagentError::new(
            "CONTINUATION_UNAVAILABLE",
            "continuable subagents require the agents service (continuation manager not ported)",
        ))
    }

    /// Enumerate the parent's direct session-backed subagents (not ported
    /// yet; the listing ladder arrives with the session projections).
    pub async fn list_children(
        &self,
        _parent_session_id: &dsh_session::SessionId,
    ) -> Result<Vec<serde_json::Value>, SubagentError> {
        Err(SubagentError::new(
            "UNSUPPORTED_CAPABILITY",
            "subagent child listing requires the session projections (not ported)",
        ))
    }
}

impl cordis::Service for SubagentRuntime {
    fn service_name(&self) -> &'static str {
        "subagents"
    }
}

/// The Cordis plugin form of the subagent service.
pub struct SubagentPlugin;

#[async_trait::async_trait]
impl Plugin for SubagentPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("subagent")
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["agents", "sessions", "tools"])
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        SubagentRuntime::install(ctx);
        Ok(())
    }
}

// Re-exported capability anchor.
pub use crate::types::SubagentCapabilities as SubagentCapabilitiesType;
