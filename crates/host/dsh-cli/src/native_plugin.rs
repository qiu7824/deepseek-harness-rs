use std::path::{Component, Path, PathBuf};
use std::process::Command;

use dsh_app_boot::plugin_profile::{Documents, Profile};
use serde_json::{Value, json};

fn valid_package_name(name: &str) -> bool {
    let parts: Vec<_> = name.split('/').collect();
    match parts.as_slice() {
        [plain] => {
            !matches!(*plain, "" | "." | "..")
                && plain
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        }
        [scope, plain] if scope.starts_with('@') => {
            scope.len() > 1
                && scope[1..]
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
                && !matches!(*plain, "" | "." | "..")
                && plain
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        }
        _ => false,
    }
}

fn package_dir(profile: &Path, name: &str) -> Result<PathBuf, String> {
    if !valid_package_name(name) {
        return Err(format!("dsh: invalid plugin package name {name:?}"));
    }
    let mut path = profile.join("node_modules");
    for part in name.split('/') {
        path.push(part);
    }
    Ok(path)
}

fn read_manifest(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(
        &std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?,
    )
    .map_err(|error| format!("parse {}: {error}", path.display()))
}

fn web_client_export(manifest: &Value) -> Option<&str> {
    if manifest
        .pointer("/dsh/client/platform")
        .and_then(Value::as_str)
        != Some("web")
    {
        return None;
    }
    let export = manifest.get("exports")?.get("./client")?;
    match export {
        Value::String(path) => Some(path),
        Value::Object(object) => object
            .get("browser")
            .or_else(|| object.get("default"))
            .and_then(Value::as_str),
        _ => None,
    }
}

fn validate_web_plugin(root: &Path) -> Result<(String, bool), String> {
    let manifest = read_manifest(&root.join("package.json"))?;
    let name = manifest
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "dsh: plugin package.json has no name".to_string())?;
    if !valid_package_name(name) {
        return Err(format!(
            "dsh: plugin declares invalid package name {name:?}"
        ));
    }
    let export = web_client_export(&manifest).ok_or_else(|| {
        "dsh: this Rust runtime currently installs pure Web plugins only; dsh.client.platform=web and exports[\"./client\"] are required".to_string()
    })?;
    let relative = export.strip_prefix("./").unwrap_or(export);
    let path = Path::new(relative);
    if path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
        || path.extension().and_then(|part| part.to_str()) != Some("js")
    {
        return Err("dsh: unsafe plugin client export".to_string());
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("canonicalize plugin: {error}"))?;
    let client = root
        .join(path)
        .canonicalize()
        .map_err(|error| format!("plugin client export is missing: {error}"))?;
    if !client.starts_with(&canonical_root) {
        return Err("dsh: plugin client export escapes its package".to_string());
    }
    let bytes = std::fs::metadata(&client)
        .map_err(|error| format!("stat plugin client: {error}"))?
        .len();
    if bytes > 2 * 1024 * 1024 {
        return Err("dsh: plugin client bundle exceeds 2 MiB".to_string());
    }
    let has_host = manifest.pointer("/dsh/bundle/patch").is_some();
    Ok((name.to_string(), has_host))
}

fn update_documents(
    mut documents: Documents,
    name: &str,
    spec: Option<&str>,
) -> Result<Documents, String> {
    let dependencies = documents
        .manifest
        .as_object_mut()
        .ok_or("invalid profile manifest")?
        .entry("dependencies")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("invalid dependencies")?;
    if let Some(spec) = spec {
        dependencies.insert(name.into(), json!(spec));
    } else {
        dependencies.remove(name);
    }
    let legacy = format!("web:{name}");
    let previous = documents
        .entries
        .iter()
        .find(|entry| entry["id"] == name || entry["id"] == legacy)
        .cloned();
    documents
        .entries
        .retain(|entry| entry["id"] != name && entry["id"] != legacy);
    if spec.is_some() {
        let mut entry = previous.unwrap_or_else(|| json!({"disabled":false}));
        entry["id"] = json!(name);
        entry["name"] = json!(name);
        documents.entries.push(entry);
    }
    Ok(documents)
}

fn github_source(spec: &str) -> Result<(String, &str), String> {
    let source = spec.strip_prefix("github:").ok_or_else(|| {
        "dsh: only github:owner/repo#<40-character-commit> is supported".to_string()
    })?;
    let (repo, reference) = source.split_once('#').ok_or_else(|| {
        "dsh: GitHub plugins require an immutable 40-character commit SHA".to_string()
    })?;
    if reference.len() != 40 || !reference.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("dsh: GitHub plugin ref must be a full 40-character commit SHA".to_string());
    }
    let parts: Vec<_> = repo.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || !part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        })
    {
        return Err("dsh: invalid GitHub plugin source".to_string());
    }
    Ok((format!("https://github.com/{repo}.git"), reference))
}

