//! Authoring integration tests: Rust port of the core subset of
//! `tests/authoring.spec.ts`.
//!
//! Covers: the invalid-id refusal, the exists refusal, whole-directory copy
//! with metadata rewrite, failed-copy cleanup, shipped-preset delete
//! refusal, and the containment check.

use std::path::PathBuf;

use dsh_agent_presets::{
    AgentPreset, COMPOSITION_FILE, METADATA_FILE, PresetRoot, PresetTrust, copy_composition,
    delete_composition, read_composition, writable_root,
};

fn counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "dsh-preset-authoring-{label}-{}-{}",
        std::process::id(),
        counter()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn root(path: &PathBuf, trust: PresetTrust) -> PresetRoot {
    PresetRoot {
        path: path.to_string_lossy().to_string(),
        trust,
    }
}

/// A preset directory carrying a composition, metadata, and an extra asset.
async fn seed(root_dir: &PathBuf, id: &str, trust: PresetTrust) -> AgentPreset {
    let dir = root_dir.join(id);
    tokio::fs::create_dir_all(&dir)
        .await
        .expect("create preset dir");
    tokio::fs::write(
        dir.join(COMPOSITION_FILE),
        "- id: alpha\n  name: contribute\n  config:\n    tool: alpha\n",
    )
    .await
    .expect("write composition");
    tokio::fs::write(
        dir.join(METADATA_FILE),
        "name: 标准模式\ndescription: 源描述。\n",
    )
    .await
    .expect("write metadata");
    tokio::fs::write(dir.join("asset.txt"), "payload")
        .await
        .expect("write asset");
    AgentPreset {
        id: id.to_string(),
        trust,
        path: dir.join(COMPOSITION_FILE).to_string_lossy().to_string(),
        name: Some("标准模式".to_string()),
        description: Some("源描述。".to_string()),
        order: None,
        broken: None,
    }
}

#[tokio::test]
async fn copies_a_preset_whole_with_metadata_rewritten() {
    let system = temp_dir("system");
    let user = temp_dir("user");
    let source = seed(&system, "standard", PresetTrust::System).await;
    let roots = [
        root(&system, PresetTrust::System),
        root(&user, PresetTrust::User),
    ];

    let copied = copy_composition(&roots, &source, "copied", None)
        .await
        .expect("copy succeeds");
    assert_eq!(copied, user.join("copied").to_string_lossy().to_string());

    // The whole directory travels: composition, asset, and metadata.
    let copied_composition = tokio::fs::read_to_string(user.join("copied").join(COMPOSITION_FILE))
        .await
        .expect("copied composition");
    assert!(copied_composition.contains("name: contribute"));
    assert_eq!(
        tokio::fs::read_to_string(user.join("copied").join("asset.txt"))
            .await
            .expect("copied asset"),
        "payload"
    );
    // Metadata keeps the description but drops the name (a copy presenting
    // itself identically to its source would stop the roster distinguishing
    // them).
    let metadata = tokio::fs::read_to_string(user.join("copied").join(METADATA_FILE))
        .await
        .expect("copied metadata");
    assert!(metadata.contains("description: 源描述。"), "got {metadata}");
    assert!(!metadata.contains("name: 标准模式"), "got {metadata}");

    // The copy reads back as its own preset.
    let copy_preset = dsh_agent_presets::scan_root(&root(&user, PresetTrust::User))
        .await
        .expect("scan user root")
        .into_iter()
        .find(|preset| preset.id == "copied")
        .expect("copy discovered");
    assert_eq!(copy_preset.name, None);
    assert_eq!(
        read_composition(&copy_preset).await.expect("read copy"),
        copied_composition
    );
}

#[tokio::test]
async fn a_copy_with_a_name_keeps_it_and_the_description() {
    let system = temp_dir("system");
    let user = temp_dir("user");
    let source = seed(&system, "standard", PresetTrust::System).await;
    let roots = [
        root(&system, PresetTrust::System),
        root(&user, PresetTrust::User),
    ];

    copy_composition(&roots, &source, "renamed", Some("新名字"))
        .await
        .expect("copy succeeds");
    let metadata = tokio::fs::read_to_string(user.join("renamed").join(METADATA_FILE))
        .await
        .expect("copied metadata");
    assert!(metadata.contains("name: 新名字"), "got {metadata}");
    assert!(metadata.contains("description: 源描述。"), "got {metadata}");
}

