//! User-facing permission presets over the independent sandbox-mode and
//! approval-policy knobs. A switch records the selected preset, then writes
//! changed knobs through their canonical setters. Execution, prompt
//! narration, and replay keep reading their knob folds. The preset event
//! preserves user intent when two presets share a bundle. The read side
//! ships as the `permissions` session projection; the write side ships as
//! the `/permission` command — both optional children over the same
//! service.
//! Rust port of `packages/interaction/permission-presets/src/index.ts`
//! (+ `types.ts`).
//!
//! # Deviations
//!
//! - `SandboxMode` has no `from_str` in the port; this crate carries the
//!   closed 3-way parser.
//! - The projection-unit state is plain JSON (the port's persisted-cache
//!   precondition) with typed [`KnobState`] adapters around it.
//! - The settings schema is a schemastery schema (the port has no zod);
//!   unknown stored presets are rejected by the union-of-consts schema plus
//!   an explicit validate hook with the same verbatim message.
//! - The `/permission` handler reports an unknown preset as
//!   [`dsh_commands::CommandResult::Error`] (the TS result shape) instead
//!   of the `Err` rethrow channel, which the Rust command runtime maps to a
//!   thrown execute() failure.

pub mod invariant;

use std::sync::{Arc, OnceLock};

use cordis::{
    ArcValue, Context, EventOptions, InjectSpec, Listener, Plugin, PluginError, arc, downcast,
};
use dsh_commands::{CommandDefinition, CommandInputDescriptor, CommandResult, CommandRuntime};
use dsh_sandbox::SandboxMode;
use dsh_sandbox_policy::{effective_sandbox_mode, set_sandbox_mode};
use dsh_session::{Session, SessionEvent, SessionStore};
use dsh_session_projection::{ProjectionDefinition, SessionProjectionRegistry};
use dsh_settings::{
    SettingsNamespace, SettingsSectionHooks, install_settings_section, settings_namespace,
};
use dsh_shell::ShellExecutor;
use dsh_user_approval::{
    ApprovalPolicy, ApprovalService, effective_approval_policy, set_approval_policy,
};
use indexmap::IndexMap;
use schemastery::{Data, Schema};
use serde::Serialize;

/// Returned when effective knob values match no table entry. Clients may
/// show it as the current value, but it is never a switch target or event
/// payload.
pub const CUSTOM_PRESET: &str = "custom";

/// Settings namespace carrying the default for future sessions.
pub fn permission_settings_namespace() -> &'static SettingsNamespace {
    static NAMESPACE: OnceLock<SettingsNamespace> = OnceLock::new();
    NAMESPACE.get_or_init(|| settings_namespace("permission").expect("valid permission namespace"))
}

/// Parse the closed sandbox-mode vocabulary (the port's `SandboxMode`
/// carries no `from_str`).
pub fn parse_sandbox_mode(value: &str) -> Option<SandboxMode> {
    match value {
        "read-only" => Some(SandboxMode::ReadOnly),
        "workspace-write" => Some(SandboxMode::WorkspaceWrite),
        "danger-full-access" => Some(SandboxMode::DangerFullAccess),
        _ => None,
    }
}

/// One preset's sandbox/approval bundle and optional client presentation
/// (TS `PresetSpec`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetSpec {
    /// The `sandbox/mode` value the preset writes through.
    pub sandbox: SandboxMode,
    /// The `approval/policy` value the preset writes through.
    pub approval: ApprovalPolicy,
    /// The display label a client shows for this preset; the raw table key
    /// when omitted.
    pub name: Option<String>,
    /// One user-facing sentence on what the preset means; omitted when not
    /// configured.
    pub description: Option<String>,
}

/// The select-option shape a presentation layer advertises for one preset
/// (or for the derived `custom` state) (TS `PresetOption`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct PresetOption {
    /// Stable option value: the table key, or `custom`.
    pub value: String,
    /// The display label.
    pub name: String,
    /// One user-facing sentence on what the value means; omitted when not
    /// configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Whole `permissions` projection value (TS `PermissionSelect`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct PermissionSelect {
    /// Switchable presets, plus `custom` appended exactly while it is
    /// current.
    pub options: Vec<PresetOption>,
    /// The effective current value: a preset table key, or `custom`.
    #[serde(rename = "currentValue")]
    pub current_value: String,
}

