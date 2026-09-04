//! Profile discovery, initialization, and patch-layer composition for the
//! `dsh --profile` launcher family. Rust port of the core of
//! `packages/boot/app-boot/src/profile.ts`.
//!
//! # Deviations
//!
//! - Bundle module resolution (the TS two-anchor Node resolution) collapses
//!   to a local-directory resolution: `dsh.profile.bundles` lists directory
//!   paths or registry names the caller resolves; the launcher heals the
//!   flat fallback when the node_modules milestone lands.
//! - The pnpm workspace scaffolding writes the same file shape but the
//!   Rust launcher forwards plugin management later (the plugin-forwarding
//!   milestone).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Weak};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod shipped_registry;

/// Directory under the Harness home holding every profile.
pub const PROFILES_DIR: &str = "profiles";

/// The user patch layer inside a profile directory.
pub const PROFILE_PATCH_FILENAME: &str = "cordis.patch.yml";

/// Runtime patch applied to entries (the include crate's `PatchOptions`).
pub type PatchOptions = IndexMap<String, Value>;

/// The bundle half of the `dsh` manifest section.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DshBundleManifest {
    /// The patch layer this bundle exports, relative to its package root.
    pub patch: String,
}

/// The profile half of the `dsh` manifest section.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DshProfileManifest {
    /// Ordered bundle layer list (package names or directory paths).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundles: Option<Vec<String>>,
}

/// The profile-launcher slice of the `dsh`-owned package.json section.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DshManifestSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<DshBundleManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<DshProfileManifest>,
}

/// The slice of package.json both profiles and bundles use.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileManifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub dependencies: IndexMap<String, String>,
    #[serde(
        rename = "peerDependencies",
        default,
        skip_serializing_if = "IndexMap::is_empty"
    )]
    pub peer_dependencies: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsh: Option<DshManifestSection>,
}

/// One resolved bundle layer of a profile.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileLayer {
    /// The bundle's listed name or path.
    pub package_name: String,
    /// Absolute directory of the resolved bundle package.
    pub package_dir: PathBuf,
    /// Absolute path of the bundle's patch file.
    pub patch_path: PathBuf,
    /// The parsed patch list.
    pub patches: Vec<PatchOptions>,
}

/// A loaded profile: resolved bundle layers plus the user's own patch layer.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// The profile name (its directory basename).
    pub name: String,
    /// Absolute profile directory.
    pub dir: PathBuf,
    /// Bundle layers in `dsh.profile.bundles` order.
    pub layers: Vec<ProfileLayer>,
    /// Absolute path of the profile's own patch file.
    pub patch_path: PathBuf,
    /// The profile's own patches; empty when the file is absent.
    pub patches: Vec<PatchOptions>,
}

/// Resolve a profile's directory under the Harness home.
pub fn resolve_profile_dir(name: &str, home: &Path) -> Result<PathBuf, String> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name == "."
        || name == ".."
        || name == "node_modules"
    {
        return Err(format!("dsh: invalid profile name {name:?}"));
    }
    Ok(home.join(PROFILES_DIR).join(name))
}

/// The shipped profile templates auto-initialized on first use, by name.
pub fn profile_templates()
-> &'static std::collections::HashMap<&'static str, &'static [&'static str]> {
    static TEMPLATES: std::sync::OnceLock<
        std::collections::HashMap<&'static str, &'static [&'static str]>,
    > = std::sync::OnceLock::new();
    TEMPLATES.get_or_init(|| {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "web",
            &["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"][..],
        );
        map.insert(
            "headless",
            &["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-headless"][..],
        );
        map
    })
}

/// The bundle list a `dsh plugin` init uses for a name with no shipped
/// template.
pub const DEFAULT_PROFILE_BUNDLES: &[&str] = &["@deepseek-ai/dsh-base"];

const PROFILE_PATCH_TEMPLATE: &str = "# Your patch layer for this dsh profile, applied after every bundle layer:\n\
# a top-level YAML array of loader patch entries (id-targeted config\n\
# overrides, disables, and insert lists).\n\
[]\n";

const PROFILE_PNPM_WORKSPACE: &str =
    "packages:\n  - .\n\nnodeLinker: hoisted\nautoInstallPeers: false\n";

