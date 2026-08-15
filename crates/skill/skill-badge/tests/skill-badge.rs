//! Rust port of `skill-badge.spec.ts`: the bundled `dsh-badge` provider
//! registers, lists, loads, and disposes; the shipped PNG stays byte-identical.

use std::sync::Arc;

use cordis::{Context, arc};
use dsh_skill::{SkillRegistry, SkillViewOptions};
use dsh_skill_badge::{SkillBadgePlugin, resource_base};

fn skills_of(ctx: &Context) -> Arc<SkillRegistry> {
    ctx.get_typed::<Arc<SkillRegistry>>("skills", false)
        .map(|slot| slot.as_ref().clone())
        .expect("skills")
}

#[tokio::test(flavor = "current_thread")]
async fn registers_and_disposes_the_bundled_badge_skill() {
    let ctx = Context::root();
    let _skills = SkillRegistry::install(&ctx, dsh_skill::Config::default()).expect("skills");
    let fiber = ctx.plugin(Arc::new(SkillBadgePlugin), arc(()));
    fiber.settle().await.expect("settle");
    // Drive the runtime so the registration effect lands before listing.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let listed = skills_of(&ctx)
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    let summary = &listed[0];
    assert_eq!(summary.name, "dsh-badge");
    assert_eq!(
        summary.description,
        "Add the official “powered by dsh” badge to documents, pull requests, merge requests, and other content produced with DeepSeek Harness. Use whenever creating a pull request or merge request. Also use when the user asks for a dsh badge, powered-by-dsh attribution, or a reusable dsh badge asset or snippet."
    );
    assert_eq!(summary.invocation, dsh_skill::SkillInvocationPolicy::BOTH);
    assert_eq!(summary.provider, "dsh-badge");
    assert_eq!(summary.source, "bundled");
    assert_eq!(summary.resource_base, Some(resource_base()));

    let loaded = skills_of(&ctx)
        .get("dsh-badge", SkillViewOptions::default())
        .await
        .expect("get");
    let loaded = loaded.expect("loaded");
    assert!(loaded.content.contains("Preserve the badge's 121×20 dimensions"));
    assert_eq!(loaded.resource_base, Some(resource_base()));

    fiber.dispose().await;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let listed = skills_of(&ctx)
        .list(SkillViewOptions::default())
        .await
        .expect("list");
    assert!(listed.is_empty());
}

#[test]
fn ships_the_official_png_unchanged() {
    use sha2::{Digest, Sha256};
    let image = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/dsh-badge.png"))
        .expect("badge png");
    // PNG IHDR width (offset 16) and height (offset 20) as big-endian u32.
    let width = u32::from_be_bytes(image[16..20].try_into().expect("width"));
    let height = u32::from_be_bytes(image[20..24].try_into().expect("height"));
    assert_eq!(width, 726);
    assert_eq!(height, 120);
    let digest = Sha256::digest(&image);
    assert_eq!(
        format!("{digest:x}"),
        "f2c4f5ec9cbe847c0c763545c4d839efa8485bc74203733d0a0e8259f233c653"
    );
}
