//! Pure types of the user-settings seam (TS `types.ts`): the namespace
//! brand, the commit-origin union, and the seam's event names.

/// Origin of one committed settings change (TS `SettingsUpdateSource`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsUpdateSource {
    /// The change entered through `update`/`replace`/`mutate`.
    Update,
    /// The change entered through the provider's `publish`.
    Provider,
}

impl SettingsUpdateSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            SettingsUpdateSource::Update => "update",
            SettingsUpdateSource::Provider => "provider",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "update" => Some(SettingsUpdateSource::Update),
            "provider" => Some(SettingsUpdateSource::Provider),
            _ => None,
        }
    }
}

/// `settings/updated`: committed change to one namespace's resolved value.
pub const SETTINGS_UPDATED: &str = "settings/updated";

/// `settings/document-updated`: one namespace's RAW user section changed.
pub const SETTINGS_DOCUMENT_UPDATED: &str = "settings/document-updated";
