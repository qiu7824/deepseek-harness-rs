use super::*;
fn target(revision: &str) -> ComputerTargetIdentity {
    ComputerTargetIdentity {
        host_id: "local".into(),
        device_id: "fixture-device".into(),
        application_id: "fixture-app".into(),
        application_revision: revision.into(),
        origin: Some("https://fixture.invalid".into()),
        target_revision: "window-1".into(),
        label: "Fixture Application".into(),
    }
}
fn request(owner: &str, revision: &str) -> ComputerPermissionRequest {
    ComputerPermissionRequest {
        owner_id: owner.into(),
        target: target(revision),
        action: "capture".into(),
        scopes: vec!["screen_read".into()],
        signal: Arc::new(|| false),
    }
}
fn root() -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("computer-permission-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    root
}
fn grant(service: &ComputerPermissions, scope: &str, owner: &str) -> Value {
    let snapshot = service.snapshot();
    service.mutate(&json!({"action":"grant","targetId":snapshot["targets"][0]["id"],"ownerSessionId":owner,"scope":scope,"scopes":["screen_read"],"expectedRevision":snapshot["revision"]})).unwrap()
}
async fn dispose(ctx: &Context) {
    for disposer in ctx.fiber.disposables.clear() {
        disposer().await;
    }
}
fn remove(root: &std::path::Path) {
    assert!(
        root.canonicalize()
            .unwrap()
            .starts_with(std::env::temp_dir().canonicalize().unwrap())
    );
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn once_session_persistent_revocation_and_identity_change_preserve_boundaries() {
    let root = root();
    let ctx = Context::root();
    let service = ComputerPermissions::install(&ctx, root.clone()).unwrap();
    assert!(service.authorize(request("owner-a", "v1")).await.is_err());
    grant(&service, "once", "owner-a");
    let lease = service.authorize(request("owner-a", "v1")).await.unwrap();
    assert_eq!(lease.target, target("v1"));
    assert!((lease.valid)());
    assert!(service.authorize(request("owner-a", "v1")).await.is_err());
    assert!(service.authorize(request("owner-b", "v1")).await.is_err());
    let snapshot = service.snapshot();
    assert!(service.mutate(&json!({"action":"grant","targetId":snapshot["targets"][0]["id"],"ownerSessionId":"owner-a","scope":"session","scopes":["screen_read"],"expectedRevision":snapshot["revision"]})).is_err());
    let granted = grant(&service, "session", "owner-b");
    let id = granted["grants"].as_array().unwrap().last().unwrap()["id"].clone();
    let active = service.authorize(request("owner-b", "v1")).await.unwrap();
    assert!(service.authorize(request("owner-b", "v1")).await.is_ok());
    assert!(service.authorize(request("owner-a", "v1")).await.is_err());
    let snapshot = service.snapshot();
    service
        .mutate(&json!({"action":"revoke","grantId":id,"expectedRevision":snapshot["revision"]}))
        .unwrap();
    assert!(!(active.valid)());
    assert!(
        (lease.valid)(),
        "revoking another task's grant must not cancel this once-authorized action"
    );
    let snapshot = service.snapshot();
    let once_id = snapshot["grants"][0]["id"].clone();
    service
        .mutate(
            &json!({"action":"revoke","grantId":once_id,"expectedRevision":snapshot["revision"]}),
        )
        .unwrap();
    assert!(!(lease.valid)());
    assert!(service.authorize(request("owner-a", "v1")).await.is_err());
    grant(&service, "persistent", "owner-a");
    let other = service.authorize(request("owner-b", "v1")).await.unwrap();
    assert!((other.valid)());
    let second = Context::root();
    let restored = ComputerPermissions::install(&second, root.clone()).unwrap();
    assert!(restored.authorize(request("owner-c", "v1")).await.is_ok());
    assert!(restored.authorize(request("owner-c", "v2")).await.is_err());
    dispose(&ctx).await;
    assert!(!(other.valid)());
    dispose(&second).await;
    drop(service);
    drop(restored);
    remove(&root);
}
#[tokio::test]
async fn corrupt_storage_is_closed_and_recovery_retains_bad_bytes() {
    let root = root();
    let path = root.join("computer-permissions.json");
    save(&path, &empty_saved()).unwrap();
    std::fs::write(&path, b"{corrupt").unwrap();
    let ctx = Context::root();
    let service = ComputerPermissions::install(&ctx, root.clone()).unwrap();
    assert!(service.snapshot()["storageError"].is_string());
    assert_eq!(service.snapshot()["canRestore"], true);
    assert_eq!(std::fs::read(&path).unwrap(), b"{corrupt");
    assert_eq!(
        service
            .authorize(request("owner", "v1"))
            .await
            .err()
            .unwrap()
            .code,
        "COMPUTER_USE_PERMISSION_STORAGE"
    );
    let restored = service
        .mutate(&json!({"action":"recover","strategy":"last-good","expectedRevision":0}))
        .unwrap();
    assert!(restored["storageError"].is_null());
    assert!(load(&path).is_ok());
    let backup = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("computer-permissions.invalid-")
        })
        .unwrap();
    assert_eq!(std::fs::read(backup).unwrap(), b"{corrupt");
    dispose(&ctx).await;
    drop(service);
    remove(&root);
}
#[tokio::test]
async fn external_edits_invalidate_leases_and_observed_catalog_stays_bounded() {
    let root = root();
    let ctx = Context::root();
    let service = ComputerPermissions::install(&ctx, root.clone()).unwrap();
    assert!(service.authorize(request("owner", "v1")).await.is_err());
    grant(&service, "persistent", "owner");
    let lease = service.authorize(request("owner", "v1")).await.unwrap();
    std::fs::write(root.join("computer-permissions.json"), b"changed").unwrap();
    assert!(service.authorize(request("owner", "v1")).await.is_err());
    assert!(!(lease.valid)());
    let snapshot = service.snapshot();
    service
        .mutate(
            &json!({"action":"recover","strategy":"reset","expectedRevision":snapshot["revision"]}),
        )
        .unwrap();
    for n in 0..600 {
        let _ = service.authorize(request("owner", &format!("v{n}"))).await;
    }
    assert!(service.snapshot()["targets"].as_array().unwrap().len() <= 128);
    assert!(service.state.lock().generations.len() <= 257);
    dispose(&ctx).await;
    drop(service);
    remove(&root);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn composed_host_mounts_permissions_even_when_storage_is_broken() {
    let root = root();
    std::fs::write(root.join("computer-permissions.json"), b"{bad").unwrap();
    let ctx = Context::root();
    let host = crate::compose_persistent_host_at(&ctx, &root, None).unwrap();
    assert!(
        ctx.get_typed::<Arc<dyn ComputerPermissionService>>("computerPermissions", false)
            .is_some()
    );
    assert_eq!(
        std::fs::read(root.join("computer-permissions.json")).unwrap(),
        b"{bad"
    );
    host.shutdown().await.unwrap();
    drop(host);
    drop(ctx);
    remove(&root);
}
use std::sync::atomic::AtomicUsize;
struct ControlledAdapter {entered:Arc<tokio::sync::Notify>,effects:Arc<AtomicUsize>}
#[async_trait::async_trait]
impl dsh_tool_computer_use_command::ComputerUseAdapter for ControlledAdapter {
    fn adapter_id(&self)->&'static str{"permission-fixture"}
    async fn permission_identity(&self,_:&dsh_tool_computer_use_command::AdapterRequest,_:AbortPredicate)->Result<ComputerTargetIdentity,AdapterError>{Ok(target("v1"))}
    async fn execute(&self,request:dsh_tool_computer_use_command::AdapterRequest,signal:AbortPredicate)->Result<dsh_tool_computer_use_command::AdapterOutput,AdapterError>{
        assert_eq!(request.origin,dsh_tool_computer_use_command::ControlOrigin::Agent);assert_eq!(request.permission_target,Some(target("v1")));self.entered.notify_one();
        for _ in 0..100 {if signal(){return Err(AdapterError::cancelled());}tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
        self.effects.fetch_add(1,Ordering::SeqCst);Err(error("FIXTURE_EFFECT","unexpected uncancelled fixture"))
    }
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn composed_adapter_refuses_forged_authority_and_cancels_revoked_pending_action() {
    let root=root();let ctx=Context::root();dsh_system_prompt::SystemPrompt::install(&ctx,Default::default()).unwrap();dsh_tools::ToolRuntime::install(&ctx,Default::default()).unwrap();
    let service=ComputerPermissions::install(&ctx,root.clone()).unwrap();let entered=Arc::new(tokio::sync::Notify::new());let effects=Arc::new(AtomicUsize::new(0));
    let runtime=dsh_tool_computer_use_command::install_adapter(&ctx,5000,Arc::new(ControlledAdapter {entered:entered.clone(),effects:effects.clone()})).unwrap();
    let args=json!({"action":"capture","sessionId":"fixture","origin":"human","ownerId":"other-owner","permissionTarget":target("forged")});
    assert!(runtime.execute("owner",&args,Arc::new(||false)).await.is_err());assert_eq!(effects.load(Ordering::SeqCst),0);let granted=grant(&service,"session","owner");let grant_id=granted["grants"][0]["id"].clone();
    let request_runtime=runtime.clone();let pending=tokio::spawn(async move {request_runtime.execute("owner",&args,Arc::new(||false)).await});
    tokio::time::timeout(std::time::Duration::from_secs(2),entered.notified()).await.unwrap();let snapshot=service.snapshot();service.mutate(&json!({"action":"revoke","grantId":grant_id,"expectedRevision":snapshot["revision"]})).unwrap();
    assert!(tokio::time::timeout(std::time::Duration::from_secs(2),pending).await.unwrap().unwrap().is_err());assert_eq!(effects.load(Ordering::SeqCst),0);
    runtime.shutdown().await.unwrap();dispose(&ctx).await;drop(runtime);drop(service);remove(&root);
}
