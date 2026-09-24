use cordis::Context;
use dsh_session::{SessionEvent, SessionHeader, SessionStore, session_id};
use dsh_session_persistence::{SessionPersistenceApi, SessionReadWindowRequest};
use dsh_session_persistence_jsonl::{
    JsonlCompression, JsonlConfig, JsonlSessionPersistence, log_path,
};
use serde_json::{Value, json};

fn events(legacy: bool) -> Vec<SessionEvent> {
    let mut rows = vec![
        json!({"type":"turn/start","data":{"turn":1}}),
        json!({"type":"step/start","data":{"turn":1,"step":1}}),
        json!({"type":"system/message","surfaceOp":"append","data":{"turn":1,"step":1,"message":{"id":"s","role":"system","source":{"kind":"system-prompt"},"content":[]}}}),
        json!({"type":"user/message","surfaceOp":"append","data":{"id":"u","role":"user","source":{"kind":"user"},"content":[{"type":"text","text":"work"}]}}),
        json!({"type":"request/header","data":{"header":{"config":{"provider":"mock","model":"model"}}}}),
        json!({"type":"assistant/message","surfaceOp":"append","data":{"turn":1,"step":1,"message":{"id":"a","role":"assistant","source":{"kind":"model","provider":"mock","model":"model"},"content":[{"type":"tool-call","id":"c","name":"read","arguments":"{}"}]}}}),
        json!({"type":"tool/call","data":{"turn":1,"step":1,"callId":"c","name":"read","arguments":"{}"}}),
        json!({"type":"tool/result","surfaceOp":"append","data":{"turn":1,"step":1,"message":{"id":"r","role":"tool","source":{"kind":"tool","callId":"c"},"toolCallId":"c","isError":false,"content":[{"type":"text","text":"result"}]}}}),
        json!({"type":"step/end","data":{"turn":1,"step":1}}),
        json!({"type":"turn/end","data":{"turn":1,"reason":{"kind":"completed"}}}),
    ];
    if legacy {
        rows[2]["data"]["message"]["source"] =
            json!({"kind":"plugin","plugin":"@deepseek-ai/dsh-system-prompt"});
        let message = rows[7]["data"]["message"].as_object_mut().unwrap();
        let content = message.remove("content").unwrap();
        message.remove("toolCallId");
        message.remove("isError");
        message.insert("role".into(), json!("user"));
        message.insert(
            "content".into(),
            json!([{"type":"tool-result","toolCallId":"c","isError":false,"content":content}]),
        );
    }
    rows.into_iter()
        .enumerate()
        .map(|(seq, mut row)| {
            row["seq"] = json!(seq);
            row["time"] = json!(seq);
            serde_json::from_value(row).unwrap()
        })
        .collect()
}
fn header(id: &str) -> SessionHeader {
    serde_json::from_value(
        json!({"version":4,"id":id,"createdAt":0,"isSeeded":false,"delegationDepth":0}),
    )
    .unwrap()
}
async fn close(ctx: &Context) {
    for disposer in ctx.fiber.disposables.clear() {
        disposer().await;
    }
}

