//! Rust port of the TS `packages/workspace/workspace/tests/workspace.spec.ts`
//! suite: registry lifecycle and bootstrap, create/lookup, durable ordering,
//! session ordering, header-validated membership projection, mutation/status,
//! and the registry-global session archive.
//!
//! Deviations from the TS spec:
//!
//! - Error TYPES collapse into `Result<_, String>`: assertions match the
//!   ported message prose instead of `instanceof`.
//! - `fiber.dispose()` collapses into `registry.domain().close()` before a
//!   restart harness over the same medium.
//! - Windows canonicalization may return `\\?\`-prefixed paths, so every
//!   stored fixture and assertion uses the canonicalized spelling.

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use common::{
    FailureSpec, FakeLiveSessions, Harness, MemoryMediaPool, TempRoot, harness, harness_with_backend,
    header, record, selective_failure_backend, settle, sid, stored_pool, stored_record,
    stored_state, wid,
};
use dsh_workspace::{LiveSessionStore, WorkspaceDomainState, WorkspacePendingMutation, realpath_normalize};

fn canonical(dir: &str) -> String {
    futures::executor::block_on(realpath_normalize(dir)).expect("canonical")
}

fn paths(list: &[dsh_workspace::Workspace]) -> Vec<String> {
    list.iter().map(|workspace| workspace.path()).collect()
}

fn id_strings(list: &[dsh_workspace::Workspace]) -> Vec<String> {
    list.iter().map(|workspace| workspace.id().to_string()).collect()
}

fn order_strings(ids: &[dsh_workspace::WorkspaceId]) -> Vec<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

fn session_strings(ids: &[dsh_session::SessionId]) -> Vec<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

fn global_changes(result: &Harness) -> usize {
    result
        .changes
        .lock()
        .iter()
        .filter(|change| matches!(change, dsh_storage_domain::DomainChanged::Put { table, .. } if table.is_empty()))
        .count()
}

#[cfg(windows)]
fn make_symlink(target: &str, link: &str) {
    std::os::windows::fs::symlink_dir(target, link).expect("symlink alias");
}

#[cfg(unix)]
fn make_symlink(target: &str, link: &str) {
    std::os::unix::fs::symlink(target, link).expect("symlink alias");
}

fn initialized(workspace_ids: Vec<dsh_workspace::WorkspaceId>) -> WorkspaceDomainState {
    WorkspaceDomainState {
        initialized: true,
        workspace_ids,
        archived_session_ids: Vec::new(),
        pending_mutation: None,
    }
}

// ---------------------------------------------------------------------------
// lifecycle and bootstrap

