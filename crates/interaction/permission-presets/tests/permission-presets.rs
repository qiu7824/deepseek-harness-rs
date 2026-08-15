//! Rust port of `permission-presets.spec.ts` + `invariant.spec.ts`: the
//! preset fold, the derive/table mathematics, the write-through `set`, the
//! reserved-name and composition-validation throws, the settings-backed
//! new-session default, seeded-session pinning, and the invariant
//! companion.

use std::sync::Arc;

use cordis::{Context, arc};
use dsh_invariants::{InvariantConfig, InvariantRegistry};
use dsh_permission_presets::invariant::{self, PermissionPresetsInvariantPlugin};
use dsh_permission_presets::{
    CUSTOM_PRESET, Config, PermissionPresetService, PresetSpec, effective_permission_preset,
    knob_state_from_json, knob_state_to_json, permission_settings_namespace,
};
use dsh_sandbox::SandboxMode;
use dsh_session::{CreateSessionOptions, Session, SessionEvent, SessionStore, session_id};
use dsh_settings::{SettingsNamespace, SettingsProvider, SettingsStorage};
use dsh_shell::{ShellExecutor, ShellExecRequest, ShellExecSpec, ShellProcess, ShellRunResult};
use dsh_user_approval::{ApprovalPolicy, ApprovalService, Config as ApprovalConfig};
use indexmap::IndexMap;
use schemastery::Data;

/// The TS test stand-in: confines by default, never executes.
struct FakeShell {
    sandbox: Option<SandboxMode>,
}

impl ShellExecutor for FakeShell {
    fn sandbox_mode(&self) -> Option<SandboxMode> {
        self.sandbox
    }

    fn resolve(&self, _request: ShellExecRequest) -> ShellExecSpec {
        panic!("permission tests do not execute bash")
    }

    fn run(
        &self,
        _spec: ShellExecSpec,
    ) -> cordis::BoxFuture<'static, Result<ShellRunResult, String>> {
        panic!("permission tests do not execute bash")
    }

    fn start(&self, _spec: ShellExecSpec) -> Arc<dyn ShellProcess> {
        panic!("permission tests do not execute bash")
    }
}

/// Writable in-memory settings storage (TS `MemorySettings`).
struct MemorySettings {
    doc: parking_lot::Mutex<IndexMap<String, Data>>,
}

#[async_trait::async_trait]
impl SettingsStorage for MemorySettings {
    fn writable(&self) -> bool {
        true
    }

    async fn load(&self) -> Result<IndexMap<String, Data>, String> {
        Ok(self.doc.lock().clone())
    }

    async fn persist(&self, ns: &SettingsNamespace, section: Data) -> Result<(), String> {
        self.doc.lock().insert(ns.as_str().to_string(), section);
        Ok(())
    }
}

fn mounted(
    config: Option<Config>,
    bash_default: Option<SandboxMode>,
    approval_policy: Option<ApprovalPolicy>,
) -> Result<(Context, Arc<PermissionPresetService>), String> {
    let ctx = Context::root();
    let _store = SessionStore::install(&ctx);
    let shell: Arc<dyn ShellExecutor> = Arc::new(FakeShell {
        sandbox: bash_default,
    });
    ctx.register_service(shell);
    let _approval = ApprovalService::install(
        &ctx,
        ApprovalConfig {
            policy: approval_policy,
        },
    );
    let service = PermissionPresetService::install(&ctx, config.unwrap_or_default())?;
    Ok((ctx, service))
}

async fn mounted_store(
    approval_policy: Option<ApprovalPolicy>,
) -> (
    Context,
    Arc<SessionStore>,
    Arc<PermissionPresetService>,
    Arc<SettingsProvider>,
) {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let storage = Arc::new(MemorySettings {
        doc: parking_lot::Mutex::new(IndexMap::new()),
    });
    let provider = SettingsProvider::install(&ctx, storage);
    provider.ready().await.expect("settings ready");
    let shell: Arc<dyn ShellExecutor> = Arc::new(FakeShell {
        sandbox: Some(SandboxMode::WorkspaceWrite),
    });
    ctx.register_service(shell);
    let _approval = ApprovalService::install(
        &ctx,
        ApprovalConfig {
            policy: approval_policy,
        },
    );
    let service = PermissionPresetService::install(&ctx, Config::default()).expect("install");
    service.ready().await.expect("ready");
    (ctx, store, service, provider)
}