/// The projection unit's state: the last seen value of each knob event,
/// null before an override (composition defaults apply at view time). Plain
/// JSON (persisted-cache precondition) — see [`knob_state_to_json`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KnobState {
    /// Last `permission/preset` payload, or null.
    pub preset: Option<String>,
    /// Last `sandbox/mode` payload, or null.
    pub sandbox: Option<SandboxMode>,
    /// Last `approval/policy` payload, or null.
    pub approval: Option<ApprovalPolicy>,
}

/// State for the empty log: every knob at its composition default (TS
/// `EMPTY_KNOBS`).
pub const EMPTY_KNOBS: KnobState = KnobState {
    preset: None,
    sandbox: None,
    approval: None,
};

/// Fold the last selected preset from the durable log; replay needs no
/// catch-up state (TS `effectivePermissionPreset`).
pub fn effective_permission_preset(events: &[SessionEvent]) -> Option<String> {
    for event in events.iter().rev() {
        if event.type_ == "permission/preset" {
            return event
                .data
                .get("preset")
                .and_then(|value| value.as_str())
                .map(str::to_string);
        }
    }
    None
}

/// One-event knob transition (TS `applyKnobEvent`; the projection unit's
/// typed fold — the JSON adapter [`apply_knob_json`] wraps it).
pub fn apply_knob_event(state: &KnobState, event: &SessionEvent) -> Option<KnobState> {
    match event.type_.as_str() {
        "permission/preset" => Some(KnobState {
            preset: event
                .data
                .get("preset")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            ..state.clone()
        }),
        "sandbox/mode" => Some(KnobState {
            sandbox: event
                .data
                .get("mode")
                .and_then(|value| value.as_str())
                .and_then(parse_sandbox_mode),
            ..state.clone()
        }),
        "approval/policy" => Some(KnobState {
            approval: event
                .data
                .get("policy")
                .and_then(|value| value.as_str())
                .and_then(ApprovalPolicy::from_str),
            ..state.clone()
        }),
        _ => None,
    }
}

/// Whole-log knob fold (TS `foldKnobs`).
pub fn fold_knobs(events: &[SessionEvent]) -> KnobState {
    let mut state = EMPTY_KNOBS;
    for event in events {
        if let Some(next) = apply_knob_event(&state, event) {
            state = next;
        }
    }
    state
}

/// Serialize a [`KnobState`] into the projection unit's plain-JSON state.
pub fn knob_state_to_json(state: &KnobState) -> serde_json::Value {
    serde_json::json!({
        "preset": state.preset,
        "sandbox": state.sandbox.map(|mode| mode.as_str()),
        "approval": state.approval.map(|policy| policy.as_str()),
    })
}

/// Parse a projection unit's plain-JSON state back into [`KnobState`].
pub fn knob_state_from_json(value: &serde_json::Value) -> KnobState {
    KnobState {
        preset: value
            .get("preset")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        sandbox: value
            .get("sandbox")
            .and_then(|value| value.as_str())
            .and_then(parse_sandbox_mode),
        approval: value
            .get("approval")
            .and_then(|value| value.as_str())
            .and_then(ApprovalPolicy::from_str),
    }
}

/// One-event JSON-state transition; `None` for non-knob events (the caller
/// keeps the same state reference — the registry's change gate).
pub fn apply_knob_json(
    state: &serde_json::Value,
    event: &SessionEvent,
) -> Option<serde_json::Value> {
    let mut object = state.as_object()?.clone();
    match event.type_.as_str() {
        "permission/preset" => {
            let preset = event.data.get("preset")?.as_str()?;
            object.insert("preset".to_string(), serde_json::json!(preset));
        }
        "sandbox/mode" => {
            let mode = event.data.get("mode")?.as_str()?;
            object.insert("sandbox".to_string(), serde_json::json!(mode));
        }
        "approval/policy" => {
            let policy = event.data.get("policy")?.as_str()?;
            object.insert("approval".to_string(), serde_json::json!(policy));
        }
        _ => return None,
    }
    Some(serde_json::Value::Object(object))
}

