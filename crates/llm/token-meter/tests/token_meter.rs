//! Rust port of the core `packages/llm/token-meter/tests/token-meter.spec.ts`
//! and projection behaviors: replay measurement with provider usage
//! anchoring, surface folds, and the three projection units.

use std::sync::Arc;

use cordis::Context;
use dsh_session::{Session, SessionStore, session_id};
use dsh_session_projection::SessionProjectionRegistry;
use dsh_token_meter::TokenMeter;

async fn harness() -> (
    Context,
    Arc<SessionStore>,
    Arc<SessionProjectionRegistry>,
    Arc<TokenMeter>,
) {
    let ctx = Context::root();
    let store = SessionStore::install(&ctx);
    let registry = SessionProjectionRegistry::install(&ctx);
    let meter = TokenMeter::install(&ctx, Default::default());
    (ctx, store, registry, meter)
}

fn session<'a>(store: &'a SessionStore, id: &str) -> Session {
    store
        .prepare(
            Some(session_id(id)),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .expect("session")
}

fn append(session: &Session, type_: &str, data: serde_json::Value) -> dsh_session::SessionEvent {
    // Surface-eligible events require the append marker.
    let intent = match type_ {
        "user/message" | "assistant/message" => Some(dsh_session::SurfaceIntent {
            surface_op: dsh_session::SurfaceOp::Append,
            source_event_seqs: None,
        }),
        _ => None,
    };
    session.append(type_, data, intent).expect("append")
}

fn user_message(id: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "role": "user",
        "content": [{"type": "text", "text": text}],
        "source": {"kind": "user"},
    })
}

fn assistant_message(id: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "role": "assistant",
        "content": [{"type": "text", "text": text}],
        "source": {"kind": "model", "provider": "mock", "model": "mock"},
    })
}

