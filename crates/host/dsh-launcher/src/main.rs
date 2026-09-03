#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::fs;
use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::fd::AsRawFd;

#[cfg(windows)]
use std::{
    ffi::OsStr,
    os::windows::{ffi::OsStrExt, process::CommandExt},
    ptr,
};

#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;

use serde::{Deserialize, Serialize};
use zsui::stable::Dp;
use zsui::{
    AppCx, ColorRole, Command as ZsuiCommand, HorizontalAlign, NativeWindowBuilder,
    SemanticTextStyle, TextRole, TextWeight, TextWrap, ThemeColorToken, TraySpec,
    UiInvalidationHandle, VerticalAlign, ViewNode, ZsIcon, ZsIconSize, ZsuiThemeMode, button,
    column, icon, primary_button, row, section, styled_text, toggle,
};
use zsui::{MenuItemSpec, MenuSpec};

#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, FILETIME, GetLastError, HANDLE},
    System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey,
        RegCreateKeyW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    },
    System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    },
    UI::WindowsAndMessaging::{FindWindowW, SW_RESTORE, SetForegroundWindow, ShowWindow},
};

const DEFAULT_PORT: u16 = 58080;
const ADDRESS: &str = "http://127.0.0.1:58080/";
const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");
const UPDATE_RELEASES_API: &str =
    "https://api.github.com/repos/qiu7824/deepseek-harness-rs/releases?per_page=20";
const UPDATE_RELEASES_URL: &str = "https://github.com/qiu7824/deepseek-harness-rs/releases";
const LAUNCHER_ICON_FILE: &str = "deepseek-black.ico";
const SINGLE_INSTANCE_MUTEX_NAME: &str = "Local\\DeepSeekHarnessRsLauncher";
const AUTOSTART_REGISTRY_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const AUTOSTART_VALUE_NAME: &str = "DeepSeek Harness-rs Launcher";

#[cfg(windows)]
unsafe extern "system" {
    fn CreateMutexW(
        mutex_attributes: *const core::ffi::c_void,
        initial_owner: i32,
        name: *const u16,
    ) -> HANDLE;
}
const LAUNCHER_CONTENT_PADDING: Dp = Dp::new(24.0);
const LAUNCHER_SECTION_GAP: Dp = Dp::new(16.0);
const LAUNCHER_ACTION_GAP: Dp = Dp::new(8.0);

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LauncherAction {
    Start,
    Stop,
    Restart,
    OpenHarness,
    Autostart,
    CheckUpdate,
    Refresh,
}

#[cfg(test)]
const PRIMARY_ACTIONS: [LauncherAction; 3] = [
    LauncherAction::Start,
    LauncherAction::Stop,
    LauncherAction::Restart,
];
#[cfg(test)]
const SECONDARY_ACTIONS: [LauncherAction; 4] = [
    LauncherAction::OpenHarness,
    LauncherAction::Autostart,
    LauncherAction::CheckUpdate,
    LauncherAction::Refresh,
];

const LAUNCHER_STATE_VERSION: u32 = 1;
const LAUNCHER_STATE_FILE: &str = "dsh-launcher-state.json";
const TRAY_OPEN_PANEL_COMMAND: &str = "launcher.open-panel";
const TRAY_OPEN_HARNESS_COMMAND: &str = "launcher.open-harness";
const TRAY_START_COMMAND: &str = "launcher.start";
const TRAY_STOP_COMMAND: &str = "launcher.stop";
const TRAY_RESTART_COMMAND: &str = "launcher.restart";
const TRAY_AUTOSTART_COMMAND: &str = "launcher.autostart";
const TRAY_CHECK_UPDATE_COMMAND: &str = "launcher.check-update";
const TRAY_QUIT_COMMAND: &str = "launcher.quit";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceOwnership {
    Stopped,
    ManagedRunning,
    ForeignPort,
}

fn tray_menu_spec(copy: Copy, ownership: ServiceOwnership, autostart: bool) -> MenuSpec {
    let managed = ownership == ServiceOwnership::ManagedRunning;
    let stopped = ownership == ServiceOwnership::Stopped;
    MenuSpec {
        id: Some("launcher-tray".to_string()),
        title: None,
        items: vec![
            tray_command(
                TRAY_OPEN_PANEL_COMMAND,
                copy.open_panel,
                ZsuiCommand::ShowMainWindow,
                true,
            ),
            tray_command(
                TRAY_OPEN_HARNESS_COMMAND,
                copy.open_web,
                ZsuiCommand::custom(TRAY_OPEN_HARNESS_COMMAND),
                true,
            ),
            MenuItemSpec::Separator,
            tray_command(
                TRAY_START_COMMAND,
                copy.start,
                ZsuiCommand::custom(TRAY_START_COMMAND),
                stopped,
            ),
            tray_command(
                TRAY_STOP_COMMAND,
                copy.stop,
                ZsuiCommand::custom(TRAY_STOP_COMMAND),
                managed,
            ),
            tray_command(
                TRAY_RESTART_COMMAND,
                copy.restart,
                ZsuiCommand::custom(TRAY_RESTART_COMMAND),
                managed,
            ),
            MenuItemSpec::Separator,
            tray_command_checked(
                TRAY_AUTOSTART_COMMAND,
                copy.autostart,
                ZsuiCommand::custom(TRAY_AUTOSTART_COMMAND),
                true,
                autostart,
            ),
            tray_command(
                TRAY_CHECK_UPDATE_COMMAND,
                copy.check_update,
                ZsuiCommand::custom(TRAY_CHECK_UPDATE_COMMAND),
                true,
            ),
            MenuItemSpec::Separator,
            tray_command(TRAY_QUIT_COMMAND, copy.quit, ZsuiCommand::Quit, true),
        ],
    }
}

fn tray_command(id: &str, label: &str, command: ZsuiCommand, enabled: bool) -> MenuItemSpec {
    tray_command_checked(id, label, command, enabled, false)
}

