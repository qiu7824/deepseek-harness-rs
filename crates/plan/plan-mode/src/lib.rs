//! Plan mode is logged per-agent collaboration state: while active, a
//! deployment-owned guidance section is included in each model request,
//! and `exit_plan_mode` presents the completed plan for user review, while
//! the `/plan off` command lets a user leave directly. Sandbox mode and
//! approval policy enforce restrictions independently and do not read or
//! write plan state.
//! Rust port of `packages/plan/plan-mode/src/index.ts` (+ `types.ts`).
//!
//! # Deviations
//!
//! - The `plan:policy` section provider resolves to the TS no-agent empty
//!   branch (the Rust [`dsh_system_prompt::AssembleContext`] carries no
//!   agent yet).
//! - The Rust user-questions service collapses the TS dismissed/aborted
//!   taxonomy to the `ASK_ABORTED` code, so the exit tool's dismissal catch
//!   matches that code.
//! - The pre-step payload carries no signal (dsh-agent deviation).

pub mod invariant;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cordis::{
    ArcValue, Context, InjectSpec, Listener, NextFn, Plugin, PluginError, arc, downcast,
    downcast_arc,
};
use dsh_agent::{Agent, AgentPreStepPayload, PreStepDecision};
use dsh_commands::{CommandDefinition, CommandInputDescriptor, CommandResult, CommandRuntime};
use dsh_llm::{ContentBlock, ContextForm, MessageSource, create_user_message};
use dsh_session::{Session, SessionEvent, UserMessage};
use dsh_session_projection::{ProjectionDefinition, SessionProjectionRegistry};
use dsh_system_prompt::{PromptSection, PromptText, SystemPrompt};
use dsh_tools::{ToolCallKind, ToolCallView, ToolDefinition, ToolOutputDefinition, ToolRuntime};
use dsh_user_questions::{
    AskUserQuestionIntent, AskUserQuestionItem, AskUserQuestionOption,
    AskUserQuestionRequest, UserQuestionService,
};

pub const NAME: &str = "plan-mode";

/// The model-facing exit tool's name. It stays registered while plan mode
/// is inactive so the request tool catalog is stable across transitions.
pub const EXIT_PLAN_MODE: &str = "exit_plan_mode";

/// The review question's id, echoed in the answer this tool reads.
const REVIEW_ID: &str = "plan-review";

/// The review question's approve option label.
const APPROVE_LABEL: &str = "Approve";

/// The review question's keep-planning option label.
const KEEP_PLANNING_LABEL: &str = "Keep planning";

const EXIT_DESCRIPTION: &str = "Use only in plan mode. Present your plan for the user's review and, on approval, leave plan mode. Send the COMPLETE plan as markdown, starting with a # heading that names it. The user may approve (carry out the plan from your next step) or keep planning — their feedback comes back in the tool result; revise and present again.";

/// Deployment-owned plan guidance.
#[derive(Debug, Clone, Default)]
pub struct PlanModeConfig {
    /// Guidance rendered as the `plan:policy` prompt section while plan
    /// mode is active.
    pub section: String,
}

/// The plan's first markdown heading (any level), or `None` when it has
/// none.
pub fn first_heading(plan: &str) -> Option<String> {
    static HEADING: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^#{1,6}\s+(.+?)\s*$").expect("static pattern"));
    for line in plan.split('\n') {
        if let Some(captures) = HEADING.captures(line) {
            return captures.get(1).map(|m| m.as_str().to_string());
        }
    }
    None
}

/// Validate deployment-owned plan guidance (TS `resolveConfig`).
pub fn resolve_config(config: &PlanModeConfig) -> Result<String, String> {
    let section = config.section.trim();
    if section.is_empty() {
        return Err("PlanModeConfig needs a non-empty `section`".to_string());
    }
    Ok(section.to_string())
}

/// Whether plan mode is active after the first `end` events. The last
/// `plan/mode` wins; a prefix with none is inactive (TS `foldPlanMode`).
pub fn fold_plan_mode(events: &[SessionEvent], end: usize) -> bool {
    let mut active = false;
    for (index, event) in events.iter().enumerate() {
        if index >= end {
            break;
        }
        if event.type_ == "plan/mode" {
            active = event.data["active"].as_bool().unwrap_or(false);
        }
    }
    active
}

