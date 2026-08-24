#![allow(clippy::type_complexity)]
// Test environment lookups mirror the adapter's injected callback shape.

//! Rust port of the core `packages/e2b/e2b/tests/e2b.spec.ts` behaviors:
//! the control-env isolation, the shared protected sandbox lifecycle,
//! disposal races, environment/config validation, setup rollback, and the
//! helper + invariant companion surface.
//!
//! # Deviations
//!
//! - The `vi.mock('e2b')` SDK factory collapse is a `FakeSdk` with scripted
//!   results and a settable creation gate.
//! - The environment lookup is injected instead of stubbing the
//!   process-global `E2B_API_KEY`.
//! - `ctx.logger.error` stubbing collapses to a capturing exporter.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use cordis::{Context, Exporter, FiberCore, LoggerLevel, arc};
use dsh_e2b::{
    E2bBackgroundOptions, E2bCommandHandle, E2bCommandOptions, E2bCommandResult, E2bCreateOptions,
    E2bEntryInfo, E2bPlugin, E2bRuntime, E2bSandbox, E2bSdk, E2bSdkError, FileType,
    SandboxNotFoundError, e2b_control_envs, quote_e2b_shell_arg,
};
use futures::future::BoxFuture;
use parking_lot::Mutex;

fn env_lookup() -> Arc<dyn Fn(&str) -> Option<String> + Send + Sync> {
    Arc::new(|name: &str| {
        if name == "E2B_API_KEY" {
            std::env::var(name).ok()
        } else {
            None
        }
    })
}

/// A scripted remote sandbox (TS `fakeSandbox`).
struct FakeSandbox {
    id: String,
    make_dirs: Mutex<Vec<String>>,
    make_dir_results: Mutex<VecDeque<Result<bool, E2bSdkError>>>,
    get_infos: Mutex<Vec<String>>,
    get_info_results: Mutex<VecDeque<Result<E2bEntryInfo, E2bSdkError>>>,
    runs: Mutex<Vec<(String, Option<HashMap<String, String>>)>>,
    run_results: Mutex<VecDeque<Result<E2bCommandResult, E2bSdkError>>>,
    kills: Mutex<u32>,
    kill_results: Mutex<VecDeque<Result<(), E2bSdkError>>>,
}

impl FakeSandbox {
    fn new(id: &str) -> Arc<Self> {
        Arc::new(Self {
            id: id.to_string(),
            make_dirs: Mutex::new(Vec::new()),
            make_dir_results: Mutex::new(VecDeque::new()),
            get_infos: Mutex::new(Vec::new()),
            get_info_results: Mutex::new(VecDeque::new()),
            runs: Mutex::new(Vec::new()),
            run_results: Mutex::new(VecDeque::new()),
            kills: Mutex::new(0),
            kill_results: Mutex::new(VecDeque::new()),
        })
    }
}

#[async_trait::async_trait]
impl E2bSandbox for FakeSandbox {
    fn sandbox_id(&self) -> &str {
        &self.id
    }

    async fn make_dir(&self, path: &str) -> Result<bool, E2bSdkError> {
        self.make_dirs.lock().push(path.to_string());
        self.make_dir_results.lock().pop_front().unwrap_or(Ok(true))
    }

    async fn get_info(&self, path: &str) -> Result<E2bEntryInfo, E2bSdkError> {
        self.get_infos.lock().push(path.to_string());
        self.get_info_results
            .lock()
            .pop_front()
            .unwrap_or(Ok(E2bEntryInfo {
                file_type: FileType::Dir,
                symlink_target: None,
                ..Default::default()
            }))
    }

    async fn read_bytes(&self, _path: &str) -> Result<Vec<u8>, E2bSdkError> {
        Ok(Vec::new())
    }

    async fn read_stream(
        &self,
        _path: &str,
    ) -> Result<Box<dyn dsh_e2b::E2bReadStream>, E2bSdkError> {
        Err(E2bSdkError::other("read stream not scripted"))
    }

    async fn list(&self, _path: &str) -> Result<Vec<E2bEntryInfo>, E2bSdkError> {
        Ok(Vec::new())
    }

    async fn write(
        &self,
        _path: &str,
        _content: &[u8],
        _metadata: Option<HashMap<String, String>>,
    ) -> Result<(), E2bSdkError> {
        Ok(())
    }

    async fn rename(&self, _from: &str, _to: &str) -> Result<E2bEntryInfo, E2bSdkError> {
        Ok(E2bEntryInfo::default())
    }

    async fn remove(&self, _path: &str) -> Result<(), E2bSdkError> {
        Ok(())
    }

