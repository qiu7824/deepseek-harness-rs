use cordis::Context;
use dsh_session::{Session, SessionHeader, SessionStore, CreateSessionMeta, CreateSessionOptions, session_id};
use dsh_session_persistence::SessionPersistenceApi;
use dsh_session_persistence_jsonl::{JsonlConfig, JsonlCompression, JsonlSessionPersistence, log_path};
use std::{path::{Path, PathBuf}, sync::Arc};

fn root() -> PathBuf {
    std::env::temp_dir().join(format!("session-writer-runtime-{}", uuid::Uuid::new_v4()))
}
async fn shutdown(ctx: &Context) {
    // Root fibers are permanent; drain their owned effects as plugin unload
    // does, rather than calling the child-fiber-only dispose entry point.
    for disposer in ctx.fiber.disposables.clear() { disposer().await; }
}
fn backend(root: &Path) -> (Context, Arc<SessionStore>, Arc<JsonlSessionPersistence>) {
    let ctx = Context::root(); let sessions = SessionStore::install(&ctx);
    let persistence = JsonlSessionPersistence::install(&ctx, JsonlConfig {
        root: root.to_string_lossy().into_owned(), ..Default::default()
    }).unwrap();
    (ctx, sessions, persistence)
}
fn session(store: &SessionStore, id: &str, cwd: Option<String>) -> Session {
    let session = store.prepare(Some(session_id(id)), Some(CreateSessionOptions {
        meta: Some(CreateSessionMeta { cwd, ..Default::default() }), ..Default::default()
    })).unwrap();
    session.append("feedback/record", serde_json::json!({"text":"ownership fixture"}), None).unwrap();
    session
}
fn artifact(root: &Path, header: &SessionHeader) -> PathBuf {
    log_path(&root.to_string_lossy(), header.cwd.as_deref(), &header.id, JsonlCompression::Zstd)
}

