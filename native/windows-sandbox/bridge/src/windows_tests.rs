use super::*;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use windows_sys::Win32::{Foundation::*, System::Threading::*};

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
