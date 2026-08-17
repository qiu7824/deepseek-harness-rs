//! Filesystem discovery of agent presets. Rust port of `src/discovery.ts`.
//!
//! A preset is a directory holding [`COMPOSITION_FILE`], optionally beside a
//! [`METADATA_FILE`] carrying its display text; the directory name is the
//! preset id. Discovery re-reads the roots on every call, and it also owns
//! preset HEALTH: a directory whose composition is missing or unloadable is
//! reported as a broken roster row rather than skipped.

use std::path::{Path, PathBuf};

use dsh_cordis_include::yaml::parse_yaml;
use dsh_home_paths::expand_home_path;
use serde_json::Value;

use crate::metadata::read_preset_metadata;
use crate::preset::{AgentPreset, PresetRoot, preset_id_ok};

/// The composition file that makes a directory a preset.
pub const COMPOSITION_FILE: &str = "agent.cordis.yml";

/// Harness-home directory holding locally authored presets. Package-internal
/// on purpose, exported for tests that assert the convention.
pub const USER_PRESET_DIR: &str = ".agent-presets";

/// Why `rows` cannot be an entry list, or `None` when it can
/// (TS `entryListProblem`). A shallow shape check, deliberately short of the
/// loader's work: rows are only required to be maps carrying a plugin `name`
/// (groups recurse into their own lists).
fn entry_list_problem(rows: &Value, at: &str) -> Option<String> {
    let Some(list) = rows.as_array() else {
        return Some(if at.is_empty() {
            "the composition must be a top-level list of plugin rows".to_string()
        } else {
            format!("group {at} must hold a list of plugin rows")
        });
    };
    for (index, row) in list.iter().enumerate() {
        let label = if at.is_empty() {
            format!("row {}", index + 1)
        } else {
            format!("{at} row {}", index + 1)
        };
        let Some(record) = row.as_object() else {
            return Some(format!(
                "{label} is not a plugin row (expected a map with a \"name\")"
            ));
        };
        let Some(name) = record.get("name") else {
            return Some(format!(
                "{label} names no plugin (a \"name\" string is required)"
            ));
        };
        if !name.is_string() || name.as_str().unwrap_or_default().is_empty() {
            return Some(format!(
                "{label} names no plugin (a \"name\" string is required)"
            ));
        }
        if record.get("group").and_then(|value| value.as_bool()) == Some(true) {
            let nested = record.get("config").unwrap_or(&Value::Null);
            if let Some(problem) = entry_list_problem(nested, &label) {
                return Some(problem);
            }
        }
    }
    None
}

/// Why the composition at `path` cannot mount, or `None` when it looks
/// loadable (TS `compositionProblem`). Parsed with the loader's own YAML
/// dialect so health can never call a composition broken that the loader
/// would accept.
pub async fn composition_problem(path: &Path) -> Option<String> {
    let content = match tokio::fs::read_to_string(path).await {
        Ok(content) => content,
        // The caller statted this file moments ago; any read failure now is
        // the same answer as unparsable.
        Err(_) => {
            return Some(format!(
                "the composition file {COMPOSITION_FILE} cannot be read"
            ));
        }
    };
    let rows: Value = match parse_yaml(&content) {
        Ok(rows) => rows,
        Err(error) => {
            // First line only: the reason is displayed on a roster card.
            let first_line = error.lines().next().unwrap_or(&error).to_string();
            return Some(format!("the composition is not valid YAML: {first_line}"));
        }
    };
    entry_list_problem(&rows, "")
}

/// Whether `path` names an existing regular file (TS `isFile`).
async fn is_file(path: &Path) -> bool {
    match tokio::fs::metadata(path).await {
        Ok(metadata) => metadata.is_file(),
        // Any stat failure means this directory does not present a
        // composition, which is not an error.
        Err(_) => false,
    }
}

