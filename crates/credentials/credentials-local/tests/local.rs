//! Rust port of the TS `local.spec.ts` + `review-fixes.spec.ts` suites
//! (non-watcher parts): spec resolution, environment layering, document
//! validation, comment-preserving writes, read-modify-write under the
//! writer lock, the contained update fan-out, and the document editor's
//! entry isolation.
//!
//! Deviations:
//!
//! - The POSIX mode checks (`0o600` file, `0o700` dir, world-readable
//!   rejection) are `#[cfg(unix)]`-gated, mirroring the TS `win32` skips.
//! - Process-environment injection uses `std::env::set_var/remove_var`
//!   (unsafe in Rust 2024) on per-test unique names.
//! - The EISDIR boot case reports a generic read error on Windows (the OS
//!   reports access-denied there); the assertion checks error presence.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::{TempRoot, boot, write_credentials};
use cordis::{Context, arc};
use dsh_credentials::{CredentialProvider, credential_ref};
use dsh_credentials_local::{Config, LocalCredentialProvider, ResolvedSpec, resolve_spec};
use dsh_invariants::InvariantError;
use dsh_launch_environment::{
    DSH_LAUNCH_ENVIRONMENT_KEY, LaunchEnvironmentLayerInput, LaunchEnvironmentSource,
    create_launch_environment_snapshot,
};

fn key() -> dsh_credentials::CredentialRef {
    credential_ref("DSH_CRED_TEST")
}

fn other() -> dsh_credentials::CredentialRef {
    credential_ref("DSH_CRED_OTHER")
}

fn provider_of(ctx: &Context) -> Arc<dyn CredentialProvider> {
    ctx.get_typed::<Arc<dyn CredentialProvider>>("credentials", false)
        .expect("credentials service")
        .as_ref()
        .clone()
}

fn update_listener(ctx: &Context) -> Arc<parking_lot::Mutex<Vec<dsh_credentials::CredentialRef>>> {
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
    seen
}

// ---------------------------------------------------------------------------
// resolveSpec

#[test]
fn resolve_spec_defaults_to_the_harness_home_with_watching_on() {
    let spec = resolve_spec(&Config {
        dsh_home: Some("/custom/home".to_string()),
        ..Default::default()
    });
    assert_eq!(
        spec,
        ResolvedSpec {
            filename: std::path::absolute("/custom/home/.credentials.yaml")
                .expect("absolute")
                .to_string_lossy()
                .into_owned(),
            watch: true,
            debounce_ms: 100,
        }
    );
}

#[test]
fn resolve_spec_lets_an_explicit_path_win_over_the_home() {
    let spec = resolve_spec(&Config {
        path: Some("/etc/dsh/creds.yaml".to_string()),
        dsh_home: Some("/ignored".to_string()),
        watch: Some(false),
        debounce_ms: Some(5),
    });
    assert_eq!(
        spec,
        ResolvedSpec {
            filename: std::path::absolute("/etc/dsh/creds.yaml")
                .expect("absolute")
                .to_string_lossy()
                .into_owned(),
            watch: false,
            debounce_ms: 5,
        }
    );
}

// ---------------------------------------------------------------------------
// layering and reads

#[tokio::test(flavor = "current_thread")]
async fn treats_an_absent_file_as_an_empty_writable_store() {
    let temp = TempRoot::new();
    let (_ctx, provider) = boot(&temp.path(".credentials.yaml"), false);
    assert_eq!(provider.resolve(&key()).await, None);
    assert_eq!(
        provider.describe(&key()).await,
        dsh_credentials::CredentialInfo {
            configured: false,
            source: None,
            writable: true
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn serves_file_entries_alongside_comments_and_quoted_values() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    write_credentials(
        &path,
        "# notes\nDSH_CRED_TEST: plain\nDSH_CRED_OTHER: \"with space\"\n",
    );
    let (_ctx, provider) = boot(&path, false);
    assert_eq!(
        provider.resolve(&key()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "plain".to_string(),
            source: "file".to_string()
        })
    );
    assert_eq!(
        provider.resolve(&other()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "with space".to_string(),
            source: "file".to_string()
        })
    );
    assert_eq!(
        provider.describe(&key()).await,
        dsh_credentials::CredentialInfo {
            configured: true,
            source: Some("file".to_string()),
            writable: true
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn lets_a_non_empty_process_environment_win_read_only_over_the_file() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    write_credentials(&path, "DSH_CRED_TEST: from-file\n");
    let probe = format!("DSH_CRED_TEST_NON_EMPTY_{}", std::process::id());
    // SAFETY: test-process-local variable, removed at the end.
    unsafe { std::env::set_var(&probe, "from-env") };
    let reference = credential_ref(&probe);
    let (_ctx, provider) = boot(&path, false);
    assert_eq!(
        provider.resolve(&reference).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "from-env".to_string(),
            source: "env".to_string()
        })
    );
    assert_eq!(
        provider.describe(&reference).await,
        dsh_credentials::CredentialInfo {
            configured: true,
            source: Some("env".to_string()),
            writable: false
        }
    );
    unsafe { std::env::remove_var(&probe) };
}

