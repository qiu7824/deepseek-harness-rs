use crate::{
    args::{Action, Request},
    environment,
};
use anyhow::{Result, ensure};
use codex_protocol::{
    config_types::WindowsSandboxLevel, models::PermissionProfile, permissions::NetworkSandboxPolicy,
};
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_windows_sandbox::{
    ResolvedWindowsSandboxPermissions, SandboxSetupRequest, SetupRootOverrides,
    WindowsSandboxProxySettingsMode, WindowsSandboxSessionRequest,
};
use std::{sync::Arc, time::Duration};

pub fn run(request: Request) -> Result<i32> {
    use crate::pool;
    ensure!(
        request.action != Action::Run || request.network,
        "[NETWORK_POLICY_UNAVAILABLE] offline network enforcement has not passed runtime validation; command not dispatched"
    );
    let owned = pool::validate_owner(&request.home, request.action == Action::Setup)?;
    let indices = if request.read_only {
        0..pool::READ_SLOTS
    } else {
        pool::READ_SLOTS..pool::SIZE
    };
    if request.action == Action::Status {
        let initialized = owned
            && indices.clone().all(|index| {
                codex_windows_sandbox::sandbox_setup_is_complete(&pool::home(
                    &request.home,
                    &request.workspace,
                    index,
                ))
            });
        println!(
            "{}",
            serde_json::json!({"backend":"windows-native","protocolVersion":1,"initialized":initialized,"home":request.home,"poolSize":pool::SIZE,"offlineNetworkValidated":false,"networkModes":["enabled"]})
        );
        return Ok(0);
    }
    ensure!(
        owned,
        "SETUP_REQUIRED: native account pool is not initialized"
    );
    if request.action == Action::Setup {
        ensure!(
            !codex_windows_sandbox::setup_caller_is_restricted()?,
            "SETUP_DENIED: a confined command cannot initialize sandbox identities"
        );
        let mut leases = Vec::new();
        leases.extend(pool::acquire_other_projects(
            &request.home,
            &request.workspace,
        )?);
        for index in 0..pool::SIZE {
            leases.push(
                pool::acquire(&request.home, &request.workspace, index)?.ok_or_else(|| {
                    anyhow::anyhow!("NATIVE_SLOT_BUSY: stop native commands before setup")
                })?,
            );
        }
        pool::protect_owner(&request.home)?;
        for index in 0..pool::SIZE {
            let mut slot = request.clone();
            slot.home = pool::home(&request.home, &request.workspace, index);
            slot.read_only = index < pool::READ_SLOTS;
            run_slot(slot)?;
        }
        println!(
            "{}",
            serde_json::json!({"backend":"windows-native","protocolVersion":1,"initialized":true,"poolSize":pool::SIZE,"home":request.home})
        );
        return Ok(0);
    }
    let mut failures = Vec::new();
    ensure!(
        indices.clone().all(
            |index| codex_windows_sandbox::sandbox_setup_is_complete(&pool::home(
                &request.home,
                &request.workspace,
                index
            ))
        ),
        "SETUP_REQUIRED: initialize this workspace before native execution"
    );
    for index in indices {
        match pool::acquire(&request.home, &request.workspace, index) {
            Ok(Some(_lease)) => {
                let mut slot = request.clone();
                slot.home = pool::home(&request.home, &request.workspace, index);
                let home = slot.home.clone();
                let result = run_slot(slot);
                pool::settle(&home);
                return result;
            }
            Ok(None) => {}
            Err(error) => failures.push(error.to_string()),
        }
    }
    anyhow::bail!(
        "NATIVE_SLOT_BUSY: all isolated account slots are occupied or quarantined: {}",
        failures.join("; ")
    )
}