fn tray_command_checked(
    id: &str,
    label: &str,
    command: ZsuiCommand,
    enabled: bool,
    checked: bool,
) -> MenuItemSpec {
    MenuItemSpec::Command {
        id: Some(id.to_string()),
        label: label.to_string(),
        command,
        enabled,
        checked,
        accelerator: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessIdentity {
    pid: u32,
    creation_time: u64,
    executable: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LauncherStateFile {
    version: u32,
    pid: u32,
    creation_time: u64,
    executable: PathBuf,
    home: PathBuf,
    port: u16,
    started_at_ms: u64,
}

impl LauncherStateFile {
    fn owned(pid: u32, creation_time: u64, executable: PathBuf, home: PathBuf, port: u16) -> Self {
        Self {
            version: LAUNCHER_STATE_VERSION,
            pid,
            creation_time,
            executable,
            home,
            port,
            started_at_ms: now_unix_millis(),
        }
    }

    fn matches_process(&self, process: &ProcessIdentity) -> bool {
        self.version == LAUNCHER_STATE_VERSION
            && self.pid == process.pid
            && self.creation_time == process.creation_time
            && same_executable(&self.executable, &process.executable)
    }
}

fn launcher_state_path(root: &Path) -> PathBuf {
    launcher_state_path_in_runtime_root(&launcher_runtime_root(root))
}

fn launcher_state_path_in_runtime_root(runtime_root: &Path) -> PathBuf {
    runtime_root.join("run").join(LAUNCHER_STATE_FILE)
}

fn launcher_runtime_root(root: &Path) -> PathBuf {
    let home = active_home(root);
    if home == root {
        root.to_path_buf()
    } else {
        home.join("launcher")
    }
}

fn launcher_log_dir(root: &Path) -> PathBuf {
    launcher_runtime_root(root).join("logs")
}

fn read_launcher_state(root: &Path) -> io::Result<Option<LauncherStateFile>> {
    read_launcher_state_at(&launcher_state_path(root))
}

fn read_launcher_state_at(path: &Path) -> io::Result<Option<LauncherStateFile>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_launcher_state(root: &Path, state: &LauncherStateFile) -> io::Result<()> {
    write_launcher_state_at(&launcher_state_path(root), state)
}

fn write_launcher_state_at(path: &Path, state: &LauncherStateFile) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("launcher state path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".{LAUNCHER_STATE_FILE}.tmp-{}", std::process::id()));
    let bytes = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
    fs::write(&temp, bytes)?;
    if let Err(error) = fs::rename(&temp, path) {
        if error.kind() == io::ErrorKind::AlreadyExists || path.exists() {
            fs::remove_file(path)?;
            fs::rename(&temp, path)?;
        } else {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
    }
    Ok(())
}

fn remove_launcher_state(root: &Path) -> io::Result<()> {
    match fs::remove_file(launcher_state_path(root)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn same_executable(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

fn inspect_process(pid: u32) -> io::Result<ProcessIdentity> {
    inspect_process_platform(pid)
}

fn wait_for_process_identity(pid: u32, expected_executable: &Path) -> io::Result<ProcessIdentity> {
    let mut last_error = None;
    for _ in 0..20 {
        match inspect_process(pid) {
            Ok(identity) if same_executable(&identity.executable, expected_executable) => {
                return Ok(identity);
            }
            Ok(identity) => {
                last_error = Some(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "started process executable mismatch: expected {}, observed {}",
                        expected_executable.display(),
                        identity.executable.display()
                    ),
                ));
            }
            Err(error) => last_error = Some(error),
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Err(last_error.unwrap_or_else(|| io::Error::other("process identity unavailable")))
}

fn active_home(_root: &Path) -> PathBuf {
    std::env::var_os("DSH_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(dsh_home_paths::default_dsh_home)
}

fn stop_process(identity: &ProcessIdentity) -> io::Result<()> {
    let observed = inspect_process(identity.pid)?;
    if observed != *identity {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "owned process {} identity changed before stop",
                identity.pid
            ),
        ));
    }
    stop_process_platform(identity)
}

fn wait_for_process_exit(identity: &ProcessIdentity) -> io::Result<()> {
    for _ in 0..100 {
        match inspect_process(identity.pid) {
            Ok(observed) if observed == *identity => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(_) | Err(_) => return Ok(()),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("owned process {} did not exit", identity.pid),
    ))
}

#[cfg(windows)]
fn stop_process_platform(identity: &ProcessIdentity) -> io::Result<()> {
    let status = Command::new("taskkill.exe")
        .args(["/PID", &identity.pid.to_string(), "/T", "/F"])
        .creation_flags(0x08000000)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "taskkill failed for owned process {}",
            identity.pid
        )))
    }
}

#[cfg(target_os = "linux")]
struct LinuxPidFd(i32);

#[cfg(target_os = "linux")]
impl Drop for LinuxPidFd {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_pidfd_open(pid: u32) -> io::Result<LinuxPidFd> {
    let pid = i32::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "pid exceeds i32"))?;
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(LinuxPidFd(fd as i32))
    }
}

#[cfg(target_os = "linux")]
fn linux_pidfd_send_signal(pidfd: &LinuxPidFd, signal: i32) -> io::Result<()> {
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.0,
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn stop_process_platform(identity: &ProcessIdentity) -> io::Result<()> {
    let pidfd = linux_pidfd_open(identity.pid)?;
    let observed = inspect_process(identity.pid)?;
    if observed != *identity {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "owned process {} identity changed before pidfd stop",
                identity.pid
            ),
        ));
    }
    linux_pidfd_send_signal(&pidfd, libc::SIGTERM)
}

#[cfg(target_os = "macos")]
fn stop_process_platform(identity: &ProcessIdentity) -> io::Result<()> {
    let pid = i32::try_from(identity.pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "pid exceeds i32"))?;
    let status = unsafe { libc::kill(pid, libc::SIGTERM) };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn inspect_process_platform(pid: u32) -> io::Result<ProcessIdentity> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    let result = (|| {
        let mut creation = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exit = creation;
        let mut kernel = creation;
        let mut user = creation;
        if unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut path = vec![0_u16; 32_768];
        let mut length = path.len() as u32;
        if unsafe { QueryFullProcessImageNameW(handle, 0, path.as_mut_ptr(), &mut length) } == 0 {
            return Err(io::Error::last_os_error());
        }
        path.truncate(length as usize);
        Ok(ProcessIdentity {
            pid,
            creation_time: (u64::from(creation.dwHighDateTime) << 32)
                | u64::from(creation.dwLowDateTime),
            executable: PathBuf::from(String::from_utf16_lossy(&path)),
        })
    })();
    unsafe {
        CloseHandle(handle);
    }
    result
}

#[cfg(target_os = "linux")]
fn inspect_process_platform(pid: u32) -> io::Result<ProcessIdentity> {
    let executable = fs::read_link(format!("/proc/{pid}/exe"))?;
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let creation_time = linux_process_start_time(&stat)?;
    Ok(ProcessIdentity {
        pid,
        creation_time,
        executable,
    })
}

#[cfg(any(target_os = "linux", test))]
fn linux_process_start_time(stat: &str) -> io::Result<u64> {
    let after_command = stat
        .rfind(") ")
        .map(|index| &stat[index + 2..])
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid Linux process stat"))?;
    after_command
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "missing Linux process start time",
            )
        })?
        .parse::<u64>()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(target_os = "macos")]
fn inspect_process_platform(pid: u32) -> io::Result<ProcessIdentity> {
    let pid = i32::try_from(pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "PID does not fit pid_t"))?;
    let status = unsafe { libc::kill(pid, 0) };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    let executable = process_executable_macos(pid)?;
    let creation_time = process_creation_time_macos(pid)?;
    Ok(ProcessIdentity {
        pid: pid as u32,
        creation_time,
        executable,
    })
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct ProcBsdInfo {
    pbi_flags: u32,
    pbi_status: u32,
    pbi_xstatus: u32,
    pbi_pid: u32,
    pbi_ppid: u32,
    pbi_uid: u32,
    pbi_gid: u32,
    pbi_ruid: u32,
    pbi_rgid: u32,
    pbi_svuid: u32,
    pbi_svgid: u32,
    rfu_1: u32,
    pbi_comm: [i8; 16],
    pbi_name: [i8; 32],
    pbi_nfiles: u32,
    pbi_pgid: u32,
    pbi_pjobc: u32,
    e_tdev: u32,
    e_tpgid: u32,
    pbi_nice: i32,
    pbi_start_tvsec: u64,
    pbi_start_tvusec: u64,
}

#[cfg(target_os = "macos")]
fn process_creation_time_macos(pid: i32) -> io::Result<u64> {
    const PROC_PIDTBSDINFO: i32 = 3;
    let mut info: ProcBsdInfo = unsafe { std::mem::zeroed() };
    unsafe extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut core::ffi::c_void,
            buffersize: i32,
        ) -> i32;
    }
    let expected = std::mem::size_of::<ProcBsdInfo>();
    let length = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut ProcBsdInfo).cast::<core::ffi::c_void>(),
            expected as i32,
        )
    };
    if length != expected as i32 {
        return Err(io::Error::last_os_error());
    }
    Ok(info
        .pbi_start_tvsec
        .saturating_mul(1_000_000)
        .saturating_add(info.pbi_start_tvusec))
}