#[tokio::test(flavor = "current_thread")]
async fn bootstraps_once_from_list_headers_only_in_workspace_and_session_created_at_order() {
    let temp = TempRoot::new();
    let older = canonical(&temp.dir("older"));
    let newer = canonical(&temp.dir("newer"));
    let alias = temp.path("older-link");
    make_symlink(&older, &alias);
    let plain = temp.path("plain.txt");
    std::fs::write(&plain, "not a directory").expect("write file");
    let missing = temp.path("missing");

    let result = harness(
        Arc::new(MemoryMediaPool::new()),
        &[
            header("older-first", Some(&older), 100),
            header("newer-only", Some(&newer), 500),
            header("older-latest", Some(&alias), 300),
            header("no-cwd", None, 900),
            header("missing-dir", Some(&missing), 800),
            header("plain-file", Some(&plain), 700),
        ],
        None,
    )
    .await;

    assert_eq!(result.persistence.list_calls.load(Ordering::SeqCst), 1);
    assert_eq!(result.persistence.load_calls.load(Ordering::SeqCst), 0);
    assert_eq!(result.persistence.inspect_calls.load(Ordering::SeqCst), 0);
    let list = result.registry.list().expect("list");
    assert_eq!(paths(&list), vec![newer, older]);
    assert_eq!(
        list.iter()
            .map(|workspace| session_strings(&workspace.session_ids()))
            .collect::<Vec<_>>(),
        vec![
            vec!["newer-only".to_string()],
            vec!["older-latest".to_string(), "older-first".to_string()],
        ],
    );
    let state = stored_state(&result.pool);
    assert!(state.initialized);
    assert_eq!(order_strings(&state.workspace_ids), id_strings(&list));
    assert!(state.archived_session_ids.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn breaks_equal_bootstrap_timestamps_by_session_id_and_canonical_path() {
    let temp = TempRoot::new();
    let first = canonical(&temp.dir("tie-first"));
    let second = canonical(&temp.dir("tie-second"));

    let result = harness(
        Arc::new(MemoryMediaPool::new()),
        &[
            header("z-session", Some(&first), 100),
            header("a-session", Some(&first), 100),
            header("second-session", Some(&second), 100),
        ],
        None,
    )
    .await;

    let list = result.registry.list().expect("list");
    let path_set: std::collections::HashSet<String> = paths(&list).into_iter().collect();
    assert_eq!(
        path_set,
        std::collections::HashSet::from([first.clone(), second.clone()])
    );
    let first_workspace = list
        .iter()
        .find(|workspace| workspace.path() == first)
        .expect("first workspace");
    assert_eq!(
        session_strings(&first_workspace.session_ids()),
        vec!["a-session".to_string(), "z-session".to_string()]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn does_not_rerun_bootstrap_for_a_genuinely_initialized_empty_registry() {
    let temp = TempRoot::new();
    let late = canonical(&temp.dir("late-cwd-only"));
    let pool = Arc::new(MemoryMediaPool::new());
    let first = harness(pool.clone(), &[], None).await;
    assert_eq!(first.persistence.list_calls.load(Ordering::SeqCst), 1);
    first.registry.domain().close().await;

    let second = harness(pool.clone(), &[header("late", Some(&late), 100)], None).await;
    assert_eq!(second.persistence.list_calls.load(Ordering::SeqCst), 0);
    assert_eq!(second.registry.list().expect("list").len(), 0);
    let state = stored_state(&pool);
    assert_eq!(
        state,
        WorkspaceDomainState {
            initialized: true,
            workspace_ids: vec![],
            archived_session_ids: vec![],
            pending_mutation: None,
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn reuses_partial_records_after_a_bootstrap_record_write_fails() {
    let temp = TempRoot::new();
    let first_dir = canonical(&temp.dir("partial-first"));
    let second_dir = canonical(&temp.dir("partial-second"));
    let sessions = [
        header("first", Some(&first_dir), 200),
        header("second", Some(&second_dir), 100),
    ];
    let pool = Arc::new(MemoryMediaPool::new());
    let error = harness_with_backend(
        pool.clone(),
        &sessions,
        None,
        Some(selective_failure_backend(
            pool.clone(),
            FailureSpec { put_at: Some(2), ..Default::default() },
        )),
    )
    .await
    .err()
    .expect("bootstrap rejects");
    assert!(error.contains("selected bootstrap put failure"), "{error}");
    {
        let media = pool.media.lock();
        let medium = media.get("workspace").expect("medium");
        assert_eq!(medium.tables.get("workspaces").expect("table").len(), 1);
        assert!(medium.global.is_null());
    }

    let retried = harness(pool.clone(), &sessions, None).await;
    assert_eq!(retried.registry.list().expect("list").len(), 2);
    let media = pool.media.lock();
    let medium = media.get("workspace").expect("medium");
    assert_eq!(medium.tables.get("workspaces").expect("table").len(), 2);
    drop(media);
    assert!(stored_state(&pool).initialized);
}

#[tokio::test(flavor = "current_thread")]
async fn reuses_durable_order_when_the_final_initialized_marker_write_fails() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("marker-retry"));
    let sessions = [header("session", Some(&dir), 100)];
    let pool = Arc::new(MemoryMediaPool::new());
    let error = harness_with_backend(
        pool.clone(),
        &sessions,
        None,
        Some(selective_failure_backend(
            pool.clone(),
            FailureSpec { global_at: Some(2), ..Default::default() },
        )),
    )
    .await
    .err()
    .expect("bootstrap rejects");
    assert!(error.contains("selected bootstrap marker failure"), "{error}");
    let state = stored_state(&pool);
    assert!(!state.initialized);
    assert_eq!(state.workspace_ids.len(), 1);

    let retried = harness(pool.clone(), &sessions, None).await;
    assert_eq!(retried.registry.list().expect("list").len(), 1);
    {
        let media = pool.media.lock();
        let medium = media.get("workspace").expect("medium");
        assert_eq!(medium.tables.get("workspaces").expect("table").len(), 1);
    }
    assert!(stored_state(&pool).initialized);
}

#[tokio::test(flavor = "current_thread")]
async fn merges_partial_records_and_leaves_an_already_accounted_cwd_drift_ungrouped() {
    let temp = TempRoot::new();
    let owned = canonical(&temp.dir("partial-owned"));
    let prior = canonical(&temp.dir("partial-prior"));
    let drifted = canonical(&temp.dir("partial-drifted"));
    let owned_id = wid("00000000-0000-4000-8000-000000000010");
    let prior_id = wid("00000000-0000-4000-8000-000000000011");
    let pool = stored_pool(
        &[
            ("00000000-0000-4000-8000-000000000010", record(&owned, &["old"], "2026-07-24T00:00:00.000Z")),
            ("00000000-0000-4000-8000-000000000011", record(&prior, &["drift"], "2026-07-23T00:00:00.000Z")),
        ],
        WorkspaceDomainState {
            initialized: false,
            workspace_ids: vec![],
            archived_session_ids: vec![],
            pending_mutation: None,
        },
        false,
    );
    let result = harness(
        pool.clone(),
        &[
            header("new", Some(&owned), 200),
            header("old", Some(&owned), 100),
            header("drift", Some(&drifted), 300),
        ],
        None,
    )
    .await;

    let list = result.registry.list().expect("list");
    assert!(list.iter().any(|workspace| workspace.id() == &owned_id));
    let owned_workspace = result.registry.get(&owned_id).expect("owned entity");
    assert_eq!(
        session_strings(&owned_workspace.session_ids()),
        vec!["new".to_string(), "old".to_string()]
    );
    assert!(!list.iter().any(|workspace| workspace.path() == drifted));
    let _ = prior_id;
}

#[tokio::test(flavor = "current_thread")]
async fn orders_headerless_partial_records_by_prior_order_then_stable_id() {
    let temp = TempRoot::new();
    let first = canonical(&temp.dir("fallback-first"));
    let second = canonical(&temp.dir("fallback-second"));
    let first_id = wid("00000000-0000-4000-8000-000000000020");
    let second_id = wid("00000000-0000-4000-8000-000000000021");
    let entries = [
        ("00000000-0000-4000-8000-000000000021", record(&second, &[], "2026-07-24T00:00:00.000Z")),
        ("00000000-0000-4000-8000-000000000020", record(&first, &[], "2026-07-24T00:00:00.000Z")),
    ];

    let prior_pool = stored_pool(
        &entries,
        WorkspaceDomainState {
            initialized: false,
            workspace_ids: vec![second_id.clone(), first_id.clone()],
            archived_session_ids: vec![],
            pending_mutation: None,
        },
        false,
    );
    let prior = harness(prior_pool, &[], None).await;
    assert_eq!(
        id_strings(&prior.registry.list().expect("list")),
        order_strings(&[second_id.clone(), first_id.clone()])
    );

    let by_id_pool = stored_pool(
        &entries,
        WorkspaceDomainState {
            initialized: false,
            workspace_ids: vec![],
            archived_session_ids: vec![],
            pending_mutation: None,
        },
        false,
    );
    let by_id = harness(by_id_pool, &[], None).await;
    assert_eq!(
        id_strings(&by_id.registry.list().expect("list")),
        order_strings(&[first_id.clone(), second_id.clone()])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn closes_its_domain_on_disposal_and_reloads_the_persisted_stable_order() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("replug"));
    let pool = Arc::new(MemoryMediaPool::new());
    let result = harness(pool.clone(), &[], None).await;
    let first = result.registry.create(&dir, None).await.expect("create");
    // The TS spec disposes the registry fiber (closing the domain) and
    // re-plugs the same plugin on the same ctx; the Rust service store
    // rejects double registration, so the port restarts over the same
    // medium through a fresh harness instead.
    result.registry.domain().close().await;

    let next = harness(pool.clone(), &[], None).await;
    assert_eq!(
        id_strings(&next.registry.list().expect("list")),
        vec![first.id().to_string()]
    );
}

// ---------------------------------------------------------------------------
// create and lookup

#[tokio::test(flavor = "current_thread")]
async fn creates_newest_first_and_idempotently_reuses_a_canonical_path_without_retitling() {
    let temp = TempRoot::new();
    let first_dir = canonical(&temp.dir("first"));
    let second_dir = canonical(&temp.dir("second"));
    let alias = temp.path("first-link");
    make_symlink(&first_dir, &alias);

    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    let first = result.registry.create(&first_dir, Some("Original")).await.expect("create");
    let second = result.registry.create(&second_dir, None).await.expect("create");
    let reused = result.registry.create(&alias, Some("Ignored")).await.expect("reuse");
    assert_eq!(reused, first);
    assert_eq!(first.title(), "Original");
    assert_eq!(result.registry.list().expect("list"), vec![second.clone(), first.clone()]);
    let state = stored_state(&result.pool);
    assert_eq!(
        order_strings(&state.workspace_ids),
        vec![second.id().to_string(), first.id().to_string()]
    );
    let resolved = result.registry.resolve_by_path(&alias).await.expect("resolve");
    assert_eq!(resolved, Some(first.clone()));
    let unowned = canonical(&temp.dir("unowned"));
    assert_eq!(
        result.registry.resolve_by_path(&unowned).await.expect("resolve"),
        None
    );
}

#[tokio::test(flavor = "current_thread")]
async fn serializes_concurrent_same_path_creates_into_one_entity() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("concurrent"));
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    let (left, right) = tokio::join!(
        result.registry.create(&dir, Some("Winner")),
        result.registry.create(&dir, Some("Loser")),
    );
    let left = left.expect("left create");
    let right = right.expect("right create");
    assert_eq!(left, right);
    assert_eq!(result.registry.list().expect("list"), vec![left]);
    let media = result.pool.media.lock();
    let medium = media.get("workspace").expect("medium");
    assert_eq!(medium.tables.get("workspaces").expect("table").len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn allows_a_duplicate_display_name_on_a_different_canonical_path() {
    let temp = TempRoot::new();
    let first_dir = canonical(&temp.dir("named-first"));
    let second_dir = canonical(&temp.dir("named-second"));
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    let first = result.registry.create(&first_dir, Some("Shared")).await.expect("create");
    let second = result.registry.create(&second_dir, Some("Shared")).await.expect("create");
    assert_eq!(first.title(), "Shared");
    assert_eq!(second.title(), "Shared");
    assert_eq!(result.registry.list().expect("list"), vec![second, first]);
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_nonexistent_and_non_directory_paths_without_changing_order() {
    let temp = TempRoot::new();
    let parent = canonical(&temp.dir("invalid"));
    let file = temp.path("plain.txt");
    std::fs::write(&file, "file").expect("write file");
    let missing = temp.path("missing");

    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    let error = result.registry.create(&missing, None).await.err().expect("rejects");
    assert!(error.contains("cannot create a workspace at"), "{error}");
    let error = result.registry.create(&file, None).await.err().expect("rejects");
    assert!(error.contains("path is not a directory"), "{error}");
    let error = result
        .registry
        .resolve_by_path(&missing)
        .await
        .err()
        .expect("rejects");
    assert!(error.contains("cannot resolve workspace path"), "{error}");
    assert_eq!(result.registry.list().expect("list").len(), 0);
    let _ = parent;
}

#[tokio::test(flavor = "current_thread")]
async fn rolls_back_the_provisional_cache_when_the_record_write_fails() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("write-failure"));
    let pool = Arc::new(MemoryMediaPool::new());
    let result = harness_with_backend(
        pool.clone(),
        &[],
        None,
        Some(selective_failure_backend(
            pool.clone(),
            FailureSpec { put_at: Some(1), ..Default::default() },
        )),
    )
    .await
    .expect("install");
    let error = result.registry.create(&dir, None).await.err().expect("create rejects");
    assert!(error.contains("selected bootstrap put failure"), "{error}");
    assert_eq!(result.registry.list().expect("list").len(), 0);
    assert!(result.registry.create(&dir, None).await.is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn does_not_publish_a_workspace_when_its_pending_marker_cannot_be_written() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("pending-marker-write-failure"));
    let pool = Arc::new(MemoryMediaPool::new());
    let result = harness_with_backend(
        pool.clone(),
        &[],
        None,
        Some(selective_failure_backend(
            pool.clone(),
            FailureSpec { global_at: Some(2), ..Default::default() },
        )),
    )
    .await
    .expect("install");
    let error = result.registry.create(&dir, None).await.err().expect("create rejects");
    assert!(error.contains("selected bootstrap marker failure"), "{error}");
    assert_eq!(result.registry.list().expect("list").len(), 0);
    let media = pool.media.lock();
    let medium = media.get("workspace").expect("medium");
    let rows = medium.tables.get("workspaces").map(|table| table.len()).unwrap_or(0);
    assert_eq!(rows, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn rolls_back_a_record_when_registry_order_persistence_fails() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("order-write-failure"));
    let pool = Arc::new(MemoryMediaPool::new());
    let result = harness_with_backend(
        pool.clone(),
        &[],
        None,
        Some(selective_failure_backend(
            pool.clone(),
            FailureSpec { global_at: Some(3), ..Default::default() },
        )),
    )
    .await
    .expect("install");
    let error = result.registry.create(&dir, None).await.err().expect("create rejects");
    assert!(error.contains("marker failure"), "{error}");
    assert_eq!(result.registry.list().expect("list").len(), 0);
    let media = pool.media.lock();
    let medium = media.get("workspace").expect("medium");
    assert_eq!(medium.tables.get("workspaces").expect("table").len(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn reports_both_order_and_rollback_failures_while_retaining_the_recoverable_record() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("rollback-write-failure"));
    let pool = Arc::new(MemoryMediaPool::new());
    let result = harness_with_backend(
        pool.clone(),
        &[],
        None,
        Some(selective_failure_backend(
            pool.clone(),
            FailureSpec {
                global_at: Some(3),
                delete_at: Some(1),
                ..Default::default()
            },
        )),
    )
    .await
    .expect("install");
    let error = result.registry.create(&dir, None).await.err().expect("create rejects");
    assert!(error.contains("selected bootstrap marker failure"), "{error}");
    assert!(error.contains("selected rollback delete failure"), "{error}");
    let media = pool.media.lock();
    let medium = media.get("workspace").expect("medium");
    assert_eq!(medium.tables.get("workspaces").expect("table").len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn reports_a_record_write_and_pending_marker_rollback_failure_together() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("record-marker-rollback-failure"));
    let pool = Arc::new(MemoryMediaPool::new());
    let result = harness_with_backend(
        pool.clone(),
        &[],
        None,
        Some(selective_failure_backend(
            pool.clone(),
            FailureSpec {
                put_at: Some(1),
                global_at: Some(3),
                ..Default::default()
            },
        )),
    )
    .await
    .expect("install");
    let error = result.registry.create(&dir, None).await.err().expect("create rejects");
    assert!(error.contains("selected bootstrap put failure"), "{error}");
    assert!(error.contains("selected bootstrap marker failure"), "{error}");
    let state = stored_state(&pool);
    assert!(matches!(
        state.pending_mutation,
        Some(WorkspacePendingMutation::Create { .. })
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn reports_an_order_write_and_pending_marker_rollback_failure_together() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("order-marker-rollback-failure"));
    let pool = Arc::new(MemoryMediaPool::new());
    let result = harness_with_backend(
        pool.clone(),
        &[],
        None,
        Some(selective_failure_backend(
            pool.clone(),
            FailureSpec {
                global_at: Some(3),
                extra_global_at: std::collections::HashSet::from([4]),
                ..Default::default()
            },
        )),
    )
    .await
    .expect("install");
    let error = result.registry.create(&dir, None).await.err().expect("create rejects");
    assert!(error.contains("selected bootstrap marker failure"), "{error}");
    let state = stored_state(&pool);
    assert!(matches!(
        state.pending_mutation,
        Some(WorkspacePendingMutation::Create { .. })
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn deletes_only_the_registration_and_leaves_its_directory_and_session_headers_untouched() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("delete-registration"));
    let result = harness(
        Arc::new(MemoryMediaPool::new()),
        &[header("kept-session", Some(&dir), 0)],
        None,
    )
    .await;
    let workspace = result.registry.create(&dir, None).await.expect("create");
    workspace.attach_session(&sid("kept-session")).await.expect("attach");

    assert_eq!(result.registry.delete(workspace.id()).await.expect("delete"), true);
    assert_eq!(result.registry.delete(workspace.id()).await.expect("delete"), false);
    assert_eq!(result.registry.get(workspace.id()), None);
    assert_eq!(result.registry.list().expect("list").len(), 0);
    let state = stored_state(&result.pool);
    assert_eq!(
        state,
        WorkspaceDomainState {
            initialized: true,
            workspace_ids: vec![],
            archived_session_ids: vec![],
            pending_mutation: None,
        }
    );
    {
        let media = result.pool.media.lock();
        let medium = media.get("workspace").expect("medium");
        assert!(!medium
            .tables
            .get("workspaces")
            .expect("table")
            .contains_key(workspace.id().as_str()));
    }
    assert_eq!(realpath_normalize(&dir).await.expect("realpath"), dir);
    assert_eq!(result.persistence.list_calls.load(Ordering::SeqCst), 1);
    assert_eq!(result.persistence.load_calls.load(Ordering::SeqCst), 0);
    assert_eq!(result.persistence.inspect_calls.load(Ordering::SeqCst), 0);

    let reregistered = result.registry.create(&dir, None).await.expect("recreate");
    assert_ne!(reregistered.id(), workspace.id());
    assert_eq!(reregistered.path(), dir);
    assert_eq!(reregistered.session_ids().len(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn rolls_registry_order_and_cache_back_when_record_deletion_fails() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("delete-rollback"));
    let pool = Arc::new(MemoryMediaPool::new());
    let result = harness_with_backend(
        pool.clone(),
        &[],
        None,
        Some(selective_failure_backend(
            pool.clone(),
            FailureSpec { delete_at: Some(1), ..Default::default() },
        )),
    )
    .await
    .expect("install");
    let workspace = result.registry.create(&dir, None).await.expect("create");

    let error = result.registry.delete(workspace.id()).await.err().expect("delete rejects");
    assert!(error.contains("selected rollback delete failure"), "{error}");
    assert_eq!(result.registry.get(workspace.id()), Some(workspace.clone()));
    assert_eq!(result.registry.list().expect("list"), vec![workspace.clone()]);
    let state = stored_state(&pool);
    assert_eq!(order_strings(&state.workspace_ids), vec![workspace.id().to_string()]);
    let stored = stored_record(&pool, workspace.id().as_str());
    assert_eq!(stored.path, dir);
}

#[tokio::test(flavor = "current_thread")]
async fn commits_deletion_and_leaves_a_recoverable_marker_when_marker_cleanup_fails() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("delete-marker-cleanup"));
    let pool = Arc::new(MemoryMediaPool::new());
    let first = harness_with_backend(
        pool.clone(),
        &[],
        None,
        Some(selective_failure_backend(
            pool.clone(),
            FailureSpec { global_at: Some(5), ..Default::default() },
        )),
    )
    .await
    .expect("install");
    let workspace = first.registry.create(&dir, None).await.expect("create");

    assert_eq!(first.registry.delete(workspace.id()).await.expect("delete"), true);
    assert_eq!(first.registry.list().expect("list").len(), 0);
    let state = stored_state(&pool);
    assert_eq!(
        state,
        WorkspaceDomainState {
            initialized: true,
            workspace_ids: vec![],
            archived_session_ids: vec![],
            pending_mutation: Some(WorkspacePendingMutation::Delete {
                workspace_id: workspace.id().clone(),
            }),
        }
    );
    let reregistered = first.registry.create(&dir, None).await.expect("recreate");
    assert_ne!(reregistered.id(), workspace.id());
    let state = stored_state(&pool);
    assert_eq!(
        state,
        WorkspaceDomainState {
            initialized: true,
            workspace_ids: vec![reregistered.id().clone()],
            archived_session_ids: vec![],
            pending_mutation: None,
        }
    );
    first.registry.domain().close().await;

    let restarted = harness(pool.clone(), &[], None).await;
    assert_eq!(
        id_strings(&restarted.registry.list().expect("list")),
        vec![reregistered.id().to_string()]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn keeps_the_failed_deletion_unpublished_when_record_and_order_rollback_both_fail() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("delete-double-failure"));
    let pool = Arc::new(MemoryMediaPool::new());
    let result = harness_with_backend(
        pool.clone(),
        &[],
        None,
        Some(selective_failure_backend(
            pool.clone(),
            FailureSpec {
                delete_at: Some(1),
                global_at: Some(5),
                ..Default::default()
            },
        )),
    )
    .await
    .expect("install");
    let workspace = result.registry.create(&dir, None).await.expect("create");

    let error = result.registry.delete(workspace.id()).await.err().expect("delete rejects");
    assert!(error.contains("selected rollback delete failure"), "{error}");
    assert_eq!(result.registry.get(workspace.id()), None);
    let state = stored_state(&pool);
    assert_eq!(state.workspace_ids.len(), 0);
    assert!(matches!(
        state.pending_mutation,
        Some(WorkspacePendingMutation::Delete { .. })
    ));
}

// ---------------------------------------------------------------------------
// registry ordering

#[tokio::test(flavor = "current_thread")]
async fn moves_a_workspace_before_an_anchor_or_to_the_end_and_restores_that_order_after_restart() {
    let temp = TempRoot::new();
    let first_dir = canonical(&temp.dir("order-first"));
    let second_dir = canonical(&temp.dir("order-second"));
    let third_dir = canonical(&temp.dir("order-third"));
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    let first = result.registry.create(&first_dir, None).await.expect("create");
    let second = result.registry.create(&second_dir, None).await.expect("create");
    let third = result.registry.create(&third_dir, None).await.expect("create");
    assert_eq!(
        id_strings(&result.registry.list().expect("list")),
        vec![third.id().to_string(), second.id().to_string(), first.id().to_string()]
    );

    let moved = result
        .registry
        .insert_before(&first.id(), Some(&second.id()))
        .await
        .expect("move");
    assert_eq!(
        order_strings(&moved),
        vec![third.id().to_string(), first.id().to_string(), second.id().to_string()]
    );
    let moved = result
        .registry
        .insert_before(&third.id(), None)
        .await
        .expect("move to end");
    assert_eq!(
        order_strings(&moved),
        vec![first.id().to_string(), second.id().to_string(), third.id().to_string()]
    );
    let state = stored_state(&result.pool);
    assert_eq!(
        order_strings(&state.workspace_ids),
        vec![first.id().to_string(), second.id().to_string(), third.id().to_string()]
    );

    result.registry.domain().close().await;
    let restarted = harness(result.pool.clone(), &[], None).await;
    assert_eq!(
        id_strings(&restarted.registry.list().expect("list")),
        vec![first.id().to_string(), second.id().to_string(), third.id().to_string()]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn keeps_self_anchored_and_already_positioned_moves_write_free_and_rejects_unknown_ids() {
    let temp = TempRoot::new();
    let first_dir = canonical(&temp.dir("order-noop-first"));
    let second_dir = canonical(&temp.dir("order-noop-second"));
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    let first = result.registry.create(&first_dir, None).await.expect("create");
    let second = result.registry.create(&second_dir, None).await.expect("create");
    settle().await;
    let written = result.changes.lock().len();

    result
        .registry
        .insert_before(&second.id(), Some(&second.id()))
        .await
        .expect("self anchor");
    result
        .registry
        .insert_before(&second.id(), Some(&first.id()))
        .await
        .expect("already positioned");
    result
        .registry
        .insert_before(&first.id(), None)
        .await
        .expect("already at end");
    settle().await;
    assert_eq!(result.changes.lock().len(), written);
    assert_eq!(
        id_strings(&result.registry.list().expect("list")),
        vec![second.id().to_string(), first.id().to_string()]
    );

    let error = result
        .registry
        .insert_before(&wid("missing"), None)
        .await
        .err()
        .expect("rejects unknown");
    assert!(error.contains("cannot reorder unknown workspace 'missing'"), "{error}");
    let error = result
        .registry
        .insert_before(&second.id(), Some(&wid("missing-anchor")))
        .await
        .err()
        .expect("rejects unknown anchor");
    assert!(error.contains("'missing-anchor'"), "{error}");
    settle().await;
    assert_eq!(result.changes.lock().len(), written);
}

// ---------------------------------------------------------------------------
// session ordering

#[tokio::test(flavor = "current_thread")]
async fn prepends_new_attaches_and_keeps_repeat_attach_idempotent() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("attach-order"));
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    result
        .persistence
        .set_sessions(&[header("s1", Some(&dir), 1), header("s2", Some(&dir), 2)]);
    let workspace = result.registry.create(&dir, None).await.expect("create");
    workspace.attach_session(&sid("s1")).await.expect("attach");
    workspace.attach_session(&sid("s2")).await.expect("attach");
    assert_eq!(
        session_strings(&workspace.session_ids()),
        vec!["s2".to_string(), "s1".to_string()]
    );
    workspace.attach_session(&sid("s1")).await.expect("repeat attach");
    assert_eq!(
        session_strings(&workspace.session_ids()),
        vec!["s2".to_string(), "s1".to_string()]
    );
    let stored = stored_record(&result.pool, workspace.id().as_str());
    assert_eq!(
        session_strings(&stored.session_ids),
        vec!["s2".to_string(), "s1".to_string()]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn moves_one_id_before_an_anchor_or_to_the_end_durably() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("insert-before"));
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    result.persistence.set_sessions(&[
        header("s1", Some(&dir), 1),
        header("s2", Some(&dir), 2),
        header("s3", Some(&dir), 3),
    ]);
    let workspace = result.registry.create(&dir, None).await.expect("create");
    workspace.attach_session(&sid("s1")).await.expect("attach");
    workspace.attach_session(&sid("s2")).await.expect("attach");
    workspace.attach_session(&sid("s3")).await.expect("attach");
    assert_eq!(
        session_strings(&workspace.session_ids()),
        vec!["s3".to_string(), "s2".to_string(), "s1".to_string()]
    );

    workspace
        .insert_session_before(&sid("s1"), Some(&sid("s2")))
        .await
        .expect("move");
    assert_eq!(
        session_strings(&workspace.session_ids()),
        vec!["s3".to_string(), "s1".to_string(), "s2".to_string()]
    );
    workspace
        .insert_session_before(&sid("s3"), None)
        .await
        .expect("move to end");
    assert_eq!(
        session_strings(&workspace.session_ids()),
        vec!["s1".to_string(), "s2".to_string(), "s3".to_string()]
    );
    let stored = stored_record(&result.pool, workspace.id().as_str());
    assert_eq!(
        session_strings(&stored.session_ids),
        vec!["s1".to_string(), "s2".to_string(), "s3".to_string()]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn treats_self_anchored_and_already_in_place_moves_as_no_ops_without_writing() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("insert-noop"));
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    result
        .persistence
        .set_sessions(&[header("s1", Some(&dir), 1), header("s2", Some(&dir), 2)]);
    let workspace = result.registry.create(&dir, None).await.expect("create");
    workspace.attach_session(&sid("s1")).await.expect("attach");
    workspace.attach_session(&sid("s2")).await.expect("attach");
    settle().await;
    let written = result.changes.lock().len();

    workspace
        .insert_session_before(&sid("s1"), Some(&sid("s1")))
        .await
        .expect("self anchor");
    workspace
        .insert_session_before(&sid("s2"), Some(&sid("s1")))
        .await
        .expect("already positioned");
    workspace
        .insert_session_before(&sid("s1"), None)
        .await
        .expect("already at end");
    workspace.detach_session(&sid("absent")).await.expect("absent detach");
    settle().await;
    assert_eq!(result.changes.lock().len(), written);
    assert_eq!(
        session_strings(&workspace.session_ids()),
        vec!["s2".to_string(), "s1".to_string()]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_moves_naming_an_unaccounted_session_or_anchor() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("insert-invalid"));
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    result.persistence.set_sessions(&[header("s1", Some(&dir), 1)]);
    let workspace = result.registry.create(&dir, None).await.expect("create");
    workspace.attach_session(&sid("s1")).await.expect("attach");
    settle().await;
    let written = result.changes.lock().len();

    let error = workspace
        .insert_session_before(&sid("ghost"), None)
        .await
        .err()
        .expect("rejects unaccounted");
    assert!(error.contains("the session is not accounted"), "{error}");
    let error = workspace
        .insert_session_before(&sid("s1"), Some(&sid("ghost")))
        .await
        .err()
        .expect("rejects unaccounted anchor");
    assert!(error.contains("the anchor session is not accounted"), "{error}");
    settle().await;
    assert_eq!(result.changes.lock().len(), written);
    assert_eq!(session_strings(&workspace.session_ids()), vec!["s1".to_string()]);
}

#[tokio::test(flavor = "current_thread")]
async fn validates_a_lazy_live_session_without_requiring_it_in_persistence_list() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("live"));
    let live = FakeLiveSessions::new(&[header("live", Some(&dir), 1)]);
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], Some(live)).await;
    let workspace = result.registry.create(&dir, None).await.expect("create");
    workspace.attach_session(&sid("live")).await.expect("attach");
    assert_eq!(session_strings(&workspace.session_ids()), vec!["live".to_string()]);
    assert_eq!(result.persistence.list_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_mismatched_missing_unresolved_non_directory_and_unknown_cwd_facts() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("strict"));
    let elsewhere = canonical(&temp.dir("elsewhere"));
    let gone = temp.dir("gone");
    let file = temp.path("cwd-file");
    std::fs::write(&file, "file").expect("write file");
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    result.persistence.set_sessions(&[
        header("mismatch", Some(&elsewhere), 0),
        header("no-cwd", None, 0),
        header("gone", Some(&gone), 0),
        header("file", Some(&file), 0),
    ]);
    std::fs::remove_dir_all(&gone).expect("remove gone");
    let workspace = result.registry.create(&dir, None).await.expect("create");

    let error = workspace
        .attach_session(&sid("mismatch"))
        .await
        .err()
        .expect("rejects mismatch");
    assert!(error.contains("resolves to"), "{error}");
    let error = workspace.attach_session(&sid("no-cwd")).await.err().expect("rejects no-cwd");
    assert!(error.contains("no cwd"), "{error}");
    let error = workspace.attach_session(&sid("gone")).await.err().expect("rejects gone");
    assert!(error.contains("does not resolve"), "{error}");
    let error = workspace.attach_session(&sid("file")).await.err().expect("rejects file");
    assert!(error.contains("not a directory"), "{error}");
    let error = workspace
        .attach_session(&sid("unknown"))
        .await
        .err()
        .expect("rejects unknown");
    assert!(error.contains("no such session"), "{error}");
    assert_eq!(workspace.session_ids().len(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn decides_detach_attach_membership_at_domain_write_chain_slots() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("race"));
    let result = harness(
        Arc::new(MemoryMediaPool::new()),
        &[header("s1", Some(&dir), 0)],
        None,
    )
    .await;
    let workspace = result.registry.create(&dir, None).await.expect("create");
    workspace.attach_session(&sid("s1")).await.expect("attach");
    let s1 = sid("s1");
    let detached = workspace.detach_session(&s1);
    let attached = workspace.attach_session(&s1);
    let (detached, attached) = tokio::join!(detached, attached);
    detached.expect("detach");
    attached.expect("attach");
    assert_eq!(session_strings(&workspace.session_ids()), vec!["s1".to_string()]);
}

// ---------------------------------------------------------------------------
// header-validated membership projection

#[tokio::test(flavor = "current_thread")]
async fn requires_both_candidate_id_and_matching_canonical_cwd_without_re_reading_on_list() {
    let temp = TempRoot::new();
    let owned = canonical(&temp.dir("owned"));
    let elsewhere = canonical(&temp.dir("projection-elsewhere"));
    let id = wid("00000000-0000-4000-8000-000000000001");
    let pool = stored_pool(
        &[(
            "00000000-0000-4000-8000-000000000001",
            record(&owned, &["good", "mismatch", "missing"], "2026-07-24T00:00:00.000Z"),
        )],
        initialized(vec![id.clone()]),
        false,
    );
    let result = harness(
        pool.clone(),
        &[
            header("good", Some(&owned), 0),
            header("mismatch", Some(&elsewhere), 0),
            header("cwd-only", Some(&owned), 0),
        ],
        None,
    )
    .await;
    let workspace = result.registry.list().expect("list").remove(0);
    assert_eq!(session_strings(&workspace.session_ids()), vec!["good".to_string()]);
    assert_eq!(
        session_strings(&result.registry.list().expect("list")[0].session_ids()),
        vec!["good".to_string()]
    );
    assert_eq!(result.persistence.list_calls.load(Ordering::SeqCst), 1);
    let stored = stored_record(&pool, "00000000-0000-4000-8000-000000000001");
    assert_eq!(
        session_strings(&stored.session_ids),
        vec!["good".to_string(), "mismatch".to_string(), "missing".to_string()]
    );

    workspace.set_title("pruned").await.expect("set title");
    let stored = stored_record(&pool, "00000000-0000-4000-8000-000000000001");
    assert_eq!(session_strings(&stored.session_ids), vec!["good".to_string()]);
    assert!(!workspace.session_ids().contains(&sid("cwd-only")));
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_duplicate_candidate_ownership_duplicate_paths_and_initialized_order_drift() {
    let temp = TempRoot::new();
    let first = canonical(&temp.dir("corrupt-first"));
    let second = canonical(&temp.dir("corrupt-second"));
    let first_id = wid("00000000-0000-4000-8000-000000000002");
    let second_id = wid("00000000-0000-4000-8000-000000000003");

    let duplicate_session = stored_pool(
        &[
            ("00000000-0000-4000-8000-000000000002", record(&first, &["dup"], "2026-07-24T00:00:00.000Z")),
            ("00000000-0000-4000-8000-000000000003", record(&second, &["dup"], "2026-07-24T00:00:00.000Z")),
        ],
        initialized(vec![first_id.clone(), second_id.clone()]),
        false,
    );
    let error = harness_with_backend(duplicate_session, &[], None, None)
        .await
        .err()
        .expect("rejects duplicate session");
    assert!(error.contains("accounted"), "{error}");

    let duplicate_path = stored_pool(
        &[
            ("00000000-0000-4000-8000-000000000002", record(&first, &[], "2026-07-24T00:00:00.000Z")),
            ("00000000-0000-4000-8000-000000000003", record(&first, &[], "2026-07-24T00:00:00.000Z")),
        ],
        initialized(vec![first_id.clone(), second_id.clone()]),
        false,
    );
    let error = harness_with_backend(duplicate_path, &[], None, None)
        .await
        .err()
        .expect("rejects duplicate path");
    assert!(error.contains("claimed"), "{error}");

    let orphan = stored_pool(
        &[
            ("00000000-0000-4000-8000-000000000002", record(&first, &[], "2026-07-24T00:00:00.000Z")),
            ("00000000-0000-4000-8000-000000000003", record(&second, &[], "2026-07-24T00:00:00.000Z")),
        ],
        initialized(vec![first_id.clone()]),
        false,
    );
    let error = harness_with_backend(orphan, &[], None, None)
        .await
        .err()
        .expect("rejects orphan");
    assert!(error.contains("absent from registry order"), "{error}");

    let repeated = stored_pool(
        &[(
            "00000000-0000-4000-8000-000000000002",
            record(&first, &[], "2026-07-24T00:00:00.000Z"),
        )],
        initialized(vec![first_id.clone(), first_id.clone()]),
        false,
    );
    let error = harness_with_backend(repeated, &[], None, None)
        .await
        .err()
        .expect("rejects repeated order");
    assert!(error.contains("repeats workspace"), "{error}");

    let missing = stored_pool(&[], initialized(vec![first_id.clone()]), false);
    let error = harness_with_backend(missing, &[], None, None)
        .await
        .err()
        .expect("rejects missing record");
    assert!(error.contains("references missing workspace"), "{error}");
    let _ = second_id;
}

#[tokio::test(flavor = "current_thread")]
async fn fails_list_if_the_durable_order_and_entity_cache_are_externally_diverged() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("cache-diverged"));
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    let workspace = result.registry.create(&dir, None).await.expect("create");
    result.registry.uncache(workspace.id());
    let error = result.registry.list().err().expect("list fails");
    assert!(error.contains("references missing workspace"), "{error}");
}

#[tokio::test(flavor = "current_thread")]
async fn recovers_only_an_explicitly_marked_interrupted_create_or_delete() {
    let temp = TempRoot::new();
    let create_dir = canonical(&temp.dir("pending-create"));
    let delete_dir = canonical(&temp.dir("pending-delete"));
    let create_id = wid("00000000-0000-4000-8000-000000000004");
    let delete_id = wid("00000000-0000-4000-8000-000000000005");

    let interrupted_create = stored_pool(
        &[(
            "00000000-0000-4000-8000-000000000004",
            record(&create_dir, &[], "2026-07-24T00:00:00.000Z"),
        )],
        WorkspaceDomainState {
            initialized: true,
            workspace_ids: vec![],
            archived_session_ids: vec![],
            pending_mutation: Some(WorkspacePendingMutation::Create {
                workspace_id: create_id.clone(),
            }),
        },
        false,
    );
    let create_recovery = harness(interrupted_create.clone(), &[], None).await;
    assert_eq!(create_recovery.registry.list().expect("list").len(), 0);
    {
        let media = interrupted_create.media.lock();
        let medium = media.get("workspace").expect("medium");
        assert!(!medium
            .tables
            .get("workspaces")
            .expect("table")
            .contains_key("00000000-0000-4000-8000-000000000004"));
    }
    assert_eq!(
        stored_state(&interrupted_create),
        WorkspaceDomainState {
            initialized: true,
            workspace_ids: vec![],
            archived_session_ids: vec![],
            pending_mutation: None,
        }
    );

    let interrupted_delete = stored_pool(
        &[(
            "00000000-0000-4000-8000-000000000005",
            record(&delete_dir, &[], "2026-07-24T00:00:00.000Z"),
        )],
        WorkspaceDomainState {
            initialized: true,
            workspace_ids: vec![],
            archived_session_ids: vec![],
            pending_mutation: Some(WorkspacePendingMutation::Delete {
                workspace_id: delete_id.clone(),
            }),
        },
        false,
    );
    let delete_recovery = harness(interrupted_delete.clone(), &[], None).await;
    assert_eq!(delete_recovery.registry.list().expect("list").len(), 0);
    {
        let media = interrupted_delete.media.lock();
        let medium = media.get("workspace").expect("medium");
        assert!(!medium
            .tables
            .get("workspaces")
            .expect("table")
            .contains_key("00000000-0000-4000-8000-000000000005"));
    }
    assert_eq!(
        stored_state(&interrupted_delete),
        WorkspaceDomainState {
            initialized: true,
            workspace_ids: vec![],
            archived_session_ids: vec![],
            pending_mutation: None,
        }
    );

    let corrupt_pending = stored_pool(
        &[(
            "00000000-0000-4000-8000-000000000005",
            record(&delete_dir, &[], "2026-07-24T00:00:00.000Z"),
        )],
        WorkspaceDomainState {
            initialized: true,
            workspace_ids: vec![delete_id.clone()],
            archived_session_ids: vec![],
            pending_mutation: Some(WorkspacePendingMutation::Delete {
                workspace_id: delete_id.clone(),
            }),
        },
        false,
    );
    let error = harness_with_backend(corrupt_pending, &[], None, None)
        .await
        .err()
        .expect("rejects corrupt pending");
    assert!(error.contains("still present in registry order"), "{error}");
    let _ = create_id;
}

// ---------------------------------------------------------------------------
// mutation and status

#[tokio::test(flavor = "current_thread")]
async fn keeps_created_at_stable_advances_updated_at_and_preserves_snapshot_on_write_failure() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("timestamps"));
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    let workspace = result.registry.create(&dir, None).await.expect("create");
    let created_at = workspace.created_at();
    assert_eq!(workspace.updated_at(), created_at);
    workspace.set_title("kept").await.expect("set title");
    assert_eq!(workspace.created_at(), created_at);
    assert!(
        chrono::DateTime::parse_from_rfc3339(&workspace.updated_at())
            .expect("updatedAt parses")
            >= chrono::DateTime::parse_from_rfc3339(&created_at).expect("createdAt parses")
    );
    result.pool.fail_next_writes.store(1, Ordering::SeqCst);
    let error = workspace.set_title("lost").await.err().expect("write fails");
    assert!(error.contains("injected"), "{error}");
    assert_eq!(workspace.title(), "kept");
}

