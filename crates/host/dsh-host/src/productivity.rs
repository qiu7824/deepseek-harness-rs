//! Workspace task, local memory import, and connection diagnostic UI boundary.
use cordis::Context;
use dsh_schemastery::Schema;
use dsh_settings::{SettingsProvider, SettingsRegisterOptions, settings_namespace};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};
pub fn install(
    ctx: &Context,
    home: &Path,
    settings: &Arc<SettingsProvider>,
    registry: Arc<dsh_workspace::WorkspaceRegistry>,
    server: &Arc<dsh_host_webserver::WebServer>,
    allow_remote: bool,
    system_prompt: &Arc<dsh_system_prompt::SystemPrompt>,
) -> Result<dsh_host_webserver::RouteDisposer, String> {
    settings.register(
        ctx,
        settings_namespace("mini-menu")?,
        Schema::object(
            ["trajectory", "artifacts", "code-graph", "context"]
                .into_iter()
                .map(|key| {
                    (
                        key.into(),
                        Schema::boolean().default(dsh_schemastery::Data::Bool(true)),
                    )
                })
                .collect(),
        ),
        SettingsRegisterOptions::default(),
    )?;
    let board = Arc::new(super::project_tasks::Board::new(registry.clone()));
    board.install_tool(ctx)?;
    let summary_board = board.clone();
    system_prompt.context(
        ctx,
        dsh_system_prompt::PromptContext {
            name: "project:tasks".into(),
            order: 106.0,
            text: dsh_system_prompt::PromptText::Provider(Arc::new(move |assembly| {
                assembly
                    .field_str("cwd")
                    .map(|cwd| summary_board.summary(cwd))
                    .unwrap_or_default()
            })),
        },
    );
    let imports = Arc::new(super::memory_import::Imports::new(ctx, home, registry));
    imports.start(ctx);
    Ok(server.register(dsh_host_webserver::WebRoute{kind:dsh_host_webserver::WebRouteKind::Prefix,path:"/__dsh-productivity".into(),handler:Arc::new(move|request|{
        let board=board.clone();let imports=imports.clone();Box::pin(async move{
            let allowed=request.method()==http::Method::POST&&super::trusted_web_request(&request,allow_remote);
            let operation=request.uri().path().trim_start_matches("/__dsh-productivity/").to_string();
            let result:Result<Value,String>=async{
                if !allowed{return Err("forbidden".into())}
                let bytes=axum::body::to_bytes(axum::body::Body::new(request.into_body()),2*1024*1024).await.map_err(|_|"请求过大")?;
                let args:Value=serde_json::from_slice(&bytes).map_err(|_|"请求格式无效")?;
                if let Some(action)=operation.strip_prefix("tasks/"){return board.request(action,&args).await}
                if let Some(action)=operation.strip_prefix("memory/"){return imports.request(action,&args).await}
                if operation=="network"{
                    let client=dsh_http_proxy::builder()?.connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(20)).redirect(reqwest::redirect::Policy::none()).build().map_err(|e|e.to_string())?;
                    let start=std::time::Instant::now();
                    return match client.get("https://chatgpt.com/backend-api/codex/responses").send().await{Ok(response)=>Ok(json!({"reachable":true,"status":response.status().as_u16(),"elapsedMs":start.elapsed().as_millis(),"message":"已到达服务端；此检查不发送账号凭据，不验证模型或订阅权限"})),Err(error)=>{let error=error.without_url();let mut messages=vec![error.to_string()];let mut cause=std::error::Error::source(&error);for _ in 0..8 {let Some(error)=cause else{break};messages.push(error.to_string().chars().take(500).collect());cause=error.source();}Ok(json!({"reachable":false,"elapsedMs":start.elapsed().as_millis(),"message":messages.join("; ")}))}};
                }
                Err("未知操作".into())
            }.await;
            let (status,value)=match result{Ok(value)=>(200,value),Err(error)=>(if allowed{400}else{403},json!({"error":error}))};
            Ok(http::Response::builder().status(status).header("content-type","application/json").header("cache-control","no-store").body(axum::body::Body::from(value.to_string())).unwrap())
        })
    })}))
}