/// Validate the wire shape of a `permissions` projection value (the TS zod
/// `selectSchema`: options of non-empty value/name with optional string
/// description, non-empty currentValue).
pub fn validate_permission_select(value: &serde_json::Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| "permissions projection must be an object".to_string())?;
    let options = object
        .get("options")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "permissions.options must be an array".to_string())?;
    for option in options {
        let option = option
            .as_object()
            .ok_or_else(|| "permissions option must be an object".to_string())?;
        let option_value = option
            .get("value")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "permissions option value must be a string".to_string())?;
        if option_value.is_empty() {
            return Err("permissions option value must be non-empty".to_string());
        }
        let name = option
            .get("name")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "permissions option name must be a string".to_string())?;
        if name.is_empty() {
            return Err("permissions option name must be non-empty".to_string());
        }
        if let Some(description) = option.get("description") {
            if !description.is_string() {
                return Err("permissions option description must be a string".to_string());
            }
        }
    }
    let current = object
        .get("currentValue")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "permissions.currentValue must be a string".to_string())?;
    if current.is_empty() {
        return Err("permissions.currentValue must be non-empty".to_string());
    }
    Ok(())
}

/// The [`PermissionPresetService`] config: preset table and composition
/// default (TS `Config`).
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// The preset table: name → knob bundle. Defaults to `workspace-write`
    /// (workspace-write + ask) and `danger-full-access` (danger-full-access
    /// + never). The name `custom` is reserved for the derived
    /// not-a-preset state.
    pub presets: Option<IndexMap<String, PresetSpec>>,
    /// Default for new sessions. When omitted, the preset matching the
    /// composed sandbox and approval defaults is used.
    pub default_preset: Option<String>,
}

impl Config {
    /// The schema-defaulted table (TS `static Config` preset default).
    pub fn shipped_presets() -> IndexMap<String, PresetSpec> {
        let mut presets = IndexMap::new();
        presets.insert(
            "workspace-write".to_string(),
            PresetSpec {
                sandbox: SandboxMode::WorkspaceWrite,
                approval: ApprovalPolicy::Ask,
                name: Some("workspace-write".to_string()),
                description: Some(
                    "Write inside the workspace and permitted temporary directories; wider retries require approval."
                        .to_string(),
                ),
            },
        );
        presets.insert(
            "danger-full-access".to_string(),
            PresetSpec {
                sandbox: SandboxMode::DangerFullAccess,
                approval: ApprovalPolicy::Never,
                name: Some("danger-full-access".to_string()),
                description: Some("Full file access without approval prompts.".to_string()),
            },
        );
        presets
    }
}

fn preset_default_of(data: &Data) -> Option<String> {
    let Data::Object(object) = data else {
        return None;
    };
    match object.get("defaultPreset") {
        Some(Data::String(value)) => Some(value.clone()),
        _ => None,
    }
}

/// Owns the deployment's permission presets and their write path. Requires
/// a confining `ctx.shell` executor and `ctx.approval`; unmatched knob
/// values are reported as [`CUSTOM_PRESET`], not an error.
pub struct PermissionPresetService {
    ctx: Context,
    presets: IndexMap<String, PresetSpec>,
    /// The executor's confining sandbox default (constructor-time fact).
    shell_default: SandboxMode,
    approval: Arc<ApprovalService>,
    /// The latest settings source thunk (TS `defaultSettings`).
    default_settings: Arc<parking_lot::Mutex<Arc<dyn Fn() -> Data + Send + Sync>>>,
    /// The optional-children inject fibers; `ready()` awaits their settle
    /// (a pending fiber waiting on an unmounted service settles trivially).
    wiring: parking_lot::Mutex<Option<Arc<cordis::FiberCore>>>,
    projection_fiber: parking_lot::Mutex<Option<Arc<cordis::FiberCore>>>,
    command_fiber: parking_lot::Mutex<Option<Arc<cordis::FiberCore>>>,
}

impl cordis::Service for PermissionPresetService {
    fn service_name(&self) -> &'static str {
        "permissionPresets"
    }
}

