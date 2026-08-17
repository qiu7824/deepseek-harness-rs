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
use std::sync::Arc;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
pub fn profile_templates() -> &'static std::collections::HashMap<&'static str, &'static [&'static str]> {
    static TEMPLATES: std::sync::OnceLock<
        std::collections::HashMap<&'static str, &'static [&'static str]>,
    > = std::sync::OnceLock::new();
    TEMPLATES.get_or_init(|| {
        let mut map = std::collections::HashMap::new();
        map.insert("web", &["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"][..]);
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

const PROFILE_PNPM_WORKSPACE: &str = "packages:\n  - .\n\nnodeLinker: hoisted\nautoInstallPeers: false\n";

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
        let text = format!("{}\n", serde_json::to_string_pretty(&manifest).expect("manifest"));
        std::fs::write(&manifest_path, text).map_err(|error| error.to_string())?;
    }
    let patch_path = dir.join(PROFILE_PATCH_FILENAME);
    if !patch_path.exists() {
        std::fs::write(&patch_path, PROFILE_PATCH_TEMPLATE).map_err(|error| error.to_string())?;
    }
    let workspace_path = dir.join("pnpm-workspace.yaml");
    if !workspace_path.exists() {
        std::fs::write(&workspace_path, PROFILE_PNPM_WORKSPACE).map_err(|error| error.to_string())?;
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
        // Directory-style resolution: a bundle may name a directory beside
        // the profile or an absolute path (the TS node resolution arrives
        // with the node_modules milestone).
        let package_dir = if Path::new(&bundle).is_absolute() {
            PathBuf::from(&bundle)
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
pub fn load_optional_patches(bin_name: &str, file: &Path) -> Result<Option<Vec<PatchOptions>>, String> {
    match load_patch_file(file) {
        Ok(patches) => Ok(Some(patches)),
        Err(_) if !file.exists() => Ok(None),
        Err(error) => Err(format!("{bin_name}: failed to read patches {}: {error}", file.display())),
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
    load_patch_file(file)
        .map_err(|error| format!("{bin_name}: failed to read overlay {}: {error}", file.display()))
}

/// Compose the effective entry list exactly as `boot()` would mount it:
/// apply every layer's patches as ONE flattened list through the include's
/// own patch algorithm over an empty entry list.
pub fn compose_entries(
    layers: &[Vec<PatchOptions>],
) -> Result<Vec<Value>, String> {
    let mut flattened: Vec<PatchOptions> = Vec::new();
    for layer in layers {
        flattened.extend(layer.iter().cloned());
    }
    let root: Vec<Value> = Vec::new();
    let mut warnings = Vec::new();
    let entries = dsh_cordis_include::apply_entry_patches(&root, Some(&flattened), &mut |warning: &str| {
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
        format!("{bin_name}: failed to read config {}: {error}", absolute_config_path.display())
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
            } else if serde_json::to_string(entry).unwrap_or_default() != before[row_index] {
                if let Some(record) = provenance.get_mut(row_index) {
                    record.1.push(layer.label.clone());
                }
            }
        }
        previous = composed.clone();
        previous_warnings_len = warnings.len();
    }
    Ok(grouped_dump(&composed, &provenance))
}

/// Render the composed rows grouped under one source-and-patches comment
/// per contiguous run (TS `groupedDump`).
fn grouped_dump(
    composed: &[Value],
    provenance: &[(String, Vec<String>)],
) -> String {
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
        .filter(|entry| {
            entry.fiber.lock().is_none()
                && !entry
                    .disabled()
                    .unwrap_or(false)
        })
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
    let noun = if failures.len() == 1 { "entry" } else { "entries" };
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
        loader
            .tree
            .create(options, None, None)
            .await
            .map_err(|error| error.to_string())?;
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
    release: Option<Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>>,
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
            let _ = tokio::spawn(async move {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(FAIL_LOUD_RELEASE_TIMEOUT_MS),
                    release(),
                )
                .await;
                // Release failure is swallowed: the fatal exit owns the outcome.
                proc.exit(1);
            });
        }) as Arc<dyn Fn(String) + Send + Sync>
    };
    let uninstall = Arc::new(move || {
        installed.store(false, std::sync::atomic::Ordering::SeqCst);
    }) as Arc<dyn Fn() + Send + Sync>;
    FailLoudGuard { handle, uninstall }
}

/// Async disposer returned by watchers (TS `() => Promise<void>`).
pub type AsyncDisposer =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// HMR service face used by user-patch watching (TS `hmr.registerConfig`).
pub trait UserPatchWatcher {
    fn register_config(
        &self,
        filename: PathBuf,
        refresh: Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<AsyncDisposer, String>>
                + Send,
        >,
    >;
}

/// Options for [`watch_user_patches`] (TS `UserPatchWatchOptions`).
pub struct UserPatchWatchOptions {
    pub bin_name: String,
    pub filename: PathBuf,
    pub compose: Option<Arc<dyn Fn(Vec<PatchOptions>) -> Vec<PatchOptions> + Send + Sync>>,
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
    let UserPatchWatchOptions { bin_name, filename, compose } = options;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "dsh-app-boot-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        root
    }

    #[test]
    fn profile_names_are_validated() {
        let home = temp_root("names");
        assert!(resolve_profile_dir("web", &home).is_ok());
        for invalid in ["", "a/b", "a\\b", ".", "..", "node_modules"] {
            assert!(resolve_profile_dir(invalid, &home).is_err(), "{invalid}");
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn init_profile_is_idempotent_and_writes_the_scaffold() {
        let home = temp_root("init");
        let dir = resolve_profile_dir("web", &home).expect("dir");
        init_profile(&dir, &["@deepseek-ai/dsh-base".to_string()]).expect("init");
        assert!(dir.join("package.json").exists());
        assert!(dir.join(PROFILE_PATCH_FILENAME).exists());
        assert!(dir.join("pnpm-workspace.yaml").exists());
        // Re-running never touches existing files.
        let before = std::fs::read(dir.join("package.json")).expect("read");
        init_profile(&dir, &["other".to_string()]).expect("re-init");
        let after = std::fs::read(dir.join("package.json")).expect("read");
        assert_eq!(before, after);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn load_profile_resolves_bundle_layers_and_own_patches() {
        let home = temp_root("load");
        let dir = resolve_profile_dir("web", &home).expect("dir");
        init_profile(&dir, &[]).expect("init");
        // A local bundle directory beside the profile.
        let bundle_dir = dir.join("test-bundle");
        std::fs::create_dir_all(&bundle_dir).expect("bundle dir");
        std::fs::write(
            bundle_dir.join("package.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "name": "test-bundle",
                "dsh": { "bundle": { "patch": "./cordis.patch.yml" } },
            }))
            .expect("manifest"),
        )
        .expect("write");
        std::fs::write(
            bundle_dir.join("cordis.patch.yml"),
            "- id: row-1\n  config: {}\n",
        )
        .expect("patch");
        // Point the profile manifest at the local bundle.
        std::fs::write(
            dir.join("package.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "name": "dsh-profile-web",
                "private": true,
                "dsh": { "profile": { "bundles": ["test-bundle"] } },
            }))
            .expect("profile manifest"),
        )
        .expect("write");

        let profile = load_profile("web", &home).expect("load");
        assert_eq!(profile.name, "web");
        assert_eq!(profile.layers.len(), 1);
        assert_eq!(profile.layers[0].package_name, "test-bundle");
        assert_eq!(profile.layers[0].patches.len(), 1);
        assert_eq!(profile.layers[0].patches[0].get("id").and_then(Value::as_str), Some("row-1"));
        // The empty template patch parses as zero patches.
        assert!(profile.patches.is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn boot_mounts_registered_entries_and_audits_missing_names() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError};
            use std::sync::Arc;

            struct OkPlugin;
            #[async_trait::async_trait]
            impl Plugin for OkPlugin {
                fn name(&self) -> Option<&'static str> {
                    Some("ok-plugin")
                }
                fn inject(&self) -> InjectSpec {
                    InjectSpec::new([])
                }
                async fn apply(&self, _ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
                    Ok(())
                }
            }

            let ctx = Context::root();
            let loader = dsh_cordis_loader::LoaderService::new(&ctx).await;
            ctx.register_service(loader.clone());
            loader.core.register("ok-plugin", Arc::new(OkPlugin));

            // A registered entry boots cleanly.
            let entries = vec![serde_json::json!({ "name": "cordis:ok-plugin" })];
            boot("dsh", &loader, &entries).await.expect("boot");

            // An unregistered name fails with the TS boot chain: the
            // loader's `failed to import loader entry` detail under the
            // `plugin tree failed to load` label.
            let entries = vec![serde_json::json!({ "name": "cordis:no-such-plugin" })];
            let error = boot("dsh", &loader, &entries).await.expect_err("fails");
            assert!(error.contains("plugin tree failed to load"), "{error}");
            assert!(error.contains("failed to import loader entry"), "{error}");
            assert!(error.contains("cordis:no-such-plugin"), "{error}");
        });
    }

    #[test]
    fn assert_entries_loaded_audits_fiberless_entries() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let ctx = cordis::Context::root();
            let loader = dsh_cordis_loader::LoaderService::new(&ctx).await;
            ctx.register_service(loader.clone());
            // A settled entry whose fiber never appeared (TS fiber-less
            // entry: module failed to resolve but the row stays mounted).
            let group = loader.tree.root_group();
            let options = dsh_cordis_loader::EntryOptions {
                name: "ghost-plugin".to_string(),
                ..Default::default()
            };
            let entry = dsh_cordis_loader::Entry::new(loader.tree.core.clone(), group.ctx.clone());
            *entry.options.lock() = options.clone();
            *entry.parent.lock() = Some(group.clone());
            loader.tree.store.lock().insert("ghost".to_string(), entry);
            group.data.lock().push(options);

            let error = assert_entries_loaded(&loader, "dsh").expect_err("audits");
            assert!(error.contains("plugin(s) failed to load"), "{error}");
            assert!(error.contains("ghost-plugin"), "{error}");
        });
    }

    struct FakeProc {
        stderr_lines: std::sync::Mutex<Vec<String>>,
        exited: std::sync::atomic::AtomicI32,
    }

    impl FakeProc {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                stderr_lines: std::sync::Mutex::new(Vec::new()),
                exited: std::sync::atomic::AtomicI32::new(0),
            })
        }

        fn lines(&self) -> Vec<String> {
            self.stderr_lines.lock().unwrap().clone()
        }
    }

    impl FailLoudProcess for FakeProc {
        fn stderr(&self, line: &str) {
            self.stderr_lines.lock().unwrap().push(line.to_string());
        }

        fn exit(&self, code: i32) {
            self.exited
                .store(code, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[test]
    fn fail_loud_reports_once_then_exits_without_release() {
        let fake = FakeProc::new();
        let proc: Arc<dyn FailLoudProcess> = fake.clone();
        let guard = install_fail_loud("dsh", proc, None);
        guard.handle("boom".to_string());
        let lines = fake.lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("dsh: fatal load failure: boom"), "{}", lines[0]);
        assert_eq!(
            fake.exited.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        // A later rejection falls through the latch: still one report.
        guard.handle("later".to_string());
        assert_eq!(fake.lines().len(), 1);
    }

    #[test]
    fn fail_loud_reports_once_then_exits_after_release_timeout() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            tokio::time::pause();
            let fake = FakeProc::new();
            let proc: Arc<dyn FailLoudProcess> = fake.clone();
            let release: Arc<
                dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync,
            > = Arc::new(|| Box::pin(std::future::pending::<()>()));
            let guard = install_fail_loud("dsh", proc, Some(release));

            guard.handle("boom".to_string());
            assert_eq!(fake.lines().len(), 1);
            assert_eq!(fake.exited.load(std::sync::atomic::Ordering::SeqCst), 0);
            // The latch swallows a second rejection mid-release.
            guard.handle("second".to_string());
            assert_eq!(fake.lines().len(), 1);

            // First poll the spawned release task so its timeout timer is
            // registered before advancing the virtual clock.
            tokio::task::yield_now().await;
            tokio::time::advance(std::time::Duration::from_millis(
                FAIL_LOUD_RELEASE_TIMEOUT_MS + 100,
            ))
            .await;
            // The spawned release task runs after the timer fires; bounded
            // yields replace a fixed sleep.
            for _ in 0..10 {
                if fake.exited.load(std::sync::atomic::Ordering::SeqCst) == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert_eq!(fake.exited.load(std::sync::atomic::Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn fail_loud_uninstall_removes_the_handler() {
        let fake = FakeProc::new();
        let proc: Arc<dyn FailLoudProcess> = fake.clone();
        let guard = install_fail_loud("dsh", proc, None);
        guard.uninstall();
        guard.handle("boom".to_string());
        assert!(fake.lines().is_empty());
        assert_eq!(fake.exited.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    struct ConfigProbe {
        configs: std::sync::Mutex<Vec<Value>>,
    }

    #[async_trait::async_trait]
    impl cordis::Plugin for ConfigProbe {
        fn name(&self) -> Option<&'static str> {
            Some("probe")
        }

        async fn apply(
            &self,
            _ctx: &cordis::Context,
            config: cordis::ArcValue,
        ) -> Result<(), cordis::PluginError> {
            let config = cordis::downcast::<Value>(&config)
                .cloned()
                .unwrap_or(Value::Null);
            self.configs.lock().unwrap().push(config);
            Ok(())
        }
    }

    async fn include_fixture(
        config: dsh_cordis_include::IncludeConfig,
    ) -> (
        Arc<dsh_cordis_loader::LoaderService>,
        Arc<dsh_cordis_loader::Entry>,
        Arc<ConfigProbe>,
    ) {
        let ctx = cordis::Context::root();
        let loader = dsh_cordis_loader::LoaderService::new(&ctx).await;
        ctx.register_service(loader.clone());
        let probe = Arc::new(ConfigProbe {
            configs: std::sync::Mutex::new(Vec::new()),
        });
        loader.core.register("probe", probe.clone());
        loader.core.register("include", dsh_cordis_include::plugin());
        let entry = dsh_cordis_loader::EntryOptions {
            name: "include".to_string(),
            config: Some(serde_json::to_value(&config).expect("config")),
            ..Default::default()
        };
        loader
            .tree
            .create(entry, None, None)
            .await
            .expect("include entry");
        loader.tree.await_ready().await.expect("tree settles");
        let include_entry = loader
            .tree
            .entries()
            .into_iter()
            .find(|entry| entry.options.lock().name == "include")
            .expect("include entry");
        (loader, include_entry, probe)
    }

    #[test]
    fn refresh_user_patches_reapplies_the_user_layer() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let dir = temp_root("refresh-patches");
            let config_path = dir.join("cordis.yml");
            std::fs::write(&config_path, "- id: a\n  name: probe\n  config: 1\n")
                .expect("config file");
            let patch_path = dir.join(PROFILE_PATCH_FILENAME);
            let config = dsh_cordis_include::IncludeConfig {
                path: config_path.to_string_lossy().to_string(),
                initial: None,
                patches: None,
                enable_logs: None,
            };
            let (_loader, include_entry, probe) = include_fixture(config).await;
            assert_eq!(probe.configs.lock().unwrap().as_slice(), &[serde_json::json!(1)]);

            // First refresh: the patch file appears with an override.
            std::fs::write(&patch_path, "- id: a\n  config: 2\n").expect("patch file");
            refresh_user_patches("dsh", &include_entry, &patch_path, None)
                .await
                .expect("refresh");
            assert_eq!(
                probe.configs.lock().unwrap().as_slice(),
                &[serde_json::json!(1), serde_json::json!(2)]
            );

            // Second refresh: a changed patch re-applies; the include's
            // non-patch config (path) survives the update.
            std::fs::write(&patch_path, "- id: a\n  config: 3\n").expect("patch file");
            refresh_user_patches("dsh", &include_entry, &patch_path, None)
                .await
                .expect("refresh");
            assert_eq!(
                probe.configs.lock().unwrap().last(),
                Some(&serde_json::json!(3))
            );
            let include_config: dsh_cordis_include::IncludeConfig = serde_json::from_value(
                include_entry.options.lock().config.clone().expect("config"),
            )
            .expect("include config");
            assert_eq!(
                include_config.path,
                config_path.to_string_lossy().to_string()
            );
            assert_eq!(include_config.patches.expect("patches").len(), 1);

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn watch_user_patches_requires_the_hmr_service() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let ctx = cordis::Context::root();
            let loader = dsh_cordis_loader::LoaderService::new(&ctx).await;
            ctx.register_service(loader.clone());
            let entry = dsh_cordis_loader::Entry::new(
                loader.tree.core.clone(),
                loader.tree.root_group().ctx.clone(),
            );
            let options = UserPatchWatchOptions {
                bin_name: "dsh".to_string(),
                filename: PathBuf::from("cordis.patch.yml"),
                compose: None,
            };
            let error = match watch_user_patches(entry, options, None).await {
                Ok(_) => panic!("expected hmr error"),
                Err(error) => error,
            };
            assert!(error.contains("requires the Cordis HMR service"), "{error}");
        });
    }

    #[test]
    fn watch_user_patches_inactive_effect_returns_noop_disposer() {
        struct InactiveWatcher;
        impl UserPatchWatcher for InactiveWatcher {
            fn register_config(
                &self,
                _filename: PathBuf,
                _refresh: Arc<
                    dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync,
                >,
            ) -> Pin<
                Box<dyn Future<Output = Result<AsyncDisposer, String>> + Send>,
            > {
                Box::pin(async { Err("INACTIVE_EFFECT".to_string()) })
            }
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let ctx = cordis::Context::root();
            let loader = dsh_cordis_loader::LoaderService::new(&ctx).await;
            ctx.register_service(loader.clone());
            let entry = dsh_cordis_loader::Entry::new(
                loader.tree.core.clone(),
                loader.tree.root_group().ctx.clone(),
            );
            let options = UserPatchWatchOptions {
                bin_name: "dsh".to_string(),
                filename: PathBuf::from("cordis.patch.yml"),
                compose: None,
            };
            let disposer = watch_user_patches(entry, options, Some(&InactiveWatcher))
                .await
                .expect("no-op disposer");
            disposer().await;
        });
    }

    #[test]
    fn render_config_dump_groups_rows_by_source_and_patches() {
        let dir = temp_root("dump");
        let config_path = dir.join("cordis.yml");
        std::fs::write(
            &config_path,
            "- id: a\n  name: probe\n  config: 1\n- id: b\n  name: probe\n  config: 2\n",
        )
        .expect("base config");
        let mut patch = IndexMap::new();
        patch.insert("id".to_string(), serde_json::json!("a"));
        patch.insert("config".to_string(), serde_json::json!(10));
        let mut insert = IndexMap::new();
        insert.insert(
            "insert".to_string(),
            serde_json::json!([{ "id": "c", "name": "probe", "config": 3 }]),
        );
        let mut noop = IndexMap::new();
        noop.insert("id".to_string(), serde_json::json!("nope"));
        noop.insert("config".to_string(), serde_json::json!(0));
        let layers = vec![
            ConfigDumpLayer {
                label: "layer-one".to_string(),
                patches: vec![patch],
            },
            ConfigDumpLayer {
                label: "layer-two".to_string(),
                patches: vec![insert, noop],
            },
        ];
        let mut warnings = Vec::new();
        let dump = render_config_dump("dsh", &config_path, &layers, &mut |line| {
            warnings.push(line.to_string());
        })
        .expect("dump");
        // Row a was patched by layer-one; b stayed base; c came from layer-two.
        assert!(dump.contains("# == cordis.yml, patched by layer-one"), "{dump}");
        assert!(dump.contains("# == cordis.yml"), "{dump}");
        assert!(dump.contains("# == layer-two"), "{dump}");
        // The patched config value appears in the rendered rows.
        assert!(dump.contains("config: 10"), "{dump}");
        assert!(dump.contains("config: 3"), "{dump}");
        // The unmatched patch warns with its layer label.
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("dsh: [layer-two] patch: entry nope not found"),
            "{}",
            warnings[0]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