#[tokio::test(flavor = "current_thread")]
async fn treats_an_empty_environment_value_as_absent_falling_through_to_the_file() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let probe = format!("DSH_CRED_TEST_EMPTY_{}", std::process::id());
    write_credentials(&path, &format!("{probe}: stored\n"));
    unsafe { std::env::set_var(&probe, "") };
    let reference = credential_ref(&probe);
    let (_ctx, provider) = boot(&path, false);
    assert_eq!(
        provider.resolve(&reference).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "stored".to_string(),
            source: "file".to_string()
        })
    );
    unsafe { std::env::remove_var(&probe) };
}

#[tokio::test(flavor = "current_thread")]
async fn fails_boot_loud_when_the_document_exists_but_cannot_be_read() {
    let temp = TempRoot::new();
    let path = temp.path("occupied");
    std::fs::create_dir_all(&path).expect("occupied dir");
    let ctx = Context::root();
    let error = LocalCredentialProvider::install(
        &ctx,
        Config {
            path: Some(path),
            watch: Some(false),
            ..Default::default()
        },
    )
    .err()
    .expect("boot rejects");
    assert!(!error.is_empty());
}

// ---------------------------------------------------------------------------
// layer ladder

fn layered_env(
    path: &str,
    layers: &[LaunchEnvironmentLayerInput],
) -> (Context, Arc<LocalCredentialProvider>) {
    let ctx = Context::root();
    let snapshot = create_launch_environment_snapshot(layers);
    ctx.provide(DSH_LAUNCH_ENVIRONMENT_KEY, Some(arc(snapshot)));
    let provider = LocalCredentialProvider::install(
        &ctx,
        Config {
            path: Some(path.to_string()),
            watch: Some(false),
            ..Default::default()
        },
    )
    .expect("boot layered");
    (ctx, provider)
}