impl PermissionPresetService {
    /// Create the service, register it as `ctx.permissionPresets`, validate
    /// the composition, wire the settings section, and pin sessions (TS
    /// constructor).
    pub fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        let presets = config.presets.unwrap_or_else(Config::shipped_presets);
        if presets.contains_key(CUSTOM_PRESET) {
            return Err(format!(
                "permission: \"{CUSTOM_PRESET}\" is reserved for the derived not-a-preset state and cannot name a table entry"
            ));
        }
        let shell = ctx
            .get_typed::<Arc<dyn ShellExecutor>>("shell", false)
            .map(|slot| slot.as_ref().clone())
            .expect("permission presets require the shell service");
        let shell_default = shell.sandbox_mode().ok_or_else(|| {
            "permission: the mounted bash executor does not confine (no sandboxMode) — presets bundle a sandbox mode, so composing this plugin over an unconfined executor is a misconfiguration"
                .to_string()
        })?;
        let approval = ctx
            .get_typed::<Arc<ApprovalService>>("approval", false)
            .map(|slot| slot.as_ref().clone())
            .expect("permission presets require the approval service");

        let service = Arc::new(Self {
            ctx: ctx.clone(),
            presets,
            shell_default,
            approval,
            // Bootstrap thunk replaced by the settings section wiring; the
            // initial resolved value equals the entry.
            default_settings: Arc::new(parking_lot::Mutex::new(Arc::new(|| {
                Data::Object(IndexMap::from([(
                    "defaultPreset".to_string(),
                    Data::String("workspace-write".to_string()),
                )]))
            }))),
            wiring: parking_lot::Mutex::new(None),
            projection_fiber: parking_lot::Mutex::new(None),
            command_fiber: parking_lot::Mutex::new(None),
        });
        ctx.register_service(service.clone());

        // The schema defaulted the table; the default applies when the
        // composed defaults match a preset, else the config must say.
        let inferred_default = service.derive(&EMPTY_KNOBS);
        let default_preset = config.default_preset.clone().unwrap_or(inferred_default);
        if default_preset == CUSTOM_PRESET {
            return Err(
                "permission: composed sandbox and approval defaults match no preset; configure defaultPreset explicitly"
                    .to_string(),
            );
        }
        service.resolve(&default_preset)?;

        let mut entry_map = IndexMap::new();
        entry_map.insert(
            "defaultPreset".to_string(),
            Data::String(default_preset.clone()),
        );
        let entry = Data::Object(entry_map);

        let choices: Vec<Schema> = service
            .names()
            .iter()
            .map(|name| Schema::constant(Data::String(name.to_string())))
            .collect();
        let mut properties = IndexMap::new();
        properties.insert(
            "defaultPreset".to_string(),
            Schema::union(choices).required(true),
        );
        let settings_schema = Schema::object(properties);

