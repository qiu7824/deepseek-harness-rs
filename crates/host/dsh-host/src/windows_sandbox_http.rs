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

#[cfg(windows)]
pub(super) fn install_tool(ctx: &cordis::Context, home: PathBuf) -> Result<(), String> {
    use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .ok_or("tools unavailable")?;
    let context = ctx.clone();
    tools.register(ctx,ToolDefinition{
        name:"environment_initialize".into(),description:"Initialize the configured Windows sandbox for this session's workspace when SANDBOX_SETUP_REQUIRED is reported. Uses only the installed verified helper and the existing selected implementation/network policy. No arbitrary commands or outside workspace can be supplied. Windows may request administrator authorization for dedicated-account setup. After success retry environment_validate; do not ask the user to navigate settings for routine project initialization.".into(),
        parameters:json!({"type":"object","properties":{},"additionalProperties":false}),output:ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,v|Ok(vec![dsh_llm::ContentBlock::Text{text:v.to_string()}])),presentation_meta:None},timeout_ms:Some(dsh_sandbox::startup::ENVIRONMENT_INITIALIZATION_TIMEOUT_MS),is_concurrency_safe:Some(Arc::new(|_|false)),finalize_content:None,present_call:None,present_result:None,
        execute:Arc::new(move |_,run|{let home=home.clone();let context=context.clone();let agent=run.agent.clone();let signal=run.signal.lock().clone();Box::pin(async move{
            let agent=agent.ok_or_else(||ToolBodyError::plain("Initialization requires a session"))?;
            let service=context.get_typed::<Arc<dsh_sandbox_policy::SandboxPolicyService>>("sandboxPolicy",false).ok_or_else(||ToolBodyError::plain("Sandbox policy unavailable"))?;
            let policy=service.try_resolve(&dsh_sandbox_policy::SandboxPolicyRequest{session:Some(Arc::new(agent.session().clone())),mode:None}).map_err(ToolBodyError::plain)?;
            let sandbox=context.get_typed::<Arc<dyn dsh_sandbox::SandboxProvider>>("sandbox",false).ok_or_else(||ToolBodyError::plain("Sandbox unavailable"))?;
            sandbox.prepare_cancellable(&policy, Some(signal)).await.map_err(|error| {
                let code=if error.contains("SANDBOX_SETUP_CANCELLED") {"CANCELLED"}
                    else if error.contains("SANDBOX_SETUP_TIMEOUT") {"SANDBOX_SETUP_TIMEOUT"}
                    else {"SANDBOX_SETUP_FAILED"};
                ToolBodyError::coded(error,"SandboxSetupError",code)
            })?;
            let mut value=dsh_sandbox_local::windows_backend_configuration(&home).map_err(ToolBodyError::plain)?;
            value["prepared"]=json!(true);
            value["scope"]=json!("effective-session-policy");
            value["sessionId"]=json!(policy.session_id);
            value["permissionMode"]=json!(policy.mode.as_str());
            Ok(value)
        })})
    })?;
    Ok(())
}
