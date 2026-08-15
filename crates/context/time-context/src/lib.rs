//! Opt-in request clock context. Eligible steps add durable,
//! source-attributed time readings to the request history.
//! Rust port of `packages/context/time-context/src/index.ts`.
//!
//! # Deviations
//!
//! - The TS listener consults `signal.aborted` from the pre-step payload; the
//!   Rust [`dsh_agent::AgentPreStepPayload`] carries no signal yet, so the
//!   already-aborted short-circuit is skipped (the loop cancels the step
//!   itself).
//! - `Date.now()` is `chrono::Utc::now().timestamp_millis()`.
//! - IANA canonicalization follows the tz database (chrono-tz) with the CLDR
//!   `Etc/UTC`-family alias collapsed to `UTC`, matching ICU
//!   `resolvedOptions()` for every IANA-shaped input.

pub mod invariant;
pub mod request_zone;
pub mod timestamp;
mod tz_links;

use std::collections::HashMap;
use std::sync::Arc;

use cordis::{
    ArcValue, Context, Disposer, EventOptions, Listener, Plugin, PluginError, arc, downcast_arc,
};
use dsh_agent::{Agent, AgentPreStepPayload, PreStepDecision};
use dsh_llm::{
    ContentBlock, ContextForm, ContextSnapshotSection, MessageSource, UserMessage,
    create_user_message,
};
use dsh_schemastery::Schema;
use dsh_session::SessionEvent;
use indexmap::IndexMap;
use parking_lot::Mutex;

use crate::request_zone::{
    BrowserTimeZoneContext, derive_browser_time_zone_context, render_browser_time_zone_context,
};
use crate::timestamp::{TimestampFormatter, format_timestamp};

/// Cordis plugin name used by loader diagnostics.
pub const NAME: &str = "time-context";

/// The agent registry that owns pre-step processing.
pub const INJECT: [&str; 1] = ["agents"];

/// Request-preparation clock formatting and append scheduling. Invalid values
/// fail plugin load (TS `Config`).
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Fallback display zone when the open turn has no unique browser zone.
    /// Omit to use the process zone.
    pub time_zone: Option<String>,
    /// Minimum milliseconds between durable injections in one session. Omit
    /// or set to 0 to inject at every eligible step.
    pub refresh_interval_ms: Option<f64>,
}

/// Schemastery validation for [`Config`] (the TS `Config` schema export; both
/// fields are required, exactly like `z.object({ ... })`).
pub fn config_schema() -> Schema {
    Schema::object(IndexMap::from([
        ("timeZone".to_string(), Schema::string()),
        ("refreshIntervalMs".to_string(), Schema::number()),
    ]))
}

/// Format a non-negative elapsed millisecond count as compact whole-second
/// units (TS `formatDuration`).
pub fn format_duration(elapsed_ms: f64) -> String {
    let mut seconds = (elapsed_ms.max(0.0) / 1000.0).floor() as i64;
    let days = seconds / 86_400;
    seconds %= 86_400;
    let hours = seconds / 3600;
    seconds %= 3600;
    let minutes = seconds / 60;
    seconds %= 60;
    let mut parts: Vec<String> = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    parts.push(format!("{seconds}s"));
    parts.join(" ")
}

/// Whether one session event carries a message attributed to this plugin.
fn is_plugin_message(event: &SessionEvent, plugin: &str) -> bool {
    let Some(source) = event.data.get("source") else {
        return false;
    };
    source.get("kind").and_then(|kind| kind.as_str()) == Some("plugin")
        && source.get("plugin").and_then(|name| name.as_str()) == Some(plugin)
}

/// Find the latest model-visible event, excluding this plugin's pending
/// append (TS `precedingMessageTime`).
pub fn preceding_message_time(agent: &dyn Agent) -> Option<i64> {
    for event in agent.session().events().iter().rev() {
        match event.type_.as_str() {
            "user/message" | "assistant/message" | "tool/result" => return Some(event.time),
            _ => {}
        }
    }
    None
}

/// Find the preceding time-context event within the open turn (TS
/// `precedingStepContextTime`).
pub fn preceding_step_context_time(agent: &dyn Agent, turn: u64) -> Option<i64> {
    for event in agent.session().events().iter().rev() {
        if event.type_ == "turn/start"
            && event.data.get("turn").and_then(|value| value.as_u64()) == Some(turn)
        {
            return None;
        }
        if event.type_ == "user/message" && is_plugin_message(event, NAME) {
            return Some(event.time);
        }
    }
    None
}

