//! Adaptive chooser of the directory-picker seam: resolves the host's
//! situation once at boot (bind host, SSH launch, display session, Linux
//! chooser binary) to a concrete backend kind. Rust port of
//! `packages/host/directory-picker-auto` (resolve.ts + probe.ts).
//!
//! # Deviations
//!
//! - The TS plugin mounts the chosen backend + client surface as runtime
//!   Loader entries; the Rust composition is static, so this crate owns the
//!   pure decision ([`resolve_directory_picker_backend`]) and the host app
//!   composes the matching backend crate directly.

use std::collections::HashMap;
use std::path::Path;

/// Concrete interaction backend the resolver chooses between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryPickerBackendKind {
    Native,
    Browse,
}

impl DirectoryPickerBackendKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Browse => "browse",
        }
    }
}

/// Effective webserver bind host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindHost {
    Loopback,
    AllInterfaces,
}

/// Host facts the backend choice is a pure function of, sampled once at
/// boot.
#[derive(Debug, Clone)]
pub struct DirectoryPickerHostFacts {
    /// Effective webserver bind host.
    pub bind_host: BindHost,
    /// Host process platform (`darwin` | `win32` | `linux` | ...).
    pub platform: String,
    /// Environment sample; SSH marks a remote operator,
    /// DISPLAY/WAYLAND_DISPLAY a Linux display.
    pub env: HashMap<String, String>,
    /// Whether a Linux chooser binary the native backend can drive
    /// (zenity/kdialog) is on PATH; consulted only when `platform` is
    /// linux.
    pub linux_chooser: bool,
}

/// An env value counts only when set and non-blank (an empty export is
/// "unset" by shell convention).
fn present(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

/// Resolve which backend serves this boot. `native` requires every signal
/// that the operator can see the host display and the native backend can
/// serve it: a loopback-only bind, no SSH launch, and a servable display
/// session — assumed on darwin/win32, requiring `DISPLAY`/`WAYLAND_DISPLAY`
/// plus a chooser binary on linux, and never true elsewhere. Anything
/// ambiguous resolves to `browse`, which works everywhere.
pub fn resolve_directory_picker_backend(
    facts: &DirectoryPickerHostFacts,
) -> DirectoryPickerBackendKind {
    if facts.bind_host != BindHost::Loopback {
        return DirectoryPickerBackendKind::Browse;
    }
    if present(facts.env.get("SSH_CONNECTION").map(String::as_str))
        || present(facts.env.get("SSH_TTY").map(String::as_str))
    {
        return DirectoryPickerBackendKind::Browse;
    }
    if facts.platform == "darwin" || facts.platform == "win32" {
        return DirectoryPickerBackendKind::Native;
    }
    if facts.platform != "linux" || !facts.linux_chooser {
        return DirectoryPickerBackendKind::Browse;
    }
    if present(facts.env.get("DISPLAY").map(String::as_str))
        || present(facts.env.get("WAYLAND_DISPLAY").map(String::as_str))
    {
        DirectoryPickerBackendKind::Native
    } else {
        DirectoryPickerBackendKind::Browse
    }
}

/// The chooser binaries the native backend can drive on Linux (zenity,
/// KDialog fallback).
pub const LINUX_CHOOSER_BINARIES: [&str; 2] = ["zenity", "kdialog"];

/// Whether the current process may execute the candidate path.
pub fn can_execute(candidate: &str) -> bool {
    let path = Path::new(candidate);
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        // Windows has no X_OK distinction; file existence is the probe.
        true
    }
}

/// Scan a PATH value for one of the native backend's Linux chooser
/// binaries. The executability predicate is injectable for deterministic
/// tests.
pub fn has_linux_chooser_binary(
    path_value: Option<&str>,
    is_executable: &dyn Fn(&str) -> bool,
) -> bool {
    for dir in path_value
        .unwrap_or_default()
        .split_terminator(if cfg!(windows) { ';' } else { ':' })
    {
        if dir.is_empty() {
            continue;
        }
        for name in LINUX_CHOOSER_BINARIES {
            if is_executable(&Path::new(dir).join(name).to_string_lossy()) {
                return true;
            }
        }
    }
    false
}
