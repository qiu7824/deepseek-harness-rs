//! User-settings capability seam for the DeepSeek Harness. Rust port of
//! `@deepseek-ai/dsh-settings`.

pub mod index;
pub mod invariant;
pub mod redact;
pub mod types;

pub use index::{
    SETTINGS_DOCUMENT_UPDATED, SETTINGS_UPDATED, SettingsApplies, SettingsConflictError,
    SettingsDescribeOptions, SettingsDescriptor, SettingsNamespace, SettingsPathOp,
    SettingsProvider, SettingsRegisterOptions, SettingsScope, SettingsSectionHooks,
    SettingsStorage, deep_equal_json, install_settings_section, settings_namespace,
};
pub use redact::{RedactedSecret, RedactedValue, redact_secrets};
pub use types::SettingsUpdateSource;
