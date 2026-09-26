//! Package-owned durable clock-context invariants. Rust port of
//! `packages/context/time-context/src/invariant.ts`.
//!
//! # Deviations
//!
//! - The TS `fail()` throws synchronously inside the `internal/dispatch`
//!   pre-hook, aborting publication. The Rust event bus contains per-listener
//!   panics in that pre-hook (established workspace deviation), so a violated
//!   reading is reported but does not abort the append.

use std::sync::Arc;

use cordis::{
    ArcValue, Context, EventOptions, InjectSpec, Listener, Plugin, PluginError, downcast,
    downcast_arc,
};
use dsh_invariants::{InvariantInstaller, InvariantRegistry};
use dsh_llm::{ContentBlock, UserMessage};
use dsh_session::{Session, SessionEvent};

use crate::request_zone::{
    BrowserTimeZoneContext, derive_browser_time_zone_context, render_browser_time_zone_context,
};
use crate::timestamp::{TimestampFormatter, format_timestamp};

/// Full package name reserved with the invariant registry.
pub const PACKAGE_NAME: &str = "@deepseek-ai/dsh-time-context";

/// Cordis companion plugin name (TS `name`).
pub const NAME: &str = "time-context-invariant";

/// Service required before the companion can reserve package ownership.
pub const INJECT: [&str; 1] = ["invariants"];

/// The durable reading source name (TS `SOURCE_NAME`).
const SOURCE_NAME: &str = "time-context";

/// The fixed durable reading shape (TS `READING`).
const READING: &str = "^Time sampled while preparing turn (\\d+), step (\\d+): \
     (\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}(?:Z|[+-]\\d{2}:\\d{2})\\[[^\\]]+\\])\\n\
     (Browser time zone for this request: .+)\\n\
     Elapsed since the preceding (model-visible message|step context): \
     (?:unavailable|(?:(?:\\d+d )?(?:\\d+h )?(?:\\d+m )?\\d+s))\\.$";

/// Whether one event's data carries a message attributed to this package.
fn is_reading_source(data: &serde_json::Value) -> bool {
    let Some(source) = data.get("source") else {
        return false;
    };
    source.get("kind").and_then(|kind| kind.as_str()) == Some(SOURCE_NAME)
        || source.get("kind").and_then(|kind| kind.as_str()) == Some("plugin")
            && source.get("plugin").and_then(|name| name.as_str()) == Some(SOURCE_NAME)
}

/// Derive the open step boundary at which a time-context reading may append
/// (TS `preparationPosition`; failures carry the exact TS messages).
pub fn preparation_position(history: &[SessionEvent]) -> Result<(u64, u64), &'static str> {
    let mut open_turn: Option<u64> = None;
    let mut open_step: Option<u64> = None;
    let mut request_started = false;
    for event in history {
        match event.type_.as_str() {
            "turn/start" => {
                open_turn = event.data.get("turn").and_then(|value| value.as_u64());
                open_step = None;
                request_started = false;
            }
            "step/start" => {
                open_step = event.data.get("step").and_then(|value| value.as_u64());
                request_started = false;
            }
            "request/header" => {
                request_started = true;
            }
            "step/end" => {
                open_step = None;
                request_started = false;
            }
            "turn/end" => {
                open_turn = None;
                open_step = None;
                request_started = false;
            }
            _ => {}
        }
    }
    let turn = open_turn.ok_or("time-context reading must be appended inside an open turn")?;
    let step = open_step.ok_or("time-context reading must follow step/start")?;
    if request_started {
        return Err("time-context reading must precede request/header");
    }
    Ok((turn, step))
}

/// Collect the entered user messages belonging to one open turn (TS
/// `requestMessages`; the history form).
pub fn request_messages(history: &[SessionEvent], turn: u64) -> Vec<UserMessage> {
    let start = history.iter().rposition(|event| {
        event.type_ == "turn/start"
            && event.data.get("turn").and_then(|value| value.as_u64()) == Some(turn)
    });
    history
        .iter()
        .skip(start.map_or(0, |index| index + 1))
        .filter(|event| event.type_ == "user/message")
        .filter_map(|event| serde_json::from_value::<UserMessage>(event.data.clone()).ok())
        .collect()
}

