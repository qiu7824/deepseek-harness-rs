//! One-shot, process-bound readiness for desktop launchers. Ordinary stdout
//! remains human-readable; consumers read only their own fresh private file.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn take_ready_file(
    profile: &str,
    args: &[String],
) -> Result<(Vec<String>, Option<PathBuf>, Option<PathBuf>), String> {
    let mut rest = Vec::new();
    let mut path = None;
    let mut log = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        if flag != "--ready-file" && flag != "--stdio-log" {
            rest.push(args[index].clone());
            index += 1;
            continue;
        }
        if profile != "web"
            || (flag == "--ready-file" && path.is_some())
            || (flag == "--stdio-log" && log.is_some())
        {
            return Err(format!("dsh: {flag} is accepted once by the web profile"));
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("dsh: {flag} needs an absolute path"))?;
        let candidate = PathBuf::from(value);
        if !candidate.is_absolute() || candidate.file_name().is_none() {
            return Err(format!("dsh: {flag} needs an absolute file path"));
        }
        if flag == "--ready-file" && candidate.symlink_metadata().is_ok() {
            return Err("dsh: --ready-file must not already exist".into());
        }
        let parent = candidate
            .parent()
            .ok_or_else(|| format!("dsh: {flag} has no parent"))?;
        if !parent.is_dir() {
            return Err(format!("dsh: {flag} parent directory must already exist"));
        }
        if flag == "--ready-file" {
            path = Some(candidate);
        } else {
            log = Some(candidate);
        }
        index += 2;
    }
    if let Some(log) = &log {
        crate::web_stdio::validate_log_path(log, path.is_some())
            .map_err(|error| format!("dsh: {error}"))?;
    }
    Ok((rest, path, log))
}

/// Publish complete JSON without replacing an existing destination. Publishing a
/// fully-written sibling makes appearance atomic and enforces no-overwrite.
pub fn publish(path: &Path, url: &str, home: &Path) -> Result<(), String> {
    let (pid, instance_id) = dsh_host_apiproxy::host_process_identity();
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let home = fs::canonicalize(home).map_err(|error| error.to_string())?;
    let report = serde_json::json!({
        "version": 1, "pid": pid, "instanceId": instance_id,
        "url": url, "executable": executable, "home": home,
    });
    publish_json(path, &report)
        .map_err(|error| format!("dsh: readiness publication failed: {error}"))
}

fn publish_json(path: &Path, report: &serde_json::Value) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("readiness path has no parent"))?;
    let temp = parent.join(format!(".dsh-ready-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(&serde_json::to_vec(report)?)?;
        file.sync_all()?;
        drop(file);
        crate::web_readiness_publish::publish_no_replace(&temp, path)
    })();
    let _ = fs::remove_file(&temp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_file_is_removed_from_web_args_and_rejects_ambiguous_paths() {
        let root = std::env::temp_dir().join(format!("dsh-readiness-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.join("ready.json").to_string_lossy().into_owned();
        let args = vec![
            "--port".into(),
            "0".into(),
            "--ready-file".into(),
            path.clone(),
        ];
        let (remaining, ready, log) = take_ready_file("web", &args).unwrap();
        assert!(log.is_none());
        assert_eq!(remaining, ["--port", "0"]);
        assert_eq!(ready, Some(PathBuf::from(&path)));
        assert!(take_ready_file("headless", &args).is_err());
        assert!(take_ready_file("web", &["--ready-file".into(), "relative.json".into()]).is_err());
        assert!(take_ready_file("web", &["--ready-file".into()]).is_err());
        let mut duplicate = args.clone();
        duplicate.extend(["--ready-file".into(), path.clone()]);
        assert!(take_ready_file("web", &duplicate).is_err());
        fs::write(&path, "old").unwrap();
        assert!(take_ready_file("web", &args).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn readiness_publication_is_complete_and_never_overwrites_existing_report() {
        let root = std::env::temp_dir().join(format!("dsh-readiness-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.join("ready.json");
        let report = serde_json::json!({"version":1,"pid":42,"url":"http://127.0.0.1:41321"});
        publish_json(&path, &report).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap()).unwrap(),
            report
        );
        assert!(publish_json(&path, &serde_json::json!({"pid":7})).is_err());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap()).unwrap(),
            report
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persistent_stdio_flags_are_paired_and_do_not_reach_the_web_profile() {
        let root = std::env::temp_dir().join(format!("dsh-readiness-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let ready = root.join("ready.json").to_string_lossy().into_owned();
        let log = root.join("host.log").to_string_lossy().into_owned();
        let args = vec![
            "--stdio-log".into(),
            log.clone(),
            "--port".into(),
            "0".into(),
            "--ready-file".into(),
            ready.clone(),
        ];
        let (rest, actual_ready, actual_log) = take_ready_file("web", &args).unwrap();
        assert_eq!(rest, ["--port", "0"]);
        assert_eq!(actual_ready, Some(PathBuf::from(&ready)));
        assert_eq!(actual_log, Some(PathBuf::from(&log)));
        assert!(take_ready_file("web", &["--stdio-log".into(), log.clone()]).is_err());
        let mut duplicate = args.clone();
        duplicate.extend(["--stdio-log".into(), log]);
        assert!(take_ready_file("web", &duplicate).is_err());
        assert!(take_ready_file("headless", &args).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
