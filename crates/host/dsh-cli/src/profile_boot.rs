//! Shared profile boot for every `dsh` surface (TS
//! `apps/cli/src/profile-boot.ts`): resolve the profile, stack its patch
//! layers (bundle layers in `dsh.profile.bundles` order, the profile's own
//! `cordis.patch.yml`, the home-level user layer, `--patch` overlays, the
//! telemetry switch), and mount the tree over the profile's empty root
//! config.
//!
//! # Deviations
//!
//! - The shipped agent-preset root overlay is skipped (no
//!   `dsh-agent-presets` crate exists yet).
//! - `DSH_TELEMETRY_DISABLED` is passed in by the caller instead of read
//!   from the process environment (test isolation).
//! - Dynamic JavaScript loader plugins and the concrete HMR filesystem
//!   provider remain outside the static Rust profile surface.

use std::path::{Path, PathBuf};

use dsh_app_boot::{
    PROFILE_PATCH_FILENAME, PatchOptions, Profile, compose_entries, init_profile,
    load_optional_patches, load_overlay_patches, profile_templates, resolve_profile_dir,
};
use indexmap::IndexMap;
use serde_json::Value;

/// The launcher's diagnostic prefix.
pub const NAME: &str = "dsh";

/// Root config filename inside a profile directory.
pub const PROFILE_ROOT_FILENAME: &str = "cordis.yml";

/// The empty root entry list every profile tree patches over.
pub const PROFILE_ROOT_CONFIG: &str = "# dsh profile root — an empty entry list. The tree is composed as patches:\n\
# each bundle in package.json's dsh.profile.bundles, then cordis.patch.yml, then any\n\
# --patch overlays. Edit cordis.patch.yml, not this file.\n\
[]\n";

/// The session-telemetry row id the DSH_TELEMETRY_DISABLED switch targets.
const TELEMETRY_ROW_ID: &str = "session-telemetry-otel";

/// The agent-presets row id the shipped-root overlay targets.
pub const AGENT_PRESETS_ROW_ID: &str = "agent-presets";

/// The shipped preset root beside this app's config (TS
/// `SHIPPED_PRESET_ROOT`, `apps/cli/config/agent-presets` beside the
/// package). Anchored to the manifest, not the process cwd, so profile
/// boots from any working directory resolve the same root.
pub fn shipped_preset_root() -> String {
    let adjacent = std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.parent()
                .map(|parent| parent.join("config/agent-presets"))
        })
        .filter(|path| path.exists());
    adjacent
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../config/agent-presets")
        })
        .to_string_lossy()
        .into_owned()
}

/// Resolve the shipped preset root into its boot patch (TS
/// `composeProfile`): the SHIPPED root is the part of the roster only this
/// app can resolve, so it rides a top overlay; the writable root the roster
/// appends is `dsh-agent-presets`' own (its `include_user_root`), so a
/// launcher that never reaches this patch still finds a person's presets.
/// The existing row config survives — only `roots` is replaced.
pub fn resolve_agent_presets_patch(row: Option<&Value>) -> Option<PatchOptions> {
    let row = row?;
    let mut config = row
        .get("config")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    config.insert(
        "roots".to_string(),
        serde_json::json!([{ "path": shipped_preset_root(), "trust": "system" }]),
    );
    let mut patch = IndexMap::new();
    patch.insert(
        "id".to_string(),
        Value::String(AGENT_PRESETS_ROW_ID.to_string()),
    );
    patch.insert("config".to_string(), Value::Object(config));
    Some(patch)
}

/// The home-level user patch layer (`$DSH_HOME/cordis.patch.yml`), applied
/// over every profile's own layer (TS `homePatchPath`).
pub fn home_patch_path(home: &Path) -> PathBuf {
    home.join(PROFILE_PATCH_FILENAME)
}

/// Resolve the telemetry opt-out switch into its boot patch (TS
/// `resolveTelemetryPatch`): ANY non-empty value (including `'0'`/`'false'`)
/// disables — a privacy switch prefers off-by-mistake. A composition without
/// the telemetry row exports nothing, so the switch is then trivially
/// satisfied.
pub fn resolve_telemetry_patch(disabled_env: Option<&str>, has_row: bool) -> Option<PatchOptions> {
    let disabled = disabled_env.unwrap_or_default();
    if disabled.is_empty() || !has_row {
        return None;
    }
    let mut patch = IndexMap::new();
    patch.insert(
        "id".to_string(),
        Value::String(TELEMETRY_ROW_ID.to_string()),
    );
    patch.insert("disabled".to_string(), Value::Bool(true));
    Some(patch)
}

