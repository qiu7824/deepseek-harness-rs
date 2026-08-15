//! Storage hub (`ctx.storage`). Rust port of
//! `packages/storage/storage/src/index.ts`.
//!
//! A named backend registry plus mounted data-form facilities. The hub
//! itself performs no IO — backends own media, data forms (the domain
//! layer first) own semantics.
//!
//! # Deviations
//!
//! - The TS `StorageForms` declaration-merging map collapses to a string →
//!   [`ArcValue`] table (`mount`/`form` take a form name); typed consumers
//!   downcast the returned facility.
//! - Backend plugin lifecycle wiring (`storageBackendServiceKey` injection)
//!   is exported for the composition milestone; the Rust registry
//!   registration itself is synchronous.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use cordis::{ArcValue, Context, Disposer, Service, make_disposer};

use crate::error::{StorageError, StorageErrorCode};
use crate::registry::BackendRegistry;

/// Derive the Cordis lifecycle service that one named backend plugin
/// provides (TS `storageBackendServiceKey`).
pub fn storage_backend_service_key(name: &str) -> String {
    format!("storage.backend.{name}")
}

/// The storage hub service (TS `Storage`).
pub struct Storage {
    /// Named backend table; multiple backends stay mounted side by side.
    pub backend: BackendRegistry,
    forms: Arc<Mutex<HashMap<String, ArcValue>>>,
}

impl Service for Storage {
    fn service_name(&self) -> &'static str {
        "storage"
    }
}

impl Storage {
    /// Create the hub and register it as `ctx.storage` (TS constructor).
    pub fn install(ctx: &Context) -> Arc<Self> {
        let service = Arc::new(Self {
            backend: BackendRegistry::new(),
            forms: Arc::new(Mutex::new(HashMap::new())),
        });
        ctx.register_service(service.clone());
        service
    }

    /// Mount a data-form facility on the hub (TS `mount`). Mounting is an
    /// effect: the returned disposer unmounts the form.
    pub fn mount(&self, form: &str, facility: ArcValue) -> Result<Disposer, StorageError> {
        {
            let mut forms = self.forms.lock();
            if forms.contains_key(form) {
                return Err(StorageError::new(
                    StorageErrorCode::DuplicateMount,
                    format!("storage form '{form}' is already mounted"),
                ));
            }
            forms.insert(form.to_string(), facility.clone());
        }
        let forms = Arc::clone(&self.forms);
        let form = form.to_string();
        Ok(make_disposer(move || {
            let forms = forms.clone();
            let form = form.clone();
            let facility = facility.clone();
            Box::pin(async move {
                let mut forms = forms.lock();
                // Same stale-disposer guard as BackendRegistry.register.
                if forms
                    .get(&form)
                    .is_some_and(|current| std::sync::Arc::ptr_eq(current, &facility))
                {
                    forms.remove(&form);
                }
            })
        }))
    }

    /// Resolve a mounted data form (TS `form`).
    pub fn form(&self, form: &str) -> Result<ArcValue, StorageError> {
        match self.forms.lock().get(form) {
            Some(facility) => Ok(facility.clone()),
            None => Err(StorageError::new(
                StorageErrorCode::FormNotMounted,
                format!("storage form '{form}' is not mounted"),
            )),
        }
    }
}

/// The no-op companion installer (TS `invariant.ts`).
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-storage";

/// Cordis companion plugin name (TS `name`).
pub const INVARIANT_NAME: &str = "storage-invariant";

/// Service required before the companion can reserve package ownership.
pub const INVARIANT_INJECT: [&str; 1] = ["invariants"];
