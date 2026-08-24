//! Static projections for the shipped profile bundles.
//!
//! Rust cannot import npm bundle modules. The launcher therefore projects
//! only rows whose implementations are already compiled into the Rust binary.

use std::path::PathBuf;

use indexmap::IndexMap;
use serde_json::{Value, json};

use crate::{PatchOptions, ProfileLayer};

pub const BASE_BUNDLE: &str = "@deepseek-ai/dsh-base";
pub const WEB_BUNDLE: &str = "@deepseek-ai/dsh-web-app";
pub const HEADLESS_BUNDLE: &str = "@deepseek-ai/dsh-headless";

fn insert_patch(rows: Vec<Value>) -> PatchOptions {
    let mut patch = IndexMap::new();
    patch.insert("insert".to_string(), Value::Array(rows));
    patch
}

fn base_rows() -> Vec<Value> {
    vec![
        json!({"id": "invariants", "name": "@deepseek-ai/dsh-invariants"}),
        json!({"id": "sessions", "name": "@deepseek-ai/dsh-session"}),
        json!({"id": "llm", "name": "@deepseek-ai/dsh-llm"}),
        json!({"id": "llm-deepseek", "name": "@deepseek-ai/dsh-llm-deepseek"}),
        json!({"id": "system-prompt", "name": "@deepseek-ai/dsh-system-prompt"}),
        json!({"id": "tools", "name": "@deepseek-ai/dsh-tools"}),
        json!({"id": "agent-loop", "name": "@deepseek-ai/dsh-agent-loop"}),
        json!({"id": "commands", "name": "@deepseek-ai/dsh-commands"}),
        json!({"id": "goals", "name": "@deepseek-ai/dsh-goal"}),
        json!({"id": "goal-round-driver", "name": "@deepseek-ai/dsh-goal-round-driver"}),
        json!({"id": "command-goal", "name": "@deepseek-ai/dsh-command-goal"}),
        json!({"id": "tool-goal", "name": "@deepseek-ai/dsh-tool-goal"}),
        json!({"id": "session-persistence", "name": "@deepseek-ai/dsh-session-persistence-jsonl"}),
        json!({"id": "session-query", "name": "@deepseek-ai/dsh-session-query-sqlite"}),
        json!({"id": "schedule", "name": "@deepseek-ai/dsh-schedule"}),
        json!({"id": "agent-presets", "name": "@deepseek-ai/dsh-agent-presets"}),
        json!({"id": "jobs", "name": "@deepseek-ai/dsh-jobs-local"}),
        json!({"id": "subprocess", "name": "@deepseek-ai/dsh-subprocess-local"}),
        json!({"id": "sandbox", "name": "@deepseek-ai/dsh-sandbox-local"}),
        json!({"id": "sandbox-policy", "name": "@deepseek-ai/dsh-sandbox-policy"}),
        json!({"id": "shell-env", "name": "@deepseek-ai/dsh-shell-env"}),
        json!({"id": "tool-jobs", "name": "@deepseek-ai/dsh-tool-jobs"}),
        json!({"id": "subagent", "name": "@deepseek-ai/dsh-subagent"}),
        json!({
            "id": "subagent-spawn-in-process",
            "name": "@deepseek-ai/dsh-subagent-spawn-in-process",
            "config": { "providerName": "spawn" }
        }),
    ]
}

fn web_rows() -> Vec<Value> {
    vec![
        json!({"id": "code-runtime", "name": "@deepseek-ai/dsh-code-runtime-worker-thread"}),
        json!({"id": "webserver", "name": "@deepseek-ai/dsh-host-webserver"}),
        json!({"id": "frontend-static", "name": "@deepseek-ai/dsh-host-frontend-static"}),
        json!({"id": "directory-picker", "name": "@deepseek-ai/dsh-host-directory-picker-auto"}),
        json!({"id": "plugin-inventory", "name": "@deepseek-ai/dsh-host-plugin-inventory"}),
        json!({"id": "api-gateway", "name": "@deepseek-ai/dsh-host-apiproxy"}),
        json!({"id": "web-runtime", "name": "@deepseek-ai/dsh-web-app"}),
    ]
}

fn headless_rows() -> Vec<Value> {
    vec![
        json!({"id": "code-runtime", "name": "@deepseek-ai/dsh-code-runtime-worker-thread"}),
        json!({"id": "headless-runner", "name": "@deepseek-ai/dsh-headless"}),
    ]
}

/// Resolve one npm-named shipped bundle into its compiled Rust projection.
/// `None` leaves ordinary directory resolution to the caller.
pub fn shipped_bundle_layer(name: &str) -> Option<ProfileLayer> {
    let rows = match name {
        BASE_BUNDLE => base_rows(),
        WEB_BUNDLE => web_rows(),
        HEADLESS_BUNDLE => headless_rows(),
        _ => return None,
    };
    let anchor = PathBuf::from(format!("<shipped:{name}>"));
    Some(ProfileLayer {
        package_name: name.to_string(),
        package_dir: anchor.clone(),
        patch_path: anchor,
        patches: vec![insert_patch(rows)],
    })
}
