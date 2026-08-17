//! Package-owned invariant companion for `@deepseek-ai/dsh-agent-presets`.
//! Rust port of `src/invariant.ts`.
//!
//! Asserts that no installed preset composition reaches the root service
//! realm, and that a deployment configuring a roster composes every agent
//! from it.
//!
//! # Deviations
//!
//! - The `system-prompt/assemble` agent check reads the `agent` field from
//!   the assembly context's extension fields; Rust agents are not yet merged
//!   into [`AssembleContext`], so the check is inert until that wiring lands
//!   (recorded deviation on the TS side's no-agent branch).

use std::sync::Arc;

use cordis::{ArcValue, Context, NextFn};
use dsh_invariants::InvariantInstaller;

use crate::index::AgentPresets;
use crate::mount::{leaked_services, live_preset_mounts};

/// Cordis companion plugin name.
pub const NAME: &str = "agent-presets-invariant";
/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];
const PACKAGE_NAME: &str = "@deepseek-ai/dsh-agent-presets";

/// Build the installer (TS `install`).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: None,
        install: Arc::new(|ctx, fail| {
            let ctx = ctx.clone();
            Box::pin(async move {
                // Clone per listener: move closures capture the parameter by
                // value, so one listener must not consume the other's copy.
                let fail_for_service = fail.clone();
                let fail_for_assemble = fail.clone();
                // Re-check every live mount whenever a service registration
                // changes: a row that publishes later — from a timer, or an
                // asynchronous continuation after its plugin returned —
                // would escape the one-shot mount audit.
                ctx.on(
                    "internal/service",
                    Arc::new(move |listener_ctx: &Context, args: Vec<ArcValue>| {
                        let listener_ctx = listener_ctx.clone();
                        let fail = fail_for_service.clone();
                        Box::pin(async move {
                            let observed = args
                                .first()
                                .and_then(|value| cordis::downcast::<String>(value))
                                .cloned()
                                .unwrap_or_default();
                            for mount in live_preset_mounts() {
                                let leaked = leaked_services(&listener_ctx, &mount.fiber);
                                if leaked.is_empty() {
                                    continue;
                                }
                                fail(&format!(
                                    "preset \"{}\" published process-global service(s) [{}] \
                                     after its mount was audited (observed while notifying \
                                     \"{}\") — a preset service must sit behind an `isolate` \
                                     realm or move to the host composition",
                                    mount.preset_id,
                                    leaked.join(", "),
                                    observed
                                ));
                            }
                            None
                        })
                    }),
                    cordis::EventOptions::default(),
                )
                .await;

                // An agent that joined no preset resolves `tools`,
                // `system-prompt`, and `skill` against the empty global
                // layer. Assembly rather than publication is the moment that
                // matters: an unjoined agent is legal until it addresses a
                // model.
                //
                // Deviation: the TS check reads `context.agent` from the
                // merge-extensible assembly context; the Rust
                // `AssembleContext.fields` carries no agent yet (dsh-agent
                // deviation, recorded in dsh-system-prompt), so this check
                // currently resolves to the TS no-agent branch and is inert
                // until that wiring lands.
                ctx.on(
                    "system-prompt/assemble",
                    Arc::new(move |listener_ctx: &Context, args: Vec<ArcValue>| {
                        let listener_ctx = listener_ctx.clone();
                        let fail = fail_for_assemble.clone();
                        let args = args.clone();
                        Box::pin(async move {
                            let presets =
                                listener_ctx.get_typed::<Arc<AgentPresets>>("agentPresets", false);
                            // Deviation: the Rust `AssembleContext.fields` is
                            // a serde_json map and carries no agent yet
                            // (dsh-agent deviation), so this resolves to the
                            // TS no-agent branch. When dsh-agent merges its
                            // agent into the assembly context, read it here
                            // and run the unjoined-agent check below.
                            let agent: Option<Arc<dyn dsh_agent::runtime_types::Agent>> = None;
                            if let (Some(presets), Some(agent)) = (presets, agent) {
                                if !presets.roots().is_empty()
                                    && presets.composed_preset(agent.ctx()).is_none()
                                {
                                    fail(&format!(
                                        "agent \"{}\" addressed a model without joining any \
                                         agent preset while a roster is composed; its tools, \
                                         prompt sections, and skill catalog resolve against \
                                         the empty global layer",
                                        agent.id()
                                    ));
                                }
                            }
                            // Continue the waterfall.
                            let next = args
                                .last()
                                .and_then(|value| cordis::downcast::<NextFn>(value));
                            match next {
                                Some(next) => Some(next.call().await),
                                None => None,
                            }
                        })
                    }),
                    cordis::EventOptions::default(),
                )
                .await;
            })
        }),
    }
}

/// Register this package's invariant companion (TS `apply`).
pub async fn apply(ctx: &Context) -> Result<cordis::Disposer, String> {
    let registry = ctx
        .get_typed::<Arc<dsh_invariants::InvariantRegistry>>("invariants", true)
        .ok_or_else(|| "invariants service is not available".to_string())?;
    Ok(registry.register(ctx, PACKAGE_NAME, installer()))
}
