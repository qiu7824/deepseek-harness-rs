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
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../config/agent-presets")
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

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_app_boot::{PROFILES_DIR, init_profile, resolve_profile_dir};

    fn temp_home(tag: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let home = std::env::temp_dir().join(format!(
            "dsh-profile-boot-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&home).expect("temp home");
        home
    }

    #[test]
    fn home_patch_path_sits_at_the_home_root() {
        let home = PathBuf::from("/home/me/.dsh");
        assert_eq!(home_patch_path(&home), home.join(PROFILE_PATCH_FILENAME));
    }

    #[test]
    fn telemetry_patch_prefers_off_by_mistake() {
        assert_eq!(resolve_telemetry_patch(None, true), None);
        assert_eq!(resolve_telemetry_patch(Some(""), true), None);
        assert_eq!(resolve_telemetry_patch(Some("1"), false), None);
        for value in ["1", "0", "false", "yes"] {
            let patch = resolve_telemetry_patch(Some(value), true).expect("patch");
            assert_eq!(
                patch.get("id").and_then(Value::as_str),
                Some(TELEMETRY_ROW_ID)
            );
            assert_eq!(patch.get("disabled"), Some(&Value::Bool(true)));
        }
    }

    #[test]
    fn compose_profile_stacks_layers_in_application_order() {
        let home = temp_home("stack");
        let dir = resolve_profile_dir("web", &home).expect("dir");
        init_profile(&dir, &[]).expect("init");
        // The profile's own user layer inserts one row.
        std::fs::write(
            dir.join(PROFILE_PATCH_FILENAME),
            "- insert:\n    - id: probe-row\n      name: probe\n      config: 1\n",
        )
        .expect("user layer");
        // The home-level user layer inserts a second row plus the telemetry
        // row the switch targets.
        std::fs::write(
            home.join(PROFILE_PATCH_FILENAME),
            "- insert:\n    - id: home-row\n      name: probe\n      config: 2\n    - id: session-telemetry-otel\n      name: probe\n      config: {}\n",
        )
        .expect("home layer");
        // One overlay inserts a third row.
        let overlay = home.join("extra.yml");
        std::fs::write(
            &overlay,
            "- insert:\n    - id: overlay-row\n      name: probe\n      config: 3\n",
        )
        .expect("overlay");

        let composed = compose_profile(
            "web",
            &[overlay.to_string_lossy().to_string()],
            &home,
            Some("1"),
        )
        .expect("composed");
        assert!(composed.bundle_patches.is_empty());
        assert_eq!(composed.profile.patches.len(), 1);
        assert_eq!(composed.home_patches.len(), 1);
        // overlays = the --patch overlay + the telemetry patch.
        assert_eq!(composed.overlays.len(), 2);
        assert!(composed.rows.contains_key("probe-row"));
        assert!(composed.rows.contains_key("home-row"));
        assert!(composed.rows.contains_key("overlay-row"));

        // The full stack applies bundle → profile → home → overlays.
        let all = all_patches(&composed);
        assert_eq!(all.len(), 4);
        assert_eq!(
            all[0]
                .get("insert")
                .and_then(Value::as_array)
                .and_then(|rows| rows.first())
                .and_then(|row| row.get("id"))
                .and_then(Value::as_str),
            Some("probe-row")
        );
        assert_eq!(
            all[1]
                .get("insert")
                .and_then(Value::as_array)
                .and_then(|rows| rows.first())
                .and_then(|row| row.get("id"))
                .and_then(Value::as_str),
            Some("home-row")
        );
        assert_eq!(
            all[2]
                .get("insert")
                .and_then(Value::as_array)
                .and_then(|rows| rows.first())
                .and_then(|row| row.get("id"))
                .and_then(Value::as_str),
            Some("overlay-row")
        );
        assert_eq!(
            all[3].get("id").and_then(Value::as_str),
            Some(TELEMETRY_ROW_ID)
        );

        // compose_live re-reads both user layers under the overlays.
        let live = compose_live(&composed, &home).expect("live");
        assert_eq!(live.len(), 4);
        assert_eq!(
            live[1]
                .get("insert")
                .and_then(Value::as_array)
                .and_then(|rows| rows.first())
                .and_then(|row| row.get("id"))
                .and_then(Value::as_str),
            Some("home-row")
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn prepare_profile_auto_initializes_a_shipped_profile() {
        let home = temp_home("auto-init");

        let profile = prepare_profile("web", &home).expect("prepared");

        assert_eq!(profile.name, "web");
        assert_eq!(
            profile
                .layers
                .iter()
                .map(|layer| layer.package_name.as_str())
                .collect::<Vec<_>>(),
            vec!["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"]
        );
        assert!(profile.dir.join("package.json").exists());
        assert!(profile.dir.join(PROFILE_PATCH_FILENAME).exists());
        assert!(profile.dir.join(PROFILE_ROOT_FILENAME).exists());

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn prepare_profile_rewrites_the_empty_root() {
        let home = temp_home("root");
        let dir = resolve_profile_dir("web", &home).expect("dir");
        init_profile(&dir, &[]).expect("init");
        let root = dir.join(PROFILE_ROOT_FILENAME);
        // Bake a stale composed row, then prepare: the root must reset.
        std::fs::write(&root, "- id: stale\n  name: probe\n").expect("stale root");
        let profile = prepare_profile("web", &home).expect("prepared");
        let content = std::fs::read_to_string(&root).expect("read");
        assert_eq!(content, PROFILE_ROOT_CONFIG);
        assert_eq!(profile.name, "web");
        assert_eq!(profile.dir, home.join(PROFILES_DIR).join("web"));

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn dump_config_layers_and_default_only_recovery() {
        let home = temp_home("dump");
        let dir = resolve_profile_dir("web", &home).expect("dir");
        init_profile(&dir, &[]).expect("init");
        std::fs::write(
            dir.join(PROFILE_PATCH_FILENAME),
            "- insert:\n    - id: user-row\n      name: probe\n      config: 1\n",
        )
        .expect("user layer");
        std::fs::write(
            home.join(PROFILE_PATCH_FILENAME),
            "- insert:\n    - id: home-row\n      name: probe\n      config: 2\n",
        )
        .expect("home layer");
        let overlay = home.join("extra.yml");
        std::fs::write(
            &overlay,
            "- insert:\n    - id: overlay-row\n      name: probe\n      config: 3\n",
        )
        .expect("overlay");

        let (dump, warnings) = run_dump_config(
            "web",
            false,
            &[overlay.to_string_lossy().to_string()],
            &home,
        )
        .expect("dump");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(dump.contains("user-row"), "{dump}");
        assert!(dump.contains("home-row"), "{dump}");
        assert!(dump.contains("overlay-row"), "{dump}");
        // Source labels from the three layers appear as comments.
        assert!(dump.contains("# =="), "{dump}");

        // defaultOnly skips the broken user layer entirely (recovery).
        std::fs::write(dir.join(PROFILE_PATCH_FILENAME), "not: [valid\n")
            .expect("broken user layer");
        let (dump, _) = run_dump_config("web", true, &[], &home).expect("recovery dump");
        assert!(!dump.contains("user-row"), "{dump}");
        assert!(!dump.contains("home-row"), "{dump}");

        let _ = std::fs::remove_dir_all(&home);
    }
}
