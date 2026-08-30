#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::fs;
use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use zsui::stable::{Dp, Element, UpdateContext, button, column, row, text, window};

const DEFAULT_PORT: u16 = 58080;
const ADDRESS: &str = "http://127.0.0.1:58080/";

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
    open_logs: &'static str,
    install_skins: &'static str,
    refresh: &'static str,
    quit: &'static str,
    status: &'static str,
    done: &'static str,
    missing_host: &'static str,
    foreign_port: &'static str,
    start_failed: &'static str,
    stop_failed: &'static str,
    wait_failed: &'static str,
    open_failed: &'static str,
    shell_open_failed: &'static str,
    skin_installed: &'static str,
    skin_failed: &'static str,
    lock_error: &'static str,
}

fn chinese_copy() -> Copy {
    Copy {
        title: "DeepSeek Harness-rs 启动器",
        subtitle: "正式 Web Host",
        running: "运行中",
        stopped: "已停止",
        start: "启动",
        stop: "停止",
        restart: "重启",
        open_web: "打开网页",
        open_logs: "日志目录",
        install_skins: "安装皮肤",
        refresh: "刷新状态",
        quit: "退出",
        status: "状态",
        done: "操作完成",
        missing_host: "未找到程序",
        foreign_port: "58080 已由外部进程占用；启动器拒绝停止非本次启动的进程",
        start_failed: "启动失败",
        stop_failed: "停止失败",
        wait_failed: "等待进程退出失败",
        open_failed: "打开失败",
        shell_open_failed: "系统打开命令失败",
        skin_installed: "皮肤安装完成，刷新网页后生效",
        skin_failed: "皮肤安装失败",
        lock_error: "启动器状态锁已损坏",
    }
}

fn english_copy() -> Copy {
    Copy {
        title: "DeepSeek Harness-rs Launcher",
        subtitle: "Production Web Host",
        running: "Running",
        stopped: "Stopped",
        start: "Start",
        stop: "Stop",
        restart: "Restart",
        open_web: "Open Web",
        open_logs: "Open Logs",
        install_skins: "Install Skins",
        refresh: "Refresh",
        quit: "Quit",
        status: "Status",
        done: "Operation completed",
        missing_host: "Executable was not found",
        foreign_port: "Port 58080 is owned by another process; the launcher will not stop it",
        start_failed: "Start failed",
        stop_failed: "Stop failed",
        wait_failed: "Waiting for process exit failed",
        open_failed: "Open failed",
        shell_open_failed: "System open command failed",
        skin_installed: "Skins installed; refresh the Web UI to apply them",
        skin_failed: "Skin installation failed",
        lock_error: "Launcher state lock is poisoned",
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

    fn is_running(&mut self) -> bool {
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(None) => return true,
                Ok(Some(_)) | Err(_) => self.child = None,
            }
        }
        port_is_open(DEFAULT_PORT)
    }

    fn start(&mut self) -> Result<(), String> {
        if self.is_running() {
            return Ok(());
        }
        if !self.executable.is_file() {
            return Err(format!(
                "{}: {}",
                self.copy.missing_host,
                self.executable.display()
            ));
        }
        let log_dir = self.root.join("logs");
        fs::create_dir_all(&log_dir).map_err(|error| error.to_string())?;
        let stdout =
            fs::File::create(log_dir.join("dsh.out.log")).map_err(|error| error.to_string())?;
        let stderr =
            fs::File::create(log_dir.join("dsh.err.log")).map_err(|error| error.to_string())?;
        let mut command = Command::new(&self.executable);
        command
            .args(["web", "--port", &DEFAULT_PORT.to_string()])
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let child = command
            .spawn()
            .map_err(|error| format!("{}: {error}", self.copy.start_failed))?;
        self.child = Some(child);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), String> {
        let Some(mut child) = self.child.take() else {
            if port_is_open(DEFAULT_PORT) {
                return Err(self.copy.foreign_port.to_string());
            }
            return Ok(());
        };
        child
            .kill()
            .map_err(|error| format!("{}: {error}", self.copy.stop_failed))?;
        child
            .wait()
            .map_err(|error| format!("{}: {error}", self.copy.wait_failed))?;
        Ok(())
    }

    fn restart(&mut self) -> Result<(), String> {
        self.stop()?;
        self.start()
    }

    fn open_web(&self) -> Result<(), String> {
        open_target(ADDRESS, self.copy)
    }

    fn open_logs(&self) -> Result<(), String> {
        let log_dir = self.root.join("logs");
        fs::create_dir_all(&log_dir).map_err(|error| error.to_string())?;
        open_target(log_dir.as_os_str().to_string_lossy().as_ref(), self.copy)
    }

    fn install_skins(&self) -> Result<String, String> {
        let payload = self.root.join(skin_payload_name());
        if !payload.is_file() {
            return Err(format!("{}: {}", self.copy.missing_host, payload.display()));
        }
        let status = Command::new(payload)
            .arg(self.root.join("web").join("dist"))
            .current_dir(&self.root)
            .status()
            .map_err(|error| error.to_string())?;
        status
            .success()
            .then(|| self.copy.skin_installed.to_string())
            .ok_or_else(|| self.copy.skin_failed.to_string())
    }
}