fn process_layer(values: &[(&str, &str)]) -> LaunchEnvironmentLayerInput {
    LaunchEnvironmentLayerInput {
        source: LaunchEnvironmentSource::Process,
        path: None,
        values: values
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn lets_the_stored_value_beat_the_user_env_so_a_ui_write_takes_effect_immediately() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    write_credentials(&path, "DSH_CRED_TEST: stored\n");
    let (_ctx, provider) = layered_env(
        &path,
        &[
            process_layer(&[]),
            LaunchEnvironmentLayerInput {
                source: LaunchEnvironmentSource::UserEnv,
                path: Some("/home/.dsh/.env".to_string()),
                values: vec![("DSH_CRED_TEST".to_string(), "older-user-env".to_string())],
            },
        ],
    );
    assert_eq!(
        provider.resolve(&key()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "stored".to_string(),
            source: "file".to_string()
        })
    );
    assert_eq!(
        provider.describe(&key()).await,
        dsh_credentials::CredentialInfo {
            configured: true,
            source: Some("file".to_string()),
            writable: true
        }
    );
    provider.set(&key(), "rotated").await.expect("set");
    assert_eq!(
        provider.resolve(&key()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "rotated".to_string(),
            source: "file".to_string()
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn serves_the_user_env_only_when_nothing_is_stored() {
    let temp = TempRoot::new();
    let (_ctx, provider) = layered_env(
        &temp.path(".credentials.yaml"),
        &[
            process_layer(&[]),
            LaunchEnvironmentLayerInput {
                source: LaunchEnvironmentSource::UserEnv,
                path: Some("/home/.dsh/.env".to_string()),
                values: vec![("DSH_CRED_TEST".to_string(), "from-user-env".to_string())],
            },
        ],
    );
    assert_eq!(
        provider.resolve(&key()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "from-user-env".to_string(),
            source: "user-env".to_string(),
        })
    );
    assert_eq!(
        provider.describe(&key()).await,
        dsh_credentials::CredentialInfo {
            configured: true,
            source: Some("user-env".to_string()),
            writable: true,
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn serves_the_invoking_project_env_over_the_user_one_but_never_over_the_store() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let layers = vec![
        process_layer(&[]),
        LaunchEnvironmentLayerInput {
            source: LaunchEnvironmentSource::ProjectEnv,
            path: Some("/work/.env".to_string()),
            values: vec![("DSH_CRED_TEST".to_string(), "from-project".to_string())],
        },
        LaunchEnvironmentLayerInput {
            source: LaunchEnvironmentSource::UserEnv,
            path: Some("/home/.dsh/.env".to_string()),
            values: vec![("DSH_CRED_TEST".to_string(), "from-user".to_string())],
        },
    ];
    let (_ctx, bare) = layered_env(&path, &layers);
    assert_eq!(
        bare.resolve(&key()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "from-project".to_string(),
            source: "project-env".to_string(),
        })
    );

    write_credentials(&path, "DSH_CRED_TEST: stored\n");
    let (_ctx, stored) = layered_env(&path, &layers);
    assert_eq!(
        stored.resolve(&key()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "stored".to_string(),
            source: "file".to_string()
        })
    );
}

#[tokio::test(flavor = "current_thread")]
#[cfg(unix)]
async fn refuses_a_document_other_os_users_can_read() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    std::fs::write(&path, "DSH_CRED_TEST: leaked\n").expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let ctx = Context::root();
    let error = LocalCredentialProvider::install(
        &ctx,
        Config {
            path: Some(path.clone()),
            watch: Some(false),
            ..Default::default()
        },
    )
    .err()
    .expect("rejects world-readable");
    assert!(
        error.contains("readable beyond its owner (mode 644)"),
        "{error}"
    );
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
}

#[tokio::test(flavor = "current_thread")]
async fn propagates_a_permission_check_that_fails_for_a_reason_other_than_absence() {
    let temp = TempRoot::new();
    let not_a_directory = temp.path("occupied");
    std::fs::write(&not_a_directory, "a regular file\n").expect("write");
    let ctx = Context::root();
    let error = LocalCredentialProvider::install(
        &ctx,
        Config {
            path: Some(
                std::path::Path::new(&not_a_directory)
                    .join(".credentials.yaml")
                    .to_string_lossy()
                    .into_owned(),
            ),
            watch: Some(false),
            ..Default::default()
        },
    )
    .err()
    .expect("rejects");
    assert!(error.contains("ENOTDIR"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn lets_only_the_inherited_environment_shadow_the_store_read_only() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    write_credentials(&path, "DSH_CRED_TEST: stored\n");
    let (_ctx, provider) = layered_env(
        &path,
        &[
            process_layer(&[("DSH_CRED_TEST", "from-shell")]),
            LaunchEnvironmentLayerInput {
                source: LaunchEnvironmentSource::UserEnv,
                path: Some("/home/.dsh/.env".to_string()),
                values: vec![("DSH_CRED_TEST".to_string(), "from-user-env".to_string())],
            },
        ],
    );
    assert_eq!(
        provider.resolve(&key()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "from-shell".to_string(),
            source: "env".to_string()
        })
    );
    let error = provider
        .set(&key(), "next")
        .await
        .err()
        .expect("shadowed set rejects");
    assert!(error.contains("launching environment"), "{error}");
}

// ---------------------------------------------------------------------------
// document validation

#[tokio::test(flavor = "current_thread")]
async fn fails_boot_on_every_invalid_document_shape() {
    let cases = [
        ("just a string\n", "must be a mapping"),
        ("- DSH_CRED_TEST\n", "must be a mapping"),
        ("not-a-ref: value\n", "must match"),
        ("DSH_CRED_TEST: 123\n", "must be a string"),
        ("DSH_CRED_TEST: \"\"\n", "is empty"),
        (
            "DSH_CRED_TEST: one\nDSH_CRED_TEST: two\n",
            "invalid document",
        ),
        ("DSH_CRED_TEST: \"unterminated\n", "invalid document"),
    ];
    for (text, needle) in cases {
        let temp = TempRoot::new();
        let path = temp.path(".credentials.yaml");
        write_credentials(&path, text);
        let ctx = Context::root();
        let error = LocalCredentialProvider::install(
            &ctx,
            Config {
                path: Some(path),
                watch: Some(false),
                ..Default::default()
            },
        )
        .err()
        .expect("boot rejects");
        assert!(error.contains(needle), "{text:?} -> {error}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn never_puts_a_credential_value_in_a_diagnostic() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let secret = "sk-live-DO-NOT-LOG-abcdef123456";
    write_credentials(&path, &format!("DSH_CRED_TEST: \"{secret}\n"));
    let ctx = Context::root();
    let error = LocalCredentialProvider::install(
        &ctx,
        Config {
            path: Some(path),
            watch: Some(false),
            ..Default::default()
        },
    )
    .err()
    .expect("boot rejects");
    assert!(error.contains("invalid document"), "{error}");
    assert!(!error.contains(secret), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn reads_an_empty_document_as_an_empty_store() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    write_credentials(&path, "# nothing stored yet\n");
    let (_ctx, provider) = boot(&path, false);
    assert_eq!(provider.resolve(&key()).await, None);
}

// ---------------------------------------------------------------------------
// document writes

#[tokio::test(flavor = "current_thread")]
async fn adds_a_missing_key_to_a_fresh_document_and_emits_the_commit() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let (ctx, provider) = boot(&path, false);
    let seen = update_listener(&ctx);
    provider.set(&key(), "sk-fresh").await.expect("set");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "DSH_CRED_TEST: sk-fresh\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(
        provider.resolve(&key()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "sk-fresh".to_string(),
            source: "file".to_string()
        })
    );
    assert_eq!(seen.lock().clone(), vec![key()]);
}

#[tokio::test(flavor = "current_thread")]
async fn patches_one_entry_preserving_comments_and_every_untouched_entry() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    write_credentials(
        &path,
        "# deployment notes\nDSH_CRED_OTHER: keep\n\n# the one under edit\nDSH_CRED_TEST: old\n",
    );
    let (_ctx, provider) = boot(&path, false);
    provider.set(&key(), "new value!").await.expect("set");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "# deployment notes\nDSH_CRED_OTHER: keep\n\n# the one under edit\nDSH_CRED_TEST: new value!\n"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn round_trips_values_no_dotenv_line_could_represent() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let (_ctx, provider) = boot(&path, false);
    let multi_line = "line one\nline two";
    let mixed_quotes = "both ' and \"";
    provider.set(&key(), multi_line).await.expect("set");
    provider.set(&other(), mixed_quotes).await.expect("set");
    let (_ctx, reread) = boot(&path, false);
    assert_eq!(
        reread.resolve(&key()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: multi_line.to_string(),
            source: "file".to_string()
        })
    );
    assert_eq!(
        reread.resolve(&other()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: mixed_quotes.to_string(),
            source: "file".to_string()
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn unsets_only_the_owning_entry_with_its_own_annotation_and_keeps_an_absent_unset_silent() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    write_credentials(
        &path,
        "# about the doomed one\nDSH_CRED_TEST: gone\n# about the survivor\nDSH_CRED_OTHER: stays\n",
    );
    let (ctx, provider) = boot(&path, false);
    let seen = update_listener(&ctx);
    provider.unset(&key()).await.expect("unset");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "# about the survivor\nDSH_CRED_OTHER: stays\n"
    );
    provider.unset(&key()).await.expect("absent unset silent");
    assert_eq!(seen.lock().clone(), vec![key()]);
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_empty_values_and_writes_the_environment_would_shadow() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    write_credentials(&path, "DSH_CRED_TEST: stored\n");
    let (_ctx, provider) = boot(&path, false);

    let error = provider
        .set(&key(), "")
        .await
        .err()
        .expect("empty set rejects");
    assert!(error.contains("empty value"), "{error}");

    let probe = format!("DSH_CRED_TEST_SHADOW_{}", std::process::id());
    unsafe { std::env::set_var(&probe, "shadowing") };
    let reference = credential_ref(&probe);
    let error = provider
        .set(&reference, "next")
        .await
        .err()
        .expect("shadowed set rejects");
    assert!(error.contains("shadowed"), "{error}");
    let error = provider
        .unset(&reference)
        .await
        .err()
        .expect("shadowed unset rejects");
    assert!(error.contains("shadowed"), "{error}");
    unsafe { std::env::remove_var(&probe) };
}

#[tokio::test(flavor = "current_thread")]
async fn leaves_an_empty_mapping_after_unsetting_the_only_entry() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    write_credentials(&path, "DSH_CRED_TEST: only\n");
    let (_ctx, provider) = boot(&path, false);
    provider.unset(&key()).await.expect("unset");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "{}\n");
    let (_ctx, reread) = boot(&path, false);
    assert_eq!(reread.resolve(&key()).await, None);
}

