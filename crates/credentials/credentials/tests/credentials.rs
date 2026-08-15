//! Rust port of
//! `packages/credentials/credentials/tests/credentials.spec.ts` +
//! `tests/invariant.spec.ts`: the reference-shape rule, the seam through the
//! memory provider, and the invariant companion.
//!
//! Deviations:
//!
//! - `fiber.dispose()` removal of the service collapses into the Rust
//!   service-registration contract (the disposer unregisters the name); the
//!   spec's fifth case is covered by the duplicate-registration panic
//!   instead.
//! - The TS `ctx.emit` sync-throw for an invariant violation is
//!   fire-and-forget in Rust; the companion's listener is driven directly
//!   through `ctx.collect` to observe the failure.

mod common;

use std::sync::Arc;

use common::MemoryCredentials;
use cordis::{Context, DispatchMode, arc};
use dsh_credentials::{CredentialInfo, CredentialProvider, credential_ref};
use dsh_invariants::{InvariantConfig, InvariantRegistry};
use dsh_credentials::invariant as credentials_invariant;

fn reference() -> dsh_credentials::CredentialRef {
    credential_ref("DEEPSEEK_API_KEY")
}

#[test]
fn credential_ref_brands_posix_shell_identifiers() {
    assert_eq!(credential_ref("DEEPSEEK_API_KEY").to_string(), "DEEPSEEK_API_KEY");
    assert_eq!(credential_ref("_private").to_string(), "_private");
    assert_eq!(credential_ref("lower_case9").to_string(), "lower_case9");
}