fn fresh_session(id: &str) -> Session {
    Session::create(session_id(id), None, None).expect("session")
}

fn synthetic_event(type_: &str, data: serde_json::Value) -> SessionEvent {
    SessionEvent {
        type_: type_.to_string(),
        seq: 0,
        time: 0,
        data,
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    }
}

// ---- effectivePermissionPreset ----

#[test]
fn folds_to_the_last_event_or_none_without_one() {
    let session = fresh_session("sess-fold");
    assert_eq!(effective_permission_preset(&session.events()), None);
    session
        .append(
            "permission/preset",
            serde_json::json!({ "preset": "danger-full-access" }),
            None,
        )
        .expect("preset");
    session
        .append(
            "permission/preset",
            serde_json::json!({ "preset": "workspace-write" }),
            None,
        )
        .expect("preset");
    assert_eq!(
        effective_permission_preset(&session.events()),
        Some("workspace-write".to_string())
    );
    // The backward scan steps over non-preset events to the latest
    // selection.
    session
        .append(
            "sandbox/mode",
            serde_json::json!({ "mode": "read-only" }),
            None,
        )
        .expect("mode");
    assert_eq!(
        effective_permission_preset(&session.events()),
        Some("workspace-write".to_string())
    );
}

// ---- PermissionPresetService ----

