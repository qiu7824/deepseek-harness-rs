//! Authenticated user-only plugin operations with observable cancellation and recovery.
mod operations;
use axum::body::{Body, to_bytes};
use cordis::Context;
use dsh_host_webserver::{RouteDisposer, WebRoute, WebRouteKind, WebServer};
use http::{Method, Response, StatusCode, header};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};

pub(crate) fn register(
    ctx: &Context,
    server: &Arc<WebServer>,
    home: PathBuf,
    profile: String,
    runtime: Arc<dyn dsh_subprocess::SubprocessRuntime>,
    api: Arc<dsh_host_apiproxy::proxy::ApiProxyService>,
    allow_remote: bool,
) -> RouteDisposer {
    let manager = operations::Manager::install(ctx, home, profile, runtime, api);
    server.register(WebRoute {
        kind: WebRouteKind::Exact,
        path: "/__dsh-plugin-manager".into(),
        handler: Arc::new(move |request| {
            let manager = manager.clone();
            Box::pin(async move {
                let trusted = request
                    .headers()
                    .get(header::HOST)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|host| super::allowed_web_authority(host, allow_remote))
                    && super::trusted_web_request(&request, allow_remote);
                let (status, value) = if !trusted {
                    (StatusCode::FORBIDDEN, json!({"error":"请求来源不可信"}))
                } else if request.method() != Method::POST {
                    (
                        StatusCode::METHOD_NOT_ALLOWED,
                        json!({"error":"POST required"}),
                    )
                } else {
                    let result = async {
                        let bytes = to_bytes(Body::new(request.into_body()), 4096)
                            .await
                            .map_err(|error| error.to_string())?;
                        let input: Value =
                            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
                        manager.dispatch(input)
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
