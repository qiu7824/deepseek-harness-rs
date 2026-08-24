use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value, json};

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

fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("create {}: {error}", destination.display()))?;
    for entry in
        std::fs::read_dir(source).map_err(|error| format!("read {}: {error}", source.display()))?
    {
        let entry = entry.map_err(|error| format!("read plugin entry: {error}"))?;
        let kind = entry
            .file_type()
            .map_err(|error| format!("stat plugin entry: {error}"))?;
        let target = destination.join(entry.file_name());
        if kind.is_symlink() {
            return Err(format!(
                "dsh: plugin contains unsupported symlink {}",
                entry.path().display()
            ));
        }
        if kind.is_dir() {
            if entry.file_name() == ".git" {
                continue;
            }
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)
                .map_err(|error| format!("copy {}: {error}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn update_plugin_inventory(profile: &Path, name: &str, installed: bool) -> Result<(), String> {
    let path = profile.join("plugins.json");
    let mut entries: Vec<Value> = if path.is_file() {
        serde_json::from_slice(
            &std::fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?,
        )
        .map_err(|error| format!("parse {}: {error}", path.display()))?
    } else {
        Vec::new()
    };
    let legacy_entry_id = format!("web:{name}");
    entries.retain(|entry| {
        let id = entry.get("id").and_then(Value::as_str);
        id != Some(name) && id != Some(&legacy_entry_id)
    });
    if installed {
        entries.push(json!({"id": name, "name": name, "disabled": false}));
    }
    let bytes = serde_json::to_vec_pretty(&entries)
        .map_err(|error| format!("encode plugin inventory: {error}"))?;
    std::fs::write(&path, bytes).map_err(|error| format!("write {}: {error}", path.display()))
}

fn update_dependency(profile: &Path, name: &str, spec: Option<&str>) -> Result<(), String> {
    let path = profile.join("package.json");
    let mut manifest = read_manifest(&path)?;
    let object = manifest
        .as_object_mut()
        .ok_or_else(|| "dsh: profile package.json must be an object".to_string())?;
    let dependencies = object
        .entry("dependencies")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| "dsh: profile dependencies must be an object".to_string())?;
    if let Some(spec) = spec {
        dependencies.insert(name.to_string(), Value::String(spec.to_string()));
    } else {
        dependencies.remove(name);
    }
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("encode profile manifest: {error}"))?;
    std::fs::write(&path, bytes).map_err(|error| format!("write {}: {error}", path.display()))
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

fn add(profile: &Path, spec: &str) -> Result<(), String> {
    let (url, reference) = github_source(spec)?;
    let staging = profile.join(format!(".dsh-plugin-stage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let status = Command::new("git")
        .args(["clone", "--depth", "1", &url])
        .arg(&staging)
        .status()
        .map_err(|error| format!("dsh: failed to start git: {error}"))?;
    if !status.success() {
        return Err(format!("dsh: git clone failed with {status}"));
    }
    let fetch = Command::new("git")
        .current_dir(&staging)
        .args(["fetch", "--depth", "1", "origin", reference])
        .status()
        .map_err(|error| format!("dsh: failed to fetch plugin commit: {error}"))?;
    if !fetch.success() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("dsh: git fetch {reference:?} failed"));
    }
    let checkout = Command::new("git")
        .current_dir(&staging)
        .args(["checkout", "--detach", "FETCH_HEAD"])
        .status()
        .map_err(|error| format!("dsh: failed to checkout plugin commit: {error}"))?;
    if !checkout.success() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("dsh: git checkout {reference:?} failed"));
    }
    let (name, has_host) = validate_web_plugin(&staging)?;
    let target = package_dir(profile, &name)?;
    let install = profile.join(format!(".dsh-plugin-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&install);
    copy_tree(&staging, &install)?;
    let _ = std::fs::remove_dir_all(&staging);
    if target.exists() {
        std::fs::remove_dir_all(&target)
            .map_err(|error| format!("remove old {}: {error}", target.display()))?;
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    std::fs::rename(&install, &target)
        .map_err(|error| format!("install {}: {error}", target.display()))?;
    update_dependency(profile, &name, Some(spec))?;
    update_plugin_inventory(profile, &name, true)?;
    println!("installed {name} (pure Web client)");
    if has_host {
        eprintln!(
            "dsh: note: {name} also declares a Node Host bundle; Rust loads its Web client only"
        );
    }
    Ok(())
}

fn remove(profile: &Path, name: &str) -> Result<(), String> {
    let target = package_dir(profile, name)?;
    if target.exists() {
        let root = profile
            .join("node_modules")
            .canonicalize()
            .map_err(|error| format!("canonicalize plugin root: {error}"))?;
        let canonical = target
            .canonicalize()
            .map_err(|error| format!("canonicalize plugin target: {error}"))?;
        if canonical == root || !canonical.starts_with(&root) {
            return Err("dsh: refusing to remove a path outside the plugin root".to_string());
        }
        std::fs::remove_dir_all(&target)
            .map_err(|error| format!("remove {}: {error}", target.display()))?;
    }
    update_dependency(profile, name, None)?;
    update_plugin_inventory(profile, name, false)?;
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
    match args {
        [command, spec] if command == "add" => add(profile, spec),
        [command, name] if command == "remove" => remove(profile, name),
        [command] if command == "list" => list(profile),
        _ => Err("usage: dsh plugin --profile <name> add github:owner/repo[#ref] | remove <package> | list".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_package_names_and_rejects_unsafe_exports() {
        assert!(valid_package_name("pkg"));
        assert!(valid_package_name("@scope/pkg"));
        assert!(!valid_package_name("../pkg"));
        let root = std::env::temp_dir().join(format!("dsh-native-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("package.json"),json!({"name":"safe","exports":{"./client":"../bad.js"},"dsh":{"client":{"platform":"web"}}}).to_string()).unwrap();
        assert!(validate_web_plugin(&root).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
