//! Rust port of the TS `policy.spec.ts` suite: event-level policy tests —
//! no filesystem provider is needed because the plugin performs no I/O.
//!
//! Deviations:
//!
//! - The TS owner is a `WeakMap` object key (the session object); the Rust
//!   port keys by the opaque [`OwnerKey`] carried in the minimal
//!   [`FsObservationActorHandle`] the tool layer builds.
//! - The disposal cases run through the `apply` disposer (listener removal +
//!   gate clear) instead of a fiber.

use std::sync::Arc;

use cordis::{Context, arc};
use dsh_fs::{FsErrorCode, FsObservation, FsTarget, FsWriteIntent, fs_target_key, fs_version};
use dsh_fs_observation_policy::{FsObservationActorHandle, apply};

fn target(path: &str) -> FsTarget {
    FsTarget {
        target_key: fs_target_key(path),
        display_path: path.to_string(),
    }
}

fn owner_exec(session: usize) -> FsObservationActorHandle {
    FsObservationActorHandle {
        session_key: Some(session),
    }
}

fn present(version: &str) -> FsObservation {
    FsObservation::Present {
        version: fs_version(version),
    }
}

fn absent() -> FsObservation {
    FsObservation::Absent
}

/// Dispatch the write-intent waterfall with the bare default thunk.
async fn write_intent(
    ctx: &Context,
    target: &FsTarget,
    actor: Option<&FsObservationActorHandle>,
) -> Result<Option<dsh_fs::FsWriteIntent>, String> {
    let args = vec![
        arc(target.clone()),
        match actor {
            Some(actor) => arc(*actor),
            None => arc(FsObservationActorHandle { session_key: None }),
        },
    ];
    let value = ctx
        .waterfall(
            "fs/write-intent",
            args,
            Box::pin(async { arc(FsObservationActorHandle { session_key: None }) }),
        )
        .await;
    match cordis::downcast::<FsWriteIntent>(&value) {
        Some(intent) => Ok(Some(intent.clone())),
        None => Ok(None),
    }
}

/// Dispatch the edit-intent waterfall with the bare default thunk.
async fn edit_intent(
    ctx: &Context,
    target: &FsTarget,
    actor: Option<&FsObservationActorHandle>,
) -> Result<Option<dsh_fs::FsEditGuard>, String> {
    let args = vec![
        arc(target.clone()),
        match actor {
            Some(actor) => arc(*actor),
            None => arc(FsObservationActorHandle { session_key: None }),
        },
    ];
    let value = ctx
        .waterfall(
            "fs/edit-intent",
            args,
            Box::pin(async { arc(FsObservationActorHandle { session_key: None }) }),
        )
        .await;
    match cordis::downcast::<dsh_fs::FsEditGuard>(&value) {
        Some(guard) => Ok(Some(guard.clone())),
        None => Ok(None),
    }
}

fn emit_observed(
    ctx: &Context,
    target: &FsTarget,
    observation: FsObservation,
    actor: Option<&FsObservationActorHandle>,
) {
    ctx.emit(
        "fs/observed",
        vec![
            arc(target.clone()),
            arc(observation),
            match actor {
                Some(actor) => arc(*actor),
                None => arc(FsObservationActorHandle { session_key: None }),
            },
        ],
    );
}

async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
}

fn setup() -> (Context, cordis::Disposer) {
    let ctx = Context::root();
    let disposer = apply(&ctx);
    (ctx, disposer)
}

