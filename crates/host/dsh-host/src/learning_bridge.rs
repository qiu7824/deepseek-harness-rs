//! Host event adapters for local, structured experience observations.
use std::collections::HashMap;
use std::sync::Arc;

use cordis::{Context, EventOptions, Listener, NextFn, downcast, downcast_arc};
use dsh_session::{Session, SessionEvent};
use dsh_tool_memory_local::learning::{FailureObservation, LearningStore, workspace_key};

fn provider_failure(
    session: &Session,
    event: &SessionEvent,
    route: Option<(String, String)>,
) -> Option<FailureObservation> {
    if event.type_ != "turn/end" || event.data["reason"]["kind"] != "error" {
        return None;
    }
    let error = &event.data["reason"]["error"];
    let code = error["code"].as_str()?;
    // Unstructured Host exceptions do not establish a provider failure.
    if code.is_empty() || code == "UNKNOWN" {
        return None;
    }
    let cwd = session
        .header()
        .cwd
        .as_deref()
        .filter(|cwd| !cwd.trim().is_empty())?;
    // Preparation can fail before a new durable header is written. Only the
    // route resolved in this turn establishes which provider was attempted.
    let (provider, model) = route?;
    Some(FailureObservation {
        workspace_key: workspace_key(cwd),
        session_id: session.id().to_string(),
        provider,
        model,
        source: "provider".into(),
        code: code.to_string(),
        call_id: format!("turn-end:{}", event.seq),
        ..Default::default()
    })
}

pub fn install(ctx: &Context, memory_scope: &dsh_settings::SettingsScope) -> Result<(), String> {
    let store = ctx
        .get_typed::<Arc<LearningStore>>("learningStore", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or("learning store is unavailable")?;
    fn enabled(value: &dsh_schemastery::Data) -> bool {
        matches!(value, dsh_schemastery::Data::Object(object)
            if matches!(object.get("enabled"), Some(dsh_schemastery::Data::Bool(true))))
    }
    store.set_policy_enabled(enabled(&(memory_scope.get)()));
    let policy_store = store.clone();
    let dispose = (memory_scope.watch)(Arc::new(move |next, _| {
        policy_store.set_policy_enabled(enabled(next));
        Box::pin(async {})
    }));
    let _ = ctx.effect(
        "learning-memory-policy",
        Box::pin(async move { Some(dispose) }),
    );
    let routes = Arc::new(parking_lot::Mutex::new(HashMap::<
        (String, u64),
        (String, String),
    >::new()));
    let request_routes = routes.clone();
    let listener: Arc<Listener> = Arc::new(move |_, args| {
        let payload = args
            .first()
            .and_then(downcast_arc::<dsh_agent::AgentRequestPayload>);
        let next = args.get(1).and_then(downcast_arc::<NextFn>);
        let routes = request_routes.clone();
        Box::pin(async move {
            let Some(next) = next else {
                return None;
            };
            let result = next.call().await;
            if let Some(payload) = payload
                && let Some(config) = downcast::<dsh_llm::LlmCallConfig>(&result)
                && !config.provider.is_empty()
                && !config.model.is_empty()
            {
                let mut routes = routes.lock();
                let key = (payload.agent.id().to_string(), payload.turn);
                if routes.len() >= 256
                    && !routes.contains_key(&key)
                    && let Some(oldest) = routes.keys().next().cloned()
                {
                    routes.remove(&oldest);
                }
                routes.insert(key, (config.provider.clone(), config.model.clone()));
            }
            Some(result)
        })
    });
    futures::executor::block_on(ctx.on(
        "agent/request",
        listener,
        EventOptions::default().prepend(true).global(true),
    ));
    let failures = store.clone();
    let listener: Arc<Listener> = Arc::new(move |_ctx, args| {
        let observation = args
            .first()
            .and_then(downcast::<Session>)
            .and_then(|session| {
                args.get(1)
                    .and_then(downcast::<SessionEvent>)
                    .and_then(|event| {
                        if event.type_ != "turn/end" {
                            return None;
                        }
                        let turn = event.data["turn"].as_u64()?;
                        let route = routes.lock().remove(&(session.id().to_string(), turn));
                        provider_failure(session, event, route)
                    })
            });
        if let Some(observation) = observation {
            let _ = failures.enqueue_failure(observation);
        }
        Box::pin(async { None })
    });
    futures::executor::block_on(ctx.on("session/event", listener, EventOptions::default()));
    let listener: Arc<Listener> = Arc::new(move |_ctx, args| {
        let observation = args
            .first()
            .and_then(downcast::<dsh_message_feedback::NegativeFeedbackRecorded>)
            .and_then(|feedback| {
                Some(FailureObservation {
                    workspace_key: workspace_key(
                        feedback
                            .cwd
                            .as_deref()
                            .filter(|cwd| !cwd.trim().is_empty())?,
                    ),
                    session_id: feedback.session_id.clone(),
                    provider: feedback.provider.clone(),
                    model: feedback.model.clone(),
                    source: "feedback".into(),
                    code: "USER_NEGATIVE_FEEDBACK".into(),
                    call_id: format!("feedback:{}", feedback.message_id),
                    ..Default::default()
                })
            });
        if let Some(observation) = observation {
            let _ = store.enqueue_failure(observation);
        }
        Box::pin(async { None })
    });
    futures::executor::block_on(ctx.on(
        "message-feedback/recorded",
        listener,
        EventOptions::default(),
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_failure_uses_error_boundary_route_and_never_raw_detail() {
        let header = serde_json::from_value(json!({"version":0,"id":"failure","createdAt":1,"cwd":std::env::temp_dir(),"isSeeded":false})).unwrap();
        let session = Session::create(
            dsh_session::session_id("failure"),
            None,
            Some(&header),
            None,
        )
        .unwrap();
        session
            .append(
                "request/context",
                json!({"provider":"old","model":"old-model"}),
                None,
            )
            .unwrap();
        let event = session.append("turn/end", json!({"turn":1,"reason":{"kind":"error","error":{"code":"RATE_LIMIT","message":"Bearer private-token"}}}), None).unwrap();
        session
            .append(
                "request/context",
                json!({"provider":"new","model":"new-model"}),
                None,
            )
            .unwrap();
        assert!(provider_failure(&session, &event, None).is_none());
        let observation = provider_failure(
            &session,
            &event,
            Some(("attempted".into(), "attempted-model".into())),
        )
        .unwrap();
        assert_eq!(observation.provider, "attempted");
        assert_eq!(observation.model, "attempted-model");
        assert_eq!(observation.code, "RATE_LIMIT");
        assert!(observation.message.is_empty());
        let unknown = session.append("turn/end", json!({"turn":2,"reason":{"kind":"error","error":{"code":"UNKNOWN","message":"Host error"}}}), None).unwrap();
        assert!(
            provider_failure(
                &session,
                &unknown,
                Some(("attempted".into(), "attempted-model".into()))
            )
            .is_none()
        );
        let cancelled = session
            .append(
                "turn/end",
                json!({"turn":3,"reason":{"kind":"aborted"}}),
                None,
            )
            .unwrap();
        assert!(
            provider_failure(
                &session,
                &cancelled,
                Some(("attempted".into(), "attempted-model".into()))
            )
            .is_none()
        );
    }
}
