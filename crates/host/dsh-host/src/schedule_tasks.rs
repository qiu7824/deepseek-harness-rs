//! Host wiring for scheduled tasks: the task store under the data root,
//! delivery into the original session through the same control admission
//! that `session.prompt` uses (restoring a cold session when needed), the
//! model tools, and the `/__dsh-schedule/*` management route for the Web and
//! desktop clients.

use std::path::Path;
use std::sync::{Arc, Weak};
use std::time::Duration;

use dsh_host_apiproxy::ApiProxyService;
use dsh_schedule_host::{
    CreateTask, Deliver, ScheduleError, ScheduleService, ServiceConfig, TaskOrigin, TaskRule,
    UpdateTask,
};
use futures::future::BoxFuture;
use serde_json::{Value, json};

/// Open the task store and register the model tools. Delivery starts once
/// [`attach`] provides the session gateway.
pub fn install(ctx: &cordis::Context, data_root: &Path) -> Arc<ScheduleService> {
    let service = ScheduleService::open(data_root.join("schedule.json"), ServiceConfig::default());
    if let Some(error) = service.load_error() {
        eprintln!("scheduled task storage is unavailable: {error}");
    }
    if let Err(error) = dsh_schedule_host::register_tools(ctx, service.clone()) {
        eprintln!("scheduled task tools were not registered: {error}");
    }
    service
}

struct SessionDelivery {
    api: Weak<ApiProxyService>,
    ctx: cordis::Context,
}

impl Deliver for SessionDelivery {
    fn deliver(
        &self,
        session_id: String,
        text: String,
    ) -> BoxFuture<'static, Result<Option<String>, String>> {
        let api = self.api.upgrade();
        let ctx = self.ctx.clone();
        Box::pin(async move {
            let api = api.ok_or_else(|| "主机正在关闭".to_string())?;
            let lease = api
                .resolve_control_agent(&session_id)
                .await
                .map_err(|error| format!("无法打开目标会话：{error}"))?;
            let message = dsh_llm::create_user_message(
                vec![dsh_llm::ContentBlock::Text { text }],
                dsh_llm::MessageSource::Plugin {
                    plugin: "schedule".to_string(),
                    form: None,
                    sections: None,
                    summary: None,
                    compaction_id: None,
                    source_command_id: None,
                },
            );
            let id = message.id.as_str().to_string();
            lease.agent.followup(message);
            if let Some(store) = ctx
                .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
                .map(|slot| slot.as_ref().clone())
                && !matches!(store.flush(lease.agent.session()).await, Ok(true))
            {
                eprintln!(
                    "scheduled message for {session_id} queued before session flush completed"
                );
            }
            Ok(Some(id))
        })
    }
}

fn error_body(error: &ScheduleError) -> Value {
    json!({"error": error.message, "code": error.code})
}

fn text(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}

fn rule(args: &Value) -> Result<Option<TaskRule>, ScheduleError> {
    match args.get("rule") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| ScheduleError::new("invalid_rule", format!("规则格式无效：{error}"))),
    }
}

async fn operation(
    service: &ScheduleService,
    name: &str,
    args: Value,
) -> Result<Value, ScheduleError> {
    let id = text(&args, "id").unwrap_or_default();
    let session = text(&args, "sessionId");
    let session = session.as_deref();
    match name {
        "catalog" => Ok(service.catalog(session)),
        "create" => {
            let rule = rule(&args)?
                .ok_or_else(|| ScheduleError::new("invalid_rule", "需要提供时间规则"))?;
            let view = service
                .create(CreateTask {
                    session_id: session.unwrap_or_default().to_string(),
                    title: text(&args, "title"),
                    prompt: text(&args, "prompt").unwrap_or_default(),
                    rule,
                    origin: TaskOrigin::User,
                })
                .await?;
            Ok(json!({"task": view}))
        }
        "update" => {
            let view = service
                .update(
                    &id,
                    session,
                    args.get("expectedUpdatedAt").and_then(Value::as_str),
                    UpdateTask {
                        title: text(&args, "title"),
                        prompt: text(&args, "prompt"),
                        rule: rule(&args)?,
                    },
                )
                .await?;
            Ok(json!({"task": view}))
        }
        "setActive" => {
            let active = args.get("active").and_then(Value::as_bool).unwrap_or(true);
            Ok(json!({"task": service.set_active(&id, session, active).await?}))
        }
        "delete" => {
            service.delete(&id, session).await?;
            Ok(json!({"id": id, "deleted": true}))
        }
        "history" => {
            let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
            Ok(
                serde_json::to_value(service.history(&id, session, offset, limit)?)
                    .unwrap_or(Value::Null),
            )
        }
        "runNow" => Ok(json!({"delivery": service.run_now(&id, session).await?})),
        "wait" => {
            let seen = args.get("revision").and_then(Value::as_u64).unwrap_or(0);
            let revision = service.wait_change(seen, Duration::from_secs(25)).await;
            Ok(json!({"revision": revision}))
        }
        _ => Err(ScheduleError::new("unknown_operation", "未知操作")),
    }
}

/// Start delivery and the scheduler, and register the management route.
pub fn attach(
    ctx: &cordis::Context,
    service: Arc<ScheduleService>,
    api: &Arc<ApiProxyService>,
    server: &Arc<dsh_host_webserver::WebServer>,
    allow_remote: bool,
) -> dsh_host_webserver::RouteDisposer {
    service.set_deliver(Arc::new(SessionDelivery {
        api: Arc::downgrade(api),
        ctx: ctx.clone(),
    }));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let runner = service.clone();
    tokio::spawn(async move {
        runner
            .run(async move {
                let _ = stopped.await;
            })
            .await;
    });
    let stop = parking_lot::Mutex::new(Some(stop));
    let _ = ctx.effect(
        "scheduled task delivery",
        Box::pin(async move {
            Some(cordis::make_disposer(move || {
                if let Some(stop) = stop.lock().take() {
                    let _ = stop.send(());
                }
                Box::pin(async {})
            }))
        }),
    );
    server.register(dsh_host_webserver::WebRoute {
        kind: dsh_host_webserver::WebRouteKind::Prefix,
        path: "/__dsh-schedule".into(),
        handler: Arc::new(move |request| {
            let service = service.clone();
            Box::pin(async move {
                let allowed = request.method() == http::Method::POST
                    && super::trusted_web_request(&request, allow_remote);
                let name = request
                    .uri()
                    .path()
                    .trim_start_matches("/__dsh-schedule/")
                    .to_string();
                let (status, body) = if !allowed {
                    (403, json!({"error": "forbidden", "code": "forbidden"}))
                } else {
                    match axum::body::to_bytes(
                        axum::body::Body::new(request.into_body()),
                        256 * 1024,
                    )
                    .await
                    {
                        Err(_) => (413, json!({"error": "请求过大", "code": "too_large"})),
                        Ok(bytes) => match serde_json::from_slice::<Value>(if bytes.is_empty() {
                            b"{}"
                        } else {
                            &bytes
                        }) {
                            Err(_) => {
                                (400, json!({"error": "请求格式无效", "code": "bad_request"}))
                            }
                            Ok(args) => match operation(&service, &name, args).await {
                                Ok(value) => (200, value),
                                Err(error) => (
                                    if error.code == "schedule_not_found" {
                                        404
                                    } else if error.code == "conflict" {
                                        409
                                    } else {
                                        400
                                    },
                                    error_body(&error),
                                ),
                            },
                        },
                    }
                };
                Ok(http::Response::builder()
                    .status(status)
                    .header("content-type", "application/json")
                    .header("cache-control", "no-store")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap())
            })
        }),
    })
}
