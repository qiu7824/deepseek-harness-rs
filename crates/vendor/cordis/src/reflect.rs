//! Reflection and service-resolution layer installed as `ctx.reflect`.
//!
//! Rust port of `vendor/cordis/src/reflect.ts`. Methods take the *caller*
//! context explicitly — the TS runtime rebinds `this` to the accessing
//! context via its proxy/mixin machinery, which has no Rust equivalent.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::context::Context;
use crate::events::Disposer;
use crate::fiber::{FiberCore, FiberState};
use crate::util::{ArcValue, arc};

/// Concrete service implementation record stored in the root reflect service.
pub struct Impl {
    /// The service name.
    pub name: String,
    /// The fiber that provided the service (owns its lifetime).
    pub fiber: Arc<FiberCore>,
    /// The current service value.
    pub value: Option<ArcValue>,
    /// Optional availability predicate consulted before dependents may load.
    pub check: Option<Arc<dyn Fn(&Context) -> bool + Send + Sync>>,
}

/// Computed context property backed by custom get/set hooks.
pub struct Accessor {
    pub get: Arc<dyn Fn(&Context) -> ArcValue + Send + Sync>,
    pub set: Option<Arc<dyn Fn(&Context, ArcValue) -> bool + Send + Sync>>,
}

/// Context property definition known by the reflection service.
pub enum Property {
    Service,
    Accessor(Arc<Accessor>),
}

/// Reflection and service-resolution layer installed as `ctx.reflect`.
pub struct ReflectService {
    /// Service implementations, keyed by isolation label.
    pub store: Arc<Mutex<HashMap<u64, Arc<Impl>>>>,
    /// Declared context properties (services and accessors), by name.
    pub props: Arc<Mutex<HashMap<String, Property>>>,
}

