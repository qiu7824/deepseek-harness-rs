//! Only the fresh report emitted by the child we spawned can select a port.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::{ProcessIdentity, inspect_process, same_executable};

const MAX_READY_BYTES: u64 = 4096;

pub struct ReadyDirectory(PathBuf);

impl ReadyDirectory {
    pub fn create(parent: &Path) -> io::Result<Self> {
        fs::create_dir_all(parent)?;
        let path = parent.join(format!(".host-start-{}", uuid::Uuid::new_v4()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self(path))
    }

    pub fn path(&self) -> PathBuf {
        self.0.join("ready.json")
    }
}

impl Drop for ReadyDirectory {
    fn drop(&mut self) {
        // Only this fresh private directory, including an interrupted producer's
        // sibling temp file. Never remove its parent run directory.
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    version: u32,
    pid: u32,
    instance_id: String,
    url: String,
    executable: PathBuf,
    home: PathBuf,
}

#[derive(Debug)]
pub struct ReadyHost {
    pub port: u16,
    pub instance_id: String,
    pub home: PathBuf,
}

pub fn same_home(left: &Path, right: &Path) -> bool {
    match (
        dsh_home_paths::resolve_redirect(left),
        dsh_home_paths::resolve_redirect(right),
    ) {
        (Ok(left), Ok(right)) => same_executable(&left, &right),
        _ => false,
    }
}

pub fn same_path(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => same_executable(&left, &right),
        _ => same_executable(left, right),
    }
}

fn decode(bytes: &[u8], identity: &ProcessIdentity, home: &Path) -> Result<ReadyHost, String> {
    let report: Report = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid Host readiness report: {error}"))?;
    if report.version != 1
        || report.pid != identity.pid
        || report.instance_id.is_empty()
        || report.instance_id.len() > 128
        || !report.executable.is_absolute()
        || !report.home.is_absolute()
        || !same_path(&report.executable, &identity.executable)
        || !same_home(&report.home, home)
    {
        return Err("Host readiness identity does not match the started process".into());
    }
    let url = url::Url::parse(&report.url).map_err(|_| "invalid Host readiness URL")?;
    let port = url
        .port()
        .filter(|port| *port > 0)
        .ok_or("Host readiness has no actual port")?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || ![
            format!("http://127.0.0.1:{port}"),
            format!("http://127.0.0.1:{port}/"),
        ]
        .contains(&report.url)
    {
        return Err("Host readiness URL must be an actual loopback HTTP address".into());
    }
    Ok(ReadyHost {
        port,
        instance_id: report.instance_id,
        home: report.home,
    })
}

fn wait(
    child: &mut Child,
    identity: &ProcessIdentity,
    home: &Path,
    path: &Path,
    timeout: Duration,
) -> Result<ReadyHost, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Err(format!("Host exited before readiness: {status}"));
        }
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.len() > MAX_READY_BYTES {
                    return Err("Host readiness is not a bounded regular file".into());
                }
                let bytes = fs::read(path).map_err(|error| error.to_string())?;
                if bytes.len() as u64 > MAX_READY_BYTES {
                    return Err("Host readiness exceeds its size limit".into());
                }
                let ready = decode(&bytes, identity, home)?;
                if inspect_process(identity.pid).ok().as_ref() != Some(identity) {
                    return Err("Host process identity changed before readiness".into());
                }
                return Ok(ready);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        if Instant::now() >= deadline {
            return Err("Host readiness timeout".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Failed startup owns only this Child handle. Reap it on timeout, early exit,
/// or a forged/malformed report before the caller can publish ownership.
pub fn wait_owned(
    child: &mut Child,
    identity: &ProcessIdentity,
    home: &Path,
    path: &Path,
    timeout: Duration,
) -> Result<ReadyHost, String> {
    let result = wait(child, identity, home, path, timeout);
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> ProcessIdentity {
        ProcessIdentity {
            pid: 42,
            creation_time: 123,
            executable: std::env::current_exe().unwrap(),
        }
    }

    fn report(port: u16) -> serde_json::Value {
        serde_json::json!({"version":1,"pid":42,"instanceId":"test-instance",
            "url":format!("http://127.0.0.1:{port}"),
            "executable":identity().executable,"home":std::env::temp_dir()})
    }

    #[test]
    fn readiness_accepts_distinct_os_assigned_ports_without_using_the_legacy_listener() {
        let first = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let second = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let ports = [
            first.local_addr().unwrap().port(),
            second.local_addr().unwrap().port(),
        ];
        assert_ne!(ports[0], ports[1]);
        for port in ports {
            let ready = decode(
                &serde_json::to_vec(&report(port)).unwrap(),
                &identity(),
                &std::env::temp_dir(),
            )
            .unwrap();
            assert_eq!(ready.port, port);
        }
    }

    #[test]
    fn readiness_rejects_wrong_pid_home_executable_instance_and_foreign_urls() {
        for (key, value) in [
            ("version", serde_json::json!(2)),
            ("pid", serde_json::json!(43)),
            ("instanceId", serde_json::json!("")),
            ("executable", serde_json::json!("relative")),
            ("home", serde_json::json!("wrong-home")),
        ] {
            let mut invalid = report(41321);
            invalid[key] = value;
            assert!(
                decode(
                    &serde_json::to_vec(&invalid).unwrap(),
                    &identity(),
                    &std::env::temp_dir()
                )
                .is_err(),
                "{key}"
            );
        }
        for url in [
            "http://127.0.0.1:0",
            "http://localhost:41321",
            "http://127.1:41321",
            "https://127.0.0.1:41321",
            "http://evil.test:41321",
            "http://user@127.0.0.1:41321",
            "http://127.0.0.1:41321/api",
            "http://127.0.0.1:41321/?x=1",
            "http://127.0.0.1:41321/#x",
        ] {
            let mut invalid = report(41321);
            invalid["url"] = url.into();
            assert!(
                decode(
                    &serde_json::to_vec(&invalid).unwrap(),
                    &identity(),
                    &std::env::temp_dir()
                )
                .is_err(),
                "{url}"
            );
        }
        assert!(decode(b"{", &identity(), &std::env::temp_dir()).is_err());
    }

    #[test]
    fn readiness_accepts_only_a_validated_migration_of_the_expected_home() {
        let root =
            std::env::temp_dir().join(format!("dsh-ready-redirect-{}", uuid::Uuid::new_v4()));
        let source = root.join("source");
        let target = root.join("actual with spaces");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(target.join("settings.json"), "{}").unwrap();
        fs::write(
            source.join(".dsh-home-redirect.json"),
            serde_json::to_vec(&serde_json::json!({"target":target})).unwrap(),
        )
        .unwrap();
        let mut actual = report(41321);
        actual["home"] = target.to_string_lossy().into_owned().into();
        assert!(decode(&serde_json::to_vec(&actual).unwrap(), &identity(), &source).is_ok());
        fs::remove_file(target.join("settings.json")).unwrap();
        assert!(decode(&serde_json::to_vec(&actual).unwrap(), &identity(), &source).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn each_start_uses_a_fresh_private_directory_and_cleans_its_report() {
        let parent =
            std::env::temp_dir().join(format!("dsh-launcher-ready-{}", uuid::Uuid::new_v4()));
        let first = ReadyDirectory::create(&parent).unwrap();
        let second = ReadyDirectory::create(&parent).unwrap();
        assert_ne!(first.path(), second.path());
        let path = first.path();
        fs::write(&path, b"{}").unwrap();
        fs::write(first.0.join(".dsh-ready-interrupted.tmp"), b"partial").unwrap();
        drop(first);
        assert!(!path.exists());
        assert!(!path.parent().unwrap().exists());
        drop(second);
        fs::remove_dir(parent).unwrap();
    }

    // The same executable exercises OS process identity and an actual bound
    // socket without needing a built Host, GUI, or a shell-specific fixture.
    #[test]
    fn host_fixture_entry() {
        let Ok(path) = std::env::var("DSH_LAUNCHER_READY_FIXTURE_PATH") else {
            return;
        };
        let mode = std::env::var("DSH_LAUNCHER_READY_FIXTURE_MODE").unwrap();
        if mode == "exit" {
            let exit = PathBuf::from(&path).with_extension("exit");
            while !exit.exists() {
                std::thread::sleep(Duration::from_millis(10));
            }
            return;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        if mode != "timeout" {
            let bytes = if mode == "malformed" {
                b"{".to_vec()
            } else {
                serde_json::to_vec(&serde_json::json!({
                    "version":1,"pid":std::process::id() + u32::from(mode == "wrong-pid"),
                    "instanceId":uuid::Uuid::new_v4().to_string(),"url":format!("http://127.0.0.1:{port}"),
                    "executable":std::env::current_exe().unwrap(),
                    "home":fs::canonicalize(std::env::temp_dir()).unwrap(),
                }))
                .unwrap()
            };
            let temp = PathBuf::from(&path).with_extension("tmp");
            fs::write(&temp, bytes).unwrap();
            fs::rename(temp, path).unwrap();
        }
        // Keep the listener and child alive until the parent explicitly stops it.
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    struct FixtureChild(Child);
    impl Drop for FixtureChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn fixture(mode: &str, path: &Path) -> (FixtureChild, ProcessIdentity) {
        let executable = std::env::current_exe().unwrap();
        let child = std::process::Command::new(&executable)
            .args([
                "--exact",
                "readiness::tests::host_fixture_entry",
                "--nocapture",
            ])
            .env("DSH_LAUNCHER_READY_FIXTURE_PATH", path)
            .env("DSH_LAUNCHER_READY_FIXTURE_MODE", mode)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let identity = super::super::wait_for_process_identity(child.id(), &executable).unwrap();
        if mode == "exit" {
            fs::write(path.with_extension("exit"), b"exit").unwrap();
        }
        (FixtureChild(child), identity)
    }

    #[test]
    fn two_live_instances_receive_distinct_ports_while_58080_is_occupied() {
        let legacy = std::net::TcpListener::bind("127.0.0.1:58080");
        if let Err(error) = &legacy {
            assert!(matches!(
                error.kind(),
                io::ErrorKind::AddrInUse | io::ErrorKind::PermissionDenied
            ));
        }
        let parent = std::env::temp_dir().join(format!("dsh-ready-live-{}", uuid::Uuid::new_v4()));
        let first = ReadyDirectory::create(&parent).unwrap();
        let second = ReadyDirectory::create(&parent).unwrap();
        let (mut one, identity_one) = fixture("ready", &first.path());
        let (mut two, identity_two) = fixture("ready", &second.path());
        let ready_one = wait_owned(
            &mut one.0,
            &identity_one,
            &std::env::temp_dir(),
            &first.path(),
            Duration::from_secs(5),
        )
        .unwrap();
        let ready_two = wait_owned(
            &mut two.0,
            &identity_two,
            &std::env::temp_dir(),
            &second.path(),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_ne!(ready_one.port, ready_two.port);
        assert_ne!(ready_one.port, 58080);
        assert_ne!(ready_two.port, 58080);
        assert_ne!(identity_one.pid, identity_two.pid);
        assert_ne!(ready_one.instance_id, ready_two.instance_id);
        drop((one, two, first, second));
        fs::remove_dir(parent).unwrap();
    }

    #[test]
    fn startup_failure_reaps_only_the_started_child_and_allows_a_fresh_start() {
        let parent =
            std::env::temp_dir().join(format!("dsh-ready-failure-{}", uuid::Uuid::new_v4()));
        for mode in ["wrong-pid", "malformed", "timeout", "exit"] {
            let directory = ReadyDirectory::create(&parent).unwrap();
            let (mut child, identity) = fixture(mode, &directory.path());
            let timeout = if mode == "timeout" {
                Duration::from_millis(150)
            } else {
                Duration::from_secs(5)
            };
            assert!(
                wait_owned(
                    &mut child.0,
                    &identity,
                    &std::env::temp_dir(),
                    &directory.path(),
                    timeout
                )
                .is_err(),
                "{mode}"
            );
            assert!(
                child.0.try_wait().unwrap().is_some(),
                "{mode} child must be reaped"
            );
        }
        let directory = ReadyDirectory::create(&parent).unwrap();
        let (mut child, identity) = fixture("ready", &directory.path());
        assert!(
            wait_owned(
                &mut child.0,
                &identity,
                &std::env::temp_dir(),
                &directory.path(),
                Duration::from_secs(5)
            )
            .is_ok()
        );
        drop((child, directory));
        fs::remove_dir(parent).unwrap();
    }

    #[test]
    fn an_owned_host_can_be_stopped_and_restarted_with_a_fresh_process_identity() {
        let parent =
            std::env::temp_dir().join(format!("dsh-ready-restart-{}", uuid::Uuid::new_v4()));
        let first = ReadyDirectory::create(&parent).unwrap();
        let (mut child, identity) = fixture("ready", &first.path());
        let ready = wait_owned(
            &mut child.0,
            &identity,
            &std::env::temp_dir(),
            &first.path(),
            Duration::from_secs(5),
        )
        .unwrap();
        super::super::stop_process(&identity).unwrap();
        child.0.wait().unwrap();
        super::super::wait_for_process_exit(&identity).unwrap();
        drop((child, first));
        let second = ReadyDirectory::create(&parent).unwrap();
        let (mut restarted, new_identity) = fixture("ready", &second.path());
        let new_ready = wait_owned(
            &mut restarted.0,
            &new_identity,
            &std::env::temp_dir(),
            &second.path(),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_ne!(identity.pid, new_identity.pid);
        assert_ne!(ready.instance_id, new_ready.instance_id);
        drop((restarted, second));
        fs::remove_dir(parent).unwrap();
    }
}