/// Load a resolved profile and (re)write the empty root config (TS
/// `prepareProfile`). The root is always rewritten: the whole composition
/// is patch layers, and the loader's tree write-back can bake composed rows
/// into this file — which would duplicate every bundle insert on the next
/// boot. `user_layer` false skips parsing `cordis.patch.yml` (the dump's
/// recovery diagnostic for a broken user layer).
pub fn prepare_profile(name: &str, home: &Path) -> Result<Profile, String> {
    prepare_profile_with_user_layer(name, home, true)
}

/// Full Node-compatible profile preparation used by launchers that carry the
/// installed app package.json anchor. It heals the shared module fallback
/// before applying installation-first/profile-second bundle resolution.
pub fn prepare_profile_with_install_anchor(
    name: &str,
    home: &Path,
    user_layer: bool,
    install_anchor: &Path,
) -> Result<Profile, String> {
    dsh_app_boot::heal_profiles_module_fallback(install_anchor, home)?;
    let dir = resolve_profile_dir(name, home)?;
    if !dir.join("package.json").exists() {
        let template = profile_templates()
            .get(name)
            .ok_or_else(|| format!("{NAME}: profile {name:?} is not initialized"))?;
        init_profile(
            &dir,
            &template
                .iter()
                .map(|bundle| (*bundle).to_string())
                .collect::<Vec<_>>(),
        )?;
    }
    let profile = dsh_app_boot::load_profile_with_anchors(
        name,
        home,
        user_layer,
        Some(install_anchor),
        NAME,
    )?;
    std::fs::write(profile.dir.join(PROFILE_ROOT_FILENAME), PROFILE_ROOT_CONFIG)
        .map_err(|error| format!("{NAME}: cannot write profile root config: {error}"))?;
    Ok(profile)
}

/// [`prepare_profile`] with the TS `userLayer` switch.
pub fn prepare_profile_with_user_layer(
    name: &str,
    home: &Path,
    user_layer: bool,
) -> Result<Profile, String> {
    let dir = resolve_profile_dir(name, home)?;
    if !dir.join("package.json").exists() {
        let template = profile_templates()
            .get(name)
            .ok_or_else(|| format!("{NAME}: profile {name:?} is not initialized"))?;
        let bundles = template
            .iter()
            .map(|bundle| (*bundle).to_string())
            .collect::<Vec<_>>();
        init_profile(&dir, &bundles)
            .map_err(|error| format!("{NAME}: cannot initialize profile {name:?}: {error}"))?;
    }
    let profile = dsh_app_boot::load_profile_with_user_layer(name, home, user_layer)?;
    std::fs::write(profile.dir.join(PROFILE_ROOT_FILENAME), PROFILE_ROOT_CONFIG)
        .map_err(|error| format!("{NAME}: cannot write profile root config: {error}"))?;
    Ok(profile)
}

/// One profile's patch layers (application order) and the row index of its
/// pre-flag composition (TS `ComposedProfile`).
#[derive(Debug)]
pub struct ComposedProfile {
    pub profile: Profile,
    /// Bundle layers concatenated — the part below the user layers on a live
    /// reload.
    pub bundle_patches: Vec<PatchOptions>,
    /// The home-level user layer, applied after the profile's own.
    pub home_patches: Vec<PatchOptions>,
    /// Layers above the user layers on a live reload: `--patch` overlays and
    /// the telemetry switch.
    pub overlays: Vec<PatchOptions>,
    /// id → row of the composed tree (bundles + user layers + overlays).
    pub rows: IndexMap<String, Value>,
}

/// The full patch stack of one composed profile, in application order (TS
/// `allPatches`).
pub fn all_patches(composed: &ComposedProfile) -> Vec<PatchOptions> {
    let mut patches = Vec::new();
    patches.extend(composed.bundle_patches.iter().cloned());
    patches.extend(composed.profile.patches.iter().cloned());
    patches.extend(composed.home_patches.iter().cloned());
    patches.extend(composed.overlays.iter().cloned());
    patches
}

/// Recomposition for the live user layers: bundle layers below, overlays
/// above, so a user edit can never displace them. Both user files are
/// re-read per generation (TS `composeLive`); Rust `IndexMap` clones are
/// deep, so the TS insert-aliasing concern does not apply.
pub fn compose_live(composed: &ComposedProfile, home: &Path) -> Result<Vec<PatchOptions>, String> {
    let mut patches = Vec::new();
    patches.extend(composed.bundle_patches.iter().cloned());
    patches.extend(load_optional_patches(NAME, &composed.profile.patch_path)?.unwrap_or_default());
    patches.extend(load_optional_patches(NAME, &home_patch_path(home))?.unwrap_or_default());
    patches.extend(composed.overlays.iter().cloned());
    Ok(patches)
}

