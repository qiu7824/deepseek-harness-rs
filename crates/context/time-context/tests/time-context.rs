//! Rust port of the core `time-context.spec.ts` behaviors: the durable
//! reading format, the browser request-zone derivation, the event-scanning
//! seams, and the prepended `agent/pre-step` waterfall listener (the
//! agent-loop testkit integration is deferred with the loop harness).
//!
//! Deviations: `Date.now()` cannot be faked, so exact-threshold and
//! backward-wall-clock scenarios assert only the deterministic parts
//! (suppression, clamp-free "0s" same-millisecond elapsed).

use std::sync::Arc;

use cordis::{Context, arc, downcast_arc};
use dsh_agent::{Agent, AgentOptions, AgentStatus, AgentPreStepPayload, CancelOptions, Inbox, InboxTarget, PreStepDecision};
use dsh_llm::{
    ContentBlock, ContextForm, ContextSnapshotSection, MessageSource, UserMessage,
    create_user_message,
};
use dsh_scope::ScopeKey;
use dsh_session::{AgentCancelCause, Session, SessionId, session_id};
use dsh_time_context::{
    Config, INJECT, NAME, TimeContextPlugin, config_schema, format_duration,
    latest_injection_time, preceding_message_time, preceding_step_context_time, render_text,
    request_messages, request_zone::{BrowserTimeZoneContext, derive_browser_time_zone_context, render_browser_time_zone_context},
    timestamp::{TimestampFormatter, format_timestamp, canonical_time_zone},
    validate_refresh_interval,
};

fn text_message(text: &str, source: MessageSource) -> UserMessage {
    create_user_message(
        vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        source,
    )
}

fn rpc_message(zone: &str) -> UserMessage {
    text_message(
        zone,
        MessageSource::User {
            rpc_id: Some("rpc-1".to_string()),
            client_time_zone: Some(zone.to_string()),
        },
    )
}

fn plain_message(text: &str) -> UserMessage {
    text_message(
        text,
        MessageSource::User {
            rpc_id: None,
            client_time_zone: None,
        },
    )
}

struct ProbeAgent {
    id: SessionId,
    session: Session,
}

impl ProbeAgent {
    fn new(id: &str) -> Arc<Self> {
        let id = session_id(id);
        let session = Session::create(id.clone(), None, None).expect("session");
        Arc::new(Self { id, session })
    }
}

impl Agent for ProbeAgent {
    fn id(&self) -> &SessionId {
        &self.id
    }