#[tokio::test]
async fn refuses_an_unusable_id() {
    let system = temp_dir("system");
    let user = temp_dir("user");
    let source = seed(&system, "standard", PresetTrust::System).await;
    let roots = [
        root(&system, PresetTrust::System),
        root(&user, PresetTrust::User),
    ];

    for bad in ["../escape", "UPPER", "with space", "a/b", "-lead"] {
        let error = copy_composition(&roots, &source, bad, None)
            .await
            .expect_err(&format!("{bad} must be refused"));
        assert!(
            error.to_string().contains("must match"),
            "unexpected diagnostic for {bad}: {error}"
        );
    }
    assert!(!user.join("..").join("escape").exists());
}

#[tokio::test]
async fn refuses_an_occupied_id_even_without_a_composition() {
    let system = temp_dir("system");
    let user = temp_dir("user");
    let source = seed(&system, "standard", PresetTrust::System).await;
    let roots = [
        root(&system, PresetTrust::System),
        root(&user, PresetTrust::User),
    ];
    // A directory with no composition file still occupies the name.
    tokio::fs::create_dir_all(user.join("taken"))
        .await
        .expect("occupy the name");

    let error = copy_composition(&roots, &source, "taken", None)
        .await
        .expect_err("occupied id must be refused");
    assert!(
        error.to_string().contains("already exists"),
        "unexpected diagnostic: {error}"
    );
}

#[tokio::test]
async fn a_failed_copy_leaves_nothing() {
    let system = temp_dir("system");
    let user = temp_dir("user");
    // A source directory that vanishes mid-copy: simulate by pointing the
    // source at a path whose parent does not exist.
    let source = AgentPreset {
        id: "ghost".to_string(),
        trust: PresetTrust::System,
        path: system
            .join("ghost")
            .join(COMPOSITION_FILE)
            .to_string_lossy()
            .to_string(),
        name: None,
        description: None,
        order: None,
        broken: None,
    };
    let roots = [
        root(&system, PresetTrust::System),
        root(&user, PresetTrust::User),
    ];

    let error = copy_composition(&roots, &source, "phantom", None)
        .await
        .expect_err("missing source fails the copy");
    assert!(!error.to_string().is_empty());
    assert!(
        !user.join("phantom").exists(),
        "a half-copied directory must be cleaned up"
    );
}

#[tokio::test]
async fn deletes_only_user_presets_under_the_writable_root() {
    let system = temp_dir("system");
    let user = temp_dir("user");
    let shipped = seed(&system, "standard", PresetTrust::System).await;
    let authored = seed(&user, "mine", PresetTrust::User).await;
    let roots = [
        root(&system, PresetTrust::System),
        root(&user, PresetTrust::User),
    ];

    // Shipped presets are refused.
    let error = delete_composition(&roots, &shipped)
        .await
        .expect_err("shipped presets must not be deleted");
    assert!(
        error.to_string().contains("ships with the deployment"),
        "unexpected diagnostic: {error}"
    );

    // A user preset outside the writable root is refused (containment).
    let stranger = AgentPreset {
        id: "elsewhere".to_string(),
        trust: PresetTrust::User,
        path: system
            .join("elsewhere")
            .join(COMPOSITION_FILE)
            .to_string_lossy()
            .to_string(),
        ..authored.clone()
    };
    let error = delete_composition(&roots, &stranger)
        .await
        .expect_err("a user preset outside the writable root must be refused");
    assert!(
        error.to_string().contains("does not live under"),
        "unexpected diagnostic: {error}"
    );

    // A locally authored preset deletes.
    delete_composition(&roots, &authored)
        .await
        .expect("user presets delete");
    assert!(!user.join("mine").exists());
}

#[tokio::test]
async fn writable_root_reports_when_no_user_root_is_configured() {
    let system = temp_dir("system");
    let roots = [root(&system, PresetTrust::System)];
    let error = writable_root(&roots).expect_err("no writable root");
    assert!(
        error.to_string().contains("no user-writable preset root"),
        "unexpected diagnostic: {error}"
    );
}