#[cfg(target_os = "macos")]
fn process_executable_macos(pid: i32) -> io::Result<PathBuf> {
    const BUFFER_LEN: usize = 4_096;
    let mut buffer = [0_u8; BUFFER_LEN];
    unsafe extern "C" {
        fn proc_pidpath(pid: i32, buffer: *mut core::ffi::c_void, buffersize: u32) -> i32;
    }
    let length = unsafe {
        proc_pidpath(
            pid,
            buffer.as_mut_ptr().cast::<core::ffi::c_void>(),
            BUFFER_LEN as u32,
        )
    };
    if length <= 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(
        &buffer[..length as usize],
    )))
}

#[derive(Debug, Clone, Copy)]
struct Copy {
    title: &'static str,
    subtitle: &'static str,
    running: &'static str,
    stopped: &'static str,
    start: &'static str,
    stop: &'static str,
    restart: &'static str,
    open_web: &'static str,
    autostart: &'static str,
    autostart_on: &'static str,
    autostart_off: &'static str,
    check_update: &'static str,
    version: &'static str,
    update_current: &'static str,
    update_available: &'static str,
    update_failed: &'static str,
    refresh: &'static str,
    open_panel: &'static str,
    quit: &'static str,
    service: &'static str,
    preferences: &'static str,
    last_action: &'static str,
    done: &'static str,
    missing_host: &'static str,
    foreign_port: &'static str,
    start_failed: &'static str,
    stop_failed: &'static str,
    wait_failed: &'static str,
    open_failed: &'static str,
    shell_open_failed: &'static str,
    lock_error: &'static str,
}

fn chinese_copy() -> Copy {
    Copy {
        title: "DeepSeek Harness-rs",
        subtitle: "本机服务与 Web 控制台",
        running: "运行中",
        stopped: "已停止",
        start: "启动服务",
        stop: "停止服务",
        restart: "重新启动",
        open_web: "打开 Harness",
        autostart: "开机自启",
        autostart_on: "开机自启已开启",
        autostart_off: "开机自启已关闭",
        check_update: "检查更新",
        version: "版本",
        update_current: "当前已是最新版本",
        update_available: "发现新版本",
        update_failed: "检查更新失败",
        refresh: "刷新状态",
        open_panel: "打开控制面板",
        quit: "退出启动器",
        service: "服务",
        preferences: "启动与更新",
        last_action: "最近操作",
        done: "操作完成",
        missing_host: "未找到主程序",
        foreign_port: "58080 已由外部进程占用；启动器不会停止不属于它的进程",
        start_failed: "启动失败",
        stop_failed: "停止失败",
        wait_failed: "等待进程退出失败",
        open_failed: "打开失败",
        shell_open_failed: "系统打开命令失败",
        lock_error: "启动器状态不可用",
    }
}

fn english_copy() -> Copy {
    Copy {
        title: "DeepSeek Harness-rs",
        subtitle: "Local service and Web console",
        running: "Running",
        stopped: "Stopped",
        start: "Start service",
        stop: "Stop service",
        restart: "Restart service",
        open_web: "Open Harness",
        autostart: "Start with Windows",
        autostart_on: "Start with Windows is enabled",
        autostart_off: "Start with Windows is disabled",
        check_update: "Check for updates",
        version: "Version",
        update_current: "You are up to date",
        update_available: "Update available",
        update_failed: "Update check failed",
        refresh: "Refresh status",
        open_panel: "Open control panel",
        quit: "Quit launcher",
        service: "Service",
        preferences: "Startup and updates",
        last_action: "Last action",
        done: "Operation completed",
        missing_host: "Host executable was not found",
        foreign_port: "Port 58080 is owned by another process; the launcher will not stop it",
        start_failed: "Start failed",
        stop_failed: "Stop failed",
        wait_failed: "Waiting for process exit failed",
        open_failed: "Open failed",
        shell_open_failed: "System open command failed",
        lock_error: "Launcher status is unavailable",
    }
}

fn localized_copy() -> Copy {
    if system_prefers_chinese() {
        chinese_copy()
    } else {
        english_copy()
    }
}

fn system_prefers_chinese() -> bool {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn GetUserDefaultUILanguage() -> u16;
        }
        let language = unsafe { GetUserDefaultUILanguage() };
        language == 0x0004
            || matches!(language & 0x03ff, 0x0004)
            || matches!(language, 0x0804 | 0x1004)
    }
    #[cfg(not(windows))]
    {
        let locale = std::env::var("LC_ALL")
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(|| std::env::var("LC_MESSAGES").ok())
            .or_else(|| std::env::var("LANG").ok())
            .or_else(|| std::env::var("LANGUAGE").ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        locale.starts_with("zh") || locale.contains("zh_") || locale.contains("zh-")
    }
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: Option<String>,
    draft: bool,
}

fn parse_version(value: &str) -> Option<semver::Version> {
    semver::Version::parse(value.trim().trim_start_matches(['v', 'V'])).ok()
}

fn is_newer_version(candidate: &str, current: &str) -> bool {
    matches!(
        (parse_version(candidate), parse_version(current)),
        (Some(candidate), Some(current)) if candidate > current
    )
}

fn update_status(copy: Copy) -> Result<String, String> {
    update_status_from(UPDATE_RELEASES_API, copy)
}

fn update_status_from(url: &str, copy: Copy) -> Result<String, String> {
    let mut response = ureq::get(url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "deepseek-harness-rs-launcher")
        .call()
        .map_err(|error| format!("{}: {error}", copy.update_failed))?;
    let releases: Vec<GithubRelease> = response
        .body_mut()
        .read_json()
        .map_err(|error| format!("{}: {error}", copy.update_failed))?;
    let release = releases
        .into_iter()
        .filter(|release| !release.draft && parse_version(&release.tag_name).is_some())
        .max_by_key(|release| parse_version(&release.tag_name))
        .ok_or_else(|| copy.update_current.to_string())?;
    if is_newer_version(&release.tag_name, PRODUCT_VERSION) {
        let url = release.html_url.as_deref().unwrap_or(UPDATE_RELEASES_URL);
        Ok(format!(
            "{} {}（{} {}）\n{url}",
            copy.update_available, release.tag_name, copy.version, PRODUCT_VERSION
        ))
    } else {
        Ok(format!(
            "{}（{} {}）",
            copy.update_current, copy.version, PRODUCT_VERSION
        ))
    }
}