impl Drop for ServiceController {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[derive(Clone)]
enum Message {
    Start,
    Stop,
    Restart,
    OpenWeb,
    OpenLogs,
    InstallSkins,
    Refresh,
    Quit,
}

struct State {
    controller: Arc<Mutex<ServiceController>>,
    copy: Copy,
    running: bool,
    status: String,
}

impl State {
    fn new(controller: Arc<Mutex<ServiceController>>) -> Self {
        let copy = localized_copy();
        let running = controller
            .lock()
            .map(|mut controller| controller.is_running())
            .unwrap_or(false);
        Self {
            controller,
            copy,
            running,
            status: if running {
                copy.running.to_string()
            } else {
                copy.stopped.to_string()
            },
        }
    }

    fn refresh(&mut self) {
        self.running = self
            .controller
            .lock()
            .map(|mut controller| controller.is_running())
            .unwrap_or(false);
    }

    fn run(&mut self, action: impl FnOnce(&mut ServiceController) -> Result<String, String>) {
        let result = self
            .controller
            .lock()
            .map_err(|_| self.copy.lock_error.to_string())
            .and_then(|mut controller| action(&mut controller));
        self.status = match result {
            Ok(message) => message,
            Err(error) => error,
        };
        self.refresh();
    }
}

fn view(state: &State) -> Element<Message> {
    let running = if state.running {
        state.copy.running
    } else {
        state.copy.stopped
    };
    column([
        text("DeepSeek Harness-rs"),
        text(format!("{} · {running}", state.copy.subtitle)),
        text(ADDRESS),
        row([
            button(state.copy.start)
                .enabled(!state.running)
                .on_click(Message::Start),
            button(state.copy.stop)
                .enabled(state.running)
                .on_click(Message::Stop),
            button(state.copy.restart)
                .enabled(state.running)
                .on_click(Message::Restart),
        ])
        .gap(Dp::new(8.0)),
        row([
            button(state.copy.open_web).on_click(Message::OpenWeb),
            button(state.copy.open_logs).on_click(Message::OpenLogs),
            button(state.copy.install_skins).on_click(Message::InstallSkins),
        ])
        .gap(Dp::new(8.0)),
        row([
            button(state.copy.refresh).on_click(Message::Refresh),
            button(state.copy.quit).on_click(Message::Quit),
        ])
        .gap(Dp::new(8.0)),
        text(format!("{}: {}", state.copy.status, state.status)),
    ])
    .gap(Dp::new(12.0))
    .padding(Dp::new(20.0))
}

fn update(state: &mut State, message: Message, cx: &mut UpdateContext<'_>) {
    match message {
        Message::Start => state.run(|controller| {
            controller.start()?;
            Ok(controller.copy.done.to_string())
        }),
        Message::Stop => state.run(|controller| {
            controller.stop()?;
            Ok(controller.copy.done.to_string())
        }),
        Message::Restart => state.run(|controller| {
            controller.restart()?;
            Ok(controller.copy.done.to_string())
        }),
        Message::OpenWeb => state.run(|controller| {
            controller.open_web()?;
            Ok(controller.copy.done.to_string())
        }),
        Message::OpenLogs => state.run(|controller| {
            controller.open_logs()?;
            Ok(controller.copy.done.to_string())
        }),
        Message::InstallSkins => state.run(|controller| controller.install_skins()),
        Message::Refresh => {
            state.refresh();
            state.status = if state.running {
                state.copy.running.to_string()
            } else {
                state.copy.stopped.to_string()
            };
        }
        Message::Quit => cx.quit(),
    }
}

fn port_is_open(port: u16) -> bool {
    ("127.0.0.1", port)
        .to_socket_addrs()
        .ok()
        .into_iter()
        .flatten()
        .any(|address| TcpStream::connect_timeout(&address, Duration::from_millis(150)).is_ok())
}

fn core_executable_name() -> &'static str {
    if cfg!(windows) {
        "deepseek-harness-rs.exe"
    } else {
        "deepseek-harness-rs"
    }
}

fn skin_payload_name() -> &'static str {
    if cfg!(windows) {
        "deepseek-harness-rs-skin.exe"
    } else {
        "deepseek-harness-rs-skin"
    }
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

#[cfg(test)]
mod tests {
    use super::{chinese_copy, english_copy};

    #[test]
    fn launcher_has_complete_chinese_and_english_copy() {
        let zh = chinese_copy();
        let en = english_copy();
        assert_eq!(zh.start, "启动");
        assert_eq!(zh.open_web, "打开网页");
        assert_eq!(en.start, "Start");
        assert_eq!(en.open_web, "Open Web");
    }
}

fn main() -> Result<(), zsui::stable::Error> {
    let controller = Arc::new(Mutex::new(ServiceController::discover().unwrap_or_else(
        |_error| ServiceController {
            root: Path::new(".").to_path_buf(),
            executable: Path::new(core_executable_name()).to_path_buf(),
            child: None,
            copy: localized_copy(),
        },
    )));
    let copy = localized_copy();
    window(copy.title)
        .app_name("DeepSeek Harness-rs")
        .size(600, 390)
        .min_size(540, 350)
        .resizable(false)
        .release_view_when_hidden()
        .stateful(State::new(controller), view, update)
        .run()
}
