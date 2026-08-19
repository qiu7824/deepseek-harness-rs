//! Rust port of the TS `watcher.spec.ts` + `drain.spec.ts` suites, plus the
//! real end-to-end hot reload from `local.spec.ts`. The watcher pipeline is
//! driven through the crate's fake-watcher seam (the TS `vi.mock('chokidar')`
//! equivalent); the real `notify` backend is exercised by one end-to-end
//! test.
//!
//! Deviations:
//!
//! - `chmod 0o000` unreadability is POSIX-only; the equivalent case drives a
//!   reader-seam failure instead on every platform.
//! - The read-failure injection goes through the reader seam (the TS
//!   `vi.mock('node:fs/promises')`).

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::{
    GatedWriter, TempRoot, boot_with, default_reader, emit_to, fake_instances,
    fake_watcher_factory, real_writer, wait_for, write_credentials,
};
use cordis::Context;
use dsh_credentials::{CredentialProvider, credential_ref};
use dsh_credentials_local::{Config, LocalCredentialProvider, WatchSignal};
use dsh_invariants::InvariantError;

fn key() -> dsh_credentials::CredentialRef {
    credential_ref("DSH_CRED_PIPE")
}

// ---------------------------------------------------------------------------
// watcher pipeline (fake watcher)

#[tokio::test(flavor = "current_thread")]
async fn records_the_configured_write_settle_window_for_a_zero_debounce() {
    let temp = TempRoot::new();
    let ctx = Context::root();
    let _provider = boot_with(
        &ctx,
        &temp.path(".credentials.yaml"),
        true,
        real_writer(),
        Some(fake_watcher_factory()),
        Some(default_reader()),
    )
    .expect("boot");
    // The fake records the debounce the provider resolved (0 as configured).
    let instances = fake_instances().lock();
    let instance = instances.first().expect("fake instance");
    assert_eq!(instance.debounce, 100);
}

