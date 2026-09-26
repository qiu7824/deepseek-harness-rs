//! Opt-in request clock context. Eligible steps add durable,
//! source-attributed time readings to the request history.
//! Rust port of `packages/context/time-context/src/index.ts`.
//!
//! # Deviations
//!
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
use dsh_schemastery::{Data, Schema};
use dsh_session::SessionEvent;
use indexmap::IndexMap;
use parking_lot::Mutex;

use crate::request_zone::{
    BrowserTimeZoneContext, derive_browser_time_zone_context, render_browser_time_zone_context,
};
use crate::timestamp::{TimestampFormatter, format_timestamp};

/// Cordis plugin name used by loader diagnostics.
pub const NAME: &str = "time-context";

/// Default cadence for durable time readings: ten minutes.
pub const DEFAULT_REFRESH_INTERVAL_MS: f64 = 600_000.0;

/// The agent registry that owns pre-step processing.
pub const INJECT: [&str; 1] = ["agents"];

/// Request-preparation clock formatting and append scheduling. Invalid values
/// fail plugin load (TS `Config`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    /// Fallback display zone when the open turn has no unique browser zone.
    /// Omit to use the process zone.
    pub time_zone: Option<String>,
    /// Minimum milliseconds between durable injections in one session. Omit
    /// for ten minutes, or set to 0 to inject at every eligible step.
    pub refresh_interval_ms: Option<f64>,
}

/// Decode the actual Loader/Profile JSON vocabulary without replacing explicit
/// zero or custom intervals with schema defaults. Null and unknown fields are
/// rejected rather than silently changing the configured behavior.
pub fn decode_config(value: &serde_json::Value) -> Result<Config, String> {
    let object = value
        .as_object()
        .ok_or("time-context: configuration must be an object")?;
    for key in object.keys() {
        if !matches!(key.as_str(), "timeZone" | "refreshIntervalMs") {
            return Err(format!("time-context: unknown configuration field {key:?}"));
        }
    }
    let time_zone = object
        .get("timeZone")
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "time-context: timeZone must be a string".to_owned())
        })
        .transpose()?;
    let refresh_interval_ms = object
        .get("refreshIntervalMs")
        .map(|value| {
            value.as_f64().ok_or_else(|| {
                "time-context: refreshIntervalMs must be a non-negative safe integer".to_owned()
            })
        })
        .transpose()?;
    let config = Config {
        time_zone,
        refresh_interval_ms,
    };
    validate_config(&config)?;
    Ok(config)
}

fn validate_config(config: &Config) -> Result<(), String> {
    validate_refresh_interval(config.refresh_interval_ms)?;
    TimestampFormatter::create(config.time_zone.as_deref())
        .map_err(|error| format!("time-context: {}", error.message()))?;
    Ok(())
}

fn plugin_config(config: &ArcValue) -> Result<Config, String> {
    let decoded = if let Some(config) = config.downcast_ref::<Config>() {
        config.clone()
    } else if let Some(value) = config.downcast_ref::<serde_json::Value>() {
        return decode_config(value);
    } else if config.is::<()>() {
        Config::default()
    } else {
        return Err("time-context: unsupported configuration value".into());
    };
    validate_config(&decoded)?;
    Ok(decoded)
}

