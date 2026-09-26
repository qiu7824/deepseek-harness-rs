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
