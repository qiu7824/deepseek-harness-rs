//! Task routes reuse the existing provider registry and credential ownership.
use cordis::{Context, Service};
use dsh_llm::{
    ContentBlock, FinishReason, GenerateOptions, LlmRuntime, MessageSource, StreamChunk,
};
use dsh_schemastery::{Data, Schema};
use dsh_settings::{SettingsProvider, SettingsRegisterOptions, SettingsScope, settings_namespace};
use futures::StreamExt;
use serde_json::{Value, json};
use std::sync::Arc;

pub(crate) const ROLES: &[&str] = &["diagnose", "optimize", "vision", "image"];
pub(crate) struct TaskModels {
    pub ctx: Context,
    pub settings: Arc<SettingsProvider>,
    pub llm: Arc<LlmRuntime>,
    scope: SettingsScope,
}
impl Service for TaskModels {
    fn service_name(&self) -> &'static str {
        "taskModels"
    }
}
impl TaskModels {
    pub fn install(
        ctx: &Context,
        settings: Arc<SettingsProvider>,
        llm: Arc<LlmRuntime>,
    ) -> Result<Arc<Self>, String> {
        let route = Schema::object(indexmap::IndexMap::from([
            ("provider".into(), Schema::string()),
            ("model".into(), Schema::string()),
            ("reasoningEffort".into(), Schema::string()),
        ]));
        let scope = settings.register(
            ctx,
            settings_namespace("task-models")?,
            Schema::object(
                ROLES
                    .iter()
                    .map(|role| {
                        (
                            role.to_string(),
                            route.clone().default(Data::Object(Default::default())),
                        )
                    })
                    .collect(),
            ),
            SettingsRegisterOptions::default(),
        )?;
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            settings,
            llm,
            scope,
        });
        ctx.register_service(service.clone());
        service.register_tool()?;
        Ok(service)
    }
    pub async fn route(
        &self,
        role: &str,
        execution: &dsh_tools::ToolExecution,
    ) -> Result<Value, String> {
        if !ROLES.contains(&role) {
            return Err("未知任务用途".into());
        }
        let value = (self.scope.get)().to_json().ok_or("任务模型配置无效")?;
        let agent = execution.agent.as_ref().ok_or("需要当前会话")?;
        let selected = agent
            .ctx()
            .get_typed::<Arc<parking_lot::Mutex<dsh_agent::ModelSelectionRef>>>(
                &dsh_agent::model_selection_service_name(agent.ctx()),
                false,
            )
            .and_then(|selection| {
                let state = selection.lock();
                state.assembled.clone().or_else(|| state.resolved_current())
            });
        let defaults = self
            .settings
            .describe(Default::default())
            .into_iter()
            .find(|s| s.ns.as_str() == "agent-default-model")
            .and_then(|s| s.value.to_json())
            .unwrap_or(json!({}));
        let current_provider = selected
            .as_ref()
            .map(|s| s.provider.as_str())
            .or(agent.options().provider.as_deref())
            .or(defaults["provider"].as_str())
            .ok_or("当前会话未选择模型连接")?;
        let current_model = selected
            .as_ref()
            .map(|s| s.model.as_str())
            .or(agent.options().model.as_deref())
            .or(defaults["model"].as_str())
            .ok_or("当前会话未选择模型")?;
        let configured = &value[role];
        let provider = configured["provider"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(current_provider);
        let models = self
            .llm
            .list_models(provider)
            .await
            .map_err(|e| e.to_string())?;
        let route = resolve_route(
            role,
            configured,
            current_provider,
            current_model,
            &models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        )?;
        Ok(route)
    }
    async fn snapshot(&self) -> Result<Value, String> {
        let mut providers = vec![];
        for provider in self.llm.list_providers() {
            let models = self.llm.list_models(&provider.id).await.unwrap_or_default();
            providers.push(json!({"id":provider.id,"name":provider.name,"models":models}));
        }
        let settings = self.settings.describe(Default::default());
        let task = settings
            .iter()
            .find(|s| s.ns.as_str() == "task-models")
            .ok_or("任务模型配置不可用")?;
        let main = settings
            .iter()
            .find(|s| s.ns.as_str() == "agent-default-model");
        Ok(
            json!({"routes":(self.scope.get)().to_json(),"revision":task.revision,"main":main.and_then(|s|s.value.to_json()),"mainRevision":main.map(|s|s.revision),"providers":providers}),
        )
    }
    pub fn register_http(
        self: &Arc<Self>,
        server: &Arc<dsh_host_webserver::WebServer>,
    ) -> dsh_host_webserver::RouteDisposer {
        let service = self.clone();
        server.register(dsh_host_webserver::WebRoute {
            kind: dsh_host_webserver::WebRouteKind::Prefix,
            path: "/task-models".into(),
            handler: Arc::new(move |request| {
                let service = service.clone();
                Box::pin(async move {
                    let allowed = super::trusted_web_request(&request, false)
                        && request.method() == http::Method::POST;
                    let path = request.uri().path().to_string();
                    let result = async {
                        if !allowed {
                            return Err("forbidden".to_string());
                        }
                        let bytes =
                            axum::body::to_bytes(axum::body::Body::new(request.into_body()), 16384)
                                .await
                                .map_err(|_| "请求过大")?;
                        let body: Value = serde_json::from_slice(&bytes).map_err(|_| "无效JSON")?;
                        if path.ends_with("/save") {
                            if body.get("main").is_some() {
                                let mut main = body["main"].clone();
                                if main["reasoningEffort"] == "" {
                                    main.as_object_mut()
                                        .ok_or("主模型配置无效")?
                                        .remove("reasoningEffort");
                                }
                                let config: dsh_llm::LlmCallConfig =
                                    serde_json::from_value(main.clone())
                                        .map_err(|_| "主模型配置无效")?;
                                if image_only(&config.model) {
                                    return Err("生图专用模型不能作为主对话模型".into());
                                }
                                service
                                    .llm
                                    .resolve_call_config(&config, None)
                                    .await
                                    .map_err(|e| e.to_string())?;
                                service
                                    .settings
                                    .replace(
                                        &settings_namespace("agent-default-model")?,
                                        main,
                                        body["mainRevision"].as_u64(),
                                    )
                                    .await?;
                            } else {
                                let routes = body.get("routes").cloned().ok_or("缺少任务配置")?;
                                for role in ROLES {
                                    if let Some(route) = routes.get(*role) {
                                        let p = route["provider"].as_str().unwrap_or("");
                                        let m = route["model"].as_str().unwrap_or("");
                                        if !p.is_empty()
                                            && !service
                                                .llm
                                                .list_providers()
                                                .iter()
                                                .any(|v| v.id == p)
                                        {
                                            return Err(format!("模型连接不可用：{p}"));
                                        }
                                        if *role != "image" && !p.is_empty() && !m.is_empty() {
                                            if image_only(m) {
                                                return Err(format!("{role}不能使用生图专用模型"));
                                            }
                                            let config = dsh_llm::LlmCallConfig {
                                                provider: p.into(),
                                                model: m.into(),
                                                reasoning_effort: route["reasoningEffort"]
                                                    .as_str()
                                                    .filter(|s| !s.is_empty())
                                                    .map(dsh_llm::reasoning_effort_id),
                                                ..Default::default()
                                            };
                                            service
                                                .llm
                                                .resolve_call_config(&config, None)
                                                .await
                                                .map_err(|e| e.to_string())?;
                                        }
                                    }
                                }
                                service
                                    .settings
                                    .replace(
                                        &settings_namespace("task-models")?,
                                        routes,
                                        body["revision"].as_u64(),
                                    )
                                    .await?;
                            }
                        } else if !path.ends_with("/describe") {
                            return Err("未知任务模型操作".into());
                        }
                        service.snapshot().await
                    }
                    .await;
                    let (status, value) = match result {
                        Ok(v) => (200, v),
                        Err(e) => (if allowed { 400 } else { 403 }, json!({"error":e})),
                    };
                    Ok(http::Response::builder()
                        .status(status)
                        .header("content-type", "application/json")
                        .header("cache-control", "no-store")
                        .body(axum::body::Body::from(value.to_string()))
                        .unwrap())
                })
            }),
        })
    }
    fn register_tool(self: &Arc<Self>) -> Result<(), String> {
        use dsh_tools::*;
        let tools = self
            .ctx
            .get_typed::<Arc<ToolRuntime>>("tools", false)
            .ok_or("缺少工具运行时")?;
        let service = self.clone();
        tools.register(&self.ctx,ToolDefinition{name:"consult_model".into(),description:"Consult an optionally assigned diagnose, optimize, or vision model. Use dedicated consultation when configured or explicitly requested; otherwise handle the task directly with the current model. Supply the evidence and question; this consultation returns analysis, does not edit files, and uses the existing provider account. Reference images may be session attachment IDs or workspace image paths. Use vision when the main model cannot inspect an image.".into(),
            parameters:json!({"type":"object","additionalProperties":false,"properties":{"task":{"type":"string","enum":["diagnose","optimize","vision"]},"prompt":{"type":"string","maxLength":64000},"reference_images":{"type":"array","maxItems":4,"items":{"type":"string"}}},"required":["task","prompt"]}),
            output:ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,v|Ok(vec![ContentBlock::Text{text:v.to_string()}])),presentation_meta:None},timeout_ms:Some(300000),is_concurrency_safe:Some(Arc::new(|_|true)),finalize_content:None,present_call:None,present_result:None,
            execute:Arc::new(move|args,run|{let service=service.clone();let args=args.clone();let execution=run.execution.clone();Box::pin(async move{
                let role=args["task"].as_str().unwrap_or("");let route=service.route(role,&execution).await.map_err(ToolBodyError::plain)?;
                let signal=execution.signal.lock().clone();let agent=execution.agent.as_ref().ok_or_else(||ToolBodyError::plain("需要当前会话"))?;
                let mut content=vec![ContentBlock::Text{text:args["prompt"].as_str().unwrap_or("").into()}];
                let images=super::image_generation::read_references(&service.ctx,&execution,&args["reference_images"]).await.map_err(ToolBodyError::plain)?;
                for image in images{content.push(ContentBlock::Image{attachment:serde_json::from_value(serde_json::to_value(image.reference).unwrap()).map_err(|e|ToolBodyError::plain(e.to_string()))?});}
                let options=GenerateOptions{provider:route["provider"].as_str().unwrap().into(),model:route["model"].as_str().unwrap().into(),reasoning_effort:route["reasoningEffort"].as_str().filter(|s|!s.is_empty()).map(dsh_llm::reasoning_effort_id),messages:vec![dsh_llm::create_user_message(content,MessageSource::User{rpc_id:None,client_time_zone:None})],system:Some(format!("You are the {role} consultant. Analyze the supplied evidence, distinguish facts from hypotheses, and give actionable findings. Do not claim to have executed tools or changed files.")),tools:None,temperature:None,max_tokens:None,stop:None,signal:Some(signal.clone()),session_id:Some(agent.id().to_string()),purpose:Some(role.into()),agent_loop_request:false};
                let mut stream=service.llm.stream(options);let mut assembler=dsh_llm::BlockAssembler::new();let mut received=0usize;let mut chunks=0usize;let mut finished=false;
                loop {tokio::select!{
                    chunk=stream.next()=>match chunk{Some(StreamChunk::Finish{reason:FinishReason::Error{failure}|FinishReason::Aborted{failure},..})=>return Err(ToolBodyError::plain(failure.message)),Some(chunk)=>{finished|=matches!(chunk,StreamChunk::Finish{..});chunks+=1;received+=match &chunk{StreamChunk::TextDelta{text,..}|StreamChunk::ReasoningDelta{text,..}=>text.len(),StreamChunk::ToolCallDelta{arguments_delta,..}=>arguments_delta.len(),StreamChunk::BlockEnd{block,..}=>serde_json::to_vec(block).map_or(0,|value|value.len()),_=>0};if chunks>100000||received>512*1024{return Err(ToolBodyError::plain("辅助模型输出过大"))}assembler.push(&chunk)},None=>break},
                    _=tokio::time::sleep(std::time::Duration::from_millis(30))=>if signal(){return Err(ToolBodyError::plain("辅助模型请求已取消"))}
                }}
                if !finished{return Err(ToolBodyError::plain("辅助模型响应未完整结束"))}let finish=assembler.finish();
                let text=assembler.blocks().iter().filter_map(|block|if let ContentBlock::Text{text}=block{Some(text.clone())}else{None}).collect::<Vec<_>>().join("\n");
                if text.is_empty(){return Err(ToolBodyError::plain("辅助模型未返回分析内容"))}
                Ok(json!({"task":role,"provider":route["provider"],"model":route["model"],"text":text,"finishReason":finish.kind(),"usage":assembler.usage()}))
            })})})?;
        Ok(())
    }
}

