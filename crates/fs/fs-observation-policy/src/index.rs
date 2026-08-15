//! Event-only filesystem observation policy; it registers no service. Rust
//! port of `packages/fs/fs-observation-policy/src/index.ts`. A weak
//! owner/target map records every authoritative presence/absence
//! observation, single-slot intent listeners derive guards from that state,
//! and the provider performs the atomic freshness/no-clobber check. Without
//! this plugin, tools retain the bare provider's unconditional mutation
//! behavior.
//!
//! # Deviations
//!
//! - The TS `WeakMap<object, Map<targetKey, observation>>` keys owners by
//!   object identity. The Rust port keys by an [`OwnerKey`] — the opaque
//!   pointer of the owner session — carried in the minimal
//!   [`FsObservationActorHandle`] view the tool layer builds from its
//!   execution before emitting the `fs/*` events (structural narrowing has
//!   no Rust equivalent without importing the tool package).
//! - The TS `ctx.effect` teardown clears the gate on fiber disposal; the
//!   Rust [`apply`] returns a disposer that removes the three listeners AND
//!   clears the gate (HMR safety), matching the observable behavior of the
//!   TS fiber disposal.

use std::collections::HashMap;
use std::sync::Arc;

use cordis::{
    ArcValue, Context, Disposer, EventOptions, Listener, arc, downcast, make_disposer,
};
use dsh_fs::{
    FsEditGuard, FsError, FsErrorCode, FsObservation, FsTarget, FsWriteIntent,
};
use parking_lot::Mutex;

/// The opaque observed-state owner identity (the TS `WeakMap` object key).
pub type OwnerKey = usize;

/// The minimal execution-context view the policy needs to derive an
/// observed-state owner (TS `FsObservationActor`). The tool layer builds one
/// from its execution — `session_key` is the owner session's opaque pointer
/// identity, `None` for agent-less executions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsObservationActorHandle {
    /// The owning session's opaque identity; `None` derives no owner.
    pub session_key: Option<OwnerKey>,
}

/// Per-context observed-file state and the three `fs/*` decisions over it
/// (TS `ObservedStateGate`). One instance is created per [`apply`] so
/// disposal can drop all state for HMR.
#[derive(Default)]
pub struct ObservedStateGate {
    /// Observed-file state, keyed first by the owner key, then by
    /// [`FsTarget::target_key`]. An entry's presence is the
    /// prior-observation record; its discriminant keeps confirmed absence
    /// distinct from an unseen target.
    observed: Mutex<HashMap<OwnerKey, HashMap<String, FsObservation>>>,
}

impl ObservedStateGate {
    pub fn new() -> Self {
        Self::default()
    }

    fn owner(&self, actor: Option<&FsObservationActorHandle>) -> Option<OwnerKey> {
        actor.and_then(|actor| actor.session_key)
    }

    fn get(&self, owner: OwnerKey, target_key: &str) -> Option<FsObservation> {
        self.observed.lock().get(&owner)?.get(target_key).cloned()
    }

    fn set(&self, owner: OwnerKey, target_key: &str, observation: FsObservation) {
        let mut observed = self.observed.lock();
        observed
            .entry(owner)
            .or_default()
            .insert(target_key.to_string(), observation);
    }

    /// Drop all recorded state (HMR safety / disposal).
    pub fn clear(&self) {
        self.observed.lock().clear();
    }

    /// Decide the write intent: unseen or confirmed absent ⇒
    /// `CreateIfAbsent`; confirmed present ⇒ `ReplaceIfVersion` at the
    /// observed version.
    pub fn write_intent(&self, target: &FsTarget, actor: Option<&FsObservationActorHandle>) -> FsWriteIntent {
        let owner = self.owner(actor);
        let prior = owner.and_then(|owner| self.get(owner, target.target_key.as_str()));
        match prior {
            Some(FsObservation::Present { version }) => FsWriteIntent::ReplaceIfVersion { version },
            _ => FsWriteIntent::CreateIfAbsent,
        }
    }