#[tokio::test]
async fn ordinary_backend_writes_reads_pages_and_restores_native_v4() {
    assert_eq!(dsh_session::SESSION_FORMAT_VERSION, 4);
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let root = std::env::temp_dir().join(format!("native-v4-runtime-{}", uuid::Uuid::new_v4()));
        let ctx = Context::root();
        SessionStore::install(&ctx);
        let backend = JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root: root.to_string_lossy().into_owned(),
                compression,
                ..Default::default()
            },
        )
        .unwrap();
        let meta = header("native");
        let rows = events(false);
        backend.create(meta.clone(), None).await.unwrap();
        backend.append(&meta.id, &rows).await.unwrap();
        let path = log_path(&root.to_string_lossy(), None, &meta.id, compression);
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("session.v4.")
        );
        let verified = dsh_session_persistence_jsonl::v4_artifact::validate_v4_artifact(
            &path,
            compression,
            meta.id.as_str(),
            &|| false,
        )
        .unwrap();
        assert_eq!(verified.lifecycle.event_count, 10);
        let listed = backend.read_list_metadata(&meta.id).await.unwrap();
        assert_eq!(listed.last_seq, 9);
        assert_eq!(listed.meta.version, 4);
        let window = backend
            .read_window(
                &meta.id,
                SessionReadWindowRequest {
                    before_seq: None,
                    max_messages: 10,
                    max_events: 30,
                },
            )
            .await
            .unwrap();
        assert!(
            window
                .events
                .iter()
                .any(|event| event.data.pointer("/message/role") == Some(&json!("tool")))
        );
        let chunk = backend.read_event_chunk(&meta.id, 5, 2).await.unwrap();
        assert_eq!(chunk.events.len(), 2);
        assert_eq!(chunk.next_seq, Some(7));
        let restored = backend.load(&meta.id).await.unwrap();
        assert_eq!(restored.meta.version, 4);
        let raw = backend.read_raw(&meta.id).await.unwrap().unwrap();
        let first: Value = serde_json::from_str(raw.content.lines().next().unwrap()).unwrap();
        assert_eq!(first["version"], 4);
        assert_eq!(first["isSeeded"], false);
        assert!(first.get("seedLength").is_none());
        close(&ctx).await;
        drop(backend);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn opening_legacy_v3_publishes_a_native_successor_and_retains_original_bytes() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let root =
            std::env::temp_dir().join(format!("native-v4-migration-{}", uuid::Uuid::new_v4()));
        let id = session_id("old");
        let directory =
            dsh_session_persistence_jsonl::session_dir(&root.to_string_lossy(), None, &id);
        std::fs::create_dir_all(&directory).unwrap();
        let original = directory.join(format!(
            "session{}",
            dsh_session_persistence_jsonl::log_suffix(compression)
        ));
        let header = format!(
            "{}\n",
            json!({"type":"session","version":3,"id":"old","createdAt":0,"delegationDepth":0})
        );
        let body = format!(
            "{}\n",
            dsh_session_persistence_jsonl::event_lines(&events(true), false)
        );
        let bytes = if compression == JsonlCompression::None {
            format!("{header}{body}").into_bytes()
        } else {
            let mut out =
                dsh_session_persistence_jsonl::compress_zstd_frame(header.as_bytes()).unwrap();
            out.extend(
                dsh_session_persistence_jsonl::compress_zstd_frame(body.as_bytes()).unwrap(),
            );
            out
        };
        std::fs::write(&original, &bytes).unwrap();
        let ctx = Context::root();
        SessionStore::install(&ctx);
        let backend = JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root: root.to_string_lossy().into_owned(),
                compression,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(backend.list().await.unwrap()[0].version, 3);
        let restored = backend.load(&id).await.unwrap();
        assert_eq!(restored.meta.version, 4);
        assert_eq!(std::fs::read(&original).unwrap(), bytes);
        let successor =
            dsh_session_persistence_jsonl::generations::generation_path(&directory, 4, compression)
                .unwrap();
        dsh_session_persistence_jsonl::v4_artifact::validate_v4_artifact(
            &successor,
            compression,
            id.as_str(),
            &|| false,
        )
        .unwrap();
        assert_eq!(
            backend.read_list_metadata(&id).await.unwrap().meta.version,
            4
        );
        close(&ctx).await;
        drop(backend);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn seeded_native_history_preserves_cuts_references_and_actual_encoding_after_restart() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let root = std::env::temp_dir().join(format!("native-v4-fork-{}", uuid::Uuid::new_v4()));
        let ctx = Context::root();
        SessionStore::install(&ctx);
        let backend = JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root: root.to_string_lossy().into_owned(),
                compression,
                ..Default::default()
            },
        )
        .unwrap();
        let mut meta = header("child");
        meta.is_seeded = true;
        meta.parent_session = Some(session_id("parent"));
        let source = events(false);
        let inherited = dsh_session::SessionLogOffset::new(source.len() as u64).unwrap();
        let child = dsh_session::Session::create(
            meta.id.clone(),
            Some(source.clone()),
            Some(&meta),
            Some(inherited),
        )
        .unwrap();
        child
            .append(
                "user/message",
                json!({"id":"followup","role":"user","source":{"kind":"user"},"content":[]}),
                Some(dsh_session::SurfaceIntent {
                    surface_op: dsh_session::SurfaceOp::Append,
                    source_event_seqs: Some(vec![2, 3, 4]),
                }),
            )
            .unwrap();
        backend
            .create(child.header().clone(), Some(inherited))
            .await
            .unwrap();
        backend.append(child.id(), &child.events()).await.unwrap();
        let path = log_path(&root.to_string_lossy(), None, child.id(), compression);
        let raw = backend.read_raw(child.id()).await.unwrap().unwrap();
        assert_eq!(raw.inherited_event_count, inherited);
        assert!(raw.content.contains("\"sourceEventSeqs\":[[2,4]]"));
        close(&ctx).await;
        drop(backend);
        let ctx = Context::root();
        SessionStore::install(&ctx);
        let other = if compression == JsonlCompression::None {
            JsonlCompression::Zstd
        } else {
            JsonlCompression::None
        };
        let backend = JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root: root.to_string_lossy().into_owned(),
                compression: other,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            backend.locate(child.header()).unwrap().path,
            path.to_string_lossy()
        );
        let restored = backend.load(child.id()).await.unwrap();
        assert_eq!(restored.inherited_event_count, inherited);
        assert_eq!(
            restored.events.last().unwrap().source_event_seqs,
            Some(vec![2, 3, 4])
        );
        let read = backend.read_user_message_events(child.id()).await.unwrap();
        assert_eq!(read.inherited_event_count, inherited);
        assert_eq!(&restored.events[..source.len()], source);
        close(&ctx).await;
        drop(backend);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn torn_legacy_frame_migrates_its_recovered_prefix_without_modifying_the_source() {
    let root = std::env::temp_dir().join(format!("native-v4-torn-{}", uuid::Uuid::new_v4()));
    let id = session_id("torn-old");
    let directory = dsh_session_persistence_jsonl::session_dir(&root.to_string_lossy(), None, &id);
    std::fs::create_dir_all(&directory).unwrap();
    let original = directory.join("session.jsonl.zstd");
    let first = format!(
        "{}\n",
        json!({"type":"session","version":3,"id":id,"createdAt":0,"delegationDepth":0})
    );
    let body = format!(
        "{}\n",
        dsh_session_persistence_jsonl::event_lines(&events(true), false)
    );
    let mut bytes = dsh_session_persistence_jsonl::compress_zstd_frame(first.as_bytes()).unwrap();
    let mut frame = dsh_session_persistence_jsonl::compress_zstd_frame(body.as_bytes()).unwrap();
    frame.pop();
    bytes.extend(frame);
    std::fs::write(&original, &bytes).unwrap();
    let ctx = Context::root();
    SessionStore::install(&ctx);
    let backend = JsonlSessionPersistence::install(
        &ctx,
        JsonlConfig {
            root: root.to_string_lossy().into_owned(),
            compression: JsonlCompression::Zstd,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(backend.load(&id).await.unwrap().events.len(), 10);
    assert_eq!(std::fs::read(original).unwrap(), bytes);
    close(&ctx).await;
    drop(backend);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn cold_restore_refuses_a_native_semantic_corruption_without_modifying_its_raw_export() {
    let root = std::env::temp_dir().join(format!("native-v4-invalid-{}", uuid::Uuid::new_v4()));
    let meta = header("invalid");
    let path = log_path(
        &root.to_string_lossy(),
        None,
        &meta.id,
        JsonlCompression::None,
    );
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut rows = events(false);
    rows[8].data["step"] = json!(2);
    let first =
        serde_json::to_value(dsh_session_persistence_jsonl::to_header_line(&meta, None).unwrap())
            .unwrap();
    let bytes = format!(
        "{first}\n{}\n",
        dsh_session_persistence_jsonl::event_lines(&rows, false)
    )
    .into_bytes();
    std::fs::write(&path, &bytes).unwrap();
    let ctx = Context::root();
    SessionStore::install(&ctx);
    let backend = JsonlSessionPersistence::install(
        &ctx,
        JsonlConfig {
            root: root.to_string_lossy().into_owned(),
            compression: JsonlCompression::None,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        backend
            .prepare(&meta.id)
            .await
            .err()
            .unwrap()
            .contains("invalid V4 lifecycle")
    );
    assert_eq!(
        backend
            .read_raw(&meta.id)
            .await
            .unwrap()
            .unwrap()
            .content
            .as_bytes(),
        bytes
    );
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    close(&ctx).await;
    drop(backend);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn interrupted_native_tool_calls_recover_flat_error_results_once() {
    for started in [false, true] {
        let root = std::env::temp_dir().join(format!("native-v4-interrupted-{}", uuid::Uuid::new_v4()));
        let meta = header("interrupted");
        let ctx = Context::root(); SessionStore::install(&ctx);
        let backend = JsonlSessionPersistence::install(&ctx, JsonlConfig { root:root.to_string_lossy().into_owned(), ..Default::default() }).unwrap();
        backend.create(meta.clone(), None).await.unwrap();
        backend.append(&meta.id, &events(false)[..if started {7} else {6}]).await.unwrap();
        close(&ctx).await; drop(backend);
        let ctx = Context::root(); SessionStore::install(&ctx);
        let backend = JsonlSessionPersistence::install(&ctx, JsonlConfig { root:root.to_string_lossy().into_owned(), ..Default::default() }).unwrap();
        let restored = backend.load(&meta.id).await.unwrap();
        let results: Vec<_> = restored.events.iter().filter(|event| event.type_ == "tool/result").collect();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].data["message"]["role"], "tool");
        assert_eq!(results[0].data["message"]["isError"], true);
        assert_eq!(results[0].data["error"]["code"], if started {"TOOL_OUTCOME_UNKNOWN"} else {"TOOL_NOT_STARTED"});
        assert_eq!(backend.load(&meta.id).await.unwrap().events, restored.events);
        let path = log_path(&root.to_string_lossy(), None, &meta.id, JsonlCompression::Zstd);
        dsh_session_persistence_jsonl::v4_artifact::validate_v4_artifact(&path, JsonlCompression::Zstd, meta.id.as_str(), &|| false).unwrap();
        close(&ctx).await; drop(backend); std::fs::remove_dir_all(root).unwrap();
    }
}
