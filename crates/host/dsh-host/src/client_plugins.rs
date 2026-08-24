use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use dsh_host_webserver::{WebHandlerError, WebResponse, WebRoute, WebRouteKind, WebServer};
use http::{Method, Response, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const MAX_CLIENT_BYTES: u64 = 2 * 1024 * 1024;

fn copy_bundled_tree(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("create {}: {error}", destination.display()))?;
    for entry in
        std::fs::read_dir(source).map_err(|error| format!("read {}: {error}", source.display()))?
    {
        let entry = entry.map_err(|error| format!("read bundled plugin: {error}"))?;
        let kind = entry
            .file_type()
            .map_err(|error| format!("stat bundled plugin: {error}"))?;
        let target = destination.join(entry.file_name());
        if kind.is_symlink() {
            return Err(format!(
                "bundled plugin contains a symlink: {}",
                entry.path().display()
            ));
        }
        if kind.is_dir() {
            copy_bundled_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target)
                .map_err(|error| format!("copy bundled plugin: {error}"))?;
        }
    }
    Ok(())
}

pub fn materialize_bundled(profile: &Path) -> Result<(), String> {
    let Some(root) = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("plugins")))
        .filter(|path| path.is_dir())
    else {
        return Ok(());
    };
    let package_path = profile.join("package.json");
    let mut profile_manifest: Value = serde_json::from_slice(
        &std::fs::read(&package_path)
            .map_err(|error| format!("read {}: {error}", package_path.display()))?,
    )
    .map_err(|error| format!("parse {}: {error}", package_path.display()))?;
    let dependencies = profile_manifest
        .as_object_mut()
        .ok_or_else(|| "profile package.json must be an object".to_string())?
        .entry("dependencies")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "profile dependencies must be an object".to_string())?;
    let inventory_path = profile.join("plugins.json");
    let mut inventory: Vec<Value> = std::fs::read(&inventory_path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default();
    let mut changed = false;
    for entry in
        std::fs::read_dir(&root).map_err(|error| format!("read {}: {error}", root.display()))?
    {
        let entry = entry.map_err(|error| format!("read bundled plugin: {error}"))?;
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let manifest_path = entry.path().join("package.json");
        let Ok(raw) = std::fs::read(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<Value>(&raw) else {
            continue;
        };
        let Some(name) = manifest.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(destination) = package_directory(profile, name) else {
            continue;
        };
        if !destination.exists() {
            copy_bundled_tree(&entry.path(), &destination)?;
            changed = true;
        }
        if !dependencies.contains_key(name) {
            dependencies.insert(name.to_string(), Value::String("bundled".to_string()));
            changed = true;
        }
        if !inventory
            .iter()
            .any(|item| item.get("id").and_then(Value::as_str) == Some(name))
        {
            inventory.push(json!({"id": name, "name": name, "disabled": false}));
            changed = true;
        }
    }
    if changed {
        std::fs::write(
            &package_path,
            serde_json::to_vec_pretty(&profile_manifest).map_err(|e| e.to_string())?,
        )
        .map_err(|error| format!("write {}: {error}", package_path.display()))?;
        std::fs::write(
            &inventory_path,
            serde_json::to_vec_pretty(&inventory).map_err(|e| e.to_string())?,
        )
        .map_err(|error| format!("write {}: {error}", inventory_path.display()))?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientPlugin {
    pub id: String,
    pub route: String,
    pub rev: String,
    pub inject: Vec<String>,
    pub source: PathBuf,
}

fn valid_package_segment(segment: &str, allow_scope: bool) -> bool {
    let segment = if allow_scope {
        segment.strip_prefix('@').unwrap_or("")
    } else {
        segment
    };
    !matches!(segment, "" | "." | "..")
        && segment
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn package_directory(profile: &Path, name: &str) -> Option<PathBuf> {
    let parts: Vec<_> = name.split('/').collect();
    let valid = match parts.as_slice() {
        [plain] => valid_package_segment(plain, false),
        [scope, plain] => valid_package_segment(scope, true) && valid_package_segment(plain, false),
        _ => false,
    };
    if !valid {
        return None;
    }
    let mut path = profile.join("node_modules");
    for part in parts {
        path.push(part);
    }
    Some(path)
}

fn client_export(value: &Value) -> Option<&str> {
    let export = value.get("exports")?.get("./client")?;
    match export {
        Value::String(path) => Some(path),
        Value::Object(object) => object
            .get("browser")
            .or_else(|| object.get("default"))
            .and_then(Value::as_str),
        _ => None,
    }
}

fn inside_package(package: &Path, relative: &str) -> Option<PathBuf> {
    let relative = relative.strip_prefix("./").unwrap_or(relative);
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
        return None;
    }
    let root = package.canonicalize().ok()?;
    let target = package.join(path).canonicalize().ok()?;
    target.starts_with(&root).then_some(target)
}

pub fn discover(profile: &Path) -> Result<Vec<ClientPlugin>, String> {
    let manifest_path = profile.join("package.json");
    let raw = match std::fs::read(&manifest_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read {}: {error}", manifest_path.display())),
    };
    let manifest: Value = serde_json::from_slice(&raw)
        .map_err(|error| format!("parse {}: {error}", manifest_path.display()))?;
    let dependencies = manifest
        .get("dependencies")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut plugins = Vec::new();
    for name in dependencies.keys() {
        let Some(package) = package_directory(profile, name) else {
            continue;
        };
        let package_json = package.join("package.json");
        let Ok(raw) = std::fs::read(&package_json) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<Value>(&raw) else {
            eprintln!("dsh: skipping plugin {name:?}: invalid package.json");
            continue;
        };
        let client = manifest.get("dsh").and_then(|dsh| dsh.get("client"));
        if client
            .and_then(|client| client.get("platform"))
            .and_then(Value::as_str)
            != Some("web")
        {
            if manifest
                .get("dsh")
                .and_then(|dsh| dsh.get("bundle"))
                .is_some()
            {
                eprintln!(
                    "dsh: plugin {name:?} has no Rust-compatible web client entry; host-side Node plugins are not executed"
                );
            }
            continue;
        }
        let Some(export) = client_export(&manifest) else {
            eprintln!("dsh: skipping client plugin {name:?}: exports[\"./client\"] is missing");
            continue;
        };
        let Some(source) = inside_package(&package, export) else {
            eprintln!("dsh: skipping client plugin {name:?}: unsafe or missing client export");
            continue;
        };
        let metadata = std::fs::metadata(&source)
            .map_err(|error| format!("stat {}: {error}", source.display()))?;
        if metadata.len() > MAX_CLIENT_BYTES {
            eprintln!("dsh: skipping client plugin {name:?}: client bundle exceeds 2 MiB");
            continue;
        }
        let bytes = std::fs::read(&source)
            .map_err(|error| format!("read {}: {error}", source.display()))?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let inject = client
            .and_then(|client| client.get("inject"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        plugins.push(ClientPlugin {
            id: name.clone(),
            route: format!("/plugins/external/{}.js", &digest[..16]),
            rev: digest[..16].to_string(),
            inject,
            source,
        });
    }
    plugins.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(plugins)
}

fn disabled_plugins(profile: &Path) -> std::collections::HashSet<String> {
    let path = profile.join("plugins.json");
    let Ok(raw) = std::fs::read(path) else {
        return std::collections::HashSet::new();
    };
    serde_json::from_slice::<Vec<Value>>(&raw)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.get("disabled").and_then(Value::as_bool) == Some(true))
        .filter_map(|entry| entry.get("id").and_then(Value::as_str).map(str::to_string))
        .collect()
}

pub fn compose(
    web_server: &Arc<WebServer>,
    boot_payload: &mut Value,
    profile: &Path,
) -> Result<Vec<dsh_host_webserver::RouteDisposer>, String> {
    let disabled = disabled_plugins(profile);
    let plugins = discover(profile)?
        .into_iter()
        .filter(|plugin| !disabled.contains(&plugin.id));
    let entries = boot_payload
        .get_mut("entries")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "web plugin manifest entries are absent".to_string())?;
    let mut disposers = Vec::new();
    for plugin in plugins {
        let bytes = Arc::new(
            std::fs::read(&plugin.source)
                .map_err(|error| format!("read {}: {error}", plugin.source.display()))?,
        );
        let route = plugin.route.clone();
        let handler = Arc::new(move |request: dsh_host_webserver::WebRequest| {
            let bytes = bytes.clone();
            Box::pin(async move {
                if !matches!(*request.method(), Method::GET | Method::HEAD) {
                    return Ok(Response::builder()
                        .status(StatusCode::METHOD_NOT_ALLOWED)
                        .body(Body::empty())
                        .expect("plugin method response"));
                }
                let body = if request.method() == Method::HEAD {
                    Body::empty()
                } else {
                    Body::from(bytes.as_ref().clone())
                };
                Ok::<WebResponse, WebHandlerError>(
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(http::header::CONTENT_TYPE, "text/javascript; charset=utf-8")
                        .header(http::header::CACHE_CONTROL, "no-store")
                        .body(body)
                        .expect("plugin response"),
                )
            })
                as futures::future::BoxFuture<'static, Result<WebResponse, WebHandlerError>>
        });
        disposers.push(web_server.register(WebRoute {
            kind: WebRouteKind::Exact,
            path: route.clone(),
            handler,
        }));
        entries.push(json!({
            "id": plugin.id,
            "url": route,
            "rev": plugin.rev,
            "inject": plugin.inject,
            "immediately": false
        }));
    }
    Ok(disposers)
}