fn image_only(model: &str) -> bool {
    model.starts_with("gpt-image") || model.starts_with("dall-e")
}

fn resolve_route(
    role: &str,
    configured: &Value,
    current_provider: &str,
    current_model: &str,
    models: &[&str],
) -> Result<Value, String> {
    let provider = configured["provider"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(current_provider);
    let model = configured["model"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            if role == "image" {
                if image_only(current_model) && provider == current_provider {
                    Some(current_model)
                } else {
                    models
                        .iter()
                        .copied()
                        .find(|m| image_only(m))
                        .or(Some("gpt-image-2.5-sunburst"))
                }
            } else if provider == current_provider {
                Some(current_model)
            } else {
                models.iter().copied().find(|m| !image_only(m))
            }
        })
        .ok_or("所选连接没有可用模型")?;
    Ok(json!({"provider":provider,"model":model,"reasoningEffort":configured["reasoningEffort"]}))
}

#[cfg(test)]
mod optional_route_tests {
    use super::*;
    #[test]
    fn unassigned_tasks_use_current_connection() {
        let r = resolve_route("image", &json!({}), "openai-codex", "gpt-main", &[]).unwrap();
        assert_eq!(r["provider"], "openai-codex");
        assert_eq!(r["model"], "gpt-image-2.5-sunburst");
        let r = resolve_route("diagnose", &json!({}), "current", "main", &[]).unwrap();
        assert_eq!(r["model"], "main");
    }
    #[test]
    fn provider_and_model_overrides_are_independently_optional() {
        let r = resolve_route(
            "image",
            &json!({"model":"gpt-image-2.5-flare"}),
            "current",
            "main",
            &[],
        )
        .unwrap();
        assert_eq!(r["provider"], "current");
        assert_eq!(r["model"], "gpt-image-2.5-flare");
        let r = resolve_route(
            "image",
            &json!({"provider":"pictures"}),
            "current",
            "main",
            &["gpt-image-2"],
        )
        .unwrap();
        assert_eq!(r["provider"], "pictures");
        assert_eq!(r["model"], "gpt-image-2");
    }
}
