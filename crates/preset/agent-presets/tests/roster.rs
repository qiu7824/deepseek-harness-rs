//! Roster integration tests: Rust port of the core subset of
//! `tests/user-root.spec.ts` and `tests/settings.spec.ts`.
//!
//! Covers: the derived harness-home user root (via the injected env
//! reader), first-root-wins resolution, copy into the derived root, the
//! `includeUserRoot: false` opt-out, default-id layering over the settings
//! document, and the remove-clears-deleted-default behavior.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use common::boot;
use dsh_agent_presets::{AgentPresets, Config, PresetRoot, PresetTrust};
use dsh_schemastery::Data;
use dsh_settings::SettingsStorage;
use indexmap::IndexMap;
use parking_lot::Mutex;

const VALID: &str = "- id: tool-alpha\n  name: contribute\n  config:\n    tool: alpha\n";

/// In-memory settings storage (writable, starts empty).
#[derive(Default)]
struct MemoryStorage {
    document: Mutex<IndexMap<String, Data>>,
}

#[async_trait::async_trait]
impl SettingsStorage for MemoryStorage {
    fn writable(&self) -> bool {
        true
    }

    async fn load(&self) -> Result<IndexMap<String, Data>, String> {
        Ok(self.document.lock().clone())
    }

    async fn persist(
        &self,
        ns: &dsh_settings::SettingsNamespace,
        section: Data,
    ) -> Result<(), String> {
        self.document
            .lock()
            .insert(ns.as_str().to_string(), section);
        Ok(())
    }
}

fn counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn temp_dir(label: &str) -> std::path::PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "dsh-preset-roster-{label}-{}-{}-{nonce}",
        std::process::id(),
        counter()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

async fn seed(root: &std::path::Path, id: &str) {
    let dir = root.join(id);
    tokio::fs::create_dir_all(&dir)
        .await
        .expect("create preset dir");
    tokio::fs::write(dir.join(dsh_agent_presets::COMPOSITION_FILE), VALID)
        .await
        .expect("write composition");
}

struct Roster {
    ctx: cordis::Context,
    service: Arc<AgentPresets>,
}

async fn roster(config: Config, env: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>) -> Roster {
    let ctx = boot().await;
    let storage = Arc::new(MemoryStorage::default());
    dsh_settings::SettingsProvider::install(&ctx, storage.clone());
    let service = AgentPresets::install(&ctx, config, env).expect("roster installs");
    service.ready().await.expect("settings wiring settles");
    Roster { ctx, service }
}

fn process_env_reader() -> Arc<dyn Fn(&str) -> Option<String> + Send + Sync> {
    Arc::new(|name: &str| std::env::var(name).ok())
}

#[tokio::test]
async fn derives_the_harness_home_user_root() {
    let home = temp_dir("home");
    let home_for_env = home.clone();
    let env = Arc::new(move |name: &str| {
        (name == "DSH_HOME").then(|| home_for_env.to_string_lossy().to_string())
    });
    let system = temp_dir("system");
    seed(&system, "standard").await;
    seed(&home.join(".agent-presets"), "mine").await;

    let roster = roster(
        Config {
            default: "standard".to_string(),
            roots: vec![PresetRoot {
                path: system.to_string_lossy().to_string(),
                trust: PresetTrust::System,
            }],
            include_user_root: true,
        },
        env,
    )
    .await;

    // The home preset is discovered without any app configuring it.
    let listed = roster.service.list().await.expect("list");
    let mine = listed
        .iter()
        .find(|preset| preset.id == "mine")
        .expect("home preset");
    assert_eq!(mine.trust, PresetTrust::User);
    assert_eq!(
        roster
            .service
            .resolve(Some("mine"))
            .await
            .expect("resolve")
            .path,
        home.join(".agent-presets")
            .join("mine")
            .join(dsh_agent_presets::COMPOSITION_FILE)
            .to_string_lossy()
    );
    // A roster with only a system root is authorable: the copy lands at
    // home.
    assert!(roster.service.authorable());
    roster
        .service
        .copy("standard", "copied", None)
        .await
        .expect("copy succeeds");
    assert!(
        home.join(".agent-presets")
            .join("copied")
            .join(dsh_agent_presets::COMPOSITION_FILE)
            .exists()
    );
}