#[tokio::test(flavor = "current_thread")]
async fn reports_directory_disappearance_without_mutating_the_workspace() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("vanishing"));
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    let workspace = result.registry.create(&dir, None).await.expect("create");
    assert_eq!(workspace.status().await.expect("status"), "ok");
    std::fs::remove_dir_all(&dir).expect("remove");
    assert_eq!(workspace.status().await.expect("status"), "missing-dir");
    std::fs::write(&dir, "now a file").expect("replace with file");
    assert_eq!(workspace.status().await.expect("status"), "missing-dir");
    assert_eq!(result.registry.get(workspace.id()), Some(workspace));
}

// ---------------------------------------------------------------------------
// registry-global session archive

#[tokio::test(flavor = "current_thread")]
async fn archives_durably_in_order_idempotently_skips_repeats_and_leaves_accounting_untouched() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("archive-home"));
    let result = harness(
        Arc::new(MemoryMediaPool::new()),
        &[header("kept", Some(&dir), 100), header("gone", Some(&dir), 200)],
        None,
    )
    .await;
    let workspace = result.registry.list().expect("list").remove(0);
    assert_eq!(result.registry.archived_session_ids().len(), 0);

    result.registry.archive_session(&sid("gone")).await.expect("archive");
    assert_eq!(
        session_strings(&result.registry.archived_session_ids()),
        vec!["gone".to_string()]
    );
    assert!(workspace.session_ids().contains(&sid("gone")));
    settle().await;
    assert_eq!(
        session_strings(&stored_state(&result.pool).archived_session_ids),
        vec!["gone".to_string()]
    );
    let changes_after_first = global_changes(&result);

    result.registry.archive_session(&sid("gone")).await.expect("repeat archive");
    settle().await;
    assert_eq!(
        session_strings(&result.registry.archived_session_ids()),
        vec!["gone".to_string()]
    );
    assert_eq!(global_changes(&result), changes_after_first);

    result.registry.archive_session(&sid("kept")).await.expect("archive kept");
    assert_eq!(
        session_strings(&result.registry.archived_session_ids()),
        vec!["gone".to_string(), "kept".to_string()]
    );

    result.registry.unarchive_session(&sid("gone")).await.expect("unarchive");
    assert_eq!(
        session_strings(&result.registry.archived_session_ids()),
        vec!["kept".to_string()]
    );
    assert!(workspace.session_ids().contains(&sid("gone")));
    settle().await;
    assert_eq!(
        session_strings(&stored_state(&result.pool).archived_session_ids),
        vec!["kept".to_string()]
    );
    let changes_after_restore = global_changes(&result);

    result.registry.unarchive_session(&sid("gone")).await.expect("repeat unarchive");
    settle().await;
    assert_eq!(
        session_strings(&result.registry.archived_session_ids()),
        vec!["kept".to_string()]
    );
    assert_eq!(global_changes(&result), changes_after_restore);
}

