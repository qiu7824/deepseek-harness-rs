//! Entry-list patch semantics (port of `applyEntryPatches` in the include
//! plugin). Shared by mounting and (later) offline config tooling so a dump
//! can never drift from what boots.

use std::collections::HashMap;

use indexmap::IndexMap;
use serde_json::Value;

/// Runtime patch applied to entries loaded from an included config file
/// (TS `PatchOptions`; known keys plus arbitrary overrides).
pub type PatchOptions = IndexMap<String, Value>;

/// A location path into the entry list (group configs nest arbitrarily).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Entry(usize),
    Config,
}

fn index_entries(
    entries: &[Value],
    prefix: &[Seg],
    base: usize,
    map: &mut HashMap<String, Vec<Seg>>,
) {
    for (offset, entry) in entries.iter().enumerate() {
        let mut path = prefix.to_vec();
        path.push(Seg::Entry(base + offset));
        if let Some(id) = entry.get("id").and_then(|value| value.as_str()) {
            map.insert(id.to_string(), path.clone());
        }
        let is_group = entry.get("group").and_then(|value| value.as_bool()).unwrap_or(false);
        if is_group {
            if let Some(list) = entry.get("config").and_then(|value| value.as_array()) {
                let mut child = path.clone();
                child.push(Seg::Config);
                index_entries(list, &child, 0, map);
            }
        }
    }
}

fn get_mut<'a>(root: &'a mut [Value], path: &[Seg]) -> &'a mut Value {
    let (Seg::Entry(index), rest) = path.split_first().expect("entry segment") else {
        unreachable!("path must start with Entry");
    };
    let value = &mut root[*index];
    match rest.split_first() {
        Some((Seg::Config, tail)) => {
            let list = value
                .get_mut("config")
                .and_then(|config| config.as_array_mut())
                .expect("group entry config list");
            get_mut(list, tail)
        }
        Some(_) => unreachable!("Entry cannot follow Entry"),
        None => value,
    }
}

/// Apply patch lists to an entry list (TS `applyEntryPatches`).
///
/// The input is never mutated and the result is always detached from it.
/// Inserted entries are indexed as they are added, so a later patch in the
/// same list can target a row an earlier patch inserted. A patch that matches
/// nothing warns and is skipped.
pub fn apply_entry_patches(
    data: &[Value],
    patches: Option<&[PatchOptions]>,
    warn: &mut dyn FnMut(&str),
) -> Vec<Value> {
    let mut data = data.to_vec();
    let Some(patches) = patches else { return data };
    if patches.is_empty() {
        return data;
    }

    let mut entry_map: HashMap<String, Vec<Seg>> = HashMap::new();
    index_entries(&data, &[], 0, &mut entry_map);

    for patch in patches {
        let id = patch.get("id").and_then(|value| value.as_str()).map(|s| s.to_string());
        let insert = patch.get("insert").and_then(|value| value.as_array());
        let name = patch.get("name").and_then(|value| value.as_str()).map(|s| s.to_string());

        if let Some(insert) = insert {
            if let Some(id) = &id {
                let Some(path) = entry_map.get(id).cloned() else {
                    warn(&format!("patch insert: entry {id} not found"));
                    continue;
                };
                let target = get_mut(&mut data, &path);
                let is_group =
                    target.get("group").and_then(|value| value.as_bool()).unwrap_or(false);
                if !is_group {
                    warn(&format!("patch insert: entry {id} is not a group"));
                    continue;
                }
                if !target
                    .get("config")
                    .is_some_and(|config| config.is_array())
                {
                    target["config"] = Value::Array(Vec::new());
                }
                let config = target
                    .get_mut("config")
                    .expect("config slot")
                    .as_array_mut()
                    .expect("config list");
                let base = config.len();
                config.extend(insert.iter().cloned());
                // Index what this patch added (TS buildMap(insert)): the new
                // rows sit at `base..` inside the group's config array.
                let mut prefix = path;
                prefix.push(Seg::Config);
                index_entries(insert, &prefix, base, &mut entry_map);
            } else {
                let base = data.len();
                data.extend(insert.iter().cloned());
                index_entries(insert, &[], base, &mut entry_map);
            }
            continue;
        }

        let Some(id) = &id else {
            warn("patch: id is required for non-insert patches");
            continue;
        };
        let Some(path) = entry_map.get(id).cloned() else {
            warn(&format!("patch: entry {id} not found"));
            continue;
        };

        if let Some(name) = &name {
            let target = get_mut(&mut data, &path);
            let target_name = target.get("name").and_then(|value| value.as_str());
            if target_name != Some(name.as_str()) {
                warn(&format!(
                    "patch: name mismatch for {id} (expected {}, got {}), skipping",
                    target_name.unwrap_or("<none>"),
                    name
                ));
                continue;
            }
        }

        let target = get_mut(&mut data, &path);
        for (key, value) in patch {
            if key == "id" || key == "insert" || key == "name" {
                continue;
            }
            target[key] = value.clone();
        }
    }

    data
}

