//! Read-only, same-origin team board for the conversation UI.
use axum::body::{Body, to_bytes};
use dsh_host_webserver::{RouteDisposer, WebRoute, WebRouteKind, WebServer};
use http::{Method, Response, StatusCode, header};
use serde_json::json;
use std::sync::Arc;

pub fn register(
    server: &Arc<WebServer>,
    teams: Option<Arc<dsh_agent_team::AgentTeams>>,
    allow_remote_host: bool,
) -> RouteDisposer {
    server.register(WebRoute {
        kind: WebRouteKind::Prefix,
        path: "/__dsh-agent-team".into(),
        handler: Arc::new(move |request| {
            let teams = teams.clone();
            Box::pin(async move {
                let allowed = request
                    .headers()
                    .get(header::HOST)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|host| super::allowed_web_authority(host, allow_remote_host))
                    && super::trusted_web_request(&request, allow_remote_host);
                let (status, value) = if !allowed {
                    (StatusCode::FORBIDDEN, json!({"error":"forbidden"}))
                } else if request.method() != Method::POST {
                    (
                        StatusCode::METHOD_NOT_ALLOWED,
                        json!({"error":"POST required"}),
                    )
                } else if let Some(teams) = teams {
                    match to_bytes(Body::new(request.into_body()), 4096)
                        .await
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    {
                        Some(value)
                            if value["sessionId"]
                                .as_str()
                                .is_some_and(|id| !id.is_empty() && id.len() <= 200) =>
                        {
                            match teams.view(value["sessionId"].as_str().unwrap()).await {
                                Ok(board) => {
                                    (StatusCode::OK, json!({"enabled":true,"board":board}))
                                }
                                Err(message) => (
                                    StatusCode::BAD_REQUEST,
                                    json!({"enabled":true,"error":message}),
                                ),
                            }
                        }
                        _ => (
                            StatusCode::BAD_REQUEST,
                            json!({"error":"invalid session request"}),
                        ),
                    }
                } else {
                    (StatusCode::OK, json!({"enabled":false}))
                };
                Ok(Response::builder()
                    .status(status)
                    .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                    .header(header::CACHE_CONTROL, "no-store")
                    .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
                    .body(Body::from(value.to_string()))
                    .expect("team board response"))
            })
        }),
    })
}