#[cfg(windows)]
fn wide_null(value: impl AsRef<OsStr>) -> Vec<u16> {
    value
        .as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
struct RegistryKey(HKEY);

#[cfg(windows)]
impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

#[cfg(windows)]
fn open_autostart_key(access: u32) -> io::Result<RegistryKey> {
    let subkey = wide_null(AUTOSTART_REGISTRY_SUBKEY);
    let mut key = ptr::null_mut();
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, access, &mut key) };
    if status == 0 {
        Ok(RegistryKey(key))
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

#[cfg(windows)]
fn create_autostart_key(access: u32) -> io::Result<RegistryKey> {
    let subkey = wide_null(AUTOSTART_REGISTRY_SUBKEY);
    let mut key = ptr::null_mut();
    let status = unsafe { RegCreateKeyW(HKEY_CURRENT_USER, subkey.as_ptr(), &mut key) };
    if status == 0 {
        drop(RegistryKey(key));
        open_autostart_key(access)
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

#[cfg(windows)]
fn autostart_enabled() -> io::Result<bool> {
    let key = open_autostart_key(KEY_QUERY_VALUE)?;
    let name = wide_null(AUTOSTART_VALUE_NAME);
    let mut value_type = 0_u32;
    let mut size = 0_u32;
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            ptr::null(),
            &mut value_type,
            ptr::null_mut(),
            &mut size,
        )
    };
    if status == 2 {
        Ok(false)
    } else if status == 0 {
        Ok(value_type == REG_SZ && size > 2)
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

#[cfg(not(windows))]
fn autostart_enabled() -> io::Result<bool> {
    Ok(false)
}

#[cfg(windows)]
fn set_autostart(enabled: bool) -> io::Result<()> {
    let key = create_autostart_key(KEY_SET_VALUE)?;
    let name = wide_null(AUTOSTART_VALUE_NAME);
    if !enabled {
        let status = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
        return if status == 0 || status == 2 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(status as i32))
        };
    }
    let command = format!("\"{}\" --background", std::env::current_exe()?.display());
    let value = wide_null(command);
    let byte_len = value.len() * std::mem::size_of::<u16>();
    let bytes = unsafe { std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), byte_len) };
    let status = unsafe {
        RegSetValueExW(
            key.0,
            name.as_ptr(),
            0,
            REG_SZ,
            bytes.as_ptr(),
            bytes.len() as u32,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

#[cfg(not(windows))]
fn set_autostart(_enabled: bool) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "launcher autostart is only implemented on Windows",
    ))
}

#[cfg(windows)]
struct SingleInstanceGuard(HANDLE);

#[cfg(windows)]
impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(unix)]
struct SingleInstanceGuard(fs::File);

#[cfg(unix)]
impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[cfg(windows)]
fn focus_existing_launcher() {
    let class_name = wide_null("ZsuiMainWindow");
    let title = wide_null("DeepSeek Harness-rs");
    let window = unsafe { FindWindowW(class_name.as_ptr(), title.as_ptr()) };
    if !window.is_null() {
        unsafe {
            ShowWindow(window, SW_RESTORE);
            SetForegroundWindow(window);
        }
    }
}

#[cfg(windows)]
fn acquire_single_instance() -> io::Result<Option<SingleInstanceGuard>> {
    let name = wide_null(SINGLE_INSTANCE_MUTEX_NAME);
    let handle = unsafe { CreateMutexW(ptr::null(), 1, name.as_ptr()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { CloseHandle(handle) };
        focus_existing_launcher();
        Ok(None)
    } else {
        Ok(Some(SingleInstanceGuard(handle)))
    }
}

#[cfg(unix)]
fn acquire_single_instance() -> io::Result<Option<SingleInstanceGuard>> {
    let runtime_root = launcher_runtime_root(Path::new("."));
    fs::create_dir_all(&runtime_root)?;
    acquire_unix_single_instance_at(&runtime_root.join("dsh-launcher.lock"))
}

#[cfg(unix)]
fn acquire_unix_single_instance_at(lock_path: &Path) -> io::Result<Option<SingleInstanceGuard>> {
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(lock_path)?;
    let status = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if status == 0 {
        Ok(Some(SingleInstanceGuard(lock)))
    } else {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            Ok(None)
        } else {
            Err(error)
        }
    }
}

#[derive(Debug)]
struct ServiceController {
    root: PathBuf,
    executable: PathBuf,
    child: Option<Child>,
    copy: Copy,
}

impl ServiceController {
    fn discover() -> io::Result<Self> {
        let launcher = std::env::current_exe()?;
        let root = launcher
            .parent()
            .ok_or_else(|| io::Error::other("launcher has no parent directory"))?
            .to_path_buf();
        let executable = root.join(core_executable_name());
        Ok(Self {
            root,
            executable,
            child: None,
            copy: localized_copy(),
        })
    }

    fn ownership(&mut self) -> ServiceOwnership {
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(None) => return ServiceOwnership::ManagedRunning,
                Ok(Some(_)) | Err(_) => self.child = None,
            }
        }
        if self.owned_process().is_some() {
            ServiceOwnership::ManagedRunning
        } else if port_is_open(DEFAULT_PORT) {
            ServiceOwnership::ForeignPort
        } else {
            ServiceOwnership::Stopped
        }
    }

    fn owned_process(&self) -> Option<(LauncherStateFile, ProcessIdentity)> {
        let state = match read_launcher_state(&self.root) {
            Ok(Some(state)) => state,
            Ok(None) => return None,
            Err(_) => {
                let _ = remove_launcher_state(&self.root);
                return None;
            }
        };
        match inspect_process(state.pid) {
            Ok(process) if state.matches_process(&process) => Some((state, process)),
            Ok(_) | Err(_) => {
                let _ = remove_launcher_state(&self.root);
                None
            }
        }
    }

    fn start(&mut self) -> Result<(), String> {
        match self.ownership() {
            ServiceOwnership::ManagedRunning => return Ok(()),
            ServiceOwnership::ForeignPort => return Err(self.copy.foreign_port.to_string()),
            ServiceOwnership::Stopped => {}
        }
        if !self.executable.is_file() {
            return Err(format!(
                "{}: {}",
                self.copy.missing_host,
                self.executable.display()
            ));
        }
        let log_dir = launcher_log_dir(&self.root);
        fs::create_dir_all(&log_dir).map_err(|error| error.to_string())?;
        let stdout =
            fs::File::create(log_dir.join("dsh.out.log")).map_err(|error| error.to_string())?;
        let stderr =
            fs::File::create(log_dir.join("dsh.err.log")).map_err(|error| error.to_string())?;
        let home = active_home(&self.root);
        let mut command = Command::new(&self.executable);
        command
            .args(["web", "--port", &DEFAULT_PORT.to_string()])
            .env("DSH_HOME", &home)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command
            .spawn()
            .map_err(|error| format!("{}: {error}", self.copy.start_failed))?;
        let pid = child.id();
        let identity = match wait_for_process_identity(pid, &self.executable) {
            Ok(identity) => identity,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{}: {error}", self.copy.start_failed));
            }
        };
        let state = LauncherStateFile::owned(
            identity.pid,
            identity.creation_time,
            identity.executable,
            home,
            DEFAULT_PORT,
        );
        if let Err(error) = write_launcher_state(&self.root, &state) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{}: {error}", self.copy.start_failed));
        }
        self.child = Some(child);
        for _ in 0..80 {
            if port_is_open(DEFAULT_PORT) {
                return Ok(());
            }
            if self
                .child
                .as_mut()
                .and_then(|child| child.try_wait().ok())
                .flatten()
                .is_some()
            {
                self.child = None;
                let _ = remove_launcher_state(&self.root);
                let error = fs::read_to_string(launcher_log_dir(&self.root).join("dsh.err.log"))
                    .unwrap_or_default();
                let detail = error.lines().last().unwrap_or(self.copy.start_failed);
                return Err(format!("{}: {detail}", self.copy.start_failed));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(format!(
            "{}: 58080 readiness timeout",
            self.copy.start_failed
        ))
    }

    fn stop(&mut self) -> Result<(), String> {
        if let Some(mut child) = self.child.take() {
            let pid = child.id();
            let state = read_launcher_state(&self.root)
                .map_err(|error| format!("{}: {error}", self.copy.stop_failed))?;
            let identity = inspect_process(pid)
                .map_err(|error| format!("{}: {error}", self.copy.stop_failed))?;
            let owned = state
                .as_ref()
                .is_some_and(|state| state.matches_process(&identity));
            if !owned {
                self.child = Some(child);
                return Err(self.copy.foreign_port.to_string());
            }
            #[cfg(windows)]
            child
                .kill()
                .map_err(|error| format!("{}: {error}", self.copy.stop_failed))?;
            #[cfg(unix)]
            stop_process(&identity)
                .map_err(|error| format!("{}: {error}", self.copy.stop_failed))?;
            child
                .wait()
                .map_err(|error| format!("{}: {error}", self.copy.wait_failed))?;
            remove_launcher_state(&self.root)
                .map_err(|error| format!("{}: {error}", self.copy.stop_failed))?;
            return Ok(());
        }
        if let Some((_state, process)) = self.owned_process() {
            stop_process(&process)
                .map_err(|error| format!("{}: {error}", self.copy.stop_failed))?;
            wait_for_process_exit(&process)
                .map_err(|error| format!("{}: {error}", self.copy.wait_failed))?;
            remove_launcher_state(&self.root)
                .map_err(|error| format!("{}: {error}", self.copy.stop_failed))?;
            return Ok(());
        }
        if port_is_open(DEFAULT_PORT) {
            return Err(self.copy.foreign_port.to_string());
        }
        Ok(())
    }

    fn restart(&mut self) -> Result<(), String> {
        self.stop()?;
        self.start()
    }

    fn open_web(&mut self) -> Result<(), String> {
        if !port_is_open(DEFAULT_PORT) {
            self.start()?;
        }
        open_target(ADDRESS, self.copy)
    }

    fn set_autostart(&self, enabled: bool) -> Result<String, String> {
        set_autostart(enabled).map_err(|error| error.to_string())?;
        Ok(if enabled {
            self.copy.autostart_on
        } else {
            self.copy.autostart_off
        }
        .to_string())
    }

    fn check_update(&self) -> Result<String, String> {
        update_status(self.copy)
    }

    const fn stop_owned_child_on_drop(&self) -> bool {
        false
    }

    fn execute(&mut self, command: LauncherCommand) -> Result<String, String> {
        match command {
            LauncherCommand::Start => {
                self.start()?;
                Ok(self.copy.done.to_string())
            }
            LauncherCommand::Stop => {
                self.stop()?;
                Ok(self.copy.done.to_string())
            }
            LauncherCommand::Restart => {
                self.restart()?;
                Ok(self.copy.done.to_string())
            }
            LauncherCommand::OpenWeb => {
                self.open_web()?;
                Ok(self.copy.done.to_string())
            }
            LauncherCommand::SetAutostart(enabled) => self.set_autostart(enabled),
            LauncherCommand::CheckUpdate => self.check_update(),
            LauncherCommand::Refresh => Ok(match self.ownership() {
                ServiceOwnership::Stopped => self.copy.stopped,
                ServiceOwnership::ManagedRunning => self.copy.running,
                ServiceOwnership::ForeignPort => self.copy.foreign_port,
            }
            .to_string()),
        }
    }
}

