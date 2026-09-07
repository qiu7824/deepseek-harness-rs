use super::*;
use dsh_cordis_loader::{EntryOptions, LoaderCore, LoaderService};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

struct Probe {
    core: Arc<LoaderCore>,
    fail_planning: Arc<AtomicBool>,
}
#[async_trait::async_trait]
impl cordis::Plugin for Probe {
    async fn apply(&self, ctx: &Context, _: ArcValue) -> Result<(), PluginError> {
        let entry = self
            .core
            .entry_of(&ctx.fiber)
            .ok_or_else(|| PluginError::new(arc("missing preset entry owner".to_owned())))?;
        if entry.options.lock().id == "plan-mode" && self.fail_planning.load(Ordering::SeqCst) {
            return Err(PluginError::new(arc(
                "planning initialization failed".to_owned()
            )));
        }
        Ok(())
    }
}
struct Fixture {
    host: Scope,
    loader: Arc<LoaderService>,
    service: Arc<AgentPresets>,
    preset: AgentPreset,
    file: std::path::PathBuf,
    directory: std::path::PathBuf,
    fail: Arc<AtomicBool>,
}
impl Fixture {
    async fn new() -> Self {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let parent = std::env::var_os("DSH_TEST_TMP_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let directory = parent.join(format!(
            "dsh-standard-standing-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../config/agent-presets/standard/agent.cordis.yml");
        let text = std::fs::read_to_string(source).unwrap();
        let file = directory.join("agent.cordis.yml");
        std::fs::write(&file, &text).unwrap();
        let root = Context::root();
        let host = create_scope(&root, ScopeKey::new(), &Default::default());
        let loader = LoaderService::new(&host.ctx).await;
        host.ctx.register_service(loader.clone());
        let fail = Arc::new(AtomicBool::new(false));
        let probe: Arc<dyn cordis::Plugin> = Arc::new(Probe {
            core: loader.core.clone(),
            fail_planning: fail.clone(),
        });
        fn register(
            entries: Vec<EntryOptions>,
            core: &Arc<LoaderCore>,
            probe: &Arc<dyn cordis::Plugin>,
        ) {
            for entry in entries {
                if matches!(entry.name.as_str(), "cordis:group" | "group") {
                    register(
                        serde_json::from_value(entry.config.unwrap()).unwrap(),
                        core,
                        probe,
                    );
                } else {
                    core.register(&entry.name, probe.clone());
                }
            }
        }
        register(serde_yaml::from_str(&text).unwrap(), &loader.core, &probe);
        let service = AgentPresets::install(
            &host.ctx,
            Config {
                default: "standard".into(),
                roots: vec![],
                include_user_root: false,
            },
            Arc::new(|_| None),
        )
        .unwrap();
        let preset = AgentPreset {
            id: "standard".into(),
            trust: crate::PresetTrust::System,
            path: file.to_string_lossy().into(),
            name: None,
            description: None,
            order: None,
            broken: None,
        };
        Self {
            host,
            loader,
            service,
            preset,
            file,
            directory,
            fail,
        }
    }
    async fn concurrent_mounts(&self) -> Vec<Arc<StandingMount>> {
        let barrier = Arc::new(tokio::sync::Barrier::new(24));
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..24 {
            let barrier = barrier.clone();
            let service = self.service.clone();
            let preset = self.preset.clone();
            tasks.spawn(async move {
                barrier.wait().await;
                service.ensure_standing(&preset).await.unwrap()
            });
        }
        let mut values = Vec::new();
        while let Some(value) =
            tokio::time::timeout(std::time::Duration::from_secs(20), tasks.join_next())
                .await
                .unwrap()
        {
            values.push(value.unwrap());
        }
        values
    }
    async fn dispose(self) {
        (self.host.dispose)().await;
        assert!(
            self.loader.core.entries_by_fiber.lock().is_empty(),
            "disposed preset retained loader fiber ownership"
        );
        assert!(self.loader.core.carrier_fibers.lock().is_empty());
        let _ = std::fs::remove_file(self.file);
        let _ = std::fs::remove_dir(self.directory);
        crate::live_preset_mounts();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn standard_preset_concurrent_create_and_rebuild_share_one_generation() {
    let fixture = Fixture::new().await;
    let first = fixture.concurrent_mounts().await;
    assert!(first.iter().all(|mount| mount.key == first[0].key));
    let baseline = fixture.loader.core.entries_by_fiber.lock().len();
    assert!(baseline > 10);
    let old_flight = fixture.service.standing.lock()["standard"].clone();
    let old_keys = fixture
        .loader
        .core
        .entries_by_fiber
        .lock()
        .keys()
        .copied()
        .collect::<Vec<_>>();
    let mut text = std::fs::read_to_string(&fixture.file).unwrap();
    text.push_str("\n# fixture second generation\n");
    std::fs::write(&fixture.file, text).unwrap();
    let second = fixture.concurrent_mounts().await;
    assert!(second.iter().all(|mount| mount.key == second[0].key));
    assert_ne!(first[0].key, second[0].key);
    let current = fixture.service.standing.lock()["standard"].clone();
    fixture.service.remove_standing_if("standard", &old_flight);
    assert!(
        Arc::ptr_eq(&fixture.service.standing.lock()["standard"], &current),
        "stale generation cleanup removed current mount"
    );
    (first[0].scope.dispose)().await;
    let live = fixture.loader.core.entries_by_fiber.lock();
    assert_eq!(live.len(), baseline);
    assert!(old_keys.iter().all(|key| !live.contains_key(key)));
    drop(live);
    fixture.dispose().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_standard_initialization_is_evicted_and_can_mount_after_repair() {
    let fixture = Fixture::new().await;
    fixture.fail.store(true, Ordering::SeqCst);
    let failure = fixture.service.ensure_standing(&fixture.preset).await;
    assert!(failure.is_err());
    assert!(fixture.service.standing.lock().is_empty());
    assert!(fixture.loader.core.entries_by_fiber.lock().is_empty());
    assert!(fixture.loader.core.carrier_fibers.lock().is_empty());
    fixture.fail.store(false, Ordering::SeqCst);
    let mounts = fixture.concurrent_mounts().await;
    assert!(mounts.iter().all(|mount| mount.key == mounts[0].key));
    fixture.dispose().await;
}
