use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};
pub struct OwnedHost {
    child: Child,
    #[cfg(windows)]
    job: usize,
}
impl OwnedHost {
    fn new(mut child: Child) -> Result<Self, String> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if job.is_null()
                    || SetInformationJobObject(
                        job,
                        JobObjectExtendedLimitInformation,
                        &info as *const _ as *const _,
                        std::mem::size_of_val(&info) as u32,
                    ) == 0
                    || AssignProcessToJobObject(job, child.as_raw_handle()) == 0
                {
                    let err = std::io::Error::last_os_error().to_string();
                    if !job.is_null() {
                        CloseHandle(job);
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(err);
                }
                Ok(Self {
                    child,
                    job: job as usize,
                })
            }
        }
        #[cfg(not(windows))]
        {
            Ok(Self { child })
        }
    }
}
impl Drop for OwnedHost {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job as *mut _);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
pub fn connect(base: &str) -> Result<(String, Option<OwnedHost>), String> {
    if crate::model::rpc(base, "session.list", serde_json::json!({})).is_ok() {
        return Ok((base.into(), None));
    }
    if base != "http://127.0.0.1:58080" {
        return Err(format!("无法连接 {base}"));
    }
    let root = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("无法定位安装目录")?
        .to_path_buf();
    let exe = ["deepseek-harness-rs.exe", "dsh.exe"]
        .iter()
        .map(|n| root.join(n))
        .find(|p| p.is_file())
        .ok_or("未找到随附 Host；请启动现有 WebUI 服务")?;
    let mut cmd = Command::new(exe);
    cmd.args(["web", "--port", "0"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let mut owned = OwnedHost::new(cmd.spawn().map_err(|e| e.to_string())?)?;
    let stdout = owned.child.stdout.take().ok_or("Host 输出不可用")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Some(url) = line.strip_prefix("dsh web: ") {
                let _ = tx.send(url.to_string());
            }
        }
    });
    let url = rx
        .recv_timeout(Duration::from_secs(20))
        .map_err(|_| "Host 启动超时")?;
    crate::model::rpc(&url, "session.list", serde_json::json!({}))?;
    Ok((url, Some(owned)))
}