fn run_slot(mut request: Request) -> Result<i32> {
    if request.home.exists() {
        request.home = codex_windows_sandbox::canonicalize_path(&request.home);
    }
    codex_windows_sandbox::assert_state_namespace(&request.home)?;
    ensure!(
        request.action != Action::Run || request.network,
        "[NETWORK_POLICY_UNAVAILABLE] offline network enforcement has not passed runtime validation on this platform; command not dispatched"
    );
    let engine = std::env::current_exe()?
        .parent()
        .ok_or_else(|| anyhow::anyhow!("missing binary directory"))?
        .to_path_buf();
    for name in ["dsh-command-runner.exe", "dsh-windows-sandbox-setup.exe"] {
        ensure!(
            engine.join(name).is_file(),
            "[SETUP_REQUIRED] missing {name}"
        );
    }
    if request.action == Action::Status {
        println!(
            "{}",
            serde_json::json!({"backend":"windows-native","protocolVersion":1,"initialized":codex_windows_sandbox::sandbox_setup_is_complete(&request.home),"home":request.home,"networkModes":["enabled"],"offlineNetworkValidated":false})
        );
        return Ok(0);
    }
    if request.action == Action::Run {
        ensure!(
            codex_windows_sandbox::sandbox_setup_is_complete(&request.home),
            "[SETUP_REQUIRED] run explicit native sandbox setup before dispatch"
        );
    } else {
        std::fs::create_dir_all(&request.home)?;
        request.home = codex_windows_sandbox::canonicalize_path(&request.home);
        std::fs::write(
            request.home.join("dsh-native-owner.json"),
            b"{\"product\":\"deepseek-harness-rs\",\"version\":1}",
        )?;
    }
    let env = environment::prepare(&request)?;
    crate::toolchain::restore_broker_folders(std::path::Path::new(
        env.values
            .get("TEMP")
            .ok_or_else(|| anyhow::anyhow!("missing private temporary directory"))?,
    ))?;
    let workspace = AbsolutePathBuf::try_from(request.workspace.clone())?;
    let workspace_roots = vec![workspace];
    let mut policy = if request.read_only {
        PermissionProfile::read_only()
    } else {
        PermissionProfile::workspace_write_with(&[], NetworkSandboxPolicy::Enabled, true, true)
    };
    if let PermissionProfile::Managed { network, .. } = &mut policy {
        *network = if request.network {
            NetworkSandboxPolicy::Enabled
        } else {
            NetworkSandboxPolicy::Restricted
        };
    }
    let mut writes = env.writes;
    if !request.read_only {
        writes.push(request.workspace.clone());
    }
    let denied: Vec<_> = env
        .denied
        .iter()
        .filter(|p| p.exists())
        .cloned()
        .map(AbsolutePathBuf::try_from)
        .collect::<std::result::Result<_, _>>()?;
    if request.action == Action::Setup {
        let permissions =
            ResolvedWindowsSandboxPermissions::try_from_permission_profile_for_workspace_roots(
                &policy,
                &workspace_roots,
            )?;
        codex_windows_sandbox::run_elevated_setup(
            SandboxSetupRequest {
                permissions: &permissions,
                command_cwd: &request.workspace,
                env_map: &env.values,
                codex_home: &request.home,
                proxy_enforced: false,
            },
            SetupRootOverrides {
                read_roots: Some(env.reads),
                read_roots_include_platform_defaults: false,
                write_roots: Some(writes),
                deny_read_paths: Some(env.denied.into_iter().filter(|p| p.exists()).collect()),
                deny_write_paths: None,
            },
        )?;
        ensure!(
            codex_windows_sandbox::sandbox_setup_is_complete(&request.home),
            "setup did not publish a ready identity"
        );
        return Ok(0);
    }
    ensure!(
        codex_windows_sandbox::sandbox_setup_is_complete(&request.home),
        "[SETUP_REQUIRED] run explicit native sandbox setup before dispatch"
    );
    let cwd = std::env::current_dir()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        let tty = request.tty || console_size().is_some();
        let spawned = codex_windows_sandbox::spawn_windows_sandbox_session_for_level(
            WindowsSandboxSessionRequest {
                permission_profile: &policy,
                workspace_roots: &workspace_roots,
                codex_home: &request.home,
                command: env.command,
                cwd: &cwd,
                env_map: env.values,
                windows_sandbox_level: WindowsSandboxLevel::Elevated,
                proxy_enforced: false,
                network_proxy_restricting_sid: None,
                proxy_settings_mode: WindowsSandboxProxySettingsMode::Reconcile,
                timeout_ms: request.timeout_ms,
                read_roots_override: Some(&env.reads),
                read_roots_include_platform_defaults: false,
                write_roots_override: Some(&writes),
                deny_read_paths_override: &denied,
                deny_write_paths_override: &[],
                tty,
                stdin_open: true,
                use_private_desktop: true,
            },
        )
        .await?;
        signal_event(request.ready_event.as_deref())?;
        let code = forward(spawned, tty).await;
        if code == 124 {
            if let Some(name) = &request.ready_event {
                let _ = signal_event(Some(&format!("{name}-timeout")));
            }
        }
        Ok(code)
    });
    runtime.shutdown_timeout(Duration::from_secs(1));
    result
}

