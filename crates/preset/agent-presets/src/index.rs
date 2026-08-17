//! The AgentPresets service: registry over the deployment's agent presets.
//! Rust port of `src/index.ts`.
//!
//! Discovery is unmemoized: `list()` and `resolve()` re-read the roots on
//! every call so a preset authored while the process runs is visible
//! immediately.
//!
//! # Deviations
//!
//! - The TS `Context` proxy rebinding (`this.ctx` re-bound to the caller)
//!   collapses to explicit caller parameters on methods that need them
//!   (see the `dsh-cordis` conformance note).
//! - The harness-home derivation takes an injected environment reader so
//!   tests can pin `$DSH_HOME` without racing the shared process env
//!   (TS reads `process.env.DSH_HOME` directly).

use std::sync::Arc;
use std::time::SystemTime;

use cordis::{ArcValue, BoxFuture, Context, InjectSpec, PluginError, Service, arc, downcast_arc};
use dsh_agent::runtime_types::AgentLifecyclePayload;
use dsh_home_paths::dsh_home_path;
use dsh_schemastery::{Data, Schema};
use dsh_scope::{Scope, ScopeKey, ScopeParentBinding, bind_scope_parent, create_scope, scope_of};
use dsh_session::SessionEvent;
use dsh_settings::{SettingsPathOp, SettingsProvider, SettingsScope, settings_namespace};
use futures::FutureExt;
use futures::future::Shared;
use indexmap::IndexMap;
use parking_lot::Mutex;

use crate::authoring::{copy_composition, delete_composition, read_composition};
use crate::discovery::{USER_PRESET_DIR, discover_presets};
use crate::mount::{mount_preset, service_for_agent, standing_mount_for};
use crate::preset::{AgentPreset, Config, PresetMountError, PresetRoot, UnknownPresetError};
use crate::session::AGENT_PRESET_SELECTED;

/// Settings namespace carrying the user's chosen default preset.
pub const SETTINGS_NAMESPACE: &str = "agent-presets";

/// The user-writable slice of this plugin's config.
#[derive(Debug, Clone, Default)]
pub struct AgentPresetSettings {
    /// Preset mounted when a session names none.
    pub default: Option<String>,
}

/// The composition file identity one standing generation was mounted from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CompositionStamp {
    /// Modification time as `stat` reports it.
    mtime: SystemTime,
    /// File size in bytes, the tiebreak for edits within one mtime tick.
    size: u64,
}

/// Read one composition file's stamp, or `None` when it cannot be statted.
async fn composition_stamp(path: &str) -> Option<CompositionStamp> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    let mtime = metadata.modified().ok()?;
    Some(CompositionStamp {
        mtime,
        size: metadata.len(),
    })
}

/// Whether two stamps name the same file state.
fn same_stamp(a: &CompositionStamp, b: &CompositionStamp) -> bool {
    a == b
}

/// One preset's standing composition.
struct StandingMount {
    /// Scope key agents are parented to; also the mount's registration scope.
    key: ScopeKey,
    /// Disposal boundary; held for whole-tree teardown, never per-session
    /// (retained for the process lifetime, matching the TS scope handle).
    #[allow(dead_code)]
    scope: Scope,
    /// Stamp of the composition file this generation was mounted from.
    stamp: CompositionStamp,
}

type StandingFuture = Shared<BoxFuture<'static, Result<Arc<StandingMount>, PresetMountError>>>;

/// Registry over the deployment's agent presets.
pub struct AgentPresets {
    /// The service's own untraced context (TS `selfCtx`).
    ctx: Context,
    pub config: Config,
    /// The roots discovery and authoring actually scan.
    resolved_roots: Vec<PresetRoot>,
    /// The user layer over `config.default`, present only while a settings
    /// provider is composed.
    settings: Mutex<Option<SettingsScope>>,
    /// The settings service behind `settings`, held for the one write this
    /// service makes: clearing a user default it has just deleted.
    settings_service: Mutex<Option<Arc<SettingsProvider>>>,
    /// The settings-inject child fiber; settle it to await the settings
    /// section registration (TS `ctx.inject(['settings'], ...)` resolved
    /// during plugin mount).
    settings_inject: Mutex<Option<Arc<cordis::FiberCore>>>,
    /// Standing mounts by preset id, single-flight (TS `standing`).
    standing: Mutex<IndexMap<String, StandingFuture>>,
    /// Parent bindings of the agents this roster composed, keyed by the
    /// agent's scope key (TS `WeakMap`; Rust keys are identity-hashed and
    /// retained for the process lifetime — see the `dsh-scope` note).
    bindings: Mutex<IndexMap<ScopeKey, ScopeParentBinding>>,
}