#[tokio::test(flavor = "current_thread")]
async fn survives_a_watcher_error_and_keeps_publishing_later_edits() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let ctx = Context::root();
    let provider = boot_with(
        &ctx,
        &path,
        true,
        real_writer(),
        Some(fake_watcher_factory()),
        Some(default_reader()),
    )
    .expect("boot");
    let instances = fake_instances().lock();
    let instance = instances.first().expect("fake instance");
    instance.sink.on_error("watch backend failure".to_string());
    drop(instances);
    assert_eq!(provider.resolve(&key()).await, None);

    write_credentials(&path, "DSH_CRED_PIPE: arrived\n");
    emit_to(&path, WatchSignal::Changed);
    wait_for(
        "arrived",
        || {
            futures::executor::block_on(provider.resolve(&key()))
                == Some(dsh_credentials::ResolvedCredential {
                    value: "arrived".to_string(),
                    source: "file".to_string(),
                })
        },
        3000,
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn keeps_the_last_good_snapshot_when_the_read_fails_after_its_permission_check() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    write_credentials(&path, "DSH_CRED_PIPE: good\n");
    let ctx = Context::root();
    let fail = Arc::new(AtomicBool::new(false));
    let reader: dsh_credentials_local::DocumentReader = {
        let fail = fail.clone();
        Arc::new(move |file: &std::path::Path| {
            let file = file.to_path_buf();
            let fail = fail.clone();
            Box::pin(async move {
                if fail.load(Ordering::SeqCst) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "EACCES: injected read failure",
                    ));
                }
                tokio::fs::read_to_string(file).await
            })
        })
    };
    let provider = boot_with(
        &ctx,
        &path,
        true,
        real_writer(),
        Some(fake_watcher_factory()),
        Some(reader),
    )
    .expect("boot");
    fail.store(true, Ordering::SeqCst);
    emit_to(&path, WatchSignal::Changed);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    // The warn-and-keep path holds the last good snapshot.
    assert_eq!(
        provider.resolve(&key()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "good".to_string(),
            source: "file".to_string()
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn keeps_the_reload_queue_alive_after_an_invariant_violation_escapes_the_fan_out() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let ctx = Context::root();
    let provider = boot_with(
        &ctx,
        &path,
        true,
        real_writer(),
        Some(fake_watcher_factory()),
        Some(default_reader()),
    )
    .expect("boot");
    let arm = Arc::new(AtomicBool::new(true));
    let arm_for_listener = arm.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, _args| {
        let arm = arm_for_listener.clone();
        Box::pin(async move {
            if arm.load(Ordering::SeqCst) {
                std::panic::panic_any(InvariantError::new(
                    "@deepseek-ai/dsh-credentials",
                    "forged relation",
                ));
            }
            None
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "credentials/updated",
        listener,
        cordis::EventOptions::default(),
    ));

    write_credentials(&path, "DSH_CRED_PIPE: first\n");
    emit_to(&path, WatchSignal::Changed);
    wait_for(
        "first lands",
        || {
            futures::executor::block_on(provider.resolve(&key()))
                == Some(dsh_credentials::ResolvedCredential {
                    value: "first".to_string(),
                    source: "file".to_string(),
                })
        },
        3000,
    )
    .await;

    arm.store(false, Ordering::SeqCst);
    write_credentials(&path, "DSH_CRED_PIPE: second\n");
    emit_to(&path, WatchSignal::Changed);
    wait_for(
        "second lands",
        || {
            futures::executor::block_on(provider.resolve(&key()))
                == Some(dsh_credentials::ResolvedCredential {
                    value: "second".to_string(),
                    source: "file".to_string(),
                })
        },
        3000,
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn quiesces_the_refresh_pipeline_before_dispose_completes() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    write_credentials(&path, "DSH_CRED_PIPE: initial\n");
    let ctx = Context::root();
    let provider = boot_with(
        &ctx,
        &path,
        true,
        real_writer(),
        Some(fake_watcher_factory()),
        Some(default_reader()),
    )
    .expect("boot");
    let disposed = Arc::new(AtomicBool::new(false));
    let post_dispose_commits = Arc::new(AtomicBool::new(false));
    let listener: Arc<cordis::Listener> = {
        let disposed = disposed.clone();
        let post_dispose_commits = post_dispose_commits.clone();
        Arc::new(move |_ctx, _args| {
            let disposed = disposed.clone();
            let post_dispose_commits = post_dispose_commits.clone();
            Box::pin(async move {
                if disposed.load(Ordering::SeqCst) {
                    post_dispose_commits.store(true, Ordering::SeqCst);
                }
                None
            })
        })
    };
    let _ = futures::executor::block_on(ctx.on(
        "credentials/updated",
        listener,
        cordis::EventOptions::default(),
    ));

    write_credentials(&path, "DSH_CRED_PIPE: changed\n");
    emit_to(&path, WatchSignal::Changed);
    emit_to(&path, WatchSignal::Changed);
    provider.drain().await;
    disposed.store(true, Ordering::SeqCst);
    emit_to(&path, WatchSignal::Changed);
    emit_to(&path, WatchSignal::Ready);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(!post_dispose_commits.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "current_thread")]
async fn empties_the_snapshot_when_the_document_is_deleted_and_emits_the_removals() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    write_credentials(&path, "DSH_CRED_PIPE: doomed\n");
    let ctx = Context::root();
    let provider = boot_with(
        &ctx,
        &path,
        true,
        real_writer(),
        Some(fake_watcher_factory()),
        Some(default_reader()),
    )
    .expect("boot");
    let seen: Arc<parking_lot::Mutex<Vec<dsh_credentials::CredentialRef>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let seen_for_listener = seen.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
        let reference = cordis::downcast::<dsh_credentials::CredentialRef>(&args[0]).cloned();
        let seen = seen_for_listener.clone();
        Box::pin(async move {
            if let Some(reference) = reference {
                seen.lock().push(reference);
            }
            None
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "credentials/updated",
        listener,
        cordis::EventOptions::default(),
    ));

    std::fs::remove_file(&path).expect("rm");
    emit_to(&path, WatchSignal::Changed);
    wait_for(
        "emptied",
        || futures::executor::block_on(provider.resolve(&key())).is_none(),
        3000,
    )
    .await;
    assert_eq!(seen.lock().clone(), vec![key()]);
}

#[tokio::test(flavor = "current_thread")]
async fn keeps_the_last_good_snapshot_when_an_external_edit_makes_the_document_invalid() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    write_credentials(&path, "DSH_CRED_PIPE: a\n");
    let ctx = Context::root();
    let provider = boot_with(
        &ctx,
        &path,
        true,
        real_writer(),
        Some(fake_watcher_factory()),
        Some(default_reader()),
    )
    .expect("boot");
    let seen: Arc<parking_lot::Mutex<Vec<dsh_credentials::CredentialRef>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let seen_for_listener = seen.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
        let reference = cordis::downcast::<dsh_credentials::CredentialRef>(&args[0]).cloned();
        let seen = seen_for_listener.clone();
        Box::pin(async move {
            if let Some(reference) = reference {
                seen.lock().push(reference);
            }
            None
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "credentials/updated",
        listener,
        cordis::EventOptions::default(),
    ));

    write_credentials(&path, "BAD-KEY: 2\nDSH_CRED_PIPE: b\n");
    emit_to(&path, WatchSignal::Changed);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        provider.resolve(&key()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "a".to_string(),
            source: "file".to_string()
        })
    );
    assert!(seen.lock().is_empty());

    // Repairing the document resumes publishing.
    write_credentials(&path, "DSH_CRED_PIPE: b\n");
    emit_to(&path, WatchSignal::Changed);
    wait_for(
        "repaired",
        || {
            futures::executor::block_on(provider.resolve(&key()))
                == Some(dsh_credentials::ResolvedCredential {
                    value: "b".to_string(),
                    source: "file".to_string(),
                })
        },
        3000,
    )
    .await;
    assert_eq!(seen.lock().clone(), vec![key()]);
}

#[tokio::test(flavor = "current_thread")]
async fn treats_an_event_for_a_still_absent_file_as_a_no_op() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let ctx = Context::root();
    let provider = boot_with(
        &ctx,
        &path,
        true,
        real_writer(),
        Some(fake_watcher_factory()),
        Some(default_reader()),
    )
    .expect("boot");
    emit_to(&path, WatchSignal::Changed);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(provider.resolve(&key()).await, None);
}

