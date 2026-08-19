//! Durable session skill catalog and model-facing `skill` loader tool.
//! Rust port of `packages/skill/tool-skill/src/index.ts`.
//!
//! # Deviations
//!
//! - The pre-step payload carries no signal in the port (dsh-agent
//!   deviation), so the invocation and catalog listeners resolve lookups
//!   with no abort predicate.
//! - The wire schemas use the port's standard JSON Schema form (`required`
//!   arrays) instead of the TS inline `required: true` DSL.
//! - Unreadable catalog records collapse to "not this plugin's catalog"
//!   through the typed [`dsh_llm::MessageSource`] variants (the TS
//!   duck-typed malformed seeds are unrepresentable).

pub mod invariant;

use std::collections::HashSet;
use std::sync::Arc;

use cordis::{
    ArcValue, Context, Disposer, InjectSpec, Listener, NextFn, Plugin, PluginError, arc, downcast,
    downcast_arc,
};
use dsh_agent::{AgentPreStepPayload, PreStepDecision};
use dsh_llm::{
    ContentBlock, ContextForm, MessageSource, SkillCatalogEntry, UserMessage, create_user_message,
};
use dsh_skill::{
    SkillCatalogSnapshot, SkillRegistry, SkillViewOptions, escape_text, is_model_invocable,
    is_skill_name, render_skill_content,
};
use dsh_tools::{
    ToolBodyError, ToolCallKind, ToolCallView, ToolDefinition, ToolOutputDefinition,
    ToolRunContext, ToolRuntime,
};

pub const NAME: &str = "tool-skill";

const DEFAULT_CATALOG_DESCRIPTION_MAX_LENGTH: usize = 500;

/// Model-facing skill catalog configuration.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Maximum normalized description length rendered in the session
    /// catalog; minimum 3.
    pub catalog_description_max_length: Option<usize>,
}