/// Whether the log holds an opened turn without its closing `turn/end`.
pub fn has_open_turn(events: &[SessionEvent]) -> bool {
    let mut open = false;
    for event in events {
        if event.type_ == "turn/start" {
            open = true;
        } else if event.type_ == "turn/end" {
            open = false;
        }
    }
    open
}

/// Plan state at the last logged request header, or `None` before the
/// first header.
pub fn plan_mode_at_last_header(events: &[SessionEvent]) -> Option<bool> {
    let mut last_header: Option<usize> = None;
    for (index, event) in events.iter().enumerate() {
        if event.type_ == "request/header" {
            last_header = Some(index);
        }
    }
    last_header.map(|header| fold_plan_mode(events, header + 1))
}

/// What [`PlanModeController::set`] did (TS outcome union).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOutcome {
    /// Logged now (idle session).
    Committed,
    /// Awaiting the next accepted in-turn pre-step.
    Queued,
    /// An opposite pending selection was cleared; the logged state already
    /// matches.
    Cancelled,
    /// Already in that state.
    Noop,
}

#[derive(Debug, Clone, Copy)]
struct PendingIntent {
    active: bool,
    narrate: bool,
}

/// `ctx.planMode`: owns logged plan state, applies and narrates selected
/// state at step start, the `plan:policy` section, the `/plan` command, and
/// the stable exit tool.
pub struct PlanModeController {
    ctx: Context,
    /// Deployment-owned guidance. The section provider resolves the TS
    /// no-agent empty branch until [`dsh_system_prompt::AssembleContext`]
    /// carries the agent, so the field is currently unused at assembly
    /// time.
    #[allow(dead_code)]
    section: String,
    pending_intents: parking_lot::Mutex<HashMap<usize, PendingIntent>>,
    disposed: AtomicBool,
}

impl cordis::Service for PlanModeController {
    fn service_name(&self) -> &'static str {
        "planMode"
    }
}