impl Drop for ServiceController {
    #[allow(clippy::collapsible_if)]
    fn drop(&mut self) {
        if self.stop_owned_child_on_drop() {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LauncherCommand {
    Start,
    Stop,
    Restart,
    OpenWeb,
    SetAutostart(bool),
    CheckUpdate,
    Refresh,
}

#[derive(Clone)]
enum Message {
    SetRunning(bool),
    Restart,
    OpenWeb,
    SetAutostart(bool),
    CheckUpdate,
    Refresh,
}

struct State {
    controller: Arc<Mutex<ServiceController>>,
    copy: Copy,
    ownership: ServiceOwnership,
    autostart: bool,
    status: String,
}

impl State {
    fn new(controller: Arc<Mutex<ServiceController>>) -> Self {
        let copy = localized_copy();
        let ownership = controller
            .lock()
            .map(|mut controller| controller.ownership())
            .unwrap_or(ServiceOwnership::ForeignPort);
        let autostart = autostart_enabled().unwrap_or(false);
        Self {
            controller,
            copy,
            ownership,
            autostart,
            status: ownership_label(copy, ownership).to_string(),
        }
    }

    fn refresh(&mut self) {
        self.ownership = self
            .controller
            .lock()
            .map(|mut controller| controller.ownership())
            .unwrap_or(ServiceOwnership::ForeignPort);
        self.autostart = autostart_enabled().unwrap_or(false);
    }

    fn run(&mut self, command: LauncherCommand) {
        let result = self
            .controller
            .lock()
            .map_err(|_| self.copy.lock_error.to_string())
            .and_then(|mut controller| controller.execute(command));
        self.status = match result {
            Ok(message) => message,
            Err(error) => error,
        };
        self.refresh();
    }
}

fn ownership_label(copy: Copy, ownership: ServiceOwnership) -> &'static str {
    match ownership {
        ServiceOwnership::Stopped => copy.stopped,
        ServiceOwnership::ManagedRunning => copy.running,
        ServiceOwnership::ForeignPort => copy.foreign_port,
    }
}

fn text_style(role: TextRole, color: ColorRole, weight: TextWeight) -> SemanticTextStyle {
    SemanticTextStyle {
        role,
        color,
        weight,
        horizontal_align: HorizontalAlign::Start,
        vertical_align: VerticalAlign::Center,
        wrap: TextWrap::NoWrap,
        ellipsis: true,
    }
}

fn view(state: &State) -> ViewNode<Message> {
    let managed = state.ownership == ServiceOwnership::ManagedRunning;
    let status_icon = match state.ownership {
        ServiceOwnership::Stopped => ZsIcon::Info,
        ServiceOwnership::ManagedRunning => ZsIcon::Success,
        ServiceOwnership::ForeignPort => ZsIcon::Warning,
    };
    let status_color = match state.ownership {
        ServiceOwnership::Stopped => ColorRole::SecondaryText,
        ServiceOwnership::ManagedRunning => ColorRole::Success,
        ServiceOwnership::ForeignPort => ColorRole::Warning,
    };
    column([
        row([
            icon(status_icon)
                .icon_size(ZsIconSize::Large)
                .icon_color(status_color),
            column([
                styled_text(
                    state.copy.title,
                    text_style(
                        TextRole::Title,
                        ColorRole::PrimaryText,
                        TextWeight::Semibold,
                    ),
                ),
                styled_text(
                    state.copy.subtitle,
                    text_style(
                        TextRole::Body,
                        ColorRole::SecondaryText,
                        TextWeight::Regular,
                    ),
                ),
            ])
            .gap(Dp::new(2.0).into()),
        ])
        .gap(Dp::new(12.0).into()),
        section(
            state.copy.service,
            [
                row([
                    column([
                        styled_text(
                            ownership_label(state.copy, state.ownership),
                            text_style(TextRole::BodyLarge, status_color, TextWeight::Semibold),
                        ),
                        styled_text(
                            ADDRESS,
                            text_style(
                                TextRole::Monospace,
                                ColorRole::SecondaryText,
                                TextWeight::Regular,
                            ),
                        ),
                    ])
                    .gap(Dp::new(2.0).into())
                    .flex(1.0),
                    if state.ownership == ServiceOwnership::ForeignPort {
                        styled_text(
                            state.copy.foreign_port,
                            text_style(TextRole::Caption, ColorRole::Warning, TextWeight::Medium),
                        )
                    } else {
                        toggle(managed).on_toggle(Message::SetRunning)
                    },
                ])
                .gap(Dp::new(12.0).into()),
                row([
                    primary_button(state.copy.open_web).on_click(Message::OpenWeb),
                    button(state.copy.restart)
                        .enabled(managed)
                        .on_click(Message::Restart),
                    button(state.copy.refresh).on_click(Message::Refresh),
                ])
                .gap(LAUNCHER_ACTION_GAP.into()),
            ],
        ),
        section(
            state.copy.preferences,
            [row([
                column([
                    styled_text(
                        state.copy.autostart,
                        text_style(TextRole::Body, ColorRole::PrimaryText, TextWeight::Medium),
                    ),
                    styled_text(
                        format!("{} {PRODUCT_VERSION}", state.copy.version),
                        text_style(
                            TextRole::Caption,
                            ColorRole::SecondaryText,
                            TextWeight::Regular,
                        ),
                    ),
                ])
                .gap(Dp::new(2.0).into())
                .flex(1.0),
                row([
                    toggle(state.autostart).on_toggle(Message::SetAutostart),
                    button(state.copy.check_update).on_click(Message::CheckUpdate),
                ])
                .gap(LAUNCHER_ACTION_GAP.into()),
            ])
            .gap(Dp::new(12.0).into())],
        ),
        section(
            state.copy.last_action,
            [styled_text(
                &state.status,
                text_style(
                    TextRole::Caption,
                    ColorRole::SecondaryText,
                    TextWeight::Regular,
                ),
            )
            .flex(1.0)],
        ),
    ])
    .gap(LAUNCHER_SECTION_GAP.into())
    .padding(LAUNCHER_CONTENT_PADDING.into())
    .bg(ThemeColorToken::Surface)
    .theme_mode(ZsuiThemeMode::System)
    .min_height(Dp::new(360.0).into())
}

fn update(state: &mut State, message: Message, _cx: &mut AppCx) {
    match message {
        Message::SetRunning(true) => state.run(LauncherCommand::Start),
        Message::SetRunning(false) => state.run(LauncherCommand::Stop),
        Message::Restart => state.run(LauncherCommand::Restart),
        Message::OpenWeb => state.run(LauncherCommand::OpenWeb),
        Message::SetAutostart(enabled) => state.run(LauncherCommand::SetAutostart(enabled)),
        Message::CheckUpdate => state.run(LauncherCommand::CheckUpdate),
        Message::Refresh => {
            state.run(LauncherCommand::Refresh);
        }
    }
}

fn port_is_open(port: u16) -> bool {
    ("127.0.0.1", port)
        .to_socket_addrs()
        .ok()
        .into_iter()
        .flatten()
        .any(|address| {
            TcpStream::connect_timeout(&address, Duration::from_millis(150))
                .and_then(|stream| stream.peer_addr())
                .is_ok()
        })
}

fn core_executable_name() -> &'static str {
    if cfg!(windows) {
        "deepseek-harness-rs.exe"
    } else {
        "deepseek-harness-rs"
    }
}

fn launcher_icon_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|executable| {
            executable
                .parent()
                .map(|root| root.join(LAUNCHER_ICON_FILE))
        })
        .filter(|path| path.is_file())
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../packaging/windows")
                .join(LAUNCHER_ICON_FILE)
        })
}