/// Validate one plugin-attributed time reading against its session position
/// and timestamp (TS `validateReading`; failures carry the exact TS
/// messages).
pub fn validate_reading(history: &[SessionEvent], event: &SessionEvent) -> Result<(), String> {
    validate_reading_context(
        preparation_position(history),
        derive_browser_time_zone_context(&request_messages(
            history,
            preparation_position(history)
                .map(|position| position.0)
                .unwrap_or(0),
        ))
        .map_err(|error| error.message()),
        event,
    )
}

fn validate_reading_context(
    position: Result<(u64, u64), &'static str>,
    browser: Result<BrowserTimeZoneContext, String>,
    event: &SessionEvent,
) -> Result<(), String> {
    let message: UserMessage = serde_json::from_value(event.data.clone())
        .map_err(|_| "time-context messages must contain exactly one text block".to_string())?;
    let text = match message.content.as_slice() {
        [ContentBlock::Text { text }] => text.clone(),
        _ => return Err("time-context messages must contain exactly one text block".to_string()),
    };
    let captures = regex::Regex::new(READING)
        .expect("static pattern")
        .captures(&text)
        .ok_or("time-context message does not match the durable reading format")?;
    const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
    let turn = captures[1].parse::<u64>().ok();
    let step = captures[2].parse::<u64>().ok();
    if !turn.is_some_and(|turn| (1..=MAX_SAFE_INTEGER).contains(&turn))
        || !step.is_some_and(|step| (1..=MAX_SAFE_INTEGER).contains(&step))
    {
        return Err("time-context turn and step must be positive safe integers".to_string());
    }
    let (turn, step) = (turn.expect("checked"), step.expect("checked"));
    let (expected_turn, expected_step) = position?;
    if turn != expected_turn || step != expected_step {
        return Err(format!(
            "time-context reading names turn {turn}/step {step}, expected turn {expected_turn}/step {expected_step}"
        ));
    }
    if message.source.plugin_name() != Some(SOURCE_NAME) {
        return Err("time-context source must retain package ownership".to_string());
    }
    let source = serde_json::to_value(&message.source).expect("clock source serializes");
    let legacy = source["kind"] == "plugin";
    let exact_section = source["form"] == "snapshot"
        && source["sections"] == serde_json::json!([{"name":SOURCE_NAME,"text":text}])
        && source.as_object().is_some_and(|fields| {
            fields.keys().all(|key| {
                matches!(key.as_str(), "kind" | "form" | "sections") || legacy && key == "plugin"
            })
        });
    if !exact_section {
        return Err(
            "time-context source must carry only the exact snapshot text, not request authority"
                .to_string(),
        );
    }

    let rendered_browser_context = captures[4].to_string();
    let browser_context = browser?;
    if rendered_browser_context != render_browser_time_zone_context(&browser_context) {
        return Err(
            "time-context browser-zone text does not match current-turn user messages".to_string(),
        );
    }

    let baseline = captures[5].to_string();
    if (step == 1) != (baseline == "model-visible message") {
        return Err(format!(
            "time-context step {step} uses the wrong elapsed-time baseline {}",
            serde_json::to_string(&baseline).expect("baseline")
        ));
    }

    let rendered = captures[3].to_string();
    let rendered_core = regex::Regex::new(r"\[[^\]]+\]$")
        .expect("static pattern")
        .replace(&rendered, "");
    let rendered_time = chrono::DateTime::parse_from_rfc3339(&rendered_core)
        .map(|instant| instant.timestamp_millis())
        .ok();
    if rendered_time.is_none() || event.time < rendered_time.expect("checked") {
        return Err(
            "time-context rendered timestamp must parse and not postdate its durable event"
                .to_string(),
        );
    }
    let rendered_time = rendered_time.expect("checked");

    if let BrowserTimeZoneContext::Resolved { time_zone } = &browser_context {
        let formatter = TimestampFormatter::create(Some(time_zone)).map_err(|error| {
            format!(
                "time-context browser zone cannot format its durable timestamp: {}",
                error.message()
            )
        })?;
        if rendered != format_timestamp(rendered_time, &formatter, time_zone) {
            return Err(
                "time-context rendered timestamp does not match the unique browser zone"
                    .to_string(),
            );
        }
    }
    Ok(())
}