    fn options(&self) -> &AgentOptions {
        static OPTIONS: std::sync::OnceLock<AgentOptions> = std::sync::OnceLock::new();
        OPTIONS.get_or_init(AgentOptions::default)
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn inbox(&self) -> &Inbox {
        static INBOX: std::sync::OnceLock<Inbox> = std::sync::OnceLock::new();
        INBOX.get_or_init(|| {
            Inbox::new(
                &Session::create(session_id("probe"), None, None).expect("session"),
                Default::default(),
            )
            .expect("inbox")
        })
    }

    fn status(&self) -> AgentStatus {
        AgentStatus::Running
    }

    fn ctx(&self) -> &Context {
        static CTX: std::sync::OnceLock<Context> = std::sync::OnceLock::new();
        CTX.get_or_init(Context::root)
    }

    fn scope_key(&self) -> &ScopeKey {
        static KEY: std::sync::OnceLock<ScopeKey> = std::sync::OnceLock::new();
        KEY.get_or_init(ScopeKey::new)
    }

    fn cancel(&self, _cause: AgentCancelCause, _options: Option<&CancelOptions>) {}

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn send(&self, _message: UserMessage, _target: InboxTarget, _wakeup: bool) {}

    fn followup(&self, _message: UserMessage) {}

    fn steer(&self, _message: UserMessage) {}

    fn inject(&self, _message: UserMessage) {}
}

fn open_turn(session: &Session, turn: u64) {
    session
        .append("turn/start", serde_json::json!({ "turn": turn }), None)
        .expect("turn/start");
}

fn append_message(session: &Session, message: &UserMessage) {
    session
        .append(
            "user/message",
            serde_json::to_value(message).expect("message"),
            Some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect("user/message");
}

fn plugin_proposal() -> UserMessage {
    text_message(
        "request proposal",
        MessageSource::Plugin {
            plugin: "time-context-test".to_string(),
            form: None,
            sections: None,
            summary: None,
            compaction_id: None,
            source_command_id: None,
        },
    )
}

#[test]
fn plugin_metadata_matches_the_ts_exports() {
    assert_eq!(NAME, "time-context");
    assert_eq!(INJECT, ["agents"]);
    let _schema = config_schema();
    let _plugin = TimeContextPlugin;
}

#[test]
fn format_duration_compacts_whole_seconds() {
    assert_eq!(format_duration(0.0), "0s");
    assert_eq!(format_duration(999.0), "0s");
    assert_eq!(format_duration(-5.0), "0s");
    assert_eq!(format_duration(1_000.0), "1s");
    assert_eq!(format_duration(61_000.0), "1m 1s");
    assert_eq!(format_duration(3_600_000.0), "1h 0s");
    assert_eq!(format_duration(86_400_000.0), "1d 0s");
    assert_eq!(format_duration(90_061_000.0), "1d 1h 1m 1s");
}

#[test]
fn refresh_interval_validation_rejects_non_integers_and_negatives() {
    for value in [None, Some(0.0), Some(60_000.0)] {
        validate_refresh_interval(value).expect("valid interval");
    }
    for value in [-1.0, 0.5, 9_007_199_254_740_992.0, f64::INFINITY, f64::NAN] {
        let message = validate_refresh_interval(Some(value)).expect_err("must reject");
        assert!(
            message.contains(
                "time-context: refreshIntervalMs must be a non-negative safe integer"
            ),
            "{message}"
        );
    }
    assert!(validate_refresh_interval(Some(-1.0))
        .unwrap_err()
        .ends_with("got -1"));
    assert!(validate_refresh_interval(Some(0.5))
        .unwrap_err()
        .ends_with("got 0.5"));
    assert!(validate_refresh_interval(Some(f64::INFINITY))
        .unwrap_err()
        .ends_with("got Infinity"));
}

#[test]
fn browser_zone_derivation_classifies_and_renders_every_context() {
    let plugin = text_message(
        "plugin",
        MessageSource::Plugin {
            plugin: "test".to_string(),
            form: None,
            sections: None,
            summary: None,
            compaction_id: None,
            source_command_id: None,
        },
    );
    assert_eq!(
        derive_browser_time_zone_context(&[plugin]).unwrap(),
        BrowserTimeZoneContext::Missing
    );
    assert_eq!(
        derive_browser_time_zone_context(&[
            rpc_message("Asia/Shanghai"),
            rpc_message("Asia/Shanghai"),
        ])
        .unwrap(),
        BrowserTimeZoneContext::Resolved {
            time_zone: "Asia/Shanghai".to_string()
        }
    );
    assert_eq!(
        derive_browser_time_zone_context(&[
            rpc_message("Asia/Shanghai"),
            rpc_message("America/New_York"),
        ])
        .unwrap(),
        BrowserTimeZoneContext::Mixed {
            time_zones: vec![
                "America/New_York".to_string(),
                "Asia/Shanghai".to_string()
            ]
        }
    );

    // Only user-rpc messages contribute; unzoned user messages do not.
    assert_eq!(
        derive_browser_time_zone_context(&[plain_message("hi"), rpc_message("Asia/Shanghai")])
            .unwrap(),
        BrowserTimeZoneContext::Resolved {
            time_zone: "Asia/Shanghai".to_string()
        }
    );

    let outcome = derive_browser_time_zone_context(&[rpc_message("+08:00")]);
    assert!(outcome
        .unwrap_err()
        .message()
        .contains("canonical UTC or IANA Area/Location"));
    let outcome = derive_browser_time_zone_context(&[rpc_message("Not/A_Real_Zone")]);
    assert!(outcome.unwrap_err().message().contains("is unsupported"));
    let outcome = derive_browser_time_zone_context(&[rpc_message("Etc/UTC")]);
    assert!(outcome.unwrap_err().message().contains("must be canonical"));

    assert!(render_browser_time_zone_context(&BrowserTimeZoneContext::Resolved {
        time_zone: "Asia/Shanghai".to_string()
    })
    .contains("Interpret otherwise-unqualified dates and times in this zone."));
    assert_eq!(
        render_browser_time_zone_context(&BrowserTimeZoneContext::Mixed {
            time_zones: vec![
                "America/New_York".to_string(),
                "Asia/Shanghai".to_string()
            ]
        }),
        "Browser time zone for this request: mixed [\"America/New_York\",\"Asia/Shanghai\"]. Ask the user to clarify otherwise-unqualified dates and times."
    );
    assert!(render_browser_time_zone_context(&BrowserTimeZoneContext::Missing).contains("unavailable"));
}

#[test]
fn canonicalization_and_timestamp_formatting_follow_icu() {
    assert_eq!(canonical_time_zone("UTC").unwrap(), "UTC");
    assert_eq!(canonical_time_zone("Asia/Shanghai").unwrap(), "Asia/Shanghai");
    let base = chrono::DateTime::parse_from_rfc3339("2026-07-14T00:00:00+00:00")
        .unwrap()
        .timestamp_millis();
    let shanghai = TimestampFormatter::create(Some("Asia/Shanghai")).unwrap();
    assert_eq!(
        format_timestamp(base + 90_061_000, &shanghai, "Asia/Shanghai"),
        "2026-07-15T09:01:01+08:00[Asia/Shanghai]"
    );
}

#[test]
fn event_scans_find_the_right_preceding_times() {
    let agent = ProbeAgent::new("scans");
    assert_eq!(preceding_message_time(agent.as_ref()), None);
    open_turn(agent.session(), 1);
    let user = append_time(agent.session(), "user/message", plain_message("start"));
    assert_eq!(preceding_message_time(agent.as_ref()), Some(user));
    assert_eq!(preceding_step_context_time(agent.as_ref(), 1), None);
    assert_eq!(latest_injection_time(agent.as_ref()), None);

    let reading = text_message(
        "reading",
        MessageSource::Plugin {
            plugin: "time-context".to_string(),
            form: Some(ContextForm::Snapshot),
            sections: Some(vec![ContextSnapshotSection {
                name: "time-context".to_string(),
                text: "reading".to_string(),
            }]),
            summary: None,
            compaction_id: None,
            source_command_id: None,
        },
    );
    let reading_time = append_time(agent.session(), "user/message", reading);
    assert_eq!(latest_injection_time(agent.as_ref()), Some(reading_time));
    assert_eq!(
        preceding_step_context_time(agent.as_ref(), 1),
        Some(reading_time)
    );
    // A different turn boundary is not the open turn's boundary.
    assert_eq!(
        preceding_step_context_time(agent.as_ref(), 2),
        Some(reading_time)
    );

    let tool = append_time(
        agent.session(),
        "tool/result",
        plain_message("tool result"),
    );
    assert_eq!(preceding_message_time(agent.as_ref()), Some(tool));
}

/// Append one event and return its durable time.
fn append_time(session: &Session, type_: &str, message: UserMessage) -> i64 {
    session
        .append(
            type_,
            serde_json::to_value(message).expect("message"),
            Some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .expect(type_)
        .time
}

#[test]
fn request_message_collection_follows_the_turn_boundary() {
    let agent = ProbeAgent::new("collect");
    open_turn(agent.session(), 1);
    let first = plain_message("first");
    let second = plain_message("second");
    append_message(agent.session(), &first);
    append_message(agent.session(), &second);
    open_turn(agent.session(), 2);
    let third = plain_message("third");
    append_message(agent.session(), &third);

    let proposed = plugin_proposal();
    let collected = request_messages(agent.as_ref(), 1, vec![proposed.clone()]);
    // TS slices from the LAST matching turn/start onward: the later turn's
    // message is still included (only the start boundary matters).
    assert_eq!(collected.len(), 4);
    assert_eq!(collected[0].content, first.content);
    assert_eq!(collected[1].content, second.content);
    assert_eq!(collected[2].content, third.content);
    assert_eq!(collected[3].content, proposed.content);

    let collected = request_messages(agent.as_ref(), 2, vec![]);
    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].content, third.content);

    // No matching turn boundary: the whole history is entered.
    let collected = request_messages(agent.as_ref(), 9, vec![]);
    assert_eq!(collected.len(), 3);
}

#[test]
fn render_text_matches_the_durable_reading_format() {
    let base = chrono::DateTime::parse_from_rfc3339("2026-07-14T00:00:00+00:00")
        .unwrap()
        .timestamp_millis();
    let shanghai = TimestampFormatter::create(Some("Asia/Shanghai")).unwrap();
    let text = render_text(
        base + 90_061_000,
        1,
        1,
        Some(base),
        &shanghai,
        "Asia/Shanghai",
        &BrowserTimeZoneContext::Resolved {
            time_zone: "Asia/Shanghai".to_string(),
        },
    );
    assert_eq!(
        text,
        "Time sampled while preparing turn 1, step 1: 2026-07-15T09:01:01+08:00[Asia/Shanghai]\nBrowser time zone for this request: Asia/Shanghai. Interpret otherwise-unqualified dates and times in this zone.\nElapsed since the preceding model-visible message: 1d 1h 1m 1s."
    );
    let step_two = render_text(
        base,
        3,
        2,
        None,
        &shanghai,
        "Asia/Shanghai",
        &BrowserTimeZoneContext::Missing,
    );
    assert!(step_two.starts_with("Time sampled while preparing turn 3, step 2: "));
    assert!(step_two.contains(
        "Browser time zone for this request: unavailable. Ask the user to clarify otherwise-unqualified dates and times."
    ));
    assert!(step_two.ends_with("Elapsed since the preceding step context: unavailable."));
}

#[tokio::test(flavor = "current_thread")]
async fn prepended_listener_injects_one_snapshot_reading_per_step() {
    let ctx = Context::root();
    let disposer = dsh_time_context::apply(
        &ctx,
        &Config {
            time_zone: Some("UTC".to_string()),
            refresh_interval_ms: None,
        },
    )
    .expect("apply");
    (disposer)().await;
    let agent = ProbeAgent::new("waterfall");
    open_turn(agent.session(), 1);
    append_message(agent.session(), &rpc_message("Asia/Shanghai"));

    let proposed = plugin_proposal();
    let payload = AgentPreStepPayload {
        agent: agent.clone(),
        messages: vec![proposed.clone()],
        turn: 1,
        step: 1,
    };
    let fallback = Box::pin(async move {
        arc(PreStepDecision::Enter {
            messages: vec![proposed],
        })
    });
    let decision = ctx
        .waterfall("agent/pre-step", vec![arc(payload)], fallback)
        .await;
    let PreStepDecision::Enter { messages } =
        downcast_arc::<PreStepDecision>(&decision).expect("decision").as_ref().clone()
    else {
        panic!("enter decision");
    };
    assert_eq!(messages.len(), 2);
    let reading = &messages[1];
    let MessageSource::Plugin {
        plugin,
        form,
        sections,
        summary,
        ..
    } = &reading.source
    else {
        panic!("plugin source");
    };
    assert_eq!(plugin, "time-context");
    assert_eq!(*form, Some(ContextForm::Snapshot));
    assert!(summary.is_none());
    let sections = sections.as_ref().expect("sections");
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0].name, "time-context");
    let [ContentBlock::Text { text }] = reading.content.as_slice() else {
        panic!("single text block");
    };
    assert!(text.starts_with("Time sampled while preparing turn 1, step 1: "), "{text}");
    assert!(text.contains("[Asia/Shanghai]"), "{text}");
    assert!(text.contains(
        "Browser time zone for this request: Asia/Shanghai. Interpret otherwise-unqualified dates and times in this zone."
    ), "{text}");
    assert!(text.contains("Elapsed since the preceding model-visible message: 0s."), "{text}");
    assert_eq!(sections[0].text, *text);

    // The loop appends the proposed reading, then step 2 uses the durable
    // step-context baseline.
    append_message(agent.session(), reading);
    let second_proposal = plugin_proposal();
    let payload = AgentPreStepPayload {
        agent: agent.clone(),
        messages: vec![second_proposal.clone()],
        turn: 1,
        step: 2,
    };
    let fallback = Box::pin(async move {
        arc(PreStepDecision::Enter {
            messages: vec![second_proposal],
        })
    });
    let decision = ctx
        .waterfall("agent/pre-step", vec![arc(payload)], fallback)
        .await;
    let PreStepDecision::Enter { messages } =
        downcast_arc::<PreStepDecision>(&decision).expect("decision").as_ref().clone()
    else {
        panic!("enter decision");
    };
    let reading = &messages[1];
    let [ContentBlock::Text { text }] = reading.content.as_slice() else {
        panic!("single text block");
    };
    assert!(text.starts_with("Time sampled while preparing turn 1, step 2: "), "{text}");
    assert!(text.contains("Elapsed since the preceding step context: 0s."), "{text}");
}

