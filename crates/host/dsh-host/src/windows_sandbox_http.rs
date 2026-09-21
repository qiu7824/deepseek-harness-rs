use serde_json::json;
use std::{path::PathBuf, sync::Arc};

pub(super) fn register(
    server: &Arc<dsh_host_webserver::WebServer>,
    home: PathBuf,
    remote: bool,
) -> dsh_host_webserver::RouteDisposer {
    use axum::body::{Body, to_bytes};
    use http::{Method, Response, StatusCode, header};
    server.register(dsh_host_webserver::WebRoute {
        kind: dsh_host_webserver::WebRouteKind::Prefix,
        path: "/__dsh-windows-sandbox".into(),
        handler: Arc::new(move |request| {
            let home = home.clone();
            Box::pin(async move {
                let trusted = request
                    .headers()
                    .get(header::HOST)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|host| super::allowed_web_authority(host, remote))
                    && super::trusted_web_request(&request, remote);
                let (status, value) = if !trusted {
                    (StatusCode::FORBIDDEN, json!({"error":"forbidden"}))
                } else {
                    #[cfg(windows)]
                    let result = if request.method() == Method::GET {
                        dsh_sandbox_local::windows_backend_configuration(&home)
                    } else if request.method() == Method::POST {
                        async {
                            let bytes = to_bytes(Body::new(request.into_body()), 16 * 1024)
                                .await
                                .map_err(|e| e.to_string())?;
                            let args = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                            dsh_sandbox_local::windows_backend_manage(home, args).await
                        }
                        .await
                    } else {
                        Err("GET or POST required".into())
                    };
                    #[cfg(not(windows))]
                    let result: Result<serde_json::Value, String> = {
                        let _ = (home, request);
                        Ok(json!({"available":false}))
                    };
                    match result {
                        Ok(value) => (StatusCode::OK, value),
                        Err(error) => (StatusCode::BAD_REQUEST, json!({"error":error})),
                    }
                };
                Ok(Response::builder()
                    .status(status)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::CACHE_CONTROL, "no-store")
                    .body(Body::from(value.to_string()))
                    .expect("Windows sandbox response"))
            })
        }),
    })
}