/// Scan one root for preset directories (TS `scanRoot`). An absent root
/// yields no presets rather than throwing. Every directory whose name is a
/// usable preset id is a roster row — broken when its composition is missing
/// or unloadable. A directory named outside the id pattern is skipped.
pub async fn scan_root(root: &PresetRoot) -> Result<Vec<AgentPreset>, String> {
    let dir = expand_home_path(&root.path);
    let children = tokio::fs::read_dir(&dir).await;
    let mut children = match children {
        Ok(children) => children,
        Err(error) => {
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(Vec::new());
            }
            return Err(format!(
                "agent-presets: cannot read preset root {}: {error}",
                dir.display()
            ));
        }
    };
    let mut found: Vec<AgentPreset> = Vec::new();
    loop {
        let child = match children.next_entry().await {
            Ok(Some(child)) => child,
            Ok(None) => break,
            Err(error) => {
                return Err(format!(
                    "agent-presets: cannot read preset root {}: {error}",
                    dir.display()
                ));
            }
        };
        let file_type = match child.file_type().await {
            Ok(file_type) => file_type,
            Err(error) => {
                return Err(format!(
                    "agent-presets: cannot read preset root {}: {error}",
                    dir.display()
                ));
            }
        };
        let name = child.file_name();
        let name = name.to_string_lossy();
        if !file_type.is_dir() || !preset_id_ok(&name) {
            continue;
        }
        let directory = dir.join(name.as_ref());
        let path = directory.join(COMPOSITION_FILE);
        let broken = if is_file(&path).await {
            composition_problem(&path).await
        } else {
            Some(format!(
                "the composition file {COMPOSITION_FILE} is missing — the directory still occupies the id; delete it or restore the file"
            ))
        };
        // Display text only, and never fatal.
        let metadata = read_preset_metadata(&directory).await;
        found.push(AgentPreset {
            id: name.to_string(),
            trust: root.trust,
            path: path.to_string_lossy().to_string(),
            name: metadata.name,
            description: metadata.description,
            order: metadata.order,
            broken,
        });
    }
    // Declared order first so the shipped set reads by capability; everything
    // else falls back to the id.
    // Deviation: TS sorts ties by `localeCompare`; Rust compares by UTF-8
    // code point order (identical for ASCII preset ids).
    found.sort_by(|left, right| {
        let by_order = left
            .order
            .unwrap_or(f64::INFINITY)
            .total_cmp(&right.order.unwrap_or(f64::INFINITY));
        match by_order {
            std::cmp::Ordering::Equal => left.id.cmp(&right.id),
            other => other,
        }
    });
    Ok(found)
}

/// Scan every root in precedence order; an earlier root wins a duplicate id
/// (TS `discoverPresets`).
pub async fn discover_presets(roots: &[PresetRoot]) -> Result<Vec<AgentPreset>, String> {
    let mut by_id: indexmap::IndexMap<String, AgentPreset> = indexmap::IndexMap::new();
    for root in roots {
        for preset in scan_root(root).await? {
            if by_id.contains_key(&preset.id) {
                continue;
            }
            by_id.insert(preset.id.clone(), preset);
        }
    }
    Ok(by_id.into_values().collect())
}

/// Resolve one root's absolute scan directory (test helper + shared
/// derivation, TS `resolve(expandHomePath(root.path))`).
pub fn resolved_root_dir(root: &PresetRoot) -> PathBuf {
    expand_home_path(&root.path)
}

#[cfg(test)]
mod tests {
    use crate::metadata::METADATA_FILE;

    use super::*;

