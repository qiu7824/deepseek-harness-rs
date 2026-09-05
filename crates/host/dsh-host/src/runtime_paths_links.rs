//! Retain only already-existing installation links while moving user data.
//! New Rust distributions no longer create these legacy fallback slots.

use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Default)]
pub(super) struct ManagedModuleLinks {
    modules: PathBuf,
    current: BTreeMap<String, PathBuf>,
}

fn version_name(version: &str) -> bool {
    if version.len() > 96
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".+-".contains(&b))
    {
        return false;
    }
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<_> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn portable_slot(name: &str) -> bool {
    let Some(body) = name.strip_prefix("deepseek-harness-rs-v") else {
        return false;
    };
    for platform in [
        "windows-x86_64",
        "linux-x86_64",
        "macos-x86_64",
        "macos-aarch64",
    ] {
        for variant in ["core", "skin", "free"] {
            if let Some(version) = body.strip_suffix(&format!("-{platform}-{variant}")) {
                if version_name(version) {
                    return true;
                }
            }
        }
    }
    false
}

fn portable_target(name: &str, target: &Path) -> bool {
    let manifest_path = target.join("PACKAGE.json");
    let Ok(metadata) = fs::symlink_metadata(&manifest_path) else {
        return false;
    };
    if !metadata.file_type().is_file() || metadata.len() > 65536 {
        return false;
    }
    let Ok(bytes) = fs::read(&manifest_path) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    let read = |key| manifest.get(key).and_then(Value::as_str);
    let (Some(version), Some(platform), Some(arch), Some(variant)) = (
        read("version"),
        read("platform"),
        read("arch"),
        read("variant"),
    ) else {
        return false;
    };
    if read("name") != Some(name)
        || !portable_slot(name)
        || name != format!("deepseek-harness-rs-v{version}-{platform}-{arch}-{variant}")
    {
        return false;
    }
    let suffix = if platform == "windows" { ".exe" } else { "" };
    let host = format!("deepseek-harness-rs{suffix}");
    let launcher = format!("dsh-launcher{suffix}");
    if read("host") != Some(host.as_str()) || read("entry") != Some(launcher.as_str()) {
        return false;
    }
    [host, launcher].iter().all(|name| {
        fs::symlink_metadata(target.join(name))
            .is_ok_and(|metadata| metadata.file_type().is_file() && metadata.len() > 0)
    })
}

impl ManagedModuleLinks {
    pub(super) fn resolve(home: &Path, anchor: Option<&Path>) -> Result<Self, String> {
        let mut current = BTreeMap::new();
        if let Some(anchor) = anchor {
            for (name, target) in dsh_app_boot::profiles_module_fallback_targets(anchor)? {
                current.insert(
                    name,
                    fs::canonicalize(target).map_err(|error| error.to_string())?,
                );
            }
        }
        Ok(Self {
            modules: home.join("profiles").join("node_modules"),
            current,
        })
    }

