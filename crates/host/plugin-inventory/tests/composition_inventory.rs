use std::sync::Arc;

use cordis::Context;
use dsh_agent_presets::{AgentPresets, Config, PresetRoot, PresetTrust};
use dsh_host_plugin_inventory::composition_inventory;
use serde_json::json;

#[tokio::test]
async fn composition_inventory_groups_presets_and_global_entries() {
    let root =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../config/agent-presets");
    let ctx = Context::root();
    let service = AgentPresets::install(
        &ctx,
        Config {
            default: "standard".to_string(),
            roots: vec![PresetRoot {
                path: root.to_string_lossy().into_owned(),
                trust: PresetTrust::System,
            }],
            include_user_root: false,
        },
        Arc::new(|_| None),
    )
    .unwrap();
    let presets = composition_inventory(&service).await;
    assert!(
        presets
            .iter()
            .any(|preset| preset.id == "standard" && preset.is_default)
    );
    let standard = presets
        .iter()
        .find(|preset| preset.id == "standard")
        .unwrap();
    assert!(
        standard
            .rows
            .iter()
            .any(|row| row.module_name == "@deepseek-ai/dsh-tool-fs")
    );
    assert!(
        standard
            .rows
            .iter()
            .any(|row| row.module_name == "@deepseek-ai/dsh-tool-subagent")
    );
    assert!(presets.iter().any(|preset| preset.id == "minimal"));
}

#[tokio::test]
async fn composition_inventory_reads_the_composed_agent_preset_roster() {
    let root = tempfile::tempdir().unwrap();
    let preset_dir = root.path().join("custom");
    std::fs::create_dir_all(&preset_dir).unwrap();
    std::fs::write(
        preset_dir.join("preset.yml"),
        "name: Custom\ndescription: User preset\n",
    )
    .unwrap();
    std::fs::write(
        preset_dir.join("agent.cordis.yml"),
        "- name: custom-module\n  id: custom-entry\n",
    )
    .unwrap();

    let ctx = Context::root();
    let presets = AgentPresets::install(
        &ctx,
        Config {
            default: "custom".to_string(),
            roots: vec![PresetRoot {
                path: root.path().to_string_lossy().into_owned(),
                trust: PresetTrust::User,
            }],
            include_user_root: false,
        },
        Arc::new(|_| None),
    )
    .unwrap();

    let inventory = composition_inventory(&presets).await;
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].id, "custom");
    assert_eq!(inventory[0].trust, "user");
    assert!(inventory[0].is_default);
    assert_eq!(inventory[0].rows[0].module_name, "custom-module");
}

#[tokio::test]
async fn composition_inventory_serializes_the_strict_wire_contract() {
    let root = tempfile::tempdir().unwrap();
    let preset_dir = root.path().join("conditional");
    std::fs::create_dir_all(&preset_dir).unwrap();
    std::fs::write(preset_dir.join("preset.yml"), "name: Conditional\n").unwrap();
    std::fs::write(
        preset_dir.join("agent.cordis.yml"),
        "- name: conditional-module\n  disabled:\n    __jsExpr: env.flag\n",
    )
    .unwrap();

    let ctx = Context::root();
    let presets = AgentPresets::install(
        &ctx,
        Config {
            default: "conditional".to_string(),
            roots: vec![PresetRoot {
                path: root.path().to_string_lossy().into_owned(),
                trust: PresetTrust::User,
            }],
            include_user_root: false,
        },
        Arc::new(|_| None),
    )
    .unwrap();

    let inventory = composition_inventory(&presets).await;
    let value = serde_json::to_value(&inventory[0]).unwrap();
    assert_eq!(value["rows"][0]["entryId"], json!(null));
    assert_eq!(value["rows"][0]["enabled"], "conditional");
    assert_eq!(value["rows"][0]["fiberPhase"], json!(null));
    assert!(value.get("description").is_none());
}