#[tokio::test]
async fn a_shipped_id_shadows_a_home_directory() {
    let home = temp_dir("home");
    let home_for_env = home.clone();
    let env = Arc::new(move |name: &str| {
        (name == "DSH_HOME").then(|| home_for_env.to_string_lossy().to_string())
    });
    let system = temp_dir("system");
    seed(&system, "standard").await;
    seed(&home.join(".agent-presets"), "standard").await;

    let roster = roster(
        Config {
            default: "standard".to_string(),
            roots: vec![PresetRoot {
                path: system.to_string_lossy().to_string(),
                trust: PresetTrust::System,
            }],
            include_user_root: true,
        },
        env,
    )
    .await;

    // The earlier configured root wins the duplicate id.
    assert_eq!(
        roster
            .service
            .resolve(Some("standard"))
            .await
            .expect("resolve")
            .trust,
        PresetTrust::System
    );
    // The roster check refuses the id: a user directory named like a
    // shipped preset is shadowed by it.
    let error = roster
        .service
        .copy("standard", "standard", None)
        .await
        .expect_err("shadowed id refused");
    assert!(
        error.contains("already exists"),
        "unexpected diagnostic: {error}"
    );
}

#[tokio::test]
async fn include_user_root_false_leaves_the_roster_unauthorable() {
    let home = temp_dir("home");
    let home_for_env = home.clone();
    let env = Arc::new(move |name: &str| {
        (name == "DSH_HOME").then(|| home_for_env.to_string_lossy().to_string())
    });
    let system = temp_dir("system");
    seed(&system, "standard").await;
    seed(&home.join(".agent-presets"), "mine").await;

    let roster = roster(
        Config {
            default: "standard".to_string(),
            roots: vec![PresetRoot {
                path: system.to_string_lossy().to_string(),
                trust: PresetTrust::System,
            }],
            include_user_root: false,
        },
        env,
    )
    .await;

    let ids: Vec<String> = roster
        .service
        .list()
        .await
        .expect("list")
        .into_iter()
        .map(|preset| preset.id)
        .collect();
    assert!(!ids.contains(&"mine".to_string()));
    assert!(!roster.service.authorable());
    let error = roster
        .service
        .copy("standard", "mine", None)
        .await
        .expect_err("no writable root");
    assert!(
        error.contains("no user-writable preset root"),
        "unexpected diagnostic: {error}"
    );
}

#[tokio::test]
async fn a_configured_user_root_receives_copies_first() {
    let home = temp_dir("home");
    let home_for_env = home.clone();
    let env = Arc::new(move |name: &str| {
        (name == "DSH_HOME").then(|| home_for_env.to_string_lossy().to_string())
    });
    let system = temp_dir("system");
    let explicit = temp_dir("explicit");
    seed(&system, "standard").await;

    let roster = roster(
        Config {
            default: "standard".to_string(),
            roots: vec![
                PresetRoot {
                    path: system.to_string_lossy().to_string(),
                    trust: PresetTrust::System,
                },
                PresetRoot {
                    path: explicit.to_string_lossy().to_string(),
                    trust: PresetTrust::User,
                },
            ],
            include_user_root: true,
        },
        env,
    )
    .await;

    roster
        .service
        .copy("standard", "copied", None)
        .await
        .expect("copy succeeds");
    assert!(explicit.join("copied").exists());
    assert!(!home.join(".agent-presets").join("copied").exists());
}

