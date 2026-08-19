//! Plugin-owned human-command registry shared by interactive UI adapters.
//! Rust port of `packages/interaction/commands/src/index.ts` (+ `brand.ts`,
//! `types.ts`).
//!
//! # Deviations
//!
//! - The abort seam is a predicate without a reason payload; an aborted
//!   execution reports "command aborted" and settles the `command/done`
//!   error record.
//! - Handlers are async closures returning `Result<CommandResult, String>`
//!   (the TS sync/async throw path collapses to the `Err` channel).
//! - `register` takes the caller context explicitly (the workspace
//!   Proxy-rebinding convention).
//! - `commands/change` observers run through `ctx.emit` (per-listener
//!   containment already provided by the event bus).

pub mod invariant;

use std::sync::Arc;

use cordis::{ArcValue, Context, Plugin, PluginError};
use dsh_agent::Agent;
use dsh_brand::Branded;
use dsh_scope::{NamedEntries, ScopeKey, ScopeLayer, ScopedLayers};
use dsh_session::{Session, SessionId};
use serde::{Deserialize, Serialize};

/// The brand marker for [`CommandId`].
#[doc(hidden)]
pub enum CommandIdTag {}

/// Pairing id carried by one command execution's lifecycle events.
pub type CommandId = Branded<CommandIdTag>;

/// Brand a minted command id (TS `CommandId`).
pub fn command_id(value: impl Into<String>) -> CommandId {
    CommandId::new(value)
}

/// Immutable metadata for a command's optional unstructured input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandInputDescriptor {
    /// Placeholder shown before the user supplies free-form input.
    pub hint: String,
}

/// Expected command outcome rendered directly by the dispatching UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CommandResult {
    Success {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Earlier authoritative domain event that owns a richer
        /// presentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_event_seq: Option<u64>,
    },
    Error {
        text: String,
    },
}

/// One settled command execution: the normalized result plus the pairing id.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandExecution {
    pub command_id: CommandId,
    pub result: CommandResult,
}

/// Handler-free immutable command view returned to UI adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandDescriptor {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<CommandInputDescriptor>,
}

/// The cancellation seam (TS `AbortSignal`).
pub type CommandAbort = Arc<dyn Fn() -> bool + Send + Sync>;

/// Invocation passed to one registered command handler.
#[derive(Clone)]
pub struct CommandInvocation {
    /// Pairing id already written to this invocation's `command/run` event.
    pub command_id: CommandId,
    /// Exact agent whose UI received the command.
    pub agent: Arc<dyn Agent>,
    /// Exact text following the registered command name, including separator
    /// whitespace.
    pub raw_input: String,
    /// Cancellation signal owned by the dispatching UI request.
    pub signal: CommandAbort,
}

/// Plugin-owned command registration.
#[derive(Clone)]
pub struct CommandDefinition {
    /// Lowercase command name without the leading slash.
    pub name: String,
    /// Human-readable summary used in discovery UI.
    pub description: String,
    /// Optional free-form input hint advertised to capable clients.
    pub input: Option<CommandInputDescriptor>,
    /// Whether `command/run` records `rawInput`. Defaults to true.
    pub record_input: Option<bool>,
    /// Execute against the receiving agent without sending the command to
    /// the model.
    pub handler: Arc<
        dyn Fn(&CommandInvocation) -> cordis::BoxFuture<'static, Result<CommandResult, String>>
            + Send
            + Sync,
    >,
}

/// Syntactically valid slash command before registry resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    /// Lowercase command name without the leading slash.
    pub name: String,
    /// Exact text following the command name.
    pub raw_input: String,
}

const COMMAND_NAME: &str = r"^[a-z][a-z0-9_-]*$";

/// Parse an exact slash command without normalizing its trailing input (TS
/// `parseCommand`; the lookahead boundary is checked manually — the Rust
/// regex crate has no look-around).
pub fn parse_command(line: &str) -> Option<ParsedCommand> {
    let pattern = regex::Regex::new(r"^/([a-z][a-z0-9_-]*)").expect("static pattern");
    let captures = pattern.captures(line)?;
    let name = captures.get(1)?.as_str();
    let end = captures.get(0)?.end();
    let boundary = line[end..].chars().next();
    if !boundary.is_none_or(|ch| matches!(ch, '\t' | '\n' | '\r' | ' ')) {
        return None;
    }
    Some(ParsedCommand {
        name: name.to_string(),
        raw_input: line[end..].to_string(),
    })
}

