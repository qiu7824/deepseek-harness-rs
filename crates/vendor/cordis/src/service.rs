//! Base service trait and service lifecycle helpers.
//!
//! Rust port of `vendor/cordis/src/service.ts`.

use std::sync::Arc;

use crate::context::Context;
use crate::util::{ArcValue, arc};

/// Availability predicate: consulted before dependents may load.
pub type CheckFn = Arc<dyn Fn(&Context) -> bool + Send + Sync>;

/// Base trait for services that expose a named API on `ctx`.
///
/// TS subclasses call `super(ctx, name)` from their constructor; the Rust
/// equivalent registers via [`Context::register_service`] once, returning a
/// disposer that unregisters with the owning fiber.
pub trait Service: Send + Sync + 'static {
    /// The service name this instance is registered under.
    fn service_name(&self) -> &'static str;

    /// Optional availability predicate consulted before dependents load.
    fn service_check(&self, _ctx: &Context) -> bool {
        true
    }
}

impl Context {
    /// Register a service implementation owned by the current fiber.
    ///
    /// The service becomes visible to dependents in the same isolation scope
    /// once the fiber is active; it is unregistered (waking dependents) when
    /// the returned disposer runs or the fiber unloads.
    pub fn register_service<S: Service + ?Sized>(
        &self,
        service: Arc<S>,
    ) -> crate::events::Disposer {
        let check: CheckFn = Arc::new({
            let service = service.clone();
            move |ctx: &Context| service.service_check(ctx)
        });
        self.reflect.provide(
            self,
            service.service_name(),
            Some(arc(service)),
            Some(check),
        )
    }

    /// Read a service from the store without the inject requirement.
    pub fn get(&self, name: &str, strict: bool) -> Option<ArcValue> {
        self.reflect.get(self, name, strict)
    }

    /// Read a service typed as `T` from the store.
    pub fn get_typed<T: Send + Sync + 'static>(&self, name: &str, strict: bool) -> Option<Arc<T>> {
        let value = self.reflect.get(self, name, strict)?;
        crate::util::downcast_arc::<T>(&value)
    }

    /// Register a service implementation owned by the current fiber.
    pub fn provide(&self, name: &str, value: Option<ArcValue>) -> crate::events::Disposer {
        self.reflect.provide(self, name, value, None)
    }

    /// Overwrite a provided service's value (same-fiber only).
    pub fn set(&self, name: &str, value: ArcValue) -> Result<(), String> {
        self.reflect.set(self, name, value)
    }
}