impl PlanModeController {
    /// Create the service, register it as `ctx.planMode`, wire the
    /// guidance section, the pre-step boundary, the projection unit, the
    /// command, and the exit tool (TS constructor).
    pub fn install(ctx: &Context, config: &PlanModeConfig) -> Result<Arc<Self>, String> {
        let section = resolve_config(config)?;
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            section,
            pending_intents: parking_lot::Mutex::new(HashMap::new()),
            disposed: AtomicBool::new(false),
        });
        ctx.register_service(service.clone());

        // Pre-step boundary: append the pending selection only for accepted
        // in-turn steps (the step is outside Session.append publication, so
        // appending a log-only event cannot re-enter the session).
        let service_for_step = service.clone();
        let step_listener: Arc<Listener> = Arc::new(move |_dispatch_ctx: &Context, args: Vec<ArcValue>| {
            let service = service_for_step.clone();
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
                let pending = service
                    .pending_intents
                    .lock()
                    .get(&payload.agent.session().identity())
                    .copied();
                if matches!(decision, PreStepDecision::Reject) || pending.is_none() {
                    return Some(decision_value);
                }
                let pending = pending.expect("checked");
                let narration = service.narration(payload.agent.session(), pending.active);
                if let Err(error) = service.on_boundary(payload.agent.session()) {
                    service
                        .ctx
                        .named_logger(None)
                        .warn(vec![arc(format!(
                            "dsh-plan-mode: failed to append selected plan mode at step start: {error}"
                        ))]);
                    return Some(decision_value);
                }
                if !pending.narrate || narration.is_none() {
                    return Some(decision_value);
                }
                let PreStepDecision::Enter { messages } = decision else {
                    unreachable!("reject returned above");
                };
                let mut merged = messages;
                merged.push(narration.expect("checked"));
                Some(arc(PreStepDecision::Enter { messages: merged }))
            })
        });
        let _step_disposer = futures::executor::block_on(ctx.on(
            "agent/pre-step",
            step_listener,
            cordis::EventOptions::default(),
        ));

        // The plan:policy guidance section. The Rust AssembleContext carries
        // no agent, so the provider resolves the TS no-agent empty branch
        // (documented deviation).
        if let Some(system_prompt) = ctx
            .get_typed::<Arc<SystemPrompt>>("systemPrompt", false)
            .map(|slot| slot.as_ref().clone())
        {
            let disposer = system_prompt.section(
                ctx,
                PromptSection {
                    name: "plan:policy".to_string(),
                    order: 50.0,
                    text: PromptText::Static(String::new()),
                    complete: None,
                },
            );
            let _ = ctx.effect(
                "plan-mode plan:policy section",
                Box::pin(async move { Some(disposer) }),
            );
        }

        // The plan projection unit (optional child).
        let _projection_fiber = ctx.inject(
            InjectSpec::new(["sessionProjections"]),
            Arc::new(
                move |projection_ctx: &Context,
                      _config: ArcValue|
                      -> cordis::BoxFuture<'static, Result<(), PluginError>> {
                    let projection_ctx = projection_ctx.clone();
                    Box::pin(async move {
                        let Some(registry) = projection_ctx
                            .get_typed::<Arc<SessionProjectionRegistry>>(
                                "sessionProjections",
                                false,
                            )
                            .map(|slot| slot.as_ref().clone())
                        else {
                            return Ok(());
                        };
                        let definition = ProjectionDefinition {
                            key: "plan".to_string(),
                            schema: Arc::new(|value: &ArcValue| -> Result<serde_json::Value, String> {
                                let json = downcast::<serde_json::Value>(value).ok_or_else(
                                    || "plan projection view must be JSON".to_string(),
                                )?;
                                let active = json.get("active").and_then(|v| v.as_bool());
                                let pending = json.get("pending").and_then(|v| v.as_bool());
                                if active.is_none() || pending.is_none() {
                                    return Err(
                                        "plan projection must carry active and pending booleans"
                                            .to_string(),
                                    );
                                }
                                Ok(json.clone())
                            }),
                            init: Arc::new(|| {
                                arc(serde_json::json!({ "active": false, "wanted": null }))
                            }),
                            apply: Arc::new(
                                move |state: &ArcValue, event: &SessionEvent| -> ArcValue {
                                    let current = downcast::<serde_json::Value>(state)
                                        .expect("plan projection state must be plain JSON");
                                    if event.type_ == "command/run"
                                        && event.data["name"].as_str() == Some("plan")
                                    {
                                        let Some(args) = event.data.get("args").and_then(|v| v.as_str())
                                        else {
                                            return state.clone();
                                        };
                                        let wanted = args.trim() != "off";
                                        if current["wanted"].as_bool() == Some(wanted) {
                                            return state.clone();
                                        }
                                        let mut next = current.clone();
                                        next["wanted"] = serde_json::json!(wanted);
                                        return arc(next);
                                    }
                                    if event.type_ == "plan/mode" {
                                        let mut next = current.clone();
                                        next["active"] = serde_json::json!(
                                            event.data["active"].as_bool().unwrap_or(false)
                                        );
                                        next["wanted"] = serde_json::Value::Null;
                                        return arc(next);
                                    }
                                    state.clone()
                                },
                            ),
                            view: Arc::new(|state: &ArcValue| -> ArcValue {
                                let json = downcast::<serde_json::Value>(state)
                                    .expect("plan projection state must be plain JSON");
                                let active = json["active"].as_bool().unwrap_or(false);
                                let wanted = json["wanted"].as_bool();
                                arc(serde_json::json!({
                                    "active": active,
                                    "pending": wanted.is_some() && wanted != Some(active),
                                }))
                            }),
                            state_version: 1,
                        };
                        registry
                            .register(&projection_ctx, definition)
                            .map_err(|error| PluginError::new(arc(error)))?;
                        Ok(())
                    })
                },
            ),
        );

        // The /plan command (optional child).
        let service_for_command = service.clone();
        let _command_fiber = ctx.inject(
            InjectSpec::new(["commands"]),
            Arc::new(
                move |command_ctx: &Context,
                      _config: ArcValue|
                      -> cordis::BoxFuture<'static, Result<(), PluginError>> {
                    let command_ctx = command_ctx.clone();
                    let service = service_for_command.clone();
                    Box::pin(async move {
                        let Some(runtime) = command_ctx
                            .get_typed::<Arc<CommandRuntime>>("commands", false)
                            .map(|slot| slot.as_ref().clone())
                        else {
                            return Ok(());
                        };
                        let service_for_handler = service.clone();
                        let definition = CommandDefinition {
                            name: "plan".to_string(),
                            description: "Enter or leave plan mode".to_string(),
                            input: Some(CommandInputDescriptor {
                                hint: "[off|message]".to_string(),
                            }),
                            record_input: None,
                            handler: Arc::new(move |invocation| {
                                let service = service_for_handler.clone();
                                let message = invocation.raw_input.trim().to_string();
                                let agent = invocation.agent.clone();
                                Box::pin(async move {
                                    if message == "off" {
                                        let text = match service.set(&agent, false) {
                                            SetOutcome::Committed => "Plan mode off.".to_string(),
                                            SetOutcome::Queued => {
                                                "Leaving plan mode (applies from the next step).".to_string()
                                            }
                                            SetOutcome::Cancelled => "Plan mode entry cancelled.".to_string(),
                                            SetOutcome::Noop => {
                                                if fold_plan_mode(&agent.session().events(), agent.session().events().len()) {
                                                    "Leaving plan mode (applies from the next step).".to_string()
                                                } else {
                                                    "Plan mode is already inactive.".to_string()
                                                }
                                            }
                                        };
                                        return Ok(CommandResult::Success { text: Some(text), source_event_seq: None });
                                    }
                                    let outcome = service.set(&agent, true);
                                    if !message.is_empty() {
                                        agent.steer(create_user_message(
                                            vec![ContentBlock::Text { text: message }],
                                            MessageSource::User {
                                                rpc_id: None,
                                                client_time_zone: None,
                                            },
                                        ));
                                    }
                                    Ok(CommandResult::Success {
                                        text: Some(if outcome == SetOutcome::Committed {
                                            "Plan mode on. Use /plan off to leave.".to_string()
                                        } else {
                                            "Entering plan mode (applies from the next step). Use /plan off to leave."
                                                .to_string()
                                        }),
                                        source_event_seq: None,
                                    })
                                })
                            }),
                        };
                        runtime
                            .register(&command_ctx, definition)
                            .map_err(|error| PluginError::new(arc(error)))?;
                        Ok(())
                    })
                },
            ),
        );

        // The stable exit tool.
        let tools = ctx
            .get_typed::<Arc<ToolRuntime>>("tools", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| "plan-mode requires the tools service".to_string())?;
        let service_for_tool = service.clone();
        let ctx_for_tool = ctx.clone();
        let definition = ToolDefinition {
            name: EXIT_PLAN_MODE.to_string(),
            description: EXIT_DESCRIPTION.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "plan": {
                        "type": "string",
                        "description": "The complete plan, as markdown, starting with a # heading that names it."
                    }
                },
                "required": ["plan"]
            }),
            output: ToolOutputDefinition {
                schema: serde_json::json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "approved": { "type": "boolean", "const": true }
                    },
                    "required": ["approved"]
                }),
                render: Arc::new(|_args, _value| {
                    Ok(vec![ContentBlock::Text {
                        text: "Plan approved — plan mode exited; carry out the plan starting with your next step."
                            .to_string(),
                    }])
                }),
                presentation_meta: None,
            },
            timeout_ms: None,
            is_concurrency_safe: None,
            execute: Arc::new(move |args: &serde_json::Value, exec: &dsh_tools::ToolRunContext| {
                let args = args.clone();
                let agent = exec.agent.clone();
                let signal = exec.signal.lock().clone();
                let service = service_for_tool.clone();
                let ctx = ctx_for_tool.clone();
                Box::pin(async move {
                    let agent = agent
                        .ok_or_else(|| dsh_tools::ToolBodyError::plain(format!("{EXIT_PLAN_MODE} requires a calling agent (no session to switch)")))?;
                    let events = agent.session().events();
                    if !fold_plan_mode(&events, events.len()) {
                        return Err(dsh_tools::ToolBodyError::plain(format!(
                            "{EXIT_PLAN_MODE} is only available in plan mode"
                        )));
                    }
                    let plan = args["plan"].as_str().unwrap_or_default().trim();
                    if !regex::Regex::new(r"^#\s+\S").expect("static pattern").is_match(plan) {
                        return Err(dsh_tools::ToolBodyError::plain(format!(
                            "{EXIT_PLAN_MODE} requires a non-empty markdown plan starting with a # heading"
                        )));
                    }
                    let interaction = ctx
                        .get_typed::<Arc<UserQuestionService>>("userQuestions", false)
                        .map(|slot| slot.as_ref().clone());
                    let Some(interaction) = interaction else {
                        return Err(dsh_tools::ToolBodyError::plain(
                            "no user-questions channel is available to review the plan; ask the user to switch the session mode instead",
                        ));
                    };
                    let answer = interaction
                        .ask(&AskUserQuestionRequest {
                            questions: vec![AskUserQuestionItem {
                                id: REVIEW_ID.to_string(),
                                header: Some("Plan review".to_string()),
                                question: "Approve this plan and leave plan mode?".to_string(),
                                detail: Some(plan.to_string()),
                                options: Some(vec![
                                    AskUserQuestionOption {
                                        label: APPROVE_LABEL.to_string(),
                                        description: Some(
                                            "Leave plan mode; the plan is carried out from the next step."
                                                .to_string(),
                                        ),
                                    },
                                    AskUserQuestionOption {
                                        label: KEEP_PLANNING_LABEL.to_string(),
                                        description: Some(
                                            "Stay in plan mode; feedback goes back to the model."
                                                .to_string(),
                                        ),
                                    },
                                ]),
                                multi_select: None,
                                intent: Some(AskUserQuestionIntent {
                                    kind: "plan-review".to_string(),
                                    approve: APPROVE_LABEL.to_string(),
                                }),
                            }],
                            agent: Some(agent.clone()),
                            signal: Some(signal),
                        })
                        .await;
                    let answer = match answer {
                        Ok(answer) => answer,
                        Err(error) => {
                            // A dismissed review is not a failed one (the Rust
                            // questions service collapses the TS taxonomy to
                            // ASK_ABORTED).
                            if error.code == "ASK_ABORTED" {
                                return Err(dsh_tools::ToolBodyError::plain(
                                    "The user dismissed the plan review to speak instead; stay in plan mode, stop here, and wait for their message.",
                                ));
                            }
                            return Err(dsh_tools::ToolBodyError::plain(error.message));
                        }
                    };
                    // A review may outlive this plugin fiber.
                    if service.disposed.load(Ordering::SeqCst) {
                        return Err(dsh_tools::ToolBodyError::plain(
                            "the plan-mode service was reloaded while the plan was under review; present the plan again",
                        ));
                    }
                    let review_items: Vec<&dsh_user_questions::AskUserQuestionAnswerItem> = answer
                        .answers
                        .iter()
                        .filter(|entry| entry.id == REVIEW_ID)
                        .collect();
                    let item = match review_items.as_slice() {
                        [item] => *item,
                        _ => return Err(dsh_tools::ToolBodyError::plain(
                            "The user chose to keep planning; revise the plan and present it again.",
                        )),
                    };
                    if item.selected.len() != 1
                        || item.selected[0] != APPROVE_LABEL
                        || item.custom.is_some()
                    {
                        let feedback = item.custom.clone().unwrap_or_default();
                        return Err(dsh_tools::ToolBodyError::plain(if feedback.is_empty() {
                            "The user chose to keep planning; revise the plan and present it again."
                                .to_string()
                        } else {
                            format!("The user chose to keep planning; their feedback: {feedback}")
                        }));
                    }
                    // Keep plan guidance for the rest of this assistant tool
                    // batch; the silent selection is appended at the next
                    // accepted in-turn pre-step.
                    service
                        .pending_intents
                        .lock()
                        .insert(agent.session().identity(), PendingIntent { active: false, narrate: false });
                    Ok(serde_json::json!({ "approved": true }))
                })
            }),
            finalize_content: None,
            present_call: Some(Arc::new(|args: &serde_json::Value| {
                let plan = args["plan"].as_str().unwrap_or_default();
                Some(ToolCallView::Generic {
                    title: first_heading(plan).unwrap_or_else(|| "Plan".to_string()),
                    kind: Some(ToolCallKind::Other),
                    raw_input: None,
                    content: Some(vec![ContentBlock::Text {
                        text: plan.to_string(),
                    }]),
                    locations: None,
                })
            })),
            present_result: Some(Arc::new(|_args, result| {
                Some(dsh_tools::ToolResultView::Generic {
                    title: Some("Plan review".to_string()),
                    content: Some(result.content.clone()),
                })
            })),
        };
        tools
            .register_arc(ctx, Arc::new(definition))
            .map_err(|error| format!("plan-mode: {error}"))?;

        Ok(service)
    }

    /// Read the logged plan state and any selected state awaiting the next
    /// accepted in-turn pre-step.
    pub fn get(&self, agent: &Arc<dyn Agent>) -> PlanRead {
        let events = agent.session().events();
        let active = fold_plan_mode(&events, events.len());
        let pending = self
            .pending_intents
            .lock()
            .get(&agent.session().identity())
            .copied();
        match pending {
            None => PlanRead { active, pending: None },
            Some(pending) => PlanRead { active, pending: Some(pending.active) },
        }
    }

    /// Select whether plan mode should be active (TS `set`).
    pub fn set(&self, agent: &Arc<dyn Agent>, active: bool) -> SetOutcome {
        let session = agent.session();
        let events = session.events();
        let pending = self
            .pending_intents
            .lock()
            .get(&session.identity())
            .copied();
        let target = pending.map(|pending| pending.active).unwrap_or_else(|| fold_plan_mode(&events, events.len()));
        if active == target {
            return SetOutcome::Noop;
        }
        if has_open_turn(&events) {
            self.pending_intents
                .lock()
                .insert(session.identity(), PendingIntent { active, narrate: true });
            return if fold_plan_mode(&events, events.len()) == active {
                SetOutcome::Cancelled
            } else {
                SetOutcome::Queued
            };
        }
        // No open turn: commit now. Delete only after append succeeds so a
        // failed durable write leaves the selection retryable.
        if active == fold_plan_mode(&events, events.len()) {
            self.pending_intents.lock().remove(&session.identity());
            return SetOutcome::Cancelled;
        }
        session
            .append("plan/mode", serde_json::json!({ "active": active }), None)
            .expect("plan/mode append");
        self.pending_intents.lock().remove(&session.identity());
        let narration = self.narration(session, active);
        if let Some(narration) = narration {
            agent.inject(narration);
        }
        SetOutcome::Committed
    }

    /// Append one pending selection before the next request assembly.
    fn on_boundary(&self, session: &Session) -> Result<(), String> {
        let pending = self
            .pending_intents
            .lock()
            .get(&session.identity())
            .copied();
        let Some(pending) = pending else {
            return Ok(());
        };
        let events = session.events();
        if pending.active == fold_plan_mode(&events, events.len()) {
            self.pending_intents.lock().remove(&session.identity());
            return Ok(());
        }
        session.append("plan/mode", serde_json::json!({ "active": pending.active }), None)?;
        // Delete only after append succeeds so a later accepted in-turn
        // pre-step can retry a failed durable write.
        self.pending_intents.lock().remove(&session.identity());
        Ok(())
    }

    /// Build a user-switch notice when the last logged header described the
    /// other mode.
    fn narration(&self, session: &Session, target: bool) -> Option<UserMessage> {
        let told = plan_mode_at_last_header(&session.events())?;
        if told == target {
            return None;
        }
        let text = if target {
            "The user switched this session to plan mode."
        } else {
            "The user switched this session back to the default mode."
        };
        Some(create_user_message(
            vec![ContentBlock::Text { text: text.to_string() }],
            MessageSource::Plugin {
                plugin: "plan-mode".to_string(),
                form: Some(ContextForm::Notice),
                sections: None,
                summary: Some(text.to_string()),
                compaction_id: None,
                source_command_id: None,
            },
        ))
    }
}

/// The logged plus pending plan state (TS `get` result).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanRead {
    pub active: bool,
    pub pending: Option<bool>,
}

/// The Cordis plugin form (TS module exports: `name`, `inject`, `Config`,
/// `apply`).
pub struct PlanModePlugin {
    config: PlanModeConfig,
}

impl PlanModePlugin {
    pub fn new(config: PlanModeConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Plugin for PlanModePlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["tools", "systemPrompt"])
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        PlanModeController::install(ctx, &self.config)
            .map(|_| ())
            .map_err(|message| PluginError::from(anyhow::anyhow!(message)))
    }
}
