//! Shared test helpers for the credentials suite: the in-memory provider
//! (Rust port of the TS `tests/memory.ts`).

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use cordis::Context;
use parking_lot::Mutex;

use dsh_credentials::{CredentialInfo, CredentialProvider, CredentialRef, ResolvedCredential};

/// One always-writable `memory` source seeded from a map.
pub struct MemoryCredentials {
    ctx: Context,
    store: Mutex<HashMap<String, String>>,
}

impl MemoryCredentials {
    /// Construct, seed, and register as `ctx.credentials`.
    pub fn install(ctx: &Context, seed: &[(&str, &str)]) -> Arc<Self> {
        let store = seed
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        let provider = Arc::new(Self { ctx: ctx.clone(), store: Mutex::new(store) });
        let erased: Arc<dyn CredentialProvider> = provider.clone();
        ctx.register_service(erased);
        provider
    }
}

#[async_trait::async_trait]
impl CredentialProvider for MemoryCredentials {
    async fn resolve(&self, reference: &CredentialRef) -> Option<ResolvedCredential> {
        let value = self.store.lock().get(reference.as_str()).cloned();
        match value {
            Some(value) if !value.is_empty() => {
                Some(ResolvedCredential { value, source: "memory".to_string() })
            }
            _ => None,
        }
    }

    async fn describe(&self, reference: &CredentialRef) -> CredentialInfo {
        let configured = self
            .store
            .lock()
            .get(reference.as_str())
            .is_some_and(|value| !value.is_empty());
        CredentialInfo {
            configured,
            source: configured.then(|| "memory".to_string()),
            writable: true,
        }
    }

    async fn set(&self, reference: &CredentialRef, value: &str) -> Result<(), String> {
        if value.is_empty() {
            return Err(
                "memory credentials: an empty value cannot be stored; use unset".to_string()
            );
        }
        self.store.lock().insert(reference.as_str().to_string(), value.to_string());
        self.notify_updated(&self.ctx, reference).await
    }

    async fn unset(&self, reference: &CredentialRef) -> Result<(), String> {
        let removed = self.store.lock().remove(reference.as_str()).is_some();
        if removed {
            self.notify_updated(&self.ctx, reference).await?;
        }
        Ok(())
    }
}