/// Find this plugin's latest durable injection, including a shadowed surface
/// event (TS `latestInjectionTime`).
pub fn latest_injection_time(agent: &dyn Agent) -> Option<i64> {
    agent
        .session()
        .events()
        .iter()
        .rev()
        .find(|event| event.type_ == "user/message" && is_plugin_message(event, NAME))
        .map(|event| event.time)
}

/// Collect already-entered and proposed user messages belonging to one open
/// turn (TS `requestMessages`).
pub fn request_messages(
    agent: &dyn Agent,
    turn: u64,
    proposed: Vec<UserMessage>,
) -> Vec<UserMessage> {
    let events = agent.session().events();
    let start = events.iter().rposition(|event| {
        event.type_ == "turn/start"
            && event.data.get("turn").and_then(|value| value.as_u64()) == Some(turn)
    });
    let mut entered: Vec<UserMessage> = events
        .iter()
        .skip(start.map_or(0, |index| index + 1))
        .filter(|event| event.type_ == "user/message")
        .filter_map(|event| serde_json::from_value::<UserMessage>(event.data.clone()).ok())
        .collect();
    entered.extend(proposed);
    entered
}

/// Assemble one durable reading (TS `renderText`).
pub fn render_text(
    now: i64,
    turn: u64,
    step: u64,
    previous: Option<i64>,
    formatter: &TimestampFormatter,
    time_zone: &str,
    browser_context: &BrowserTimeZoneContext,
) -> String {
    let elapsed = match previous {
        Some(previous) => format_duration((now - previous) as f64),
        None => "unavailable".to_string(),
    };
    let baseline = if step == 1 {
        "model-visible message"
    } else {
        "step context"
    };
    let browser_text = render_browser_time_zone_context(browser_context);
    format!(
        "Time sampled while preparing turn {turn}, step {step}: {}\n{browser_text}\nElapsed since the preceding {baseline}: {elapsed}.",
        format_timestamp(now, formatter, time_zone),
    )
}

/// Reject refresh intervals that cannot represent an exact elapsed-
/// millisecond threshold (TS `validateRefreshInterval`).
pub fn validate_refresh_interval(refresh_interval_ms: Option<f64>) -> Result<(), String> {
    const MAX_SAFE: f64 = 9_007_199_254_740_991.0;
    if let Some(value) = refresh_interval_ms {
        let safe_integer = value.is_finite()
            && value.fract() == 0.0
            && value >= -MAX_SAFE
            && value <= MAX_SAFE;
        if !safe_integer || value < 0.0 {
            return Err(format!(
                "time-context: refreshIntervalMs must be a non-negative safe integer, got {}",
                js_number_string(value)
            ));
        }
    }
    Ok(())
}

/// The TS `String(number)` rendering for diagnostics.
fn js_number_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value == f64::INFINITY {
        return "Infinity".to_string();
    }
    if value == f64::NEG_INFINITY {
        return "-Infinity".to_string();
    }
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

