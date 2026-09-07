//! cgroup v2 ownership, then a user-systemd scope, then the explicit process
//! group fallback. Membership is installed before the command executes.
use parking_lot::Mutex;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub(crate) enum LinuxScope {
    Cgroup {
        path: PathBuf,
        attach: File,
    },
    Systemd {
        runner: String,
        unit: String,
        state: Mutex<ScopeState>,
    },
}

pub(crate) struct ScopeState {
    path: Option<PathBuf>,
    last_probe: Option<Instant>,
    alive: Option<bool>,
}

fn run_bounded(program: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut text = String::new();
                child
                    .stdout
                    .take()?
                    .take(16 * 1024)
                    .read_to_string(&mut text)
                    .ok()?;
                return Some(text);
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn cgroup_path(relative: &str) -> Option<PathBuf> {
    let relative = relative.strip_prefix('/')?;
    if relative.split('/').any(|part| part == ".." || part == ".") {
        return None;
    }
    Some(Path::new("/sys/fs/cgroup").join(relative))
}

fn populated(path: &Path) -> Option<bool> {
    match std::fs::read_to_string(path.join("cgroup.events")) {
        Ok(text) => text.lines().find_map(|line| match line {
            "populated 0" => Some(false),
            "populated 1" => Some(true),
            _ => None,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

/// Resolve the original command as the child will see it, before replacing
/// argv[0] with a scope launcher. Existence alone is not execute permission.
fn resolve_program(
    program: &str,
    cwd: &str,
    environment: &[(String, String)],
) -> Result<String, String> {
    let fail =
        |error: std::io::Error| format!("subprocess-local: failed to spawn {program:?}: {error}");
    let cwd = if Path::new(cwd).is_absolute() {
        PathBuf::from(cwd)
    } else {
        std::env::current_dir().map_err(&fail)?.join(cwd)
    };
    let metadata = std::fs::metadata(&cwd).map_err(&fail)?;
    if !metadata.is_dir() {
        return Err(fail(std::io::Error::from_raw_os_error(libc::ENOTDIR)));
    }
    let candidates = if program.contains('/') {
        vec![if Path::new(program).is_absolute() {
            PathBuf::from(program)
        } else {
            cwd.join(program)
        }]
    } else {
        let path = environment
            .iter()
            .rev()
            .find(|(key, _)| key == "PATH")
            .map(|(_, value)| value.clone())
            .unwrap_or_else(default_search_path);
        std::env::split_paths(&path)
            .map(|path| {
                if path.is_absolute() {
                    path.join(program)
                } else {
                    cwd.join(path).join(program)
                }
            })
            .collect()
    };
    let mut denied = false;
    for candidate in candidates {
        let metadata = match std::fs::metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                denied = true;
                continue;
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    || error.raw_os_error() == Some(libc::ENOTDIR) =>
            {
                continue;
            }
            Err(error) => return Err(fail(error)),
        };
        if !metadata.is_file() {
            denied = true;
            continue;
        }
        let path = std::ffi::CString::new(candidate.as_os_str().as_bytes())
            .map_err(|_| fail(std::io::Error::from(std::io::ErrorKind::InvalidInput)))?;
        if unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), libc::X_OK, libc::AT_EACCESS) }
            == 0
        {
            return candidate
                .into_os_string()
                .into_string()
                .map_err(|_| "subprocess-local: executable path is not UTF-8".into());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            denied = true;
        } else if error.kind() != std::io::ErrorKind::NotFound {
            return Err(fail(error));
        }
    }
    Err(fail(std::io::Error::from_raw_os_error(if denied {
        libc::EACCES
    } else {
        libc::ENOENT
    })))
}

fn default_search_path() -> String {
    let length = unsafe { libc::confstr(libc::_CS_PATH, std::ptr::null_mut(), 0) };
    if length == 0 || length > 64 * 1024 {
        return "/bin:/usr/bin".into();
    }
    let mut bytes = vec![0u8; length];
    let actual = unsafe { libc::confstr(libc::_CS_PATH, bytes.as_mut_ptr().cast(), bytes.len()) };
    if actual == 0 || actual > bytes.len() {
        return "/bin:/usr/bin".into();
    }
    String::from_utf8_lossy(&bytes[..actual.saturating_sub(1)]).into_owned()
}

impl LinuxScope {
    pub(crate) fn prepare() -> Option<Self> {
        let name = format!("dsh-{}", uuid::Uuid::new_v4().simple());
        let native = || -> Option<Self> {
            let membership = std::fs::read_to_string("/proc/self/cgroup").ok()?;
            let current = membership
                .lines()
                .find_map(|line| line.strip_prefix("0::"))?;
            let path = cgroup_path(current)?.join(&name);
            std::fs::create_dir(&path).ok()?;
            match OpenOptions::new()
                .write(true)
                .open(path.join("cgroup.procs"))
            {
                Ok(attach) => Some(Self::Cgroup { path, attach }),
                Err(_) => {
                    let _ = std::fs::remove_dir(path);
                    None
                }
            }
        };
        if let Some(scope) = native() {
            return Some(scope);
        }
        static SYSTEMD_RUNNER: OnceLock<Option<String>> = OnceLock::new();
        let runner = SYSTEMD_RUNNER.get_or_init(|| {
            let cwd = std::env::current_dir()
                .ok()?
                .into_os_string()
                .into_string()
                .ok()?;
            let runner =
                resolve_program("systemd-run", &cwd, &dsh_subprocess::scrubbed_parent_env())
                    .ok()?;
            // A live user bus alone is insufficient: successfully starting a
            // trivial transient scope proves that this manager accepts scopes.
            let probe = format!("dsh-probe-{}.scope", uuid::Uuid::new_v4().simple());
            run_bounded(
                &runner,
                &[
                    "--user",
                    "--scope",
                    "--quiet",
                    "--collect",
                    "--unit",
                    &probe,
                    "--",
                    "/bin/true",
                ],
            )
            .map(|_| runner)
        });
        runner.as_ref().map(|runner| Self::Systemd {
            runner: runner.clone(),
            unit: format!("{name}.scope"),
            state: Mutex::new(ScopeState {
                path: None,
                last_probe: None,
                alive: None,
            }),
        })
    }

    pub(crate) fn argv(
        &self,
        argv: &[String],
        cwd: &str,
        environment: &[(String, String)],
    ) -> Result<Vec<String>, String> {
        match self {
            Self::Cgroup { .. } => Ok(argv.to_vec()),
            Self::Systemd { runner, unit, .. } => {
                let program = argv.first().ok_or("subprocess-local: empty command")?;
                let resolved = resolve_program(program, cwd, environment)?;
                Ok([
                    runner.as_str(),
                    "--user",
                    "--scope",
                    "--quiet",
                    "--collect",
                    "--unit",
                    unit,
                    "--",
                ]
                .into_iter()
                .map(str::to_owned)
                .chain(std::iter::once(resolved))
                .chain(argv.iter().skip(1).cloned())
                .collect())
            }
        }
    }

    pub(crate) fn configure(&self, command: &mut tokio::process::Command) {
        if let Self::Cgroup { attach, .. } = self {
            let fd = attach.as_raw_fd();
            unsafe {
                command.pre_exec(move || {
                    // Only async-signal-safe write is used after fork. Writing
                    // zero moves the writing process, before it can fork.
                    if libc::write(fd, b"0".as_ptr().cast(), 1) == 1 {
                        Ok(())
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                });
            }
        }
    }

    pub(crate) fn alive(&self, child_exited: bool) -> Option<bool> {
        match self {
            Self::Cgroup { path, .. } => populated(path),
            Self::Systemd { unit, state, .. } => {
                let mut state = state.lock();
                if let Some(path) = &state.path {
                    return populated(path);
                }
                if state
                    .last_probe
                    .is_some_and(|last| last.elapsed() < Duration::from_millis(250))
                {
                    return state.alive;
                }
                state.last_probe = Some(Instant::now());
                let text = run_bounded(
                    "systemctl",
                    &[
                        "--user",
                        "show",
                        "--property=ControlGroup",
                        "--property=ActiveState",
                        "--property=LoadState",
                        unit,
                    ],
                )?;
                for line in text.lines() {
                    if let Some(group) = line.strip_prefix("ControlGroup=") {
                        if !group.is_empty() {
                            state.path = cgroup_path(group);
                        }
                    }
                }
                state.alive = if let Some(path) = &state.path {
                    populated(path)
                } else if child_exited
                    && text.lines().any(|line| {
                        matches!(
                            line,
                            "ActiveState=inactive" | "ActiveState=failed" | "LoadState=not-found"
                        )
                    })
                {
                    Some(false)
                } else {
                    Some(true)
                };
                state.alive
            }
        }
    }

    pub(crate) fn signal(&self, signal: i32) -> bool {
        match self {
            Self::Cgroup { path, .. } => {
                if signal == libc::SIGKILL && std::fs::write(path.join("cgroup.kill"), "1").is_ok()
                {
                    return true;
                }
                signal_cgroup(path, signal)
            }
            Self::Systemd { unit, .. } => {
                let signal = format!("--signal={signal}");
                run_bounded(
                    "systemctl",
                    &["--user", "kill", "--kill-who=all", &signal, unit],
                )
                .is_some()
            }
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Cgroup { .. } => "cgroup-v2",
            Self::Systemd { .. } => "systemd-user-scope",
        }
    }
}

fn signal_cgroup(path: &Path, signal: i32) -> bool {
    let Ok(procs) = std::fs::read_to_string(path.join("cgroup.procs")) else {
        return false;
    };
    let mut success = true;
    for pid in procs
        .lines()
        .filter_map(|line| line.parse::<i32>().ok())
        .filter(|pid| *pid > 0)
    {
        if unsafe { libc::kill(pid, signal) } != 0
            && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
        {
            success = false;
        }
    }
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                success &= signal_cgroup(&entry.path(), signal);
            }
        }
    }
    success
}