fn open_target(target: &str, copy: Copy) -> Result<(), String> {
    #[cfg(windows)]
    let status = Command::new("rundll32.exe")
        .arg("url.dll,FileProtocolHandler")
        .arg(target)
        .status();
    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg(target).status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open").arg(target).status();
    status
        .map_err(|error| format!("{}: {error}", copy.open_failed))?
        .success()
        .then_some(())
        .ok_or_else(|| copy.shell_open_failed.to_string())
}

fn main() -> Result<(), zsui::ZsuiError> {
    let Some(_single_instance) = acquire_single_instance()
        .map_err(|error| zsui::ZsuiError::host("launcher_single_instance", error.to_string()))?
    else {
        return Ok(());
    };
    let background = std::env::args_os().any(|argument| argument == "--background");
    #[cfg(target_os = "linux")]
    let initial_window_visible = true;
    #[cfg(not(target_os = "linux"))]
    let initial_window_visible = !background;
    let controller = Arc::new(Mutex::new(ServiceController::discover().unwrap_or_else(
        |_error| ServiceController {
            root: Path::new(".").to_path_buf(),
            executable: Path::new(core_executable_name()).to_path_buf(),
            child: None,
            copy: localized_copy(),
        },
    )));
    let copy = localized_copy();
    let invalidation = UiInvalidationHandle::new();

    let icon_path = launcher_icon_path().to_string_lossy().into_owned();
    let tray_ownership = controller
        .lock()
        .map(|mut controller| controller.ownership())
        .unwrap_or(ServiceOwnership::ForeignPort);
    let tray = TraySpec::new()
        .tooltip(copy.title)
        .icon_path(icon_path.clone())
        .menu(tray_menu_spec(
            copy,
            tray_ownership,
            autostart_enabled().unwrap_or(false),
        ))
        .dynamic_menu({
            let controller = Arc::clone(&controller);
            move || {
                let ownership = controller
                    .lock()
                    .map(|mut controller| controller.ownership())
                    .unwrap_or(ServiceOwnership::ForeignPort);
                tray_menu_spec(copy, ownership, autostart_enabled().unwrap_or(false))
            }
        });
    let command_controller = Arc::clone(&controller);
    let builder = NativeWindowBuilder::new(copy.title)
        .app_name("DeepSeek Harness-rs")
        .size(680, 430)
        .min_size(600, 390)
        .icon_path(icon_path)
        .visible(initial_window_visible)
        .resizable(false)
        .invalidation_handle(invalidation)
        .release_view_when_hidden();
    #[cfg(not(target_os = "linux"))]
    let builder = builder.tray(tray);
    #[cfg(target_os = "linux")]
    let builder = {
        let _unsupported_tray = tray;
        builder
    };
    #[cfg(target_os = "linux")]
    let close_command = ZsuiCommand::Quit;
    #[cfg(not(target_os = "linux"))]
    let close_command = ZsuiCommand::HideMainWindow;
    builder
        .on_close_requested(close_command)
        .app_command_executor(move |command| match command {
            ZsuiCommand::Custom { id, .. } => {
                let action = match id.as_str() {
                    TRAY_OPEN_HARNESS_COMMAND => Some(LauncherCommand::OpenWeb),
                    TRAY_START_COMMAND => Some(LauncherCommand::Start),
                    TRAY_STOP_COMMAND => Some(LauncherCommand::Stop),
                    TRAY_RESTART_COMMAND => Some(LauncherCommand::Restart),
                    TRAY_AUTOSTART_COMMAND => Some(LauncherCommand::SetAutostart(
                        !autostart_enabled().unwrap_or(false),
                    )),
                    TRAY_CHECK_UPDATE_COMMAND => Some(LauncherCommand::CheckUpdate),
                    _ => None,
                };
                if let Some(action) = action {
                    command_controller
                        .lock()
                        .map_err(|_| {
                            zsui::ZsuiError::host(
                                "launcher_command",
                                localized_copy().lock_error.to_string(),
                            )
                        })?
                        .execute(action)
                        .map_err(|error| zsui::ZsuiError::host("launcher_command", error))?;
                }
                Ok(Vec::new())
            }
            _ => Ok(Vec::new()),
        })
        .stateful_view_with_app_commands(State::new(controller), view, update, |command| {
            match command {
                ZsuiCommand::Custom { id, .. } if id == TRAY_OPEN_HARNESS_COMMAND => {
                    Some(Message::OpenWeb)
                }
                ZsuiCommand::Custom { id, .. } if id == TRAY_START_COMMAND => {
                    Some(Message::SetRunning(true))
                }
                ZsuiCommand::Custom { id, .. } if id == TRAY_STOP_COMMAND => {
                    Some(Message::SetRunning(false))
                }
                ZsuiCommand::Custom { id, .. } if id == TRAY_RESTART_COMMAND => {
                    Some(Message::Restart)
                }
                ZsuiCommand::Custom { id, .. } if id == TRAY_AUTOSTART_COMMAND => {
                    Some(Message::SetAutostart(!autostart_enabled().unwrap_or(false)))
                }
                ZsuiCommand::Custom { id, .. } if id == TRAY_CHECK_UPDATE_COMMAND => {
                    Some(Message::CheckUpdate)
                }
                _ => None,
            }
        })
        .run()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::thread;

    use super::{
        DEFAULT_PORT, Dp, LAUNCHER_ACTION_GAP, LAUNCHER_CONTENT_PADDING, LAUNCHER_SECTION_GAP,
        LauncherAction, LauncherStateFile, MenuItemSpec, MenuSpec, PRIMARY_ACTIONS,
        ProcessIdentity, SECONDARY_ACTIONS, ServiceController, ServiceOwnership,
        TRAY_RESTART_COMMAND, TRAY_START_COMMAND, TRAY_STOP_COMMAND, chinese_copy,
        core_executable_name, english_copy, is_newer_version, now_unix_millis, parse_version,
        read_launcher_state, tray_menu_spec, update_status_from, write_launcher_state,
    };

    #[test]
    fn launcher_has_complete_chinese_and_english_copy() {
        let zh = chinese_copy();
        let en = english_copy();
        assert_eq!(zh.start, "启动服务");
        assert_eq!(zh.open_web, "打开 Harness");
        assert_eq!(zh.subtitle, "本机服务与 Web 控制台");
        assert_eq!(en.start, "Start service");
        assert_eq!(en.open_web, "Open Harness");
        assert_eq!(en.subtitle, "Local service and Web console");
    }

    #[test]
    fn launcher_passes_the_resolved_formal_home_to_the_host() {
        let source = include_str!("main.rs");
        assert!(source.contains(".env(\"DSH_HOME\", &home)"));
        assert!(source.contains("let home = active_home(&self.root);"));
        assert!(source.contains("LauncherStateFile::owned("));
        assert!(source.contains("home,"));
    }

    #[test]
    fn unix_launcher_inspects_and_stops_the_spawned_host_process() {
        let source = include_str!("main.rs");
        assert!(source.contains("/proc/{pid}/exe"));
        assert!(source.contains("linux_pidfd_send_signal(&pidfd, libc::SIGTERM)"));
        assert!(source.contains("libc::syscall"));
        assert!(source.contains("libc::SYS_pidfd_open"));
        assert!(source.contains("libc::SYS_pidfd_send_signal"));
        assert!(source.contains("libc::kill(pid, libc::SIGTERM)"));
        assert!(source.contains("libc::kill(pid, 0)"));
        assert!(source.contains("fn process_creation_time_macos(pid: i32)"));
        assert!(source.contains("let observed = inspect_process(identity.pid)?;"));
        assert!(source.contains("#[cfg(unix)]\n            stop_process(&identity)"));
    }

    #[test]
    fn linux_process_start_time_parser_handles_spaces_and_parentheses_in_comm() {
        let mut fields = vec!["S".to_string()];
        fields.extend((4..=21).map(|field| field.to_string()));
        fields.push("987654".to_string());
        fields.push("0".to_string());
        let stat = format!("42 (host worker) helper) {}", fields.join(" "));
        assert_eq!(super::linux_process_start_time(&stat).unwrap(), 987654);
    }

    #[test]
    fn tray_open_panel_and_quit_use_native_window_lifecycle_commands() {
        let menu = tray_menu_spec(english_copy(), ServiceOwnership::Stopped, false);
        assert_eq!(
            command_for(&menu, super::TRAY_OPEN_PANEL_COMMAND),
            Some(&zsui::Command::ShowMainWindow)
        );
        assert_eq!(
            command_for(&menu, super::TRAY_QUIT_COMMAND),
            Some(&zsui::Command::Quit)
        );
    }

    #[test]
    fn release_versions_follow_semver_including_prerelease_ordering() {
        assert_eq!(
            parse_version("v0.1.2-alpha.2").map(|version| version.to_string()),
            Some("0.1.2-alpha.2".to_string())
        );
        assert!(is_newer_version("v0.1.2-alpha.3", "0.1.2-alpha.2"));
        assert!(is_newer_version("v0.1.2", "0.1.2-alpha.2"));
        assert!(is_newer_version("v0.1.3", "0.1.2-alpha.2"));
        assert!(!is_newer_version("not-a-version", "0.1.2"));
    }

    #[test]
    fn update_check_uses_release_metadata_and_reports_a_newer_release() {
        let server = TcpListener::bind("127.0.0.1:0").expect("bind update fixture");
        let address = format!(
            "http://{}/releases",
            server.local_addr().expect("fixture address")
        );
        let worker = thread::spawn(move || {
            let (mut stream, _) = server.accept().expect("receive update request");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).expect("read update request");
            let body = r#"[{"tag_name":"v0.1.2-alpha.5","html_url":"https://example.invalid/release","draft":false}]"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("respond to update request");
        });
        let message = update_status_from(&address, english_copy()).expect("check update");
        worker.join().expect("join update fixture");
        assert!(message.contains("Update available"));
        assert!(message.contains("v0.1.2-alpha.5"));
        assert!(message.contains("https://example.invalid/release"));
    }

    #[test]
    fn launcher_surface_keeps_one_status_region_and_two_action_groups() {
        assert_eq!(
            PRIMARY_ACTIONS,
            [
                LauncherAction::Start,
                LauncherAction::Stop,
                LauncherAction::Restart,
            ]
        );
        assert_eq!(
            SECONDARY_ACTIONS,
            [
                LauncherAction::OpenHarness,
                LauncherAction::Autostart,
                LauncherAction::CheckUpdate,
                LauncherAction::Refresh,
            ]
        );
        assert_eq!(LAUNCHER_CONTENT_PADDING, Dp::new(24.0));
        assert_eq!(LAUNCHER_SECTION_GAP, Dp::new(16.0));
        assert_eq!(LAUNCHER_ACTION_GAP, Dp::new(8.0));
    }

    #[test]
    fn launcher_surface_uses_one_service_switch_and_harness_visual_primitives() {
        let source = include_str!("main.rs");
        assert!(source.contains("toggle("));
        assert!(source.contains("primary_button("));
        assert!(source.contains("section("));
        assert!(source.contains("styled_text("));
        assert!(source.contains("ThemeColorToken::Surface"));
        assert!(source.contains("ZsuiThemeMode::System"));
        let view_source = source
            .split("fn view(state: &State)")
            .nth(1)
            .expect("view source")
            .split("fn update(")
            .next()
            .expect("view boundary");
        assert!(!view_source.contains("button(state.copy.start)"));
        assert!(!view_source.contains("button(state.copy.stop)"));
    }

    #[test]
    fn foreign_port_is_not_presented_as_a_switchable_managed_service() {
        let source = include_str!("main.rs");
        let view_source = source
            .split("fn view(state: &State)")
            .nth(1)
            .expect("view source");
        assert!(view_source.contains("if state.ownership == ServiceOwnership::ForeignPort"));
        assert!(view_source.contains("toggle(managed)"));
    }

    #[test]
    fn windows_launcher_has_a_production_tray_bridge() {
        let source = include_str!("main.rs");
        let application = include_str!("../../../vendor/zsui/src/platform/windows/application.rs");
        let tray = include_str!("../../../vendor/zsui/src/platform/windows/services/tray.rs");
        let window_proc = include_str!("../../../vendor/zsui/src/platform/windows/window_proc.rs");
        let menu = include_str!("../../../vendor/zsui/src/platform/windows/services/menu.rs");
        assert!(source.contains(".tray(tray)"));
        for required in [
            "Shell_NotifyIconW",
            "dispatch_windows_win32_status_item_callback",
            "dispatch_windows_win32_app_command",
        ] {
            assert!(
                tray.contains(required) || application.contains(required),
                "missing production tray primitive: {required}"
            );
        }
        assert!(window_proc.contains("WM_CLOSE"));
        assert!(window_proc.contains("dispatch_windows_win32_status_item_callback"));
        assert!(menu.contains("dispatch_windows_win32_window_view_input"));
        assert!(menu.contains("dispatch_windows_win32_app_command"));
    }

    #[test]
    fn tray_bridge_targets_the_visible_zsui_window_and_has_a_testable_close_contract() {
        let tray = include_str!("../../../vendor/zsui/src/platform/windows/services/tray.rs");
        let window_proc = include_str!("../../../vendor/zsui/src/platform/windows/window_proc.rs");
        assert!(tray.contains("dispatch_windows_win32_status_item_callback"));
        assert!(tray.contains("dispatch_windows_win32_app_command"));
        assert!(window_proc.contains("restore_windows_win32_status_items(hwnd)"));
        assert!(window_proc.contains("dispatch_windows_win32_status_item_callback"));
    }

    #[test]
    fn launcher_does_not_duplicate_native_exit_inside_the_window() {
        assert_eq!(PRIMARY_ACTIONS.len() + SECONDARY_ACTIONS.len(), 7);
    }

    #[test]
    fn dropping_the_launcher_does_not_stop_the_owned_host() {
        let controller = ServiceController {
            root: PathBuf::from("."),
            executable: PathBuf::from(core_executable_name()),
            child: None,
            copy: english_copy(),
        };
        assert!(!controller.stop_owned_child_on_drop());
    }

    #[test]
    fn launcher_state_round_trips_the_exact_owned_process_identity() {
        let runtime_root = unique_test_root("state-roundtrip");
        let state_path = super::launcher_state_path_in_runtime_root(&runtime_root);
        let state = LauncherStateFile::owned(
            4242,
            7_654_321,
            PathBuf::from(r"C:\Program Files\DeepSeek Harness-rs\deepseek-harness-rs.exe"),
            PathBuf::from(r"C:\Users\Administrator\AppData\Local\DeepSeek Harness"),
            DEFAULT_PORT,
        );
        super::write_launcher_state_at(&state_path, &state).expect("write launcher state");
        let restored = super::read_launcher_state_at(&state_path)
            .expect("read isolated launcher state")
            .expect("isolated launcher state should exist");
        assert_eq!(restored, state);
        std::fs::remove_dir_all(runtime_root).expect("remove test runtime root");
    }

    #[test]
    fn launcher_state_rejects_a_different_process_start_identity() {
        let state = LauncherStateFile::owned(
            4242,
            7_654_321,
            PathBuf::from(r"C:\Harness\deepseek-harness-rs.exe"),
            PathBuf::from(r"C:\HarnessHome"),
            DEFAULT_PORT,
        );
        let observed = ProcessIdentity {
            pid: 4242,
            creation_time: 7_654_322,
            executable: PathBuf::from(r"C:\Harness\deepseek-harness-rs.exe"),
        };
        assert!(!state.matches_process(&observed));
    }

    #[cfg(unix)]
    #[test]
    fn unix_single_instance_lock_rejects_contention_and_reopens_after_drop() {
        let root = unique_test_root("single-instance");
        std::fs::create_dir_all(&root).expect("create lock test root");
        let lock_path = root.join("dsh-launcher.lock");

        let first = super::acquire_unix_single_instance_at(&lock_path)
            .expect("acquire first Unix launcher lock")
            .expect("first Unix launcher lock should be available");
        assert!(
            super::acquire_unix_single_instance_at(&lock_path)
                .expect("check contended Unix launcher lock")
                .is_none()
        );

        drop(first);
        let reopened = super::acquire_unix_single_instance_at(&lock_path)
            .expect("reacquire Unix launcher lock")
            .expect("Unix launcher lock should reopen after drop");
        drop(reopened);
        std::fs::remove_dir_all(root).expect("remove lock test root");
    }

    #[test]
    fn tray_menu_tracks_owned_and_foreign_service_states() {
        let copy = english_copy();
        let stopped = tray_menu_spec(copy, ServiceOwnership::Stopped, false);
        assert_eq!(command_enabled(&stopped, TRAY_START_COMMAND), Some(true));
        assert_eq!(command_enabled(&stopped, TRAY_STOP_COMMAND), Some(false));
        assert_eq!(command_enabled(&stopped, TRAY_RESTART_COMMAND), Some(false));

        let managed = tray_menu_spec(copy, ServiceOwnership::ManagedRunning, true);
        assert_eq!(command_enabled(&managed, TRAY_START_COMMAND), Some(false));
        assert_eq!(command_enabled(&managed, TRAY_STOP_COMMAND), Some(true));
        assert_eq!(command_enabled(&managed, TRAY_RESTART_COMMAND), Some(true));

        let foreign = tray_menu_spec(copy, ServiceOwnership::ForeignPort, false);
        assert_eq!(command_enabled(&foreign, TRAY_START_COMMAND), Some(false));
        assert_eq!(command_enabled(&foreign, TRAY_STOP_COMMAND), Some(false));
        assert_eq!(command_enabled(&foreign, TRAY_RESTART_COMMAND), Some(false));
    }

    #[test]
    fn tray_menu_marks_autostart_when_windows_run_entry_is_enabled() {
        let menu = tray_menu_spec(english_copy(), ServiceOwnership::Stopped, true);
        assert_eq!(
            command_checked(&menu, super::TRAY_AUTOSTART_COMMAND),
            Some(true)
        );
    }

    fn command_enabled(menu: &MenuSpec, id: &str) -> Option<bool> {
        menu.items.iter().find_map(|item| match item {
            MenuItemSpec::Command {
                id: Some(item_id),
                enabled,
                ..
            } if item_id == id => Some(*enabled),
            _ => None,
        })
    }

    fn command_checked(menu: &MenuSpec, id: &str) -> Option<bool> {
        menu.items.iter().find_map(|item| match item {
            MenuItemSpec::Command {
                id: Some(item_id),
                checked,
                ..
            } if item_id == id => Some(*checked),
            _ => None,
        })
    }

    fn command_for<'a>(menu: &'a MenuSpec, id: &str) -> Option<&'a zsui::Command> {
        menu.items.iter().find_map(|item| match item {
            MenuItemSpec::Command {
                id: Some(item_id),
                command,
                ..
            } if item_id == id => Some(command),
            _ => None,
        })
    }

    fn unique_test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "dsh-launcher-{label}-{}-{}",
            std::process::id(),
            now_unix_millis()
        ))
    }
}