fn add(profile: &Profile, spec: &str) -> Result<(), String> {
    let (url, reference) = github_source(spec)?;
    profile.documents()?;
    let operation = profile.operation_dir()?;
    let staging = operation.join("download");
    let result = (|| {
        for (cwd, args) in [
            (
                profile.root(),
                vec![
                    "clone".to_string(),
                    "--no-checkout".into(),
                    "--depth".into(),
                    "1".into(),
                    url,
                    staging.to_string_lossy().into_owned(),
                ],
            ),
            (
                staging.as_path(),
                vec![
                    "fetch".into(),
                    "--depth".into(),
                    "1".into(),
                    "origin".into(),
                    reference.into(),
                ],
            ),
            (
                staging.as_path(),
                vec!["checkout".into(), "--detach".into(), "FETCH_HEAD".into()],
            ),
        ] {
            let mut command = Command::new("git");
            command
                .current_dir(cwd)
                .args(args)
                .env("GIT_TERMINAL_PROMPT", "0");
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000);
            }
            let status = command
                .status()
                .map_err(|error| format!("dsh: failed to run Git: {error}"))?;
            if !status.success() {
                return Err(format!("dsh: plugin Git operation failed: {status}"));
            }
        }
        let (name, has_host) = validate_web_plugin(&staging)?;
        let documents = update_documents(profile.documents()?, &name, Some(spec))?;
        profile.replace(documents, Some((&name, Some(&staging))))?;
        println!("installed {name} (pure Web client)");
        if has_host {
            eprintln!(
                "dsh: {name} also declares a Node Host bundle; only its Web client was installed"
            );
        }
        Ok(())
    })();
    if let Err(cleanup) = profile.discard_stage(&operation) {
        if let Err(error) = result {
            return Err(format!("{error}; staging cleanup failed: {cleanup}"));
        }
        eprintln!("dsh: plugin committed; staging cleanup needs retry: {cleanup}");
    }
    result
}

fn remove(profile: &Profile, name: &str) -> Result<(), String> {
    package_dir(profile.root(), name)?;
    let documents = update_documents(profile.documents()?, name, None)?;
    profile.replace(documents, Some((name, None)))?;
    println!("removed {name}");
    Ok(())
}

fn list(profile: &Path) -> Result<(), String> {
    let manifest = read_manifest(&profile.join("package.json"))?;
    let dependencies = manifest
        .get("dependencies")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (name, spec) in dependencies {
        println!("{name}\t{}", spec.as_str().unwrap_or(""));
    }
    Ok(())
}

pub fn run(profile: &Path, args: &[String]) -> Result<(), String> {
    if args.len() == 1 && args[0] == "list" {
        return list(profile);
    }
    let mut profile = Profile::open(profile)?;
    if let Ok(operation_id) = std::env::var("DSH_PLUGIN_OPERATION_ID") {
        profile.set_operation(dsh_app_boot::plugin_profile::OperationTag {
            operation_id,
            action: args.first().cloned().unwrap_or_default(),
            spec: args.get(1).cloned().unwrap_or_default(),
        })?;
    }
    match args {
        [command,spec] if command=="add"=>add(&profile,spec),
        [command,name] if command=="remove"=>remove(&profile,name),
        [command] if command=="list"=>list(profile.root()),
        [command] if command=="recover"=>{profile.restore_last_good()?;println!("restored last validated plugin configuration; rejected documents retained");Ok(())},
        _=>Err("usage: dsh plugin --profile <name> add github:owner/repo#<commit> | remove <package> | list | recover".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plugin_upgrade_preserves_disabled_state_and_user_configuration() {
        let documents = Documents {
            manifest: json!({"dependencies":{"demo":"old"},"kept":true}),
            entries: vec![
                json!({"id":"web:demo","name":"demo","disabled":true,"config":{"workspacePanel":"keep"}}),
                json!({"id":"unrelated","name":"unrelated"}),
            ],
        };
        let next = update_documents(documents, "demo", Some("new")).unwrap();
        assert_eq!(next.manifest["kept"], true);
        assert_eq!(next.manifest["dependencies"]["demo"], "new");
        let plugin = next
            .entries
            .iter()
            .find(|entry| entry["id"] == "demo")
            .unwrap();
        assert_eq!(plugin["disabled"], true);
        assert_eq!(plugin["config"]["workspacePanel"], "keep");
        assert!(next.entries.iter().any(|entry| entry["id"] == "unrelated"));
    }
}
