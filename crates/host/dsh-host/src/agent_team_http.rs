//! Same-origin collaboration controls over the shared team runtime.
use axum::body::{Body, to_bytes};
use dsh_host_webserver::{RouteDisposer, WebRoute, WebRouteKind, WebServer};
use http::{Method, Response, StatusCode, header};
use serde_json::json;
use std::sync::Arc;

pub fn register(
    server: &Arc<WebServer>,
    teams: Option<Arc<dsh_agent_team::AgentTeams>>,
    api: Arc<dsh_host_apiproxy::proxy::ApiProxyService>,
    allow_remote_host: bool,
) -> RouteDisposer {
    server.register(WebRoute {
        kind: WebRouteKind::Prefix,
        path: "/__dsh-agent-team".into(),
        handler: Arc::new(move |request| {
            let teams = teams.clone();
            let api = api.clone();
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
                    match to_bytes(Body::new(request.into_body()), 65_536)
                        .await
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    {
                        Some(value)
                            if value["sessionId"]
                                .as_str()
                                .is_some_and(|id| !id.is_empty() && id.len() <= 200) =>
                        {
                            let id=value["sessionId"].as_str().unwrap();
                            let action=value.get("action").and_then(|v|v.as_str()).unwrap_or("view");
                            let result=if action=="view" {teams.view(id).await} else if action=="control" {
                                match api.resolve_control_agent(id).await {
                                    Ok(lease)=>teams.control(lease.agent.clone(),value.get("arguments").cloned().unwrap_or_else(||json!({}))).await,
                                    Err(error)=>Err(error),
                                }
                            } else {Err("unknown collaboration operation".into())};
                            match result {
                                Ok(board) => {
                                    let settings=teams.settings();
                                    (StatusCode::OK, json!({"enabled":settings.enabled,"settings":settings,"board":board}))
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

pub fn settings_schema() -> dsh_schemastery::Schema {
    use dsh_schemastery::{Data,Schema};
    let text=||Schema::string().default(Data::String(String::new()));
    let role=Schema::object(indexmap::IndexMap::from([
        ("id".into(),Schema::string()),("name".into(),Schema::string()),("instructions".into(),text()),
        ("provider".into(),text()),("model".into(),text()),("reasoningEffort".into(),text()),
        ("maxTokens".into(),Schema::union(vec![Schema::number().min(1.0).max(1_000_000.0).step(1.0),Schema::constant(Data::Null)]).default(Data::Null)),
        ("allowTools".into(),Schema::array(Schema::string()).default(Data::Array(vec![]))),
        ("canSpawn".into(),Schema::boolean().default(Data::Bool(false))),
    ]));
    let profile=Schema::object(indexmap::IndexMap::from([("id".into(),Schema::string()),("name".into(),Schema::string()),("roles".into(),Schema::array(role))]));
    Schema::object(indexmap::IndexMap::from([
        ("enabled".into(),Schema::boolean().default(Data::Bool(false))),
        ("maxMembers".into(),Schema::number().min(1.0).max(16.0).step(1.0).default(Data::Number(8.0))),
        ("showButton".into(),Schema::boolean().default(Data::Bool(true))),
        ("defaultMode".into(),Schema::union(["off","auto","custom"].map(|v|Schema::constant(Data::String(v.into()))).to_vec()).default(Data::String("off".into()))),
        ("defaultProfile".into(),text()),
        ("profiles".into(),Schema::array(profile).default(Data::Array(vec![]))),
    ]))
}