        let default_settings_sink = Arc::clone(&service.default_settings);
        let presets_for_validate = service.presets.clone();
        let wiring = install_settings_section(
            ctx,
            permission_settings_namespace().clone(),
            settings_schema,
            entry,
            SettingsSectionHooks {
                set_source: Arc::new(move |source| {
                    // The source thunk reads the latest resolved snapshot at
                    // session creation; no process-level registration needs
                    // replacement on change.
                    *default_settings_sink.lock() = source;
                }),
                on_change: Arc::new(|| {}),
                validate: Some(Arc::new(move |data: &Data| {
                    let preset = preset_default_of(data).unwrap_or_default();
                    if preset.is_empty() || !presets_for_validate.contains_key(&preset) {
                        return Err(format!(
                            "permission: unknown preset \"{preset}\" (known: {})",
                            presets_for_validate
                                .keys()
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    Ok(())
                })),
            },
        );
        *service.wiring.lock() = Some(wiring);

        // Pin every missing permission fact before a session is published:
        // future sessions through session/created, existing ones right now.
        let service_for_listener = service.clone();
        let listener: Arc<Listener> = Arc::new(move |_ctx, args| {
            let service = service_for_listener.clone();
            Box::pin(async move {
                let session = args
                    .first()
                    .and_then(|value| downcast::<Session>(value))
                    .cloned();
                if let Some(session) = session {
                    service
                        .pin_initial_permission(&session)
                        .expect("pin initial permission");
                }
                None
            })
        });
        let _disposer = futures::executor::block_on(ctx.on(
            "session/created",
            listener,
            EventOptions::default(),
        ));

        if let Some(store) = ctx
            .get_typed::<Arc<SessionStore>>("sessions", false)
            .map(|slot| slot.as_ref().clone())
        {
            for session in store.list() {
                service
                    .pin_initial_permission(&session)
                    .expect("pin initial permission");
            }
        }

        // The permissions projection unit (optional child).
        let service_for_projection = service.clone();
        *service.projection_fiber.lock() = Some(ctx.inject(
            InjectSpec::new(["sessionProjections"]),
            Arc::new(
                move |projection_ctx: &Context,
                      _config: ArcValue|
                      -> cordis::BoxFuture<'static, Result<(), PluginError>> {
                    let projection_ctx = projection_ctx.clone();
                    let service = service_for_projection.clone();
                    Box::pin(async move {
                        let Some(registry) = projection_ctx
                            .get_typed::<Arc<SessionProjectionRegistry>>(
                                "sessionProjections",
                                false,
                            )
                            .map(|slot| slot.as_ref().clone())
                        else {
                            return Ok(());
                        };
                        let service_for_view = service.clone();
                        let definition = ProjectionDefinition {
                            key: "permissions".to_string(),
                            schema: Arc::new(
                                |value: &ArcValue| -> Result<serde_json::Value, String> {
                                    let json =
                                        downcast::<serde_json::Value>(value).ok_or_else(|| {
                                            "permissions projection view must be JSON".to_string()
                                        })?;
                                    validate_permission_select(json)?;
                                    Ok(json.clone())
                                },
                            ),
                            init: Arc::new(|_header| arc(knob_state_to_json(&EMPTY_KNOBS))),
                            apply: Arc::new(
                                move |state: &ArcValue, event: &SessionEvent| -> ArcValue {
                                    let current = downcast::<serde_json::Value>(state)
                                        .expect("permissions projection state must be plain JSON");
                                    let Some(next) = apply_knob_json(current, event) else {
                                        return state.clone();
                                    };
                                    arc(next)
                                },
                            ),
                            view: Arc::new(move |state: &ArcValue| -> ArcValue {
                                let json = downcast::<serde_json::Value>(state)
                                    .expect("permissions projection state must be plain JSON");
                                let select =
                                    service_for_view.select_for(&knob_state_from_json(json));
                                arc(serde_json::to_value(select).expect("select serializes"))
                            }),
                            state_version: 1,
                        };
                        registry
                            .register(&projection_ctx, definition)
                            .map_err(|error| PluginError::new(arc(error)))?;
                        Ok(())
                    })
                },
            ),
        ));

        // The /permission command (optional child).
        let service_for_command = service.clone();
        *service.command_fiber.lock() = Some(ctx.inject(
            InjectSpec::new(["commands"]),
            Arc::new(
                move |command_ctx: &Context,
                      _config: ArcValue|
                      -> cordis::BoxFuture<'static, Result<(), PluginError>> {
                    let command_ctx = command_ctx.clone();
                    let service = service_for_command.clone();
                    Box::pin(async move {
                        let Some(runtime) = command_ctx
                            .get_typed::<Arc<CommandRuntime>>("commands", false)
                            .map(|slot| slot.as_ref().clone())
                        else {
                            return Ok(());
                        };
                        let service_for_handler = service.clone();
                        let definition = CommandDefinition {
                            name: "permission".to_string(),
                            description: "切换权限预设（沙箱模式和审批策略）".to_string(),
                            input: Some(CommandInputDescriptor {
                                hint: "<preset>".to_string(),
                            }),
                            record_input: None,
                            handler: Arc::new(move |invocation| {
                                let service = service_for_handler.clone();
                                let name = invocation.raw_input.trim().to_string();
                                let session = invocation.agent.session().clone();
                                let agent = invocation.agent.clone();
                                Box::pin(async move {
                                    if name.is_empty() {
                                        let current = service.current(&session.events());
                                        return Ok(CommandResult::Success {
                                            text: Some(format!(
                                                "current preset {current} (available: {})",
                                                service.names().join(", ")
                                            )),
                                            source_event_seq: None,
                                        });
                                    }
                                    if !service.names().contains(&name.as_str()) {
                                        return Ok(CommandResult::Error {
                                            text: format!(
                                                "unknown preset \"{name}\" (available: {})",
                                                service.names().join(", ")
                                            ),
                                        });
                                    }
                                    let service_for_set = service.clone();
                                    service.apply(
                                        &session,
                                        &name,
                                        Arc::new(move |policy| {
                                            service_for_set.approval().set_policy(&agent, policy)
                                        }),
                                    )?;
                                    Ok(CommandResult::Success {
                                        text: Some(format!("preset {name}")),
                                        source_event_seq: None,
                                    })
                                })
                            }),
                        };
                        runtime
                            .register(&command_ctx, definition)
                            .map_err(|error| PluginError::new(arc(error)))?;
                        Ok(())
                    })
                },
            ),
        ));

        Ok(service)
    }