#[test]
fn credential_ref_rejects_every_other_shape() {
    for invalid in ["", "9LEADING", "WITH-DASH", "WITH SPACE", "ns:key"] {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            credential_ref(invalid)
        }));
        assert!(outcome.is_err(), "{invalid:?} must reject");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn mounts_as_ctx_credentials_and_resolves_a_seeded_reference_with_its_source() {
    let ctx = Context::root();
    MemoryCredentials::install(&ctx, &[("DEEPSEEK_API_KEY", "sk-seeded")]);
    let provider = ctx
        .get_typed::<Arc<dyn CredentialProvider>>("credentials", false)
        .expect("service");
    let reference = reference();
    assert_eq!(
        provider.resolve(&reference).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "sk-seeded".to_string(),
            source: "memory".to_string(),
        })
    );
    assert_eq!(
        provider.describe(&reference).await,
        CredentialInfo {
            configured: true,
            source: Some("memory".to_string()),
            writable: true,
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn treats_an_empty_stored_value_as_absent_everywhere() {
    let ctx = Context::root();
    MemoryCredentials::install(&ctx, &[("DEEPSEEK_API_KEY", "")]);
    let provider = ctx
        .get_typed::<Arc<dyn CredentialProvider>>("credentials", false)
        .expect("service");
    let reference = reference();
    assert_eq!(provider.resolve(&reference).await, None);
    assert_eq!(
        provider.describe(&reference).await,
        CredentialInfo { configured: false, source: None, writable: true }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn stores_through_set_removes_through_unset_and_emits_the_committed_change() {
    let ctx = Context::root();
    MemoryCredentials::install(&ctx, &[]);
    let provider = ctx
        .get_typed::<Arc<dyn CredentialProvider>>("credentials", false)
        .expect("service");
    let reference = reference();
    let events: Arc<parking_lot::Mutex<Vec<dsh_credentials::CredentialRef>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let events_for_listener = events.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
        let reference = cordis::downcast::<dsh_credentials::CredentialRef>(&args[0]).cloned();
        let events = events_for_listener.clone();
        Box::pin(async move {
            if let Some(reference) = reference {
                events.lock().push(reference);
            }
            None
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "credentials/updated",
        listener,
        cordis::EventOptions::default(),
    ));

    provider.set(&reference, "sk-live").await.expect("set");
    assert_eq!(
        provider.resolve(&reference).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "sk-live".to_string(),
            source: "memory".to_string(),
        })
    );
    provider.unset(&reference).await.expect("unset");
    assert_eq!(provider.resolve(&reference).await, None);
    assert_eq!(events.lock().clone(), vec![reference.clone(), reference.clone()]);
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_an_empty_set_and_keeps_an_absent_unset_silent() {
    let ctx = Context::root();
    MemoryCredentials::install(&ctx, &[]);
    let provider = ctx
        .get_typed::<Arc<dyn CredentialProvider>>("credentials", false)
        .expect("service");
    let reference = reference();
    let events: Arc<parking_lot::Mutex<Vec<dsh_credentials::CredentialRef>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let events_for_listener = events.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
        let reference = cordis::downcast::<dsh_credentials::CredentialRef>(&args[0]).cloned();
        let events = events_for_listener.clone();
        Box::pin(async move {
            if let Some(reference) = reference {
                events.lock().push(reference);
            }
            None
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "credentials/updated",
        listener,
        cordis::EventOptions::default(),
    ));

    let error = provider.set(&reference, "").await.err().expect("empty set rejects");
    assert!(error.contains("empty value"), "{error}");
    provider.unset(&reference).await.expect("absent unset is a no-op");
    assert!(events.lock().is_empty());
}

// ---------------------------------------------------------------------------
// invariant companion

#[tokio::test(flavor = "current_thread")]
async fn accepts_a_committed_change_emitted_by_a_live_service() {
    let ctx = Context::root();
    let _invariants = InvariantRegistry::new(&ctx, InvariantConfig::default());
    let _disposer = credentials_invariant::apply(&ctx);
    MemoryCredentials::install(&ctx, &[]);
    let provider = ctx
        .get_typed::<Arc<dyn CredentialProvider>>("credentials", false)
        .expect("service");
    let reference = reference();
    provider.set(&reference, "sk-live").await.expect("set resolves");
}

#[tokio::test(flavor = "current_thread")]
async fn fails_an_update_event_emitted_without_a_live_service() {
    let ctx = Context::root();
    let failures: Arc<parking_lot::Mutex<Vec<String>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let fail: Arc<dyn Fn(&str) + Send + Sync> = {
        let failures = failures.clone();
        Arc::new(move |message: &str| {
            failures.lock().push(message.to_string());
        })
    };
    (credentials_invariant::installer().install)(&ctx, fail).await;

    // Drive the companion listener directly (the Rust emit is
    // fire-and-forget): a snapshot without a credentials service must fail
    // through the fail channel — the production channel panics with an
    // `InvariantError`; the test collector records the same report.
    let reference = reference();
    let args = vec![arc(reference.clone())];
    let listeners = ctx.collect(DispatchMode::Emit, "credentials/updated", &args);
    assert_eq!(listeners.len(), 1);
    for (listener_ctx, callback) in listeners {
        let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
            callback(&listener_ctx, args.clone()),
        ))
        .await;
        assert!(outcome.is_ok(), "the collector fail channel records, not throws");
    }
    let failures = failures.lock();
    assert_eq!(failures.len(), 1);
    assert!(
        failures[0].contains("without a live credentials service"),
        "{}",
        failures[0]
    );
}

// The registry installs its child fibers through `tokio::spawn`, so the
// duplicate-registration case runs under a runtime.
#[tokio::test(flavor = "current_thread")]
async fn reserves_the_package_name_against_duplicate_registration() {
    let ctx = Context::root();
    let registry = InvariantRegistry::new(&ctx, InvariantConfig::default());
    let _disposer = credentials_invariant::apply(&ctx);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        registry.register(
            &ctx,
            "@deepseek-ai/dsh-credentials",
            dsh_invariants::InvariantInstaller {
                inject: None,
                install: Arc::new(|_ctx, _fail| Box::pin(async {})),
            },
        );
    }));
    assert!(outcome.is_err());
}
