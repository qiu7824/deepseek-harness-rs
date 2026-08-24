//! Agent-preset vocabulary shared by discovery, mounting, and consumers.
//! Rust port of `src/preset.ts`.

/// Where a preset's composition came from. A `system` preset ships with the
/// deployment; a `user` preset was authored locally, by a person or by an
/// agent, and therefore carries the same trust as shell access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PresetTrust {
    System,
    User,
}

/// Whether `value` is a usable preset id: `[a-z0-9][a-z0-9-]*`
/// (TS `PRESET_ID`). The id becomes a path segment, so this is a containment
/// boundary rather than a style rule; discovery shares it.
pub fn preset_id_ok(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// One preset directory that carries a mountable agent composition.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentPreset {
    /// Stable identifier; the preset directory's name.
    pub id: String,
    /// Trust recorded from the root this preset was discovered under.
    pub trust: PresetTrust,
    /// Absolute path of the preset's agent composition file.
    pub path: String,
    /// Display name from the preset's own metadata; absent falls back to `id`.
    pub name: Option<String>,
    /// One sentence on what this preset is for, when it published one.
    pub description: Option<String>,
    /// Declared position within its group; absent sorts after those that
    /// declare one.
    pub order: Option<f64>,
    /// Why this preset cannot compose a session, absent when it can.
    pub broken: Option<String>,
}

/// One directory scanned for preset subdirectories.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PresetRoot {
    /// Directory holding one subdirectory per preset; a leading `~` expands.
    pub path: String,
    /// Trust recorded on every preset discovered under this root.
    #[serde(default = "default_trust")]
    pub trust: PresetTrust,
}

fn default_trust() -> PresetTrust {
    PresetTrust::User
}

/// Plugin config: which preset is the default, and where presets live.
#[derive(Debug, Clone)]
pub struct Config {
    /// Preset id mounted when a caller names none. Missing at mount time
    /// fails loud.
    pub default: String,
    /// Scanned roots in precedence order; an earlier root wins a duplicate id.
    pub roots: Vec<PresetRoot>,
    /// Append the harness home's `USER_PRESET_DIR` as a `user` root, after
    /// every configured root. False mounts a roster over `roots` alone.
    pub include_user_root: bool,
}

/// No configured root supplies the requested preset. Separate from a mount
/// failure because the two mean different things to a caller: an unknown id
/// is a bad request, while an unusable composition is a broken preset the
/// deployment must fix.
#[derive(Debug, thiserror::Error)]
#[error("agent-presets: preset \"{preset_id}\" not found (available: {available})")]
pub struct UnknownPresetError {
    /// The id that was requested.
    pub preset_id: String,
    /// Ids the roster does supply, for the caller to offer instead.
    pub available: String,
}

impl UnknownPresetError {
    pub fn new(preset_id: &str, available: &[String]) -> Self {
        let available = available
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        let available = if available.is_empty() {
            "none".to_string()
        } else {
            available
        };
        Self {
            preset_id: preset_id.to_string(),
            available,
        }
    }
}

/// A preset exists but its composition cannot be installed.
#[derive(Debug, Clone, thiserror::Error)]
#[error("agent-presets: preset \"{preset_id}\" failed to mount: {reason}")]
pub struct PresetMountError {
    /// The preset whose composition failed.
    pub preset_id: String,
    /// Why it failed, without this package's own message prefix.
    pub reason: String,
}

impl PresetMountError {
    pub fn new(preset_id: &str, reason: impl Into<String>) -> Self {
        Self {
            preset_id: preset_id.to_string(),
            reason: reason.into(),
        }
    }
}