#[tokio::test(flavor = "current_thread")]
async fn plugin_fiber_disposal_removes_the_listener() {
    let ctx = Context::root();
    // The TS plugin declares `inject = ['agents']`.
    let _agents = dsh_agent::AgentRegistry::install(&ctx);
    let fiber = ctx.plugin(
        Arc::new(TimeContextPlugin),
        arc(Config {
            time_zone: Some("UTC".to_string()),
            refresh_interval_ms: None,
        }),
    );
    fiber.settle().await.expect("settle");
    let agent = ProbeAgent::new("dispose");
    open_turn(agent.session(), 1);

    let proposed = plugin_proposal();
    let payload = AgentPreStepPayload {
        agent: agent.clone(),
        messages: vec![proposed.clone()],
        turn: 1,
        step: 1,
    };
    let fallback = Box::pin(async move {
        arc(PreStepDecision::Enter {
            messages: vec![proposed],
        })
    });
    let decision = ctx
        .waterfall("agent/pre-step", vec![arc(payload)], fallback)
        .await;
    let PreStepDecision::Enter { messages } =
        downcast_arc::<PreStepDecision>(&decision).expect("decision").as_ref().clone()
    else {
        panic!("enter decision");
    };
    assert_eq!(messages.len(), 2, "listener active while the fiber runs");

    fiber.dispose().await;
    let last_proposal = plugin_proposal();
    let payload = AgentPreStepPayload {
        agent: agent.clone(),
        messages: vec![],
        turn: 1,
        step: 2,
    };
    let fallback = Box::pin(async move {
        arc(PreStepDecision::Enter {
            messages: vec![last_proposal],
        })
    });
    let decision = ctx
        .waterfall("agent/pre-step", vec![arc(payload)], fallback)
        .await;
    let PreStepDecision::Enter { messages } =
        downcast_arc::<PreStepDecision>(&decision).expect("decision").as_ref().clone()
    else {
        panic!("enter decision");
    };
    assert_eq!(messages.len(), 1, "no reading after disposal");
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_interval_suppresses_a_recent_second_reading() {
    let ctx = Context::root();
    let disposer = dsh_time_context::apply(
        &ctx,
        &Config {
            time_zone: None,
            refresh_interval_ms: Some(3_600_000.0),
        },
    )
    .expect("apply");
    (disposer)().await;
    let agent = ProbeAgent::new("refresh");
    open_turn(agent.session(), 1);

    let proposed = plugin_proposal();
    let payload = AgentPreStepPayload {
        agent: agent.clone(),
        messages: vec![proposed.clone()],
        turn: 1,
        step: 1,
    };
    let fallback = Box::pin(async move {
        arc(PreStepDecision::Enter {
            messages: vec![proposed],
        })
    });
    let decision = ctx
        .waterfall("agent/pre-step", vec![arc(payload)], fallback)
        .await;
    let PreStepDecision::Enter { messages } =
        downcast_arc::<PreStepDecision>(&decision).expect("decision").as_ref().clone()
    else {
        panic!("enter decision");
    };
    assert_eq!(messages.len(), 2, "first step injects");
    append_message(agent.session(), &messages[1]);

    // A second step within the interval keeps the proposal only.
    let proposed = plugin_proposal();
    let payload = AgentPreStepPayload {
        agent: agent.clone(),
        messages: vec![proposed.clone()],
        turn: 1,
        step: 2,
    };
    let fallback = Box::pin(async move {
        arc(PreStepDecision::Enter {
            messages: vec![proposed],
        })
    });
    let decision = ctx
        .waterfall("agent/pre-step", vec![arc(payload)], fallback)
        .await;
    let PreStepDecision::Enter { messages } =
        downcast_arc::<PreStepDecision>(&decision).expect("decision").as_ref().clone()
    else {
        panic!("enter decision");
    };
    assert_eq!(messages.len(), 1, "suppressed by the refresh interval");
    (disposer)().await;
}

#[test]
fn invalid_plugin_configuration_fails_load() {
    let ctx = Context::root();
    let outcome = dsh_time_context::apply(
        &ctx,
        &Config {
            time_zone: Some("Not/A_Real_Zone".to_string()),
            refresh_interval_ms: None,
        },
    );
    assert!(outcome
        .err()
        .expect("must fail")
        .starts_with("time-context: invalid IANA timeZone \"Not/A_Real_Zone\""));
    let outcome = dsh_time_context::apply(
        &ctx,
        &Config {
            time_zone: None,
            refresh_interval_ms: Some(-1.0),
        },
    );
    assert!(outcome
        .err()
        .expect("must fail")
        .contains("refreshIntervalMs must be a non-negative safe integer"));
}