impl Drop for LinuxScope {
    fn drop(&mut self) {
        // rmdir is deliberately non-recursive: failure must leave an owned
        // live group intact instead of deleting evidence of unfinished cleanup.
        if let Self::Cgroup { path, .. } = self {
            let _ = std::fs::remove_dir(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "dsh-linux-resolve-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn file(&self, relative: &str, content: &str, mode: u32) -> PathBuf {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, content).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            path
        }
        fn cwd(&self) -> &str {
            self.0.to_str().unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if self
                .0
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("dsh-linux-resolve-"))
            {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn command_resolution_uses_child_cwd_path_and_execute_permission() {
        let fixture = Fixture::new();
        fixture.file("denied/probe", "#!/bin/sh\nexit 0\n", 0o644);
        let executable = fixture.file("bin/probe", "#!/bin/sh\nexit 0\n", 0o755);
        let env = vec![("PATH".into(), "denied:bin".into())];
        assert_eq!(
            resolve_program("probe", fixture.cwd(), &env).unwrap(),
            executable.to_str().unwrap()
        );
        assert_eq!(
            resolve_program("./bin/probe", fixture.cwd(), &[]).unwrap(),
            fixture.0.join("./bin/probe").to_str().unwrap()
        );
        assert!(resolve_program("missing-command", fixture.cwd(), &env).is_err());
        assert!(resolve_program("./denied/probe", fixture.cwd(), &env).is_err());
        assert!(
            resolve_program(
                "probe",
                fixture.0.join("missing-cwd").to_str().unwrap(),
                &env
            )
            .is_err()
        );
        let here = fixture.file("probe", "#!/bin/sh\nexit 0\n", 0o755);
        assert_eq!(
            resolve_program("probe", fixture.cwd(), &[("PATH".into(), String::new())]).unwrap(),
            here.to_str().unwrap()
        );
        assert!(
            resolve_program("sh", fixture.cwd(), &[]).is_ok(),
            "unset PATH uses the platform default, not an empty child PATH"
        );
    }

    #[test]
    fn absolute_scope_runner_survives_a_replaced_child_path() {
        let fixture = Fixture::new();
        // A stand-in launcher exercises argv/PATH mechanics without claiming
        // that a user-systemd manager exists on the test host.
        fixture.file("runner", "#!/bin/sh\nwhile [ \"$#\" -gt 0 ] && [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"\n", 0o755);
        fixture.file("bin/target", "#!/bin/sh\nprintf original-command\n", 0o755);
        let runner = resolve_program("./runner", fixture.cwd(), &[]).unwrap();
        let scope = LinuxScope::Systemd {
            runner: runner.clone(),
            unit: "fixture.scope".into(),
            state: Mutex::new(ScopeState {
                path: None,
                last_probe: None,
                alive: None,
            }),
        };
        let child_env = vec![("PATH".into(), "bin".into())];
        let argv = scope
            .argv(&["target".into()], fixture.cwd(), &child_env)
            .unwrap();
        assert_eq!(argv[0], runner);
        assert!(Path::new(&argv[0]).is_absolute());
        let output = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&fixture.0)
            .env_clear()
            .envs(child_env)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"original-command");
        assert!(
            scope
                .argv(&["missing-command".into()], fixture.cwd(), &[])
                .is_err()
        );
    }
}