#[derive(Default)]
struct ReadingTrace {
    turn: Option<u64>,
    step: Option<u64>,
    request_started: bool,
    zones: std::collections::BTreeSet<String>,
    zone_error: Option<String>,
}

impl ReadingTrace {
    fn position(&self) -> Result<(u64, u64), &'static str> {
        let turn = self
            .turn
            .ok_or("time-context reading must be appended inside an open turn")?;
        let step = self
            .step
            .ok_or("time-context reading must follow step/start")?;
        if self.request_started {
            return Err("time-context reading must precede request/header");
        }
        Ok((turn, step))
    }
    fn browser(&self) -> Result<BrowserTimeZoneContext, String> {
        if let Some(error) = &self.zone_error {
            return Err(error.clone());
        }
        let zones: Vec<_> = self.zones.iter().cloned().collect();
        Ok(match zones.as_slice() {
            [] => BrowserTimeZoneContext::Missing,
            [zone] => BrowserTimeZoneContext::Resolved {
                time_zone: zone.clone(),
            },
            _ => BrowserTimeZoneContext::Mixed { time_zones: zones },
        })
    }
    fn observe(&mut self, event: &SessionEvent) {
        match event.type_.as_str() {
            "turn/start" => {
                self.turn = event.data["turn"].as_u64();
                self.step = None;
                self.request_started = false;
                self.zones.clear();
                self.zone_error = None;
            }
            "step/start" => {
                self.step = event.data["step"].as_u64();
                self.request_started = false;
            }
            "step/end" => {
                self.step = None;
                self.request_started = false;
            }
            "turn/end" => {
                self.turn = None;
                self.step = None;
                self.request_started = false;
            }
            "request/header" => self.request_started = true,
            "user/message" => {
                if let Ok(message) = serde_json::from_value::<UserMessage>(event.data.clone()) {
                    match derive_browser_time_zone_context(&[message]) {
                        Ok(BrowserTimeZoneContext::Resolved { time_zone }) => {
                            self.zones.insert(time_zone);
                        }
                        Err(error) if self.zone_error.is_none() => {
                            self.zone_error = Some(error.message())
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    fn validate(&self, event: &SessionEvent) -> Result<(), String> {
        if event.type_ == "user/message" && is_reading_source(&event.data) {
            validate_reading_context(self.position(), self.browser(), event)?;
        }
        Ok(())
    }
}

/// Validate all package-owned readings already present in one session (TS
/// `validateSession`).
pub fn validate_session(session: &Session) -> Result<(), String> {
    let mut trace = ReadingTrace::default();
    session.visit_events(0, None, |event| {
        trace.validate(event)?;
        trace.observe(event);
        Ok(true)
    })
}

/// Build the installer registered under [`PACKAGE_NAME`] (TS `install`).
pub fn installer() -> InvariantInstaller {
    InvariantInstaller {
        inject: Some(InjectSpec::new(["sessions"])),
        install: Arc::new(|ctx: &Context, fail: Arc<dyn Fn(&str) + Send + Sync>| {
            let ctx = ctx.clone();
            Box::pin(async move {
                // The `internal/dispatch` pre-hook runs INLINE while
                // `Session::append` holds the session state lock, so this
                // companion keeps its own per-session event history instead
                // of re-reading `session.events()` inside the pre-hook
                // (mirrors the session-invariant companion design).
                let histories: Arc<
                    parking_lot::Mutex<std::collections::HashMap<String, ReadingTrace>>,
                > = Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));

                let seed = |session: &Session,
                            histories: &parking_lot::Mutex<
                    std::collections::HashMap<String, ReadingTrace>,
                >,
                            fail: &Arc<dyn Fn(&str) + Send + Sync>| {
                    let mut trace = ReadingTrace::default();
                    if let Err(message) = session.visit_events(0, None, |event| {
                        trace.validate(event)?;
                        trace.observe(event);
                        Ok(true)
                    }) {
                        fail(&message);
                        return;
                    }
                    histories
                        .lock()
                        .insert(session.id().as_str().to_string(), trace);
                };

                // Seed every attached session.
                if let Some(store) = ctx
                    .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
                    .map(|slot| slot.as_ref().clone())
                {
                    for session in store.list() {
                        seed(&session, &histories, &fail);
                    }
                }

                // Validate sessions created later.
                let histories_for_created = histories.clone();
                let fail_for_created = fail.clone();
                let created_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
                    let histories = histories_for_created.clone();
                    let fail = fail_for_created.clone();
                    Box::pin(async move {
                        if let Some(session) =
                            args.first().and_then(|value| downcast::<Session>(value))
                        {
                            seed(session, &histories, &fail);
                        }
                        None
                    })
                });
                ctx.on(
                    "session/created",
                    created_listener,
                    EventOptions::default().global(true),
                )
                .await;

                let committed = histories.clone();
                ctx.on(
                    "session/event",
                    Arc::new(move |_, args| {
                        let histories = committed.clone();
                        Box::pin(async move {
                            if let (Some(session), Some(event)) = (
                                args.first().and_then(downcast::<Session>),
                                args.get(1).and_then(downcast::<SessionEvent>),
                            ) {
                                histories
                                    .lock()
                                    .entry(session.id().to_string())
                                    .or_default()
                                    .observe(event);
                            }
                            None
                        })
                    }),
                    EventOptions::default().global(true),
                )
                .await;

                // Validate each package-owned reading before publication.
                // internal/dispatch args: [mode, eventName, eventArgs, ctx].
                let histories_for_dispatch = histories.clone();
                let fail_for_dispatch = fail.clone();
                let dispatch_listener: Arc<Listener> = Arc::new(move |_ctx, args| {
                    let event_name = args
                        .get(1)
                        .and_then(|value| downcast::<String>(value))
                        .cloned()
                        .unwrap_or_default();
                    let event_args = args
                        .get(2)
                        .and_then(|value| downcast_arc::<Vec<ArcValue>>(value));
                    let histories = histories_for_dispatch.clone();
                    let fail = fail_for_dispatch.clone();
                    Box::pin(async move {
                        if event_name != "session/event" {
                            return None;
                        }
                        let Some(event_args) = event_args else {
                            return None;
                        };
                        let session = event_args
                            .first()
                            .and_then(|value| downcast::<Session>(value))
                            .cloned();
                        let event = event_args
                            .get(1)
                            .and_then(|value| downcast::<SessionEvent>(value))
                            .cloned();
                        let (Some(session), Some(event)) = (session, event) else {
                            return None;
                        };
                        if event.type_ != "user/message" || !is_reading_source(&event.data) {
                            return None;
                        }
                        let mut histories = histories.lock();
                        let history = histories
                            .entry(session.id().as_str().to_string())
                            .or_default();
                        if let Err(message) = history.validate(&event) {
                            fail(&message);
                        }
                        None
                    })
                });
                ctx.on(
                    "internal/dispatch",
                    dispatch_listener,
                    EventOptions::default().global(true),
                )
                .await;
            })
        }),
    }
}

/// Register this package's invariant companion (TS `apply`).
pub fn apply(ctx: &Context) -> cordis::Disposer {
    let registry = ctx
        .get_typed::<Arc<InvariantRegistry>>("invariants", false)
        .expect("the time-context invariant companion requires the invariants service");
    registry.register(ctx, PACKAGE_NAME, installer())
}

/// The Cordis plugin form of the companion.
pub struct TimeContextInvariantPlugin;

#[async_trait::async_trait]
impl Plugin for TimeContextInvariantPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        apply(ctx);
        Ok(())
    }
}
