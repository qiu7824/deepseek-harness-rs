use super::*;
use dsh_agent::{AgentFactory, AgentOptions, CreateAgentOptions};
use dsh_session_persistence::SessionPersistenceApi;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

struct ReminderAdapter {
    turns: AtomicUsize,
    delivered: parking_lot::Mutex<Vec<(String, String)>>,
}

impl dsh_llm::LlmAdapter for ReminderAdapter {
    fn stream(&self, options: &dsh_llm::GenerateOptions) -> dsh_llm::ChunkStream {
        if options.agent_loop_request {
            self.turns.fetch_add(1, Ordering::SeqCst);
            for message in &options.messages {
                if message.source.plugin_name() == Some("schedule") {
                    self.delivered.lock().push((
                        message.id.to_string(),
                        serde_json::to_string(&message.content).unwrap(),
                    ));
                }
            }
        }
        Box::pin(futures::stream::iter([
            dsh_llm::StreamChunk::BlockStart {
                index: 0,
                block_type: "text".into(),
            },
            dsh_llm::StreamChunk::TextDelta {
                index: 0,
                text: "Reminder received".into(),
            },
            dsh_llm::StreamChunk::BlockEnd {
                index: 0,
                block: dsh_llm::ContentBlock::Text {
                    text: "Reminder received".into(),
                },
            },
            dsh_llm::StreamChunk::Finish {
                reason: dsh_llm::FinishReason::Stop,
                replay_state: None,
            },
        ]))
    }
}

async fn rpc(host: &HostSpine, method: &str, payload: Value) -> Value {
    let response = to_fetch_handler(host.api_proxy.clone()).handle(CarrierRequest {
        method:http::Method::POST, path:format!("/api/{method}"), query:vec![], headers:vec![("content-type".into(),"application/json".into())],
        body:Some(json!({"type":"client-request","rpcId":uuid::Uuid::new_v4().to_string(),"method":method,"payload":payload}).to_string().into_bytes()),
    }).await;
    let CarrierBody::Bytes(bytes) = response.into_body() else {
        panic!("unary result required")
    };
    let response: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(response["result"]["ok"], true, "{response}");
    response["result"]["value"].clone()
}

async fn toggle(host: &HostSpine, enabled: bool) {
    host.api_proxy
        .apply_plugin_enablement(
            "dsh-schedule".into(),
            enabled,
            Default::default(),
            None,
            Arc::new(|_| {}),
            None,
        )
        .await
        .unwrap();
}

fn reminder_names(host: &HostSpine, agent: &dyn dsh_agent::Agent) -> Vec<String> {
    host.tools
        .schemas(Some(agent.scope_key()))
        .into_iter()
        .filter(|tool| tool.name.starts_with("schedule_"))
        .map(|tool| tool.name)
        .collect()
}