/// Register the model-facing skill loader and its visibility-matched
/// durable session catalog (TS `apply`).
pub async fn apply(ctx: &Context, config: Config) -> Result<Disposer, String> {
    let catalog_description_max_length = config
        .catalog_description_max_length
        .unwrap_or(DEFAULT_CATALOG_DESCRIPTION_MAX_LENGTH);
    if catalog_description_max_length < 3 {
        return Err(format!(
            "tool-skill: catalogDescriptionMaxLength must be an integer greater than or equal to 3"
        ));
    }
    let skills = ctx
        .get_typed::<Arc<SkillRegistry>>("skills", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-skill requires the skills service".to_string())?;
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-skill requires the tools service".to_string())?;

    let skills_for_tool = skills.clone();
    let definition = Arc::new(ToolDefinition {
        name: "skill".to_string(),
        description: "Load the full instructions for an available skill. Call this with the exact skill name from the session skill catalog before acting on a task that names or clearly matches that skill."
            .to_string(),
        parameters: parameters_schema(),
        output: ToolOutputDefinition {
            schema: output_schema(),
            render: Arc::new(|_args, value| {
                let name = value["name"].as_str().unwrap_or_default();
                let provider = value["provider"].as_str().unwrap_or_default();
                let resource_base = value.get("resourceBase");
                let content = value["content"].as_str().unwrap_or_default();
                let resource_base = match resource_base {
                    None | Some(serde_json::Value::Null) => None,
                    Some(base) => Some(match base {
                        serde_json::Value::Object(_) => {
                            let kind = base["kind"].as_str().unwrap_or_default();
                            match kind {
                                "directory" => dsh_skill::SkillResourceBase::Directory {
                                    path: base["path"].as_str().unwrap_or_default().to_string(),
                                },
                                "url" => dsh_skill::SkillResourceBase::Url {
                                    url: base["url"].as_str().unwrap_or_default().to_string(),
                                },
                                _ => dsh_skill::SkillResourceBase::Opaque {
                                    description: base["description"]
                                        .as_str()
                                        .unwrap_or_default()
                                        .to_string(),
                                },
                            }
                        }
                        _ => dsh_skill::SkillResourceBase::Opaque {
                            description: String::new(),
                        },
                    }),
                };
                Ok(vec![ContentBlock::Text {
                    text: render_skill_content(name, provider, resource_base.as_ref(), content),
                }])
            }),
            presentation_meta: None,
        },
        timeout_ms: None,
        is_concurrency_safe: None,
        execute: Arc::new(move |args: &serde_json::Value, exec: &ToolRunContext| {
            let args = args.clone();
            let agent = exec.agent.clone();
            let signal = exec.signal.lock().clone();
            let skills = skills_for_tool.clone();
            Box::pin(async move {
                let name = args["name"].as_str().unwrap_or_default().to_string();
                if !is_skill_name(&name) {
                    return Err(ToolBodyError::plain(format!(
                        "invalid skill name \"{name}\""
                    )));
                }
                // The agent is its own scope key, so the lookup resolves the
                // layered registry exactly as this agent's composition sees
                // it.
                let lookup = SkillViewOptions {
                    cwd: agent
                        .as_ref()
                        .and_then(|agent| agent.session().header().cwd.clone()),
                    signal: Some(signal),
                    scope: agent.as_ref().map(|agent| agent.scope_key().clone()),
                };
                let summary = skills
                    .list(lookup.clone())
                    .await
                    .map_err(ToolBodyError::plain)?
                    .into_iter()
                    .find(|skill| skill.name == name);
                let Some(summary) = summary else {
                    return Err(ToolBodyError::plain(format!(
                        "skill \"{name}\" is unknown or no longer available"
                    )));
                };
                if !is_model_invocable(&summary) {
                    return Err(ToolBodyError::plain(format!(
                        "skill \"{name}\" is not available for model invocation"
                    )));
                }
                let skill = skills
                    .get(&name, lookup)
                    .await
                    .map_err(ToolBodyError::plain)?;
                let Some(skill) = skill else {
                    return Err(ToolBodyError::plain(format!(
                        "skill \"{name}\" is unknown or no longer available"
                    )));
                };
                if !skill.invocation.model_invocable {
                    return Err(ToolBodyError::plain(format!(
                        "skill \"{name}\" is not available for model invocation"
                    )));
                }
                let mut value = serde_json::json!({
                    "name": skill.name,
                    "provider": skill.provider,
                });
                if let Some(base) = &skill.resource_base {
                    value["resourceBase"] = resource_base_json(base);
                }
                value["content"] = serde_json::json!(skill.content);
                Ok(value)
            })
        }),
        finalize_content: None,
        present_call: Some(Arc::new(|args: &serde_json::Value| {
            let name = args["name"].as_str().unwrap_or_default();
            Some(ToolCallView::Generic {
                title: format!("Load skill {name}"),
                kind: Some(ToolCallKind::Read),
                raw_input: Some(serde_json::json!(name)),
                content: None,
                locations: None,
            })
        })),
        present_result: None,
    });
    let tool_disposer = tools
        .register_arc(ctx, definition.clone())
        .map_err(|error| format!("tool-skill: {error}"))?;

    // User-explicit skill invocation. Registered FIRST so the waterfall
    // hands it the catalog-bearing list to extend: injected material must
    // come last, closest to the answer.
    let skills_for_invocation = skills.clone();
    let invocation_listener: Arc<Listener> =
        Arc::new(move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
            let skills = skills_for_invocation.clone();
            Box::pin(async move {
                let payload = args
                    .first()
                    .and_then(|value| downcast::<AgentPreStepPayload>(value))
                    .cloned()
                    .expect("agent/pre-step payload");
                let next = downcast_arc::<NextFn>(args.last().expect("agent/pre-step next"))
                    .expect("agent/pre-step next");
                let decision_value = next.call().await;
                let decision = downcast_arc::<PreStepDecision>(&decision_value)
                    .expect("agent/pre-step decision")
                    .as_ref()
                    .clone();
                if matches!(decision, PreStepDecision::Reject) {
                    return Some(decision_value);
                }
                let names = invoked_skill_names(&payload.messages);
                if names.is_empty() {
                    return Some(decision_value);
                }
                // The payload carries no signal in the port (dsh-agent
                // deviation); lookups run without an abort predicate.
                let lookup = SkillViewOptions {
                    cwd: payload.agent.session().header().cwd.clone(),
                    signal: None,
                    scope: Some(payload.agent.scope_key().clone()),
                };
                let mut injections: Vec<UserMessage> = Vec::new();
                for name in names {
                    let skill = skills
                        .get(&name, lookup.clone())
                        .await
                        .expect("skill lookup without a signal cannot abort");
                    // Unknown names and user-disabled skills stay plain prose.
                    let Some(skill) = skill else {
                        continue;
                    };
                    if !skill.invocation.user_invocable {
                        continue;
                    }
                    injections.push(create_user_message(
                        vec![ContentBlock::Text {
                            text: render_skill_content(
                                &skill.name,
                                &skill.provider,
                                skill.resource_base.as_ref(),
                                &skill.content,
                            ),
                        }],
                        MessageSource::SkillInvocation {
                            name,
                            form: ContextForm::Instructions,
                        },
                    ));
                }
                if injections.is_empty() {
                    return Some(decision_value);
                }
                let PreStepDecision::Enter { messages } = decision else {
                    unreachable!("reject returned above");
                };
                let mut merged = messages;
                merged.extend(injections);
                Some(arc(PreStepDecision::Enter { messages: merged }))
            })
        });
    let invocation_disposer = ctx
        .on(
            "agent/pre-step",
            invocation_listener,
            cordis::EventOptions::default(),
        )
        .await;

    // The catalog listener. Registered after the invocation listener so
    // reverse teardown removes guidance first; exact definition identity
    // prevents a scoped shadow merely named `skill` from inheriting it.
    let skills_for_catalog = skills.clone();
    let tools_for_catalog = tools.clone();
    let definition_for_catalog = definition.clone();
    let catalog_listener: Arc<Listener> =
        Arc::new(move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
            let skills = skills_for_catalog.clone();
            let tools = tools_for_catalog.clone();
            let definition = definition_for_catalog.clone();
            Box::pin(async move {
                let payload = args
                    .first()
                    .and_then(|value| downcast::<AgentPreStepPayload>(value))
                    .cloned()
                    .expect("agent/pre-step payload");
                let next = downcast_arc::<NextFn>(args.last().expect("agent/pre-step next"))
                    .expect("agent/pre-step next");
                let decision_value = next.call().await;
                let decision = downcast_arc::<PreStepDecision>(&decision_value)
                    .expect("agent/pre-step decision")
                    .as_ref()
                    .clone();
                if matches!(decision, PreStepDecision::Reject) {
                    return Some(decision_value);
                }
                let tool_visible = tools
                    .get("skill", Some(payload.agent.scope_key()))
                    .is_some_and(|registered| Arc::ptr_eq(&registered, &definition));
                let lookup = SkillViewOptions {
                    cwd: payload.agent.session().header().cwd.clone(),
                    signal: None,
                    scope: Some(payload.agent.scope_key().clone()),
                };
                let snapshot = if tool_visible {
                    skills.snapshot(lookup).await.expect("catalog snapshot")
                } else {
                    SkillCatalogSnapshot {
                        skills: Vec::new(),
                        complete: true,
                    }
                };
                if !snapshot.complete {
                    return Some(decision_value);
                }
                let skills: Vec<_> = snapshot
                    .skills
                    .into_iter()
                    .filter(|skill| is_model_invocable(skill))
                    .collect();
                let entries: Vec<SkillCatalogEntry> = skills
                    .iter()
                    .map(|skill| SkillCatalogEntry {
                        name: skill.name.clone(),
                        description: catalog_description(
                            &skill.description,
                            catalog_description_max_length,
                        ),
                    })
                    .collect();
                let digest = digest_catalog_entries(&entries);
                let history = catalog_history(&payload.agent);
                let existing = catalog_message(&decision_messages(&decision));
                if history.visible_digest.as_deref() == Some(digest.as_str()) {
                    return Some(match existing {
                        None => decision_value,
                        Some(existing) => {
                            let PreStepDecision::Enter { messages } = decision else {
                                unreachable!("reject returned above");
                            };
                            arc(PreStepDecision::Enter {
                                messages: messages
                                    .into_iter()
                                    .filter(|message| message.id != existing.message.id)
                                    .collect(),
                            })
                        }
                    });
                }
                if let Some(existing) = &existing {
                    if digest_catalog_entries(&existing.entries) == digest {
                        return Some(decision_value);
                    }
                }
                if !history.published && skills.is_empty() {
                    return Some(match existing {
                        None => decision_value,
                        Some(existing) => {
                            let PreStepDecision::Enter { messages } = decision else {
                                unreachable!("reject returned above");
                            };
                            arc(PreStepDecision::Enter {
                                messages: messages
                                    .into_iter()
                                    .filter(|message| message.id != existing.message.id)
                                    .collect(),
                            })
                        }
                    });
                }
                let catalog = if history.published {
                    render_catalog_update(&entries)
                } else {
                    render_catalog_message(&entries)
                };
                let PreStepDecision::Enter { messages } = decision else {
                    unreachable!("reject returned above");
                };
                let merged = match existing {
                    None => {
                        let mut merged = messages;
                        merged.push(catalog);
                        merged
                    }
                    Some(existing) => messages
                        .into_iter()
                        .map(|message| {
                            if message.id == existing.message.id {
                                catalog.clone()
                            } else {
                                message
                            }
                        })
                        .collect(),
                };
                Some(arc(PreStepDecision::Enter { messages: merged }))
            })
        });
    let catalog_disposer = ctx
        .on(
            "agent/pre-step",
            catalog_listener,
            cordis::EventOptions::default(),
        )
        .await;

    Ok(cordis::make_disposer(move || {
        let tool_disposer = tool_disposer.clone();
        let invocation_disposer = invocation_disposer.clone();
        let catalog_disposer = catalog_disposer.clone();
        Box::pin(async move {
            // Reverse teardown: guidance first, then the injection listener,
            // then the tool registration.
            catalog_disposer().await;
            invocation_disposer().await;
            tool_disposer().await;
        })
    }))
}