/// Schemastery validation and defaults for [`Config`].
pub fn config_schema() -> Schema {
    Schema::object(IndexMap::from([
        ("timeZone".to_string(), Schema::string()),
        (
            "refreshIntervalMs".to_string(),
            Schema::number().default(Data::Number(DEFAULT_REFRESH_INTERVAL_MS)),
        ),
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
    source.get("kind").and_then(|kind| kind.as_str()) == Some(plugin)
        || source.get("kind").and_then(|kind| kind.as_str()) == Some("plugin")
            && source.get("plugin").and_then(|name| name.as_str()) == Some(plugin)
}

/// Find the latest model-visible event, excluding this plugin's pending
/// append (TS `precedingMessageTime`).
pub fn preceding_message_time(agent: &dyn Agent) -> Option<i64> {
    agent
        .session()
        .find_event_rev(|event| {
            matches!(
                event.type_.as_str(),
                "developer/message" | "user/message" | "assistant/message" | "tool/result"
            )
        })
        .expect("time-context Session archive must remain readable")
        .map(|event| event.time)
}

/// Find the preceding time-context event within the open turn (TS
/// `precedingStepContextTime`).
pub fn preceding_step_context_time(agent: &dyn Agent, turn: u64) -> Option<i64> {
    agent
        .session()
        .find_event_rev(|event| {
            event.type_ == "turn/start" && event.data["turn"].as_u64() == Some(turn)
                || event.type_ == "user/message" && is_plugin_message(event, NAME)
        })
        .expect("time-context Session archive must remain readable")
        .filter(|event| event.type_ == "user/message")
        .map(|event| event.time)
}

/// Find this plugin's latest durable injection, including a shadowed surface
/// event (TS `latestInjectionTime`).
pub fn latest_injection_time(agent: &dyn Agent) -> Option<i64> {
    agent
        .session()
        .find_event_rev(|event| event.type_ == "user/message" && is_plugin_message(event, NAME))
        .expect("time-context Session archive must remain readable")
        .map(|event| event.time)
}

/// Collect already-entered and proposed user messages belonging to one open
/// turn (TS `requestMessages`).
pub fn request_messages(
    agent: &dyn Agent,
    turn: u64,
    proposed: Vec<UserMessage>,
) -> Vec<UserMessage> {
    let mut entered = agent
        .session()
        .with_event_reader(|reader| -> Result<Vec<UserMessage>, String> {
            let start = reader
                .find_rev(|event| {
                    event.type_ == "turn/start" && event.data["turn"].as_u64() == Some(turn)
                })?
                .map_or(0, |event| event.seq.get() + 1);
            let mut entered = Vec::new();
            reader.visit(start, None, |event| {
                if event.type_ == "user/message" {
                    if let Ok(message) = serde_json::from_value::<UserMessage>(event.data.clone()) {
                        entered.push(message);
                    }
                }
                Ok(true)
            })?;
            Ok(entered)
        })
        .expect("time-context Session archive must remain readable");
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
        let safe_integer =
            value.is_finite() && value.fract() == 0.0 && value >= -MAX_SAFE && value <= MAX_SAFE;
        if !safe_integer || value < 0.0 {
            return Err(format!(
                "time-context: refreshIntervalMs must be a non-negative safe integer, got {}",
                js_number_string(value)
            ));
        }
    }
    Ok(())
}

fn refresh_due(configured: Option<f64>, now: i64, last: Option<i64>) -> bool {
    let interval = configured.unwrap_or(DEFAULT_REFRESH_INTERVAL_MS);
    interval == 0.0
        || last.is_none_or(|last| now < last || now.saturating_sub(last) as f64 >= interval)
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
    let formatters: Arc<Mutex<HashMap<String, TimestampFormatter>>> = Arc::new(Mutex::new(
        HashMap::from([(fallback_time_zone.clone(), fallback_formatter)]),
    ));

    let ctx_for_listener = ctx.clone();
    let active = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let active_for_listener = active.clone();
    let listener: Arc<Listener> = Arc::new(move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
        let formatters = Arc::clone(&formatters);
        let fallback_time_zone = fallback_time_zone.clone();
        let active = active_for_listener.clone();
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
            if matches!(decision, PreStepDecision::Reject)
                || payload.signal.aborted()
                || !active.load(std::sync::atomic::Ordering::Acquire)
            {
                return Some(decision_value);
            }
            let agent = payload.agent;
            let (turn, step) = (payload.turn, payload.step);
            let now = chrono::Utc::now().timestamp_millis();
            if !refresh_due(
                refresh_interval_ms,
                now,
                latest_injection_time(agent.as_ref()),
            ) {
                return Some(decision_value);
            }
            let PreStepDecision::Enter {
                messages,
                starts_request_series,
            } = decision
            else {
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
            if payload.signal.aborted() || !active.load(std::sync::atomic::Ordering::Acquire) {
                return Some(decision_value);
            }
            let mut merged = messages;
            merged.push(create_user_message(
                vec![ContentBlock::Text { text: text.clone() }],
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
            Some(arc(PreStepDecision::Enter {
                messages: merged,
                starts_request_series,
            }))
        })
    });

    let disposer_ctx = ctx_for_listener;
    let installed: Arc<tokio::sync::OnceCell<Disposer>> = Arc::new(tokio::sync::OnceCell::new());
    Ok(cordis::make_disposer(move || {
        let ctx = disposer_ctx.clone();
        let listener = listener.clone();
        let installed = installed.clone();
        let active = active.clone();
        Box::pin(async move {
            // Idempotent: repeated runs keep a single registration. When run
            // inside a plugin fiber, `ctx.on` also attaches the removal
            // disposer to that fiber.
            installed
                .get_or_init(|| async move {
                    let disposer = ctx
                        .on(
                            "agent/pre-step",
                            listener,
                            EventOptions::default().prepend(true),
                        )
                        .await;
                    // Event dispatch snapshots may outlive hook removal. A
                    // generation-local guard also revokes those in-flight
                    // callbacks when this exact plugin load is disposed.
                    let revoke = cordis::make_disposer(move || {
                        active.store(false, std::sync::atomic::Ordering::Release);
                        Box::pin(async {})
                    });
                    let _ = ctx.fiber.disposables.push(revoke);
                    disposer
                })
                .await;
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

    fn validate(&self, config: ArcValue) -> Result<ArcValue, cordis::ValidationError> {
        plugin_config(&config)
            .map(arc)
            .map_err(|message| cordis::ValidationError::new([message]))
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = plugin_config(&config)
            .map_err(|message| PluginError::from(anyhow::anyhow!(message)))?;
        let disposer =
            apply(ctx, &config).map_err(|message| PluginError::from(anyhow::anyhow!(message)))?;
        // Registration attaches the removal disposer to this fiber.
        (disposer)().await;
        Ok(())
    }
}

#[cfg(test)]
mod refresh_tests {
    use super::*;

    #[test]
    fn loader_json_is_strict_and_preserves_explicit_intervals() {
        for interval in [0, 600000, 1234567] {
            assert_eq!(
                decode_config(&serde_json::json!({"timeZone":"UTC","refreshIntervalMs":interval}))
                    .unwrap(),
                Config {
                    time_zone: Some("UTC".into()),
                    refresh_interval_ms: Some(interval as f64)
                }
            );
        }
        assert_eq!(
            decode_config(&serde_json::json!({})).unwrap(),
            Config::default()
        );
        for invalid in [
            serde_json::json!(null),
            serde_json::json!([]),
            serde_json::json!("clock"),
            serde_json::json!({"refreshIntervalMs":null}),
            serde_json::json!({"refreshIntervalMs":"0"}),
            serde_json::json!({"refreshIntervalMs":-1}),
            serde_json::json!({"refreshIntervalMs":1.5}),
            serde_json::json!({"refreshIntervalMs":9007199254740992u64}),
            serde_json::json!({"timeZone":null}),
            serde_json::json!({"timeZone":"invalid/zone"}),
            serde_json::json!({"unexpected":true}),
        ] {
            assert!(decode_config(&invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn omitted_interval_refreshes_at_ten_minutes_and_after_clock_rollback() {
        assert!(refresh_due(None, 1_000, None));
        assert!(!refresh_due(None, 600_999, Some(1_000)));
        assert!(refresh_due(None, 601_000, Some(1_000)));
        assert!(refresh_due(None, 999, Some(1_000)));
    }

    #[test]
    fn explicit_zero_and_custom_intervals_keep_their_cadence() {
        assert!(refresh_due(Some(0.0), 1_000, Some(1_000)));
        assert!(!refresh_due(Some(30_000.0), 30_999, Some(1_000)));
        assert!(refresh_due(Some(30_000.0), 31_000, Some(1_000)));
        for invalid in [f64::NAN, f64::INFINITY, -1.0, 0.5, 9_007_199_254_740_992.0] {
            assert!(validate_refresh_interval(Some(invalid)).is_err());
        }
    }

    #[test]
    fn configuration_default_does_not_replace_explicit_intervals() {
        for (configured, expected) in [
            (None, 600_000.0),
            (Some(0.0), 0.0),
            (Some(30_000.0), 30_000.0),
        ] {
            let input = Data::Object(
                configured
                    .map(|value| ("refreshIntervalMs".into(), Data::Number(value)))
                    .into_iter()
                    .collect(),
            );
            let resolved = Schema::validate(&config_schema(), input)
                .unwrap()
                .to_json()
                .unwrap();
            assert_eq!(resolved["refreshIntervalMs"], serde_json::json!(expected));
        }
    }
}
