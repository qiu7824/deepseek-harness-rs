//! Advisory per-agent repeat-call detector. It enriches post-execute
//! decisions with logged model context without vetoing or rewriting calls.
//! Rust port of
//! `packages/guard/repeat-tool-reminder/src/index.ts`.
//!
//! # Deviations
//!
//! - The TS `WeakMap<Agent, Chain>` keys on agent identity; Rust keys the
//!   chain map on `agent.id()` strings (agent ids are unique per registry,
//!   and the id is the observable identity).
//! - Canonical argument strings follow `serde_json`/ryu number formatting;
//!   integer floats render as integers exactly like `JSON.stringify` (ryu
//!   matches JS for the remaining cases; the input domain is JSON values).

pub mod invariant;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cordis::{ArcValue, Context, Disposer, Listener, Plugin, PluginError, arc, downcast_arc};
use dsh_agent::AgentPreStepPayload;
use dsh_llm::{ContentBlock, ContextForm, MessageSource, UserMessage, create_user_message};
use dsh_tools::{PostToolDecision, ToolExecution};
use parking_lot::Mutex;

/// Cordis plugin name used by loader diagnostics.
pub const NAME: &str = "repeat-tool-reminder";

/// Plugin config; the load-time checks in [`apply`] fail loud (an empty
/// `thresholds` list, a non-integer, a value below 2, or a duplicate throws
/// at plugin load). `include`/`exclude` entries are `*`-wildcard predicates
/// over tool names at call time.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Consecutive-repeat counts that trigger a reminder (default
    /// `[3, 5, 8]`).
    pub thresholds: Option<Vec<f64>>,
    /// Tool-name patterns to track; empty means every tool is tracked.
    pub include: Option<Vec<String>>,
    /// Tool-name patterns transparent to the chain.
    pub exclude: Option<Vec<String>>,
    /// Maximum characters of canonical arguments quoted in the DETAILED
    /// reminder (default 500).
    pub arguments_preview_chars: Option<f64>,
}

/// Schemastery validation for [`Config`] (the TS `Config` schema export).
pub fn config_schema() -> dsh_schemastery::Schema {
    use dsh_schemastery::{Data, Schema};
    use indexmap::IndexMap;
    Schema::object(IndexMap::from([
        (
            "thresholds".to_string(),
            Schema::array(Schema::number()).default(Data::Array(vec![
                Data::Number(3.0),
                Data::Number(5.0),
                Data::Number(8.0),
            ])),
        ),
        (
            "include".to_string(),
            Schema::array(Schema::string()).default(Data::Array(Vec::new())),
        ),
        (
            "exclude".to_string(),
            Schema::array(Schema::string()).default(Data::Array(Vec::new())),
        ),
        (
            "argumentsPreviewChars".to_string(),
            Schema::number().default(Data::Number(500.0)),
        ),
    ]))
}

/// The gentle first-threshold reminder. Keyed to `thresholds[0]`, not a
/// literal count, so a custom first threshold keeps the gentle-then-detailed
/// escalation.
const GENTLE_REMINDER: &str = "You are repeating the exact same tool call with identical arguments. \
     Carefully analyze the previous result before calling again: if the task is \
     not complete, try a different approach or different arguments instead of \
     repeating the call.";

/// The detailed later-threshold reminder naming the tool, the run length,
/// and the canonical arguments.
pub fn detailed_reminder(tool_name: &str, count: i64, canonical_arguments: &str) -> String {
    format!(
        "Repeated tool call detected:\n- tool: {tool_name}\n- consecutive_calls: {count}\n- arguments: {canonical_arguments}\nThe repeated calls are not making progress. Do not call this tool with these exact arguments again. Inspect the latest result and choose a different action, different arguments, or finish the task if enough evidence has been gathered."
    )
}

