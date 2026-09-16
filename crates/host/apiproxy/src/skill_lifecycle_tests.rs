use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

struct Verifier {
    stale: AtomicBool,
}
#[async_trait]
impl SkillEvidenceVerifier for Verifier {
    async fn verify(
        &self,
        owner: &str,
        subject: &str,
        samples: &[SampleRef],
    ) -> Result<ValidationEvidence, String> {
        if owner != "owner" {
            return Err("owner mismatch".into());
        }
        Ok(ValidationEvidence {
            subject_identity: subject.into(),
            environment_fingerprint: "env1".into(),
            project: samples[0].task_id.clone(),
            checker_version: "test-1".into(),
            evidence_refs: vec!["immutable-result".into()],
            positive_samples: samples.iter().filter(|s| s.expected_success).count(),
            negative_samples: samples.iter().filter(|s| !s.expected_success).count(),
            all_matched: true,
        })
    }
    async fn environment_fingerprint(&self, _: &str, _: Option<&str>) -> Result<String, String> {
        Ok(if self.stale.load(Ordering::SeqCst) {
            "env2"
        } else {
            "env1"
        }
        .into())
    }
}
fn fixture() -> PathBuf {
    let root = std::env::temp_dir().join(format!("skill-lifecycle-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    root
}
fn samples(project: &str) -> Vec<SampleRef> {
    vec![
        SampleRef {
            task_id: project.into(),
            revision: 1,
            expected_success: true,
        },
        SampleRef {
            task_id: "negative".into(),
            revision: 1,
            expected_success: false,
        },
    ]
}
async fn create(store: &SkillLifecycle, root: &Path, revision: u64, body: &str) -> SkillRevision {
    store
        .create(
            revision,
            "safe-fixture",
            "Fixture skill",
            body,
            &root.to_string_lossy(),
            "owner",
            vec!["observed-failure".into()],
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn candidates_require_trusted_positive_and_negative_evidence_and_remain_scoped() {
    let root = fixture();
    let store = SkillLifecycle::open(&root).await.unwrap();
    let item = create(&store, &root, 0, "Inspect only the project fixture.").await;
    let opts = SkillLookupOptions {
        cwd: Some(root.to_string_lossy().into_owned()),
        signal: None,
        session_id: Some("owner".into()),
    };
    assert!(
        SkillProvider::list(store.as_ref(), &opts)
            .await
            .unwrap()
            .candidates
            .is_empty()
    );
    assert!(store.activate(1, &item.id).await.is_err());
    assert!(
        store
            .validate(1, &item.id, samples(&item.project))
            .await
            .is_err()
    );
    let verifier = Arc::new(Verifier {
        stale: AtomicBool::new(false),
    });
    store.set_verifier(verifier.clone());
    assert!(
        store
            .validate(1, &item.id, vec![samples(&item.project)[0].clone()])
            .await
            .is_err()
    );
    store
        .validate(1, &item.id, samples(&item.project))
        .await
        .unwrap();
    store.activate(2, &item.id).await.unwrap();
    let listed = SkillProvider::list(store.as_ref(), &opts).await.unwrap();
    assert_eq!(listed.candidates.len(), 1);
    assert!(
        SkillProvider::get(store.as_ref(), &listed.candidates[0], &opts)
            .await
            .unwrap()
            .is_some()
    );
    let other = fixture();
    let other_opts = SkillLookupOptions {
        cwd: Some(other.to_string_lossy().into_owned()),
        signal: None,
        session_id: Some("owner".into()),
    };
    assert!(
        SkillProvider::list(store.as_ref(), &other_opts)
            .await
            .unwrap()
            .candidates
            .is_empty()
    );
    verifier.stale.store(true, Ordering::SeqCst);
    assert!(
        SkillProvider::get(store.as_ref(), &listed.candidates[0], &opts)
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.activate(3, &item.id).await.is_err());
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(other).unwrap();
}

#[tokio::test]
async fn versions_survive_restart_and_withdrawal_invalidates_loaded_handles() {
    let root = fixture();
    let store = SkillLifecycle::open(&root).await.unwrap();
    let first = create(&store, &root, 0, "First version.").await;
    store.set_verifier(Arc::new(Verifier {
        stale: AtomicBool::new(false),
    }));
    store
        .validate(1, &first.id, samples(&first.project))
        .await
        .unwrap();
    store.activate(2, &first.id).await.unwrap();
    let second = create(&store, &root, 3, "Second version.").await;
    assert!(store.withdraw(3, &first.id).await.is_err());
    let opts = SkillLookupOptions {
        cwd: Some(root.to_string_lossy().into_owned()),
        signal: None,
        session_id: Some("owner".into()),
    };
    let handle = SkillProvider::list(store.as_ref(), &opts)
        .await
        .unwrap()
        .candidates
        .remove(0);
    store.withdraw(4, &first.id).await.unwrap();
    assert!(
        SkillProvider::get(store.as_ref(), &handle, &opts)
            .await
            .unwrap()
            .is_none()
    );
    drop(store);
    let restored = SkillLifecycle::open(&root).await.unwrap();
    assert!(restored.get(&first.id).await.unwrap().withdrawn);
    assert_eq!(
        restored.get(&second.id).await.unwrap().content,
        "Second version."
    );
    assert!(restored.activate(5, &first.id).await.is_err());
    restored.remove(5, &second.id).await.unwrap();
    assert!(restored.get(&second.id).await.is_err());
    restored.set_verifier(Arc::new(Verifier {
        stale: AtomicBool::new(false),
    }));
    restored.restore(6, &first.id).await.unwrap();
    assert!(!restored.get(&first.id).await.unwrap().withdrawn);
    assert_eq!(
        SkillProvider::list(restored.as_ref(), &opts)
            .await
            .unwrap()
            .candidates
            .len(),
        1
    );
    restored.set_enabled(7, false).await.unwrap();
    assert!(
        SkillProvider::get(restored.as_ref(), &handle, &opts)
            .await
            .unwrap()
            .is_none()
    );
    assert!(restored.restore(8, &first.id).await.is_err());
    assert!(
        restored
            .create(
                8,
                "another",
                "Fixture",
                "Content",
                &root.to_string_lossy(),
                "owner",
                vec!["source".into()]
            )
            .await
            .is_err()
    );
    restored.set_enabled(8, true).await.unwrap();
    assert!(
        SkillProvider::get(restored.as_ref(), &handle, &opts)
            .await
            .unwrap()
            .is_some()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn corrupt_or_oversized_revision_files_do_not_activate_instructions() {
    let root = fixture();
    let store = SkillLifecycle::open(&root).await.unwrap();
    let item = create(&store, &root, 0, "Original.").await;
    drop(store);
    let path = root.join("skill-revisions-v1.json");
    let mut data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    data["revisions"][0]["content"] = "Replaced without validation".into();
    std::fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
    assert!(SkillLifecycle::open(&root).await.is_err());
    assert_ne!(
        SkillLifecycle::hash("Replaced without validation"),
        item.content_hash
    );
    std::fs::remove_dir_all(root).unwrap();
}