#[tokio::test(flavor = "current_thread")]
async fn fails_a_write_loud_when_the_on_disk_document_became_invalid() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let (_ctx, provider) = boot(&path, false);
    write_credentials(&path, "DSH_CRED_TEST: \"unterminated\n");
    let error = provider
        .set(&other(), "lands")
        .await
        .err()
        .expect("write rejects");
    assert!(error.contains("invalid document"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn chains_past_a_rejected_write_so_one_bad_value_cannot_poison_the_queue() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let (_ctx, provider) = boot(&path, false);
    let k = key();
    let o = other();
    let bad = provider.set(&k, "");
    let good = provider.set(&o, "lands");
    assert!(bad.await.is_err());
    good.await.expect("good write lands");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "DSH_CRED_OTHER: lands\n"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn serializes_concurrent_writes_so_both_land_in_the_one_document() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let (_ctx, provider) = boot(&path, false);
    let k = key();
    let o = other();
    let (left, right) = tokio::join!(provider.set(&k, "one"), provider.set(&o, "two"));
    left.expect("left");
    right.expect("right");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "DSH_CRED_TEST: one\nDSH_CRED_OTHER: two\n"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn refuses_writes_after_disposal() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let (_ctx, provider) = boot(&path, false);
    provider.drain().await;
    let error = provider
        .set(&key(), "late")
        .await
        .err()
        .expect("late set rejects");
    assert!(error.contains("disposed"), "{error}");
}

// ---------------------------------------------------------------------------
// read-modify-write (review fixes)

#[tokio::test(flavor = "current_thread")]
async fn folds_an_unobserved_external_edit_into_a_write_instead_of_overwriting_it() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let (ctx, provider) = boot(&path, false);
    let seen = update_listener(&ctx);
    let alpha = credential_ref("DSH_REVIEW_ALPHA");
    let beta = credential_ref("DSH_REVIEW_BETA");
    provider.set(&alpha, "one").await.expect("set");
    write_credentials(&path, "DSH_REVIEW_ALPHA: one\nDSH_REVIEW_BETA: external\n");
    provider.set(&alpha, "two").await.expect("set");
    let text = std::fs::read_to_string(&path).expect("read");
    assert!(text.contains("DSH_REVIEW_BETA: external"), "{text}");
    assert!(text.contains("DSH_REVIEW_ALPHA: two"), "{text}");
    assert_eq!(
        seen.lock().clone(),
        vec![alpha.clone(), beta.clone(), alpha.clone()]
    );
    assert_eq!(
        provider.resolve(&beta).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "external".to_string(),
            source: "file".to_string()
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn keeps_both_refs_when_two_providers_write_the_same_document_concurrently() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let alpha = credential_ref("DSH_REVIEW_ALPHA");
    let beta = credential_ref("DSH_REVIEW_BETA");
    let (_ctx, first) = boot(&path, false);
    let (_ctx, second) = boot(&path, false);
    let first_loop = async {
        for value in ["1", "2", "3"] {
            first.set(&alpha, value).await.expect("set");
        }
    };
    let second_loop = async {
        for value in ["1", "2", "3"] {
            second.set(&beta, value).await.expect("set");
        }
    };
    tokio::join!(first_loop, second_loop);
    let (_ctx, third) = boot(&path, false);
    assert_eq!(
        third.resolve(&alpha).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "3".to_string(),
            source: "file".to_string()
        })
    );
    assert_eq!(
        third.resolve(&beta).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "3".to_string(),
            source: "file".to_string()
        })
    );
}