#[tokio::test]
async fn unpublished_creation_excludes_same_id_across_projects_and_releases_on_drop() {
    let root = root(); let (a_ctx, a_store, a) = backend(&root); let (b_ctx, b_store, b) = backend(&root);
    let original = session(&a_store, "shared-id", None);
    let first = a.prepare_new(original.clone()).await.unwrap();
    assert!(!artifact(&root, original.header()).exists());
    let second = session(&b_store, "shared-id", Some(root.join("other-project").to_string_lossy().into_owned()));
    assert!(b.prepare_new(second.clone()).await.err().unwrap().contains("SESSION_IN_USE"));
    drop(first);
    let next = b.prepare_new(second).await.unwrap(); drop(next);
    assert!(a.list().await.unwrap().is_empty()); assert!(b.list().await.unwrap().is_empty());
    shutdown(&a_ctx).await; shutdown(&b_ctx).await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn writer_ownership_survives_publication_and_readers_do_not_repair_the_live_log() {
    let root = root(); let (a_ctx, a_store, a) = backend(&root); let (b_ctx, _, b) = backend(&root);
    let session = session(&a_store, "resident", None);
    let mut admission = a.prepare_new(session.clone()).await.unwrap();
    let detach = a_store.enter(&session).unwrap(); a_store.announce(&session).await.unwrap();
    admission.dispose();
    session.append("feedback/record", serde_json::json!({"text":"still owned"}), None).unwrap();
    a_store.flush(&session).await.unwrap();
    let path = artifact(&root, session.header()); let before = std::fs::read(&path).unwrap();
    let inspected = b.inspect(session.id()).await.unwrap();
    assert_eq!(inspected.meta.id, *session.id());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(b.prepare(session.id()).await.err().unwrap().contains("SESSION_IN_USE"));
    assert!(b.delete(session.id()).await.unwrap_err().contains("SESSION_IN_USE"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    detach().await;
    a.inspect(session.id()).await.unwrap(); // Wait for the former owner's flush/retirement.
    let resumed = b.prepare(session.id()).await.unwrap(); drop(resumed);
    shutdown(&a_ctx).await; shutdown(&b_ctx).await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn abandoned_restore_and_committed_read_release_writer_ownership() {
    let root = root(); let (seed_ctx, seed_store, seed) = backend(&root);
    let session = session(&seed_store, "cold", None);
    seed.create(session.header().clone(), None).await.unwrap(); seed.append(session.id(), &session.events()).await.unwrap();
    shutdown(&seed_ctx).await;
    let (a_ctx, _, a) = backend(&root); let (b_ctx, _, b) = backend(&root);
    let prepared = a.prepare(session.id()).await.unwrap();
    assert!(b.prepare(session.id()).await.err().unwrap().contains("SESSION_IN_USE"));
    drop(prepared);
    b.load(session.id()).await.unwrap();
    let after_read = a.prepare(session.id()).await.unwrap(); drop(after_read);
    shutdown(&a_ctx).await; shutdown(&b_ctx).await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn deleting_and_recreating_a_session_reuses_its_permanent_ownership_files() {
    let root = root(); let (ctx, store, backend) = backend(&root);
    let session = session(&store, "delete-and-recreate", None);
    backend.create(session.header().clone(), None).await.unwrap(); backend.append(session.id(), &session.events()).await.unwrap();
    let path = artifact(&root, session.header());
    assert!(backend.delete(session.id()).await.unwrap()); assert!(!path.exists());
    assert!(path.parent().unwrap().join(".session-writer.lock").is_file());
    assert!(backend.list().await.unwrap().is_empty());
    backend.create(session.header().clone(), None).await.unwrap(); backend.append(session.id(), &session.events()).await.unwrap();
    assert!(path.is_file());
    shutdown(&ctx).await;
    assert!(backend.append(session.id(), &[]).await.is_ok()); // Empty append has no write effect.
    assert!(backend.prepare_new(session.clone()).await.err().unwrap().contains("closed"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn an_unknown_new_generation_refuses_cached_resume_and_old_artifact_reads() {
    let root = root(); let (seed_ctx, store, seed) = backend(&root);
    let session = session(&store, "generation-authority", None);
    seed.create(session.header().clone(), None).await.unwrap(); seed.append(session.id(), &session.events()).await.unwrap();
    shutdown(&seed_ctx).await;
    let (ctx, _, backend) = backend(&root); backend.inspect(session.id()).await.unwrap();
    let path = artifact(&root, session.header()); let old = std::fs::read(&path).unwrap();
    let lease = dsh_session_persistence_jsonl::generations::SessionGenerationLease::acquire(path.parent().unwrap()).unwrap();
    let next = path.parent().unwrap().join("session.v5.jsonl");
    let mut future = serde_json::to_value(session.header()).unwrap();
    future["type"] = serde_json::json!("session"); future["version"] = serde_json::json!(5);
    std::fs::write(&next, format!("{future}\n")).unwrap();
    drop(lease);
    assert!(backend.prepare(session.id()).await.is_err());
    assert!(backend.read_raw(session.id()).await.is_err());
    assert!(backend.list().await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), old); assert!(next.is_file());
    shutdown(&ctx).await; std::fs::remove_dir_all(root).unwrap();
}

struct Child(std::process::Child);
impl Drop for Child { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }

#[tokio::test]
async fn an_external_runtime_owner_blocks_writes_and_process_exit_releases_admission() {
    let root = root(); std::fs::create_dir_all(&root).unwrap(); let ready = root.join("owner-ready");
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", "private_runtime_owner", "--ignored", "--nocapture"])
        .env("DSH_WRITER_RUNTIME_ROOT", &root).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
    let mut child = Child(command.spawn().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !ready.exists() && std::time::Instant::now() < deadline {
        assert!(child.0.try_wait().unwrap().is_none(), "owner exited before acquiring runtime ownership");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(ready.exists());
    let (ctx, _, backend) = backend(&root); let id = session_id("external-owner");
    let inspected = backend.inspect(&id).await.unwrap(); let path = artifact(&root, &inspected.meta);
    let before = std::fs::read(&path).unwrap();
    assert!(backend.prepare(&id).await.err().unwrap().contains("SESSION_IN_USE"));
    assert!(backend.delete(&id).await.unwrap_err().contains("SESSION_IN_USE"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    child.0.kill().unwrap(); child.0.wait().unwrap();
    let acquired = backend.prepare(&id).await.unwrap(); drop(acquired);
    assert!(backend.delete(&id).await.unwrap());
    shutdown(&ctx).await; std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
#[ignore = "private child process entry for real runtime ownership"]
async fn private_runtime_owner() {
    let Some(root) = std::env::var_os("DSH_WRITER_RUNTIME_ROOT") else { return };
    let root = PathBuf::from(root); let (_ctx, store, backend) = backend(&root);
    let session = session(&store, "external-owner", None);
    backend.create(session.header().clone(), None).await.unwrap(); backend.append(session.id(), &session.events()).await.unwrap();
    std::fs::write(root.join("owner-ready"), b"owned").unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
}