    /// Decide the edit version guard: unseen rejects with `FS_NOT_OBSERVED`,
    /// confirmed absence rejects with `FS_NOT_FOUND`, and presence supplies
    /// the observed version as the CAS basis.
    pub fn edit_intent(
        &self,
        target: &FsTarget,
        actor: Option<&FsObservationActorHandle>,
    ) -> Result<FsEditGuard, FsError> {
        let owner = self.owner(actor);
        let prior = owner.and_then(|owner| self.get(owner, target.target_key.as_str()));
        if owner.is_none() || prior.is_none() {
            return Err(FsError::new(
                format!("edit requires reading \"{}\" first", target.display_path),
                FsErrorCode::FsNotObserved,
            ));
        }
        match prior.expect("checked") {
            FsObservation::Present { version } => Ok(FsEditGuard { version }),
            FsObservation::Absent => Err(FsError::new(
                format!("cannot edit \"{}\": not found", target.display_path),
                FsErrorCode::FsNotFound,
            )),
        }
    }

    /// Record an authoritative present or absent observation for this owner
    /// and target.
    pub fn observe(&self, target: &FsTarget, observation: FsObservation, actor: Option<&FsObservationActorHandle>) {
        if let Some(owner) = self.owner(actor) {
            self.set(owner, target.target_key.as_str(), observation);
        }
    }
}

/// Cordis plugin name used by loader diagnostics.
pub const NAME: &str = "fs-observation-policy";

/// The actor argument of the three `fs/*` events (the minimal handle).
fn downcast_actor(value: &ArcValue) -> Option<FsObservationActorHandle> {
    downcast::<FsObservationActorHandle>(value).copied()
}

/// Register the three `fs/*` listeners. No `inject` — this plugin reads no
/// services; it operates only on its own gate map. Returns a disposer that
/// removes the listeners and clears the recorded state (the TS fiber
/// disposal observable).
pub fn apply(ctx: &Context) -> Disposer {
    let gate = Arc::new(ObservedStateGate::new());

    // fs/write-intent: occupy the single decision slot — do NOT call next().
    let gate_for_write = gate.clone();
    let write_listener: Arc<Listener> = Arc::new(move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
        let target = downcast::<FsTarget>(&args[0]).cloned();
        let actor = downcast_actor(&args[1]);
        let gate = gate_for_write.clone();
        Box::pin(async move {
            let Some(target) = target else {
                return None;
            };
            // A throw rejects through the waterfall; the decision itself is
            // the resolved value.
            Some(arc(gate.write_intent(&target, actor.as_ref())))
        })
    });

    // fs/edit-intent: occupy the single decision slot — do NOT call next().
    // A rejection travels through the waterfall as a panic (the Rust
    // waterfall has no error channel; the TS rejection surfaces the same
    // FsError to the caller).
    let gate_for_edit = gate.clone();
    let edit_listener: Arc<Listener> = Arc::new(move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
        let target = downcast::<FsTarget>(&args[0]).cloned();
        let actor = downcast_actor(&args[1]);
        let gate = gate_for_edit.clone();
        Box::pin(async move {
            let Some(target) = target else {
                return None;
            };
            match gate.edit_intent(&target, actor.as_ref()) {
                Ok(guard) => Some(arc(guard)),
                Err(error) => std::panic::panic_any(error),
            }
        })
    });

    // fs/observed must stay synchronous and non-throwing: emit does not
    // await promises, and successful mutations have already committed.
    let gate_for_observe = gate.clone();
    let observe_listener: Arc<Listener> = Arc::new(move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
        let target = downcast::<FsTarget>(&args[0]).cloned();
        let observation = downcast::<FsObservation>(&args[1]).cloned();
        let actor = downcast_actor(&args[2]);
        let gate = gate_for_observe.clone();
        Box::pin(async move {
            if let (Some(target), Some(observation)) = (target, observation) {
                gate.observe(&target, observation, actor.as_ref());
            }
            None
        })
    });

    let write_disposer = futures::executor::block_on(ctx.on(
        "fs/write-intent",
        write_listener,
        EventOptions::default(),
    ));
    let edit_disposer = futures::executor::block_on(ctx.on(
        "fs/edit-intent",
        edit_listener,
        EventOptions::default(),
    ));
    let observe_disposer = futures::executor::block_on(ctx.on(
        "fs/observed",
        observe_listener,
        EventOptions::default(),
    ));

    make_disposer(move || {
        let gate = gate.clone();
        let write_disposer = write_disposer.clone();
        let edit_disposer = edit_disposer.clone();
        let observe_disposer = observe_disposer.clone();
        Box::pin(async move {
            // Drop all recorded state so a reloaded plugin starts clean (HMR
            // safety).
            gate.clear();
            (write_disposer)().await;
            (edit_disposer)().await;
            (observe_disposer)().await;
        })
    })
}