fn decision_messages(decision: &PreStepDecision) -> Vec<UserMessage> {
    match decision {
        PreStepDecision::Reject => Vec::new(),
        PreStepDecision::Enter { messages } => messages.clone(),
    }
}

fn parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "name": {
                "type": "string",
                "description": "The exact skill name from the available skills list."
            }
        },
        "required": ["name"]
    })
}

fn output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "name": { "type": "string" },
            "provider": { "type": "string" },
            "resourceBase": {
                "oneOf": [
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "type": "string", "const": "directory" },
                            "path": { "type": "string" }
                        },
                        "required": ["kind", "path"]
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "type": "string", "const": "url" },
                            "url": { "type": "string" }
                        },
                        "required": ["kind", "url"]
                    },
                    {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "type": "string", "const": "opaque" },
                            "description": { "type": "string" }
                        },
                        "required": ["kind", "description"]
                    }
                ]
            },
            "content": { "type": "string" }
        },
        "required": ["name", "provider", "content"]
    })
}

fn resource_base_json(base: &dsh_skill::SkillResourceBase) -> serde_json::Value {
    match base {
        dsh_skill::SkillResourceBase::Directory { path } => {
            serde_json::json!({ "kind": "directory", "path": path })
        }
        dsh_skill::SkillResourceBase::Url { url } => {
            serde_json::json!({ "kind": "url", "url": url })
        }
        dsh_skill::SkillResourceBase::Opaque { description } => {
            serde_json::json!({ "kind": "opaque", "description": description })
        }
    }
}

