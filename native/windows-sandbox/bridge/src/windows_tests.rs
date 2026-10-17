use super::*;
use anyhow::Context;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use windows_sys::Win32::{Foundation::*, System::Threading::*};

/// Opt-in integration with real dedicated accounts and literal Node argv.
/// The fixture directory must be new; its state is retained for diagnostics.
#[test]
#[ignore = "requires installed native helpers and dedicated-account setup authority"]
fn cold_workspace_provisions_on_demand_while_other_sessions_execute() -> Result<()> {
    use std::path::{Path, PathBuf};
    let binary = PathBuf::from(std::env::var_os("DSH_COLD_NATIVE").context("DSH_COLD_NATIVE")?);
    let home = PathBuf::from(std::env::var_os("DSH_COLD_HOME").context("DSH_COLD_HOME")?);
    let root = PathBuf::from(std::env::var_os("DSH_COLD_FIXTURE").context("DSH_COLD_FIXTURE")?);
    let node = PathBuf::from(std::env::var_os("DSH_COLD_NODE").context("DSH_COLD_NODE")?);
    ensure!(!root.exists(), "cold fixture already exists");
    let a = root.join("工作区 A");
    let b = root.join("工作区 B");
    let private = root.join("private");
    std::fs::create_dir_all(&a)?;
    std::fs::create_dir_all(&b)?;
    std::fs::create_dir_all(&private)?;
    let one = private.join("one.txt");
    let two = private.join("two.txt");
    std::fs::write(&one, "one")?;
    std::fs::write(&two, "two")?;
    let command = |workspace: &Path, session: &str, read: &Path| {
        let mut command = Command::new(&binary);
        command.arg("--native-home").arg(&home)
            .arg("--workspace").arg(workspace)
            .args(["--implementation", "elevated", "--network", "enabled", "--mode", "workspace-write"])
            .arg("--session-id").arg(session)
            .arg("--private-root").arg(&private)
            .arg("--read-root").arg(read)
            .creation_flags(0x08000000).stdin(Stdio::null());
        command
    };
    let count = |workspace: &Path| (crate::pool::READ_SLOTS..crate::pool::SIZE)
        .filter(|index| codex_windows_sandbox::sandbox_setup_is_complete(&crate::pool::home(&home, workspace, *index))).count();
    let began = std::time::Instant::now();
    let output = command(&a, "cold-a", &one).arg("--setup").output()?;
    ensure!(output.status.success(), "setup: {}", String::from_utf8_lossy(&output.stderr));
    ensure!(count(&a) == 1 && count(&b) == 0, "first command eagerly provisioned extra accounts");
    let setup_seconds = began.elapsed().as_secs_f64();
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
    }
    let launch = |workspace: &Path, session: &str, own: &Path, other: &Path| -> Result<Child> {
        let script = r#"const fs=require('fs');const check=()=>{
            if(!fs.readFileSync(process.argv[1],'utf8')) throw Error('own read failed');
            let denied=false;try{fs.readFileSync(process.argv[2])}catch(e){if(e.code==='EACCES'||e.code==='EPERM')denied=true;else throw e}
            if(!denied)throw Error('foreign private file readable');
            denied=false;try{fs.writeFileSync(process.argv[5],'forbidden')}catch(e){if(e.code==='EACCES'||e.code==='EPERM')denied=true;else throw e}
            if(!denied)throw Error('foreign workspace writable');};check();
            const ready=process.argv[3],release=process.argv[4];
            const user=require('child_process').execFileSync(require('path').join(process.env.SYSTEMROOT,'System32','whoami.exe'),['/user','/fo','csv','/nh'],{encoding:'utf8'}).trim();
            fs.writeFileSync(ready+'.tmp',JSON.stringify({pid:process.pid,user}));fs.renameSync(ready+'.tmp',ready);
            const timer=setInterval(()=>{if(fs.existsSync(release)){check();clearInterval(timer);console.log('ISOLATED_OK')}},50);"#;
        let log = std::fs::File::create(root.join(format!("{session}.log")))?;
        let foreign_workspace = if workspace == a.as_path() { &b } else { &a };
        let child = command(workspace, session, own)
            .args(["--command-timeout-ms", "600000", "--"]).arg(&node).arg("-e").arg(script)
            .arg(own).arg(other).arg(workspace.join(format!("{session}.ready")))
            .arg(workspace.join("release"))
            .arg(foreign_workspace.join(format!("{session}.forbidden")))
            .stdout(log.try_clone()?).stderr(log).spawn()?;
        Ok(Child(child))
    };
    let wait_ready = |child: &mut Child, path: &Path| -> Result<serde_json::Value> {
        let start = std::time::Instant::now();
        loop {
            if path.exists() { return Ok(serde_json::from_slice(&std::fs::read(path)?)?); }
            ensure!(child.0.try_wait()?.is_none(), "native session exited before readiness; inspect fixture logs");
            ensure!(start.elapsed() < Duration::from_secs(600), "native session never became ready");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let mut first = launch(&a, "cold-a", &one, &two)?;
    let first_identity = wait_ready(&mut first, &a.join("cold-a.ready"))?;
    // A different cold workspace must initialize without acquiring first's lease.
    let output = command(&b, "cold-b", &two).arg("--setup").output()?;
    ensure!(output.status.success(), "cross-workspace setup: {}", String::from_utf8_lossy(&output.stderr));
    ensure!(count(&a) == 1 && count(&b) == 1 && first.0.try_wait()?.is_none());
    let mut third = launch(&b, "cold-b", &two, &one)?;
    let third_identity = wait_ready(&mut third, &b.join("cold-b.ready"))?;
    // The occupied warm account forces a second same-project account to be
    // initialized by the execution path, while both previous commands remain live.
    let mut second = launch(&a, "cold-a2", &two, &one)?;
    let second_identity = wait_ready(&mut second, &a.join("cold-a2.ready"))?;
    ensure!(count(&a) == 2 && count(&b) == 1);
    ensure!(first.0.try_wait()?.is_none() && third.0.try_wait()?.is_none());
    ensure!(first_identity["user"] != second_identity["user"] && second_identity["user"] != third_identity["user"]);
    std::fs::write(a.join("release"), "done")?;
    std::fs::write(b.join("release"), "done")?;
    for child in [&mut first, &mut second, &mut third] {
        let start = std::time::Instant::now();
        loop {
            if let Some(status) = child.0.try_wait()? { ensure!(status.success(), "session failed after release"); break; }
            ensure!(start.elapsed() < Duration::from_secs(30), "session did not retire");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    println!("{}", serde_json::json!({"coldSetupSeconds":setup_seconds,"concurrentAccounts":3,"workspaceSlots":[2,1],"privateReadIsolation":true,"fixture":root}));
    Ok(())
}

#[test]
fn killing_preparation_bridge_also_stops_its_setup_descendant() -> Result<()> {
    const MARKER: &str = "DSH_TEST_PREPARATION_PID_FILE";
    if let Some(path) = std::env::var_os(MARKER) {
        contain_helper_tree()?;
        let powershell = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut child = Command::new(powershell)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 60",
            ])
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        std::fs::write(path, child.id().to_string())?;
        child.wait()?;
        return Ok(());
    }
    let marker = std::env::temp_dir().join(format!(
        "dsh-preparation-pid-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let mut bridge = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "windows::tests::killing_preparation_bridge_also_stops_its_setup_descendant",
            "--nocapture",
        ])
        .env(MARKER, &marker)
        .creation_flags(0x08000000)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let began = std::time::Instant::now();
    let pid: u32 = loop {
        if let Ok(pid) =
            std::fs::read_to_string(&marker).and_then(|s| s.parse().map_err(std::io::Error::other))
        {
            break pid;
        }
        if began.elapsed() > Duration::from_secs(20) {
            let _ = bridge.kill();
            let _ = bridge.wait();
            anyhow::bail!("preparation child did not publish its helper PID");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    // Retain the process handle, so PID reuse cannot turn this into a check of
    // an unrelated process and no unrelated process is ever terminated.
    let helper = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZATION_SYNCHRONIZE, 0, pid) };
    ensure!(
        helper != 0,
        "open fixture helper: {}",
        std::io::Error::last_os_error()
    );
    bridge.kill()?;
    bridge.wait()?;
    let status = unsafe { WaitForSingleObject(helper, 5000) };
    unsafe {
        CloseHandle(helper);
    }
    std::fs::remove_file(marker)?;
    ensure!(
        status == WAIT_OBJECT_0,
        "orphaned preparation helper survived cancellation"
    );
    Ok(())
}