#[tokio::test]
async fn the_settings_default_overrides_the_config_default() {
    let home = temp_dir("home");
    let home_for_env = home.clone();
    let env = Arc::new(move |name: &str| {
        (name == "DSH_HOME").then(|| home_for_env.to_string_lossy().to_string())
    });
    let system = temp_dir("system");
    seed(&system, "standard").await;
    seed(&system, "minimal").await;

    let ctx = boot().await;
    let storage = Arc::new(MemoryStorage {
        document: Mutex::new({
            let mut map = IndexMap::new();
            map.insert(
                "agent-presets".to_string(),
                dsh_schemastery::Data::Object({
                    let mut section = IndexMap::new();
                    section.insert("default".to_string(), Data::String("minimal".to_string()));
                    section
                }),
            );
            map
        }),
    });
    dsh_settings::SettingsProvider::install(&ctx, storage.clone());
    let service = AgentPresets::install(
        &ctx,
        Config {
            default: "standard".to_string(),
            roots: vec![PresetRoot {
                path: system.to_string_lossy().to_string(),
                trust: PresetTrust::System,
            }],
            include_user_root: false,
        },
        env,
    )
    .expect("roster installs");
    service.ready().await.expect("settings wiring settles");

    assert_eq!(service.default_id(), "minimal");
    // Resolving without an id follows the layered default.
    assert_eq!(service.resolve(None).await.expect("resolve").id, "minimal");
}

#[tokio::test]
async fn removing_the_selected_default_clears_it() {
    let home = temp_dir("home");
    let home_for_env = home.clone();
    let env = Arc::new(move |name: &str| {
        (name == "DSH_HOME").then(|| home_for_env.to_string_lossy().to_string())
    });
    let system = temp_dir("system");
    seed(&system, "standard").await;
    seed(&home.join(".agent-presets"), "mine").await;

    let ctx = boot().await;
    let storage = Arc::new(MemoryStorage::default());
    dsh_settings::SettingsProvider::install(&ctx, storage.clone());
    let service = AgentPresets::install(
        &ctx,
        Config {
            default: "standard".to_string(),
            roots: vec![PresetRoot {
                path: system.to_string_lossy().to_string(),
                trust: PresetTrust::System,
            }],
            include_user_root: true,
        },
        env,
    )
    .expect("roster installs");
    service.ready().await.expect("settings wiring settles");

    // Pin the default onto the user layer, then delete that preset: the
    // deployment default must show through again.
    let namespace =
        dsh_settings::settings_namespace(dsh_agent_presets::SETTINGS_NAMESPACE).expect("namespace");
    let provider = ctx
        .get_typed::<Arc<dsh_settings::SettingsProvider>>("settings", true)
        .expect("settings service");
    provider
        .mutate(
            &namespace,
            vec![dsh_settings::SettingsPathOp::Set {
                path: vec!["default".to_string()],
                value: serde_json::Value::String("mine".to_string()),
            }],
            None,
        )
        .await
        .expect("set default");
    assert_eq!(service.default_id(), "mine");

    service.remove("mine").await.expect("remove mine");
    assert_eq!(
        service.default_id(),
        "standard",
        "the deleted default clears to the deployment default"
    );
    assert!(!home.join(".agent-presets").join("mine").exists());
}

#[tokio::test]
async fn resolve_reports_unknown_ids_with_the_roster() {
    let system = temp_dir("system");
    seed(&system, "standard").await;
    let roster = roster(
        Config {
            default: "standard".to_string(),
            roots: vec![PresetRoot {
                path: system.to_string_lossy().to_string(),
                trust: PresetTrust::System,
            }],
            include_user_root: false,
        },
        process_env_reader(),
    )
    .await;

    let error = roster
        .service
        .resolve(Some("nope"))
        .await
        .expect_err("unknown id");
    assert!(error.to_string().contains("not found"), "{error}");
    assert_eq!(error.available, "standard");
}

#[tokio::test]
async fn read_returns_the_composition_text() {
    let system = temp_dir("system");
    seed(&system, "standard").await;
    let roster = roster(
        Config {
            default: "standard".to_string(),
            roots: vec![PresetRoot {
                path: system.to_string_lossy().to_string(),
                trust: PresetTrust::System,
            }],
            include_user_root: false,
        },
        process_env_reader(),
    )
    .await;

    let text = roster.service.read("standard").await.expect("read");
    assert_eq!(text, VALID);
}