/// Deep key-sort of a parsed-JSON value so two argument objects that differ
/// only in property order canonicalize identically (TS `sortJsonValue`).
pub fn sort_json_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(sort_json_value).collect())
        }
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(String, serde_json::Value)> = map
                .iter()
                .map(|(key, value)| (key.clone(), sort_json_value(value)))
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut sorted = serde_json::Map::new();
            for (key, value) in entries {
                sorted.insert(key, value);
            }
            serde_json::Value::Object(sorted)
        }
        other => other.clone(),
    }
}

/// The TS `JSON.stringify` rendering for one JSON value (integers render
/// without a fractional part, exactly like JS).
pub fn json_stringify(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                return integer.to_string();
            }
            if let Some(unsigned) = number.as_u64() {
                return unsigned.to_string();
            }
            // JSON.stringify renders integer floats without a fraction
            // (serde_json's `as_i64` only covers its native int storage).
            if let Some(float) = number.as_f64() {
                if float.is_finite()
                    && float.fract() == 0.0
                    && float >= -9_007_199_254_740_991.0
                    && float <= 9_007_199_254_740_991.0
                {
                    return format!("{}", float as i64);
                }
            }
            number.to_string()
        }
        serde_json::Value::String(text) => serde_json::to_string(text).expect("string"),
        serde_json::Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(json_stringify)
                .collect::<Vec<_>>()
                .join(",")
        ),
        serde_json::Value::Object(map) => {
            let entries: Vec<String> = map
                .iter()
                .map(|(key, value)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("key"),
                        json_stringify(value)
                    )
                })
                .collect();
            format!("{{{}}}", entries.join(","))
        }
    }
}

/// Canonical string form of a call's arguments: deep key-sort, then
/// stringify (TS `canonicalize`).
pub fn canonicalize(arguments: &serde_json::Value) -> String {
    json_stringify(&sort_json_value(arguments))
}

/// Compile one `*`-wildcard pattern to an anchored regular expression
/// (every other regex metacharacter is matched literally; TS
/// `wildcardToRegExp`).
pub fn wildcard_to_regexp(pattern: &str) -> regex::Regex {
    let mut source = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => source.push_str(".*"),
            ch if "|\\{}()[]^$+?.".contains(ch) => {
                source.push('\\');
                source.push(ch);
            }
            ch => source.push(ch),
        }
    }
    source.push('$');
    regex::Regex::new(&source).expect("escaped wildcard pattern")
}