async fn execute_schedule_tool(
    host: &HostSpine,
    agent: Arc<dyn dsh_agent::Agent>,
    name: &str,
    arguments: Value,
) -> Arc<dsh_tools::ToolExecutionResult> {
    host.tools
        .execute(dsh_tools::ToolExecutionInput {
            call_id: dsh_llm::call_id(uuid::Uuid::new_v4().to_string()),
            root_call_id: None,
            name: name.into(),
            arguments,
            agent: Some(agent),
            parent: None,
            signal: Arc::new(|| false),
        })
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reminder_boundary_follows_real_presets_switch_cold_restore_and_plugin_enablement() {
    let home =
        std::env::temp_dir().join(format!("host-reminder-boundary-{}", uuid::Uuid::new_v4()));
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    // Test executables have no packaged resources beside deps/. Supply the
    // actual shipped compositions through normal user-root discovery.
    for preset in ["standard", "minimal", "blank", "cordis", "code"] {
        let target = home.join(".agent-presets").join(preset);
        std::fs::create_dir_all(&target).unwrap();
        for name in ["preset.yml", "agent.cordis.yml"] {
            std::fs::copy(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../config/agent-presets")
                    .join(preset)
                    .join(name),
                target.join(name),
            )
            .unwrap();
        }
    }
    let ctx = Context::root();
    let host = compose_persistent_host_at(&ctx, &home, Some("web")).unwrap();
    let created = rpc(
        &host,
        "session.create",
        json!({"cwd":workspace,"agentPreset":"standard"}),
    )
    .await;
    let id = dsh_session::session_id(created["sessionId"].as_str().unwrap());
    let agent = host.agents.get(&id).unwrap();
    assert!(
        reminder_names(&host, agent.as_ref()).is_empty(),
        "reminders remain opt-in"
    );
    let built_in_names = host
        .tools
        .schemas(Some(agent.scope_key()))
        .into_iter()
        .filter(|tool| tool.name.starts_with("scheduled_task_"))
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    assert_eq!(built_in_names.len(), 4);
    // Both built-in scheduled task kinds continue to work in Minimal.
    rpc(
        &host,
        "agentPreset.select",
        json!({"sessionId":id,"agentPreset":"minimal"}),
    )
    .await;
    for selector in [
        json!({"after_seconds":86400}),
        json!({"every_seconds":86400}),
    ] {
        let mut args = selector;
        args["prompt"] = json!("Built-in task fixture");
        let result =
            execute_schedule_tool(&host, agent.clone(), "scheduled_task_create", args).await;
        assert!(!result.is_error, "{:?}", result.error);
    }
    let built_in =
        execute_schedule_tool(&host, agent.clone(), "scheduled_task_list", json!({})).await;
    assert!(!built_in.is_error, "{:?}", built_in.error);
    let built_in_value = built_in.value.as_ref().expect("built-in task list output");
    let built_in_tasks = built_in_value["tasks"]
        .as_array()
        .expect("task array")
        .clone();
    assert_eq!(built_in_tasks.len(), 2);
    let mut last_list_time = chrono::DateTime::parse_from_rfc3339(
        built_in_value["now"]
            .as_str()
            .expect("catalog current time"),
    )
    .expect("catalog time must be RFC 3339");
    assert_eq!(last_list_time.offset().local_minus_utc(), 0);
    toggle(&host, true).await;
    let mut frozen_reminder_schema = None;
    for preset in ["minimal", "blank", "standard", "cordis", "code", "minimal"] {
        rpc(
            &host,
            "agentPreset.select",
            json!({"sessionId":id,"agentPreset":preset}),
        )
        .await;
        assert_eq!(
            host.agent_presets.composed_preset(agent.ctx()).as_deref(),
            Some(preset)
        );
        let expected = if matches!(preset, "minimal" | "blank") {
            0
        } else {
            4
        };
        assert_eq!(
            reminder_names(&host, agent.as_ref()).len(),
            expected,
            "{preset}"
        );
        let assembly = dsh_system_prompt::AssembleContext {
            scope: Some(agent.scope_key().clone()),
            fields: serde_json::from_value(json!({"sessionId":id,"cwd":workspace})).unwrap(),
        };
        let prompt = host
            .system_prompt
            .assemble(agent.ctx(), &assembly)
            .await
            .unwrap();
        if expected == 0 {
            assert!(
                prompt
                    .tools
                    .iter()
                    .all(|tool| !tool.name.starts_with("schedule_"))
            );
            for (name, args) in [
                (
                    "schedule_create",
                    json!({"title":"Denied","prompt":"Fixture","after_seconds":86400}),
                ),
                ("schedule_list", json!({})),
                ("schedule_update", json!({"id":"missing","title":"Denied"})),
                ("schedule_delete", json!({"id":"missing"})),
            ] {
                let result = execute_schedule_tool(&host, agent.clone(), name, args).await;
                assert!(
                    result.is_error,
                    "{preset}: {name} bypassed composition visibility"
                );
            }
            if let Some(schema) = frozen_reminder_schema.clone() {
                let result = host
                    .tools
                    .execute_bound(
                        dsh_tools::ToolExecutionInput {
                            call_id: dsh_llm::call_id(uuid::Uuid::new_v4().to_string()),
                            root_call_id: None,
                            name: "schedule_list".into(),
                            arguments: json!({}),
                            agent: Some(agent.clone()),
                            parent: None,
                            signal: Arc::new(|| false),
                        },
                        schema,
                    )
                    .await;
                assert!(
                    result.is_error,
                    "an earlier SDK binding cannot bypass the new preset fence"
                );
            }
        } else if preset == "code" {
            assert_eq!(
                prompt
                    .tools
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>(),
                ["run_code"],
                "the real PTC model surface must use the code transport"
            );
            assert!(prompt.tools.iter().any(|tool| tool.name == "run_code"));
            assert!(
                host.tools
                    .get("schedule_list", Some(agent.scope_key()))
                    .is_some(),
                "PTC SDK retains reminder bindings"
            );
            let direct =
                execute_schedule_tool(&host, agent.clone(), "schedule_list", json!({})).await;
            assert!(
                direct.is_error,
                "PTC reminder calls must use their SDK binding"
            );
        } else {
            frozen_reminder_schema = host
                .tools
                .schemas(Some(agent.scope_key()))
                .into_iter()
                .find(|schema| schema.name == "schedule_list");
            let result =
                execute_schedule_tool(&host, agent.clone(), "schedule_list", json!({})).await;
            assert!(!result.is_error, "{preset}: {:?}", result.error);
            assert_eq!(result.value, Some(json!([])));
            if preset == "standard" {
                let parent = agent.clone();
                let presets = host.agent_presets.clone();
                let child = host
                    .agent_loop
                    .create_agent(
                        agent.ctx(),
                        CreateAgentOptions {
                            meta: Some(dsh_session::CreateSessionMeta {
                                cwd: Some(workspace.to_string_lossy().into_owned()),
                                origin: Some("subagent".into()),
                                parent_session: Some(id.clone()),
                                agent_preset: Some("standard".into()),
                                ..Default::default()
                            }),
                            setup: Some(Arc::new(move |child_ctx, _| {
                                let child_ctx = child_ctx.clone();
                                let parent = parent.clone();
                                let presets = presets.clone();
                                Box::pin(async move {
                                    presets.compose_from(&child_ctx, parent.ctx()).ok_or_else(
                                        || {
                                            "the child must join the parent's actual preset"
                                                .to_owned()
                                        },
                                    )?;
                                    dsh_subagent::apply_child_composition(
                                        &child_ctx,
                                        parent.as_ref(),
                                        &Default::default(),
                                    )?;
                                    Ok(None)
                                })
                            })),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
                assert!(
                    reminder_names(&host, child.agent.as_ref()).is_empty(),
                    "Host setup must retain the permanent child fence"
                );
                for enabled in [false, true] {
                    toggle(&host, enabled).await;
                    assert_eq!(
                        reminder_names(&host, agent.as_ref()).len(),
                        if enabled { 4 } else { 0 }
                    );
                    assert!(
                        reminder_names(&host, child.agent.as_ref()).is_empty(),
                        "copied child catalogs stay hidden while the optional plugin toggles"
                    );
                    let denied = execute_schedule_tool(
                        &host,
                        child.agent.clone(),
                        "schedule_create",
                        json!({"title":"Denied","prompt":"Child fixture","after_seconds":86400}),
                    )
                    .await;
                    assert!(denied.is_error);
                }
                child.dispose.await;
                drop(child.agent);
            }
        }
    }
    assert_eq!(
        agent.session().header().agent_preset.as_deref(),
        Some("standard"),
        "the original header must not be mistaken for the live composition"
    );
    assert!(host.sessions.flush(agent.session()).await.unwrap());
    let lease = host
        .api_proxy
        .resolve_control_agent(id.as_str())
        .await
        .unwrap();
    drop(lease);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while host.agents.get(&id).is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the real controller must retire the idle owner");
    drop(agent);
    let restored = host
        .api_proxy
        .resolve_control_agent(id.as_str())
        .await
        .unwrap();
    assert_eq!(
        host.agent_presets
            .composed_preset(restored.agent.ctx())
            .as_deref(),
        Some("minimal")
    );
    assert!(reminder_names(&host, restored.agent.as_ref()).is_empty());
    let denied =
        execute_schedule_tool(&host, restored.agent.clone(), "schedule_list", json!({})).await;
    assert!(
        denied.is_error,
        "cold resume must honor the latest selected preset"
    );
    for enabled in [false, true] {
        toggle(&host, enabled).await;
        assert!(reminder_names(&host, restored.agent.as_ref()).is_empty());
        assert_eq!(
            host.tools
                .schemas(Some(restored.agent.scope_key()))
                .into_iter()
                .filter(|tool| tool.name.starts_with("scheduled_task_"))
                .map(|tool| tool.name)
                .collect::<Vec<_>>(),
            built_in_names
        );
        let current = execute_schedule_tool(
            &host,
            restored.agent.clone(),
            "scheduled_task_list",
            json!({}),
        )
        .await;
        assert!(!current.is_error, "{:?}", current.error);
        let current_value = current.value.as_ref().expect("built-in task list output");
        assert_eq!(
            current_value["tasks"].as_array().expect("task array"),
            &built_in_tasks,
            "optional reminder enablement must retain both built-in tasks"
        );
        let current_time = chrono::DateTime::parse_from_rfc3339(
            current_value["now"].as_str().expect("catalog current time"),
        )
        .expect("catalog time must be RFC 3339");
        assert_eq!(current_time.offset().local_minus_utc(), 0);
        assert!(
            current_time >= last_list_time,
            "successive catalog timestamps must not go backwards"
        );
        last_list_time = current_time;
    }
    drop(restored);
    host.shutdown().await.unwrap();
    drop(host);
    drop(ctx);
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_schedule_plugin_is_opt_in_and_delivers_overdue_cold_session_after_restart() {
    let root = std::env::temp_dir().join(format!("host-schedule-plugin-{}", uuid::Uuid::new_v4()));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let fixture_preset = root.join(".agent-presets/blank");
    std::fs::create_dir_all(&fixture_preset).unwrap();
    for name in ["preset.yml", "agent.cordis.yml"] {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../config/agent-presets/blank")
            .join(name);
        std::fs::copy(source, fixture_preset.join(name)).unwrap();
    }
    let mut session_id = String::new();
    let mut task_id = String::new();
    for restarted in [false, true] {
        let ctx = Context::root();
        let host = compose_persistent_host_at(&ctx, &root, Some("web")).unwrap();
        assert_eq!(
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                host.agent_presets.resolve(None)
            )
            .await
            .expect("the fixture must supply the packaged blank preset")
            .unwrap()
            .id,
            "blank"
        );
        let projections = ctx
            .get_typed::<Arc<dsh_session_projection::SessionProjectionRegistry>>(
                "sessionProjections",
                false,
            )
            .unwrap();
        assert!(projections.keys().iter().any(|key| key == "schedule"));
        let schedule = ctx
            .get_typed::<Arc<dsh_schedule::host_service::ScheduleService>>("schedule", false)
            .unwrap()
            .as_ref()
            .clone();
        assert!(
            !schedule.enabled(),
            "default and persisted disablement must both be respected"
        );
        let adapter = Arc::new(ReminderAdapter {
            turns: AtomicUsize::new(0),
            delivered: Default::default(),
        });
        host.llm
            .register_adapter(&ctx, vec!["reminder-fixture".into()], adapter.clone())
            .unwrap();
        let current = rpc(
            &host,
            "pluginInventory.getConfig",
            json!({"entryId":"dsh-schedule"}),
        )
        .await;
        if !restarted {
            rpc(&host, "pluginInventory.setConfig", json!({"entryId":"dsh-schedule","expectedRevision":current["revision"],"config":{"deliveryHistoryDays":7,"deliveryHistoryRecords":2}})).await;
            assert!(
                !schedule.enabled(),
                "editing retention must not enable delivery"
            );
            assert_eq!(schedule.config().delivery_history_days, 7);
            assert_eq!(schedule.config().delivery_history_records, 2);
            let handle = host
                .agent_loop
                .create_agent(
                    &ctx,
                    CreateAgentOptions {
                        meta: Some(dsh_session::CreateSessionMeta {
                            cwd: Some(workspace.to_string_lossy().into_owned()),
                            ..Default::default()
                        }),
                        agent_options: Some(AgentOptions {
                            provider: Some("reminder-fixture".into()),
                            model: Some("fixture".into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            session_id = handle.agent.id().to_string();
            for (id, title) in [
                ("legacy-unnamed", None),
                ("legacy-named", Some("Saved legacy name")),
            ] {
                let mut record = json!({"kind":"after","id":id,"prompt":"Historical reminder","afterSeconds":60,"scheduledAt":"2099-01-01T00:00:00.000Z"});
                if let Some(title) = title {
                    record["title"] = json!(title);
                }
                handle
                    .agent
                    .session()
                    .append(
                        "schedule/change",
                        json!({"version":1,"operation":"create","schedule":record}),
                        None,
                    )
                    .unwrap();
            }
            let historical = projections.try_snapshot(handle.agent.session()).unwrap();
            assert_eq!(historical.values["schedule"].as_array().unwrap().len(), 2);
            assert_eq!(historical.values["schedule"][0].get("title"), None);
            assert_eq!(
                historical.values["schedule"][1]["title"],
                "Saved legacy name"
            );
            assert!(
                schedule.catalog().await.unwrap().is_empty(),
                "legacy events must not populate Host tasks"
            );
            handle.agent.followup(dsh_llm::create_user_message(
                vec![dsh_llm::ContentBlock::Text {
                    text: "Keep the fixture model selection".into(),
                }],
                dsh_llm::MessageSource::User {
                    rpc_id: None,
                    client_time_zone: None,
                },
            ));
            tokio::time::timeout(std::time::Duration::from_secs(8), async {
                while adapter.turns.load(Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
                handle.agent.when_idle().await;
            })
            .await
            .unwrap();
            handle.dispose.await;
            drop(handle.agent);
            assert!(
                host.agents
                    .get(&dsh_session::session_id(&session_id))
                    .is_none()
            );
            toggle(&host, true).await;
            let record = rpc(&host,"schedule.create",json!({"sessionId":session_id,"title":"Review overdue build","prompt":"Check the completed build output","after_seconds":1})).await;
            task_id = record["id"].as_str().unwrap().into();
            toggle(&host, false).await;
            tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
            assert!(
                adapter.delivered.lock().is_empty(),
                "a disabled plugin cannot cold wake the owner"
            );
        } else {
            assert_eq!(
                current["config"],
                json!({"deliveryHistoryDays":7,"deliveryHistoryRecords":2})
            );
            let disabled_history = rpc(
                &host,
                "schedule.history",
                json!({"sessionId":session_id,"id":task_id,"limit":100}),
            )
            .await;
            assert_eq!(disabled_history["retention"], json!({"days":7,"records":2}));
            assert_eq!(
                rpc(&host, "schedule.catalog", json!({}))
                    .await
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            assert!(
                host.agents
                    .get(&dsh_session::session_id(&session_id))
                    .is_none()
            );
            toggle(&host, true).await;
            let completed = tokio::time::timeout(std::time::Duration::from_secs(15), async {
                loop {
                    let catalog = schedule.catalog().await.unwrap();
                    if catalog
                        .first()
                        .is_some_and(|task| task["status"] == "inactive")
                        && !adapter.delivered.lock().is_empty()
                        && host
                            .agents
                            .get(&dsh_session::session_id(&session_id))
                            .is_none()
                    {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await;
            if completed.is_err() {
                let catalog = schedule.catalog().await.unwrap();
                let agent = host.agents.get(&dsh_session::session_id(&session_id));
                let status = agent
                    .as_ref()
                    .map(|agent| (agent.status(), agent.inbox().has_pending()));
                let logs: Vec<_> = ctx
                    .logger
                    .buffer
                    .lock()
                    .iter()
                    .flat_map(|entry| entry.args.iter())
                    .filter_map(cordis::downcast::<String>)
                    .cloned()
                    .collect();
                let pending_plugins: Vec<_> = ctx.registry.values().into_iter()
                    .flat_map(|runtime| runtime.fibers.snapshot())
                    .filter(|fiber| matches!(fiber.state(), cordis::FiberState::Pending | cordis::FiberState::Loading))
                    .map(|fiber| json!({"name":fiber.name(),"state":format!("{:?}",fiber.state()),"inject":fiber.inject.keys().collect::<Vec<_>>() }))
                    .collect();
                panic!(
                    "overdue reminder did not complete: runtime={} catalog={catalog:?} provider_deliveries={:?} live={status:?} logs={logs:?} pendingPlugins={pending_plugins:?}",
                    schedule.diagnostics(),
                    adapter.delivered.lock()
                );
            }
            let history = rpc(
                &host,
                "schedule.history",
                json!({"sessionId":session_id,"id":task_id,"limit":100}),
            )
            .await;
            assert_eq!(history["records"].as_array().unwrap().len(), 1);
            let receipt = &history["records"][0];
            assert_eq!(receipt["prompt"], "Check the completed build output");
            let sent = adapter.delivered.lock();
            assert_eq!(sent.len(), 1);
            assert_eq!(receipt["messageId"], sent[0].0);
            assert!(sent[0].1.contains("Check the completed build output"));
            drop(sent);
            let persisted = host
                .persistence
                .load(&dsh_session::session_id(&session_id))
                .await
                .unwrap();
            assert!(persisted.events.iter().any(|event| {
                event.type_ == "agent/inbox/spliced"
                    && event
                        .data
                        .to_string()
                        .contains(receipt["messageId"].as_str().unwrap())
            }));
            let legacy: Vec<_> = persisted
                .events
                .iter()
                .filter(|event| event.type_ == "schedule/change")
                .collect();
            assert_eq!(
                legacy.len(),
                2,
                "Host delivery must not rewrite or import legacy reminders"
            );
            assert_eq!(legacy[0].data["schedule"].get("title"), None);
            assert_eq!(legacy[1].data["schedule"]["title"], "Saved legacy name");
            for _ in 0..3 {
                schedule.request_drive();
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            assert_eq!(
                adapter.delivered.lock().len(),
                1,
                "repeated wakes cannot redeliver the finished one-shot"
            );
            toggle(&host, false).await;
        }
        host.shutdown().await.unwrap();
        drop(schedule);
        drop(host);
        drop(ctx);
    }
    assert!(
        root.canonicalize()
            .unwrap()
            .starts_with(std::env::temp_dir().canonicalize().unwrap())
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_preset_cold_restore_returns_error_and_releases_admission_and_writer() {
    let root = std::env::temp_dir().join(format!("host-missing-preset-{}", uuid::Uuid::new_v4()));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let ctx = Context::root();
    let host = compose_persistent_host_at(&ctx, &root, Some("web")).unwrap();
    let adapter = Arc::new(ReminderAdapter {
        turns: AtomicUsize::new(0),
        delivered: Default::default(),
    });
    host.llm
        .register_adapter(&ctx, vec!["reminder-fixture".into()], adapter.clone())
        .unwrap();
    let missing = format!("missing-preset-{}", uuid::Uuid::new_v4());
    let handle = host
        .agent_loop
        .create_agent(
            &ctx,
            CreateAgentOptions {
                meta: Some(dsh_session::CreateSessionMeta {
                    cwd: Some(workspace.to_string_lossy().into_owned()),
                    agent_preset: Some(missing.clone()),
                    ..Default::default()
                }),
                agent_options: Some(AgentOptions {
                    provider: Some("reminder-fixture".into()),
                    model: Some("fixture".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let id = handle.agent.id().clone();
    handle.agent.followup(dsh_llm::create_user_message(
        vec![dsh_llm::ContentBlock::Text {
            text: "Persist a session whose preset is later unavailable".into(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ));
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        while adapter.turns.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        handle.agent.when_idle().await;
    })
    .await
    .unwrap();
    let seed_session = handle.agent.session().clone();
    assert!(
        host.sessions.flush(&seed_session).await.unwrap(),
        "the seed must cross the real persistence barrier before testing cold recovery"
    );
    let source =
        std::path::PathBuf::from(host.persistence.locate(seed_session.header()).unwrap().path);
    assert!(
        source.is_file(),
        "the backend must publish its complete seed artifact"
    );
    handle.dispose.await;
    drop(handle.agent);
    drop(seed_session);
    let original = std::fs::read(&source).unwrap();
    for _ in 0..2 {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            host.api_proxy.try_resolve_control_agent(id.as_str()),
        )
        .await
        .expect("missing-preset restoration must finish its rollback and return an error");
        let error = result
            .err()
            .expect("the unavailable preset must fail, not leave admission busy");
        assert!(error.contains(&missing), "{error}");
        assert!(host.agents.get(&id).is_none());
        assert!(host.sessions.get(&id).is_none());
        assert_eq!(std::fs::read(&source).unwrap(), original);
    }
    let probe_ctx = Context::root();
    dsh_session::SessionStore::install(&probe_ctx);
    let probe = JsonlSessionPersistence::install(
        &probe_ctx,
        JsonlConfig {
            root: root.join("sessions").to_string_lossy().into_owned(),
            ..Default::default()
        },
    )
    .unwrap();
    let prepared = tokio::time::timeout(std::time::Duration::from_secs(3), probe.prepare(&id))
        .await
        .expect("failed restoration must release its writer lease")
        .unwrap();
    drop(prepared);
    probe_ctx.fiber.dispose().await;
    drop(probe);
    drop(probe_ctx);
    assert_eq!(std::fs::read(&source).unwrap(), original);
    host.shutdown().await.unwrap();
    drop(host);
    drop(ctx);
    assert!(
        root.canonicalize()
            .unwrap()
            .starts_with(std::env::temp_dir().canonicalize().unwrap())
    );
    std::fs::remove_dir_all(root).unwrap();
}