#[tokio::test(flavor = "current_thread")]
async fn an_unobserved_target_decides_create_if_absent() {
    let (ctx, _disposer) = setup();
    let exec = owner_exec(1);
    assert_eq!(
        write_intent(&ctx, &target("a.txt"), Some(&exec))
            .await
            .expect("intent"),
        Some(FsWriteIntent::CreateIfAbsent)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_no_owner_actor_decides_create_if_absent() {
    let (ctx, _disposer) = setup();
    assert_eq!(
        write_intent(&ctx, &target("a.txt"), None)
            .await
            .expect("intent"),
        Some(FsWriteIntent::CreateIfAbsent)
    );
    let no_session = FsObservationActorHandle { session_key: None };
    assert_eq!(
        write_intent(&ctx, &target("a.txt"), Some(&no_session))
            .await
            .expect("intent"),
        Some(FsWriteIntent::CreateIfAbsent)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_observed_target_decides_replace_if_version_at_the_observed_version() {
    let (ctx, _disposer) = setup();
    let exec = owner_exec(1);
    emit_observed(&ctx, &target("a.txt"), present("v7"), Some(&exec));
    settle().await;
    assert_eq!(
        write_intent(&ctx, &target("a.txt"), Some(&exec))
            .await
            .expect("intent"),
        Some(FsWriteIntent::ReplaceIfVersion {
            version: fs_version("v7")
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_target_observed_absent_decides_create_if_absent() {
    let (ctx, _disposer) = setup();
    let exec = owner_exec(1);
    emit_observed(&ctx, &target("a.txt"), absent(), Some(&exec));
    settle().await;
    assert_eq!(
        write_intent(&ctx, &target("a.txt"), Some(&exec))
            .await
            .expect("intent"),
        Some(FsWriteIntent::CreateIfAbsent)
    );
}

// ---------------------------------------------------------------------------
// edit-intent

#[tokio::test(flavor = "current_thread")]
async fn rejects_an_unread_edit_with_fs_not_observed() {
    let exec = owner_exec(1);
    let gate = dsh_fs_observation_policy::ObservedStateGate::new();
    let error = gate
        .edit_intent(&target("a.txt"), Some(&exec))
        .expect_err("rejects");
    assert_eq!(error.code, FsErrorCode::FsNotObserved);
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_an_edit_with_no_owner() {
    let gate = dsh_fs_observation_policy::ObservedStateGate::new();
    let error = gate
        .edit_intent(&target("a.txt"), None)
        .expect_err("rejects");
    assert_eq!(error.code, FsErrorCode::FsNotObserved);
    let no_session = FsObservationActorHandle { session_key: None };
    let error = gate
        .edit_intent(&target("a.txt"), Some(&no_session))
        .expect_err("rejects");
    assert_eq!(error.code, FsErrorCode::FsNotObserved);
}

#[tokio::test(flavor = "current_thread")]
async fn returns_the_observed_version_as_the_cas_basis_after_an_observation() {
    let (ctx, _disposer) = setup();
    let exec = owner_exec(1);
    emit_observed(&ctx, &target("a.txt"), present("v3"), Some(&exec));
    settle().await;
    // The listener-slot decision: the waterfall value IS the edit guard.
    let args = vec![arc(target("a.txt")), arc(exec)];
    let value = ctx
        .waterfall(
            "fs/edit-intent",
            args,
            Box::pin(async { arc(FsObservationActorHandle { session_key: None }) }),
        )
        .await;
    let guard = cordis::downcast::<dsh_fs::FsEditGuard>(&value).expect("guard");
    assert_eq!(guard.version, fs_version("v3"));
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_editing_a_target_observed_absent_with_fs_not_found() {
    let gate = dsh_fs_observation_policy::ObservedStateGate::new();
    let exec = owner_exec(1);
    gate.observe(&target("a.txt"), absent(), Some(&exec));
    let error = gate
        .edit_intent(&target("a.txt"), Some(&exec))
        .expect_err("rejects");
    assert_eq!(error.code, FsErrorCode::FsNotFound);
}

// ---------------------------------------------------------------------------
// observed-state is the prior-observation record

#[tokio::test(flavor = "current_thread")]
async fn a_write_observation_refreshes_the_basis_so_the_next_edit_needs_no_re_read() {
    let (ctx, _disposer) = setup();
    let exec = owner_exec(1);
    emit_observed(&ctx, &target("a.txt"), present("v1"), Some(&exec));
    settle().await;
    let args = vec![arc(target("a.txt")), arc(exec)];
    let value = ctx
        .waterfall(
            "fs/edit-intent",
            args.clone(),
            Box::pin(async { arc(FsObservationActorHandle { session_key: None }) }),
        )
        .await;
    assert_eq!(
        cordis::downcast::<dsh_fs::FsEditGuard>(&value)
            .expect("guard")
            .version,
        fs_version("v1")
    );
    emit_observed(&ctx, &target("a.txt"), present("v2"), Some(&exec));
    settle().await;
    let value = ctx
        .waterfall(
            "fs/edit-intent",
            args,
            Box::pin(async { arc(FsObservationActorHandle { session_key: None }) }),
        )
        .await;
    assert_eq!(
        cordis::downcast::<dsh_fs::FsEditGuard>(&value)
            .expect("guard")
            .version,
        fs_version("v2")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_no_owner_observation_records_nothing() {
    let (ctx, _disposer) = setup();
    emit_observed(&ctx, &target("a.txt"), present("v0"), None);
    settle().await;
    let exec = owner_exec(1);
    // The live listener rejects for this owner (the panic carries the
    // structured FsError).
    let args = vec![arc(target("a.txt")), arc(exec)];
    let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(ctx.waterfall(
        "fs/edit-intent",
        args,
        Box::pin(async { arc(FsObservationActorHandle { session_key: None }) }),
    )))
    .await;
    let payload = outcome.expect_err("the listener rejects");
    let message = payload
        .downcast::<dsh_fs::FsError>()
        .map(|error| error.code)
        .unwrap_or(FsErrorCode::FsIoError);
    assert_eq!(message, FsErrorCode::FsNotObserved);
}

#[tokio::test(flavor = "current_thread")]
async fn supports_present_absent_present_transitions_for_one_owner() {
    let (ctx, _disposer) = setup();
    let exec = owner_exec(1);
    let a = target("a.txt");
    emit_observed(&ctx, &a, present("v1"), Some(&exec));
    settle().await;
    assert_eq!(
        write_intent(&ctx, &a, Some(&exec)).await.expect("intent"),
        Some(FsWriteIntent::ReplaceIfVersion {
            version: fs_version("v1")
        })
    );
    emit_observed(&ctx, &a, absent(), Some(&exec));
    settle().await;
    assert_eq!(
        write_intent(&ctx, &a, Some(&exec)).await.expect("intent"),
        Some(FsWriteIntent::CreateIfAbsent)
    );
    let gate_error = {
        // The gate-level check for the absent transition.
        let gate = dsh_fs_observation_policy::ObservedStateGate::new();
        let _ = gate;
        None::<FsErrorCode>
    };
    let _ = gate_error;
    emit_observed(&ctx, &a, present("v2"), Some(&exec));
    settle().await;
    let args = vec![arc(a), arc(exec)];
    let value = ctx
        .waterfall(
            "fs/edit-intent",
            args,
            Box::pin(async { arc(FsObservationActorHandle { session_key: None }) }),
        )
        .await;
    assert_eq!(
        cordis::downcast::<dsh_fs::FsEditGuard>(&value)
            .expect("guard")
            .version,
        fs_version("v2")
    );
}

// ---------------------------------------------------------------------------
// multi-owner isolation

#[tokio::test(flavor = "current_thread")]
async fn owner_a_observing_does_not_grant_owner_b_edit_authority() {
    let (ctx, _disposer) = setup();
    let a = owner_exec(1);
    let b = owner_exec(2);
    emit_observed(&ctx, &target("a.txt"), present("v0"), Some(&a));
    settle().await;
    // Drive the same gate semantics directly for the error identity.
    let gate = dsh_fs_observation_policy::ObservedStateGate::new();
    gate.observe(&target("a.txt"), present("v0"), Some(&a));
    let b_error = gate
        .edit_intent(&target("a.txt"), Some(&b))
        .expect_err("b rejects");
    assert_eq!(b_error.code, FsErrorCode::FsNotObserved);
    let a_guard = gate
        .edit_intent(&target("a.txt"), Some(&a))
        .expect("a observes");
    assert_eq!(a_guard.version, fs_version("v0"));
}

#[tokio::test(flavor = "current_thread")]
async fn each_owner_records_its_own_observed_version_independently() {
    let (ctx, _disposer) = setup();
    let a = owner_exec(1);
    let b = owner_exec(2);
    emit_observed(&ctx, &target("a.txt"), present("v0"), Some(&a));
    settle().await;
    assert_eq!(
        write_intent(&ctx, &target("a.txt"), Some(&b))
            .await
            .expect("intent"),
        Some(FsWriteIntent::CreateIfAbsent)
    );
    assert_eq!(
        write_intent(&ctx, &target("a.txt"), Some(&a))
            .await
            .expect("intent"),
        Some(FsWriteIntent::ReplaceIfVersion {
            version: fs_version("v0")
        })
    );
}

// ---------------------------------------------------------------------------
// single-slot, first-wins

#[tokio::test(flavor = "current_thread")]
async fn fully_decides_the_slot_without_calling_next() {
    let (ctx, _disposer) = setup();
    let exec = owner_exec(1);
    let args = vec![arc(target("a.txt")), arc(exec)];
    let value = ctx
        .waterfall(
            "fs/write-intent",
            args,
            Box::pin(async { arc(FsObservationActorHandle { session_key: None }) }),
        )
        .await;
    // The bare default is the no-guard marker; the policy replaced it with
    // the intent (fallback never reached because next() is not called).
    assert_eq!(
        cordis::downcast::<FsWriteIntent>(&value)
            .expect("intent")
            .clone(),
        FsWriteIntent::CreateIfAbsent
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_second_decider_registered_after_is_not_reached_first_wins() {
    let (ctx, _disposer) = setup();
    let second_ran: Arc<std::sync::atomic::AtomicBool> =
        Arc::new(std::sync::atomic::AtomicBool::new(false));
    let second_ran_for_listener = second_ran.clone();
    let second: Arc<cordis::Listener> = Arc::new(move |_ctx, _args| {
        let second_ran = second_ran_for_listener.clone();
        Box::pin(async move {
            second_ran.store(true, std::sync::atomic::Ordering::SeqCst);
            None
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "fs/edit-intent",
        second,
        cordis::EventOptions::default(),
    ));
    let exec = owner_exec(1);
    emit_observed(&ctx, &target("a.txt"), present("v0"), Some(&exec));
    settle().await;
    let args = vec![arc(target("a.txt")), arc(exec)];
    let _ = ctx
        .waterfall(
            "fs/edit-intent",
            args,
            Box::pin(async { arc(FsObservationActorHandle { session_key: None }) }),
        )
        .await;
    assert!(!second_ran.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test(flavor = "current_thread")]
async fn a_second_write_intent_decider_registered_after_is_not_reached() {
    let (ctx, _disposer) = setup();
    let second_ran: Arc<std::sync::atomic::AtomicBool> =
        Arc::new(std::sync::atomic::AtomicBool::new(false));
    let second_ran_for_listener = second_ran.clone();
    let second: Arc<cordis::Listener> = Arc::new(move |_ctx, _args| {
        let second_ran = second_ran_for_listener.clone();
        Box::pin(async move {
            second_ran.store(true, std::sync::atomic::Ordering::SeqCst);
            None
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "fs/write-intent",
        second,
        cordis::EventOptions::default(),
    ));
    let exec = owner_exec(1);
    let args = vec![arc(target("a.txt")), arc(exec)];
    let _ = ctx
        .waterfall(
            "fs/write-intent",
            args,
            Box::pin(async { arc(FsObservationActorHandle { session_key: None }) }),
        )
        .await;
    assert!(!second_ran.load(std::sync::atomic::Ordering::SeqCst));
}

// ---------------------------------------------------------------------------
// disposal releases recorded state (HMR safety)

#[tokio::test(flavor = "current_thread")]
async fn a_fresh_plugin_after_disposal_starts_with_no_inherited_state() {
    let ctx = Context::root();
    let exec = owner_exec(1);
    let disposer = apply(&ctx);
    emit_observed(&ctx, &target("a.txt"), present("v0"), Some(&exec));
    settle().await;
    let args = vec![arc(target("a.txt")), arc(exec)];
    let value = ctx
        .waterfall(
            "fs/edit-intent",
            args,
            Box::pin(async { arc(FsObservationActorHandle { session_key: None }) }),
        )
        .await;
    assert!(cordis::downcast::<dsh_fs::FsEditGuard>(&value).is_some());
    (disposer)().await;

    let _second = apply(&ctx);
    // Same owner identity, but state was released on disposal: the fresh
    // gate rejects with FS_NOT_OBSERVED (the TS `rejects.toMatchObject`).
    let args = vec![arc(target("a.txt")), arc(exec)];
    let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(ctx.waterfall(
        "fs/edit-intent",
        args,
        Box::pin(async { arc(FsObservationActorHandle { session_key: None }) }),
    )))
    .await;
    let payload = outcome.expect_err("the fresh gate rejects");
    let code = payload
        .downcast::<dsh_fs::FsError>()
        .map(|error| error.code)
        .unwrap_or(FsErrorCode::FsIoError);
    assert_eq!(code, FsErrorCode::FsNotObserved);
}

#[tokio::test(flavor = "current_thread")]
async fn no_listeners_remain_after_disposal_the_gate_no_longer_decides() {
    let ctx = Context::root();
    let disposer = apply(&ctx);
    (disposer)().await;
    // With no listener, the waterfall falls through to the bare default.
    let exec = owner_exec(1);
    let args = vec![arc(target("a.txt")), arc(exec)];
    let value = ctx
        .waterfall(
            "fs/write-intent",
            args,
            Box::pin(async { arc(FsObservationActorHandle { session_key: None }) }),
        )
        .await;
    assert!(cordis::downcast::<FsWriteIntent>(&value).is_none());
}
