//! Domain declaration vocabulary. Rust port of
//! `packages/storage/storage-domain/src/spec.ts`.
//!
//! A spec object is the single source of a domain's identity, layout, and
//! record schemas: the owning package defines it once with
//! [`define_domain`] and both the type surface and the runtime (validation,
//! descriptor projection) derive from it. The TS zod schemas become JSON
//! validation closures (documented collapse).

use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::Value as JsonValue;

use dsh_storage::{KvLayout, KvUnitDescriptor, unit_name_matches};

/// A record validation closure: `Ok(())` accepts, `Err(message)` rejects
/// (the TS `ZodType::parse` translated at the durable boundary).
pub type RecordSchema = Arc<dyn Fn(&JsonValue) -> Result<(), String> + Send + Sync>;

/// Global singleton declaration: schema plus the value used before the
/// first write (TS `DomainGlobalSpec`).
#[derive(Clone)]
pub struct DomainGlobalSpec {
    /// Validates the stored global at the durable boundary.
    pub schema: RecordSchema,
    /// Value served when the medium holds no global yet; not written until
    /// the first `set`.
    pub initial: JsonValue,
}

/// One table declaration (TS `DomainTableSpec`; the phantom key type
/// carrier is a compile-time-only TS device, inexpressible here).
#[derive(Clone)]
pub struct DomainTableSpec {
    /// Validates every stored record at the durable boundary.
    pub value_schema: RecordSchema,
}

/// Policy for a table record that fails its durable schema.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InvalidRecordPolicy {
    /// Authoritative data must make the whole open fail.
    #[default]
    FailLoud,
    /// Disposable derived data may be backed up and treated as absent when
    /// the selected backend supports record-level backup.
    BackupAndSkip,
}

/// Static declaration of one domain: identity, version, and record layout
/// (TS `DomainSpec`).
#[derive(Clone)]
pub struct DomainSpec {
    /// Domain name; must match [`unit_name_matches`] (doubles as the
    /// backend unit name).
    pub name: String,
    /// Domain format version; a medium stamped with a different version
    /// rejects at open.
    pub version: u64,
    /// Physical medium layout. Existing domains remain single-document.
    pub layout: KvLayout,
    /// Older per-record versions whose records satisfy the current schemas.
    pub compatible_versions: Vec<u64>,
    /// Invalid table-record handling. The global slot always fails loud.
    pub invalid_records: InvalidRecordPolicy,
    /// Optional global singleton slot.
    pub global: Option<DomainGlobalSpec>,
    /// Table declarations keyed by table name; each name must match
    /// [`unit_name_matches`].
    pub tables: IndexMap<String, DomainTableSpec>,
}

/// Declare one table (TS `domainTable`).
pub fn domain_table(schema: RecordSchema) -> DomainTableSpec {
    DomainTableSpec {
        value_schema: schema,
    }
}

/// Declare a global singleton slot (the TS `global: { schema, initial }`
/// inline shape).
pub fn domain_global(schema: RecordSchema, initial: JsonValue) -> DomainGlobalSpec {
    DomainGlobalSpec { schema, initial }
}

/// Identity helper that pins a spec and validates its fields (TS
/// `defineDomain`). Misconfiguration fails loud at the owning package's
/// load, before any medium is touched.
pub fn define_domain(
    name: impl Into<String>,
    version: u64,
    global: Option<DomainGlobalSpec>,
    tables: IndexMap<String, DomainTableSpec>,
) -> Result<DomainSpec, String> {
    define_domain_with_options(
        name,
        version,
        KvLayout::Single,
        Vec::new(),
        InvalidRecordPolicy::FailLoud,
        global,
        tables,
    )
}

/// Declare a domain with explicit storage compatibility options. This keeps
/// [`define_domain`] source-compatible for authoritative single-layout data.
pub fn define_domain_with_options(
    name: impl Into<String>,
    version: u64,
    layout: KvLayout,
    compatible_versions: Vec<u64>,
    invalid_records: InvalidRecordPolicy,
    global: Option<DomainGlobalSpec>,
    tables: IndexMap<String, DomainTableSpec>,
) -> Result<DomainSpec, String> {
    let name = name.into();
    if !unit_name_matches(&name) {
        return Err(format!("domain name '{name}' must match ^[a-z][a-z0-9_]*$"));
    }
    if layout == KvLayout::Single && !compatible_versions.is_empty() {
        return Err(format!(
            "domain '{name}' compatibleVersions requires per-record layout"
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for compatible in &compatible_versions {
        if *compatible == 0 || *compatible >= version || !seen.insert(*compatible) {
            return Err(format!(
                "domain '{name}' compatibleVersions entries must be distinct positive integers below version {version}, got {compatible}"
            ));
        }
    }
    for table in tables.keys() {
        if !unit_name_matches(table) {
            return Err(format!(
                "domain '{name}' table name '{table}' must match ^[a-z][a-z0-9_]*$"
            ));
        }
    }
    if let Some(global) = &global
        && (global.schema)(&JsonValue::Null).is_ok()
    {
        return Err(format!(
            "domain '{name}' global schema must not accept null: \
                 null is the medium's \"never written\" sentinel, so a stored null could not round-trip"
        ));
    }
    Ok(DomainSpec {
        name,
        version,
        layout,
        compatible_versions,
        invalid_records,
        global,
        tables,
    })
}

/// Project a spec onto the backend-facing unit descriptor (TS
/// `descriptorOf`).
pub fn descriptor_of(spec: &DomainSpec) -> KvUnitDescriptor {
    KvUnitDescriptor {
        name: spec.name.clone(),
        version: spec.version,
        tables: spec.tables.keys().cloned().collect(),
        has_global: spec.global.is_some(),
        layout: spec.layout,
        compatible_versions: spec.compatible_versions.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_storage::KvLayout;

    #[test]
    fn descriptor_projects_per_record_compatibility() {
        let spec = define_domain_with_options(
            "cache",
            5,
            KvLayout::PerRecord,
            vec![3, 4],
            InvalidRecordPolicy::BackupAndSkip,
            None,
            IndexMap::new(),
        )
        .unwrap();

        let descriptor = descriptor_of(&spec);
        assert_eq!(descriptor.layout, KvLayout::PerRecord);
        assert_eq!(descriptor.compatible_versions, vec![3, 4]);
        assert_eq!(spec.invalid_records, InvalidRecordPolicy::BackupAndSkip);
    }

    #[test]
    fn compatible_versions_must_be_older_than_current() {
        let error = define_domain_with_options(
            "cache",
            5,
            KvLayout::PerRecord,
            vec![5],
            InvalidRecordPolicy::FailLoud,
            None,
            IndexMap::new(),
        )
        .err()
        .expect("current version must be rejected as compatible");
        assert!(error.contains("below version 5"));
    }

    #[test]
    fn compatibility_rejects_zero_duplicates_and_single_layout() {
        for (layout, versions) in [
            (KvLayout::PerRecord, vec![0]),
            (KvLayout::PerRecord, vec![3, 3]),
            (KvLayout::Single, vec![3]),
        ] {
            assert!(
                define_domain_with_options(
                    "cache",
                    5,
                    layout,
                    versions,
                    InvalidRecordPolicy::FailLoud,
                    None,
                    IndexMap::new()
                )
                .is_err()
            );
        }
    }
}
