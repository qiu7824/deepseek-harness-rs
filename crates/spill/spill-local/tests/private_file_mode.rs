#![cfg(unix)]
use dsh_spill_local::{SaveTextOptions, save_text_file};
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn new_spill_files_are_created_with_owner_only_permissions() {
    let root = std::env::temp_dir().join(format!(
        "dsh-private-spill-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let saved = save_text_file(SaveTextOptions {
        root: root.to_str().unwrap().into(),
        session_id: "permission-fixture".into(),
        suggested_name: "../snapshot.txt".into(),
        content: "private snapshot fixture".into(),
    })
    .await
    .unwrap();
    let path = std::path::Path::new(&saved.path);
    assert!(path.starts_with(&root));
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "private snapshot fixture"
    );
    let again = save_text_file(SaveTextOptions {
        root: root.to_str().unwrap().into(),
        session_id: "permission-fixture".into(),
        suggested_name: "../snapshot.txt".into(),
        content: "second snapshot".into(),
    })
    .await
    .unwrap();
    assert_ne!(saved.path, again.path);
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "private snapshot fixture"
    );
    std::fs::remove_dir_all(&root).unwrap();
}