/// Register a prepended pre-step listener for the lifetime of `ctx` (TS
/// `apply`). The returned disposer installs the listener when it runs.
///
/// Fails plugin load when the refresh interval is invalid or the configured
/// or process time zone cannot be resolved.
pub fn apply(ctx: &Context, config: &Config) -> Result<Disposer, String> {
    let refresh_interval_ms = config.refresh_interval_ms;
    validate_refresh_interval(refresh_interval_ms)?;
    let fallback_formatter = match TimestampFormatter::create(config.time_zone.as_deref()) {
        Ok(formatter) => formatter,
        Err(error) => {
            return Err(match &config.time_zone {
                Some(time_zone) => format!(
                    "time-context: invalid IANA timeZone {}",
                    serde_json::to_string(time_zone).unwrap_or_else(|_| time_zone.clone())
                ),
                None => format!("time-context: {}", error.message()),
            });
        }
    };
    let fallback_time_zone = fallback_formatter.time_zone().to_string();
    let formatters: Arc<Mutex<HashMap<String, TimestampFormatter>>> =
        Arc::new(Mutex::new(HashMap::from([(
            fallback_time_zone.clone(),
            fallback_formatter,
        )])));

    let ctx_for_listener = ctx.clone();
    let listener: Arc<Listener> = Arc::new(move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
        let formatters = Arc::clone(&formatters);
        let fallback_time_zone = fallback_time_zone.clone();
        Box::pin(async move {
            let payload = args
                .first()
                .and_then(|value| value.downcast_ref::<AgentPreStepPayload>())
                .cloned()
                .expect("agent/pre-step payload");
            let next = downcast_arc::<cordis::NextFn>(args.last().expect("agent/pre-step next"))
                .expect("agent/pre-step next");
            let decision_value = next.call().await;
            let decision = downcast_arc::<PreStepDecision>(&decision_value)
                .expect("agent/pre-step decision")
                .as_ref()
                .clone();
            if matches!(decision, PreStepDecision::Reject) {
                return Some(decision_value);
            }
            // The TS listener also returns early when `signal.aborted`; the
            // Rust payload carries no signal yet (documented deviation).
            let agent = payload.agent;
            let (turn, step) = (payload.turn, payload.step);
            let now = chrono::Utc::now().timestamp_millis();
            if let Some(interval) = refresh_interval_ms {
                if interval > 0.0 {
                    if let Some(last) = latest_injection_time(agent.as_ref()) {
                        if now >= last && ((now - last) as f64) < interval {
                            return Some(decision_value);
                        }
                    }
                }
            }
            let PreStepDecision::Enter { messages } = decision else {
                unreachable!("reject returned above");
            };
            let previous = if step == 1 {
                preceding_message_time(agent.as_ref())
            } else {
                preceding_step_context_time(agent.as_ref(), turn)
            };
            // Entered plus proposed user messages drive browser-zone
            // derivation only; the returned decision appends the reading to
            // the downstream decision messages (TS `[...decision.messages, ...]`).
            let collected = request_messages(agent.as_ref(), turn, messages.clone());
            let browser = match derive_browser_time_zone_context(&collected) {
                Ok(context) => context,
                Err(error) => panic!("{}", error.message()),
            };
            let selected_time_zone = match &browser {
                BrowserTimeZoneContext::Resolved { time_zone } => time_zone.clone(),
                _ => fallback_time_zone.clone(),
            };
            let formatter = {
                let existing = { formatters.lock().get(&selected_time_zone).cloned() };
                match existing {
                    Some(formatter) => formatter,
                    None => {
                        let created = TimestampFormatter::create(Some(&selected_time_zone))
                            .expect("request zones are validated before formatter resolution");
                        formatters
                            .lock()
                            .insert(selected_time_zone.clone(), created.clone());
                        created
                    }
                }
            };
            let text = render_text(
                now,
                turn,
                step,
                previous,
                &formatter,
                &selected_time_zone,
                &browser,
            );
            let mut merged = messages;
            merged.push(create_user_message(
                vec![ContentBlock::Text {
                    text: text.clone(),
                }],
                MessageSource::Plugin {
                    plugin: NAME.to_string(),
                    form: Some(ContextForm::Snapshot),
                    sections: Some(vec![ContextSnapshotSection {
                        name: NAME.to_string(),
                        text,
                    }]),
                    summary: None,
                    compaction_id: None,
                    source_command_id: None,
                },
            ));
            Some(arc(PreStepDecision::Enter { messages: merged }))
        })
    });

    let disposer_ctx = ctx_for_listener;
    let installed: Arc<std::sync::OnceLock<Disposer>> = Arc::new(std::sync::OnceLock::new());
    Ok(cordis::make_disposer(move || {
        let ctx = disposer_ctx.clone();
        let listener = listener.clone();
        let installed = installed.clone();
        Box::pin(async move {
            // Idempotent: repeated runs keep a single registration. When run
            // inside a plugin fiber, `ctx.on` also attaches the removal
            // disposer to that fiber.
            if installed.get().is_none() {
                let disposer = ctx
                    .on(
                        "agent/pre-step",
                        listener,
                        EventOptions::default().prepend(true),
                    )
                    .await;
                let _ = installed.set(disposer);
            }
        })
    }))
}

/// The Cordis plugin form (TS module exports: `name`, `inject`, `Config`,
/// `apply`).
pub struct TimeContextPlugin;

#[async_trait::async_trait]
impl Plugin for TimeContextPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = config.downcast_ref::<Config>().cloned().unwrap_or_default();
        let disposer = apply(ctx, &config)
            .map_err(|message| PluginError::from(anyhow::anyhow!(message)))?;
        // Registration attaches the removal disposer to this fiber.
        (disposer)().await;
        Ok(())
    }
}
