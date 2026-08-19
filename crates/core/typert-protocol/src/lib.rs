//! Typert lookup provider registry: Rust port of the
//! `@deepseek-ai/dsh-typert-protocol` subset the session store consumes.
//!
//! The full protocol (Remote decorators, registry contracts, codecs) belongs
//! to the typert milestone. This crate delivers the pieces the core depends
//! on today: the RPC segment grammar, the lookup failure wrapper, and the
//! runtime lookup-provider registry that `dsh-session` registers into.

use std::collections::HashMap;
use std::sync::Arc;

use cordis::{ArcValue, Disposer, Service, make_disposer};
use parking_lot::Mutex;

/// Test one generated Remote name against the Connection endpoint grammar.
/// (TS `isTypertRemoteSegment`.)
pub fn is_typert_remote_segment(value: &str) -> bool {
    value != "." && value != ".." && {
        // Character-class check without pulling in regex: the TS pattern is a
        // simple one-or-more class.
        !value.is_empty()
            && value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '.' | '-'))
    }
}

/// A lookup policy rejection whose typed payload belongs to the active
/// boundary adapter.
#[derive(Debug)]
pub struct TypertLookupFailure {
    /// Adapter-owned failure returned to the caller.
    pub failure: ArcValue,
}

impl TypertLookupFailure {
    pub fn new(failure: ArcValue) -> Self {
        Self { failure }
    }
}

impl std::fmt::Display for TypertLookupFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Typert lookup policy rejected the requested identity")
    }
}

impl std::error::Error for TypertLookupFailure {}

/// One registered lookup provider (TS `TypertLookupDefinition` +
/// `TypertLookup.resolve`).
pub struct TypertLookup {
    /// Merge-declared lookup key.
    pub key: String,
    /// Source parameter name recognized by the SRC weak parser.
    pub parameter: String,
    /// Wire field replacing the Host object parameter.
    pub wire: String,
    /// Canonical Host type symbol used by strict generation.
    pub host_type_symbol: String,
    /// Canonical wire type symbol used by strict generation.
    pub wire_type_symbol: String,
    /// Resolve a wire identity through the provider's default policy.
    pub resolve: Arc<dyn Fn(&str) -> Option<ArcValue> + Send + Sync>,
}

/// Runtime registry for Host object lookup providers (TS
/// `TypertLookupRegistry`).
#[derive(Default)]
pub struct LookupRegistry {
    lookups: Arc<Mutex<HashMap<String, Arc<TypertLookup>>>>,
}

impl LookupRegistry {
    /// Register one provider under its merge-declared key; the returned
    /// disposer withdraws the exact provider.
    pub fn register(&self, key: &str, provider: TypertLookup) -> Disposer {
        if key.is_empty() || !is_typert_remote_segment(key) {
            panic!("typert-protocol: lookup key must contain only RPC endpoint segment characters");
        }
        if !is_typert_remote_segment(&provider.parameter)
            || !is_typert_remote_segment(&provider.wire)
        {
            panic!("typert-protocol: lookup parameter and wire must be valid RPC segments");
        }
        if self.lookups.lock().contains_key(key) {
            panic!("typert-protocol: lookup \"{key}\" is already registered");
        }
        let provider = Arc::new(provider);
        self.lookups
            .lock()
            .insert(key.to_string(), provider.clone());
        let lookups = Arc::clone(&self.lookups);
        let key = key.to_string();
        make_disposer(move || {
            let lookups = lookups.clone();
            let key = key.clone();
            Box::pin(async move {
                lookups.lock().remove(&key);
            })
        })
    }

    /// Resolve a wire identity through the registered provider.
    pub fn resolve(&self, key: &str, id: &str) -> Option<ArcValue> {
        self.lookups
            .lock()
            .get(key)
            .and_then(|lookup| (lookup.resolve)(id))
    }

    /// The registered lookup keys, in registration order.
    pub fn keys(&self) -> Vec<String> {
        self.lookups.lock().keys().cloned().collect()
    }
}

/// The `typert` Cordis service: the lookup registry plus the (later
/// milestone) invocation registries.
pub struct TypertService {
    pub lookups: Arc<LookupRegistry>,
    /// Host-context providers (TS `TypertContextRegistry`): resolve a wire
    /// identity to a live scoped Context.
    pub host_contexts: Arc<LookupRegistry>,
}

impl TypertService {
    /// Create and register the `typert` service on `ctx`.
    pub fn install(ctx: &cordis::Context) -> Arc<Self> {
        let service = Arc::new(Self {
            lookups: Arc::new(LookupRegistry::default()),
            host_contexts: Arc::new(LookupRegistry::default()),
        });
        ctx.register_service(service.clone());
        service
    }
}

impl Service for TypertService {
    fn service_name(&self) -> &'static str {
        "typert"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_grammar() {
        assert!(is_typert_remote_segment("abc"));
        assert!(is_typert_remote_segment("a.b-c_$9"));
        assert!(!is_typert_remote_segment("."));
        assert!(!is_typert_remote_segment(".."));
        assert!(!is_typert_remote_segment(""));
        assert!(!is_typert_remote_segment("a/b"));
        assert!(!is_typert_remote_segment("a b"));
    }

    #[tokio::test]
    async fn lookup_registry_resolves_and_disposes() {
        let registry = LookupRegistry::default();
        let dispose = registry.register(
            "session",
            TypertLookup {
                key: "session".to_string(),
                parameter: "session".to_string(),
                wire: "sessionId".to_string(),
                host_type_symbol: "@deepseek-ai/dsh-session#Session".to_string(),
                wire_type_symbol: "@deepseek-ai/dsh-session/types#SessionId".to_string(),
                resolve: Arc::new(|id| {
                    (id == "s1").then(|| cordis::arc("session-object".to_string()))
                }),
            },
        );
        assert_eq!(registry.keys(), vec!["session"]);
        assert_eq!(
            registry
                .resolve("session", "s1")
                .and_then(|v| cordis::util::downcast::<String>(&v).cloned()),
            Some("session-object".to_string())
        );
        assert!(registry.resolve("session", "other").is_none());
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                registry.register(
                    "session",
                    TypertLookup {
                        key: "session".to_string(),
                        parameter: "s".to_string(),
                        wire: "w".to_string(),
                        host_type_symbol: "h".to_string(),
                        wire_type_symbol: "w".to_string(),
                        resolve: Arc::new(|_| None),
                    },
                )
            }))
            .is_err()
        );
        dispose().await;
        assert!(registry.keys().is_empty());
    }

    #[tokio::test]
    async fn service_installs_on_context() {
        let ctx = cordis::Context::root();
        let service = TypertService::install(&ctx);
        let read: Option<Arc<Arc<TypertService>>> = ctx.get_typed("typert", false);
        assert!(read.is_some());
        assert_eq!(service.lookups.keys().len(), 0);
    }
}