#[derive(Clone)]
struct RegisteredCommand {
    definition: CommandDefinition,
    descriptor: CommandDescriptor,
}

/// All command registrations owned by one global or scoped layer.
struct CommandLayer {
    commands: NamedEntries<RegisteredCommand>,
}

impl CommandLayer {
    fn new(scope: Option<&ScopeKey>) -> Self {
        let scoped = scope.is_some();
        Self {
            commands: NamedEntries::new(
                move |name: &str| -> Box<dyn std::error::Error + Send + Sync> {
                    let message = if scoped {
                        format!("command \"{name}\" is already registered in this scope")
                    } else {
                        format!(
                            "command \"{name}\" is already registered (for a per-agent variant, mount a command-injected plugin under that agent's `agent.ctx`)"
                        )
                    };
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        message,
                    ))
                },
            ),
        }
    }
}

impl ScopeLayer for CommandLayer {
    fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

/// Human-command registry. Plain-context definitions are global; definitions
/// registered through a command-injected child of an agent context shadow
/// globals for that agent (TS `CommandRuntime`).
pub struct CommandRuntime {
    ctx: Context,
    layers: ScopedLayers<CommandLayer>,
    command_seq: std::sync::atomic::AtomicU64,
    instance_token: String,
}

impl CommandRuntime {
    /// Create the runtime and register it as the `commands` service.
    pub fn install(ctx: &Context) -> Arc<Self> {
        let notify_ctx = ctx.clone();
        let runtime = Arc::new(Self {
            ctx: ctx.clone(),
            layers: ScopedLayers::new(CommandLayer::new, move || {
                notify_ctx.emit("commands/change", Vec::new());
            }),
            command_seq: std::sync::atomic::AtomicU64::new(0),
            instance_token: uuid::Uuid::new_v4().to_string()[..8].to_string(),
        });
        ctx.register_service(runtime.clone());
        runtime
    }

    /// Register a global or calling-agent-scoped command (the caller context
    /// is explicit — the TS Proxy rebinding convention).
    pub fn register(
        &self,
        caller: &Context,
        definition: CommandDefinition,
    ) -> Result<cordis::Disposer, String> {
        let registered = normalize_definition(definition)?;
        Ok(self.layers.effect(
            caller,
            move |layer| {
                layer
                    .commands
                    .insert(&registered.definition.name, registered.clone())
            },
            "commands.register()",
            true,
        ))
    }

    /// List the effective immutable command descriptors for one agent.
    pub fn list(&self, agent: &Arc<dyn Agent>) -> Vec<CommandDescriptor> {
        let mut descriptors: Vec<CommandDescriptor> = self
            .view(agent)
            .into_values()
            .map(|command| command.descriptor)
            .collect();
        descriptors.sort_by(|left, right| left.name.cmp(&right.name));
        descriptors
    }

    /// Resolve one effective command definition.
    pub fn find(&self, agent: &Arc<dyn Agent>, name: &str) -> Option<CommandDefinition> {
        let view = self.view(agent);
        view.get(name).map(|command| command.definition.clone())
    }

