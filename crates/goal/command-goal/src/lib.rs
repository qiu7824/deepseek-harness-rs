//! Human-facing `/goal` command over the persisted same-session goal domain.
//! Rust port of `packages/goal/command-goal`.

pub mod invariant;

use std::sync::Arc;

use cordis::Context;
use dsh_commands::{
    CommandDefinition, CommandInputDescriptor, CommandInvocation, CommandResult, CommandRuntime,
};
use dsh_goal::{
    CreateGoalRequest, EditGoalRequest, GoalActivation, GoalError, GoalErrorCode, GoalPhase,
    GoalRef, GoalService, GoalView,
};

/// Cordis plugin name used by loader diagnostics.
pub const NAME: &str = "command-goal";

/// Services required by the command producer.
pub const INJECT: [&str; 2] = ["commands", "goals"];

const USAGE: &str = "Usage: /goal [<objective>|clear|edit <objective>|pause|resume]";

#[derive(Debug, Clone, PartialEq, Eq)]
enum GoalCommand {
    Show,
    Create(String),
    Edit(String),
    InvalidEdit,
    Pause,
    Resume,
    Clear,
}

fn parse_goal_command(raw_input: &str) -> GoalCommand {
    let input = raw_input.trim();
    if input.is_empty() {
        return GoalCommand::Show;
    }
    if input.eq_ignore_ascii_case("clear") {
        return GoalCommand::Clear;
    }
    if input.eq_ignore_ascii_case("pause") {
        return GoalCommand::Pause;
    }
    if input.eq_ignore_ascii_case("resume") {
        return GoalCommand::Resume;
    }
    if input.eq_ignore_ascii_case("edit") {
        return GoalCommand::InvalidEdit;
    }
    let is_edit = input
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("edit"))
        && input[4..].chars().next().is_some_and(char::is_whitespace);
    if is_edit {
        return GoalCommand::Edit(input[4..].trim().to_string());
    }
    GoalCommand::Create(input.to_string())
}

fn goal_ref(goal: &GoalView) -> GoalRef {
    GoalRef {
        id: goal.id.clone(),
        revision: goal.revision,
    }
}

fn missing_goal(action: &str) -> CommandResult {
    CommandResult::Error {
        text: format!("No goal is currently set; /goal {action} requires one. {USAGE}"),
    }
}

fn command_hint(goal: &GoalView) -> &'static str {
    match goal.phase {
        GoalPhase::Active => match goal.activation {
            GoalActivation::Armed => "/goal edit <objective>, /goal pause, /goal clear",
            GoalActivation::Disarmed => "/goal edit <objective>, /goal resume, /goal clear",
        },
        GoalPhase::Paused | GoalPhase::Blocked => {
            "/goal edit <objective>, /goal resume, /goal clear"
        }
        GoalPhase::Complete => "/goal <objective>, /goal clear",
    }
}

fn render_goal(title: &str, goal: &GoalView) -> CommandResult {
    let mut lines = vec![
        title.to_string(),
        format!("Status: {}", goal.phase.as_str()),
    ];
    if goal.phase == GoalPhase::Blocked {
        let reason = goal
            .blocked_reason
            .as_ref()
            .expect("blocked goal is missing its reason");
        lines.push(format!("Blocker: {}: {}", reason.code, reason.message));
    }
    lines.extend([
        format!("Objective: {}", goal.objective),
        format!("Rounds: {}/{}", goal.rounds_started, goal.max_goal_rounds),
        format!(
            "Activation: {}",
            match goal.activation {
                GoalActivation::Armed => "armed",
                GoalActivation::Disarmed => "disarmed",
            }
        ),
        String::new(),
        format!("Commands: {}", command_hint(goal)),
    ]);
    CommandResult::Success {
        text: Some(lines.join("\n")),
        source_event_seq: None,
    }
}