/// Convenience: patch an entry list serialized as JSON.
pub fn apply_entry_patches_json(
    data: &Value,
    patches: Option<&[PatchOptions]>,
    warn: &mut dyn FnMut(&str),
) -> Result<Value, String> {
    let list = data
        .as_array()
        .ok_or_else(|| "config file must be a top-level array".to_string())?;
    Ok(Value::Array(apply_entry_patches(list, patches, warn)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn patch(map: &[(&str, Value)]) -> PatchOptions {
        map.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn noop_warn(_message: &str) {}

    #[test]
    fn patches_override_and_insert() {
        let data = json!([
            { "id": "a", "name": "p1", "config": 1 },
            { "id": "g", "name": "group", "group": true, "config": [
                { "id": "b", "name": "p2", "config": 2 }
            ]}
        ]);
        let patches = vec![
            patch(&[("id", json!("a")), ("config", json!(9))]),
            patch(&[("id", json!("g")), ("insert", json!([{ "id": "c", "name": "p3" }]))]),
            patch(&[("id", json!("c")), ("config", json!(7))]),
        ];
        let result =
            apply_entry_patches_json(&data, Some(&patches), &mut noop_warn).unwrap();
        assert_eq!(result[0]["config"], json!(9));
        let group_children = result[1]["config"].as_array().unwrap();
        assert_eq!(group_children.len(), 2);
        assert_eq!(group_children[1]["id"], json!("c"));
        assert_eq!(group_children[1]["config"], json!(7));
    }

    #[test]
    fn missing_target_warns_and_skips() {
        let data = json!([{ "id": "a", "name": "p1" }]);
        let mut warnings = Vec::new();
        let patches = vec![patch(&[("id", json!("nope")), ("config", json!(1))])];
        let result = apply_entry_patches_json(
            &data,
            Some(&patches),
            &mut |message| warnings.push(message.to_string()),
        )
        .unwrap();
        assert_eq!(result, data);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("not found"));
    }

    #[test]
    fn name_mismatch_skips() {
        let data = json!([{ "id": "a", "name": "p1", "config": 1 }]);
        let patches = vec![patch(&[
            ("id", json!("a")),
            ("name", json!("other")),
            ("config", json!(2)),
        ])];
        let result =
            apply_entry_patches_json(&data, Some(&patches), &mut noop_warn).unwrap();
        assert_eq!(result[0]["config"], json!(1));
    }

    #[test]
    fn unknown_overrides_apply() {
        let data = json!([{ "id": "a", "name": "p1" }]);
        let patches = vec![patch(&[("id", json!("a")), ("customKey", json!("v"))])];
        let result =
            apply_entry_patches_json(&data, Some(&patches), &mut noop_warn).unwrap();
        assert_eq!(result[0]["customKey"], json!("v"));
    }
}
