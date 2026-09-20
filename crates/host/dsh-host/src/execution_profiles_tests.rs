use super::*;
use dsh_subprocess::{
    SubprocessAbort, SubprocessCollectedOutputs, SubprocessOutcome, SubprocessOutputRead,
    SubprocessOutputReader, SubprocessTerminalHandle, SubprocessTerminalSpawnSpec,
};

#[test]
fn powershell_launch_probe_reports_policy_without_changing_it() {
    let args = probe_args("shell", ShellKind::Powershell, "launch", None).unwrap();
    let command = args.last().unwrap();
    assert!(command.contains("Get-ExecutionPolicy -List"));
    assert!(command.contains("LanguageMode"));
    assert!(!command.contains("Set-ExecutionPolicy"));
    assert!(!args.iter().any(|arg| arg == "-ExecutionPolicy"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn environment_controls_resume_and_release_an_idle_persistent_session() {
    use dsh_host_apiproxy::{Body, CarrierRequest, to_fetch_handler};
    let fixture = Fixture::new(SandboxMode::WorkspaceWrite);
    let ctx = Context::root();
    let host = crate::compose_persistent_host_at(&ctx, &fixture.root.join("host"), None).unwrap();
    let service = ExecutionProfiles::install(
        &ctx.isolate("executionProfiles"),
        fixture.runtime.clone(),
        fixture.paths.clone(),
        fixture.service.host.clone(),
    );
    let response = to_fetch_handler(host.api_proxy.clone()).handle(CarrierRequest {
        method: http::Method::POST,
        path: "/api/session.create".into(),
        query: vec![],
        headers: vec![("content-type".into(), "application/json".into())],
        body: Some(json!({"type":"client-request","rpcId":"retired-environment","method":"session.create","payload":{"cwd":fixture.cwd()}}).to_string().into_bytes()),
    }).await;
    let value: Value = match response.into_body() {
        Body::Bytes(bytes) => serde_json::from_slice(&bytes).unwrap(),
        _ => panic!("expected unary session creation"),
    };
    assert_eq!(value["result"]["ok"], true, "{value}");
    let id = dsh_session::session_id(value["result"]["value"]["sessionId"].as_str().unwrap());
    drop(
        host.api_proxy
            .resolve_control_agent(id.as_str())
            .await
            .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while host.sessions.get(&id).is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("created session must retire before environment access");
    let snapshot = service
        .handle(json!({"action":"describe","sessionId":id,"cwd":fixture.cwd()}))
        .await
        .unwrap();
    assert_eq!(snapshot["revision"], 0);
    let saved = service.handle(json!({"action":"save","scope":"session","sessionId":id,"cwd":fixture.cwd(),"expectedRevision":0,"preferences":Preferences::default()})).await.unwrap();
    assert_eq!(saved["revision"], 1);
    tokio::time::timeout(Duration::from_secs(5), async {
        while host.sessions.get(&id).is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("environment lease must release the resumed session");
    host.shutdown().await.unwrap();
}

struct Reader;
#[test]
fn windows_app_aliases_are_distinct_from_real_packaged_executables() {
    assert!(is_app_execution_alias(Path::new(
        r"C:\Users\user\AppData\Local\Microsoft\WindowsApps\python.exe"
    )));
    assert!(!is_app_execution_alias(Path::new(
        r"C:\Program Files\WindowsApps\Microsoft.PowerShell_7\pwsh.exe"
    )));
    assert!(
        ensure_file(
            r"C:\Users\user\AppData\Local\Microsoft\WindowsApps\pwsh.exe".into(),
            "shell"
        )
        .unwrap_err()
        .contains("APP_EXECUTION_ALIAS_UNSUPPORTED")
    );
}

struct SlowSandbox {
    prepared: Arc<AtomicBool>,
}

struct CleanupSandbox;
impl SandboxProvider for CleanupSandbox {
    fn confine(
        &self,
        argv: &[String],
        _: &SandboxPolicy,
    ) -> Result<dsh_sandbox::ConfinedArgv, dsh_sandbox::SandboxUnavailableError> {
        let mut wrapped = vec![
            "fixture-runner".into(),
            "--ready-event".into(),
            "fixture".into(),
            "--".into(),
        ];
        wrapped.extend_from_slice(argv);
        Ok(dsh_sandbox::ConfinedArgv {
            argv: wrapped,
            enforcement: dsh_sandbox::SandboxEnforcement::Full,
            denial_signatures: vec![],
            runner_failure_rules: vec![],
            startup: Some(
                dsh_sandbox::SandboxStartup::new(|| Ok(true), || Ok(false))
                    .with_phase(|| Ok("cleanup".into())),
            ),
        })
    }
}

#[tokio::test]
async fn probe_command_budget_does_not_kill_runner_during_cleanup() {
    let f = Fixture::new(SandboxMode::WorkspaceWrite);
    f.service
        .ctx
        .register_service(Arc::new(CleanupSandbox) as Arc<dyn SandboxProvider>);
    f.runtime.delay_ms.store(20_000, Ordering::SeqCst);
    f.save().await;
    let value = f
        .service
        .inspect(
            "python",
            "launch",
            None,
            None,
            &f.cwd(),
            false,
            Arc::new(|| false),
        )
        .await
        .unwrap();
    assert_eq!(value["status"], "ready", "{value}");
    let args = f.runtime.args.lock();
    assert!(
        args[0]
            .windows(2)
            .any(|pair| pair == ["--command-timeout-ms", "15000"])
    );
}
impl SandboxProvider for SlowSandbox {
    fn prepare(&self, _: &SandboxExecutionPolicy) -> BoxFuture<'static, Result<(), String>> {
        let prepared = self.prepared.clone();
        Box::pin(async move {
            prepared.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
    fn confine(
        &self,
        argv: &[String],
        _: &SandboxPolicy,
    ) -> Result<dsh_sandbox::ConfinedArgv, dsh_sandbox::SandboxUnavailableError> {
        assert!(self.prepared.load(Ordering::SeqCst));
        let start = tokio::time::Instant::now();
        Ok(dsh_sandbox::ConfinedArgv {
            argv: argv.to_vec(),
            enforcement: dsh_sandbox::SandboxEnforcement::Full,
            denial_signatures: vec![],
            runner_failure_rules: vec![],
            startup: Some(dsh_sandbox::SandboxStartup::new(
                move || Ok(start.elapsed() >= Duration::from_secs(6)),
                || Ok(false),
            )),
        })
    }
}

#[tokio::test]
async fn sandbox_setup_longer_than_probe_budget_does_not_fail_runtime_validation() {
    let f = Fixture::new(SandboxMode::WorkspaceWrite);
    let prepared = Arc::new(AtomicBool::new(false));
    f.service.ctx.register_service(Arc::new(SlowSandbox {
        prepared: prepared.clone(),
    }) as Arc<dyn SandboxProvider>);
    f.runtime.delay_ms.store(6100, Ordering::SeqCst);
    f.save().await;
    let value = f
        .service
        .inspect(
            "python",
            "launch",
            None,
            None,
            &f.cwd(),
            false,
            Arc::new(|| false),
        )
        .await
        .unwrap();
    assert_eq!(value["status"], "ready", "{value}");
    assert!(prepared.load(Ordering::SeqCst));
    assert_eq!(f.runtime.spawns.load(Ordering::SeqCst), 1);
}
impl SubprocessOutputReader for Reader {
    fn read_from(&self, _: u64) -> SubprocessOutputRead {
        SubprocessOutputRead {
            text: "3.13.1\n".into(),
            next_offset: 7,
            lossy: false,
            spill_path: None,
        }
    }
}
struct Child(tokio::time::Instant);
impl SubprocessHandle for Child {
    fn stdin(&self) -> Option<Box<dyn tokio::io::AsyncWrite + Unpin + Send>> {
        None
    }
    fn stdout(&self) -> Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>> {
        None
    }
    fn stderr(&self) -> Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>> {
        None
    }
    fn collected(&self) -> SubprocessCollectedOutputs {
        SubprocessCollectedOutputs {
            stdout: Some(Arc::new(Reader)),
            stderr: None,
        }
    }
    fn done(&self) -> BoxFuture<'static, Result<SubprocessOutcome, String>> {
        let deadline = self.0;
        Box::pin(async move {
            tokio::time::sleep_until(deadline).await;
            Ok(SubprocessOutcome {
                exit_code: Some(0),
                signal: None,
            })
        })
    }
    fn terminate(&self) {}
    fn wait_for_exit(&self, _: Option<SubprocessAbort>) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
}
struct Runtime {
    delay_ms: AtomicUsize,
    path: String,
    spawns: AtomicUsize,
    args: Mutex<Vec<Vec<String>>>,
}
impl SubprocessRuntime for Runtime {
    fn resolve_executable(
        &self,
        _: &str,
        _: Option<&[(String, String)]>,
        _: Option<SubprocessAbort>,
    ) -> BoxFuture<'static, Result<String, String>> {
        let path = self.path.clone();
        Box::pin(async move { Ok(path) })
    }
    fn spawn(&self, spec: SubprocessSpawnSpec) -> Result<Arc<dyn SubprocessHandle>, String> {
        self.spawns.fetch_add(1, Ordering::SeqCst);
        self.args.lock().push(spec.argv);
        Ok(Arc::new(Child(
            tokio::time::Instant::now()
                + Duration::from_millis(self.delay_ms.load(Ordering::SeqCst) as u64),
        )))
    }
    fn spawn_terminal(
        &self,
        _: SubprocessTerminalSpawnSpec,
    ) -> BoxFuture<'static, Result<Arc<dyn SubprocessTerminalHandle>, String>> {
        Box::pin(async { Err("unused".into()) })
    }
}
struct Fixture {
    root: PathBuf,
    paths: Arc<RuntimePaths>,
    service: Arc<ExecutionProfiles>,
    runtime: Arc<Runtime>,
}
impl Fixture {
    fn new(mode: SandboxMode) -> Self {
        let root = std::env::temp_dir().join(format!("dsh-profiles-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let executable = root.join("python.exe");
        std::fs::write(&executable, "fixture").unwrap();
        let ctx = Context::root();
        dsh_session::SessionStore::install(&ctx);
        dsh_sandbox_policy::SandboxPolicyService::install(
            &ctx,
            dsh_sandbox_policy::Config {
                mode: Some(mode),
                workspace_root: Some(root.to_string_lossy().into_owned()),
            },
        );
        let paths = RuntimePaths::prepare_with_install_anchor(&root, None).unwrap();
        let runtime = Arc::new(Runtime {
            delay_ms: AtomicUsize::new(60),
            path: executable.to_string_lossy().into_owned(),
            spawns: AtomicUsize::new(0),
            args: Mutex::new(Vec::new()),
        });
        let host = EnvironmentCapabilities::new(runtime.clone(), paths.clone());
        let service = ExecutionProfiles::install(&ctx, runtime.clone(), paths.clone(), host);
        Self {
            root,
            paths,
            service,
            runtime,
        }
    }
    fn cwd(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }
    fn preferences(&self) -> Preferences {
        Preferences {
            python_path: Some(self.runtime.path.clone()),
            ..Default::default()
        }
    }
    async fn save(&self) -> Value {
        self.service
            .save("global", None, &self.cwd(), 0, Some(self.preferences()))
            .await
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.paths.release();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn profile_input_cannot_grant_permissions_or_run_startup_scripts() {
    assert!(
        serde_json::from_value::<Preferences>(json!({"permissionMode":"danger-full-access"}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<Preferences>(json!({"startupScript":"download | execute"}))
            .is_err()
    );
    assert!(
        validate_preferences(&Preferences {
            shell_path: Some("pwsh -Command code".into()),
            ..Default::default()
        })
        .is_err()
    );
}

#[tokio::test]
async fn cargo_is_a_selected_validated_capability_and_keeps_explicit_choice() {
    let f = Fixture::new(SandboxMode::DangerFullAccess);
    let mut preferences = f.preferences();
    preferences
        .toolchain_paths
        .insert("cargo".into(), f.runtime.path.clone());
    validate_preferences(&preferences).unwrap();
    f.service
        .save("global", None, &f.cwd(), 0, Some(preferences))
        .await
        .unwrap();
    let resolved = f.service.resolve(None, &f.cwd()).unwrap();
    assert_eq!(resolved.toolchain_paths.get("cargo"), Some(&f.runtime.path));
    assert_eq!(
        f.service.selected_path("cargo", None, &f.cwd()).unwrap(),
        Some(f.runtime.path.clone())
    );
    assert!(IDS.contains(&"cargo"));
    assert_eq!(
        probe_args("cargo", ShellKind::Powershell, "launch", None).unwrap(),
        vec!["--version"]
    );
}

#[tokio::test]
async fn preferences_persist_and_stale_revision_cannot_overwrite() {
    let f = Fixture::new(SandboxMode::DangerFullAccess);
    let saved = f.save().await;
    assert_eq!(saved["revision"], 1);
    assert_eq!(saved["permissionMode"], "danger-full-access");
    assert_eq!(saved["effective"]["pythonPath"], f.runtime.path);
    assert!(
        f.service
            .save("global", None, &f.cwd(), 0, None)
            .await
            .is_err()
    );
    let disk = read_bounded::<ProfileFile>(&f.service.profile_path).unwrap();
    assert_eq!(
        disk.profiles["global"].preferences.python_path.as_deref(),
        Some(f.runtime.path.as_str())
    );
    assert!(
        !std::fs::read_to_string(&f.service.profile_path)
            .unwrap()
            .contains("permissionMode")
    );
    let ctx = Context::root();
    let restored = ExecutionProfiles::install(
        &ctx,
        f.runtime.clone(),
        f.paths.clone(),
        f.service.host.clone(),
    );
    assert_eq!(
        restored.resolve(None, &f.cwd()).unwrap().python_path,
        Some(f.runtime.path.clone())
    );
}

#[tokio::test]
async fn project_and_session_overrides_have_exact_scope() {
    let f = Fixture::new(SandboxMode::DangerFullAccess);
    f.save().await;
    let project_python = f.root.join("project-python");
    std::fs::write(&project_python, "project").unwrap();
    let prefs = Preferences {
        python_path: Some(project_python.to_string_lossy().into_owned()),
        ..Default::default()
    };
    f.service
        .save("project", None, &f.cwd(), 1, Some(prefs.clone()))
        .await
        .unwrap();
    assert_eq!(
        f.service.resolve(None, &f.cwd()).unwrap().python_path,
        prefs.python_path
    );
    let other = f.root.join("other");
    std::fs::create_dir(&other).unwrap();
    assert_eq!(
        f.service
            .resolve(None, &other.to_string_lossy())
            .unwrap()
            .python_path,
        Some(f.runtime.path.clone())
    );
    f.service.state.lock().profiles.insert(
        "session:one".into(),
        Profile {
            revision: 3,
            preferences: f.preferences(),
        },
    );
    assert_eq!(
        f.service
            .resolve(Some("one"), &f.cwd())
            .unwrap()
            .python_path,
        Some(f.runtime.path.clone())
    );
    assert_eq!(
        f.service
            .resolve(Some("two"), &f.cwd())
            .unwrap()
            .python_path,
        prefs.python_path
    );
}

#[tokio::test]
async fn missing_pinned_program_stays_selected_and_blocks_without_fallback() {
    let f = Fixture::new(SandboxMode::DangerFullAccess);
    let missing = f.root.join("missing-python").to_string_lossy().into_owned();
    let value = f
        .service
        .save(
            "global",
            None,
            &f.cwd(),
            0,
            Some(Preferences {
                python_path: Some(missing.clone()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    assert_eq!(value["preferences"]["pythonPath"], missing);
    assert_eq!(value["effective"]["status"], "error");
    assert!(f.service.resolve(None, &f.cwd()).is_err());
    assert_eq!(f.runtime.spawns.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn host_readiness_does_not_prove_sandbox_readiness() {
    let f = Fixture::new(SandboxMode::WorkspaceWrite);
    f.save().await;
    let host = f
        .service
        .host
        .inspect("git", false, Arc::new(|| false))
        .await
        .unwrap();
    assert_eq!(host["status"], "ready");
    assert_eq!(host["executionWorld"], "host");
    let execution = f
        .service
        .inspect(
            "python",
            "launch",
            None,
            None,
            &f.cwd(),
            false,
            Arc::new(|| false),
        )
        .await
        .unwrap();
    assert_eq!(execution["status"], "unknown");
    assert_eq!(execution["executionWorld"], "selected_environment");
    assert_eq!(f.runtime.spawns.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn fixed_probe_cache_is_invalidated_by_binary_replacement_and_profile_revision() {
    let f = Fixture::new(SandboxMode::DangerFullAccess);
    f.save().await;
    let first = f
        .service
        .inspect(
            "python",
            "launch",
            None,
            None,
            &f.cwd(),
            false,
            Arc::new(|| false),
        )
        .await
        .unwrap();
    assert_eq!(first["status"], "ready");
    let hit = f
        .service
        .inspect(
            "python",
            "launch",
            None,
            None,
            &f.cwd(),
            false,
            Arc::new(|| false),
        )
        .await
        .unwrap();
    assert_eq!(hit["cacheHit"], true);
    std::fs::write(&f.runtime.path, "replaced binary").unwrap();
    let next = f
        .service
        .inspect(
            "python",
            "launch",
            None,
            None,
            &f.cwd(),
            false,
            Arc::new(|| false),
        )
        .await
        .unwrap();
    assert_eq!(next["cacheHit"], false);
    f.service
        .save("global", None, &f.cwd(), 1, Some(f.preferences()))
        .await
        .unwrap();
    let changed = f
        .service
        .inspect(
            "python",
            "launch",
            None,
            None,
            &f.cwd(),
            false,
            Arc::new(|| false),
        )
        .await
        .unwrap();
    assert_eq!(changed["cacheHit"], false);
    assert_eq!(f.runtime.spawns.load(Ordering::SeqCst), 3);
    assert!(
        f.runtime
            .args
            .lock()
            .iter()
            .all(|a| a[1..3] == ["-I", "-B"])
    );
}

#[tokio::test]
async fn one_cancelled_waiter_does_not_cancel_shared_probe() {
    let f = Fixture::new(SandboxMode::DangerFullAccess);
    f.save().await;
    let abort = Arc::new(AtomicBool::new(false));
    let flag = abort.clone();
    let cwd = f.cwd();
    let first = f.service.inspect(
        "python",
        "launch",
        None,
        None,
        &cwd,
        true,
        Arc::new(move || flag.load(Ordering::Acquire)),
    );
    let second = f.service.inspect(
        "python",
        "launch",
        None,
        None,
        &cwd,
        true,
        Arc::new(|| false),
    );
    let cancel = async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        abort.store(true, Ordering::Release);
    };
    let (a, b, _) = tokio::join!(first, second, cancel);
    assert!(a.is_err());
    assert_eq!(b.unwrap()["status"], "ready");
    assert_eq!(f.runtime.spawns.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn clearing_cache_fences_late_probe_results() {
    let f = Fixture::new(SandboxMode::DangerFullAccess);
    f.save().await;
    let cwd = f.cwd();
    let probe = f.service.inspect(
        "python",
        "launch",
        None,
        None,
        &cwd,
        false,
        Arc::new(|| false),
    );
    let clear = async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        f.service
            .handle(json!({"action":"clearCache","cwd":cwd}))
            .await
            .unwrap();
    };
    let (result, _) = tokio::join!(probe, clear);
    assert_eq!(
        result.unwrap()["invalidatedReason"],
        "cache_refreshed_during_probe"
    );
    assert!(f.service.checks.lock().is_empty());
}

#[tokio::test]
async fn unknown_or_script_like_dependency_never_launches_a_process() {
    let f = Fixture::new(SandboxMode::DangerFullAccess);
    f.save().await;
    assert!(
        f.service
            .inspect(
                "python",
                "dependency",
                Some("os; execute()"),
                None,
                &f.cwd(),
                false,
                Arc::new(|| false)
            )
            .await
            .is_err()
    );
    assert!(
        f.service
            .inspect(
                "wps",
                "launch",
                None,
                None,
                &f.cwd(),
                false,
                Arc::new(|| false)
            )
            .await
            .is_err()
    );
    assert_eq!(f.runtime.spawns.load(Ordering::SeqCst), 0);
}