    async fn run(
        &self,
        command: &str,
        options: &E2bCommandOptions,
    ) -> Result<E2bCommandResult, E2bSdkError> {
        self.runs
            .lock()
            .push((command.to_string(), options.envs.clone()));
        self.run_results
            .lock()
            .pop_front()
            .unwrap_or(Ok(E2bCommandResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            }))
    }

    async fn run_background(
        &self,
        _command: &str,
        _options: &E2bBackgroundOptions,
    ) -> Result<Arc<dyn E2bCommandHandle>, E2bSdkError> {
        Err(E2bSdkError::other(
            "background commands are unsupported in this fake",
        ))
    }

    async fn kill(&self) -> Result<(), E2bSdkError> {
        *self.kills.lock() += 1;
        self.kill_results.lock().pop_front().unwrap_or(Ok(()))
    }
}

/// A scripted SDK factory (TS `sdk.create` mock).
struct FakeSdk {
    results: Mutex<VecDeque<Result<Arc<dyn E2bSandbox>, E2bSdkError>>>,
    creates: Mutex<Vec<E2bCreateOptions>>,
    gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl FakeSdk {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            results: Mutex::new(VecDeque::new()),
            creates: Mutex::new(Vec::new()),
            gate: Mutex::new(None),
        })
    }
}

impl E2bSdk for FakeSdk {
    fn create(
        &self,
        options: &E2bCreateOptions,
    ) -> BoxFuture<'static, Result<Arc<dyn E2bSandbox>, E2bSdkError>> {
        self.creates.lock().push(options.clone());
        let result = self.results.lock().pop_front();
        let gate = self.gate.lock().take();
        Box::pin(async move {
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            result.unwrap_or_else(|| Err(E2bSdkError::other("no scripted create result")))
        })
    }
}

/// A logger exporter capturing error records.
struct CaptureExporter {
    messages: Arc<Mutex<Vec<String>>>,
}

impl Exporter for CaptureExporter {
    fn default_level(&self) -> LoggerLevel {
        LoggerLevel::Error
    }

    fn export(&self, message: &cordis::Message) {
        let text = message
            .args
            .iter()
            .filter_map(|arg| cordis::downcast::<String>(arg).cloned())
            .collect::<Vec<_>>()
            .join(" ");
        self.messages.lock().push(text);
    }
}

async fn setup(
    sdk: Arc<FakeSdk>,
    config: serde_json::Value,
    lookup: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
) -> (Context, Arc<E2bRuntime>, Arc<FiberCore>) {
    let ctx = Context::root();
    let fiber = ctx.plugin(Arc::new(E2bPlugin::new(sdk, lookup)), arc(config));
    fiber.settle().await.expect("e2b loads");
    let runtime = ctx
        .get_typed::<Arc<E2bRuntime>>("e2b", false)
        .map(|slot| slot.as_ref().clone())
        .expect("e2b service");
    (ctx, runtime, fiber)
}

// ---- helpers ----

#[test]
fn gives_each_sdk_login_shell_a_fresh_non_overridable_control_home() {
    let first = e2b_control_envs(HashMap::from([
        ("HOME".to_string(), "/hostile".to_string()),
        ("NPM_TOKEN".to_string(), String::new()),
    ]));
    let second = e2b_control_envs(HashMap::new());

    assert!(
        first["HOME"].starts_with("/.dsh-e2b-control-"),
        "{}",
        first["HOME"]
    );
    assert_eq!(first["NPM_TOKEN"], "");
    assert_ne!(first["HOME"], second["HOME"]);
}

#[test]
fn quotes_opaque_shell_arguments_without_interpolation() {
    assert_eq!(quote_e2b_shell_arg("a'b $HOME"), "'a'\"'\"'b $HOME'");
}