impl Default for ReflectService {
    fn default() -> Self {
        Self {
            store: Arc::new(Mutex::new(HashMap::new())),
            props: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl ReflectService {
    /// Read a service from the store without the inject requirement.
    pub fn get(&self, caller: &Context, name: &str, strict: bool) -> Option<ArcValue> {
        self.get_impl(caller, name, strict)
            .and_then(|impl_| impl_.value.clone())
    }

    /// Resolve the implementation record for a service name.
    pub fn get_impl(&self, caller: &Context, name: &str, strict: bool) -> Option<Arc<Impl>> {
        let label = caller.isolate_label(name)?;
        let impl_ = self.store.lock().get(&label).cloned()?;
        if strict && impl_.fiber.state() != FiberState::Active {
            return None;
        }
        Some(impl_)
    }

    /// Overwrite a provided service's value (only the providing fiber may).
    pub fn set(&self, caller: &Context, name: &str, value: ArcValue) -> Result<(), String> {
        let label = caller
            .isolate_label(name)
            .ok_or_else(|| format!("cannot set property \"{name}\" without provide"))?;
        let existing = self
            .store
            .lock()
            .get(&label)
            .cloned()
            .ok_or_else(|| format!("cannot set property \"{name}\" without provide"))?;
        if !Arc::ptr_eq(&existing.fiber, &caller.fiber) {
            return Err(format!("cannot set property \"{name}\" in multiple fibers"));
        }
        let mut guard = self.store.lock();
        if let Some(record) = guard.get(&label) {
            let updated = Impl {
                name: record.name.clone(),
                fiber: record.fiber.clone(),
                value: Some(value),
                check: record.check.clone(),
            };
            guard.insert(label, Arc::new(updated));
        }
        Ok(())
    }

    /// Register a service implementation owned by the calling fiber.
    ///
    /// Registration is synchronous (mirrors the sync part of the TS effect
    /// body). Panics if the name is already provided in this scope or was
    /// declared as an accessor.
    pub fn provide(
        &self,
        caller: &Context,
        name: &str,
        value: Option<ArcValue>,
        check: Option<Arc<dyn Fn(&Context) -> bool + Send + Sync>>,
    ) -> Disposer {
        let fiber = caller.fiber.clone();
        fiber.assert_active().unwrap_or_else(|error| panic!("{error}"));
        if fiber.state() == FiberState::Unloading {
            panic!("cannot create effect on inactive context");
        }

        {
            let mut props = self.props.lock();
            match props.get(name) {
                Some(Property::Accessor(_)) => {
                    panic!("property \"{name}\" is already declared as accessor");
                }
                _ => {
                    props.insert(name.to_string(), Property::Service);
                }
            }
        }

        let label = caller.isolate_label_ensure(name);
        let impl_ = Arc::new(Impl {
            name: name.to_string(),
            fiber: fiber.clone(),
            value,
            check,
        });
        {
            let mut store = self.store.lock();
            if let Some(existing) = store.get(&label) {
                panic!(
                    "service \"{name}\" has been registered at <{}>",
                    existing.fiber.name()
                );
            }
            store.insert(label, impl_.clone());
        }
        fiber.store_insert_active(name, impl_.clone());
        if fiber.state() == FiberState::Active {
            self.notify(caller, vec![name.to_string()]);
        }

        let store = self.store.clone();
        let caller_for_dispose = caller.clone();
        let name_for_dispose = name.to_string();
        let disposer = crate::events::make_disposer(move || {
            let store = store.clone();
            let caller = caller_for_dispose.clone();
            let name = name_for_dispose.clone();
            Box::pin(async move {
                {
                    store.lock().remove(&label);
                }
                let fibers = ReflectService::notify_impl(&caller, vec![name.clone()]);
                for fiber in fibers {
                    let _ = fiber.settle().await;
                }
                caller.fiber.store_delete_active(&name);
            })
        });
        let _ = caller.fiber.disposables.push(disposer.clone());
        disposer
    }

    /// Re-evaluate every fiber that requires one of the given services.
    pub fn notify(&self, caller: &Context, names: Vec<String>) -> Vec<Arc<FiberCore>> {
        Self::notify_impl(caller, names)
    }

    fn notify_impl(caller: &Context, names: Vec<String>) -> Vec<Arc<FiberCore>> {
        let mut fibers: Vec<Arc<FiberCore>> = Vec::new();
        for runtime in caller.registry.values() {
            for fiber in runtime.fibers.snapshot() {
                let mut has_update = false;
                for name in &names {
                    if !fiber.inject.contains_key(name) {
                        continue;
                    }
                    // scope filter: same isolation label on both sides
                    let my_label = caller.isolate_label(name);
                    let their_label = fiber.ctx().and_then(|ctx| ctx.isolate_label(name));
                    if my_label.is_some() && my_label != their_label {
                        continue;
                    }
                    has_update = true;
                    fiber.check_impl(name);
                }
                if !has_update {
                    continue;
                }
                fiber.refresh();
                fibers.push(fiber);
            }
        }
        for name in names {
            let value = caller
                .reflect
                .get_impl(caller, &name, false)
                .and_then(|impl_| impl_.value.clone())
                .unwrap_or_else(|| arc(()));
            caller
                .events
                .emit(Some(caller), "internal/service", vec![arc(name), value]);
        }
        fibers
    }

    /// Notify dependents of every service provided by `fiber` (used on ACTIVE
    /// boundary crossings).
    pub fn notify_own(&self, fiber: &Arc<FiberCore>) {
        let Some(ctx) = fiber.ctx() else { return };
        let names: Vec<String> = self
            .store
            .lock()
            .values()
            .filter(|impl_| Arc::ptr_eq(&impl_.fiber, fiber))
            .map(|impl_| impl_.name.clone())
            .collect();
        self.notify(&ctx, names);
    }

    /// Define a computed context property backed by get/set hooks.
    pub fn accessor(&self, caller: &Context, name: &str, accessor: Arc<Accessor>) -> Disposer {
        let fiber = caller.fiber.clone();
        fiber.assert_active().unwrap_or_else(|error| panic!("{error}"));
        {
            let mut props = self.props.lock();
            if let Some(existing) = props.get(name) {
                let kind = match existing {
                    Property::Service => "service",
                    Property::Accessor(_) => "accessor",
                };
                panic!("property \"{name}\" is already declared as {kind}");
            }
            props.insert(name.to_string(), Property::Accessor(accessor));
        }
        let props = self.props.clone();
        let name_for_dispose = name.to_string();
        let disposer = crate::events::make_disposer(move || {
            let props = props.clone();
            let name = name_for_dispose.clone();
            Box::pin(async move {
                props.lock().remove(&name);
            })
        });
        let _ = caller.fiber.disposables.push(disposer.clone());
        disposer
    }

    /// Expose selected members of a service directly on `ctx`.
    ///
    /// Deviation: TS proxy machinery forwards members with full typing; Rust
    /// accessors are dynamic. The core convenience methods (`ctx.on`,
    /// `ctx.provide`, ...) are inherent methods on [`Context`] instead. This
    /// exists for parity and is used only by advanced plugins.
    pub fn mixin(&self, caller: &Context, source: &str, keys: Vec<String>) -> Disposer {
        let disposers: Vec<Disposer> = keys
            .into_iter()
            .map(|key| {
                let source = source.to_string();
                self.accessor(
                    caller,
                    &key,
                    Arc::new(Accessor {
                        get: Arc::new(move |ctx| {
                            ctx.get(&source, true).unwrap_or_else(|| arc(()))
                        }),
                        set: None,
                    }),
                )
            })
            .collect();
        crate::events::make_disposer(move || {
            let disposers = disposers.clone();
            Box::pin(async move {
                for disposer in disposers.into_iter().rev() {
                    disposer().await;
                }
            })
        })
    }
}
