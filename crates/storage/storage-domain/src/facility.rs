//! The mounted domain facility. Rust port of
//! `packages/storage/storage-domain/src/index.ts` (the `DomainFacility`
//! class + plugin wiring).
//!
//! # Deviations
//!
//! - The TS plugin `apply`/`inject` (`storageBackendServiceKey` lifecycle
//!   services) collapses into [`DomainFacility::install`], which mounts the
//!   `domain` form on the hub and publishes the facility as
//!   `ctx.storageDomain`. The facility resolves backends through the hub at
//!   open time, so a backend registered later still serves.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::Value as JsonValue;

use cordis::{ArcValue, Context, Service, arc};
use dsh_storage::Storage;

use crate::domain::Domain;
use crate::spec::{DomainSpec, descriptor_of};

/// Plugin config (TS `Config`). Which backend serves which domain is
/// decided here, not globally on the hub: `backend` is the default route
/// and `routes` overrides it per domain name.
#[derive(Debug, Clone, Default)]
pub struct DomainFacilityConfig {
    /// Default backend name for every domain without an explicit route.
    pub backend: String,
    /// Per-domain overrides: domain name → backend name.
    pub routes: HashMap<String, String>,
}

/// The mounted domain facility (TS `DomainFacility`): opens declared
/// domains over routed backends; one instance owns the open-domain table
/// and enforces single-open per domain name.
pub struct DomainFacility {
    ctx: Context,
    storage: Arc<Storage>,
    config: DomainFacilityConfig,
    domains: Arc<Mutex<HashMap<String, Arc<Domain>>>>,
    reserved: Arc<Mutex<HashSet<String>>>,
}

impl Service for DomainFacility {
    fn service_name(&self) -> &'static str {
        "storageDomain"
    }
}

impl DomainFacility {
    /// Create the facility, mount the `domain` form on the hub, and
    /// publish it as `ctx.storageDomain` (TS plugin `apply`).
    pub fn install(ctx: &Context, config: DomainFacilityConfig) -> Result<Arc<Self>, String> {
        let storage = ctx
            .get_typed::<Arc<Storage>>("storage", false)
            .ok_or_else(|| "the storage hub is not configured".to_string())?
            .as_ref()
            .clone();
        let facility = Arc::new(Self {
            ctx: ctx.clone(),
            storage: storage.clone(),
            config,
            domains: Arc::new(Mutex::new(HashMap::new())),
            reserved: Arc::new(Mutex::new(HashSet::new())),
        });
        ctx.register_service(facility.clone());
        // Mount the domain form; unmounting closes every leftover domain
        // (TS effect: close leftovers before unmounting).
        let facility_for_mount = facility.clone();
        let mount_dispose = storage.mount("domain", arc(facility.clone())).map_err(|e| e.message)?;
        let _ = ctx.effect(
            "storageDomain.mount",
            Box::pin(async move {
                Some(cordis::make_disposer(move || {
                    let facility = facility_for_mount.clone();
                    let mount_dispose = mount_dispose.clone();
                    Box::pin(async move {
                        facility.close_all().await;
                        mount_dispose().await;
                    })
                }))
            }),
        );
        Ok(facility)
    }