#[tokio::test(flavor = "current_thread")]
async fn advertises_the_preset_table_in_declaration_order_and_resolves_bundles() {
    let (_ctx, service) = mounted(None, Some(SandboxMode::WorkspaceWrite), None).expect("install");
    assert_eq!(service.names(), vec!["workspace-write", "danger-full-access"]);
    let spec = service.resolve("danger-full-access").expect("resolve");
    assert_eq!(spec.sandbox, SandboxMode::DangerFullAccess);
    assert_eq!(spec.approval, ApprovalPolicy::Never);
    let error = service.resolve("plan").expect_err("unknown");
    assert!(error.contains("unknown preset \"plan\""), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn current_derives_from_the_effective_knobs() {
    let (_ctx, service) = mounted(None, Some(SandboxMode::WorkspaceWrite), None).expect("install");
    let session = fresh_session("sess-current");
    assert_eq!(service.current(&session.events()), "workspace-write");
    service
        .set(&session, "danger-full-access")
        .expect("set");
    assert_eq!(service.current(&session.events()), "danger-full-access");
}

#[tokio::test(flavor = "current_thread")]
async fn a_knob_state_matching_no_table_entry_derives_custom() {
    let (_ctx, service) = mounted(None, Some(SandboxMode::WorkspaceWrite), None).expect("install");
    let session = fresh_session("sess-custom");
    session
        .append(
            "sandbox/mode",
            serde_json::json!({ "mode": "read-only" }),
            None,
        )
        .expect("mode");
    assert_eq!(service.current(&session.events()), CUSTOM_PRESET);
    service
        .set(&session, "danger-full-access")
        .expect("set");
    assert_eq!(service.current(&session.events()), "danger-full-access");
    let error = service.resolve(CUSTOM_PRESET).expect_err("custom reserved");
    assert!(error.contains("unknown preset"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn composition_defaults_outside_the_table_derive_custom_with_an_explicit_default() {
    let config = Config {
        default_preset: Some("workspace-write".to_string()),
        presets: None,
    };
    let (_ctx, service) = mounted(
        Some(config),
        Some(SandboxMode::WorkspaceWrite),
        Some(ApprovalPolicy::Never),
    )
    .expect("install");
    let session = fresh_session("sess-defaults-custom");
    assert_eq!(service.current(&session.events()), CUSTOM_PRESET);
}

#[tokio::test(flavor = "current_thread")]
async fn the_fold_breaks_bundle_ties_and_a_stale_fold_falls_back_to_table_order() {
    let mut presets = IndexMap::new();
    presets.insert(
        "workspace-write".to_string(),
        PresetSpec {
            sandbox: SandboxMode::WorkspaceWrite,
            approval: ApprovalPolicy::Ask,
            name: None,
            description: None,
        },
    );
    presets.insert(
        "agentish".to_string(),
        PresetSpec {
            sandbox: SandboxMode::WorkspaceWrite,
            approval: ApprovalPolicy::Ask,
            name: None,
            description: None,
        },
    );
    presets.insert(
        "danger-full-access".to_string(),
        PresetSpec {
            sandbox: SandboxMode::DangerFullAccess,
            approval: ApprovalPolicy::Never,
            name: None,
            description: None,
        },
    );
    let config = Config {
        presets: Some(presets),
        default_preset: None,
    };
    let (_ctx, service) = mounted(Some(config), Some(SandboxMode::WorkspaceWrite), None)
        .expect("install");
    let session = fresh_session("sess-tie");
    service.set(&session, "agentish").expect("set");
    assert_eq!(service.current(&session.events()), "agentish");
    session
        .append(
            "approval/policy",
            serde_json::json!({ "policy": "never" }),
            None,
        )
        .expect("policy");
    session
        .append(
            "sandbox/mode",
            serde_json::json!({ "mode": "danger-full-access" }),
            None,
        )
        .expect("mode");
    assert_eq!(service.current(&session.events()), "danger-full-access");
}

#[tokio::test(flavor = "current_thread")]
async fn set_writes_through_one_preset_event_plus_both_knob_events() {
    let (_ctx, service) = mounted(None, Some(SandboxMode::WorkspaceWrite), None).expect("install");
    let session = fresh_session("sess-set");
    service
        .set(&session, "danger-full-access")
        .expect("set");
    let events = session.events();
    let shapes: Vec<(String, serde_json::Value)> = events
        .iter()
        .map(|event| (event.type_.clone(), event.data.clone()))
        .collect();
    assert_eq!(
        shapes,
        vec![
            (
                "permission/preset".to_string(),
                serde_json::json!({ "preset": "danger-full-access" })
            ),
            (
                "sandbox/mode".to_string(),
                serde_json::json!({ "mode": "danger-full-access" })
            ),
            (
                "approval/policy".to_string(),
                serde_json::json!({ "policy": "never" })
            ),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn set_to_the_current_preset_is_a_no_op_when_the_knobs_already_match() {
    let (_ctx, service) = mounted(None, Some(SandboxMode::WorkspaceWrite), None).expect("install");
    let session = fresh_session("sess-noop");
    service.set(&session, "workspace-write").expect("set");
    assert!(session.events().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn reasserting_a_preset_from_a_drifted_state_re_records_and_repairs() {
    let (_ctx, service) = mounted(None, Some(SandboxMode::WorkspaceWrite), None).expect("install");
    let session = fresh_session("sess-drift");
    service
        .set(&session, "danger-full-access")
        .expect("set");
    // Re-selecting from a drifted state records the choice and repairs only
    // the changed knob.
    session
        .append(
            "sandbox/mode",
            serde_json::json!({ "mode": "read-only" }),
            None,
        )
        .expect("mode");
    service
        .set(&session, "danger-full-access")
        .expect("set");
    let events = session.events();
    let tail: Vec<(String, serde_json::Value)> = events[4..]
        .iter()
        .map(|event| (event.type_.clone(), event.data.clone()))
        .collect();
    assert_eq!(
        tail,
        vec![
            (
                "permission/preset".to_string(),
                serde_json::json!({ "preset": "danger-full-access" })
            ),
            (
                "sandbox/mode".to_string(),
                serde_json::json!({ "mode": "danger-full-access" })
            ),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_composition_over_a_non_confining_executor() {
    let error = mounted(None, None, None).err().expect("must reject");
    assert!(error.contains("does not confine"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn option_of_presents_labels_descriptions_falls_back_and_fixes_custom() {
    let (_ctx, service) = mounted(None, Some(SandboxMode::WorkspaceWrite), None).expect("install");
    assert_eq!(
        service.option_of("danger-full-access"),
        dsh_permission_presets::PresetOption {
            value: "danger-full-access".to_string(),
            name: "danger-full-access".to_string(),
            description: Some("Full file access without approval prompts.".to_string()),
        }
    );
    assert_eq!(
        service.option_of("custom"),
        dsh_permission_presets::PresetOption {
            value: "custom".to_string(),
            name: "Custom".to_string(),
            description: Some(
                "Current sandbox and approval settings do not match a preset.".to_string()
            ),
        }
    );
    let mut presets = IndexMap::new();
    presets.insert(
        "plain".to_string(),
        PresetSpec {
            sandbox: SandboxMode::WorkspaceWrite,
            approval: ApprovalPolicy::Ask,
            name: None,
            description: None,
        },
    );
    let (_bare_ctx, bare) = mounted(
        Some(Config {
            presets: Some(presets),
            default_preset: None,
        }),
        Some(SandboxMode::WorkspaceWrite),
        None,
    )
    .expect("install");
    assert_eq!(
        bare.option_of("plain"),
        dsh_permission_presets::PresetOption {
            value: "plain".to_string(),
            name: "plain".to_string(),
            description: None,
        }
    );
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = service.option_of("plan");
    }));
    assert!(outcome.is_err(), "unknown option names fail loud");
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_a_table_entry_named_custom() {
    let mut presets = IndexMap::new();
    presets.insert(
        "custom".to_string(),
        PresetSpec {
            sandbox: SandboxMode::ReadOnly,
            approval: ApprovalPolicy::Ask,
            name: None,
            description: None,
        },
    );
    let error = mounted(
        Some(Config {
            presets: Some(presets),
            default_preset: None,
        }),
        Some(SandboxMode::WorkspaceWrite),
        None,
    )
    .err()
    .expect("must reject");
    assert!(error.contains("reserved for the derived not-a-preset state"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn requires_an_explicit_default_when_composition_defaults_match_no_preset() {
    let error = mounted(None, Some(SandboxMode::WorkspaceWrite), Some(ApprovalPolicy::Never))
        .err()
        .expect("must reject");
    assert!(error.contains("configure defaultPreset explicitly"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn reads_a_schema_less_approval_stand_in_as_the_ask_default() {
    let (_ctx, service) = mounted(None, Some(SandboxMode::WorkspaceWrite), None).expect("install");
    let session = fresh_session("sess-standin");
    service.set(&session, "workspace-write").expect("set");
    assert!(session.events().is_empty());
    assert_eq!(service.current(&session.events()), "workspace-write");
}

// ---- new-session default ----

#[tokio::test(flavor = "current_thread")]
async fn pins_the_current_setting_into_each_new_session_without_changing_earlier_sessions() {
    let (ctx, store, service, provider) = mounted_store(None).await;
    let first = store
        .create(&ctx, Some(session_id("first")), None)
        .await
        .expect("create");
    let shapes: Vec<(String, serde_json::Value)> = first
        .events()
        .iter()
        .map(|event| (event.type_.clone(), event.data.clone()))
        .collect();
    assert_eq!(
        shapes,
        vec![
            (
                "permission/preset".to_string(),
                serde_json::json!({ "preset": "workspace-write" })
            ),
            (
                "sandbox/mode".to_string(),
                serde_json::json!({ "mode": "workspace-write" })
            ),
            (
                "approval/policy".to_string(),
                serde_json::json!({ "policy": "ask" })
            ),
        ]
    );

    provider
        .update(
            permission_settings_namespace(),
            serde_json::json!({ "defaultPreset": "danger-full-access" }),
            None,
        )
        .await
        .expect("update");
    assert_eq!(service.default_preset(), "danger-full-access");
    let second = store
        .create(&ctx, Some(session_id("second")), None)
        .await
        .expect("create");
    assert_eq!(service.current(&first.events()), "workspace-write");
    assert_eq!(service.current(&second.events()), "danger-full-access");
    let second_events = second.events();
    let types: Vec<&str> = second_events
        .iter()
        .map(|event| event.type_.as_str())
        .collect();
    assert_eq!(types, vec!["permission/preset", "sandbox/mode", "approval/policy"]);
}

#[tokio::test(flavor = "current_thread")]
async fn preserves_a_seeded_legacy_session_instead_of_applying_the_latest_user_default() {
    let (ctx, store, service, provider) = mounted_store(None).await;
    provider
        .update(
            permission_settings_namespace(),
            serde_json::json!({ "defaultPreset": "danger-full-access" }),
            None,
        )
        .await
        .expect("update");
    let legacy = fresh_session("legacy-source");
    legacy
        .append("turn/start", serde_json::json!({ "turn": 1 }), None)
        .expect("turn/start");
    legacy
        .append(
            "turn/end",
            serde_json::json!({ "turn": 1, "reason": { "kind": "completed" } }),
            None,
        )
        .expect("turn/end");
    let resumed = store
        .create(
            &ctx,
            Some(session_id("legacy-resumed")),
            Some(CreateSessionOptions {
                seed: Some(legacy.events().iter().cloned().collect()),
                meta: None,
            }),
        )
        .await
        .expect("create");
    assert_eq!(service.current(&resumed.events()), "workspace-write");
    let resumed_events = resumed.events();
    let tail: Vec<&str> = resumed_events[3..]
        .iter()
        .map(|event| event.type_.as_str())
        .collect();
    assert_eq!(tail, vec!["permission/preset", "sandbox/mode", "approval/policy"]);
}

#[tokio::test(flavor = "current_thread")]
async fn preserves_composition_defaults_when_an_empty_stored_session_resumes() {
    let (ctx, store, service, provider) = mounted_store(None).await;
    provider
        .update(
            permission_settings_namespace(),
            serde_json::json!({ "defaultPreset": "danger-full-access" }),
            None,
        )
        .await
        .expect("update");
    let resumed = store
        .create(
            &ctx,
            Some(session_id("empty-resumed")),
            Some(CreateSessionOptions {
                seed: Some(Vec::new()),
                meta: None,
            }),
        )
        .await
        .expect("create");
    assert_eq!(service.current(&resumed.events()), "workspace-write");
    let resumed_events = resumed.events();
    let types: Vec<&str> = resumed_events
        .iter()
        .map(|event| event.type_.as_str())
        .collect();
    assert_eq!(
        types,
        vec![
            "session/end-seed",
            "permission/preset",
            "sandbox/mode",
            "approval/policy",
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn pins_sessions_that_already_exist_when_the_service_remounts() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let shell: Arc<dyn ShellExecutor> = Arc::new(FakeShell {
        sandbox: Some(SandboxMode::WorkspaceWrite),
    });
    ctx.register_service(shell);
    let _approval = ApprovalService::install(&ctx, ApprovalConfig::default());
    let existing = store
        .create(&ctx, Some(session_id("existing-before-permission")), None)
        .await
        .expect("create");
    assert!(existing.events().is_empty());

    let service = PermissionPresetService::install(&ctx, Config::default()).expect("install");
    let existing_events = existing.events();
    let types: Vec<&str> = existing_events
        .iter()
        .map(|event| event.type_.as_str())
        .collect();
    assert_eq!(types, vec!["permission/preset", "sandbox/mode", "approval/policy"]);
    assert_eq!(service.current(&existing.events()), "workspace-write");
}

#[tokio::test(flavor = "current_thread")]
async fn fills_only_missing_legacy_facts_and_preserves_an_unmatched_seeded_combination() {
    let (ctx, store, service, _provider) = mounted_store(None).await;
    let partial = fresh_session("partial-source");
    partial
        .append(
            "sandbox/mode",
            serde_json::json!({ "mode": "workspace-write" }),
            None,
        )
        .expect("mode");
    partial
        .append(
            "approval/policy",
            serde_json::json!({ "policy": "ask" }),
            None,
        )
        .expect("policy");
    let resumed = store
        .create(
            &ctx,
            Some(session_id("partial-resumed")),
            Some(CreateSessionOptions {
                seed: Some(partial.events().iter().cloned().collect()),
                meta: None,
            }),
        )
        .await
        .expect("create");
    let resumed_events = resumed.events();
    let last = resumed_events.last().expect("last");
    assert_eq!(last.type_, "permission/preset");
    assert_eq!(last.data["preset"], "workspace-write");

    let custom = fresh_session("custom-source");
    custom
        .append(
            "sandbox/mode",
            serde_json::json!({ "mode": "read-only" }),
            None,
        )
        .expect("mode");
    custom
        .append(
            "approval/policy",
            serde_json::json!({ "policy": "never" }),
            None,
        )
        .expect("policy");
    let unmatched = store
        .create(
            &ctx,
            Some(session_id("custom-resumed")),
            Some(CreateSessionOptions {
                seed: Some(custom.events().iter().cloned().collect()),
                meta: None,
            }),
        )
        .await
        .expect("create");
    assert_eq!(service.current(&unmatched.events()), CUSTOM_PRESET);
    let unmatched_events = unmatched.events();
    assert_eq!(
        unmatched_events.last().map(|event| event.type_.as_str()),
        Some("session/end-seed")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn materializes_ask_when_a_legacy_seed_and_approval_stand_in_omit_the_policy() {
    let (ctx, store, _service, _provider) = mounted_store(None).await;
    let partial = fresh_session("approval-fallback-source");
    partial
        .append(
            "sandbox/mode",
            serde_json::json!({ "mode": "workspace-write" }),
            None,
        )
        .expect("mode");
    let resumed = store
        .create(
            &ctx,
            Some(session_id("approval-fallback-resumed")),
            Some(CreateSessionOptions {
                seed: Some(partial.events().iter().cloned().collect()),
                meta: None,
            }),
        )
        .await
        .expect("create");
    let resumed_events = resumed.events();
    let last = resumed_events.last().expect("last");
    assert_eq!(last.type_, "approval/policy");
    assert_eq!(last.data["policy"], "ask");
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_a_stored_default_outside_the_configured_preset_table() {
    let (_ctx, _store, service, provider) = mounted_store(None).await;
    let error = provider
        .update(
            permission_settings_namespace(),
            serde_json::json!({ "defaultPreset": "missing" }),
            None,
        )
        .await
        .expect_err("must reject");
    assert!(!error.is_empty());
    assert_eq!(service.default_preset(), "workspace-write");
}

// ---- projection-state JSON adapters ----

#[test]
fn knob_state_json_round_trips() {
    let state = dsh_permission_presets::KnobState {
        preset: Some("danger-full-access".to_string()),
        sandbox: Some(SandboxMode::DangerFullAccess),
        approval: Some(ApprovalPolicy::Never),
    };
    let json = knob_state_to_json(&state);
    assert_eq!(json["preset"], "danger-full-access");
    assert_eq!(json["sandbox"], "danger-full-access");
    assert_eq!(json["approval"], "never");
    let back = knob_state_from_json(&json);
    assert_eq!(back, state);
    let empty = knob_state_from_json(&serde_json::json!({
        "preset": null,
        "sandbox": null,
        "approval": null,
    }));
    assert_eq!(empty, dsh_permission_presets::EMPTY_KNOBS);
}

// ---- invariant companion ----

#[tokio::test(flavor = "current_thread")]
async fn checker_rejects_unknown_preset_payloads_with_the_ts_messages() {
    let (_ctx, service) = mounted(None, Some(SandboxMode::WorkspaceWrite), None).expect("install");
    let unknown = synthetic_event("permission/preset", serde_json::json!({ "preset": "yolo" }));
    assert_eq!(
        invariant::validate_event(&service, &unknown).expect_err("unknown"),
        "permission/preset names unknown preset \"yolo\""
    );
    let known =
        synthetic_event("permission/preset", serde_json::json!({ "preset": "workspace-write" }));
    invariant::validate_event(&service, &known).expect("known");
    let unrelated = synthetic_event("turn/start", serde_json::json!({ "turn": 1 }));
    invariant::validate_event(&service, &unrelated).expect("unrelated");
}

#[tokio::test(flavor = "current_thread")]
async fn companion_accepts_known_presets_and_contains_unknown_ones() {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let shell: Arc<dyn ShellExecutor> = Arc::new(FakeShell {
        sandbox: Some(SandboxMode::WorkspaceWrite),
    });
    ctx.register_service(shell);
    let _approval = ApprovalService::install(&ctx, ApprovalConfig::default());
    let service = PermissionPresetService::install(&ctx, Config::default()).expect("install");
    let _registry = InvariantRegistry::new(
        &ctx,
        InvariantConfig {
            enabled: true,
            package_allowlist: vec![],
            package_blocklist: vec![],
        },
    );
    let fiber = ctx.plugin(Arc::new(PermissionPresetsInvariantPlugin), arc(()));
    fiber.settle().await.expect("settle");

    let session = store
        .create(&ctx, Some(session_id("perm-invariant")), None)
        .await
        .expect("create");
    session
        .append(
            "permission/preset",
            serde_json::json!({ "preset": "danger-full-access" }),
            None,
        )
        .expect("known preset");
    // Deviation note: the TS append veto throws; this port contains
    // internal-listener panics, so the unknown preset commits and the
    // checker rejects the same shape.
    let event = session
        .append(
            "permission/preset",
            serde_json::json!({ "preset": "yolo" }),
            None,
        )
        .expect("contained");
    assert_eq!(
        invariant::validate_event(&service, &event).expect_err("shape"),
        "permission/preset names unknown preset \"yolo\""
    );
}
