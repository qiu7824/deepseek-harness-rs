use super::*;
use dsh_session_persistence_jsonl::{
    JsonlConfig, JsonlSessionPersistence, compress_zstd_frame, session_dir,
};
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread")]
async fn cold_session_list_keeps_v3_history_beside_v4_without_rewriting_logs() {
    for with_cache in [false, true] {
        let root = std::env::temp_dir().join(format!("dsh-list-compat-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let ctx = Context::root();
        dsh_session::SessionStore::install(&ctx);
        let projections = dsh_session_projection::SessionProjectionRegistry::install(&ctx);
        dsh_session_title::SessionTitleService::install_with_registry(
            &ctx,
            dsh_session_title::Config {
                fallback_max_words: 8,
                fallback_max_bytes: 96,
                max_title_bytes: 256,
            },
            Some(projections),
        )
        .unwrap();
        let mut originals = Vec::new();
        for (id, version) in [("historical", 3), ("current", 4)] {
            let dir = session_dir(
                &root.to_string_lossy(),
                Some("C:/project"),
                &dsh_session::session_id(id),
            );
            std::fs::create_dir_all(&dir).unwrap();
            let header = json!({
                "type":"session", "version":version, "id":id, "createdAt":1,
                "cwd":"C:/project", "delegationDepth":0,
                "isSeeded":false
            });
            let mut bytes = compress_zstd_frame(format!("{header}\n").as_bytes()).unwrap();
            if version == 3 {
                let rows = [
                    json!({"seq":0,"time":2,"type":"user/message","surfaceOp":"append",
                        "data":{"id":"user-1","role":"user","source":{"kind":"user"},
                            "content":[{"type":"text","text":"历史会话"}]}}),
                    json!({"seq":1,"time":3,"type":"turn/start","data":{"turn":1}}),
                    json!({"seq":2,"time":4,"type":"session/title",
                        "data":{"title":"历史标题","messageSeqs":[0],"source":{"kind":"user"}}}),
                    json!({"seq":3,"time":5,"type":"model/selection",
                        "data":{"provider":"test","model":"test-model"}}),
                    json!({"seq":4,"time":6,"type":"turn/end",
                        "data":{"turn":1,"reason":{"kind":"completed"}}}),
                ];
                let rows = rows
                    .into_iter()
                    .map(|row| serde_json::from_value(row).unwrap())
                    .collect::<Vec<dsh_session::SessionEvent>>();
                let body = dsh_session_persistence_jsonl::event_lines(&rows, false) + "\n";
                bytes.extend(compress_zstd_frame(body.as_bytes()).unwrap());
            }
            let path = dir.join(if version == 3 {
                "session.jsonl.zstd"
            } else {
                "session.v4.jsonl.zstd"
            });
            std::fs::write(&path, &bytes).unwrap();
            originals.push((path, bytes));
        }
        let persistence = JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root: root.to_string_lossy().into_owned(),
                ..Default::default()
            },
        )
        .unwrap();
        let mut registration = None;
        if with_cache {
            let storage = dsh_storage::Storage::install(&ctx);
            registration = Some(
                storage
                    .backend
                    .register(
                        "memory",
                        Arc::new(dsh_storage_test_support::MemoryStorageBackend::new(
                            Arc::new(dsh_storage_test_support::MemoryMediaPool::new()),
                        )),
                    )
                    .unwrap(),
            );
            let facility = dsh_storage_domain::DomainFacility::install(
                &ctx,
                dsh_storage_domain::DomainFacilityConfig {
                    backend: "memory".into(),
                    routes: Default::default(),
                },
            )
            .unwrap();
            dsh_session_projection_cache::SessionProjectionCache::install(
                &ctx,
                dsh_session_projection_cache::Config {
                    write_every_events: 64,
                    write_interval_ms: 1000,
                },
                &facility,
                persistence.clone(),
            )
            .unwrap();
        }
        let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
        assert!(
            service
                .request_control_scope(&dsh_session::session_id("historical"), None)
                .await
                .is_ok(),
            "a readable V3 session retains its cancellation and prompt authority"
        );
        let response = crate::fetch::handler::to_fetch_handler(service.clone())
            .handle(crate::fetch::handler::CarrierRequest {
                method: http::Method::POST,
                path: "/api/session.list".into(),
                query: vec![],
                headers: vec![("content-type".into(), "application/json".into())],
                body: Some(
                    serde_json::to_vec(&json!({"type":"client-request","rpcId":"legacy-list",
                    "method":"session.list","payload":{}}))
                    .unwrap(),
                ),
            })
            .await;
        let Body::Bytes(bytes) = response.into_body() else {
            panic!("unary response")
        };
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result["result"]["ok"], true, "{result}");
        let items = result["result"]["value"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 2, "old logs must remain visible: {result}");
        assert_eq!(items[0]["sessionId"], "historical");
        assert_eq!(items[0]["blank"], false);
        assert_eq!(items[0]["updatedAt"], 2);
        assert_eq!(items[1]["sessionId"], "current");
        if with_cache {
            let summary = &items[0]["projections"];
            assert_eq!(summary["asOfSeq"], 4);
            assert_eq!(summary["values"]["title"], "历史标题");
            assert_eq!(summary["values"]["modelSelection"]["model"], "test-model");
            assert_eq!(
                summary["values"]["sessionListMetadata"]["lastTurnReason"],
                "completed"
            );
            assert_eq!(
                summary["values"].as_object().unwrap().len(),
                3,
                "listing does not seed transcript or navigation projections"
            );
            let cache = ctx
                .get_typed::<Arc<dsh_session_projection_cache::SessionProjectionCache>>(
                    "sessionProjectionCache",
                    false,
                )
                .unwrap();
            let header = dsh_session_persistence::SessionPersistenceApi::read_snapshot(
                persistence.as_ref(),
                &dsh_session::session_id("historical"),
            )
            .await
            .unwrap()
            .unwrap()
            .header;
            assert!(
                cache.cached_snapshot(&header).is_none(),
                "subagent and full-history consumers cannot mistake a list checkpoint for a complete snapshot"
            );
            assert_eq!(cache.cached_list_snapshot(&header).unwrap().as_of_seq, 4);
            let full = cache
                .cold_snapshot(&dsh_session::session_id("historical"))
                .await
                .unwrap();
            assert!(
                full.values
                    .contains_key(dsh_session_title::USER_MESSAGE_RAIL_KEY),
                "opening a task still fills projections omitted from the list"
            );
        }
        for (path, bytes) in originals {
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert_eq!(
                std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
                1,
                "listing does not migrate or take a writer lease"
            );
        }
        for dispose in ctx.fiber.disposables.clear() {
            dispose().await;
        }
        drop(service);
        drop(persistence);
        drop(registration);
        drop(ctx);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cold_session_list_preserves_child_usage_and_packed_tail_timing_after_restart() {
    use dsh_session::format_v4::{
        V3Dialect, V3ToV4Transform, V4Vocabulary, encode_v4_event, encode_v4_header,
    };
    use dsh_session_persistence::SessionPersistenceApi;

    for version in [3, 4] {
        let root = std::env::temp_dir().join(format!("dsh-list-metrics-{}", uuid::Uuid::new_v4()));
        let id = dsh_session::session_id("historical-child");
        let dir = session_dir(&root.to_string_lossy(), Some("C:/project"), &id);
        std::fs::create_dir_all(&dir).unwrap();
        let header = json!({"version":3,"id":id,"createdAt":1,"cwd":"C:/project",
            "parentSession":"parent","origin":"subagent","delegationDepth":1,"isSeeded":false});
        let source = [
            (
                1,
                "subagent/descriptor",
                json!({"version":3,"provider":"spawn","mode":"continuable","label":"历史子任务"}),
            ),
            (
                2,
                "user/message",
                json!({"id":"u1","role":"user","source":{"kind":"user"},"content":[{"type":"text","text":"任务"}]}),
            ),
            (1000, "turn/start", json!({"turn":1})),
            (1050, "step/start", json!({"turn":1,"step":1})),
            (
                2000,
                "assistant/chunk",
                json!({"turn":1,"step":1,"chunk":{"type":"usage","usage":{"inputTokens":100,"outputTokens":10}}}),
            ),
            (
                3000,
                "assistant/chunk",
                json!({"turn":1,"step":1,"chunk":{"type":"text-delta","index":0,"text":"第一段"}}),
            ),
            (
                4000,
                "assistant/chunk",
                json!({"turn":1,"step":1,"chunk":{"type":"text-delta","index":0,"text":"第二段"}}),
            ),
            (
                5000,
                "assistant/message",
                json!({"turn":1,"step":1,"message":{"id":"a1","role":"assistant","source":{"kind":"model","provider":"test","model":"test-model"},"content":[{"type":"text","text":"完成"}]},"usage":{"inputTokens":150,"outputTokens":30,"cacheReadTokens":10,"cacheWriteTokens":5}}),
            ),
            (7000, "step/end", json!({"turn":1,"step":1})),
            (
                8000,
                "turn/end",
                json!({"turn":1,"reason":{"kind":"completed"}}),
            ),
            (9000, "turn/start", json!({"turn":2})),
            (9050, "step/start", json!({"turn":2,"step":1})),
            (
                9100,
                "assistant/chunk",
                json!({"turn":2,"step":1,"chunk":{"type":"usage","usage":{"inputTokens":60,"outputTokens":8}}}),
            ),
            (
                9200,
                "assistant/chunk",
                json!({"turn":2,"step":1,"chunk":{"type":"text-delta","index":0,"text":"未结束"}}),
            ),
            (
                9300,
                "assistant/chunk",
                json!({"turn":2,"step":1,"chunk":{"type":"text-delta","index":0,"text":"流式"}}),
            ),
            (
                9500,
                "assistant/chunk",
                json!({"turn":2,"step":1,"chunk":{"type":"text-delta","index":0,"text":"的末尾"}}),
            ),
        ];
        let rows: Vec<Value> = source
            .into_iter()
            .enumerate()
            .map(|(seq, (time, kind, data))| {
                let mut value = json!({"seq":seq,"time":time,"type":kind,"data":data});
                if matches!(kind, "user/message" | "assistant/message") {
                    value["surfaceOp"] = json!("append");
                }
                value
            })
            .collect();
        let (physical_header, body, expected_last_seq) = if version == 3 {
            let mut physical = header.clone();
            physical["type"] = json!("session");
            let events = rows
                .iter()
                .cloned()
                .map(|value| serde_json::from_value(value).unwrap())
                .collect::<Vec<dsh_session::SessionEvent>>();
            let body = dsh_session_persistence_jsonl::event_lines(&events, true);
            assert!(
                body.contains("\"type\":\"text-chunks\""),
                "fixture contains a packed interrupted tail"
            );
            (physical, body, rows.len() as i64 - 1)
        } else {
            let mut transform =
                V3ToV4Transform::new(header, Some(vec![]), Some(0), V3Dialect::Rust).unwrap();
            let mut converted = Vec::new();
            for row in rows {
                converted.extend(transform.push(row).unwrap());
            }
            let (summary, tail) = transform.finish().unwrap();
            converted.extend(tail);
            let last = converted.last().unwrap()["seq"].as_i64().unwrap();
            let body = converted
                .into_iter()
                .map(|row| {
                    encode_v4_event(row, &V4Vocabulary::default())
                        .unwrap()
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join("\n");
            (
                encode_v4_header(summary.header, summary.inherited_event_count).unwrap(),
                body,
                last,
            )
        };
        let mut original = compress_zstd_frame(format!("{physical_header}\n").as_bytes()).unwrap();
        original.extend(compress_zstd_frame(format!("{body}\n").as_bytes()).unwrap());
        let path = dir.join(if version == 3 {
            "session.jsonl.zstd"
        } else {
            "session.v4.jsonl.zstd"
        });
        std::fs::write(&path, &original).unwrap();
        let pool = Arc::new(dsh_storage_test_support::MemoryMediaPool::new());
        for restarted in [false, true] {
            let ctx = Context::root();
            dsh_session::SessionStore::install(&ctx);
            let projections = dsh_session_projection::SessionProjectionRegistry::install(&ctx);
            dsh_session_title::SessionTitleService::install_with_registry(
                &ctx,
                dsh_session_title::Config {
                    fallback_max_words: 8,
                    fallback_max_bytes: 96,
                    max_title_bytes: 256,
                },
                Some(projections.clone()),
            )
            .unwrap();
            let _usage = projections
                .register(&ctx, dsh_token_meter::token_usage_projection_definition())
                .unwrap();
            let _timing = projections
                .register(&ctx, dsh_subagent::subagent_timing_projection_definition())
                .unwrap();
            let persistence = JsonlSessionPersistence::install(
                &ctx,
                JsonlConfig {
                    root: root.to_string_lossy().into_owned(),
                    ..Default::default()
                },
            )
            .unwrap();
            let storage = dsh_storage::Storage::install(&ctx);
            let registration = storage
                .backend
                .register(
                    "memory",
                    Arc::new(dsh_storage_test_support::MemoryStorageBackend::new(
                        pool.clone(),
                    )),
                )
                .unwrap();
            let facility = dsh_storage_domain::DomainFacility::install(
                &ctx,
                dsh_storage_domain::DomainFacilityConfig {
                    backend: "memory".into(),
                    routes: Default::default(),
                },
            )
            .unwrap();
            let cache = dsh_session_projection_cache::SessionProjectionCache::install(
                &ctx,
                dsh_session_projection_cache::Config {
                    write_every_events: 64,
                    write_interval_ms: 1000,
                },
                &facility,
                persistence.clone(),
            )
            .unwrap();
            let meta = persistence
                .read_snapshot(&id)
                .await
                .unwrap()
                .unwrap()
                .header;
            if restarted {
                assert!(
                    cache.cached_list_snapshot(&meta).is_some(),
                    "restart reuses all metric keys at one cut"
                );
            }
            let service = ApiProxyService::install(&ctx, ApiProxyDefaults::default());
            let response = crate::fetch::handler::to_fetch_handler(service.clone()).handle(crate::fetch::handler::CarrierRequest {
                method:http::Method::POST, path:"/api/session.list".into(), query:vec![],
                headers:vec![("content-type".into(), "application/json".into())],
                body:Some(serde_json::to_vec(&json!({"type":"client-request","rpcId":"metrics","method":"session.list","payload":{}})).unwrap()),
            }).await;
            let Body::Bytes(bytes) = response.into_body() else {
                panic!("unary response")
            };
            let result: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(result["result"]["ok"], true, "{result}");
            let block = &result["result"]["value"]["items"][0]["projections"];
            assert_eq!(block["asOfSeq"], expected_last_seq);
            assert_eq!(block["values"]["tokenUsage"]["uncachedInputTokens"], 210);
            assert_eq!(block["values"]["tokenUsage"]["outputTokens"], 38);
            assert_eq!(block["values"]["tokenUsage"]["cacheReadTokens"], 10);
            assert_eq!(block["values"]["tokenUsage"]["cacheWriteTokens"], 5);
            assert_eq!(
                block["values"]["subagentTiming"],
                json!({"settledMs":7000,"active":{"since":9000,"through":9500}})
            );
            assert_eq!(block["values"].as_object().unwrap().len(), 5);
            assert!(
                cache.cached_snapshot(&meta).is_none(),
                "metric summaries still omit full-history projections"
            );
            assert_eq!(
                cache.cached_list_snapshot(&meta).unwrap().as_of_seq,
                expected_last_seq
            );
            assert_eq!(std::fs::read(&path).unwrap(), original);
            assert_eq!(
                std::fs::read_dir(&dir).unwrap().count(),
                1,
                "cold list never migrates V3"
            );
            for dispose in ctx.fiber.disposables.clear() {
                dispose().await;
            }
            drop(service);
            drop(cache);
            drop(persistence);
            drop(registration);
            drop(ctx);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
