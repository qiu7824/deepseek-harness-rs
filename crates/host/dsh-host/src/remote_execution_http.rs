//! Settings/API for local-agent remote execution, separate from the remote Harness tunnel.
use std::sync::Arc;
use cordis::Context;
use dsh_host_webserver::{RouteDisposer,WebRoute,WebRouteKind,WebServer};
use dsh_remote_execution::{RemoteRuntime,Connection};
use dsh_tools::{ToolDefinition,ToolOutputDefinition,ToolBodyError,ToolRuntime};
use serde_json::{Value,json};

pub(super) fn install_tools(ctx:&Context,tools:&Arc<ToolRuntime>,remote:Arc<RemoteRuntime>)->Result<(),String>{
    tools.register(ctx,ToolDefinition{name:"remote_execution_status".into(),description:"Query an existing remote execution by its durable executionId after disconnect, or request cancellation and inspect confirmation. Never repeats the original command. Requires this session's verified remote workspace. Use execute_native/execute_script and file tools for remote work.".into(),parameters:json!({"type":"object","properties":{"executionId":{"type":"string"},"cancel":{"type":"boolean"},"stdoutOffset":{"type":"integer","minimum":0},"stderrOffset":{"type":"integer","minimum":0}},"required":["executionId"],"additionalProperties":false}),output:ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,value|Ok(vec![dsh_llm::ContentBlock::Text{text:value.to_string()}])),presentation_meta:None},timeout_ms:Some(25000),is_concurrency_safe:Some(Arc::new(|a|a["cancel"]!=true)),execute:Arc::new(move|args,exec|{
        let args=args.clone();let remote=remote.clone();let agent=exec.agent.clone();Box::pin(async move{
            let agent=agent.ok_or_else(||ToolBodyError::plain("需要当前远端会话"))?;let cwd=agent.session().header().cwd.as_deref().ok_or_else(||ToolBodyError::plain("会话没有工作区"))?;
            let(row,_)=remote.by_uri(cwd).map_err(ToolBodyError::plain)?;
            remote.query(&row.connection.id,args["executionId"].as_str().unwrap_or_default(),"read-only",args["stdoutOffset"].as_u64().unwrap_or(0),args["stderrOffset"].as_u64().unwrap_or(0),args["cancel"].as_bool().unwrap_or(false)).await.map_err(ToolBodyError::plain)
        })
    }),finalize_content:None,present_call:None,present_result:None})?;Ok(())
}

pub(super) fn register(web:&Arc<WebServer>,remote:Arc<RemoteRuntime>)->RouteDisposer{
    web.register(WebRoute{kind:WebRouteKind::Exact,path:"/__dsh-remote-execution".into(),handler:Arc::new(move|request|{let remote=remote.clone();Box::pin(async move{
        let trusted=crate::trusted_web_request(&request,false);
        let result=if !trusted{Err("只允许本机可信页面管理远端执行环境".into())}else if request.method()!=http::Method::POST{Err("需要 JSON POST".into())}else{
            match axum::body::to_bytes(axum::body::Body::new(request.into_body()),16*1024).await{
                Ok(bytes)=>match serde_json::from_slice::<Value>(&bytes){
                    Ok(args)=>match args["action"].as_str().unwrap_or("list"){
                        "list"=>Ok(remote.list()),
                        "connect"=>match serde_json::from_value::<Connection>(args["connection"].clone()){Ok(connection)=>remote.connect(connection).await.and_then(|row|serde_json::to_value(row).map_err(|e|e.to_string())),Err(e)=>Err(e.to_string())},
                        "remove"=>remote.remove(args["id"].as_str().unwrap_or_default()).await.map(|_|json!({"removed":true})),
                        "query"|"cancel"=>remote.query(args["id"].as_str().unwrap_or_default(),args["executionId"].as_str().unwrap_or_default(),"read-only",args["stdoutOffset"].as_u64().unwrap_or(0),args["stderrOffset"].as_u64().unwrap_or(0),args["action"]=="cancel").await,
                        _=>Err("未知远端操作".into()),
                    },Err(_)=>Err("无效 JSON".into()),
                },Err(_)=>Err("请求过大".into()),
            }
        };
        let(status,value)=match result{Ok(value)=>(http::StatusCode::OK,value),Err(error)=>(if trusted{http::StatusCode::BAD_REQUEST}else{http::StatusCode::FORBIDDEN},json!({"error":error}))};
        Ok(http::Response::builder().status(status).header("content-type","application/json").header("cache-control","no-store").body(axum::body::Body::from(value.to_string())).expect("remote execution response"))
    })})})
}