/// Execute one human goal command through the persisted domain.
fn execute_goal_command(
    invocation: &CommandInvocation,
    goals: &GoalService,
) -> Result<CommandResult, GoalError> {
    let command = parse_goal_command(&invocation.raw_input);
    let current = goals.get(&invocation.agent)?;
    match command {
        GoalCommand::Show => Ok(match current {
            None => CommandResult::Success {
                text: Some(format!("No goal is currently set.\n{USAGE}")),
                source_event_seq: None,
            },
            Some(goal) => render_goal("Goal", &goal),
        }),
        GoalCommand::InvalidEdit => Ok(CommandResult::Error {
            text: format!("Goal editing requires a replacement objective.\n{USAGE}"),
        }),
        GoalCommand::Pause => {
            let Some(current) = current else {
                return Ok(missing_goal("pause"));
            };
            let goal = goals.pause(&invocation.agent, &goal_ref(&current))?;
            Ok(render_goal("Goal paused", &goal))
        }
        GoalCommand::Resume => {
            let Some(current) = current else {
                return Ok(missing_goal("resume"));
            };
            let goal = goals.resume(&invocation.agent, &goal_ref(&current))?;
            Ok(render_goal("Goal resumed", &goal))
        }
        GoalCommand::Clear => {
            let Some(current) = current else {
                return Ok(CommandResult::Success {
                    text: Some("No goal to clear.".to_string()),
                    source_event_seq: None,
                });
            };
            goals.clear(&invocation.agent, &goal_ref(&current))?;
            Ok(CommandResult::Success {
                text: Some("Goal cleared.".to_string()),
                source_event_seq: None,
            })
        }
        GoalCommand::Edit(objective) => {
            let Some(current) = current else {
                return Ok(missing_goal("edit"));
            };
            let creating = current.phase == GoalPhase::Complete;
            let goal = if creating {
                goals.create(
                    &invocation.agent,
                    CreateGoalRequest {
                        objective,
                        max_goal_rounds: None,
                    },
                )?
            } else {
                goals.edit(
                    &invocation.agent,
                    &goal_ref(&current),
                    &EditGoalRequest {
                        objective: Some(objective),
                        max_goal_rounds: None,
                    },
                )?
            };
            Ok(render_goal(
                if creating {
                    "Goal created"
                } else {
                    "Goal updated"
                },
                &goal,
            ))
        }
        GoalCommand::Create(objective) => {
            if let Some(goal) = current
                && goal.phase != GoalPhase::Complete
            {
                return Ok(CommandResult::Error {
                    text: format!(
                        "A goal is already {}. Use /goal edit <objective> to change it or /goal clear before replacing it.",
                        goal.phase.as_str()
                    ),
                });
            }
            let goal = goals.create(
                &invocation.agent,
                CreateGoalRequest {
                    objective,
                    max_goal_rounds: None,
                },
            )?;
            Ok(render_goal("Goal created", &goal))
        }
    }
}

/// Register the global `/goal` command for every composed command adapter.
pub fn apply(ctx: &Context) -> Result<cordis::Disposer, String> {
    let commands = ctx
        .get_typed::<Arc<CommandRuntime>>("commands", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "command-goal requires the commands service".to_string())?;
    let goals = ctx
        .get_typed::<Arc<GoalService>>("goals", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "command-goal requires the goals service".to_string())?;
    commands.register(
        ctx,
        CommandDefinition {
            name: "goal".to_string(),
            description: "set or view the goal for a long-running task".to_string(),
            input: Some(CommandInputDescriptor {
                hint: "[<objective>|clear|edit <objective>|pause|resume]".to_string(),
            }),
            record_input: None,
            handler: Arc::new(move |invocation| {
                let invocation = invocation.clone();
                let goals = goals.clone();
                Box::pin(async move {
                    match execute_goal_command(&invocation, &goals) {
                        Ok(result) => Ok(result),
                        Err(error) if error.code == GoalErrorCode::CommitFailed => {
                            Err(error.to_string())
                        }
                        Err(_) => Ok(CommandResult::Error {
                            text: "The goal command is not valid for the current state. Run /goal to view available commands."
                                .to_string(),
                        }),
                    }
                })
            }),
        },
    )
}

/// The Cordis plugin form (TS module exports: `name`, `inject`, `apply`).
pub struct CommandGoalPlugin;

#[async_trait::async_trait]
impl cordis::Plugin for CommandGoalPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(INJECT)
    }

    async fn apply(
        &self,
        ctx: &Context,
        _config: cordis::ArcValue,
    ) -> Result<(), cordis::PluginError> {
        let disposer =
            apply(ctx).map_err(|message| cordis::PluginError::from(anyhow::anyhow!(message)))?;
        let _ = ctx.effect("command-goal", Box::pin(async move { Some(disposer) }));
        Ok(())
    }
}