    /// Await the settings-section wiring and the optional-children inject
    /// fibers (TS plugin-load timing completes the wiring synchronously;
    /// Rust attaches them through inject fibers).
    pub async fn ready(&self) -> Result<(), String> {
        let fibers = [
            self.wiring.lock().clone(),
            self.projection_fiber.lock().clone(),
            self.command_fiber.lock().clone(),
        ];
        for fiber in fibers.into_iter().flatten() {
            fiber.settle().await.map_err(|error| error.message())?;
        }
        Ok(())
    }

    /// The composed approval service (the `/permission` command writes the
    /// live policy switch through it).
    pub fn approval(&self) -> &Arc<ApprovalService> {
        &self.approval
    }

    /// The advertised preset names, in the preset table's declaration order.
    pub fn names(&self) -> Vec<&str> {
        self.presets.keys().map(|name| name.as_str()).collect()
    }

    /// The preset currently selected as the default for future sessions.
    pub fn default_preset(&self) -> String {
        let source = self.default_settings.lock().clone();
        let data = source();
        preset_default_of(&data)
            .expect("permission settings section must resolve to { defaultPreset }")
    }

    /// Resolve the preset matching the effective knob values. A
    /// still-matching last selection wins shared-bundle ties; otherwise the
    /// first table match wins, or [`CUSTOM_PRESET`] when no entry matches.
    pub fn current(&self, events: &[SessionEvent]) -> String {
        self.derive(&fold_knobs(events))
    }

    /// Resolve the preset for one folded knob state (TS `derive`).
    fn derive(&self, state: &KnobState) -> String {
        let sandbox = state.sandbox.unwrap_or(self.shell_default);
        let approval = state
            .approval
            .unwrap_or(self.approval.config().policy.unwrap_or(ApprovalPolicy::Ask));
        let matches = |spec: &PresetSpec| spec.sandbox == sandbox && spec.approval == approval;
        if let Some(preset) = &state.preset {
            if let Some(spec) = self.presets.get(preset) {
                if matches(spec) {
                    return preset.clone();
                }
            }
        }
        for (name, spec) in &self.presets {
            if matches(spec) {
                return name.clone();
            }
        }
        CUSTOM_PRESET.to_string()
    }

    /// Build the whole select value for one folded knob state (TS
    /// `selectFor`).
    pub fn select_for(&self, state: &KnobState) -> PermissionSelect {
        let current_value = self.derive(state);
        let mut options: Vec<PresetOption> = self
            .names()
            .iter()
            .map(|name| self.option_of(name))
            .collect();
        if current_value == CUSTOM_PRESET {
            options.push(self.option_of(CUSTOM_PRESET));
        }
        PermissionSelect {
            options,
            current_value,
        }
    }

    /// Resolve a preset's knob bundle (TS `resolve`).
    pub fn resolve(&self, name: &str) -> Result<PresetSpec, String> {
        self.presets.get(name).cloned().ok_or_else(|| {
            format!(
                "permission: unknown preset \"{name}\" (known: {})",
                self.names().join(", ")
            )
        })
    }