impl Service for AgentPresets {
    fn service_name(&self) -> &'static str {
        "agentPresets"
    }
}

/// Runtime schema check for [`Config`] (TS `AgentPresets.Config`).
fn validate_config(config: &Config) -> Result<(), String> {
    if config.default.trim().is_empty() {
        return Err("agent-presets: config.default is required".to_string());
    }
    for root in &config.roots {
        if root.path.trim().is_empty() {
            return Err("agent-presets: config.roots[].path is required".to_string());
        }
    }
    Ok(())
}

/// The runtime schema for the user-writable slice
/// (TS `AgentPresetSettingsSchema`).
fn settings_schema() -> Schema {
    let mut properties = IndexMap::new();
    properties.insert("default".to_string(), Schema::string().required(false));
    Schema::object(properties)
}

impl AgentPresets {
    /// Create the service, register it as `ctx.agentPresets`, wire the
    /// settings section and the lifecycle listeners (TS constructor).
    ///
    /// `env` resolves environment variables for the harness-home derivation
    /// (production callers pass the real process environment).
    pub fn install(
        ctx: &Context,
        config: Config,
        env: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
    ) -> Result<Arc<Self>, String> {
        validate_config(&config)?;
        let mut resolved_roots = config.roots.clone();
        if config.include_user_root {
            resolved_roots.push(PresetRoot {
                path: dsh_home_path(None, env.as_ref(), &[USER_PRESET_DIR])
                    .to_string_lossy()
                    .to_string(),
                trust: crate::preset::PresetTrust::User,
            });
        }
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            config,
            resolved_roots,
            settings: Mutex::new(None),
            settings_service: Mutex::new(None),
            settings_inject: Mutex::new(None),
            standing: Mutex::new(IndexMap::new()),
            bindings: Mutex::new(IndexMap::new()),
        });
        ctx.register_service(service.clone());

        // Settings section: nothing here is derived, so the plain register +
        // effect teardown is enough (TS deliberately avoids
        // `installSettingsSection`).
        {
            let service_for_settings = service.clone();
            let inject_fiber = ctx.inject(
                InjectSpec::new(["settings"]),
                Arc::new(move |settings_ctx, _value| {
                    let settings_ctx = settings_ctx.clone();
                    let service_for_settings = service_for_settings.clone();
                    Box::pin(async move {
                        let Some(provider) = settings_ctx
                            .get_typed::<Arc<SettingsProvider>>("settings", true)
                            .map(|double_arc| (*double_arc).clone())
                        else {
                            return Err(PluginError::new(arc(
                                "settings service is not available".to_string()
                            )));
                        };
                        let namespace = settings_namespace(SETTINGS_NAMESPACE)
                            .map_err(|error| PluginError::new(arc(error)))?;
                        let mut base_map = indexmap::IndexMap::new();
                        base_map.insert(
                            "default".to_string(),
                            Data::String(service_for_settings.config.default.clone()),
                        );
                        let scope = provider
                            .register(
                                &settings_ctx,
                                namespace.clone(),
                                settings_schema(),
                                dsh_settings::SettingsRegisterOptions {
                                    base: Some(Data::Object(base_map)),
                                    ..Default::default()
                                },
                            )
                            .map_err(|error| PluginError::new(arc(error)))?;
                        *service_for_settings.settings.lock() = Some(scope);
                        *service_for_settings.settings_service.lock() = Some(provider.clone());
                        // Clear on teardown (TS settingsCtx.effect).
                        settings_ctx.effect(
                            "agentPresets.settings()",
                            Box::pin(async move {
                                Some(cordis::make_disposer(move || {
                                    let service_for_settings = service_for_settings.clone();
                                    Box::pin(async move {
                                        *service_for_settings.settings.lock() = None;
                                        *service_for_settings.settings_service.lock() = None;
                                    })
                                }))
                            }),
                        );
                        Ok(())
                    })
                }),
            );
            *service.settings_inject.lock() = Some(inject_fiber);
        }

        // Advisory, not fatal: a synchronous `agent/created` listener that
        // throws VETOES publication, and this service must not.
        {
            let service_for_listener = service.clone();
            futures::executor::block_on(ctx.on(
                "agent/created",
                Arc::new(move |listener_ctx: &Context, args: Vec<ArcValue>| {
                    let listener_ctx = listener_ctx.clone();
                    let service_for_listener = service_for_listener.clone();
                    Box::pin(async move {
                        let Some(payload) = args
                            .first()
                            .and_then(|value| cordis::downcast::<AgentLifecyclePayload>(value))
                        else {
                            return None;
                        };
                        if service_for_listener.resolved_roots.is_empty() {
                            return None;
                        }
                        if service_for_listener
                            .composed_preset(payload.agent.ctx())
                            .is_some()
                        {
                            return None;
                        }
                        warn(
                            &listener_ctx,
                            &format!(
                                "agent \"{}\" was published without joining an agent preset; \
                                 its tools, prompt sections, and skill catalog resolve against \
                                 the empty global layer (join through AgentPresets.mount() or \
                                 composeFrom() in the agent factory setup)",
                                payload.agent.id()
                            ),
                        )
                        .await;
                        None
                    })
                }),
                cordis::EventOptions::default(),
            ));
        }

        // The durable record is the commit point. Its public notification
        // carries only the stable identity needed by clients, never the live
        // Session.
        {
            let service_for_listener = service.clone();
            futures::executor::block_on(ctx.on(
                "session/event",
                Arc::new(move |_listener_ctx: &Context, args: Vec<ArcValue>| {
                    let service_for_listener = service_for_listener.clone();
                    Box::pin(async move {
                        let (Some(session), Some(event)) = (
                            args.first()
                                .and_then(|value| cordis::downcast::<dsh_session::Session>(value)),
                            args.get(1)
                                .and_then(|value| cordis::downcast::<SessionEvent>(value)),
                        ) else {
                            return None;
                        };
                        if event.type_ != AGENT_PRESET_SELECTED {
                            return None;
                        }
                        let Some(agent_preset) = event
                            .data
                            .get("agentPreset")
                            .and_then(|value| value.as_str())
                        else {
                            return None;
                        };
                        service_for_listener.ctx.emit(
                            "agent-preset/selected",
                            vec![arc(session.id().clone()), arc(agent_preset.to_string())],
                        );
                        None
                    })
                }),
                cordis::EventOptions::default(),
            ));
        }

        Ok(service)
    }

    /// Settle the settings-section wiring (TS: resolved during plugin
    /// mount, which is awaited by the caller's `ctx.plugin`).
    pub async fn ready(&self) -> Result<(), String> {
        if let Some(fiber) = self.settings_inject.lock().clone() {
            fiber.settle().await.map_err(|error| error.message())?;
        }
        if let Some(provider) = self.settings_service.lock().clone() {
            provider.ready().await?;
        }
        Ok(())
    }

    /// The preset id mounted when a caller names none. Read per call rather
    /// than cached: the settings document is hot-reloaded.
    pub fn default_id(&self) -> String {
        self.settings
            .lock()
            .as_ref()
            .map(|scope| (scope.get)())
            .and_then(|data| data.to_json())
            .and_then(|json| {
                json.get("default")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| self.config.default.clone())
    }

    /// The roots this roster scans (TS `roots`).
    pub fn roots(&self) -> Vec<PresetRoot> {
        self.resolved_roots.clone()
    }

    /// Whether this deployment has a root locally authored presets go to.
    pub fn authorable(&self) -> bool {
        self.resolved_roots
            .iter()
            .any(|root| root.trust == crate::preset::PresetTrust::User)
    }

    /// Every preset the configured roots currently supply
    /// (TS `list`), first-root-wins per id.
    pub async fn list(&self) -> Result<Vec<AgentPreset>, String> {
        discover_presets(&self.resolved_roots).await
    }

    /// Resolve one preset by id (TS `resolve`). A broken preset resolves —
    /// the mounting paths refuse it after resolution.
    pub async fn resolve(&self, id: Option<&str>) -> Result<AgentPreset, UnknownPresetError> {
        let wanted = id.unwrap_or(&self.default_id()).to_string();
        let presets = self.list().await.map_err(|error| {
            tracing::warn!("agent-presets: {error}");
            UnknownPresetError::new(&wanted, &[])
        })?;
        let available: Vec<String> = presets.iter().map(|preset| preset.id.clone()).collect();
        presets
            .into_iter()
            .find(|preset| preset.id == wanted)
            .ok_or_else(|| UnknownPresetError::new(&wanted, &available))
    }

    /// Resolve one preset that is about to compose an agent, refusing a
    /// broken one with its discovery-reported reason (TS `resolveMountable`).
    async fn resolve_mountable(&self, id: Option<&str>) -> Result<AgentPreset, PresetMountError> {
        let preset = self
            .resolve(id)
            .await
            .map_err(|error| PresetMountError::new(&error.preset_id, error.to_string()))?;
        if let Some(broken) = &preset.broken {
            return Err(PresetMountError::new(&preset.id, broken.clone()));
        }
        Ok(preset)
    }

    /// Compose one agent from a preset: ensure the preset's standing mount,
    /// then parent the agent's scope key to it (TS `mount`).
    pub async fn mount(
        &self,
        agent_ctx: &Context,
        id: Option<&str>,
    ) -> Result<AgentPreset, PresetMountError> {
        let agent_key = scope_of(agent_ctx).ok_or_else(|| {
            PresetMountError::new(
                id.unwrap_or("?"),
                "agent-presets: refusing to compose an unscoped context; the scope key is what joins an agent to its preset",
            )
        })?;
        let preset = self.resolve_mountable(id).await?;
        let standing = self.ensure_standing(&preset).await?;
        self.bindings.lock().insert(
            agent_key.clone(),
            bind_scope_parent(&agent_key, &standing.key),
        );
        Ok(preset)
    }

    /// Join one agent to the SAME standing composition another already runs
    /// on (TS `composeFrom`). A parent that joined no preset yields no join
    /// and no error.
    pub fn compose_from(&self, agent_ctx: &Context, parent_ctx: &Context) -> Option<String> {
        let agent_key = scope_of(agent_ctx)?;
        let standing = standing_mount_for(parent_ctx)?;
        self.bindings.lock().insert(
            agent_key.clone(),
            bind_scope_parent(&agent_key, &standing.key),
        );
        Some(standing.preset_id)
    }

    /// The preset one live agent runs on (TS `composedPreset`).
    pub fn composed_preset(&self, agent_ctx: &Context) -> Option<String> {
        standing_mount_for(agent_ctx).map(|mount| mount.preset_id)
    }

    /// Read one preset's composition text (TS `read`).
    pub async fn read(&self, id: &str) -> Result<String, String> {
        let preset = self
            .resolve(Some(id))
            .await
            .map_err(|error| error.to_string())?;
        read_composition(&preset).await
    }

    /// Create a locally authored preset by copying an existing one whole
    /// (TS `copy`). Copy is the only authoring write.
    pub async fn copy(&self, from: &str, id: &str, name: Option<&str>) -> Result<(), String> {
        let source = self
            .resolve(Some(from))
            .await
            .map_err(|error| error.to_string())?;
        // The roster check refuses ids any root supplies — shipped ones
        // included, since a user directory named like a shipped preset is
        // shadowed by it.
        let roster = self.list().await?;
        if roster.iter().any(|preset| preset.id == id) {
            return Err(crate::authoring::PresetExistsError {
                preset_id: id.to_string(),
            }
            .to_string());
        }
        copy_composition(&self.resolved_roots, &source, id, name)
            .await
            .map_err(|error| error.to_string())?;
        // A settled mount under this id can only be stale; the new preset
        // must not inherit it.
        self.standing.lock().shift_remove(id);
        Ok(())
    }

    /// Delete a locally authored preset (TS `remove`).
    pub async fn remove(&self, id: &str) -> Result<(), String> {
        let preset = self
            .resolve(Some(id))
            .await
            .map_err(|error| error.to_string())?;
        delete_composition(&self.resolved_roots, &preset)
            .await
            .map_err(|error| error.to_string())?;
        // Sessions on the deleted preset keep their standing mount; only new
        // sessions see the roster without it.
        self.standing.lock().shift_remove(id);
        // Clearing a default this call just deleted exposes the deployment's
        // own default underneath, which is the layering.
        let user_default = self
            .settings
            .lock()
            .as_ref()
            .map(|scope| (scope.get)())
            .and_then(|data| data.to_json())
            .and_then(|json| {
                json.get("default")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            });
        if user_default.as_deref() != Some(id) {
            return Ok(());
        }
        let Some(service) = self.settings_service.lock().clone() else {
            return Ok(());
        };
        let namespace =
            settings_namespace(SETTINGS_NAMESPACE).map_err(|error| error.to_string())?;
        service
            .mutate(
                &namespace,
                vec![SettingsPathOp::Unset {
                    path: vec!["default".to_string()],
                }],
                None,
            )
            .await
            .map_err(|error| error.to_string())
    }

    /// One agent's instance of a service its preset mounted
    /// (TS `serviceFor`). Read addressing only.
    pub fn service_for<T: Send + Sync + 'static>(
        &self,
        agent_ctx: &Context,
        name: &str,
    ) -> Option<Arc<T>> {
        service_for_agent(&self.ctx, agent_ctx, name)
    }

    /// Re-link one agent to a different preset's standing composition
    /// (TS `recompose`). Only valid while the agent has produced nothing —
    /// the CALLER owns that check.
    pub async fn recompose(
        &self,
        agent_ctx: &Context,
        id: &str,
    ) -> Result<AgentPreset, PresetMountError> {
        let agent_key = scope_of(agent_ctx).ok_or_else(|| {
            PresetMountError::new(
                id,
                "agent-presets: refusing to recompose an unscoped context",
            )
        })?;
        let preset = self.resolve_mountable(Some(id)).await?;
        let standing = self.ensure_standing(&preset).await?;
        // Snapshot outside the lock: the parking_lot guard from `get` would
        // otherwise live across the whole `match` expression and the `None`
        // arm's `insert` would re-lock the same mutex on this thread.
        let existing = self.bindings.lock().get(&agent_key).cloned();
        match existing {
            Some(binding) => binding.rebind(&standing.key),
            None => {
                self.bindings.lock().insert(
                    agent_key.clone(),
                    bind_scope_parent(&agent_key, &standing.key),
                );
            }
        }
        Ok(preset)
    }

    /// The standing scope key of one preset, for a host reader with no agent
    /// (TS `standingKeyFor`).
    pub async fn standing_key_for(&self, id: Option<&str>) -> Result<ScopeKey, PresetMountError> {
        let preset = self.resolve_mountable(id).await?;
        Ok(self.ensure_standing(&preset).await?.key.clone())
    }

    /// Resolve (or create, single-flight) the standing mount of one preset.
    async fn ensure_standing(
        &self,
        preset: &AgentPreset,
    ) -> Result<Arc<StandingMount>, PresetMountError> {
        loop {
            let pending = self.standing.lock().get(&preset.id).cloned();
            if let Some(pending) = pending {
                let mounted = pending.await?;
                // Files are the only composition editor (authoring is
                // copy/delete), so the stamp is what notices an edit: a
                // changed file starts the next generation. An unreadable
                // stamp serves the current generation.
                let current = composition_stamp(&preset.path).await;
                if current
                    .as_ref()
                    .is_none_or(|stamp| same_stamp(&mounted.stamp, stamp))
                {
                    return Ok(mounted);
                }
                // Guarded delete: a caller that raced this one may have
                // already started the next generation, and dropping THAT
                // pointer would fork a third.
                if self.standing.lock().get(&preset.id).is_some() {
                    self.standing.lock().shift_remove(&preset.id);
                }
                continue;
            }
            // Stamped before the file is read: an edit racing the mount makes
            // the stamp stale rather than silently current.
            let stamp = composition_stamp(&preset.path).await.ok_or_else(|| {
                PresetMountError::new(
                    &preset.id,
                    format!("composition file is unreadable: {}", preset.path),
                )
            })?;
            let key = ScopeKey::new();
            let scope = create_scope(
                &self.ctx,
                key.clone(),
                &dsh_scope::CreateScopeOptions::default(),
            );
            let preset_for_mount = preset.clone();
            let created: Shared<BoxFuture<'static, Result<Arc<StandingMount>, PresetMountError>>> =
                async move {
                    if let Err(error) = mount_preset(&scope.ctx, &preset_for_mount).await {
                        // A settled failure is removed so a later session
                        // retries a preset whose file has been fixed.
                        // (The standing-map deletion happens in the caller
                        // loop below; the scope is disposed here.)
                        let _ = (scope.dispose)().await;
                        return Err(error);
                    }
                    Ok(Arc::new(StandingMount { key, scope, stamp }))
                }
                .boxed()
                .shared();
            self.standing
                .lock()
                .insert(preset.id.clone(), created.clone());
            let mounted = created.await?;
            return Ok(mounted);
        }
    }
}

/// Report through the cordis logger (TS `ctx.logger.warn`).
async fn warn(ctx: &Context, message: &str) {
    if let Some(logger) = ctx.get_typed::<Arc<cordis::logger::LoggerService>>("logger", false) {
        logger.warn(ctx, vec![arc(message.to_string())]);
    } else {
        tracing::warn!("{message}");
    }
}

/// Production environment reader (real `$DSH_HOME`).
pub fn process_env() -> Arc<dyn Fn(&str) -> Option<String> + Send + Sync> {
    Arc::new(|name: &str| std::env::var(name).ok())
}
