//! Shared remote-environment scrubbing for E2B process launchers.
//! Rust port of `environment.ts`.
//!
//! # Deviations
//!
//! - The base64 line-shape validation collapses into strict re-encode
//!   comparison (same acceptance as the TS `BASE64` regex).
//! - Maps keep insertion order via `IndexMap` (the TS `Map` contract).

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use dsh_e2b::{E2bCommandOptions, E2bSandbox, E2bSdkError, e2b_control_envs};
use dsh_subprocess::sensitive_env_pattern;
use indexmap::IndexMap;

/// Whether one line is canonical base64 (the TS `BASE64` regex contract).
fn is_base64_line(line: &str) -> bool {
    if line.is_empty() || !line.is_ascii() {
        return false;
    }
    match base64::engine::general_purpose::STANDARD.decode(line) {
        Ok(bytes) => base64::engine::general_purpose::STANDARD.encode(&bytes) == line,
        Err(_) => false,
    }
}

/// Parse a NUL-delimited remote environment into ordered entries
/// (TS `remoteEnvironmentEntries`).
fn remote_environment_entries(raw: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    for entry in raw.split('\0') {
        if entry.is_empty() {
            continue;
        }
        let Some(separator) = entry.find('=') else {
            continue;
        };
        if separator == 0 {
            continue;
        }
        entries.push((
            entry[..separator].to_string(),
            entry[separator + 1..].to_string(),
        ));
    }
    entries
}

/// Read the remote environment through ASCII base64 so SDK callback
/// chunking cannot corrupt UTF-8 (TS `readRemoteEnvironment`).
pub async fn read_remote_environment(
    sandbox: &Arc<dyn E2bSandbox>,
    signal: Option<&dsh_subprocess::SubprocessAbort>,
) -> Result<String, String> {
    let result = sandbox
        .run(
            "set -o pipefail; dsh_e2b_passwd=\"$(getent passwd \"$(id -u)\")\"; IFS=: read -r _ _ _ _ _ dsh_e2b_home _ <<<\"$dsh_e2b_passwd\"; test -n \"$dsh_e2b_home\" -a -d \"$dsh_e2b_home\"; printf '%s' \"$dsh_e2b_home\" | base64 -w 0; printf '\\n'; env -0 | base64 -w 0",
            &E2bCommandOptions {
                envs: Some(e2b_control_envs(HashMap::new())),
                signal: signal.cloned(),
                ..Default::default()
            },
        )
        .await
        .map_err(|error| format!("subprocess-e2b: remote environment probe failed: {error}"))?;
    let lines: Vec<&str> = result.stdout.trim().split('\n').collect();
    if lines.len() != 2 || !lines.iter().all(|line| is_base64_line(line)) {
        return Err(
            "subprocess-e2b: remote environment transport returned invalid base64".to_string(),
        );
    }
    let home = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(lines[0])
            .map_err(|error| format!("subprocess-e2b: home base64: {error}"))?,
    )
    .map_err(|error| format!("subprocess-e2b: remote home is not valid UTF-8: {error}"))?;
    let raw = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(lines[1])
            .map_err(|error| format!("subprocess-e2b: environment base64: {error}"))?,
    )
    .map_err(|error| format!("subprocess-e2b: remote environment is not valid UTF-8: {error}"))?;
    if !home.starts_with('/') || home.contains('\0') {
        return Err(format!(
            "subprocess-e2b: remote login home is invalid: {home:?}"
        ));
    }
    let mut environment: IndexMap<String, String> =
        remote_environment_entries(&raw).into_iter().collect();
    environment.insert("HOME".to_string(), home);
    Ok(environment
        .into_iter()
        .map(|(name, value)| format!("{name}={value}\0"))
        .collect())
}

/// Parse an E2B NUL-delimited environment while removing harness-private
/// and credential-shaped names (TS `scrubRemoteEnvironment`).
pub fn scrub_remote_environment(raw: &str) -> IndexMap<String, String> {
    let mut environment = IndexMap::new();
    for (name, value) in remote_environment_entries(raw) {
        if name.starts_with("DSH_") || sensitive_env_pattern().is_match(&name) {
            continue;
        }
        environment.insert(name, value);
    }
    environment
}

/// Isolate E2B's fixed login-shell bootstrap from user profiles and ambient
/// credentials (TS `bootstrapEnvironment`).
pub fn bootstrap_environment(raw: &str) -> HashMap<String, String> {
    let mut environment = HashMap::new();
    environment.insert("TERM".to_string(), "dumb".to_string());
    for (name, _value) in remote_environment_entries(raw) {
        if name.starts_with("DSH_") || sensitive_env_pattern().is_match(&name) {
            environment.insert(name, String::new());
        }
    }
    environment
}

/// Overlay explicit entries and serialize one validated E2B environment
/// (TS `serializeRemoteEnvironment`).
pub fn serialize_remote_environment(
    raw: &str,
    explicit: Option<&[(String, Option<String>)]>,
) -> Result<String, String> {
    let mut environment = scrub_remote_environment(raw);
    for (name, value) in explicit.unwrap_or_default() {
        if name.is_empty()
            || name.contains('=')
            || name.contains('\0')
            || value.as_deref().is_some_and(|value| value.contains('\0'))
        {
            return Err("subprocess-e2b: environment entries require non-empty NUL-free names without = and NUL-free values".to_string());
        }
        match value {
            // An explicit tombstone removes the ambient entry.
            None => {
                environment.shift_remove(name);
            }
            Some(value) => {
                environment.insert(name.clone(), value.clone());
            }
        }
    }
    Ok(environment
        .into_iter()
        .map(|(name, value)| format!("{name}={value}\0"))
        .collect())
}

/// Whether one background command outcome is the tolerated "groups already
/// gone" failure (TS `signalRemoteGroups`' tolerance contract).
pub fn tolerated_teardown_error(error: &E2bSdkError) -> bool {
    matches!(
        error.kind,
        dsh_e2b::E2bSdkErrorKind::CommandExit { .. } | dsh_e2b::E2bSdkErrorKind::NotFound
    )
}