    /// Return true only for an installation-owned link that was reconstructed,
    /// or an obsolete dangling portable slot deliberately omitted from the copy.
    pub(super) fn copy_link(&self, source: &Path, destination: &Path) -> Result<bool, String> {
        let Ok(relative) = source.strip_prefix(&self.modules) else {
            return Ok(false);
        };
        let parts: Vec<_> = relative.components().collect();
        if parts.is_empty()
            || parts
                .iter()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Ok(false);
        }
        let name = relative.to_string_lossy().replace('\\', "/");
        if let Some(expected) = self.current.get(&name) {
            let actual = fs::canonicalize(source)
                .map_err(|error| format!("当前安装模块链接失效 {}：{error}", source.display()))?;
            if &actual != expected {
                return Err(format!("安装模块链接目标不匹配：{}", source.display()));
            }
            dsh_app_boot::recreate_profiles_module_fallback_link(destination, expected)?;
            return Ok(true);
        }
        // Only a direct, strictly named portable-product slot is an obsolete
        // version candidate. Unknown scoped packages and arbitrary links fail.
        if parts.len() != 1 || !portable_slot(&name) {
            return Ok(false);
        }
        match fs::canonicalize(source) {
            Ok(target) if portable_target(&name, &target) => {
                dsh_app_boot::recreate_profiles_module_fallback_link(destination, &target)?;
                Ok(true)
            }
            Ok(_) => Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(error) => Err(format!(
                "无法核验旧安装模块链接 {}：{error}",
                source.display()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_paths::{RuntimePaths, hash, write_json};
    use serde_json::json;

    fn base() -> PathBuf {
        std::env::temp_dir().join(format!("dsh-packaged-links-{}", uuid::Uuid::new_v4()))
    }

    fn package(root: &Path, version: &str) -> (PathBuf, String) {
        let platform = if cfg!(windows) {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        };
        let arch = if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x86_64"
        };
        let name = format!("deepseek-harness-rs-v{version}-{platform}-{arch}-core");
        let directory = root.join(&name);
        fs::create_dir_all(&directory).unwrap();
        let suffix = if cfg!(windows) { ".exe" } else { "" };
        let host = format!("deepseek-harness-rs{suffix}");
        let entry = format!("dsh-launcher{suffix}");
        let manifest = json!({"name":name,"version":version,"platform":platform,"arch":arch,"variant":"core","host":host,"entry":entry});
        write_json(&directory.join("PACKAGE.json"), &manifest).unwrap();
        // app-boot's Node anchor is lowercase; PACKAGE.json is the portable
        // release manifest. Windows aliases them, Unix retains both.
        write_json(&directory.join("package.json"), &manifest).unwrap();
        fs::write(directory.join(host), "installed host bytes").unwrap();
        fs::write(directory.join(entry), "installed launcher bytes").unwrap();
        (directory, name)
    }

    fn stage_home(home: &Path) {
        RuntimePaths::prepare_with_install_anchor(home, None)
            .unwrap()
            .release();
        fs::write(home.join("retained.txt"), "user data").unwrap();
    }

    fn legacy_link(home: &Path, installation: &Path, name: &str) {
        dsh_app_boot::recreate_profiles_module_fallback_link(
            &home.join("profiles/node_modules").join(name),
            installation,
        )
        .unwrap();
    }

    #[test]
    fn rust_package_boot_does_not_mix_installation_inventory_with_node_modules() {
        let root = base();
        let home = root.join("home");
        let (installation, _) = if let Some(path) = std::env::var_os("DSH_PACKAGED_FIXTURE") {
            let path = PathBuf::from(path);
            let value: Value =
                serde_json::from_slice(&fs::read(path.join("PACKAGE.json")).unwrap()).unwrap();
            (path, value["name"].as_str().unwrap().to_string())
        } else {
            package(&root, "0.1.3-alpha.3")
        };
        dsh_app_boot::heal_profiles_module_fallback(&installation.join("PACKAGE.json"), &home)
            .unwrap();
        assert!(!home.join("profiles/node_modules").exists());
        assert!(
            dsh_app_boot::profiles_module_fallback_targets(&installation.join("PACKAGE.json"))
                .unwrap()
                .is_empty()
        );
        if root.exists() {
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn packaged_upgrade_preserves_both_old_and_current_installation_links() {
        let root = base();
        let home = root.join("home");
        let target = root.join("moved");
        stage_home(&home);
        let (old, old_name) = package(&root, "0.1.2-alpha.4");
        let (current, current_name) = if let Some(path) = std::env::var_os("DSH_PACKAGED_FIXTURE") {
            let path = PathBuf::from(path);
            let manifest: Value =
                serde_json::from_slice(&fs::read(path.join("PACKAGE.json")).unwrap()).unwrap();
            (path, manifest["name"].as_str().unwrap().to_string())
        } else {
            package(&root, "0.1.3-alpha.3")
        };
        let current_anchor = current.join("PACKAGE.json");
        // Reproduce data produced by an earlier release, not new boot behavior.
        legacy_link(&home, &old, &old_name);
        legacy_link(&home, &current, &current_name);
        let host_name = if cfg!(windows) {
            "deepseek-harness-rs.exe"
        } else {
            "deepseek-harness-rs"
        };
        let original_host = hash(&current.join(host_name)).unwrap();
        write_json(
            &home.join("settings.json"),
            &json!({"storage-paths":{"dataDirectory":target}}),
        )
        .unwrap();
        let migrated =
            RuntimePaths::prepare_with_install_anchor(&home, Some(&current_anchor)).unwrap();
        assert!(
            migrated.migration_error.is_none(),
            "{:?}",
            migrated.migration_error
        );
        for (name, installation) in [(&old_name, &old), (&current_name, &current)] {
            let relative = Path::new("profiles/node_modules").join(name);
            assert!(
                fs::symlink_metadata(target.join(&relative))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(
                fs::canonicalize(target.join(&relative)).unwrap(),
                fs::canonicalize(installation).unwrap()
            );
            assert_eq!(
                fs::canonicalize(home.join(&relative)).unwrap(),
                fs::canonicalize(installation).unwrap()
            );
        }
        assert_eq!(
            fs::read_to_string(target.join("retained.txt")).unwrap(),
            "user data"
        );
        assert_eq!(hash(&current.join(host_name)).unwrap(), original_host);
        migrated.release();
        RuntimePaths::prepare_with_install_anchor(&home, Some(&current_anchor))
            .unwrap()
            .release();
        fs::remove_dir_all(&home).unwrap();
        fs::remove_dir_all(&target).unwrap();
        assert_eq!(
            hash(&current.join(host_name)).unwrap(),
            original_host,
            "removing migration trees must not traverse installation links"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dangling_old_portable_slot_is_omitted_but_unknown_dangling_link_is_rejected() {
        let root = base();
        let home = root.join("home");
        let target = root.join("moved");
        stage_home(&home);
        let (old, old_name) = package(&root, "0.1.2-alpha.4");
        let (current, current_name) = package(&root, "0.1.3-alpha.3");
        let anchor = current.join("package.json");
        legacy_link(&home, &old, &old_name);
        legacy_link(&home, &current, &current_name);
        fs::remove_dir_all(old).unwrap();
        write_json(
            &home.join("settings.json"),
            &json!({"storage-paths":{"dataDirectory":target}}),
        )
        .unwrap();
        let migrated = RuntimePaths::prepare_with_install_anchor(&home, Some(&anchor)).unwrap();
        assert!(
            migrated.migration_error.is_none(),
            "{:?}",
            migrated.migration_error
        );
        assert!(fs::symlink_metadata(home.join("profiles/node_modules").join(&old_name)).is_ok());
        assert!(
            fs::symlink_metadata(target.join("profiles/node_modules").join(&old_name)).is_err()
        );
        assert_eq!(
            fs::canonicalize(target.join("profiles/node_modules").join(&current_name)).unwrap(),
            fs::canonicalize(&current).unwrap()
        );
        migrated.release();

        let outside = root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let unknown = target.join("profiles/node_modules/arbitrary-user-module");
        dsh_app_boot::recreate_profiles_module_fallback_link(&unknown, &outside).unwrap();
        fs::remove_dir_all(&outside).unwrap();
        let next_target = root.join("must-not-be-published");
        write_json(
            &target.join("settings.json"),
            &json!({"storage-paths":{"dataDirectory":next_target}}),
        )
        .unwrap();
        let recovered = RuntimePaths::prepare_with_install_anchor(&target, Some(&anchor)).unwrap();
        assert!(recovered.migration_error.is_some());
        assert_eq!(
            recovered.paths["dataDirectory"],
            fs::canonicalize(&target).unwrap()
        );
        assert!(!next_target.exists());
        recovered.release();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_slot_cannot_be_redirected_to_an_unrelated_target() {
        let root = base();
        let home = root.join("home");
        let target = root.join("moved");
        stage_home(&home);
        let (current, name) = package(&root, "0.1.3-alpha.3");
        let anchor = current.join("package.json");
        legacy_link(&home, &current, &name);
        let outside = root.join("private");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "must not be copied").unwrap();
        dsh_app_boot::recreate_profiles_module_fallback_link(
            &home.join("profiles/node_modules").join(name),
            &outside,
        )
        .unwrap();
        write_json(
            &home.join("settings.json"),
            &json!({"storage-paths":{"dataDirectory":target}}),
        )
        .unwrap();
        let recovered = RuntimePaths::prepare_with_install_anchor(&home, Some(&anchor)).unwrap();
        assert!(
            recovered
                .migration_error
                .as_deref()
                .is_some_and(|error| error.contains("符号链接") || error.contains("目标不匹配"))
        );
        assert!(!target.exists());
        assert_eq!(
            fs::read_to_string(outside.join("secret.txt")).unwrap(),
            "must not be copied"
        );
        recovered.release();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn portable_slot_namespace_is_strict() {
        assert!(portable_slot(
            "deepseek-harness-rs-v0.1.2-alpha.4-windows-x86_64-core"
        ));
        for invalid in [
            "some-module",
            "deepseek-harness-rs-v../private-windows-x86_64-core",
            "deepseek-harness-rs-vlatest-windows-x86_64-core",
            "deepseek-harness-rs-v1.2.3-unknown-x86_64-core",
            "deepseek-harness-rs-v1.2.3-windows-x86_64-other",
        ] {
            assert!(!portable_slot(invalid), "{invalid}");
        }
    }
}
