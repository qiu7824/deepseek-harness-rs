use super::*;

fn fixture() -> PathBuf {
    let root = std::env::temp_dir().join(format!("skill-lifecycle-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    root
}
async fn create(store: &SkillLifecycle, root: &Path, revision: u64, body: &str) -> SkillRevision {
    store
        .create(
            revision,
            "manual-fixture",
            "Project procedure",
            body,
            &root.to_string_lossy(),
            "",
            vec![],
        )
        .await
        .unwrap()
}
fn options(root: &Path) -> SkillLookupOptions {
    SkillLookupOptions {
        cwd: Some(root.to_string_lossy().into_owned()),
        signal: None,
        session_id: None,
    }
}

#[tokio::test]
async fn skill_lifecycle_manual_activation_is_independent_and_project_scoped() {
    let root = fixture();
    let other = fixture();
    let store = SkillLifecycle::open(&root).await.unwrap();
    let item = create(
        &store,
        &root,
        0,
        "Read the project configuration before editing.",
    )
    .await;
    let opts = options(&root);
    assert!(
        SkillProvider::list(store.as_ref(), &opts)
            .await
            .unwrap()
            .candidates
            .is_empty()
    );
    store.activate(1, &item.id).await.unwrap();
    let listed = SkillProvider::list(store.as_ref(), &opts).await.unwrap();
    assert_eq!(listed.candidates.len(), 1);
    assert_eq!(listed.candidates[0].source, "手动项目技能");
    assert_eq!(
        listed.candidates[0].metadata.as_ref().unwrap()["activationMode"],
        "manual"
    );
    assert_eq!(
        SkillProvider::get(store.as_ref(), &listed.candidates[0], &opts)
            .await
            .unwrap()
            .unwrap()
            .content,
        item.content
    );
    assert!(
        SkillProvider::list(store.as_ref(), &options(&other))
            .await
            .unwrap()
            .candidates
            .is_empty()
    );
    let cancelled = SkillLookupOptions {
        signal: Some(Arc::new(|| true)),
        ..opts
    };
    assert!(
        SkillProvider::list(store.as_ref(), &cancelled)
            .await
            .unwrap()
            .candidates
            .is_empty()
    );
    let public = store.list().await;
    assert_eq!(public["candidates"][0]["activationMode"], "manual");
    for key in ["validation", "validated", "samples"] {
        assert!(public["candidates"][0].get(key).is_none());
    }
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(other).unwrap();
}

#[tokio::test]
async fn skill_lifecycle_versions_withdraw_restore_and_disabled_state_survive_restart() {
    let root = fixture();
    let store = SkillLifecycle::open(&root).await.unwrap();
    let first = create(&store, &root, 0, "First immutable version.").await;
    store.activate(1, &first.id).await.unwrap();
    let second = create(&store, &root, 2, "Second immutable version.").await;
    assert_ne!(first.content_hash, second.content_hash);
    let opts = options(&root);
    let handle = SkillProvider::list(store.as_ref(), &opts)
        .await
        .unwrap()
        .candidates
        .remove(0);
    assert!(store.withdraw(2, &first.id).await.is_err());
    assert!(store.remove(3, &first.id).await.is_err());
    store.withdraw(3, &first.id).await.unwrap();
    assert!(
        SkillProvider::get(store.as_ref(), &handle, &opts)
            .await
            .unwrap()
            .is_none()
    );
    drop(store);
    let reopened = SkillLifecycle::open(&root).await.unwrap();
    assert!(reopened.get(&first.id).await.unwrap().withdrawn);
    assert_eq!(
        reopened.get(&second.id).await.unwrap().content,
        second.content
    );
    assert!(reopened.activate(4, &first.id).await.is_err());
    reopened.restore(4, &first.id).await.unwrap();
    assert!(!reopened.get(&first.id).await.unwrap().withdrawn);
    reopened.activate(5, &second.id).await.unwrap();
    assert!(
        SkillProvider::get(reopened.as_ref(), &handle, &opts)
            .await
            .unwrap()
            .is_none()
    );
    reopened.set_enabled(6, false).await.unwrap();
    drop(reopened);
    let reopened = SkillLifecycle::open(&root).await.unwrap();
    assert_eq!(reopened.list().await["enabled"], false);
    assert!(
        SkillProvider::list(reopened.as_ref(), &opts)
            .await
            .unwrap()
            .candidates
            .is_empty()
    );
    assert!(reopened.restore(7, &first.id).await.is_err());
    assert!(
        reopened
            .create(
                7,
                "another",
                "Procedure",
                "Content",
                &root.to_string_lossy(),
                "",
                vec![]
            )
            .await
            .is_err()
    );
    reopened.set_enabled(7, true).await.unwrap();
    assert_eq!(
        SkillProvider::list(reopened.as_ref(), &opts)
            .await
            .unwrap()
            .candidates
            .len(),
        1
    );
    reopened.restore(8, &first.id).await.unwrap();
    reopened.remove(9, &second.id).await.unwrap();
    assert!(reopened.get(&second.id).await.is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn skill_lifecycle_legacy_fields_are_ignored_and_old_active_versions_are_manual() {
    let root = fixture();
    let store = SkillLifecycle::open(&root).await.unwrap();
    let item = create(&store, &root, 0, "Retained procedure.").await;
    store.activate(1, &item.id).await.unwrap();
    drop(store);
    let path = root.join("skill-revisions-v1.json");
    let mut data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    data["revisions"][0]["samples"] = serde_json::json!({"legacy":"unavailable task database"});
    data["revisions"][0]["validation"] =
        serde_json::json!({"retiredField":"no verifier is installed"});
    data["revisions"][0]
        .as_object_mut()
        .unwrap()
        .remove("ownerSessionId");
    data["revisions"][0]
        .as_object_mut()
        .unwrap()
        .remove("sourceEvidence");
    std::fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
    let reopened = SkillLifecycle::open(&root).await.unwrap();
    let listed = SkillProvider::list(reopened.as_ref(), &options(&root))
        .await
        .unwrap();
    assert_eq!(listed.candidates.len(), 1);
    assert_eq!(listed.candidates[0].provider, "manual-skill-revisions");
    let record = serde_json::to_value(reopened.get(&item.id).await.unwrap()).unwrap();
    assert!(record.get("samples").is_none());
    assert!(record.get("validation").is_none());
    reopened.set_enabled(2, false).await.unwrap();
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(written["revisions"][0].get("samples").is_none());
    assert!(written["revisions"][0].get("validation").is_none());
    assert_eq!(written["enabled"], false);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn skill_lifecycle_content_integrity_and_project_identity_are_preserved() {
    let root = fixture();
    let project = root.join("project");
    std::fs::create_dir(&project).unwrap();
    let store = SkillLifecycle::open(&root).await.unwrap();
    let item = create(&store, &project, 0, "Original.").await;
    std::fs::remove_dir(&project).unwrap();
    assert!(store.activate(1, &item.id).await.is_err());
    assert!(
        store
            .create(
                1,
                "relative",
                "Description",
                "Body",
                "relative-project",
                "",
                vec![]
            )
            .await
            .is_err()
    );
    drop(store);
    let path = root.join("skill-revisions-v1.json");
    let mut data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    data["revisions"][0]["content"] = "Different bytes with the old hash".into();
    std::fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
    assert!(SkillLifecycle::open(&root).await.is_err());
    std::fs::remove_dir_all(root).unwrap();
}
