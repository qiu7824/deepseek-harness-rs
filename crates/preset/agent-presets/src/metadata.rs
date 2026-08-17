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

#[cfg(test)]
mod tests {
    use super::*;

    /// A preset directory holding exactly the given metadata text.
    async fn preset_dir(content: Option<&str>) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dsh-preset-meta-{}-{}",
            std::process::id(),
            fastrand()
        ));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        if let Some(content) = content {
            tokio::fs::write(dir.join(METADATA_FILE), content)
                .await
                .unwrap();
        }
        dir
    }

    /// Cheap process-unique counter for temp dirs.
    fn fastrand() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    #[tokio::test]
    async fn reads_a_name_and_a_description() {
        let dir = preset_dir(Some("name: 标准模式\ndescription: 完整的编码 agent。\n")).await;
        let metadata = read_preset_metadata(&dir).await;
        assert_eq!(metadata.name.as_deref(), Some("标准模式"));
        assert_eq!(metadata.description.as_deref(), Some("完整的编码 agent。"));
    }

    #[tokio::test]
    async fn treats_an_absent_file_as_no_metadata() {
        let dir = preset_dir(None).await;
        assert_eq!(read_preset_metadata(&dir).await, PresetMetadata::default());
    }

    #[tokio::test]
    async fn treats_malformed_yaml_as_no_metadata() {
        let dir = preset_dir(Some("name: [unclosed\n")).await;
        assert_eq!(read_preset_metadata(&dir).await, PresetMetadata::default());
    }

    #[tokio::test]
    async fn treats_wrong_shapes_as_no_metadata() {
        for (label, content) in [
            ("a list", "- name: x\n"),
            ("a scalar", "just a string\n"),
            ("an empty document", ""),
        ] {
            let dir = preset_dir(Some(content)).await;
            assert_eq!(
                read_preset_metadata(&dir).await,
                PresetMetadata::default(),
                "case: {label}"
            );
        }
    }

    #[tokio::test]
    async fn ignores_fields_that_are_not_text() {
        let dir = preset_dir(Some("name: 42\ndescription:\n  nested: true\n")).await;
        assert_eq!(read_preset_metadata(&dir).await, PresetMetadata::default());
    }

    #[tokio::test]
    async fn ignores_blank_text() {
        let dir = preset_dir(Some("name: \"   \"\ndescription: \"\"\n")).await;
        assert_eq!(read_preset_metadata(&dir).await, PresetMetadata::default());
    }

    #[tokio::test]
    async fn trims_surrounding_whitespace() {
        let dir = preset_dir(Some("name: \"  极简模式  \"\n")).await;
        let metadata = read_preset_metadata(&dir).await;
        assert_eq!(metadata.name.as_deref(), Some("极简模式"));
    }

    #[tokio::test]
    async fn reads_a_declared_order() {
        let dir = preset_dir(Some("name: 标准模式\norder: 1\n")).await;
        let metadata = read_preset_metadata(&dir).await;
        assert_eq!(metadata.name.as_deref(), Some("标准模式"));
        assert_eq!(metadata.order, Some(1.0));
    }

    #[tokio::test]
    async fn ignores_an_order_that_is_not_a_finite_number() {
        let dir = preset_dir(Some("order: first\n")).await;
        assert_eq!(read_preset_metadata(&dir).await.order, None);
        let dir = preset_dir(Some("order: .inf\n")).await;
        assert_eq!(read_preset_metadata(&dir).await.order, None);
    }

    #[tokio::test]
    async fn cannot_carry_identity_or_trust() {
        let dir = preset_dir(Some("name: mine\nid: standard\ntrust: system\n")).await;
        let metadata = read_preset_metadata(&dir).await;
        assert_eq!(metadata.name.as_deref(), Some("mine"));
        assert_eq!(metadata.description, None);
        assert_eq!(metadata.order, None);
    }

    #[tokio::test]
    async fn round_trips_through_a_read() {
        let rendered = render_preset_metadata(&PresetMetadata {
            name: Some("创造模式".to_string()),
            description: Some("可以改自己的组装。".to_string()),
            order: None,
        })
        .unwrap();
        let dir = preset_dir(Some(&rendered)).await;
        let metadata = read_preset_metadata(&dir).await;
        assert_eq!(metadata.name.as_deref(), Some("创造模式"));
        assert_eq!(metadata.description.as_deref(), Some("可以改自己的组装。"));
    }

    #[test]
    fn stores_a_declared_order() {
        let rendered = render_preset_metadata(&PresetMetadata {
            name: Some("标准模式".to_string()),
            description: None,
            order: Some(1.0),
        })
        .unwrap();
        assert_eq!(rendered, "name: 标准模式\norder: 1\n");
    }

    #[test]
    fn omits_an_absent_field_rather_than_writing_it_blank() {
        let rendered = render_preset_metadata(&PresetMetadata {
            name: Some("极简模式".to_string()),
            description: None,
            order: None,
        })
        .unwrap();
        assert_eq!(rendered, "name: 极简模式\n");
        let rendered = render_preset_metadata(&PresetMetadata {
            name: None,
            description: Some("只做检索。".to_string()),
            order: None,
        })
        .unwrap();
        assert_eq!(rendered, "description: 只做检索。\n");
    }

    #[test]
    fn renders_nothing_when_there_is_nothing_to_store() {
        assert_eq!(render_preset_metadata(&PresetMetadata::default()), None);
        assert_eq!(
            render_preset_metadata(&PresetMetadata {
                name: Some("  ".to_string()),
                description: Some(String::new()),
                order: None,
            }),
            None
        );
    }
}
