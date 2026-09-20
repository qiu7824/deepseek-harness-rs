//! Discover installed Rust toolchains without executing a host-side PATH candidate.
use anyhow::{Result, bail, ensure};
use std::path::{Path, PathBuf};

pub fn real_profile() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            System::Com::CoTaskMemFree,
            UI::Shell::{FOLDERID_Profile, SHGetKnownFolderPath},
        };
        let mut pointer = std::ptr::null_mut();
        ensure!(
            unsafe { SHGetKnownFolderPath(&FOLDERID_Profile, 0, 0, &mut pointer) } >= 0,
            "cannot resolve OS user profile"
        );
        let mut length = 0;
        while unsafe { *pointer.add(length) } != 0 {
            length += 1;
        }
        let path = PathBuf::from(String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(pointer, length)
        }));
        unsafe {
            CoTaskMemFree(pointer.cast());
        }
        Ok(path)
    }
    #[cfg(not(windows))]
    {
        Ok(PathBuf::from(std::env::var("HOME")?))
    }
}

/// Restore broker-internal directory variables from OS-known folders before
/// the engine uses them for helper materialization or setup files.
pub fn restore_broker_folders(temp: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            System::Com::CoTaskMemFree,
            UI::Shell::{
                FOLDERID_LocalAppData, FOLDERID_Profile, FOLDERID_ProgramData,
                FOLDERID_RoamingAppData, SHGetKnownFolderPath,
            },
        };
        for (name, id) in [
            ("USERPROFILE", FOLDERID_Profile),
            ("LOCALAPPDATA", FOLDERID_LocalAppData),
            ("APPDATA", FOLDERID_RoamingAppData),
            ("ProgramData", FOLDERID_ProgramData),
        ] {
            let mut pointer = std::ptr::null_mut();
            ensure!(
                unsafe { SHGetKnownFolderPath(&id, 0, 0, &mut pointer) } >= 0,
                "cannot resolve broker folder {name}"
            );
            let mut length = 0;
            while unsafe { *pointer.add(length) } != 0 {
                length += 1;
            }
            let value =
                String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(pointer, length) });
            unsafe {
                CoTaskMemFree(pointer.cast());
                std::env::set_var(name, value);
            }
        }
    }
    // Called on the single main thread, before constructing a Tokio runtime.
    unsafe {
        std::env::set_var("TEMP", temp);
        std::env::set_var("TMP", temp);
    }
    Ok(())
}

fn text(path: &Path) -> Result<String> {
    ensure!(
        path.metadata()?.len() <= 64 * 1024,
        "toolchain selection exceeds size limit"
    );
    Ok(std::fs::read_to_string(path)?)
}

pub fn resolve(
    profile: &Path,
    workspace: &Path,
    explicit: Option<&str>,
) -> Result<Option<PathBuf>> {
    let home = profile.join(".rustup");
    let settings: Option<toml::Value> = if home.join("settings.toml").is_file() {
        Some(toml::from_str(&text(&home.join("settings.toml"))?)?)
    } else {
        None
    };
    let mut selected = explicit.map(str::to_string);
    if selected.is_none() {
        selected = std::env::var("RUSTUP_TOOLCHAIN")
            .ok()
            .filter(|s| !s.is_empty());
    }
    if selected.is_none() {
        for directory in workspace.ancestors() {
            if let Some(overrides) = settings
                .as_ref()
                .and_then(|s| s.get("overrides"))
                .and_then(toml::Value::as_table)
            {
                let current = std::fs::canonicalize(directory)
                    .ok()
                    .map(|p| p.to_string_lossy().to_lowercase());
                selected = overrides
                    .iter()
                    .find(|(path, _)| {
                        current.is_some()
                            && std::fs::canonicalize(path)
                                .ok()
                                .map(|p| p.to_string_lossy().to_lowercase())
                                == current
                    })
                    .and_then(|(_, value)| value.as_str())
                    .map(str::to_owned);
                if selected.is_some() {
                    break;
                }
            }
            let manifest = directory.join("rust-toolchain.toml");
            let legacy = directory.join("rust-toolchain");
            let config = if manifest.is_file() {
                Some((manifest, true))
            } else if legacy.is_file() {
                Some((legacy, false))
            } else {
                None
            };
            if let Some((path, is_toml)) = config {
                let value = text(&path)?;
                if is_toml || value.trim_start().starts_with('[') {
                    let parsed: toml::Value = toml::from_str(&value)?;
                    if let Some(path) = parsed
                        .get("toolchain")
                        .and_then(|t| t.get("path"))
                        .and_then(toml::Value::as_str)
                    {
                        selected = Some(directory.join(path).to_string_lossy().into_owned());
                    } else {
                        selected = parsed
                            .get("toolchain")
                            .and_then(|t| t.get("channel"))
                            .and_then(toml::Value::as_str)
                            .map(str::to_owned);
                    }
                    ensure!(selected.is_some(), "toolchain file has no channel or path");
                } else {
                    selected = Some(value.trim().to_owned());
                }
                break;
            }
        }
    }
    if selected.is_none() {
        selected = settings
            .as_ref()
            .and_then(|s| s.get("default_toolchain"))
            .and_then(toml::Value::as_str)
            .map(str::to_owned);
    }
    let Some(selected) = selected else {
        return Ok(None);
    };
    ensure!(
        !selected.is_empty() && !selected.contains(['\0', '\r', '\n']),
        "invalid toolchain selection"
    );
    let path = PathBuf::from(&selected);
    let candidates = if path.is_absolute() {
        vec![path]
    } else {
        ensure!(
            !selected.contains(['\\', '/']),
            "toolchain name must not be a relative path"
        );
        vec![
            home.join("toolchains").join(&selected),
            home.join("toolchains").join(format!(
                "{selected}-{}-pc-windows-msvc",
                std::env::consts::ARCH
            )),
        ]
    };
    for root in candidates {
        if root.join("bin/cargo.exe").is_file() && root.join("bin/rustc.exe").is_file() {
            return Ok(Some(root));
        }
    }
    bail!("[TOOLCHAIN_UNAVAILABLE] selected Rust toolchain is not installed: {selected}")
}
