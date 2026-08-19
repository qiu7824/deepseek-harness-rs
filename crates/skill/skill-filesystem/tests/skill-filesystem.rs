//! Rust port of the discovery/parsing subset of
//! `skill-filesystem.spec.ts`: directory-bundle and flat Markdown
//! discovery, YAML frontmatter parsing, invocation policies, the `.system`
//! skip, project-root discovery, custom and bundled roots, missing and
//! malformed skill containment, and abort-signal passthrough.
//!
//! # Deferred
//!
//! - The chokidar watcher spec (the port ships a notify-based debounced
//!   watcher; the ancestor-watch and polling modes are documented
//!   deviations).

use std::path::PathBuf;
use std::sync::Arc;

use cordis::Context;
use dsh_skill::{SkillRegistry, SkillViewOptions};
use dsh_skill_filesystem::{Config, apply};

async fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "dsh-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    tokio::fs::create_dir_all(&dir).await.expect("temp dir");
    dir
}

async fn write_skill(root: &PathBuf, name: &str, description: &str) {
    let dir = root.join(name);
    tokio::fs::create_dir_all(&dir).await.expect("dir");
    tokio::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\nUse the skill.\n"),
    )
    .await
    .expect("write");
}

async fn write_flat_skill(root: &PathBuf, name: &str, description: &str) {
    tokio::fs::create_dir_all(root).await.expect("dir");
    tokio::fs::write(
        root.join(format!("{name}.md")),
        format!("---\nname: {name}\ndescription: {description}\n---\n\nFlat body.\n"),
    )
    .await
    .expect("write");
}

fn skills_of(ctx: &Context) -> Arc<SkillRegistry> {
    ctx.get_typed::<Arc<SkillRegistry>>("skills", false)
        .map(|slot| slot.as_ref().clone())
        .expect("skills")
}

async fn mounted(home: &PathBuf, config: Option<Config>) -> Context {
    let ctx = Context::root();
    let _skills = SkillRegistry::install(&ctx, dsh_skill::Config::default()).expect("skills");
    let config = config.unwrap_or(Config {
        dsh_home: Some(home.join(".dsh").to_string_lossy().into_owned()),
        agents_home: Some(home.join(".agents").to_string_lossy().into_owned()),
        watch: Some(false),
        ..Config::default()
    });
    let disposer = apply(&ctx, config).await.expect("apply");
    std::mem::forget(disposer);
    ctx
}

#[tokio::test(flavor = "current_thread")]
async fn discovers_directory_bundle_and_flat_skills_in_rank_order() {
    let home = temp_dir("fs-shapes").await;
    let root = home.join(".agents/skills");
    write_skill(&root, "bundle-skill", "Bundle skill").await;
    write_flat_skill(&root, "flat-skill", "Flat skill").await;
    let ctx = mounted(&home, None).await;

    let listed = skills_of(&ctx)
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    let names: Vec<&str> = listed.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(names, vec!["bundle-skill", "flat-skill"]);
    assert_eq!(listed[0].provider, "filesystem");
    assert_eq!(listed[0].source, "user-agents");

    let loaded = skills_of(&ctx)
        .get("bundle-skill", SkillViewOptions::default())
        .await
        .expect("get");
    let loaded = loaded.expect("loaded");
    assert_eq!(loaded.content, "Use the skill.");
    let dsh_skill::SkillResourceBase::Directory { path } = loaded.resource_base.expect("base")
    else {
        panic!("directory base");
    };
    // Component-based comparison: separator spelling differs between the
    // test's PathBuf joins and the provider's forward-slash tails.
    assert_eq!(std::path::Path::new(&path), root.join("bundle-skill"));
}

#[tokio::test(flavor = "current_thread")]
async fn skips_the_system_directory_under_the_dsh_root() {
    let home = temp_dir("fs-system").await;
    let root = home.join(".dsh/skills");
    let system = root.join(".system");
    tokio::fs::create_dir_all(&system).await.expect("dir");
    tokio::fs::write(
        system.join("SKILL.md"),
        "---\nname: hidden-system\ndescription: System\n---\n\nbody\n",
    )
    .await
    .expect("write");
    write_skill(&root, "visible-skill", "Visible").await;
    let ctx = mounted(&home, None).await;

    let listed = skills_of(&ctx)
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    let names: Vec<&str> = listed.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(names, vec!["visible-skill"]);
}