#[tokio::test(flavor = "current_thread")]
#[cfg(unix)]
async fn creates_the_credentials_directory_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempRoot::new();
    let home = temp.path("home");
    let (_ctx, provider) = boot(
        &std::path::Path::new(&home)
            .join(".credentials.yaml")
            .to_string_lossy()
            .as_ref(),
        false,
    );
    provider.set(&key(), "one").await.expect("set");
    assert_eq!(
        std::fs::metadata(&home).expect("stat").permissions().mode() & 0o777,
        0o700
    );
}

// ---------------------------------------------------------------------------
// contained update fan-out

#[tokio::test(flavor = "current_thread")]
async fn does_not_fail_a_committed_set_when_a_listener_throws_and_later_listeners_still_run() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let (ctx, provider) = boot(&path, false);
    let second_ran: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let boom: Arc<cordis::Listener> =
        Arc::new(move |_ctx, _args| Box::pin(async move { panic!("observer boom") }));
    let _ = futures::executor::block_on(ctx.on(
        "credentials/updated",
        boom,
        cordis::EventOptions::default(),
    ));
    let second_ran_for_listener = second_ran.clone();
    let second: Arc<cordis::Listener> = Arc::new(move |_ctx, _args| {
        let second_ran = second_ran_for_listener.clone();
        Box::pin(async move {
            second_ran.store(true, Ordering::SeqCst);
            None
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "credentials/updated",
        second,
        cordis::EventOptions::default(),
    ));
    provider.set(&key(), "one").await.expect("set resolves");
    assert!(second_ran.load(Ordering::SeqCst));
    assert_eq!(
        provider.resolve(&key()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "one".to_string(),
            source: "file".to_string()
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn rethrows_an_invariant_coded_failure_after_the_commit_and_the_remaining_listeners() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let (ctx, provider) = boot(&path, false);
    let invariant: Arc<cordis::Listener> = Arc::new(move |_ctx, _args| {
        Box::pin(async move {
            std::panic::panic_any(InvariantError::new(
                "@deepseek-ai/dsh-credentials",
                "forged relation",
            ));
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "credentials/updated",
        invariant,
        cordis::EventOptions::default(),
    ));
    let second_ran: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let second_ran_for_listener = second_ran.clone();
    let second: Arc<cordis::Listener> = Arc::new(move |_ctx, _args| {
        let second_ran = second_ran_for_listener.clone();
        Box::pin(async move {
            second_ran.store(true, Ordering::SeqCst);
            None
        })
    });
    let _ = futures::executor::block_on(ctx.on(
        "credentials/updated",
        second,
        cordis::EventOptions::default(),
    ));
    let error = provider
        .set(&key(), "one")
        .await
        .err()
        .expect("invariant failure rethrows");
    assert!(error.contains("forged relation"), "{error}");
    assert!(second_ran.load(Ordering::SeqCst));
    assert!(
        std::fs::read_to_string(&path)
            .expect("read")
            .contains("DSH_CRED_TEST: one")
    );
    assert_eq!(
        provider.resolve(&key()).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "one".to_string(),
            source: "file".to_string()
        })
    );
}

// ---------------------------------------------------------------------------
// document editor (review fixes)

#[tokio::test(flavor = "current_thread")]
async fn leaves_a_sibling_multi_line_value_untouched_while_patching_one_entry() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let wrapped = "DSH_REVIEW_WRAPPED: |-\n  line1\n  line2\nDSH_REVIEW_ALPHA: a\n";
    write_credentials(&path, wrapped);
    let (_ctx, provider) = boot(&path, false);
    let alpha = credential_ref("DSH_REVIEW_ALPHA");
    provider.set(&alpha, "b").await.expect("set");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "DSH_REVIEW_WRAPPED: |-\n  line1\n  line2\nDSH_REVIEW_ALPHA: b\n"
    );
    assert_eq!(
        provider
            .resolve(&credential_ref("DSH_REVIEW_WRAPPED"))
            .await,
        Some(dsh_credentials::ResolvedCredential {
            value: "line1\nline2".to_string(),
            source: "file".to_string()
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn stores_a_value_that_looks_like_another_entry_without_creating_one() {
    let temp = TempRoot::new();
    let path = temp.path(".credentials.yaml");
    let (_ctx, provider) = boot(&path, false);
    let alpha = credential_ref("DSH_REVIEW_ALPHA");
    let inner = credential_ref("DSH_REVIEW_INNER");
    provider
        .set(&alpha, "DSH_REVIEW_INNER: injected")
        .await
        .expect("set");
    let (_ctx, reread) = boot(&path, false);
    assert_eq!(
        reread.resolve(&alpha).await,
        Some(dsh_credentials::ResolvedCredential {
            value: "DSH_REVIEW_INNER: injected".to_string(),
            source: "file".to_string(),
        })
    );
    assert_eq!(reread.resolve(&inner).await, None);
}
