//! User-owned tool declaration settings, separate from environment and permission choices.
use axum::body::{Body, to_bytes};
use dsh_host_webserver::{RouteDisposer, WebRoute, WebRouteKind, WebServer};
use dsh_tools::{ToolRuntime, discovery::DiscoveryConfig};
use http::{Method, Response, StatusCode, header};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc};

struct Settings {
    path: PathBuf,
    tools: Arc<ToolRuntime>,
    gate: tokio::sync::Mutex<()>,
}
impl Settings {
    async fn stored(&self) -> Result<(DiscoveryConfig, String), String> {
        dsh_workspace_resources::checked_path(&self.path)?;
        let config = match tokio::fs::metadata(&self.path).await {
            Ok(m) if m.len() <= 64 * 1024 => serde_json::from_slice::<DiscoveryConfig>(
                &tokio::fs::read(&self.path)
                    .await
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?,
            Ok(_) => return Err("工具发现配置超过 64 KiB".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => DiscoveryConfig::default(),
            Err(e) => return Err(e.to_string()),
        };
        config.validate()?;
        let revision = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&config).map_err(|e| e.to_string())?)
        );
        Ok((config, revision))
    }
    async fn snapshot(&self) -> Result<Value, String> {
        let (config, revision) = self.stored().await?;
        let runtime = self.tools.discovery_diagnostics();
        let saved = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        let pending = if config.enabled {
            runtime["effectiveConfig"] != saved
        } else {
            runtime["enabled"] != false
        };
        Ok(
            json!({"configuration":config,"revision":revision,"runtime":runtime,"restartRequired":pending,"environmentCacheUnaffected":true,"permissionsUnaffected":true}),
        )
    }
    async fn save(&self, payload: &Value) -> Result<Value, String> {
        let _gate = self.gate.lock().await;
        let (_, revision) = self.stored().await?;
        if payload["expectedRevision"].as_str() != Some(revision.as_str()) {
            return Err("配置已变化，请刷新后重试".into());
        }
        let config: DiscoveryConfig =
            serde_json::from_value(payload["configuration"].clone()).map_err(|e| e.to_string())?;
        config.validate()?;
        let bytes = serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?;
        if bytes.len() > 64 * 1024 {
            return Err("工具发现配置超过 64 KiB".into());
        }
        dsh_atomic_write::write_file_atomic(
            &self.path,
            &bytes,
            dsh_atomic_write::WriteFileAtomicOptions {
                mode: 0o600,
                dir_mode: Some(0o700),
            },
        )
        .await
        .map_err(|e| e.to_string())?;
        self.snapshot().await
    }
}

pub(crate) fn register(
    server: &Arc<WebServer>,
    root: &std::path::Path,
    tools: Arc<ToolRuntime>,
    allow_remote: bool,
) -> RouteDisposer {
    let service = Arc::new(Settings {
        path: root.join("tool-discovery.json"),
        tools,
        gate: tokio::sync::Mutex::new(()),
    });
    server.register(WebRoute {
        kind: WebRouteKind::Exact,
        path: "/__dsh-tool-discovery".into(),
        handler: Arc::new(move |request| {
            let service = service.clone();
            Box::pin(async move {
                let trusted = request
                    .headers()
                    .get(header::HOST)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|host| super::allowed_web_authority(host, allow_remote))
                    && super::trusted_web_request(&request, allow_remote);
                let method = request.method().clone();
                let (status, result) = if !trusted {
                    (StatusCode::FORBIDDEN, json!({"error":"forbidden"}))
                } else if method == Method::GET {
                    match service.snapshot().await {
                        Ok(v) => (StatusCode::OK, v),
                        Err(e) => (StatusCode::BAD_REQUEST, json!({"error":e})),
                    }
                } else if method == Method::POST {
                    let result = async {
                        let bytes = to_bytes(Body::new(request.into_body()), 64 * 1024)
                            .await
                            .map_err(|e| e.to_string())?;
                        let payload: Value =
                            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                        service.save(&payload).await
                    }
                    .await;
                    match result {
                        Ok(v) => (StatusCode::OK, v),
                        Err(e) => (StatusCode::BAD_REQUEST, json!({"error":e})),
                    }
                } else {
                    (
                        StatusCode::METHOD_NOT_ALLOWED,
                        json!({"error":"GET or POST required"}),
                    )
                };
                Ok(Response::builder()
                    .status(status)
                    .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                    .header(header::CACHE_CONTROL, "no-store")
                    .body(serde_json::to_vec(&result).unwrap_or_default().into())
                    .unwrap())
            })
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn setting_changes_require_revision_and_keep_runtime_truth() {
        let root =
            std::env::temp_dir().join(format!("discovery-settings-{}", uuid::Uuid::new_v4()));
        let context = cordis::Context::root();
        let tools = ToolRuntime::install(&context, Default::default()).unwrap();
        let service = Settings {
            path: root.join("tool-discovery.json"),
            tools,
            gate: tokio::sync::Mutex::new(()),
        };
        let before = service.snapshot().await.unwrap();
        let mut config = before["configuration"].clone();
        config["enabled"] = false.into();
        assert!(
            service
                .save(&json!({"expectedRevision":"wrong","configuration":config}))
                .await
                .is_err()
        );
        let after = service
            .save(&json!({"expectedRevision":before["revision"],"configuration":config}))
            .await
            .unwrap();
        assert_eq!(after["configuration"]["enabled"], false);
        assert_eq!(after["runtime"]["enabled"], false);
        assert_eq!(after["environmentCacheUnaffected"], true);
        assert!(
            service
                .save(&json!({"expectedRevision":before["revision"],"configuration":config}))
                .await
                .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