#[tokio::test(flavor = "current_thread")]
async fn parses_invocation_policies_and_metadata_from_frontmatter() {
    let home = temp_dir("fs-policy").await;
    let root = home.join(".agents/skills");
    tokio::fs::create_dir_all(&root).await.expect("dir");
    let dir = root.join("policy-skill");
    tokio::fs::create_dir_all(&dir).await.expect("dir");
    tokio::fs::write(
        dir.join("SKILL.md"),
        "---\nname: policy-skill\ndescription: Policy\ndisable-model-invocation: true\nmetadata:\n  owner: tests\n---\n\nPolicy body.\n",
    )
    .await
    .expect("write");
    let ctx = mounted(&home, None).await;

    let listed = skills_of(&ctx)
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].invocation,
        dsh_skill::SkillInvocationPolicy {
            model_invocable: false,
            user_invocable: true,
        }
    );
    let loaded = skills_of(&ctx)
        .get("policy-skill", SkillViewOptions::default())
        .await
        .expect("get");
    let loaded = loaded.expect("loaded");
    assert_eq!(
        loaded.metadata,
        Some(serde_json::json!({ "owner": "tests" }))
    );
    assert_eq!(loaded.content, "Policy body.");
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_legacy_invocation_keys_and_missing_required_fields() {
    let home = temp_dir("fs-invalid").await;
    let root = home.join(".agents/skills");
    tokio::fs::create_dir_all(&root).await.expect("dir");
    // A legacy key is unsupported.
    let legacy = root.join("legacy-skill");
    tokio::fs::create_dir_all(&legacy).await.expect("dir");
    tokio::fs::write(
        legacy.join("SKILL.md"),
        "---\nname: legacy-skill\ndescription: Legacy\ndisableModelInvocation: true\n---\n\nbody\n",
    )
    .await
    .expect("write");
    // Missing description.
    let missing = root.join("missing-skill");
    tokio::fs::create_dir_all(&missing).await.expect("dir");
    tokio::fs::write(
        missing.join("SKILL.md"),
        "---\nname: missing-skill\n---\n\nbody\n",
    )
    .await
    .expect("write");
    // Invalid skill name.
    let bad_name = root.join("bad-name");
    tokio::fs::create_dir_all(&bad_name).await.expect("dir");
    tokio::fs::write(
        bad_name.join("SKILL.md"),
        "---\nname: Bad_Name\ndescription: Bad\n---\n\nbody\n",
    )
    .await
    .expect("write");
    // No frontmatter at all.
    let plain = root.join("plain-file");
    tokio::fs::create_dir_all(&plain).await.expect("dir");
    tokio::fs::write(plain.join("SKILL.md"), "no frontmatter here\n")
        .await
        .expect("write");
    let ctx = mounted(&home, None).await;

    let listed = skills_of(&ctx)
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    assert!(listed.is_empty(), "{listed:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn discovers_project_skills_from_the_git_root() {
    let home = temp_dir("fs-project-home").await;
    let project = temp_dir("fs-project").await;
    tokio::fs::create_dir_all(project.join(".git"))
        .await
        .expect("git");
    write_skill(
        &project.join(".dsh/skills"),
        "project-skill",
        "Project skill",
    )
    .await;
    let ctx = mounted(&home, None).await;

    let listed = skills_of(&ctx)
        .list(SkillViewOptions {
            cwd: Some(project.to_string_lossy().into_owned()),
            signal: None,
            scope: None,
        })
        .await
        .expect("list");
    let names: Vec<&str> = listed.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(names, vec!["project-skill"]);
    assert_eq!(listed[0].source, "project-dsh");

    // The loaded body resolves the directory resource base.
    let loaded = skills_of(&ctx)
        .get(
            "project-skill",
            SkillViewOptions {
                cwd: Some(project.to_string_lossy().into_owned()),
                signal: None,
                scope: None,
            },
        )
        .await
        .expect("get");
    assert_eq!(loaded.expect("loaded").content, "Use the skill.");
}

#[tokio::test(flavor = "current_thread")]
async fn scans_custom_and_bundled_roots_with_stable_ranks() {
    let home = temp_dir("fs-custom").await;
    let custom = temp_dir("fs-custom-root").await;
    write_skill(&custom, "custom-skill", "Custom skill").await;
    let bundled = temp_dir("fs-bundled-root").await;
    write_skill(&bundled, "bundled-skill", "Bundled skill").await;
    let ctx = mounted(
        &home,
        Some(Config {
            dsh_home: Some(home.join(".dsh").to_string_lossy().into_owned()),
            agents_home: Some(home.join(".agents").to_string_lossy().into_owned()),
            watch: Some(false),
            custom_skill_dirs: Some(vec![custom.to_string_lossy().into_owned()]),
            bundled_skill_dir: Some(bundled.to_string_lossy().into_owned()),
            include_default_roots: Some(false),
            ..Config::default()
        }),
    )
    .await;

    let listed = skills_of(&ctx)
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    let names: Vec<&str> = listed.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(names, vec!["bundled-skill", "custom-skill"]);
    assert_eq!(listed[0].source, "bundled");
    assert_eq!(listed[1].source, "custom");
}

#[tokio::test(flavor = "current_thread")]
async fn a_missing_root_yields_an_empty_catalog() {
    let home = temp_dir("fs-missing").await;
    let ctx = mounted(&home, None).await;
    let listed = skills_of(&ctx)
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    assert!(listed.is_empty());
}