    /// Build the client option for a table entry or [`CUSTOM_PRESET`] (TS
    /// `optionOf`; unknown names panic through `resolve`).
    pub fn option_of(&self, name: &str) -> PresetOption {
        if name == CUSTOM_PRESET {
            return PresetOption {
                value: CUSTOM_PRESET.to_string(),
                name: "Custom".to_string(),
                description: Some(
                    "Current sandbox and approval settings do not match a preset.".to_string(),
                ),
            };
        }
        let spec = self.resolve(name).unwrap_or_else(|error| panic!("{error}"));
        PresetOption {
            value: name.to_string(),
            name: spec.name.clone().unwrap_or_else(|| name.to_string()),
            description: spec.description.clone(),
        }
    }

    /// Record a changed preset, then update each changed knob through its
    /// own setter (TS `set`).
    pub fn set(&self, session: &Session, name: &str) -> Result<(), String> {
        let session_for_setter = session.clone();
        self.apply(
            session,
            name,
            Arc::new(move |policy: ApprovalPolicy| {
                set_approval_policy(&session_for_setter, policy).map(|_| ())
            }),
        )
    }

    /// Apply one preset with the caller-selected live or initialization
    /// policy writer (TS `apply`).
    pub fn apply(
        &self,
        session: &Session,
        name: &str,
        set_approval: Arc<dyn Fn(ApprovalPolicy) -> Result<(), String> + Send + Sync>,
    ) -> Result<(), String> {
        let spec = self.resolve(name)?;
        if self.current(&session.events()) != name {
            session.append(
                "permission/preset",
                serde_json::json!({ "preset": name }),
                None,
            )?;
        }
        let events = session.events();
        if spec.sandbox != effective_sandbox_mode(&events).unwrap_or(self.shell_default) {
            set_sandbox_mode(session, spec.sandbox)?;
        }
        let approval_default = self.approval.config().policy.unwrap_or(ApprovalPolicy::Ask);
        if spec.approval != effective_approval_policy(&events).unwrap_or(approval_default) {
            set_approval(spec.approval)?;
        }
        Ok(())
    }

    /// Fill every missing permission fact before a session is published
    /// (TS `pinInitialPermission`).
    pub fn pin_initial_permission(&self, session: &Session) -> Result<(), String> {
        let events = session.events();
        let selected = effective_permission_preset(&events);
        let sandbox = effective_sandbox_mode(&events);
        let approval = effective_approval_policy(&events);
        let seeded = events.iter().any(|event| event.type_ == "session/end-seed");
        if selected.is_none() && sandbox.is_none() && approval.is_none() && !seeded {
            let name = self.default_preset();
            let spec = self.resolve(&name)?;
            session.append(
                "permission/preset",
                serde_json::json!({ "preset": name }),
                None,
            )?;
            set_sandbox_mode(session, spec.sandbox)?;
            set_approval_policy(session, spec.approval)?;
            return Ok(());
        }
        let state = KnobState {
            preset: selected.clone(),
            sandbox,
            approval,
        };
        let effective = self.derive(&state);
        if selected.is_none() && effective != CUSTOM_PRESET {
            session.append(
                "permission/preset",
                serde_json::json!({ "preset": effective }),
                None,
            )?;
        }
        if sandbox.is_none() {
            set_sandbox_mode(session, self.shell_default)?;
        }
        if approval.is_none() {
            set_approval_policy(
                session,
                self.approval.config().policy.unwrap_or(ApprovalPolicy::Ask),
            )?;
        }
        Ok(())
    }

    /// The composed context (answerers and future consumers dispatch
    /// through it).
    pub fn ctx(&self) -> &Context {
        &self.ctx
    }
}

/// The Cordis plugin form (TS mounts the service class with the schema).
pub struct PermissionPresetsPlugin {
    config: Config,
}

impl PermissionPresetsPlugin {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Plugin for PermissionPresetsPlugin {
    fn name(&self) -> Option<&'static str> {
        Some("permission-presets")
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["shell", "approval", "sessions"])
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        PermissionPresetService::install(ctx, self.config.clone())
            .map(|_| ())
            .map_err(|message| PluginError::from(anyhow::anyhow!(message)))
    }
}
