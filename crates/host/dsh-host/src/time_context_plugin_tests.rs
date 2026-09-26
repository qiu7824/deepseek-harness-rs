use super::*;
use dsh_agent::{AgentFactory, AgentOptions, CreateAgentOptions};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

struct ClockAdapter(AtomicUsize, parking_lot::Mutex<Vec<Vec<String>>>);
impl dsh_llm::LlmAdapter for ClockAdapter {
    fn stream(&self, options: &dsh_llm::GenerateOptions) -> dsh_llm::ChunkStream {
        if options.agent_loop_request {
            self.1.lock().push(
                options
                    .messages
                    .iter()
                    .filter(|message| message.source.plugin_name() == Some("time-context"))
                    .map(|message| message.id.to_string())
                    .collect(),
            );
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        Box::pin(futures::stream::iter([
            dsh_llm::StreamChunk::BlockStart {
                index: 0,
                block_type: "text".into(),
            },
            dsh_llm::StreamChunk::TextDelta {
                index: 0,
                text: "done".into(),
            },
            dsh_llm::StreamChunk::BlockEnd {
                index: 0,
                block: dsh_llm::ContentBlock::Text {
                    text: "done".into(),
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
    let response = to_fetch_handler(host.api_proxy.clone())
        .handle(CarrierRequest {
            method: http::Method::POST,
            path: format!("/api/{method}"),
            query: vec![],
            headers: vec![("content-type".into(), "application/json".into())],
            body: Some(json!({"type":"client-request","rpcId":uuid::Uuid::new_v4().to_string(),"method":method,"payload":payload}).to_string().into_bytes()),
        })
        .await;
    let CarrierBody::Bytes(bytes) = response.into_body() else {
        panic!("configuration RPC is unary")
    };
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["result"]["ok"], true, "{value}");
    value["result"]["value"].clone()
}

async fn configure(host: &HostSpine, config: Value) -> Value {
    let current = rpc(
        host,
        "pluginInventory.getConfig",
        json!({"entryId":"dsh-time-context"}),
    )
    .await;
    rpc(host, "pluginInventory.setConfig", json!({"entryId":"dsh-time-context","expectedRevision":current["revision"],"config":config})).await
}

fn readings(agent: &dyn dsh_agent::Agent) -> usize {
    let mut count = 0;
    agent
        .session()
        .visit_events(0, None, |event| {
            if event.type_ == "user/message"
                && (event.data["source"]["kind"] == "time-context"
                    || event.data["source"]["plugin"] == "time-context")
            {
                count += 1;
            }
            Ok(true)
        })
        .unwrap();
    count
}

fn prompt(agent: &dyn dsh_agent::Agent) {
    agent.followup(dsh_llm::create_user_message(
        vec![dsh_llm::ContentBlock::Text {
            text: "clock context".into(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    ));
}

async fn finish(agent: &dyn dsh_agent::Agent, adapter: &ClockAdapter, before: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        while adapter.0.load(Ordering::SeqCst) == before {
            tokio::task::yield_now().await;
        }
        agent.when_idle().await;
    })
    .await
    .unwrap();
    assert_eq!(adapter.0.load(Ordering::SeqCst), before + 1);
}

async fn turn(agent: &dyn dsh_agent::Agent, adapter: &ClockAdapter) {
    let before = adapter.0.load(Ordering::SeqCst);
    prompt(agent);
    finish(agent, adapter, before).await;
}

async fn toggle(host: &HostSpine, enabled: bool) {
    host.api_proxy
        .apply_plugin_enablement(
            "dsh-time-context".into(),
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
async fn production_loader_time_context_preserves_profile_and_controls_existing_agent_steps() {
    let root = std::env::temp_dir().join(format!("host-time-plugin-{}", uuid::Uuid::new_v4()));
    let profile_path = root.join("profiles/web/plugins.json");
    for restarted in [false, true] {
        let ctx = Context::root();
        let host = compose_persistent_host_at(&ctx, &root, Some("web")).unwrap();
        let loader = ctx
            .get_typed::<Arc<dsh_cordis_loader::LoaderService>>("loader", false)
            .unwrap();
        let entry = loader.tree.resolve("dsh-time-context").unwrap();
        let snapshot = rpc(
            &host,
            "pluginInventory.getConfig",
            json!({"entryId":"dsh-time-context"}),
        )
        .await;
        if restarted {
            assert!(!entry.disabled().unwrap());
            assert_eq!(
                snapshot["config"],
                json!({"refreshIntervalMs":1234567,"timeZone":"UTC"})
            );
        } else {
            assert!(
                entry.disabled().unwrap(),
                "fresh bundled time-context must be opt-in"
            );
        }
        let adapter = Arc::new(ClockAdapter(AtomicUsize::new(0), Default::default()));
        host.llm
            .register_adapter(&ctx, vec!["clock-fixture".into()], adapter.clone())
            .unwrap();
        let handle = host
            .agent_loop
            .create_agent(
                &ctx,
                CreateAgentOptions {
                    agent_options: Some(AgentOptions {
                        provider: Some("clock-fixture".into()),
                        model: Some("fixture".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let agent = handle.agent.clone();
        turn(agent.as_ref(), &adapter).await;
        assert_eq!(readings(agent.as_ref()), usize::from(restarted));
        if !restarted {
            configure(&host, json!({"refreshIntervalMs":0,"timeZone":"UTC"})).await;
            assert!(
                entry.disabled().unwrap(),
                "saving configuration must not enable the plugin"
            );
            turn(agent.as_ref(), &adapter).await;
            assert_eq!(readings(agent.as_ref()), 0);
            for expected in 1..=3 {
                toggle(&host, true).await;
                turn(agent.as_ref(), &adapter).await;
                assert_eq!(
                    readings(agent.as_ref()),
                    expected,
                    "one reading per step after repeated enablement"
                );
                let last = agent
                    .session()
                    .find_event_rev(|event| {
                        event.type_ == "user/message"
                            && event.data["source"]["kind"] == "time-context"
                    })
                    .unwrap()
                    .expect("durable V4 clock reading");
                assert!(
                    adapter
                        .1
                        .lock()
                        .last()
                        .unwrap()
                        .iter()
                        .any(|id| Some(id.as_str()) == last.data["id"].as_str()),
                    "the actual provider request must contain the newly committed clock reading"
                );
                toggle(&host, false).await;
                turn(agent.as_ref(), &adapter).await;
                assert_eq!(readings(agent.as_ref()), expected);
            }
            toggle(&host, true).await;
            configure(&host, json!({"refreshIntervalMs":1234567,"timeZone":"UTC"})).await;
            turn(agent.as_ref(), &adapter).await;
            assert_eq!(
                readings(agent.as_ref()),
                3,
                "custom interval takes effect in the existing Agent"
            );
            configure(&host, json!({"refreshIntervalMs":0,"timeZone":"UTC"})).await;

            // Hold a downstream pre-step callback while the plugin unloads.
            // Removing the hook alone cannot revoke this captured dispatch.
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let gate = ctx
                .on(
                    "agent/pre-step",
                    Arc::new({
                        let entered = entered.clone();
                        let release = release.clone();
                        move |_, args| {
                            let entered = entered.clone();
                            let release = release.clone();
                            Box::pin(async move {
                                let next =
                                    cordis::downcast_arc::<cordis::NextFn>(args.last().unwrap())
                                        .unwrap();
                                entered.notify_one();
                                release.notified().await;
                                Some(next.call().await)
                            })
                        }
                    }),
                    cordis::EventOptions::default(),
                )
                .await;
            let before = adapter.0.load(Ordering::SeqCst);
            prompt(agent.as_ref());
            tokio::time::timeout(std::time::Duration::from_secs(8), entered.notified())
                .await
                .unwrap();
            toggle(&host, false).await;
            release.notify_one();
            finish(agent.as_ref(), &adapter, before).await;
            gate().await;
            assert_eq!(
                readings(agent.as_ref()),
                3,
                "disabled generation must not inject after its pending next callback resumes"
            );
            configure(&host, json!({"refreshIntervalMs":1234567,"timeZone":"UTC"})).await;
            assert!(entry.disabled().unwrap());
            toggle(&host, true).await;
        }
        let disk: Value = serde_json::from_slice(&std::fs::read(&profile_path).unwrap()).unwrap();
        let row = disk
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "dsh-time-context")
            .unwrap();
        assert_eq!(
            row["config"],
            json!({"refreshIntervalMs":1234567,"timeZone":"UTC"})
        );
        assert_eq!(row["disabled"], false);
        handle.dispose.await;
        drop(agent);
        drop(handle.agent);
        drop(entry);
        drop(loader);
        host.shutdown().await.unwrap();
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
