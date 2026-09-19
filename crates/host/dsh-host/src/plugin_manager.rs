//! User-only plugin installation through the existing pure-Web CLI validator.
use axum::body::{Body, to_bytes};
use dsh_host_webserver::{RouteDisposer, WebRoute, WebRouteKind, WebServer};
use http::{Method, Response, StatusCode, header};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};

pub(crate) fn register(
    server: &Arc<WebServer>,
    home: PathBuf,
    profile: String,
    allow_remote: bool,
) -> RouteDisposer {
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    server.register(WebRoute {
        kind: WebRouteKind::Exact,
        path: "/__dsh-plugin-manager".into(),
        handler: Arc::new(move |request| {
            let home = home.clone();
            let profile = profile.clone();
            let gate = gate.clone();
            Box::pin(async move {
                let trusted = request
                    .headers()
                    .get(header::HOST)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|host| super::allowed_web_authority(host, allow_remote))
                    && super::trusted_web_request(&request, allow_remote);
                let (status, value) = if !trusted {
                    (StatusCode::FORBIDDEN, json!({"error":"forbidden"}))
                } else if request.method() != Method::POST {
                    (
                        StatusCode::METHOD_NOT_ALLOWED,
                        json!({"error":"POST required"}),
                    )
                } else {
                    let result = async {
                        let bytes = to_bytes(Body::new(request.into_body()), 4096)
                            .await
                            .map_err(|e| e.to_string())?;
                        let input: Value =
                            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                        let action = input["action"].as_str().ok_or("Missing action")?;
                        let spec = input["spec"]
                            .as_str()
                            .filter(|v| {
                                !v.is_empty() && v.len() <= 400 && !v.chars().any(char::is_control)
                            })
                            .ok_or("Invalid package specification")?;
                        if !matches!(action, "add" | "remove") {
                            return Err("Unsupported plugin operation".into());
                        }
                        if action == "add" && !spec.starts_with("github:") {
                            return Err(
                                "安装来源须为 github:owner/repo#commit；仅支持纯 Web 插件".into()
                            );
                        }
                        let _guard = gate.try_lock().map_err(|_| "另一项插件操作正在执行")?;
                        let mut command = tokio::process::Command::new(
                            std::env::current_exe().map_err(|e| e.to_string())?,
                        );
                        command
                            .args(["plugin", "--profile", &profile, action, spec])
                            .env("DSH_HOME", home)
                            .kill_on_drop(true);
                        #[cfg(windows)]
                        command.creation_flags(0x08000000);
                        let output =
                            tokio::time::timeout(Duration::from_secs(120), command.output())
                                .await
                                .map_err(|_| "插件操作超时；请检查已安装状态后重试")?
                                .map_err(|e| e.to_string())?;
                        let log = format!(
                            "{}\n{}",
                            String::from_utf8_lossy(&output.stdout),
                            String::from_utf8_lossy(&output.stderr)
                        )
                        .chars()
                        .take(4096)
                        .collect::<String>();
                        if !output.status.success() {
                            return Err(log);
                        }
                        Ok::<_, String>(
                            json!({"ok":true,"restartRequired":true,"log":log,"profile":profile}),
                        )
                    }
                    .await;
                    match result {
                        Ok(value) => (StatusCode::OK, value),
                        Err(error) => (StatusCode::BAD_REQUEST, json!({"error":error})),
                    }
                };
                Ok(Response::builder()
                    .status(status)
                    .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                    .header(header::CACHE_CONTROL, "no-store")
                    .body(Body::from(value.to_string()))
                    .expect("plugin manager response"))
            })
        }),
    })
}