fn render_catalog_message(entries: &[SkillCatalogEntry]) -> UserMessage {
    let lines = [
        "<system-reminder>".to_string(),
        "A skill is a reusable set of task-specific instructions. The following skills are available in this session:".to_string(),
        String::new(),
        "<available_skills>".to_string(),
        render_catalog_entries(entries).join("\n"),
        "</available_skills>".to_string(),
        String::new(),
        "If the user names a skill, or the task clearly matches a skill's description, call the `skill` tool with the exact skill name before taking task actions. Load all applicable skills, then follow their full instructions. This catalog contains summaries only; do not infer or follow a skill's instructions until it has been loaded.".to_string(),
        "A user may also invoke a skill directly; its <skill_content> block then appears in this conversation. Follow it, and do not call the `skill` tool again for that skill.".to_string(),
        "</system-reminder>".to_string(),
    ]
    .join("\n");
    create_user_message(
        vec![ContentBlock::Text { text: lines }],
        MessageSource::SkillCatalog {
            form: ContextForm::Catalog,
            update: None,
            entries: entries.to_vec(),
        },
    )
}

fn render_catalog_update(entries: &[SkillCatalogEntry]) -> UserMessage {
    let availability: [&str; 2] = if entries.is_empty() {
        [
            "No skills are currently available through the `skill` tool. Do not use names from earlier skill catalogs.",
            "A user may still invoke a skill directly; its <skill_content> block then appears in this conversation. Follow it, and do not call the `skill` tool for it.",
        ]
    } else {
        [
            "Use only names in this replacement catalog. If the user names a listed skill, or the task clearly matches its description, call the `skill` tool with the exact name before acting.",
            "A user may also invoke a skill directly; its <skill_content> block then appears in this conversation. Follow it, and do not call the `skill` tool again for that skill.",
        ]
    };
    let body = [
        "<system-reminder>".to_string(),
        "The available skill catalog changed. This complete catalog replaces every earlier available-skills list in this session:".to_string(),
        String::new(),
        "<available_skills>".to_string(),
        render_catalog_entries(entries).join("\n"),
        "</available_skills>".to_string(),
        String::new(),
        availability.join("\n"),
        "</system-reminder>".to_string(),
    ]
    .join("\n");
    create_user_message(
        vec![ContentBlock::Text { text: body }],
        MessageSource::SkillCatalog {
            form: ContextForm::Catalog,
            update: Some(true),
            entries: entries.to_vec(),
        },
    )
}

