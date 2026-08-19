//! Named backend registry of the storage hub. Rust port of
//! `packages/storage/storage/src/registry.ts`.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::backend::StorageBackend;
use crate::error::{StorageError, StorageErrorCode};

/// Mutable name → backend table (TS `BackendRegistry`). Multiple backends
/// stay mounted side by side; which backend serves which consumer is the
/// consumer's configuration (e.g. the domain layer's route table), never a
/// hub-global choice.
pub struct BackendRegistry {
    backends: Arc<Mutex<HashMap<String, Arc<dyn StorageBackend>>>>,
}

impl Default for BackendRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl BackendRegistry {
    pub fn new() -> Self {
        Self {
            backends: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a named backend. The returned disposer removes the name
    /// (only this registration's contribution: a stale disposer firing
    /// again after dispose + re-register must not remove the successor).
    /// Disposal does NOT close the backend — the owning plugin closes it
    /// after unregistering.
    pub fn register(
        &self,
        name: &str,
        backend: Arc<dyn StorageBackend>,
    ) -> Result<cordis::Disposer, StorageError> {
        {
            let mut backends = self.backends.lock();
            if backends.contains_key(name) {
                return Err(StorageError::new(
                    StorageErrorCode::DuplicateBackend,
                    format!("storage backend '{name}' is already registered"),
                ));
            }
            backends.insert(name.to_string(), backend.clone());
        }
        let backends = Arc::clone(&self.backends);
        let name = name.to_string();
        Ok(cordis::make_disposer(move || {
            let backends = backends.clone();
            let name = name.clone();
            let backend = backend.clone();
            Box::pin(async move {
                let mut backends = backends.lock();
                if backends
                    .get(&name)
                    .is_some_and(|current| Arc::ptr_eq(current, &backend))
                {
                    backends.remove(&name);
                }
            })
        }))
    }

    /// Resolve a backend by name.
    pub fn get(&self, name: &str) -> Result<Arc<dyn StorageBackend>, StorageError> {
        let backends = self.backends.lock();
        match backends.get(name) {
            Some(backend) => Ok(backend.clone()),
            None => {
                let names: Vec<&str> = backends.keys().map(|key| key.as_str()).collect();
                Err(StorageError::new(
                    StorageErrorCode::BackendNotFound,
                    format!(
                        "storage backend '{name}' is not registered (registered: {} )",
                        if names.is_empty() {
                            "none".to_string()
                        } else {
                            names.join(", ")
                        }
                    ),
                ))
            }
        }
    }

    /// Registered backend names, for diagnostics.
    pub fn names(&self) -> Vec<String> {
        self.backends.lock().keys().cloned().collect()
    }
}
