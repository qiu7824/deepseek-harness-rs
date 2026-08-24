//! A preset's display metadata: the name and description a picker shows.
//! Rust port of `src/metadata.ts`.
//!
//! The file carries display text ONLY — `id` is the directory name and
//! `trust` comes from the root a preset was discovered under. Every read
//! failure degrades to no metadata.

use std::path::Path;

/// The optional display-metadata file beside a preset's composition.
pub const METADATA_FILE: &str = "preset.yml";

/// Display text a preset may publish about itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PresetMetadata {
    /// Human-facing name; falls back to the preset id when absent.
    pub name: Option<String>,
    /// One sentence on what this preset is for.
    pub description: Option<String>,
    /// Position within its group; lower comes first. A preset that declares
    /// none sorts after every preset that does, then by id.
    pub order: Option<f64>,
}

/// A non-empty trimmed string, or `None` for anything else (TS `text`).
fn text(value: Option<&serde_json::Value>) -> Option<String> {
    let Some(value) = value else { return None };
    let Some(value) = value.as_str() else {
        return None;
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Read one preset directory's display metadata. Absent, unparsable, and
/// wrongly-shaped files are all the same answer — empty metadata — because
/// the caller renders a picker, not a diagnostic (TS `readPresetMetadata`).
pub async fn read_preset_metadata(directory: &Path) -> PresetMetadata {
    let path = directory.join(METADATA_FILE);
    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(raw) => raw,
        // Absent is the common case: metadata is optional.
        Err(_) => return PresetMetadata::default(),
    };
    let parsed: serde_json::Value = match dsh_cordis_include::yaml::parse_yaml(&raw) {
        Ok(value) => value,
        // Malformed display text is not worth failing discovery over.
        Err(_) => return PresetMetadata::default(),
    };
    let Some(record) = parsed.as_object() else {
        return PresetMetadata::default();
    };
    let name = text(record.get("name"));
    let description = text(record.get("description"));
    let order = match record.get("order") {
        Some(value) if value.is_number() => value.as_f64(),
        _ => None,
    };
    PresetMetadata {
        name,
        description,
        order,
    }
}

/// Render display metadata as the file's contents. Absent fields are omitted
/// rather than written empty (TS `renderPresetMetadata`).
pub fn render_preset_metadata(metadata: &PresetMetadata) -> Option<String> {
    let name = metadata
        .name
        .as_deref()
        .and_then(|value| text(Some(&serde_json::Value::String(value.to_string()))));
    let description = metadata
        .description
        .as_deref()
        .and_then(|value| text(Some(&serde_json::Value::String(value.to_string()))));
    let order = metadata.order;
    if name.is_none() && description.is_none() && order.is_none() {
        return None;
    }
    let mut document = serde_json::Map::new();
    if let Some(name) = name {
        document.insert("name".to_string(), serde_json::Value::String(name));
    }
    if let Some(description) = description {
        document.insert(
            "description".to_string(),
            serde_json::Value::String(description),
        );
    }
    if let Some(order) = order {
        // Render integral orders as integers (TS js-yaml prints `1`, not
        // `1.0`).
        let number = if order.fract() == 0.0 && order.abs() < 9.0e15 {
            serde_json::Number::from(order as i64)
        } else {
            serde_json::Number::from_f64(order)?
        };
        document.insert("order".to_string(), serde_json::Value::Number(number));
    }
    let yaml = dsh_cordis_include::yaml::json_to_yaml(&serde_json::Value::Object(document));
    Some(serde_yaml::to_string(&yaml).ok()?)
}