// ---- runtime ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn creates_one_protected_shared_sandbox_and_kills_it_on_default_disposal() {
    let fixture = FakeSandbox::new("sandbox-1");
    let sdk = FakeSdk::new();
    sdk.results
        .lock()
        .push_back(Ok(fixture.clone() as Arc<dyn E2bSandbox>));
    let (ctx, runtime, fiber) = setup(
        sdk.clone(),
        serde_json::json!({ "apiKey": "test-key" }),
        env_lookup(),
    )
    .await;

    let sandbox = runtime.get_sandbox().await.expect("sandbox");
    assert!(Arc::ptr_eq(
        &sandbox,
        &(fixture.clone() as Arc<dyn E2bSandbox>)
    ));
    assert_eq!(runtime.cwd(), "/home/user/workspace");
    assert_eq!(runtime.runtime_root(), "/home/user/workspace/.dsh-e2b");
    assert_eq!(
        *sdk.creates.lock(),
        vec![E2bCreateOptions {
            api_key: "test-key".to_string(),
            timeout_ms: 300_000
        }]
    );
    assert_eq!(
        *fixture.make_dirs.lock(),
        vec![
            "/home/user/workspace".to_string(),
            "/home/user/workspace/.dsh-e2b".to_string()
        ]
    );
    assert_eq!(
        *fixture.get_infos.lock(),
        vec!["/home/user/workspace/.dsh-e2b".to_string()]
    );
    let (command, envs) = fixture.runs.lock()[0].clone();
    assert!(envs.as_ref().expect("envs")["HOME"].starts_with("/.dsh-e2b-control-"));
    assert_eq!(command, "chmod 700 -- '/home/user/workspace/.dsh-e2b'");

    fiber.dispose().await;
    assert_eq!(*fixture.kills.lock(), 1);
    let error = runtime.get_sandbox().await.err().expect("disposing");
    assert!(error.contains("disposing"), "{error}");
    let _ = ctx;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_handle_acquisition_when_disposal_starts_during_setup() {
    let fixture = FakeSandbox::new("sandbox-1");
    let sdk = FakeSdk::new();
    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel();
    *sdk.gate.lock() = Some(gate_rx);
    sdk.results
        .lock()
        .push_back(Ok(fixture.clone() as Arc<dyn E2bSandbox>));
    let (_ctx, runtime, fiber) = setup(
        sdk,
        serde_json::json!({ "apiKey": "test-key" }),
        env_lookup(),
    )
    .await;

    let acquisition = runtime.get_sandbox();
    let disposing = fiber.dispose();
    tokio::pin!(disposing);
    // Drive disposal far enough that it started setup (creation is gated).
    assert!(futures::poll!(&mut disposing).is_pending());
    // Release creation; setup finishes while disposal is already committed.
    drop(gate_tx);
    disposing.await;
    let error = acquisition.await.err().expect("acquisition rejected");
    assert!(error.contains("disposing"), "{error}");
    assert_eq!(*fixture.kills.lock(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_the_key_from_the_environment_and_honors_the_configured_cwd_and_lifetime() {
    let fixture = FakeSandbox::new("configured-sandbox");
    let sdk = FakeSdk::new();
    sdk.results
        .lock()
        .push_back(Ok(fixture.clone() as Arc<dyn E2bSandbox>));
    let lookup: Arc<dyn Fn(&str) -> Option<String> + Send + Sync> =
        Arc::new(|_: &str| Some("environment-key".to_string()));
    let (_ctx, runtime, fiber) = setup(
        sdk.clone(),
        serde_json::json!({ "cwd": "/workspace/project", "timeoutMs": 60_000 }),
        lookup,
    )
    .await;
    runtime.get_sandbox().await.expect("sandbox");

    assert_eq!(
        *sdk.creates.lock(),
        vec![E2bCreateOptions {
            api_key: "environment-key".to_string(),
            timeout_ms: 60_000
        }]
    );
    assert_eq!(runtime.cwd(), "/workspace/project");
    fiber.dispose().await;
    assert_eq!(*fixture.kills.lock(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepts_a_missing_sandbox_when_disposal_itself_requests_deletion() {
    let fixture = FakeSandbox::new("sandbox-1");
    fixture
        .kill_results
        .lock()
        .push_back(Err(SandboxNotFoundError::not_found("already deleted")));
    let sdk = FakeSdk::new();
    sdk.results
        .lock()
        .push_back(Ok(fixture.clone() as Arc<dyn E2bSandbox>));
    let (ctx, runtime, fiber) = setup(
        sdk,
        serde_json::json!({ "apiKey": "test-key" }),
        env_lookup(),
    )
    .await;
    let exporter = Arc::new(CaptureExporter {
        messages: Arc::new(Mutex::new(Vec::new())),
    });
    let messages = exporter.messages.clone();
    ctx.logger.exporter(&ctx, exporter);

    runtime.get_sandbox().await.expect("sandbox");
    fiber.dispose().await;
    assert_eq!(*fixture.kills.lock(), 1);
    assert!(messages.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn does_not_classify_other_disposal_failures_as_an_already_gone_sandbox() {
    let fixture = FakeSandbox::new("sandbox-1");
    fixture
        .kill_results
        .lock()
        .push_back(Err(E2bSdkError::other("disposition unknown")));
    let sdk = FakeSdk::new();
    sdk.results
        .lock()
        .push_back(Ok(fixture.clone() as Arc<dyn E2bSandbox>));
    let (ctx, runtime, fiber) = setup(
        sdk,
        serde_json::json!({ "apiKey": "test-key" }),
        env_lookup(),
    )
    .await;
    let exporter = Arc::new(CaptureExporter {
        messages: Arc::new(Mutex::new(Vec::new())),
    });
    let messages = exporter.messages.clone();
    ctx.logger.exporter(&ctx, exporter);

    runtime.get_sandbox().await.expect("sandbox");
    fiber.dispose().await;
    assert_eq!(*fixture.kills.lock(), 1);
    assert!(
        messages
            .lock()
            .iter()
            .any(|message| message.contains("disposition unknown")),
        "{:?}",
        *messages.lock()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kills_a_newly_created_sandbox_when_remote_directory_setup_fails() {
    let fixture = FakeSandbox::new("sandbox-1");
    fixture
        .make_dir_results
        .lock()
        .push_back(Err(E2bSdkError::other("setup failed")));
    let sdk = FakeSdk::new();
    sdk.results
        .lock()
        .push_back(Ok(fixture.clone() as Arc<dyn E2bSandbox>));
    let (_ctx, runtime, fiber) = setup(
        sdk,
        serde_json::json!({ "apiKey": "test-key" }),
        env_lookup(),
    )
    .await;

    let error = runtime.get_sandbox().await.err().expect("setup failed");
    assert!(error.contains("setup failed"), "{error}");
    assert_eq!(*fixture.kills.lock(), 1);
    fiber.dispose().await;
    assert_eq!(*fixture.kills.lock(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preserves_the_setup_failure_after_its_one_rollback_attempt_fails() {
    let fixture = FakeSandbox::new("sandbox-1");
    fixture
        .run_results
        .lock()
        .push_back(Err(E2bSdkError::other("chmod failed")));
    fixture
        .kill_results
        .lock()
        .push_back(Err(E2bSdkError::other("cleanup failed")));
    let sdk = FakeSdk::new();
    sdk.results
        .lock()
        .push_back(Ok(fixture.clone() as Arc<dyn E2bSandbox>));
    let (_ctx, runtime, fiber) = setup(
        sdk,
        serde_json::json!({ "apiKey": "test-key" }),
        env_lookup(),
    )
    .await;

    let error = runtime.get_sandbox().await.err().expect("chmod failed");
    assert!(error.contains("chmod failed"), "{error}");
    assert_eq!(*fixture.kills.lock(), 1);

    fiber.dispose().await;
    assert_eq!(*fixture.kills.lock(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_a_reserved_runtime_root_that_is_not_a_real_directory() {
    for info in [
        E2bEntryInfo {
            file_type: FileType::Dir,
            symlink_target: Some("/tmp/redirected".to_string()),
            ..Default::default()
        },
        E2bEntryInfo {
            file_type: FileType::File,
            ..Default::default()
        },
    ] {
        let fixture = FakeSandbox::new("sandbox-1");
        fixture.get_info_results.lock().push_back(Ok(info));
        let sdk = FakeSdk::new();
        sdk.results
            .lock()
            .push_back(Ok(fixture.clone() as Arc<dyn E2bSandbox>));
        let (ctx, runtime, _fiber) = setup(
            sdk,
            serde_json::json!({ "apiKey": "test-key" }),
            env_lookup(),
        )
        .await;

        let error = runtime.get_sandbox().await.err().expect("runtime root");
        assert!(
            error.contains("runtime root must be a real directory"),
            "{error}"
        );
        assert!(fixture.runs.lock().is_empty());
        assert_eq!(*fixture.kills.lock(), 1);
        drop(ctx);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fails_self_contained_configuration_before_opening_e2b() {
    let cases: Vec<(serde_json::Value, &str)> = vec![
        (serde_json::json!({ "apiKey": "" }), "configure apiKey"),
        (
            serde_json::json!({ "apiKey": "x", "cwd": "relative" }),
            "absolute Linux path",
        ),
        (
            serde_json::json!({ "apiKey": "x", "timeoutMs": 0 }),
            "positive finite",
        ),
    ];
    for (config, message) in cases {
        let sdk = FakeSdk::new();
        let ctx = Context::root();
        let fiber = ctx.plugin(
            Arc::new(E2bPlugin::new(
                sdk.clone(),
                Arc::new(|_: &str| None) as Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
            )),
            arc(config),
        );
        let error = fiber.settle().await.expect_err("config rejected");
        assert!(error.message().contains(message), "{error}");
        assert!(sdk.creates.lock().is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requires_a_key_when_both_config_and_the_environment_omit_it() {
    let sdk = FakeSdk::new();
    let ctx = Context::root();
    let fiber = ctx.plugin(
        Arc::new(E2bPlugin::new(
            sdk.clone(),
            Arc::new(|_: &str| None) as Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
        )),
        arc(serde_json::json!({})),
    );
    let error = fiber.settle().await.expect_err("key required");
    assert!(error.message().contains("configure apiKey"), "{error}");
    assert!(sdk.creates.lock().is_empty());
}

// ---- invariant companion ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registers_the_package_owned_empty_invariant_installer() {
    let ctx = Context::root();
    dsh_invariants::InvariantRegistry::new(
        &ctx,
        dsh_invariants::InvariantConfig {
            enabled: true,
            ..Default::default()
        },
    );
    let fiber = ctx.plugin(Arc::new(dsh_e2b::E2bInvariantPlugin), arc(()));
    fiber.settle().await.expect("companion loads");
    fiber.dispose().await;
}