    /// Cheap process-unique counter for temp dirs.
    fn fastrand() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    async fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dsh-preset-{label}-{}-{}",
            std::process::id(),
            fastrand()
        ));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        dir
    }

    async fn seed_preset(root: &Path, id: &str, composition: Option<&str>) {
        let dir = root.join(id);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        if let Some(composition) = composition {
            tokio::fs::write(dir.join(COMPOSITION_FILE), composition)
                .await
                .unwrap();
        }
    }

    const VALID: &str = "- id: tool-alpha\n  name: contribute\n  config:\n    tool: alpha\n";

    #[tokio::test]
    async fn scans_and_orders_by_declared_order_then_id() {
        let root = temp_root("scan").await;
        seed_preset(&root, "b-preset", Some(VALID)).await;
        seed_preset(&root, "a-preset", Some(VALID)).await;
        let root_dir = root.to_string_lossy().to_string();
        let presets = scan_root(&PresetRoot {
            path: root_dir,
            trust: crate::preset::PresetTrust::User,
        })
        .await
        .unwrap();
        let ids: Vec<&str> = presets.iter().map(|preset| preset.id.as_str()).collect();
        assert_eq!(ids, vec!["a-preset", "b-preset"]);
    }

    #[tokio::test]
    async fn reports_a_missing_composition_as_broken() {
        let root = temp_root("broken").await;
        seed_preset(&root, "hole", None).await;
        let root_dir = root.to_string_lossy().to_string();
        let presets = scan_root(&PresetRoot {
            path: root_dir,
            trust: crate::preset::PresetTrust::User,
        })
        .await
        .unwrap();
        assert_eq!(presets.len(), 1);
        let broken = presets[0].broken.as_deref().unwrap();
        assert!(
            broken.contains("is missing"),
            "unexpected diagnostic: {broken}"
        );
    }

    #[tokio::test]
    async fn reports_unparsable_yaml_as_broken() {
        let root = temp_root("yaml").await;
        seed_preset(&root, "bad", Some("- id: x\n  name: [unclosed\n")).await;
        let root_dir = root.to_string_lossy().to_string();
        let presets = scan_root(&PresetRoot {
            path: root_dir,
            trust: crate::preset::PresetTrust::User,
        })
        .await
        .unwrap();
        assert!(
            presets[0]
                .broken
                .as_deref()
                .unwrap()
                .contains("not valid YAML")
        );
    }

    #[tokio::test]
    async fn reports_an_entry_shape_problem_as_broken() {
        let root = temp_root("shape").await;
        seed_preset(&root, "bad", Some("just a string\n")).await;
        let root_dir = root.to_string_lossy().to_string();
        let presets = scan_root(&PresetRoot {
            path: root_dir,
            trust: crate::preset::PresetTrust::User,
        })
        .await
        .unwrap();
        assert!(
            presets[0]
                .broken
                .as_deref()
                .unwrap()
                .contains("must be a top-level list")
        );
    }

    #[tokio::test]
    async fn skips_directories_that_are_not_preset_ids() {
        let root = temp_root("skip").await;
        seed_preset(&root, "UPPER", Some(VALID)).await;
        seed_preset(&root, ".hidden", Some(VALID)).await;
        seed_preset(&root, "ok-1", Some(VALID)).await;
        let root_dir = root.to_string_lossy().to_string();
        let presets = scan_root(&PresetRoot {
            path: root_dir,
            trust: crate::preset::PresetTrust::User,
        })
        .await
        .unwrap();
        let ids: Vec<&str> = presets.iter().map(|preset| preset.id.as_str()).collect();
        assert_eq!(ids, vec!["ok-1"]);
    }

    #[tokio::test]
    async fn absent_root_yields_no_presets() {
        let missing = std::env::temp_dir().join(format!(
            "dsh-preset-missing-{}-{}",
            std::process::id(),
            fastrand()
        ));
        let root_dir = missing.to_string_lossy().to_string();
        let presets = scan_root(&PresetRoot {
            path: root_dir,
            trust: crate::preset::PresetTrust::User,
        })
        .await
        .unwrap();
        assert!(presets.is_empty());
    }

    #[tokio::test]
    async fn first_root_wins_a_duplicate_id() {
        let first = temp_root("first").await;
        let second = temp_root("second").await;
        seed_preset(&first, "standard", Some(VALID)).await;
        seed_preset(&second, "standard", Some(VALID)).await;
        seed_preset(&second, "only-here", Some(VALID)).await;
        let roots = vec![
            PresetRoot {
                path: first.to_string_lossy().to_string(),
                trust: crate::preset::PresetTrust::System,
            },
            PresetRoot {
                path: second.to_string_lossy().to_string(),
                trust: crate::preset::PresetTrust::User,
            },
        ];
        let presets = discover_presets(&roots).await.unwrap();
        let ids: Vec<&str> = presets.iter().map(|preset| preset.id.as_str()).collect();
        assert_eq!(ids, vec!["standard", "only-here"]);
        assert_eq!(presets[0].trust, crate::preset::PresetTrust::System);
    }

    #[tokio::test]
    async fn reads_metadata_beside_the_composition() {
        let root = temp_root("meta").await;
        let dir = root.join("described");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join(COMPOSITION_FILE), VALID)
            .await
            .unwrap();
        tokio::fs::write(
            dir.join(METADATA_FILE),
            "name: 标准模式\ndescription: 完整。\n",
        )
        .await
        .unwrap();
        let root_dir = root.to_string_lossy().to_string();
        let presets = scan_root(&PresetRoot {
            path: root_dir,
            trust: crate::preset::PresetTrust::User,
        })
        .await
        .unwrap();
        assert_eq!(presets[0].name.as_deref(), Some("标准模式"));
        assert_eq!(presets[0].description.as_deref(), Some("完整。"));
        assert_eq!(presets[0].broken, None);
    }
}