#[tokio::test(flavor = "current_thread")]
async fn reconciles_at_watcher_ready_so_a_change_during_setup_is_not_missed() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    write_credentials(&path, "DSH_CRED_PIPE: a\n");
    let ctx = Context::root();
    let provider = boot_with(
        &ctx,
        &path,
        true,
        real_writer(),
        Some(fake_watcher_factory()),
        Some(default_reader()),
    )
    .expect("boot");
    // Written after the initial load but before the watcher became active:
    // no change event will ever fire for it.
    write_credentials(&path, "DSH_CRED_PIPE: written-before-ready\n");
    emit_to(&path, WatchSignal::Ready);
    wait_for(
        "ready reconcile",
        || {
            futures::executor::block_on(provider.resolve(&key()))
                == Some(dsh_credentials::ResolvedCredential {
                    value: "written-before-ready".to_string(),
                    source: "file".to_string(),
                })
        },
        3000,
    )
    .await;
}

// ---------------------------------------------------------------------------
// write-drain teardown (drain.spec)

#[tokio::test(flavor = "current_thread")]
async fn lets_the_in_flight_write_land_and_fails_the_queued_one_after_disposal() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let ctx = Context::root();
    let gated = GatedWriter::new();
    let provider = boot_with(
        &ctx,
        &path,
        false,
        gated.writer(),
        None,
        Some(default_reader()),
    )
    .expect("boot");

    let release = gated.arm();
    let k = key();
    // Spawned: the TS async function body starts immediately; a bare Rust
    // future stays inert until polled.
    let provider_for_first = provider.clone();
    let first = tokio::spawn(async move { provider_for_first.set(&k, "one").await });
    // Let the first task pass its liveness checks and park on the gate, so
    // it is genuinely in-flight when disposal begins.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let other = credential_ref("DSH_CRED_DRAIN_B");
    let provider_for_second = provider.clone();
    let second = tokio::spawn(async move { provider_for_second.set(&other, "two").await });
    let disposal = provider.drain();
    // Give the drain its first turn (set closed) before opening the gate.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let _ = release.send(());
    disposal.await;

    first
        .await
        .expect("in-flight write lands")
        .expect("write ok");
    let second_result = second.await.expect("queued task settles");
    if let Err(error) = &second_result {
        assert!(error.contains("disposed before the queued"), "{error}");
    } else {
        panic!("queued write must reject, got Ok");
    }
    assert_eq!(
        provider.resolve(&key()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "one".to_string(),
            source: "file".to_string()
        })
    );
    assert_eq!(
        provider.resolve(&credential_ref("DSH_CRED_DRAIN_B")).await,
        None
    );
}

// ---------------------------------------------------------------------------
// real hot reload (notify backend)

#[tokio::test(flavor = "current_thread")]
async fn publishes_external_edits_replaces_the_snapshot_wholesale_and_suppresses_self_writes() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    write_credentials(&path, "DSH_CRED_PIPE: boot\n");
    let ctx = Context::root();
    let provider = LocalCredentialProvider::install(
        &ctx,
        Config {
            path: Some(path.clone()),
            watch: Some(true),
            debounce_ms: Some(50),
            ..Default::default()
        },
    )
    .expect("boot");
    let seen: Arc<parking_lot::Mutex<Vec<dsh_credentials::CredentialRef>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let seen_for_listener = seen.clone();
    let listener: Arc<cordis::Listener> = Arc::new(move |_ctx, args| {
        let reference = cordis::downcast::<dsh_credentials::CredentialRef>(&args[0]).cloned();
        let seen = seen_for_listener.clone();
        Box::pin(async move {
            if let Some(reference) = reference {
                seen.lock().push(reference);
            }
            None
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "credentials/updated",
        listener,
        cordis::EventOptions::default(),
    ));

    write_credentials(&path, "DSH_CRED_PIPE: live\nDSH_CRED_OTHER: extra\n");
    wait_for(
        "live lands",
        || {
            futures::executor::block_on(provider.resolve(&key()))
                == Some(dsh_credentials::ResolvedCredential {
                    value: "live".to_string(),
                    source: "file".to_string(),
                })
        },
        8000,
    )
    .await;

    // Wholesale replacement: an entry deleted on disk never lingers.
    write_credentials(&path, "DSH_CRED_PIPE: live\n");
    wait_for(
        "extra removed",
        || {
            futures::executor::block_on(provider.resolve(&credential_ref("DSH_CRED_OTHER")))
                .is_none()
        },
        8000,
    )
    .await;

    let before = seen.lock().len();
    provider.set(&key(), "self-written").await.expect("set");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    // Exactly the committed write's own event: the watcher echo of our own
    // content is recognized by the text cache and publishes nothing extra.
    assert_eq!(seen.lock().len(), before + 1);
    assert_eq!(
        provider.resolve(&key()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "self-written".to_string(),
            source: "file".to_string()
        })
    );
}
