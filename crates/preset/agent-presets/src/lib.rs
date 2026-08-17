//! Agent presets: each session composes its model-facing plugin set from one
//! preset `cordis.yml`, mounted ONCE per preset under a standing scope and
//! joined by every agent that names it.
//! Rust port of `@deepseek-ai/dsh-agent-presets`.
//!
//! This package owns the preset vocabulary, filesystem discovery, and the
//! guarded standing mount. It does not decide when an agent is created — the
//! agent factory's `setup` hook is the one supported call site.
//!
//! # Deviations
//!
//! - TS `ScopeKey` values carry `{ agentPreset: id }`; the Rust [`ScopeKey`]
//!   is an opaque identity (see `dsh-scope`), so standing mounts match keys
//!   by identity and carry the preset id on their own records.
//! - The mounted subtree's module resolution goes through the Rust loader's
//!   static registry (see `dsh-cordis-loader`); the `PresetTree` subclass of
//!   `Include` collapses to a read-only [`Include`] whose write-back is
//!   suppressed, matching the TS override.
//! - `WeakMap` bindings (bindings per agent key) are strong `Arc` entries in
//!   a mutex-guarded table; entries die with the agent only at whole-tree
//!   teardown (same retention profile as `dsh-scope`'s parent registry).

pub mod authoring;
pub mod discovery;
pub mod index;
pub mod invariant;
pub mod metadata;
pub mod mount;
pub mod preset;
pub mod session;

pub use authoring::{
    AuthoringError, InvalidPresetIdError, PresetExistsError, PresetNotWritableError,
    copy_composition, delete_composition, read_composition, writable_root,
};
pub use discovery::{COMPOSITION_FILE, USER_PRESET_DIR, discover_presets, scan_root};
pub use index::{AgentPresetSettings, AgentPresets, SETTINGS_NAMESPACE, process_env};
pub use metadata::{METADATA_FILE, PresetMetadata, read_preset_metadata, render_preset_metadata};
pub use mount::{
    JoinedPresetMount, PresetMount, inactive_rows, leaked_services, live_preset_mounts,
    mount_preset, service_for_agent, standing_mount_for,
};
pub use preset::{
    AgentPreset, Config, PresetMountError, PresetRoot, PresetTrust, UnknownPresetError,
    preset_id_ok,
};
pub use session::{AGENT_PRESET_SELECTED, resolve_session_preset, selected_data};

/// The package-owned invariant companion (TS `@deepseek-ai/dsh-agent-presets/invariant`).
pub mod companion {
    pub use crate::invariant::{INJECT, NAME, apply, installer};
}