#[tokio::test(flavor = "current_thread")]
async fn accepts_unaccounted_and_live_sessions_but_rejects_unknown_ids_without_writing() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("archive-strays"));
    let live_dir = canonical(&temp.dir("archive-live"));
    let live = FakeLiveSessions::new(&[header("live-only", Some(&live_dir), 200)]);
    let result = harness(
        Arc::new(MemoryMediaPool::new()),
        &[header("stray", Some(&dir), 100)],
        Some(live),
    )
    .await;
    result.registry.archive_session(&sid("stray")).await.expect("archive");
    result.registry.archive_session(&sid("live-only")).await.expect("archive");
    assert_eq!(
        session_strings(&result.registry.archived_session_ids()),
        vec!["stray".to_string(), "live-only".to_string()]
    );

    let error = result
        .registry
        .archive_session(&sid("ghost"))
        .await
        .err()
        .expect("rejects unknown");
    assert!(error.contains("cannot archive session 'ghost'"), "{error}");
    assert_eq!(
        session_strings(&stored_state(&result.pool).archived_session_ids),
        vec!["stray".to_string(), "live-only".to_string()]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn permanently_deletes_only_archived_cold_sessions_and_clears_every_account() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("archive-delete"));
    let result = harness(
        Arc::new(MemoryMediaPool::new()),
        &[header("deleted", Some(&dir), 100), header("kept", Some(&dir), 200)],
        None,
    )
    .await;
    let workspace = result.registry.list().expect("list").remove(0);

    let error = result
        .registry
        .delete_archived_session(&sid("deleted"), None)
        .await
        .err()
        .expect("rejects unarchived");
    assert!(error.contains("it is not archived"), "{error}");
    assert!(result.persistence.delete_calls.lock().is_empty());
    result.registry.archive_session(&sid("deleted")).await.expect("archive");
    assert_eq!(
        result
            .registry
            .delete_archived_session(&sid("deleted"), None)
            .await
            .expect("delete"),
        true
    );
    settle().await;

    assert_eq!(
        session_strings(&result.persistence.delete_calls.lock()),
        vec!["deleted".to_string()]
    );
    assert_eq!(result.registry.archived_session_ids().len(), 0);
    assert_eq!(
        session_strings(&workspace.session_ids()),
        vec!["kept".to_string()]
    );
    let stored = stored_record(&result.pool, workspace.id().as_str());
    assert_eq!(session_strings(&stored.session_ids), vec!["kept".to_string()]);
    assert_eq!(
        session_strings(&result.deleted_sessions.lock()),
        vec!["deleted".to_string()]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn refuses_an_unowned_live_deletion_but_accepts_an_explicit_lifecycle_release() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("archive-delete-live"));
    let live = FakeLiveSessions::new(&[header("live-delete", Some(&dir), 100)]);
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], Some(live.clone())).await;
    let session_id = sid("live-delete");
    result.registry.archive_session(&session_id).await.expect("archive");

    let error = result
        .registry
        .delete_archived_session(&session_id, None)
        .await
        .err()
        .expect("rejects live");
    assert!(error.contains("while it is live"), "{error}");
    assert!(result.persistence.delete_calls.lock().is_empty());
    assert_eq!(
        session_strings(&result.registry.archived_session_ids()),
        vec!["live-delete".to_string()]
    );

    let release: Arc<dyn Fn() -> futures::future::BoxFuture<'static, ()> + Send + Sync> =
        Arc::new({
            let live = live.clone();
            let session_id = session_id.clone();
            move || {
                let live = live.clone();
                let session_id = session_id.clone();
                Box::pin(async move {
                    live.remove(&session_id);
                })
            }
        });
    assert_eq!(
        result
            .registry
            .delete_archived_session(&session_id, Some(release))
            .await
            .expect("delete with release"),
        true
    );
    assert!(live.get(&session_id).is_none());
    assert_eq!(result.registry.archived_session_ids().len(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn propagates_a_persistence_listing_failure_instead_of_reporting_an_unknown_session() {
    let result = harness(Arc::new(MemoryMediaPool::new()), &[], None).await;
    result.persistence.set_list_error("persistence backend down");
    let error = result
        .registry
        .archive_session(&sid("unlisted"))
        .await
        .err()
        .expect("rejects");
    assert!(error.contains("persistence backend down"), "{error}");
    assert_eq!(stored_state(&result.pool).archived_session_ids.len(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn restores_the_archive_set_across_restarts_and_defaults_it_for_pre_field_media() {
    let temp = TempRoot::new();
    let dir = canonical(&temp.dir("archive-restart"));
    let pool = Arc::new(MemoryMediaPool::new());
    let first = harness(pool.clone(), &[header("s1", Some(&dir), 100)], None).await;
    first.registry.archive_session(&sid("s1")).await.expect("archive");
    first.registry.domain().close().await;

    let second = harness(pool.clone(), &[header("s1", Some(&dir), 100)], None).await;
    assert_eq!(
        session_strings(&second.registry.archived_session_ids()),
        vec!["s1".to_string()]
    );
    second.registry.domain().close().await;

    let legacy_id = wid("00000000-0000-4000-8000-00000000000a");
    let legacy = stored_pool(
        &[(
            "00000000-0000-4000-8000-00000000000a",
            record(&dir, &[], "2026-07-24T00:00:00.000Z"),
        )],
        initialized(vec![legacy_id]),
        true,
    );
    let upgraded = harness(legacy, &[], None).await;
    assert_eq!(upgraded.registry.archived_session_ids().len(), 0);
}
