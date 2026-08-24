use dsh_attachment::{attachment_id, image_variant_id};
use dsh_llm_deepseek::{
    DeepSeekUploadIndex, DeepSeekUploadRecord, deepseek_file_id, deepseek_file_scope,
};

fn temp_index() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("dsh-files-index-{}.json", uuid::Uuid::new_v4()))
}

#[tokio::test]
async fn upload_index_scopes_reuses_and_expires_records() {
    let path = temp_index();
    let scope = deepseek_file_scope("https://api.deepseek.com/", "secret-key");
    assert!(!scope.as_str().contains("secret-key"));
    let variant = image_variant_id(format!("sha256:{}", "b".repeat(64)));
    let record = DeepSeekUploadRecord {
        scope: scope.clone(),
        attachment_id: attachment_id(format!("sha256:{}", "a".repeat(64))),
        variant_id: variant.clone(),
        file_id: deepseek_file_id("file-1"),
        bytes: 42,
        created_at: 1000,
        expires_at: 10_000,
    };
    let index = DeepSeekUploadIndex::new(path.clone());
    let commit = index
        .commit(record.clone(), 1_000, 500)
        .await
        .expect("commit");
    assert!(commit.accepted);
    assert_eq!(
        index.get(&scope, &variant, 2_000, 500).await.expect("get"),
        Some(record.clone())
    );
    assert_eq!(
        index
            .get(&scope, &variant, 9_600, 500)
            .await
            .expect("expired"),
        None
    );
    let replacement = DeepSeekUploadRecord {
        file_id: deepseek_file_id("file-2"),
        created_at: 9_600,
        expires_at: 20_000,
        ..record.clone()
    };
    let replacement_commit = index
        .commit(replacement.clone(), 9_600, 500)
        .await
        .expect("replacement commit");
    assert!(replacement_commit.accepted);
    assert_eq!(replacement_commit.evicted, vec![record.clone()]);

    let other_scope = deepseek_file_scope("https://api.deepseek.com", "other-key");
    assert_eq!(
        index
            .get(&other_scope, &variant, 2_000, 500)
            .await
            .expect("scoped"),
        None
    );
    assert!(
        index
            .invalidate_exact(&scope, &variant, &deepseek_file_id("file-2"))
            .await
            .expect("invalidate")
    );
    assert_eq!(
        index
            .get(&scope, &variant, 2_000, 500)
            .await
            .expect("removed"),
        None
    );
    let _ = std::fs::remove_file(path);
}