fn render_catalog_entries(entries: &[SkillCatalogEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| format!("- `{}`: {}", entry.name, escape_text(&entry.description)))
        .collect()
}

fn digest_catalog_entries(entries: &[SkillCatalogEntry]) -> String {
    use sha2::{Digest, Sha256};
    let canonical = entries
        .iter()
        .map(|entry| {
            serde_json::to_string(&(entry.name.as_str(), entry.description.as_str()))
                .expect("entries serialize")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn catalog_history(agent: &Arc<dyn dsh_agent::Agent>) -> CatalogHistory {
    let visible: HashSet<u64> = agent
        .session()
        .surface()
        .map(|surface| surface.nodes.into_iter().collect())
        .unwrap_or_default();
    let events = agent.session().events();
    let mut published = false;
    for event in events.iter().rev() {
        if event.type_ != "user/message" {
            continue;
        }
        // Unreadable durable records are "not this plugin's catalog": the
        // typed enum rejects malformed seeds at deserialization.
        let source: MessageSource = match serde_json::from_value(event.data["source"].clone()) {
            Ok(source) => source,
            Err(_) => continue,
        };
        let MessageSource::SkillCatalog { entries, .. } = source else {
            continue;
        };
        let digest = digest_catalog_entries(&entries);
        published = true;
        if visible.contains(&event.seq) {
            return CatalogHistory {
                visible_digest: Some(digest),
                published,
            };
        }
    }
    CatalogHistory {
        visible_digest: None,
        published,
    }
}

struct CatalogHistory {
    visible_digest: Option<String>,
    published: bool,
}

struct ExistingCatalog {
    message: UserMessage,
    entries: Vec<SkillCatalogEntry>,
}

fn catalog_message(messages: &[UserMessage]) -> Option<ExistingCatalog> {
    for message in messages {
        let MessageSource::SkillCatalog { entries, .. } = &message.source else {
            continue;
        };
        return Some(ExistingCatalog {
            message: message.clone(),
            entries: entries.clone(),
        });
    }
    None
}

/// Normalized, length-bounded description exactly as the catalog publishes
/// it (unescaped).
fn catalog_description(value: &str, max_length: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.len() <= max_length {
        return normalized;
    }
    // Walk back to a UTF-8 char boundary for the `maxLength - 3` prefix.
    let mut boundary = max_length - 3;
    while boundary > 0 && !normalized.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}...", &normalized[..boundary])
}

/// `/name` gesture tokens from the claimed user messages, deduplicated in
/// first-seen order.
fn invoked_skill_names(messages: &[UserMessage]) -> Vec<String> {
    static GESTURE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(^|\s)/([a-z0-9]+(-[a-z0-9]+)*)(\s|$)").expect("static pattern")
    });
    let mut names: Vec<String> = Vec::new();
    for message in messages {
        if !matches!(message.source, MessageSource::User { .. }) {
            continue;
        }
        for block in &message.content {
            let ContentBlock::Text { text } = block else {
                continue;
            };
            for capture in GESTURE.captures_iter(text) {
                let Some(name) = capture.get(2) else {
                    continue;
                };
                let name = name.as_str().to_string();
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
    }
    names
}

/// The Cordis plugin form (TS module exports: `name`, `inject`, `Config`,
/// `apply`).
pub struct ToolSkillPlugin {
    config: Config,
}

impl ToolSkillPlugin {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Plugin for ToolSkillPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["agents", "tools", "skills"])
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        let disposer = apply(ctx, self.config.clone())
            .await
            .map_err(|message| PluginError::from(anyhow::anyhow!(message)))?;
        let _ = ctx.effect("tool-skill", Box::pin(async move { Some(disposer) }));
        Ok(())
    }
}