    /// Open one declared domain (TS `open`). Steps, each failing the whole
    /// call: reject `already-open`; resolve the backend route
    /// (`backend-not-found` passes through from the hub); require its `kv`
    /// facet (`facet-unsupported`); open the unit projected from the spec
    /// (`version-mismatch`/`malformed-medium` pass through); load and
    /// validate every stored record (`invalid-record` prose); construct the
    /// domain.
    pub async fn open(&self, spec: &DomainSpec) -> Result<Arc<Domain>, String> {
        if self.reserved.lock().contains(&spec.name) {
            return Err(format!("domain '{}' is already open", spec.name));
        }
        self.reserved.lock().insert(spec.name.clone());
        let outcome = (async {
            let backend_name = self
                .config
                .routes
                .get(&spec.name)
                .cloned()
                .unwrap_or_else(|| self.config.backend.clone());
            let backend = self
                .storage
                .backend
                .get(&backend_name)
                .map_err(|error| error.message)?;
            let kv = backend.kv().ok_or_else(|| {
                format!(
                    "backend '{backend_name}' routed for domain '{}' has no kv facet",
                    spec.name
                )
            })?;
            let unit = kv.open(&descriptor_of(spec)).await.map_err(|error| error.message)?;
            match self.build(spec, unit).await {
                Ok(domain) => Ok(domain),
                Err(error) => Err(error),
            }
        })
        .await;
        match &outcome {
            Ok(domain) => {
                self.domains.lock().insert(spec.name.clone(), domain.clone());
                Ok(domain.clone())
            }
            Err(_) => {
                self.reserved.lock().remove(&spec.name);
                outcome
            }
        }
    }

    /// Load, validate, and construct one domain from an opened unit (the
    /// inner half of TS `open`; the unit is closed on failure).
    async fn build(
        &self,
        spec: &DomainSpec,
        unit: Arc<dyn dsh_storage::KvUnit>,
    ) -> Result<Arc<Domain>, String> {
        let snapshot = match unit.load_all().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let _ = unit.close().await;
                return Err(error.message);
            }
        };
        let mut tables = HashMap::new();
        for (table_name, table_spec) in &spec.tables {
            let mut records = HashMap::new();
            let stored = snapshot.tables.get(table_name).cloned().unwrap_or_default();
            for (key, raw) in stored {
                if let Err(error) = (table_spec.value_schema)(&raw) {
                    let _ = unit.close().await;
                    return Err(format!(
                        "domain '{}': stored record '{key}' in table '{table_name}' does not match its schema: {error}",
                        spec.name
                    ));
                }
                records.insert(key, raw);
            }
            tables.insert(table_name.clone(), records);
        }
        let global_value = match &spec.global {
            None => None,
            Some(global_spec) => {
                if snapshot.global.is_null() {
                    Some(global_spec.initial.clone())
                } else {
                    if let Err(error) = (global_spec.schema)(&snapshot.global) {
                        let _ = unit.close().await;
                        return Err(format!(
                            "domain '{}': stored global does not match its schema: {error}",
                            spec.name
                        ));
                    }
                    Some(snapshot.global)
                }
            }
        };
        let domain_name = spec.name.clone();
        let domains = Arc::clone(&self.domains);
        let reserved = Arc::clone(&self.reserved);
        Ok(Domain::new(
            self.ctx.clone(),
            domain_name.clone(),
            unit,
            tables,
            global_value,
            Arc::new(move || {
                domains.lock().remove(&domain_name);
                reserved.lock().remove(&domain_name);
            }),
        ))
    }

    /// Look up an open domain by name (TS `get`).
    pub fn get(&self, name: &str) -> Option<Arc<Domain>> {
        self.domains.lock().get(name).cloned()
    }

    /// Close every domain still open on this facility (TS `closeAll`).
    pub async fn close_all(&self) {
        let domains: Vec<Arc<Domain>> = self.domains.lock().values().cloned().collect();
        for domain in domains {
            domain.close().await;
        }
    }

    /// The hub this facility is mounted on.
    pub fn storage(&self) -> &Arc<Storage> {
        &self.storage
    }
}

/// Kept for the JSON value vocabulary used by consumers.
pub type DomainJson = JsonValue;

/// The form name the facility mounts under (TS `StorageForms.domain`).
pub const DOMAIN_FORM: &str = "domain";

/// The facility handle stored in the hub's form table.
pub fn domain_form(storage: &Storage) -> Result<Arc<DomainFacility>, String> {
    let value: ArcValue = storage.form(DOMAIN_FORM).map_err(|error| error.message)?;
    cordis::downcast_arc::<Arc<DomainFacility>>(&value)
        .map(|facility| facility.as_ref().clone())
        .ok_or_else(|| "storage form 'domain' does not hold a DomainFacility".to_string())
}