/// Initialize a profile directory: manifest, empty user patch layer, and the
/// pnpm settings out-of-tree plugins need. Existing files are never touched.
pub fn init_profile(dir: &Path, bundles: &[String]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let manifest_path = dir.join("package.json");
    if !manifest_path.exists() {
        let manifest = serde_json::json!({
            "name": format!("dsh-profile-{}", dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()),
            "private": true,
            "dependencies": {},
            "dsh": { "profile": { "bundles": bundles } },
        });
        let text = format!(
            "{}\n",
            serde_json::to_string_pretty(&manifest).expect("manifest")
        );
        std::fs::write(&manifest_path, text).map_err(|error| error.to_string())?;
    }
    let patch_path = dir.join(PROFILE_PATCH_FILENAME);
    if !patch_path.exists() {
        std::fs::write(&patch_path, PROFILE_PATCH_TEMPLATE).map_err(|error| error.to_string())?;
    }
    let workspace_path = dir.join("pnpm-workspace.yaml");
    if !workspace_path.exists() {
        std::fs::write(&workspace_path, PROFILE_PNPM_WORKSPACE)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Resolve a package directory using Node's ordinary parent `node_modules`
/// walk from one file anchor. Direct manifest probing works even when an npm
/// exports map hides `./package.json`.
pub fn package_dir_from_anchor(anchor: &Path, package_name: &str) -> Option<PathBuf> {
    if package_name.is_empty()
        || package_name == "."
        || package_name == ".."
        || package_name.contains('\\')
        || (!package_name.starts_with('@') && package_name.contains('/'))
        || (package_name.starts_with('@') && package_name.split('/').count() != 2)
    {
        return None;
    }
    let mut current = if anchor.is_dir() {
        anchor.to_path_buf()
    } else {
        anchor.parent()?.to_path_buf()
    };
    loop {
        let candidate = current.join("node_modules").join(package_name);
        if candidate.join("package.json").is_file() {
            return Some(candidate);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Resolve one profile bundle installation-first, then profile-local.
pub fn resolve_bundle_dir(
    bin_name: &str,
    package_name: &str,
    install_anchor: &Path,
    profile_dir: &Path,
) -> Result<PathBuf, String> {
    package_dir_from_anchor(install_anchor, package_name)
        .or_else(|| package_dir_from_anchor(&profile_dir.join("package.json"), package_name))
        .ok_or_else(|| format!(
            "{bin_name}: cannot resolve profile bundle {package_name:?} from the dsh installation or {}; run 'dsh plugin --profile {} install' if its dependency is not installed",
            profile_dir.display(),
            profile_dir.file_name().map(|name| name.to_string_lossy()).unwrap_or_default()
        ))
}

fn read_profile_manifest(bin_name: &str, dir: &Path) -> Result<ProfileManifest, String> {
    let path = dir.join("package.json");
    let value: Value = serde_json::from_slice(&std::fs::read(&path).map_err(|error| {
        format!(
            "{bin_name}: failed to read profile manifest {}: {error}",
            path.display()
        )
    })?)
    .map_err(|error| {
        format!(
            "{bin_name}: invalid profile manifest {}: {error}",
            path.display()
        )
    })?;
    if !value.is_object() {
        return Err(format!(
            "{bin_name}: profile manifest {} must hold a JSON object",
            path.display()
        ));
    }
    serde_json::from_value(value).map_err(|error| {
        format!(
            "{bin_name}: invalid profile manifest {}: {error}",
            path.display()
        )
    })
}

fn valid_package_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('\\')
        && ((!name.starts_with('@') && !name.contains('/'))
            || (name.starts_with('@')
                && name.split('/').count() == 2
                && name
                    .split('/')
                    .all(|part| !part.is_empty() && part != "." && part != "..")))
}

fn link_matches(link: &Path, target: &Path) -> bool {
    #[cfg(windows)]
    {
        std::fs::canonicalize(link).ok() == std::fs::canonicalize(target).ok()
            && std::fs::canonicalize(target).is_ok()
    }
    #[cfg(unix)]
    {
        std::fs::symlink_metadata(link).is_ok_and(|metadata| metadata.file_type().is_symlink())
            && std::fs::read_link(link).ok().as_deref() == Some(target)
    }
}

fn managed_directory_link(link: &Path) -> bool {
    #[cfg(windows)]
    {
        junction::get_target(link).is_ok()
    }
    #[cfg(unix)]
    {
        std::fs::symlink_metadata(link).is_ok_and(|metadata| metadata.file_type().is_symlink())
    }
}

fn ensure_directory_link(link: &Path, target: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(link) {
        Ok(_) if !managed_directory_link(link) => {
            // `junction::create` first creates the directory and then commits
            // its reparse point. A concurrent process can observe that short
            // intermediate state; wait briefly before classifying it as a
            // user-owned real directory.
            #[cfg(windows)]
            for _ in 0..20 {
                std::thread::sleep(std::time::Duration::from_millis(5));
                if managed_directory_link(link) {
                    return if link_matches(link, target) {
                        Ok(())
                    } else {
                        ensure_directory_link(link, target)
                    };
                }
            }
            return Err(format!(
                "dsh: {} exists and is not a managed directory link; remove it so dsh can manage the installation fallback",
                link.display()
            ));
        }
        Ok(_) => {
            if link_matches(link, target) {
                return Ok(());
            }
            #[cfg(windows)]
            // Removing only the reparse data leaves an ordinary directory
            // behind; junction::create then fails with ERROR_ALREADY_EXISTS.
            // RemoveDirectory removes the link itself without its target.
            let removed = std::fs::remove_dir(link);
            #[cfg(unix)]
            let removed = std::fs::remove_file(link);
            if let Err(error) = removed
                && error.kind() != std::io::ErrorKind::NotFound
            {
                return Err(error.to_string());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    #[cfg(windows)]
    let created = junction::create(target, link);
    #[cfg(unix)]
    let created = std::os::unix::fs::symlink(target, link);
    match created {
        Ok(()) => Ok(()),
        Err(error)
            if error.kind() == std::io::ErrorKind::AlreadyExists
                || error.raw_os_error() == Some(183) =>
        {
            for _ in 0..20 {
                if link_matches(link, target) {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(all(test, windows))]
mod directory_link_tests {
    use super::*;

    #[test]
    fn relocation_replaces_junction_without_removing_target_data() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "dsh-module-relocation-{}-{nonce}",
            std::process::id()
        ));
        let old = root.join("old");
        let new = root.join("new");
        let link = root.join("module");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("retained.txt"), "old installation").unwrap();
        std::fs::write(new.join("retained.txt"), "new installation").unwrap();
        ensure_directory_link(&link, &old).unwrap();
        ensure_directory_link(&link, &new).unwrap();
        ensure_directory_link(&link, &new).unwrap();
        assert_eq!(
            std::fs::read_to_string(link.join("retained.txt")).unwrap(),
            "new installation"
        );
        assert_eq!(
            std::fs::read_to_string(old.join("retained.txt")).unwrap(),
            "old installation"
        );
        std::fs::remove_dir(&link).unwrap();
        ensure_directory_link(&link, &old).unwrap();
        std::fs::remove_file(old.join("retained.txt")).unwrap();
        std::fs::remove_dir(&old).unwrap();
        ensure_directory_link(&link, &new).expect("broken managed junction can be retargeted");
        std::fs::remove_dir(&link).unwrap();
        std::fs::create_dir(&link).unwrap();
        std::fs::write(link.join("user.txt"), "user data").unwrap();
        assert!(ensure_directory_link(&link, &new).is_err());
        assert_eq!(
            std::fs::read_to_string(link.join("user.txt")).unwrap(),
            "user data"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Heal `$DSH_HOME/profiles/node_modules` with the app's resolvable dependency
/// and peer-dependency closure. Declared but absent packages are skipped.
pub fn heal_profiles_module_fallback(install_anchor: &Path, home: &Path) -> Result<(), String> {
    let app_dir = install_anchor.parent().ok_or_else(|| {
        format!(
            "dsh: install anchor {} has no parent",
            install_anchor.display()
        )
    })?;
    let app_manifest = read_profile_manifest("dsh", app_dir)?;
    let mut links: IndexMap<String, PathBuf> = IndexMap::new();
    if let Some(name) = app_manifest
        .name
        .as_ref()
        .filter(|name| valid_package_name(name))
    {
        links.insert(name.clone(), app_dir.to_path_buf());
    }
    let mut queue =
        std::collections::VecDeque::from([(install_anchor.to_path_buf(), app_manifest)]);
    while let Some((anchor, manifest)) = queue.pop_front() {
        for name in manifest
            .dependencies
            .keys()
            .chain(manifest.peer_dependencies.keys())
        {
            if !valid_package_name(name) || links.contains_key(name) {
                continue;
            }
            let Some(dir) = package_dir_from_anchor(&anchor, name) else {
                continue;
            };
            let child = read_profile_manifest("dsh", &dir)?;
            links.insert(name.clone(), dir.clone());
            queue.push_back((dir.join("package.json"), child));
        }
    }
    let modules = home.join(PROFILES_DIR).join("node_modules");
    std::fs::create_dir_all(&modules).map_err(|error| error.to_string())?;
    for (name, target) in links {
        ensure_directory_link(&modules.join(name), &target)?;
    }
    Ok(())
}

/// Parse a patch file into entry patches (absent file = empty).
pub fn load_patch_file(path: &Path) -> Result<Vec<PatchOptions>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let value: Value = serde_yaml::from_str(&text)
        .map_err(|error| format!("invalid patch file {}: {error}", path.display()))?;
    let Value::Array(entries) = value else {
        return Err(format!(
            "patch file {} must be a top-level array",
            path.display()
        ));
    };
    entries
        .into_iter()
        .map(|entry| {
            let Value::Object(map) = entry else {
                return Err("patch entries must be objects".to_string());
            };
            let mut patch = IndexMap::new();
            for (key, value) in map {
                patch.insert(key, value);
            }
            Ok(patch)
        })
        .collect()
}

/// Load a profile by name: read the manifest, resolve each bundle layer's
/// patch file (directory-style resolution), and the profile's own patches.
pub fn load_profile(name: &str, home: &Path) -> Result<Profile, String> {
    load_profile_with_user_layer(name, home, true)
}

/// Load a profile; `user_layer` false skips parsing `cordis.patch.yml`
/// entirely (the TS `userLayer` option — the recovery path for a broken
/// user layer).
pub fn load_profile_with_user_layer(
    name: &str,
    home: &Path,
    user_layer: bool,
) -> Result<Profile, String> {
    load_profile_with_anchors(name, home, user_layer, None, "dsh")
}

/// Full profile loader with the TypeScript installation-first/profile-second
/// package-resolution anchors. `install_anchor: None` preserves the shipped
/// registry and legacy directory fallback for embedded/static compositions.
pub fn load_profile_with_anchors(
    name: &str,
    home: &Path,
    user_layer: bool,
    install_anchor: Option<&Path>,
    bin_name: &str,
) -> Result<Profile, String> {
    let dir = resolve_profile_dir(name, home)?;
    let manifest_path = dir.join("package.json");
    let manifest: ProfileManifest = serde_json::from_slice(
        &std::fs::read(&manifest_path)
            .map_err(|error| format!("cannot read {}: {error}", manifest_path.display()))?,
    )
    .map_err(|error| format!("invalid {}: {error}", manifest_path.display()))?;
    let bundles = manifest
        .dsh
        .as_ref()
        .and_then(|section| section.profile.as_ref())
        .and_then(|profile| profile.bundles.as_ref())
        .cloned()
        .unwrap_or_default();
    let mut layers = Vec::new();
    for bundle in bundles {
        let package_dir = if Path::new(&bundle).is_absolute() {
            PathBuf::from(&bundle)
        } else if let Some(anchor) = install_anchor {
            match resolve_bundle_dir(bin_name, &bundle, anchor, &dir) {
                Ok(dir) => dir,
                Err(error) => {
                    if let Some(layer) = shipped_registry::shipped_bundle_layer(&bundle) {
                        layers.push(layer);
                        continue;
                    }
                    return Err(error);
                }
            }
        } else if let Some(layer) = shipped_registry::shipped_bundle_layer(&bundle) {
            layers.push(layer);
            continue;
        } else {
            dir.join(&bundle)
        };
        let manifest_path = package_dir.join("package.json");
        let bundle_manifest: ProfileManifest = serde_json::from_slice(
            &std::fs::read(&manifest_path)
                .map_err(|error| format!("cannot read {}: {error}", manifest_path.display()))?,
        )
        .map_err(|error| format!("invalid {}: {error}", manifest_path.display()))?;
        let patch_rel = bundle_manifest
            .dsh
            .and_then(|section| section.bundle)
            .map(|bundle| bundle.patch)
            .ok_or_else(|| {
                format!(
                    "bundle \"{bundle}\" declares no dsh.bundle.patch in {}",
                    manifest_path.display()
                )
            })?;
        let patch_path = package_dir.join(&patch_rel);
        let patches = load_patch_file(&patch_path)?;
        layers.push(ProfileLayer {
            package_name: bundle,
            package_dir,
            patch_path,
            patches,
        });
    }
    let patch_path = dir.join(PROFILE_PATCH_FILENAME);
    let patches = if user_layer {
        load_patch_file(&patch_path)?
    } else {
        Vec::new()
    };
    Ok(Profile {
        name: name.to_string(),
        dir,
        layers,
        patch_path,
        patches,
    })
}

/// Load an optional patch-list file: a missing file means "no layer"; an
/// unreadable, unparsable, or non-array file fails loud.
pub fn load_optional_patches(
    bin_name: &str,
    file: &Path,
) -> Result<Option<Vec<PatchOptions>>, String> {
    match load_patch_file(file) {
        Ok(patches) => Ok(Some(patches)),
        Err(_) if !file.exists() => Ok(None),
        Err(error) => Err(format!(
            "{bin_name}: failed to read patches {}: {error}",
            file.display()
        )),
    }
}

/// Load a required overlay patch list: a missing file throws (the caller
/// named this file).
pub fn load_overlay_patches(bin_name: &str, file: &Path) -> Result<Vec<PatchOptions>, String> {
    if !file.exists() {
        return Err(format!(
            "{bin_name}: failed to read overlay {}: file not found",
            file.display()
        ));
    }
    load_patch_file(file).map_err(|error| {
        format!(
            "{bin_name}: failed to read overlay {}: {error}",
            file.display()
        )
    })
}

/// Compose the effective entry list exactly as `boot()` would mount it:
/// apply every layer's patches as ONE flattened list through the include's
/// own patch algorithm over an empty entry list.
pub fn compose_entries(layers: &[Vec<PatchOptions>]) -> Result<Vec<Value>, String> {
    let mut flattened: Vec<PatchOptions> = Vec::new();
    for layer in layers {
        flattened.extend(layer.iter().cloned());
    }
    let root: Vec<Value> = Vec::new();
    let mut warnings = Vec::new();
    let entries =
        dsh_cordis_include::apply_entry_patches(&root, Some(&flattened), &mut |warning: &str| {
            warnings.push(warning.to_string());
        });
    let _ = warnings;
    Ok(entries)
}

/// One overlay patch list with the source label printed in dump comments
/// (TS `ConfigDumpLayer`).
#[derive(Debug, Clone)]
pub struct ConfigDumpLayer {
    /// Source name shown in dump comments (a file basename or path).
    pub label: String,
    /// The layer's patches.
    pub patches: Vec<PatchOptions>,
}

/// Compose the effective entry list exactly as `boot()` would mount it and
/// render it as a loadable YAML document with `# ==` comment separators
/// naming the file and patch layers behind each contiguous run of rows (TS
/// `renderConfigDump`).
///
/// Every run is labelled `origin` (the base file, or the layer that
/// appended it) or `origin, patched by ...`; provenance comes from
/// positional diffs of successive prefix snapshots, where each snapshot
/// applies layers 1..k flattened through the include's own patch algorithm
/// (the same single call `boot()` makes). A patch that matches no row is
/// reported through `warn` with its layer label.
pub fn render_config_dump(
    bin_name: &str,
    absolute_config_path: &Path,
    layers: &[ConfigDumpLayer],
    warn: &mut dyn FnMut(&str),
) -> Result<String, String> {
    let content = std::fs::read_to_string(absolute_config_path).map_err(|error| {
        format!(
            "{bin_name}: failed to read config {}: {error}",
            absolute_config_path.display()
        )
    })?;
    let parsed = dsh_cordis_include::yaml::parse_yaml(&content).map_err(|error| {
        format!(
            "{bin_name}: failed to parse config {}: {error}",
            absolute_config_path.display()
        )
    })?;
    let Value::Array(base) = parsed else {
        return Err(format!(
            "{bin_name}: config {} must be a top-level YAML array of entries",
            absolute_config_path.display()
        ));
    };
    let base_label = absolute_config_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| absolute_config_path.display().to_string());
    let snapshot = |count: usize, warnings: &mut Vec<String>| -> Vec<Value> {
        let mut flattened: Vec<PatchOptions> = Vec::new();
        for layer in layers.iter().take(count) {
            flattened.extend(layer.patches.iter().cloned());
        }
        dsh_cordis_include::apply_entry_patches(&base, Some(&flattened), &mut |message: &str| {
            warnings.push(message.to_string());
        })
    };
    let mut previous = base.clone();
    let mut previous_warnings_len = 0usize;
    let mut provenance: Vec<(String, Vec<String>)> = base
        .iter()
        .map(|_| (base_label.clone(), Vec::new()))
        .collect();
    let mut composed = base.clone();
    for (index, layer) in layers.iter().enumerate() {
        let count = index + 1;
        let mut warnings: Vec<String> = Vec::new();
        composed = snapshot(count, &mut warnings);
        for line in warnings.iter().skip(previous_warnings_len) {
            warn(&format!("{bin_name}: [{}] {line}", layer.label));
        }
        let before: Vec<String> = previous
            .iter()
            .map(|entry| serde_json::to_string(entry).unwrap_or_default())
            .collect();
        for (row_index, entry) in composed.iter().enumerate() {
            if row_index >= before.len() {
                provenance.push((layer.label.clone(), Vec::new()));
            } else if serde_json::to_string(entry).unwrap_or_default() != before[row_index]
                && let Some(record) = provenance.get_mut(row_index)
            {
                record.1.push(layer.label.clone());
            }
        }
        previous = composed.clone();
        previous_warnings_len = warnings.len();
    }
    Ok(grouped_dump(&composed, &provenance))
}

/// Render the composed rows grouped under one source-and-patches comment
/// per contiguous run (TS `groupedDump`).
fn grouped_dump(composed: &[Value], provenance: &[(String, Vec<String>)]) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut current_label: Option<String> = None;
    let mut group: Vec<Value> = Vec::new();
    let flush = |label: &Option<String>, group: &mut Vec<Value>, lines: &mut Vec<String>| {
        let Some(label) = label else { return };
        if group.is_empty() {
            return;
        }
        lines.push(format!("# == {label}"));
        let yaml = dsh_cordis_include::yaml::json_to_yaml(&Value::Array(group.clone()));
        let text = serde_yaml::to_string(&yaml).unwrap_or_default();
        lines.push(text.trim_end().to_string());
        group.clear();
    };
    for (index, entry) in composed.iter().enumerate() {
        let record = &provenance[index];
        let label = if record.1.is_empty() {
            record.0.clone()
        } else {
            format!("{}, patched by {}", record.0, record.1.join(", "))
        };
        if Some(&label) != current_label.as_ref() {
            flush(&current_label, &mut group, &mut lines);
            current_label = Some(label);
        }
        group.push(entry.clone());
    }
    flush(&current_label, &mut group, &mut lines);
    if lines.is_empty() {
        return String::new();
    }
    format!("{}\n", lines.join("\n"))
}

/// Audit the settled loader tree: enabled entries must carry a live fiber
/// (TS `assertEntriesLoaded`).
pub fn assert_entries_loaded(
    loader: &dsh_cordis_loader::LoaderService,
    bin_name: &str,
) -> Result<(), String> {
    let failed: Vec<String> = loader
        .tree
        .entries()
        .into_iter()
        .filter(|entry| entry.fiber.lock().is_none() && !entry.disabled().unwrap_or(false))
        .map(|entry| entry.options.lock().name.clone())
        .collect();
    if failed.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{bin_name}: plugin(s) failed to load: {}; Cordis startup failed because these plugin(s) could not be resolved (see the error(s) logged above)",
        failed.join(", ")
    ))
}

/// Audit the settled loader tree: every enabled entry must be active (TS
/// `assertEntriesActivated`; pending entries name their unresolved
/// services).
pub fn assert_entries_activated(
    loader: &dsh_cordis_loader::LoaderService,
    bin_name: &str,
) -> Result<(), String> {
    assert_entries_loaded(loader, bin_name)?;
    let mut failures: Vec<String> = Vec::new();
    for entry in loader.tree.entries() {
        if entry.disabled().unwrap_or(false) {
            continue;
        }
        let Some(fiber) = entry.fiber.lock().clone() else {
            continue;
        };
        let name = entry.options.lock().name.clone();
        match fiber.state() {
            cordis::FiberState::Active => continue,
            cordis::FiberState::Pending => {
                failures.push(format!("{name}: pending (waiting for services: unknown)"));
            }
            other => {
                failures.push(format!("{name}: fiber state {other:?}"));
            }
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    let noun = if failures.len() == 1 {
        "entry"
    } else {
        "entries"
    };
    Err(format!(
        "{bin_name}: {} {noun} did not activate\n{}",
        failures.len(),
        failures.join("\n")
    ))
}

/// Mount one composed entry list onto the loader's root tree (each entry's
/// `name` resolves through the loader's static registry; `cordis:` prefixes
/// strip like the TS import). Import failures propagate the loader's own
/// `failed to import loader entry {id} ({name}): ...` diagnostic (TS
/// `updateError('import', ...)`); the boot wrapper labels them.
pub async fn mount_entries(
    loader: &dsh_cordis_loader::LoaderService,
    entries: &[Value],
) -> Result<(), String> {
    for entry in entries {
        let options: dsh_cordis_loader::EntryOptions = serde_json::from_value(entry.clone())
            .map_err(|error| format!("invalid loader entry: {error}"))?;
        let mounted_id = loader
            .tree
            .create(options, None, None)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(mounted) = loader
            .tree
            .entries()
            .into_iter()
            .find(|entry| entry.options.lock().id == mounted_id)
            && mounted.options.lock().name == "include"
        {
            // The loader tree owns the entry for the lifetime of this
            // composition. Retain only a weak registry pointer; stale entries
            // disappear naturally when the tree drops the entry.
            std::mem::forget(register_root_include(&loader.tree.ctx, &mounted));
        }
    }
    Ok(())
}

/// Boot the loader against a composed entry list and return only after the
/// whole tree settles (the Rust static-registry boot: no config file, no
/// include pipeline — the caller composes entries and the registry resolves
/// names). Both mount and settle failures carry the TS `plugin tree failed
/// to load` label; the final audit rejects fiber-less or never-activating
/// entries.
pub async fn boot(
    bin_name: &str,
    loader: &dsh_cordis_loader::LoaderService,
    entries: &[Value],
) -> Result<(), String> {
    mount_entries(loader, entries)
        .await
        .map_err(|error| format!("{bin_name}: plugin tree failed to load: {error}"))?;
    loader
        .tree
        .await_ready()
        .await
        .map_err(|error| format!("{bin_name}: plugin tree failed to load: {error}"))?;
    assert_entries_activated(loader, bin_name)
}

/// How long [`install_fail_loud`] waits for its `release` hook before
/// exiting anyway (TS `FAIL_LOUD_RELEASE_TIMEOUT_MS`). A wedged disposer
/// must delay the fatal exit, never cancel it.
pub const FAIL_LOUD_RELEASE_TIMEOUT_MS: u64 = 2_000;

/// Process slice the fail-loud handler reports to (TS `FailLoudProcess`;
/// tests inject a fake, the real host forwards stderr and `exit`).
pub trait FailLoudProcess: Send + Sync {
    fn stderr(&self, line: &str);
    fn exit(&self, code: i32);
}

/// Installed fail-loud handler: [`FailLoudGuard::handle`] reports the first
/// late rejection, [`FailLoudGuard::uninstall`] removes it (the TS
/// `installFailLoud` uninstaller).
pub struct FailLoudGuard {
    handle: Arc<dyn Fn(String) + Send + Sync>,
    uninstall: Arc<dyn Fn() + Send + Sync>,
}

impl FailLoudGuard {
    pub fn handle(&self, reason: String) {
        (self.handle)(reason)
    }

    pub fn uninstall(&self) {
        (self.uninstall)()
    }
}

/// Install before boot to turn a late unhandled plugin-init rejection into
/// one labelled stderr diagnostic and `exit(1)` (TS `installFailLoud`).
///
/// A single latch keeps the first rejection the reported one and lets later
/// rejections (including the release's own) fall through to the pending
/// exit; the handler stays installed while the release runs so a second
/// concurrent rejection cannot bypass teardown. The diagnostic is written
/// before the release so a hanging or failing disposer cannot swallow the
/// reason; the release is awaited under [`FAIL_LOUD_RELEASE_TIMEOUT_MS`]
/// and its own failure is swallowed because the pending fatal exit already
/// owns the outcome.
///
/// Rust deviation: the TS `assembledActivationRejections` checkpoint set
/// (rejections already covered by `assertEntriesActivated`) is not wired
/// because the Rust activation audit returns its failure strings directly
/// instead of rethrowing fiber rejections; the handler therefore treats
/// every reason as fatal.
pub fn install_fail_loud(
    bin_name: &str,
    proc: Arc<dyn FailLoudProcess>,
    release: Option<AsyncDisposer>,
) -> FailLoudGuard {
    let installed = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let latch = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let handle = {
        let proc = proc.clone();
        let installed = installed.clone();
        let latch = latch.clone();
        let release = release.clone();
        let bin_name = bin_name.to_string();
        Arc::new(move |reason: String| {
            if !installed.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            // First rejection reports; later ones (teardown's own included)
            // fall through to the pending exit.
            if latch.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            proc.stderr(&format!("{bin_name}: fatal load failure: {reason}\n"));
            let Some(release) = release.clone() else {
                proc.exit(1);
                return;
            };
            let proc = proc.clone();
            std::mem::drop(tokio::spawn(async move {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(FAIL_LOUD_RELEASE_TIMEOUT_MS),
                    release(),
                )
                .await;
                // Release failure is swallowed: the fatal exit owns the outcome.
                proc.exit(1);
            }));
        }) as Arc<dyn Fn(String) + Send + Sync>
    };
    let uninstall = Arc::new(move || {
        installed.store(false, std::sync::atomic::Ordering::SeqCst);
    }) as Arc<dyn Fn() + Send + Sync>;
    FailLoudGuard { handle, uninstall }
}

/// Async disposer returned by watchers (TS `() => Promise<void>`).
pub type AsyncDisposer = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

fn root_include_registry()
-> &'static parking_lot::Mutex<std::collections::HashMap<usize, Weak<dsh_cordis_loader::Entry>>> {
    static REGISTRY: std::sync::OnceLock<
        parking_lot::Mutex<std::collections::HashMap<usize, Weak<dsh_cordis_loader::Entry>>>,
    > = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| parking_lot::Mutex::new(std::collections::HashMap::new()))
}

fn context_identity(ctx: &cordis::Context) -> usize {
    Arc::as_ptr(&ctx.fiber) as usize
}

/// Owner-bound registration of the root Include used by live patch reloads.
pub struct RootIncludeRegistration {
    context: usize,
    entry: Weak<dsh_cordis_loader::Entry>,
}

impl Drop for RootIncludeRegistration {
    fn drop(&mut self) {
        let mut registry = root_include_registry().lock();
        if registry
            .get(&self.context)
            .is_some_and(|current| current.ptr_eq(&self.entry))
        {
            registry.remove(&self.context);
        }
    }
}

pub fn register_root_include(
    ctx: &cordis::Context,
    entry: &Arc<dsh_cordis_loader::Entry>,
) -> RootIncludeRegistration {
    let context = context_identity(ctx);
    let entry = Arc::downgrade(entry);
    root_include_registry()
        .lock()
        .insert(context, entry.clone());
    RootIncludeRegistration { context, entry }
}

pub fn root_include(ctx: &cordis::Context) -> Option<Arc<dsh_cordis_loader::Entry>> {
    root_include_registry()
        .lock()
        .get(&context_identity(ctx))
        .and_then(Weak::upgrade)
}

/// HMR service face used by user-patch watching (TS `hmr.registerConfig`).
pub trait UserPatchWatcher {
    fn register_config(
        &self,
        filename: PathBuf,
        refresh: Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>,
    ) -> Pin<Box<dyn Future<Output = Result<AsyncDisposer, String>> + Send>>;
}

/// Pure patch-composition hook used by the user-patch watcher.
pub type PatchComposer = Arc<dyn Fn(Vec<PatchOptions>) -> Vec<PatchOptions> + Send + Sync>;

/// Options for [`watch_user_patches`] (TS `UserPatchWatchOptions`).
pub struct UserPatchWatchOptions {
    pub bin_name: String,
    pub filename: PathBuf,
    pub compose: Option<PatchComposer>,
}

/// Re-read the user patch layer and transactionally re-apply it to the boot
/// include (the TS `watchUserPatches` refresh callback body): the include's
/// non-patch config is preserved per refresh, the optional layer composes
/// through `compose` (default identity), and the entry's `config` update
/// drives the include's patches-only re-apply.
pub async fn refresh_user_patches(
    bin_name: &str,
    include_entry: &Arc<dsh_cordis_loader::Entry>,
    filename: &Path,
    compose: Option<&(dyn Fn(Vec<PatchOptions>) -> Vec<PatchOptions> + Send + Sync)>,
) -> Result<(), String> {
    // TS: `{ patches: _previousPatches, ...includeConfig }` — re-read the
    // include's non-patch options so a writer updating them between
    // refreshes is not silently reverted.
    let current: dsh_cordis_include::IncludeConfig = serde_json::from_value(
        include_entry
            .options
            .lock()
            .config
            .clone()
            .unwrap_or(Value::Null),
    )
    .map_err(|error| format!("{bin_name}: invalid include config: {error}"))?;
    let user_patches = load_optional_patches(bin_name, filename)?.unwrap_or_default();
    let patches = match compose {
        Some(compose) => compose(user_patches),
        None => user_patches,
    };
    let new_config = dsh_cordis_include::IncludeConfig {
        patches: Some(patches),
        ..current
    };
    let mut update = IndexMap::new();
    update.insert(
        "config".to_string(),
        serde_json::to_value(&new_config)
            .map_err(|error| format!("{bin_name}: cannot serialize include config: {error}"))?,
    );
    include_entry
        .update(update, false)
        .await
        .map_err(|error| format!("{bin_name}: user patch refresh failed: {error}"))
}

/// Watch the user patch layer through Cordis HMR and transactionally
/// re-apply it to the boot include (TS `watchUserPatches`).
///
/// Rust deviation: the root Include entry is passed in directly (the TS
/// `bootstrapIncludes` WeakMap registry belongs to the root-include mount,
/// not yet ported); the HMR service is an injected [`UserPatchWatcher`]
/// because no `dsh-cordis-hmr` crate exists yet. An `INACTIVE_EFFECT`
/// registration failure returns a no-op disposer (the tree disposed while
/// the watcher opened — the app exiting exactly as asked).
pub async fn watch_user_patches(
    include_entry: Arc<dsh_cordis_loader::Entry>,
    options: UserPatchWatchOptions,
    watcher: Option<&dyn UserPatchWatcher>,
) -> Result<AsyncDisposer, String> {
    let UserPatchWatchOptions {
        bin_name,
        filename,
        compose,
    } = options;
    let Some(watcher) = watcher else {
        return Err(format!(
            "{bin_name}: user patch-layer watching requires the Cordis HMR service"
        ));
    };
    let register_filename = filename.clone();
    let refresh: Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync> =
        Arc::new(move || {
            let include_entry = include_entry.clone();
            let bin_name = bin_name.clone();
            let filename = filename.clone();
            let compose = compose.clone();
            Box::pin(async move {
                let compose_fn: Option<
                    &(dyn Fn(Vec<PatchOptions>) -> Vec<PatchOptions> + Send + Sync),
                > = compose.as_ref().map(|f| f.as_ref());
                if let Err(error) =
                    refresh_user_patches(&bin_name, &include_entry, &filename, compose_fn).await
                {
                    tracing::error!("{error}");
                }
            })
        });
    match watcher.register_config(register_filename, refresh).await {
        Ok(disposer) => Ok(disposer),
        Err(code) if code == "INACTIVE_EFFECT" => {
            let noop: AsyncDisposer = Arc::new(|| Box::pin(async {}));
            Ok(noop)
        }
        Err(error) => Err(error),
    }
}

/// TypeScript-compatible context-shaped watcher entry. HMR remains injected
/// in the Rust port; the root Include is resolved from its owner registration.
pub async fn watch_user_patches_from_context(
    ctx: &cordis::Context,
    options: UserPatchWatchOptions,
    watcher: Option<&dyn UserPatchWatcher>,
) -> Result<AsyncDisposer, String> {
    let entry = root_include(ctx).ok_or_else(|| {
        format!(
            "{}: user patch-layer watching requires the root Include entry",
            options.bin_name
        )
    })?;
    watch_user_patches(entry, options, watcher).await
}