fn signal_event(name: Option<&str>) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent},
    };
    if let Some(name) = name {
        let wide: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
        let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, wide.as_ptr()) };
        ensure!(event != 0, "cannot open runner readiness event");
        let ok = unsafe { SetEvent(event) };
        unsafe { CloseHandle(event) };
        ensure!(ok != 0, "cannot signal runner readiness");
    }
    Ok(())
}

fn console_size() -> Option<codex_utils_pty::TerminalSize> {
    use windows_sys::Win32::System::Console::{
        CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetStdHandle, STD_OUTPUT_HANDLE,
    };
    let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
    let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if unsafe { GetConsoleScreenBufferInfo(handle, &mut info) } == 0 {
        return None;
    }
    Some(codex_utils_pty::TerminalSize {
        rows: (info.srWindow.Bottom - info.srWindow.Top + 1).max(1) as u16,
        cols: (info.srWindow.Right - info.srWindow.Left + 1).max(1) as u16,
    })
}

async fn forward(spawned: codex_utils_pty::SpawnedProcess, tty: bool) -> i32 {
    use std::io::{Read, Write};
    let session = Arc::new(spawned.session);
    let input = session.writer_sender();
    let (eof_tx, eof_rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        let mut stdin = std::io::stdin();
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if input.blocking_send(buffer[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = eof_tx.send(());
    });
    let closer = tokio::spawn({
        let session = session.clone();
        async move {
            let _ = eof_rx.await;
            session.close_stdin();
        }
    });
    let (stdout_done, stdout) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let mut rx = spawned.stdout_rx;
        let mut out = std::io::stdout();
        while let Some(bytes) = rx.blocking_recv() {
            if out.write_all(&bytes).and_then(|_| out.flush()).is_err() {
                break;
            }
        }
        let _ = stdout_done.send(());
    });
    let (stderr_done, stderr) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let mut rx = spawned.stderr_rx;
        let mut out = std::io::stderr();
        while let Some(bytes) = rx.blocking_recv() {
            if out.write_all(&bytes).and_then(|_| out.flush()).is_err() {
                break;
            }
        }
        let _ = stderr_done.send(());
    });
    let resizer = tty.then(|| {
        tokio::spawn({
            let session = session.clone();
            async move {
                loop {
                    if let Some(size) = console_size() {
                        let _ = session.resize(size);
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        })
    });
    let mut exit = spawned.exit_rx;
    let code = tokio::select! {code=&mut exit=>code.unwrap_or(125),_=tokio::signal::ctrl_c()=>{session.request_terminate();exit.await.unwrap_or(130)}};
    closer.abort();
    if let Some(task) = resizer {
        task.abort();
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        let _ = stdout.await;
        let _ = stderr.await;
    })
    .await;
    code
}
