//! On-disk JSON unit format. Rust port of
//! `packages/storage/storage-json/src/format.ts`. The file is always the
//! current net state, kept human-readable (pretty-printed, stable key order
//! from insertion) — that legibility is this backend's reason to exist.

use indexmap::IndexMap;
use serde_json::{Map, Value as JsonValue};

use dsh_storage::{KvUnitDescriptor, StorageError, StorageErrorCode};

/// In-memory authoritative state of one unit; the file is its projection.
/// `global` is `Null` until first written (TS `UnitState`).
#[derive(Debug, Clone)]
pub struct UnitState {
    pub version: u64,
    pub global: JsonValue,
    pub tables: IndexMap<String, IndexMap<String, JsonValue>>,
}

fn ordered() -> Map<String, JsonValue> {
    Map::new()
}

/// Serialize a unit state to file content (TS `serialize`): a
/// pretty-printed JSON document with a trailing newline, keys in insertion
/// order.
pub fn serialize(name: &str, state: &UnitState) -> String {
    let mut document = ordered();
    document.insert(
        "unit".to_string(),
        JsonValue::Object({
            let mut unit = ordered();
            unit.insert("name".to_string(), JsonValue::String(name.to_string()));
            unit.insert("version".to_string(), JsonValue::from(state.version));
            unit
        }),
    );
    document.insert("global".to_string(), state.global.clone());
    document.insert(
        "tables".to_string(),
        JsonValue::Object(
            state
                .tables
                .iter()
                .map(|(table, records)| {
                    (
                        table.clone(),
                        JsonValue::Object(
                            records
                                .iter()
                                .map(|(key, value)| (key.clone(), value.clone()))
                                .collect(),
                        ),
                    )
                })
                .collect(),
        ),
    );
    format!(
        "{}\n",
        serde_json::to_string_pretty(&JsonValue::Object(document)).expect("unit state serializes")
    )
}

/// Parse file content into unit state, validating shape and version (TS
/// `parse`).
pub fn parse(text: &str, descriptor: &KvUnitDescriptor) -> Result<UnitState, StorageError> {
    let malformed = |detail: &str| {
        StorageError::new(
            StorageErrorCode::MalformedMedium,
            format!("unit '{}': {detail}", descriptor.name),
        )
    };
    let document: JsonValue = serde_json::from_str(text)
        .map_err(|error| malformed(&format!("file is not valid JSON: {error}")))?;
    let Some(object) = document.as_object() else {
        return Err(malformed("file is not a JSON object"));
    };
    let Some(unit) = object.get("unit").and_then(|unit| unit.as_object()) else {
        return Err(malformed("missing or foreign unit header"));
    };
    if unit.get("name").and_then(|name| name.as_str()) != Some(descriptor.name.as_str()) {
        return Err(malformed("missing or foreign unit header"));
    }
    let version = unit
        .get("version")
        .and_then(|version| version.as_u64())
        .ok_or_else(|| malformed("missing or foreign unit header"))?;
    if version != descriptor.version {
        return Err(StorageError::new(
            StorageErrorCode::VersionMismatch,
            format!(
                "unit '{}': stored version {version} != expected {}",
                descriptor.name, descriptor.version
            ),
        ));
    }
    let tables_value = object
        .get("tables")
        .ok_or_else(|| malformed("tables is not an object"))?;
    let tables = tables_value
        .as_object()
        .ok_or_else(|| malformed("tables is not an object"))?;
    let global = object.get("global").cloned().unwrap_or(JsonValue::Null);
    let mut state = UnitState {
        version,
        global,
        tables: IndexMap::new(),
    };
    for table in &descriptor.tables {
        match tables.get(table) {
            None => {
                state.tables.insert(table.clone(), IndexMap::new());
            }
            Some(records) => match records.as_object() {
                Some(records) => {
                    state.tables.insert(
                        table.clone(),
                        records
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect(),
                    );
                }
                None => {
                    return Err(malformed(&format!("table '{table}' is not an object")));
                }
            },
        }
    }
    Ok(state)
}