fn header_data(provider: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "config": {"provider": provider, "model": "mock"},
            "system": "You are a helpful assistant.",
            "tools": [{"name": "read", "description": "read", "parameters": {"type": "object"}}],
        }
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn measures_zero_pressure_for_an_empty_log() {
    let (_ctx, store, _registry, meter) = harness().await;
    let session = session(&store, "empty");
    let measurement = meter.measure(&session, None);
    assert_eq!(measurement.log_revision, 0);
    assert_eq!(measurement.surface_tokens, 0);
    assert_eq!(measurement.total_tokens, 0);
    assert!(measurement.nodes.is_empty());
    assert!(matches!(
        measurement.baseline,
        dsh_token_meter::TokenMeasurementBaseline::None { .. }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn anchors_measurement_on_provider_usage_and_tracks_surface_delta() {
    let (_ctx, store, _registry, meter) = harness().await;
    let session = session(&store, "metered");
    append(&session, "turn/start", serde_json::json!({"turn": 1}));
    append(&session, "request/header", header_data("mock"));
    append(
        &session,
        "step/start",
        serde_json::json!({"turn": 1, "step": 1}),
    );
    let user = append(&session, "user/message", user_message("u1", "hello"));
    assert_eq!(user.seq, 3);
    let assistant = append(
        &session,
        "assistant/message",
        serde_json::json!({
            "turn": 1, "step": 1,
            "message": assistant_message("a1", "hi"),
            "usage": {"inputTokens": 100, "outputTokens": 50},
        }),
    );
    append(
        &session,
        "step/end",
        serde_json::json!({"turn": 1, "step": 1}),
    );
    append(
        &session,
        "turn/end",
        serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
    );

    let measurement = meter.measure(&session, None);
    // The anchor uses provider usage (100+50) which exceeds the heuristic
    // estimate of header + surface, so the baseline is usage-backed; the
    // assistant message itself joins the surface after the anchor, so the
    // total adds its heuristic price.
    assert!(matches!(
        measurement.baseline,
        dsh_token_meter::TokenMeasurementBaseline::Usage { tokens: 150, .. }
    ));
    assert!(measurement.total_tokens >= 150);
    assert_eq!(
        measurement.surface_tokens,
        measurement.nodes.iter().map(|n| n.tokens).sum::<u64>()
    );
    assert!(measurement.log_revision >= assistant.seq + 1);
    assert!(measurement.surface_delta_tokens >= 0);

    // Appending a surface message after the anchor moves the delta.
    append(&session, "turn/start", serde_json::json!({"turn": 2}));
    append(&session, "request/header", header_data("mock"));
    append(
        &session,
        "step/start",
        serde_json::json!({"turn": 2, "step": 1}),
    );
    let _user2 = append(&session, "user/message", user_message("u2", "again"));
    let measurement2 = meter.measure(&session, None);
    assert!(measurement2.total_tokens > measurement.total_tokens);
}

#[tokio::test(flavor = "multi_thread")]
async fn heuristic_baseline_when_usage_is_absent_or_below_anchor() {
    let (_ctx, store, _registry, meter) = harness().await;
    let session = session(&store, "heuristic");
    append(&session, "turn/start", serde_json::json!({"turn": 1}));
    append(&session, "request/header", header_data("mock"));
    append(
        &session,
        "step/start",
        serde_json::json!({"turn": 1, "step": 1}),
    );
    append(
        &session,
        "user/message",
        user_message("u1", "hello world, this is long"),
    );
    // No usage report: the baseline is the full heuristic estimate.
    append(
        &session,
        "assistant/message",
        serde_json::json!({"turn": 1, "step": 1, "message": assistant_message("a1", "hi")}),
    );
    append(
        &session,
        "step/end",
        serde_json::json!({"turn": 1, "step": 1}),
    );
    let measurement = meter.measure(&session, None);
    assert!(matches!(
        measurement.baseline,
        dsh_token_meter::TokenMeasurementBaseline::Estimated { .. }
    ));
    assert!(measurement.total_tokens > 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn fold_surface_replacements_need_a_valid_range() {
    let nodes = vec![
        dsh_token_meter::TokenSurfaceNode { seq: 1, tokens: 10 },
        dsh_token_meter::TokenSurfaceNode { seq: 2, tokens: 20 },
    ];
    let replace = dsh_session::SessionEvent {
        type_: "user/message".to_string(),
        seq: 3,
        time: 0,
        data: user_message("u1", "replacement"),
        ignorable: None,
        surface_op: Some(dsh_session::SurfaceOp::Replace { start: 1, end: 2 }),
        source_event_seqs: None,
    };
    let folded = dsh_token_meter::fold_surface_tokens(&nodes, &replace).unwrap();
    assert_eq!(folded.nodes.len(), 1);
    assert_eq!(folded.nodes[0].seq, 3);
    assert_eq!(folded.delta_tokens, folded.tokens as i64 - 30);

    let invalid = dsh_session::SessionEvent {
        surface_op: Some(dsh_session::SurfaceOp::Replace { start: 9, end: 10 }),
        ..replace
    };
    let error = dsh_token_meter::fold_surface_tokens(&nodes, &invalid).unwrap_err();
    assert!(error.contains("invalid current range"), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn token_usage_projection_accumulates_and_replaces_samples() {
    let (ctx, store, registry, _meter) = harness().await;
    registry
        .register(&ctx, dsh_token_meter::token_usage_projection_definition())
        .unwrap();
    let session = store
        .create(
            &ctx,
            Some(session_id("usage")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .unwrap();
    append(&session, "turn/start", serde_json::json!({"turn": 1}));
    append(
        &session,
        "step/start",
        serde_json::json!({"turn": 1, "step": 1}),
    );
    append(
        &session,
        "assistant/chunk",
        serde_json::json!({
            "turn": 1, "step": 1,
            "chunk": {"type": "usage", "usage": {"inputTokens": 10, "outputTokens": 0, "cacheReadTokens": 2}},
        }),
    );
    // A final message sample replaces the chunk sample for the same step.
    append(
        &session,
        "assistant/message",
        serde_json::json!({
            "turn": 1, "step": 1,
            "message": assistant_message("a1", "done"),
            "usage": {"inputTokens": 10, "outputTokens": 4},
        }),
    );
    append(
        &session,
        "step/end",
        serde_json::json!({"turn": 1, "step": 1}),
    );
    append(
        &session,
        "turn/end",
        serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
    );

    let snapshot = registry.snapshot(&session);
    let usage = snapshot
        .values
        .get("tokenUsage")
        .cloned()
        .expect("tokenUsage");
    assert_eq!(
        usage.get("uncachedInputTokens").and_then(|v| v.as_u64()),
        Some(10)
    );
    assert_eq!(usage.get("outputTokens").and_then(|v| v.as_u64()), Some(4));
    assert_eq!(
        usage.get("cacheReadTokens").and_then(|v| v.as_u64()),
        Some(0)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn context_breakdown_projection_prices_envelope_and_surface() {
    let (ctx, store, registry, _meter) = harness().await;
    registry
        .register(
            &ctx,
            dsh_token_meter::context_breakdown_projection_definition(),
        )
        .unwrap();
    let session = store
        .create(
            &ctx,
            Some(session_id("breakdown")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .unwrap();
    append(&session, "turn/start", serde_json::json!({"turn": 1}));
    append(&session, "request/header", header_data("mock"));
    append(
        &session,
        "step/start",
        serde_json::json!({"turn": 1, "step": 1}),
    );
    append(&session, "user/message", user_message("u1", "hello"));
    append(
        &session,
        "step/end",
        serde_json::json!({"turn": 1, "step": 1}),
    );
    append(
        &session,
        "turn/end",
        serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
    );

    let snapshot = registry.snapshot(&session);
    let breakdown = snapshot
        .values
        .get("contextBreakdown")
        .cloned()
        .expect("contextBreakdown");
    assert!(
        breakdown
            .get("systemTokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            > 0
    );
    assert!(
        breakdown
            .get("toolsTokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            > 0
    );
    assert!(
        breakdown
            .get("messageTokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            > 0
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn context_pressure_projection_publishes_projected_tokens() {
    let (ctx, store, registry, _meter) = harness().await;
    registry
        .register(
            &ctx,
            dsh_token_meter::context_pressure_projection_definition(),
        )
        .unwrap();
    let session = store
        .create(
            &ctx,
            Some(session_id("pressure")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .unwrap();
    append(&session, "turn/start", serde_json::json!({"turn": 1}));
    append(
        &session,
        "request/context",
        serde_json::json!({"contextWindow": 128000}),
    );
    append(&session, "request/header", header_data("mock"));
    append(
        &session,
        "step/start",
        serde_json::json!({"turn": 1, "step": 1}),
    );
    append(&session, "user/message", user_message("u1", "hello"));
    append(
        &session,
        "assistant/message",
        serde_json::json!({
            "turn": 1, "step": 1,
            "message": assistant_message("a1", "hi"),
            "usage": {"inputTokens": 120, "outputTokens": 10},
        }),
    );
    append(
        &session,
        "step/end",
        serde_json::json!({"turn": 1, "step": 1}),
    );

    let snapshot = registry.snapshot(&session);
    let pressure = snapshot
        .values
        .get("contextPressure")
        .cloned()
        .expect("contextPressure");
    assert_eq!(
        pressure.get("contextWindow").and_then(|v| v.as_u64()),
        Some(128000)
    );
    assert_eq!(
        pressure.get("pressureTokens").and_then(|v| v.as_u64()),
        Some(120)
    );
    assert!(pressure.get("projectedTokens").is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn shadow_price_replacement_folds_the_logged_delta() {
    let (ctx, store, registry, _meter) = harness().await;
    registry
        .register(
            &ctx,
            dsh_token_meter::context_breakdown_projection_definition(),
        )
        .unwrap();
    let session = store
        .create(
            &ctx,
            Some(session_id("shadow")),
            Some(dsh_session::CreateSessionOptions::default()),
        )
        .await
        .unwrap();
    append(&session, "turn/start", serde_json::json!({"turn": 1}));
    append(&session, "request/header", header_data("mock"));
    append(
        &session,
        "step/start",
        serde_json::json!({"turn": 1, "step": 1}),
    );
    append(
        &session,
        "user/message",
        user_message("u1", "before compaction this is long"),
    );
    append(
        &session,
        "step/end",
        serde_json::json!({"turn": 1, "step": 1}),
    );
    append(
        &session,
        "turn/end",
        serde_json::json!({"turn": 1, "reason": {"kind": "completed"}}),
    );

    let before = registry.snapshot(&session);
    let before_tokens = before
        .values
        .get("contextBreakdown")
        .and_then(|v| v.get("messageTokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    // Shadow-price + replacement: the metering event names the replaced
    // range and its heuristic price; the replace consumes it.
    append(&session, "turn/start", serde_json::json!({"turn": 2}));
    append(&session, "request/header", header_data("mock"));
    append(
        &session,
        "step/start",
        serde_json::json!({"turn": 2, "step": 1}),
    );
    append(
        &session,
        "compaction/summary",
        serde_json::json!({
            "shadowedRange": {"start": 3, "end": 3},
            "shadowedTokenCount": before_tokens,
        }),
    );
    session
        .append(
            "user/message",
            serde_json::json!({
                "id": "summary", "role": "user",
                "content": [{"type": "text", "text": "summary"}],
                "source": {"kind": "plugin", "plugin": "compaction"},
            }),
            Some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Replace { start: 3, end: 3 },
                source_event_seqs: Some(vec![3]),
            }),
        )
        .expect("replace append");

    let after = registry.snapshot(&session);
    let after_tokens = after
        .values
        .get("contextBreakdown")
        .and_then(|v| v.get("messageTokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    // The summary node price minus the shadowed range price is applied.
    assert!(
        after_tokens < before_tokens,
        "compaction shrank the message figure"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn estimate_matches_the_shared_pricing_vocabulary() {
    let message = dsh_llm::create_user_message(
        vec![dsh_llm::ContentBlock::Text {
            text: "12345678".to_string(),
        }],
        dsh_llm::MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    );
    // ceil(8/4)=2 + block overhead 4 + role overhead 4 = 10.
    assert_eq!(dsh_token_meter::estimate_message(&message), 10);
    // An empty tool list prices nothing.
    let header = dsh_session::EpochHeader {
        config: dsh_llm::LlmCallConfig {
            provider: "mock".to_string(),
            model: "mock".to_string(),
            reasoning_effort: None,
            temperature: None,
            max_tokens: None,
            stop: None,
        },
        adapter_defaults: None,
        system: Some("hello".to_string()),
        tools: Some(vec![]),
    };
    assert_eq!(dsh_token_meter::estimate_tools_tokens(Some(&header)), 0);
    assert_eq!(
        dsh_token_meter::estimate_system_tokens(Some(&header)),
        2 + 4
    );
}