    /// Parse and execute a known command without sending it to the model (TS
    /// `execute`). Returns `None` when syntax or name does not resolve.
    pub async fn execute(
        &self,
        agent: &Arc<dyn Agent>,
        line: &str,
        signal: CommandAbort,
    ) -> Result<Option<CommandExecution>, String> {
        let Some(parsed) = parse_command(line) else {
            return Ok(None);
        };
        let view = self.view(agent);
        let Some(command) = view.get(&parsed.name) else {
            return Ok(None);
        };
        if signal() {
            return Err("command aborted".to_string());
        }
        let command_id = self.mint_command_id();
        let mut run_data = serde_json::json!({
            "commandId": command_id.as_str(),
            "name": parsed.name,
            "source": { "kind": "user" },
        });
        if command.definition.record_input != Some(false) {
            run_data["args"] = serde_json::Value::String(parsed.raw_input.clone());
        }
        self.append_lifecycle(agent.session(), "command/run", run_data)?;
        let invocation = CommandInvocation {
            command_id: command_id.clone(),
            agent: agent.clone(),
            raw_input: parsed.raw_input,
            signal: signal.clone(),
        };
        let output = {
            let handler_future = (command.definition.handler)(&invocation);
            tokio::pin!(handler_future);
            let poller = async {
                loop {
                    if signal() {
                        return Err("command aborted".to_string());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                }
            };
            tokio::pin!(poller);
            match tokio::select! {
                output = &mut handler_future => output,
                abort = &mut poller => abort,
            } {
                Ok(result) => result,
                Err(error) => {
                    self.append_lifecycle(
                        agent.session(),
                        "command/done",
                        serde_json::json!({ "commandId": command_id.as_str(), "kind": "error", "text": error }),
                    )
                    .map_err(|append_error| format!("command/done append failed: {append_error}"))?;
                    return Err(error);
                }
            }
        };
        let result = normalize_result(&parsed.name, &output)?;
        let mut done_data = serde_json::json!({
            "commandId": command_id.as_str(),
            "kind": match &result {
                CommandResult::Success { .. } => "success",
                CommandResult::Error { .. } => "error",
            },
        });
        match &result {
            CommandResult::Success {
                text,
                source_event_seq,
            } => {
                if let Some(text) = text {
                    done_data["text"] = serde_json::Value::String(text.clone());
                }
                if let Some(source_event_seq) = source_event_seq {
                    done_data["sourceEventSeq"] = serde_json::json!(source_event_seq);
                }
            }
            CommandResult::Error { text } => {
                done_data["text"] = serde_json::Value::String(text.clone());
            }
        }
        self.append_lifecycle(agent.session(), "command/done", done_data)?;
        Ok(Some(CommandExecution { command_id, result }))
    }

    /// Mint the next pairing id (monotonic; instance-token-prefixed so a
    /// resumed log never repeats one).
    fn mint_command_id(&self) -> CommandId {
        let seq = self
            .command_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        command_id(format!("cmd-{}-{seq}", self.instance_token))
    }

    /// Append one log-only lifecycle event directly.
    fn append_lifecycle(
        &self,
        session: &Session,
        type_: &str,
        data: serde_json::Value,
    ) -> Result<dsh_session::SessionEvent, String> {
        session.append(type_, data, None)
    }

    /// Resolve global definitions followed by exact scoped shadows.
    fn view(&self, agent: &Arc<dyn Agent>) -> std::collections::HashMap<String, RegisteredCommand> {
        let mut view = std::collections::HashMap::new();
        let merged = self
            .layers
            .merge(Some(agent.scope_key()), |layer| &layer.commands);
        for (name, command) in merged {
            view.insert(name, command);
        }
        view
    }
}

impl cordis::Service for CommandRuntime {
    fn service_name(&self) -> &'static str {
        "commands"
    }
}

fn normalize_definition(definition: CommandDefinition) -> Result<RegisteredCommand, String> {
    let pattern = regex::Regex::new(COMMAND_NAME).expect("static pattern");
    if !pattern.is_match(&definition.name) {
        return Err(format!(
            "command name \"{}\" must match {COMMAND_NAME}",
            definition.name
        ));
    }
    if definition.description.trim().is_empty() {
        return Err(format!(
            "command \"{}\" description must not be empty",
            definition.name
        ));
    }
    if let Some(input) = &definition.input {
        if input.hint.trim().is_empty() {
            return Err(format!(
                "command \"{}\" input hint must not be empty",
                definition.name
            ));
        }
    }
    let descriptor = CommandDescriptor {
        name: definition.name.clone(),
        description: definition.description.clone(),
        input: definition.input.clone(),
    };
    Ok(RegisteredCommand {
        definition,
        descriptor,
    })
}

fn normalize_result(command: &str, value: &CommandResult) -> Result<CommandResult, String> {
    match value {
        CommandResult::Success {
            text,
            source_event_seq,
        } => Ok(CommandResult::Success {
            text: text.clone(),
            source_event_seq: *source_event_seq,
        }),
        CommandResult::Error { text } => {
            if text.trim().is_empty() {
                return Err(format!(
                    "command \"{command}\" error text must be a non-empty string"
                ));
            }
            Ok(CommandResult::Error { text: text.clone() })
        }
    }
}

/// The Cordis plugin form (the TS loader mounts the class through a
/// concrete provider; this zero-config plugin mounts the bare registry).
pub struct CommandRuntimePlugin;

#[async_trait::async_trait]
impl Plugin for CommandRuntimePlugin {
    fn name(&self) -> Option<&'static str> {
        Some("commands")
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        CommandRuntime::install(ctx);
        Ok(())
    }
}