/// Head-truncate the canonical arguments for quoting in the detailed
/// reminder, marking how much was omitted (TS `previewArguments`).
pub fn preview_arguments(canonical: &str, cap: usize) -> String {
    let length = canonical.chars().count();
    if length <= cap {
        return canonical.to_string();
    }
    let head: String = canonical.chars().take(cap).collect();
    format!("{head}… (+{} more chars)", length - cap)
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

/// Validate `thresholds` per the fail-loud contract and return them sorted
/// ascending (TS `validateThresholds`).
pub fn validate_thresholds(values: Vec<f64>) -> Result<Vec<i64>, String> {
    if values.is_empty() {
        return Err("repeat-tool-reminder: `thresholds` must not be empty".to_string());
    }
    for value in &values {
        if !value.is_finite() || value.fract() != 0.0 || *value < 2.0 {
            return Err(format!(
                "repeat-tool-reminder: invalid threshold {} — every threshold must be an integer >= 2",
                js_number_string(*value)
            ));
        }
    }
    let mut sorted: Vec<i64> = values.iter().map(|value| *value as i64).collect();
    let unique: HashSet<i64> = sorted.iter().copied().collect();
    if unique.len() != sorted.len() {
        return Err("repeat-tool-reminder: `thresholds` must not contain duplicates".to_string());
    }
    sorted.sort();
    Ok(sorted)
}

/// Prepend the guard's reminder while preserving every downstream context's
/// source and metadata (TS `prependContext`).
fn prepend_context(ours: UserMessage, theirs: Option<Vec<UserMessage>>) -> Vec<UserMessage> {
    let mut merged = vec![ours];
    merged.extend(theirs.unwrap_or_default());
    merged
}

/// One agent's consecutive-repeat chain: the last tracked call's identity
/// key and its run length.
#[derive(Debug, Clone)]
struct Chain {
    key: String,
    count: i64,
}

/// Shared observation state owned by the plugin's listeners.
struct ObserveState {
    chains: Mutex<HashMap<String, Chain>>,
    first_threshold: i64,
    threshold_set: HashSet<i64>,
    include: Vec<regex::Regex>,
    exclude: Vec<regex::Regex>,
    arguments_preview_chars: usize,
}

/// Whether a tool participates in the chain (untracked calls are
/// transparent: they neither count nor reset).
fn tracked(state: &ObserveState, tool_name: &str) -> bool {
    if !state.include.is_empty()
        && !state
            .include
            .iter()
            .any(|pattern| pattern.is_match(tool_name))
    {
        return false;
    }
    !state
        .exclude
        .iter()
        .any(|pattern| pattern.is_match(tool_name))
}

/// Advance the calling agent's chain for one attempt and return the reminder
/// to deliver, if this attempt's run length hits a configured threshold.
fn observe(state: &ObserveState, exec: &ToolExecution) -> Option<UserMessage> {
    // A direct `ctx.tools.execute()` caller has no model to remind and no id
    // to key on; only agent-loop calls participate.
    let agent = exec.agent.as_ref()?;
    if !tracked(state, &exec.name) {
        return None;
    }
    let canonical = canonicalize(&exec.arguments);
    let key = serde_json::to_string(&[exec.name.as_str(), canonical.as_str()]).expect("key");
    let agent_key = agent.id().as_str().to_string();
    let count = {
        let mut chains = state.chains.lock();
        let next = match chains.get(&agent_key) {
            Some(chain) if chain.key == key => chain.count + 1,
            _ => 1,
        };
        chains.insert(agent_key, Chain { key, count: next });
        next
    };
    if !state.threshold_set.contains(&count) {
        return None;
    }
    let text = if count == state.first_threshold {
        GENTLE_REMINDER.to_string()
    } else {
        detailed_reminder(
            &exec.name,
            count,
            &preview_arguments(&canonical, state.arguments_preview_chars),
        )
    };
    Some(create_user_message(
        vec![ContentBlock::Text { text }],
        MessageSource::Plugin {
            plugin: NAME.to_string(),
            form: Some(ContextForm::Notice),
            sections: None,
            summary: Some(format!("{} × {}", exec.name, count)),
            compaction_id: None,
            source_command_id: None,
        },
    ))
}

/// Install the guard's listeners (TS `apply`). The returned disposer
/// registers both listeners on its first run.
pub fn apply(ctx: &Context, config: &Config) -> Result<Disposer, String> {
    let thresholds = validate_thresholds(
        config
            .thresholds
            .clone()
            .unwrap_or_else(|| vec![3.0, 5.0, 8.0]),
    )?;
    let threshold_set: HashSet<i64> = thresholds.iter().copied().collect();
    let first_threshold = thresholds[0];
    let include: Vec<regex::Regex> = config
        .include
        .clone()
        .unwrap_or_default()
        .iter()
        .map(|pattern| wildcard_to_regexp(pattern))
        .collect();
    let exclude: Vec<regex::Regex> = config
        .exclude
        .clone()
        .unwrap_or_default()
        .iter()
        .map(|pattern| wildcard_to_regexp(pattern))
        .collect();
    let arguments_preview_chars = config.arguments_preview_chars.unwrap_or(500.0);
    if !arguments_preview_chars.is_finite()
        || arguments_preview_chars.fract() != 0.0
        || arguments_preview_chars < 1.0
    {
        return Err(format!(
            "repeat-tool-reminder: invalid argumentsPreviewChars {} — must be an integer >= 1",
            js_number_string(arguments_preview_chars)
        ));
    }
    let state = Arc::new(ObserveState {
        chains: Mutex::new(HashMap::new()),
        first_threshold,
        threshold_set,
        include,
        exclude,
        arguments_preview_chars: arguments_preview_chars as usize,
    });

    // Observe-and-enrich, never veto: count first (state advances regardless
    // of the downstream outcome), DELEGATE so a later listener can still
    // block or replace, then fold the reminder onto whatever came back.
    let state_for_post = state.clone();
    let post_listener: Arc<Listener> =
        Arc::new(move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
            let state = state_for_post.clone();
            Box::pin(async move {
                let exec = args
                    .first()
                    .and_then(|value| value.downcast_ref::<Arc<ToolExecution>>())
                    .cloned()
                    .expect("tools/post-execute exec");
                let next =
                    downcast_arc::<cordis::NextFn>(args.last().expect("tools/post-execute next"))
                        .expect("tools/post-execute next");
                let reminder = observe(&state, &exec);
                let downstream_value = next.call().await;
                let Some(reminder) = reminder else {
                    return Some(downstream_value);
                };
                let downstream = downcast_arc::<PostToolDecision>(&downstream_value)
                    .expect("tools/post-execute decision")
                    .as_ref()
                    .clone();
                let merged = match downstream {
                    PostToolDecision::Block {
                        feedback,
                        additional_contexts,
                    } => PostToolDecision::Block {
                        feedback,
                        additional_contexts: Some(prepend_context(reminder, additional_contexts)),
                    },
                    PostToolDecision::Accept {
                        content,
                        value,
                        additional_contexts,
                    } => PostToolDecision::Accept {
                        content,
                        value,
                        additional_contexts: Some(prepend_context(reminder, additional_contexts)),
                    },
                };
                Some(arc(merged))
            })
        });

    // A user interjection changes the context; repetition across it is not a
    // loop. Pure reset hook: always delegates.
    let state_for_step = state.clone();
    let pre_step_listener: Arc<Listener> =
        Arc::new(move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
            let state = state_for_step.clone();
            Box::pin(async move {
                let payload = args
                    .first()
                    .and_then(|value| value.downcast_ref::<AgentPreStepPayload>())
                    .cloned()
                    .expect("agent/pre-step payload");
                let next =
                    downcast_arc::<cordis::NextFn>(args.last().expect("agent/pre-step next"))
                        .expect("agent/pre-step next");
                if payload
                    .messages
                    .iter()
                    .any(|message| matches!(message.source, MessageSource::User { .. }))
                {
                    state.chains.lock().remove(payload.agent.id().as_str());
                }
                Some(next.call().await)
            })
        });

    let ctx_for_post = ctx.clone();
    let ctx_for_step = ctx.clone();
    let installed: Arc<std::sync::OnceLock<()>> = Arc::new(std::sync::OnceLock::new());
    Ok(cordis::make_disposer(move || {
        let ctx_for_post = ctx_for_post.clone();
        let ctx_for_step = ctx_for_step.clone();
        let post_listener = post_listener.clone();
        let pre_step_listener = pre_step_listener.clone();
        let installed = installed.clone();
        Box::pin(async move {
            if installed.set(()).is_ok() {
                ctx_for_post
                    .on("tools/post-execute", post_listener, Default::default())
                    .await;
                ctx_for_step
                    .on("agent/pre-step", pre_step_listener, Default::default())
                    .await;
            }
        })
    }))
}

/// The Cordis plugin form (TS module exports: `name`, `Config`, `apply`).
pub struct RepeatToolReminderPlugin;

#[async_trait::async_trait]
impl Plugin for RepeatToolReminderPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = config.downcast_ref::<Config>().cloned().unwrap_or_default();
        let disposer =
            apply(ctx, &config).map_err(|message| PluginError::from(anyhow::anyhow!(message)))?;
        (disposer)().await;
        Ok(())
    }
}
