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

use dsh_storage::{KvUnitDescriptor, unit_name_matches};

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
    let name = name.into();
    if !unit_name_matches(&name) {
        return Err(format!("domain name '{name}' must match ^[a-z][a-z0-9_]*$"));
    }
    for table in tables.keys() {
        if !unit_name_matches(table) {
            return Err(format!(
                "domain '{name}' table name '{table}' must match ^[a-z][a-z0-9_]*$"
            ));
        }
    }
    if let Some(global) = &global {
        if (global.schema)(&JsonValue::Null).is_ok() {
            return Err(format!(
                "domain '{name}' global schema must not accept null: \
                 null is the medium's \"never written\" sentinel, so a stored null could not round-trip"
            ));
        }
    }
    Ok(DomainSpec {
        name,
        version,
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
    }
}