/// Load `name` and compose its effective patch stack (TS `composeProfile`):
/// bundle layers in `dsh.profile.bundles` order, the profile's user layer,
/// the home-level user layer (machine-local, outranks the per-profile
/// layer), `--patch` overlays, then the telemetry switch.
pub fn compose_profile(
    name: &str,
    patch_files: &[String],
    home: &Path,
    telemetry_env: Option<&str>,
) -> Result<ComposedProfile, String> {
    compose_profile_from_prepared(
        prepare_profile(name, home)?,
        patch_files,
        home,
        telemetry_env,
    )
}

pub fn compose_profile_with_install_anchor(
    name: &str,
    patch_files: &[String],
    home: &Path,
    telemetry_env: Option<&str>,
    install_anchor: &Path,
) -> Result<ComposedProfile, String> {
    compose_profile_from_prepared(
        prepare_profile_with_install_anchor(name, home, true, install_anchor)?,
        patch_files,
        home,
        telemetry_env,
    )
}

fn compose_profile_from_prepared(
    profile: Profile,
    patch_files: &[String],
    home: &Path,
    telemetry_env: Option<&str>,
) -> Result<ComposedProfile, String> {
    let home_patches = load_optional_patches(NAME, &home_patch_path(home))?.unwrap_or_default();
    let mut overlays: Vec<PatchOptions> = Vec::new();
    for file in patch_files {
        let absolute = std::path::absolute(Path::new(file))
            .map_err(|error| format!("{NAME}: cannot resolve overlay {file}: {error}"))?;
        overlays.extend(load_overlay_patches(NAME, &absolute)?);
    }
    let bundle_patches: Vec<PatchOptions> = profile
        .layers
        .iter()
        .flat_map(|layer| layer.patches.iter().cloned())
        .collect();
    let rows = compose_entries(&[
        bundle_patches.clone(),
        profile.patches.clone(),
        home_patches.clone(),
        overlays.clone(),
    ])?;
    let mut row_map: IndexMap<String, Value> = IndexMap::new();
    for row in &rows {
        if let Some(id) = row.get("id").and_then(Value::as_str) {
            row_map.insert(id.to_string(), row.clone());
        }
    }
    let mut composed_overlays = overlays;
    // The shipped preset root overlay sits above every user layer (TS
    // composeProfile pushes it after the --patch overlays).
    if let Some(patch) = resolve_agent_presets_patch(row_map.get(AGENT_PRESETS_ROW_ID)) {
        composed_overlays.push(patch);
    }
    if let Some(patch) =
        resolve_telemetry_patch(telemetry_env, row_map.contains_key(TELEMETRY_ROW_ID))
    {
        composed_overlays.push(patch);
    }
    Ok(ComposedProfile {
        profile,
        bundle_patches,
        home_patches,
        overlays: composed_overlays,
        rows: row_map,
    })
}

/// Print a profile composition with comments naming each source file and
/// patch layer (TS `runDumpConfig`): compose the profile's patch layers
/// through the include plugin's patch algorithm without booting or
/// evaluating `!!js`. Returns the rendered YAML document and the skipped-
/// patch warnings (TS writes them to stderr).
pub fn run_dump_config(
    profile_name: &str,
    default_only: bool,
    patches: &[String],
    home: &Path,
) -> Result<(String, Vec<String>), String> {
    let loaded = prepare_profile_with_user_layer(profile_name, home, !default_only)?;
    let mut layers: Vec<dsh_app_boot::ConfigDumpLayer> = loaded
        .layers
        .iter()
        .map(|layer| dsh_app_boot::ConfigDumpLayer {
            label: layer.package_name.clone(),
            patches: layer.patches.clone(),
        })
        .collect();
    if !default_only {
        if loaded.patch_path.exists() {
            layers.push(dsh_app_boot::ConfigDumpLayer {
                label: loaded.patch_path.display().to_string(),
                patches: loaded.patches.clone(),
            });
        }
        let home_file = home_patch_path(home);
        if let Some(home_patches) = load_optional_patches(NAME, &home_file)? {
            layers.push(dsh_app_boot::ConfigDumpLayer {
                label: home_file.display().to_string(),
                patches: home_patches,
            });
        }
        for file in patches {
            let absolute = std::path::absolute(Path::new(file))
                .map_err(|error| format!("{NAME}: cannot resolve overlay {file}: {error}"))?;
            layers.push(dsh_app_boot::ConfigDumpLayer {
                label: absolute.display().to_string(),
                patches: load_overlay_patches(NAME, &absolute)?,
            });
        }
    }
    let mut warnings: Vec<String> = Vec::new();
    let dump = dsh_app_boot::render_config_dump(
        NAME,
        &loaded.dir.join(PROFILE_ROOT_FILENAME),
        &layers,
        &mut |line| warnings.push(line.to_string()),
    )?;
    Ok((dump, warnings))
}
