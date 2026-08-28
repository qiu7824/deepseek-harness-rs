//! Tool registry, model presentation, and pre/guard/around/post/result
//! execution pipeline. Rust port of `packages/core/tools/src/index.ts`
//! (native presentation mode; the `code` presentation transport and SDK
//! renderers arrive with the dsh-code-runtime milestone).
//!
//! # Deviations
//!
//! - `ToolExecutionToken` (a TS `Symbol`) collapses to a monotonic `u64`.
//! - `AbortSignal` collapses to a cancellation predicate
//!   (`Arc<dyn Fn() -> bool + Send + Sync>`); wrapper signal replacement
//!   writes the `signal` cell, and caller/wrapper fusion composes the two
//!   predicates (no listener cleanup — predicates leak nothing).
//! - The TS `WeakMap` registries (deferred contexts, cancellation state,
//!   content finalizers, canonical markers, concluding executions) key by
//!   the execution token in one `Mutex<HashMap>`.
//! - `ToolExecutionResult` is one struct (value present exactly on
//!   success, `error` exactly on failure) instead of the discriminated
//!   union; canonical marking rides a rebuild with `canonical_token`.
//! - `snapshotJsonValue`/`deepFreeze` collapse to identity: `serde_json`
//!   values are lossless by construction and owned Rust values are frozen.
//! - The `code`/`both` presentation modes reject at schema assembly until
//!   `dsh-code-runtime` is ported (native mode is complete).

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cordis::{ArcValue, BoxFuture, Context, DispatchMode, Disposer, Service, arc, downcast_arc};
use dsh_agent::Agent;
use dsh_llm::{CallId, ContentBlock, ToolSchema, UserMessage};
use dsh_scope::{
    AnonymousEntries, NamedEntries, PreparedRegistration, ScopeKey, ScopeLayer, ScopedLayers,
    scope_of, scope_target,
};
use dsh_system_prompt::{AssembleContext, ToolProvider, ToolProviderResult};
use futures::FutureExt;
use parking_lot::Mutex;
use serde_json::Value as JsonValue;

use crate::json_schema::{assert_supported_json_schema, validate_json_schema_value};
use crate::presentation::{ToolCallView, ToolResult, ToolResultView};

/// Cancellation predicate (TS `AbortSignal`).
pub type AbortPredicate = Arc<dyn Fn() -> bool + Send + Sync>;

/// Canonical error code for cancellation after a tool body was invoked.
pub const TOOL_ABORTED: &str = "ABORTED";

/// Canonical error code for cancellation before a tool body was invoked.
pub const TOOL_ABORTED_BEFORE_DISPATCH: &str = "ABORTED_BEFORE_DISPATCH";

/// The reserved Code Mode presentation transport name.
pub const RUN_CODE_NAME: &str = "run_code";

/// Structured error metadata for a failed tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolErrorInfo {
    pub name: String,
    pub code: String,
}

/// Canonical failure detail; internal routing information remains optional.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolFailure {
    /// Human-readable failure message without the Native `Error: ` envelope.
    pub message: String,
    /// Internal error class/code used by policy and durable diagnostics.
    pub info: Option<ToolErrorInfo>,
}

/// The failure channel a tool body returns through.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolBodyError {
    pub message: String,
    pub info: Option<ToolErrorInfo>,
}

impl ToolBodyError {
    pub fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            info: None,
        }
    }

    pub fn coded(message: impl Into<String>, name: &str, code: &str) -> Self {
        Self {
            message: message.into(),
            info: Some(ToolErrorInfo {
                name: name.to_string(),
                code: code.to_string(),
            }),
        }
    }
}

impl std::fmt::Display for ToolBodyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for ToolBodyError {}

/// The model requests a tool that isn't registered (or only reachable
/// through the Code Mode transport); code `UNKNOWN_TOOL`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolNotFoundError {
    pub tool_name: String,
    pub reachable_from: Option<String>,
}

impl ToolNotFoundError {
    pub fn new(tool_name: &str, reachable_from: Option<&str>) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            reachable_from: reachable_from.map(str::to_string),
        }
    }

    pub fn code(&self) -> &'static str {
        "UNKNOWN_TOOL"
    }
}

impl std::fmt::Display for ToolNotFoundError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.reachable_from {
            Some(reachable_from) => {
                write!(
                    formatter,
                    "unknown tool \"{}\": {}",
                    self.tool_name, reachable_from
                )
            }
            None => write!(formatter, "unknown tool \"{}\"", self.tool_name),
        }
    }
}

impl std::error::Error for ToolNotFoundError {}

/// A tool body or post-policy value violates its declared output; code
/// `INVALID_TOOL_OUTPUT`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutputError {
    pub tool_name: String,
    pub violations: Vec<String>,
}

impl ToolOutputError {
    pub fn code(&self) -> &'static str {
        "INVALID_TOOL_OUTPUT"
    }
}

impl std::fmt::Display for ToolOutputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "tool \"{}\" returned invalid output: {}",
            self.tool_name,
            self.violations.join("; ")
        )
    }
}

impl std::error::Error for ToolOutputError {}

/// How the registry presents its tools to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPresentationMode {
    Native,
    Code,
    Both,
}

/// Plugin config: how the registered tools are presented to the model.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Model presentation (`native` default; `code`/`both` need the
    /// dsh-code-runtime milestone).
    pub mode: Option<ToolPresentationMode>,
    /// Concurrency cap for a `run_code` program's overlapping sub-calls.
    pub max_parallel_sub_calls: Option<u64>,
}

/// Per-scope filter over global tools.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ToolRestriction {
    /// Global tool names that stay visible; everything else is removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,
    /// Global tool names removed from visibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<String>>,
}

/// One restriction compiled at registration for repeated live-global lookup.
#[derive(Debug, Clone, Default)]
struct CompiledToolRestriction {
    allow: Option<Vec<String>>,
    deny: Option<Vec<String>>,
}

impl CompiledToolRestriction {
    fn admits(&self, name: &str) -> bool {
        if let Some(allow) = &self.allow
            && !allow.iter().any(|entry| entry == name)
        {
            return false;
        }
        if let Some(deny) = &self.deny
            && deny.iter().any(|entry| entry == name)
        {
            return false;
        }
        true
    }
}

/// A monotonic execution guard evaluated after every `tools/pre-execute`
/// listener and before the tool body.
pub type ToolGuard = Arc<dyn Fn(&ToolExecution) -> Option<String> + Send + Sync>;

pub type ToolOutputRenderer =
    Arc<dyn Fn(&JsonValue, &JsonValue) -> Result<Vec<ContentBlock>, String> + Send + Sync>;
pub type ToolPresentationMeta =
    Arc<dyn Fn(&JsonValue, &JsonValue) -> Result<JsonValue, String> + Send + Sync>;
pub type ToolConcurrencyPredicate = Arc<dyn Fn(&JsonValue) -> bool + Send + Sync>;
pub type ToolBody = Arc<
    dyn Fn(&JsonValue, &ToolRunContext) -> BoxFuture<'static, Result<JsonValue, ToolBodyError>>
        + Send
        + Sync,
>;
pub type ToolContentFinalizer =
    Arc<dyn Fn(&ToolExecution, &ToolExecutionResult) -> Option<Vec<ContentBlock>> + Send + Sync>;
pub type ToolCallPresenter = Arc<dyn Fn(&JsonValue) -> Option<ToolCallView> + Send + Sync>;
pub type ToolResultPresenter =
    Arc<dyn Fn(&JsonValue, &ToolResult) -> Option<ToolResultView> + Send + Sync>;

/// Tool-owned canonical output contract used after the body returns a JSON
/// value.
pub struct ToolOutputDefinition {
    /// Raw supported JSON Schema enforced against every successful canonical
    /// value.
    pub schema: JsonValue,
    /// Pure projection from validated arguments and value to Native/model
    /// content. A returned `Err` becomes `output.render failed: <message>`.
    pub render: ToolOutputRenderer,
    /// Pure replayable presentation projection, computed only for top-level
    /// calls.
    pub presentation_meta: Option<ToolPresentationMeta>,
}

/// A registered tool: its schema plus the execution function.
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// Raw JSON Schema for the tool arguments (wire shape).
    pub parameters: JsonValue,
    /// Mandatory canonical output declaration.
    pub output: ToolOutputDefinition,
    /// Cooperative tool-call timeout budget in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Whether this call may join a parallel group; only an exact `true`
    /// opts in.
    pub is_concurrency_safe: Option<ToolConcurrencyPredicate>,
    /// Run one accepted call and return only its canonical lossless-JSON
    /// value.
    pub execute: ToolBody,
    /// Synchronous last-mile transform for model-facing content.
    pub finalize_content: Option<ToolContentFinalizer>,
    /// How to present the PENDING state of one call in a UI.
    pub present_call: Option<ToolCallPresenter>,
    /// How to present the COMPLETED state of one call in a UI.
    pub present_result: Option<ToolResultPresenter>,
}

/// Caller-supplied description of one tool call.
#[derive(Clone)]
pub struct ToolExecutionInput {
    pub call_id: CallId,
    /// Root model-requested call owning this execution tree.
    pub root_call_id: Option<CallId>,
    pub name: String,
    /// Losslessly JSON-serializable parsed arguments.
    pub arguments: JsonValue,
    /// The agent on whose behalf the call runs (set by the agent loop).
    pub agent: Option<Arc<dyn Agent>>,
    /// Opaque token of the enclosing transport execution, when one exists.
    pub parent: Option<u64>,
    /// Required caller-owned cancellation for this invocation.
    pub signal: AbortPredicate,
}

/// One pending tool call inside the registry pipeline.
pub struct ToolExecution {
    /// Registry-assigned identity shared with nested calls only as their
    /// opaque `parent` token.
    pub token: u64,
    pub call_id: CallId,
    pub root_call_id: CallId,
    pub name: String,
    pub arguments: JsonValue,
    pub agent: Option<Arc<dyn Agent>>,
    pub parent: Option<u64>,
    /// Cancellation visible to the next wrapper or tool body; a
    /// `tools/execute` wrapper may replace it for its delegated lifetime.
    pub signal: Mutex<AbortPredicate>,
}

/// Scheduling mode for one pending call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionMode {
    Parallel,
    Exclusive,
}

/// The completed outcome of one tool call (the TS discriminated union
/// collapses into one struct: `value` on success, `error` on failure).
pub struct ToolExecutionResult {
    pub is_error: bool,
    /// Present exactly when `is_error`.
    pub error: Option<ToolFailure>,
    /// Present exactly on success; deliberately omitted from durable events.
    pub value: Option<JsonValue>,
    pub content: Vec<ContentBlock>,
    pub meta: Option<JsonValue>,
    pub additional_contexts: Vec<UserMessage>,
    /// The agent loop stops after committing this successful result batch.
    pub concludes_turn: bool,
    /// Registry-owned canonical marker (the TS `WeakMap` identity);
    /// external constructors (e.g. the loop scheduler's synthetic abort
    /// results) leave it `0`.
    pub canonical_token: u64,
}

impl ToolExecutionResult {
    pub(crate) fn canonical(&self, token: u64) -> bool {
        self.canonical_token == token
    }
}

/// Pre-dispatch decision.
#[derive(Debug, Clone, PartialEq)]
pub enum PreToolDecision {
    Allow,
    Deny { reason: String },
    Ask { reason: Option<String> },
}

/// Post-dispatch decision.
#[derive(Debug, Clone, PartialEq)]
pub enum PostToolDecision {
    Accept {
        content: Option<Vec<ContentBlock>>,
        value: Option<JsonValue>,
        additional_contexts: Option<Vec<UserMessage>>,
    },
    Block {
        feedback: Vec<ContentBlock>,
        additional_contexts: Option<Vec<UserMessage>>,
    },
}

/// Runtime context handed to a tool implementation after the registry has
/// accepted a [`ToolExecution`].
pub struct ToolRunContext {
    /// The live execution view (identity, arguments, signal).
    pub execution: Arc<ToolExecution>,
    state: Arc<Mutex<ExecutionState>>,
}

impl ToolRunContext {
    /// Defer one context until this tool's final result reaches the agent
    /// loop.
    pub fn defer_context(&self, context: UserMessage) {
        self.state.lock().deferred.push(context);
    }

    /// Mark a successful final result as terminal for the current agent
    /// turn.
    pub fn conclude_turn(&self) {
        self.state.lock().concluded = true;
    }
}

impl std::ops::Deref for ToolRunContext {
    type Target = ToolExecution;

    fn deref(&self) -> &Self::Target {
        &self.execution
    }
}

struct ExecutionState {
    deferred: Vec<UserMessage>,
    concluded: bool,
    body_invoked: bool,
    caller_signal: AbortPredicate,
    finalizer: Option<ToolContentFinalizer>,
}

/// One scope's complete tool-registry contribution.
struct ToolLayer {
    tools: NamedEntries<Arc<ToolDefinition>>,
    restrictions: AnonymousEntries<CompiledToolRestriction>,
    guards: AnonymousEntries<ToolGuard>,
    mode: Arc<Mutex<Option<ToolPresentationMode>>>,
}

impl ToolLayer {
    fn new(scope: Option<&ScopeKey>) -> Self {
        let _ = scope;
        Self {
            tools: NamedEntries::new(move |name| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("tool \"{name}\" is already registered"),
                ))
            }),
            restrictions: AnonymousEntries::new(),
            guards: AnonymousEntries::new(),
            mode: Arc::new(Mutex::new(None)),
        }
    }

    /// Whether every compiled restriction in this layer admits a global tool
    /// name.
    fn admits(&self, name: &str) -> bool {
        self.restrictions
            .values()
            .iter()
            .all(|filter| filter.admits(name))
    }

    /// First monotonic denial from this layer's live guard registrations.
    fn guard_reason(&self, exec: &ToolExecution) -> Option<String> {
        for guard in self.guards.values() {
            if let Some(reason) = guard(exec) {
                return Some(reason);
            }
        }
        None
    }
}

impl ScopeLayer for ToolLayer {
    fn is_empty(&self) -> bool {
        self.tools.is_empty()
            && self.restrictions.is_empty()
            && self.guards.is_empty()
            && self.mode.lock().is_none()
    }
}

enum CreatedExecution {
    Ready {
        run_ctx: Arc<ToolRunContext>,
    },
    Final {
        run_ctx: Arc<ToolRunContext>,
        result: Arc<ToolExecutionResult>,
    },
}

/// Scheduler-only result after ordered pre-execute and guards (the TS
/// `ScheduledToolPreparation`): a `PostResult` still receives post-execute;
/// a `FinalResult` bypasses it.
pub enum Preparation {
    Dispatch {
        run_ctx: Arc<ToolRunContext>,
    },
    PostResult {
        run_ctx: Arc<ToolRunContext>,
        result: Arc<ToolExecutionResult>,
    },
    FinalResult {
        run_ctx: Arc<ToolRunContext>,
        result: Arc<ToolExecutionResult>,
    },
}

/// Scheduler-only dispatch result (the TS `ScheduledToolDispatch`): a
/// `PostResult` still receives post-execute; a `FinalResult` already matches
/// [`ToolRuntime::execute`] failure semantics.
pub enum DispatchOutcome {
    PostResult(Arc<ToolExecutionResult>),
    FinalResult(Arc<ToolExecutionResult>),
}

/// The abstract `tools` service: registry plus streaming execution
/// pipeline, interceptable via `tools/pre-execute`, `tools/execute`, and
/// `tools/post-execute`.
pub struct ToolRuntime {
    ctx: Context,
    layers: ScopedLayers<ToolLayer>,
    default_mode: ToolPresentationMode,
    /// Consumed by the run_code transport in the dsh-code-runtime milestone.
    #[allow(dead_code)]
    max_parallel_sub_calls: u64,
    next_token: AtomicU64,
    executions: Mutex<HashMap<u64, Arc<Mutex<ExecutionState>>>>,
    code_transport: Mutex<Option<Arc<ToolDefinition>>>,
}

impl Service for ToolRuntime {
    fn service_name(&self) -> &'static str {
        "tools"
    }
}

impl ToolRuntime {
    /// Create the runtime and register it as `ctx.tools`, wiring the
    /// tool-schema provider into the `systemPrompt` service (the TS
    /// `static inject = ['systemPrompt']`).
    pub fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        let system_prompt = ctx
            .get_typed::<Arc<dsh_system_prompt::SystemPrompt>>("systemPrompt", false)
            .ok_or_else(|| "dsh-tools requires the systemPrompt service".to_string())?;
        let default_mode = config.mode.unwrap_or(ToolPresentationMode::Native);
        let max_parallel_sub_calls = config.max_parallel_sub_calls.unwrap_or(10);
        if max_parallel_sub_calls == 0 {
            return Err("maxParallelSubCalls must be a positive integer".to_string());
        }
        let runtime = {
            let change_ctx = ctx.clone();
            Arc::new(Self {
                ctx: ctx.clone(),
                layers: ScopedLayers::new(ToolLayer::new, move || {
                    change_ctx.emit("tools/change", Vec::new());
                }),
                default_mode,
                max_parallel_sub_calls,
                next_token: AtomicU64::new(1),
                executions: Mutex::new(HashMap::new()),
                code_transport: Mutex::new(None),
            })
        };
        ctx.register_service(runtime.clone());
        // Wire the schema provider into systemPrompt (tools/change
        // notification is the layers' on_change).
        let runtime_for_provider = Arc::clone(&runtime);
        let provider: ToolProvider = Arc::new(move |assemble: &AssembleContext| {
            runtime_for_provider.wire_schemas(assemble.scope.as_ref())
        });
        let _ = system_prompt.tools(ctx, provider);
        Ok(runtime)
    }

    /// The presentation one scope's agent sees: its own declaration, else
    /// the deployment default.
    fn mode_for(&self, scope: Option<&ScopeKey>) -> ToolPresentationMode {
        for layer in self.layers.chain_layers(scope).iter().rev() {
            if let Some(mode) = *layer.mode.lock() {
                return mode;
            }
        }
        self.default_mode
    }

    /// Derive the registered pending-call presentation for one scope without
    /// executing or materializing an Agent. Cold history readers use the
    /// preset's standing scope so deliverable locations remain available.
    pub fn present_call_for_scope(
        &self,
        scope: Option<&ScopeKey>,
        name: &str,
        arguments: &JsonValue,
    ) -> Option<ToolCallView> {
        let definition = self.view(scope).visible.get(name).cloned()?;
        definition.present_call.as_ref()?.as_ref()(arguments)
    }

    /// Present the calling scope's tools in `mode` instead of the
    /// deployment default.
    pub fn present_as(
        &self,
        caller: &Context,
        mode: ToolPresentationMode,
    ) -> Result<Disposer, String> {
        if scope_of(caller).is_none() {
            return Err("tools.presentAs() requires a scoped context (agent.ctx): a context-global presentation is the `mode` config field on the tools row".to_string());
        }
        Ok(self.layers.effect(
            caller,
            move |layer| {
                let cell = Arc::clone(&layer.mode);
                if let Some(existing) = *cell.lock() {
                    panic!("tools.presentAs({mode:?}) conflicts with {existing:?} already declared for this scope; one composition selects one presentation");
                }
                *cell.lock() = Some(mode);
                Box::new(move || {
                    *cell.lock() = None;
                }) as Box<dyn Fn() + Send + Sync>
            },
            "tools.presentAs()",
            true,
        ))
    }

    /// Register globally or in the calling agent scope.
    pub fn register(
        &self,
        caller: &Context,
        definition: ToolDefinition,
    ) -> Result<Disposer, String> {
        self.register_arc(caller, Arc::new(definition))
    }

    /// Register a definition the caller already owns as an `Arc`, so
    /// identity checks (the skill catalog's exact-registration guard) can
    /// compare the registered pointer with `get()`.
    pub fn register_arc(
        &self,
        caller: &Context,
        definition: Arc<ToolDefinition>,
    ) -> Result<Disposer, String> {
        match self.prepare_register_arc(caller, definition) {
            Ok(prepared) => Ok(prepared.commit(caller)),
            Err(error) if error.contains("already registered") => panic!("{error}"),
            Err(error) => Err(error),
        }
    }

    /// Prepare one definition synchronously without binding it to the caller's
    /// fiber yet. Dropping the handle rolls the insertion back.
    pub fn prepare_register_arc(
        &self,
        caller: &Context,
        definition: Arc<ToolDefinition>,
    ) -> Result<PreparedRegistration, String> {
        if definition.name == RUN_CODE_NAME {
            return Err(format!(
                "tool name \"{RUN_CODE_NAME}\" is reserved for the Code Mode presentation transport and cannot be registered or shadowed"
            ));
        }
        assert_supported_json_schema(&definition.parameters).map_err(|error| {
            format!(
                "tool \"{}\" has invalid parameters schema: {error}",
                definition.name
            )
        })?;
        assert_supported_json_schema(&definition.output.schema).map_err(|error| {
            format!(
                "tool \"{}\" has invalid output schema: {error}",
                definition.name
            )
        })?;
        if definition.timeout_ms.is_some_and(|timeout| timeout == 0) {
            return Err(format!(
                "tool \"{}\" timeoutMs must be a positive finite number",
                definition.name
            ));
        }
        self.layers.try_prepare_named(
            caller,
            |layer| &layer.tools,
            definition.name.clone(),
            definition,
            "tools.register()",
            true,
        )
    }

    /// Restrict global tools for the calling agent scope.
    pub fn restrict(&self, caller: &Context, filter: ToolRestriction) -> Result<Disposer, String> {
        let scope = scope_of(caller)
            .ok_or_else(|| "tools.restrict() requires a scoped context (agent.ctx): a context-global restriction would mask every agent — deny the tool for the intended agent instead".to_string())?;
        if filter.allow.is_none() && filter.deny.is_none() {
            return Err("tools.restrict({}) is a no-op: pass `allow` and/or `deny` (an empty filter is almost always a materialized-empty-config bug)".to_string());
        }
        let compiled = CompiledToolRestriction {
            allow: filter.allow.clone(),
            deny: filter.deny.clone(),
        };
        let mut named: Vec<&String> = Vec::new();
        if let Some(allow) = &filter.allow {
            named.extend(allow);
        }
        if let Some(deny) = &filter.deny {
            named.extend(deny);
        }
        if named.iter().any(|name| name.as_str() == RUN_CODE_NAME) {
            return Err(format!(
                "tools.restrict() cannot name reserved Code Mode presentation transport \"{RUN_CODE_NAME}\"; restrict end-capability tools instead"
            ));
        }
        let known = self.restrictable_names(Some(&scope));
        let unknown: Vec<&String> = named
            .iter()
            .filter(|name| !known.iter().any(|known| known == name.as_str()))
            .copied()
            .collect();
        if !unknown.is_empty() {
            let rendered = unknown
                .iter()
                .map(|name| format!("\"{name}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let mut known_names: Vec<&String> = known.iter().collect();
            known_names.sort();
            let known_rendered = known_names
                .iter()
                .map(|name| format!("\"{}\"", name.as_str()))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "tools.restrict() names unknown global tool{} {rendered}; known global tools: {known_rendered}",
                if unknown.len() > 1 { "s" } else { "" }
            ));
        }
        Ok(self.layers.effect(
            caller,
            move |layer| layer.restrictions.append(compiled.clone()),
            "tools.restrict()",
            true,
        ))
    }

    /// Register a monotonic guard after the extensible `tools/pre-execute`
    /// waterfall.
    pub fn guard(&self, caller: &Context, guard: ToolGuard) -> Result<Disposer, String> {
        Ok(self.layers.effect(
            caller,
            move |layer| layer.guards.append(guard.clone()),
            "tools.guard()",
            false,
        ))
    }

    /// First monotonic denial from the global then the scope chain's guard
    /// layers, farthest first.
    fn guard_reason(&self, exec: &ToolExecution) -> Option<String> {
        if let Some(reason) = self.layers.global.guard_reason(exec) {
            return Some(reason);
        }
        let Some(agent) = &exec.agent else {
            return None;
        };
        for layer in self.layers.chain_layers(Some(agent.scope_key())) {
            if let Some(reason) = layer.guard_reason(exec) {
                return Some(reason);
            }
        }
        None
    }

    /// Look up a tool as one scope sees it.
    pub fn get(&self, name: &str, scope: Option<&ScopeKey>) -> Option<Arc<ToolDefinition>> {
        if name == RUN_CODE_NAME && self.mode_for(scope) != ToolPresentationMode::Native {
            return Some(self.require_code_transport());
        }
        self.view(scope).visible.get(name).cloned()
    }

    pub(crate) fn code_runtime(&self) -> Result<Arc<dyn dsh_code_runtime::CodeRuntime>, String> {
        self.ctx
            .get_typed::<Arc<dyn dsh_code_runtime::CodeRuntime>>("codeRuntime", false)
            .map(|slot| slot.as_ref().clone())
            .ok_or_else(|| {
                "dsh-tools: code mode requires a code runtime — load a ctx.codeRuntime implementation or set tools mode to \"native\"".to_string()
            })
    }

    /// Project visible definitions onto the allowlisted model-facing schema
    /// fields.
    pub fn schemas(&self, scope: Option<&ScopeKey>) -> Vec<ToolSchema> {
        self.view(scope)
            .visible
            .values()
            .map(|definition| self.schema_of(definition))
            .collect()
    }

    /// Classify a pending call through the caller's visible tool definition.
    pub fn execution_mode(&self, input: &ToolExecutionInput) -> ToolExecutionMode {
        let Some(tool) =
            self.resolve_execution(&input.name, input.agent.as_ref(), input.parent.is_some())
        else {
            return ToolExecutionMode::Exclusive;
        };
        let Some(classifier) = &tool.is_concurrency_safe else {
            return ToolExecutionMode::Exclusive;
        };
        match catch_unwind(AssertUnwindSafe(|| classifier(&input.arguments))) {
            Ok(true) => ToolExecutionMode::Parallel,
            _ => ToolExecutionMode::Exclusive,
        }
    }

    /// Execute through pre-policy, guards, around-dispatch, post-policy,
    /// definition-owned content finalization, and final notification.
    pub async fn execute(self: &Arc<Self>, input: ToolExecutionInput) -> Arc<ToolExecutionResult> {
        match self.prepare_scheduled(input).await {
            Preparation::Dispatch { run_ctx } => {
                match self.dispatch_scheduled(Arc::clone(&run_ctx)).await {
                    DispatchOutcome::PostResult(result) => {
                        self.finalize_scheduled(run_ctx, result).await
                    }
                    DispatchOutcome::FinalResult(result) => self.finish_scheduled(run_ctx, result),
                }
            }
            Preparation::PostResult { run_ctx, result } => {
                self.finalize_scheduled(run_ctx, result).await
            }
            Preparation::FinalResult { run_ctx, result } => self.finish_scheduled(run_ctx, result),
        }
    }

    // ---- execution pipeline ----

    fn create_execution(&self, input: ToolExecutionInput) -> CreatedExecution {
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        let call_id = input.call_id.clone();
        let root_call_id = input.root_call_id.unwrap_or_else(|| input.call_id.clone());
        let name = input.name.clone();
        let agent = input.agent.clone();
        let parent = input.parent;
        let signal = input.signal;
        let visible = self.get(&name, agent.as_ref().map(|agent| agent.scope_key()));
        let collapsed =
            visible.is_some() && self.collapses(&name, agent.as_ref(), parent.is_some());
        // Capture the finalizer BEFORE anything else can replace it.
        let captured_finalizer = visible
            .as_ref()
            .and_then(|tool| tool.finalize_content.clone());
        let finalizer_for = if collapsed && signal() {
            None
        } else {
            captured_finalizer.clone()
        };
        let execution = Arc::new(ToolExecution {
            token,
            call_id,
            root_call_id,
            name: name.clone(),
            arguments: input.arguments,
            agent: agent.clone(),
            parent,
            signal: Mutex::new(signal.clone()),
        });
        let state = Arc::new(Mutex::new(ExecutionState {
            deferred: Vec::new(),
            concluded: false,
            body_invoked: false,
            caller_signal: signal,
            finalizer: finalizer_for.clone(),
        }));
        self.executions.lock().insert(token, Arc::clone(&state));
        let run_ctx = Arc::new(ToolRunContext {
            execution: Arc::clone(&execution),
            state,
        });
        if collapsed {
            // The collapse denies the call before the policy pipeline; a
            // pre-dispatch abort still keeps the cancellation contract.
            let signal = run_ctx.signal.lock().clone();
            if signal() {
                let result = Arc::new(tool_aborted_before_dispatch_result(None));
                return CreatedExecution::Final {
                    run_ctx: Arc::clone(&run_ctx),
                    result,
                };
            }
            let error = ToolNotFoundError::new(
                &name,
                Some(&format!(
                    "only `{RUN_CODE_NAME}` is callable directly — call `{name}` from inside a `{RUN_CODE_NAME}` program instead"
                )),
            );
            let result = tool_error_result(&error.to_string(), None);
            let result = Arc::new(self.mark_canonical(token, result));
            return CreatedExecution::Final { run_ctx, result };
        }
        CreatedExecution::Ready { run_ctx }
    }

    fn caller_cancelled(&self, run_ctx: &ToolRunContext) -> bool {
        let signal = run_ctx.state.lock().caller_signal.clone();
        signal()
    }

    fn cancellation_result(
        &self,
        run_ctx: &ToolRunContext,
        prior: Option<Arc<ToolExecutionResult>>,
    ) -> Arc<ToolExecutionResult> {
        let body_invoked = run_ctx.state.lock().body_invoked;
        if body_invoked {
            Arc::new(tool_aborted_result(prior.as_deref()))
        } else {
            Arc::new(tool_aborted_before_dispatch_result(prior.as_deref()))
        }
    }

    /// Materialize input, run the ordered pre-execute/guard gate, and decide
    /// what stage follows (the TS scheduler's `prepare` stage). The caller
    /// pairs it with [`ToolRuntime::dispatch_scheduled`] and
    /// [`ToolRuntime::finalize_scheduled`]/[`ToolRuntime::finish_scheduled`]
    /// for the parallel scheduler's overlapping dispatch.
    pub async fn prepare_scheduled(self: &Arc<Self>, input: ToolExecutionInput) -> Preparation {
        let created = self.create_execution(input);
        let run_ctx = match created {
            CreatedExecution::Final { run_ctx, result } => {
                return Preparation::FinalResult { run_ctx, result };
            }
            CreatedExecution::Ready { run_ctx } => run_ctx,
        };
        let scope_key = run_ctx
            .agent
            .as_ref()
            .map(|agent| agent.scope_key().clone());
        let outcome = AssertUnwindSafe(async {
            if self.caller_cancelled(&run_ctx) {
                return Preparation::FinalResult {
                    run_ctx: Arc::clone(&run_ctx),
                    result: Arc::new(tool_aborted_before_dispatch_result(None)),
                };
            }
            // tools/pre-execute waterfall, scope-filtered.
            let carrier = scope_target(None, scope_key.clone());
            let dispatch_ctx = self.ctx.with_filter(carrier.filter);
            let args = vec![arc(run_ctx.execution.clone())];
            let gate = dispatch_ctx
                .waterfall(
                    "tools/pre-execute",
                    args,
                    Box::pin(async { arc(PreToolDecision::Allow) }),
                )
                .await;
            let gate = downcast_arc::<PreToolDecision>(&gate)
                .unwrap_or_else(|| panic!("tools/pre-execute listener returned no decision"));
            let (decision, approval_cancelled) = match &*gate {
                PreToolDecision::Ask { reason } => self.service_ask(&run_ctx, reason.clone()).await,
                PreToolDecision::Allow => (PreToolDecision::Allow, false),
                PreToolDecision::Deny { reason } => (
                    PreToolDecision::Deny {
                        reason: reason.clone(),
                    },
                    false,
                ),
            };
            if self.caller_cancelled(&run_ctx) && approval_cancelled {
                return Preparation::PostResult {
                    run_ctx: Arc::clone(&run_ctx),
                    result: Arc::new(tool_aborted_before_dispatch_result(None)),
                };
            }
            let denial_reason = match &decision {
                PreToolDecision::Allow => self.guard_reason(&run_ctx.execution),
                PreToolDecision::Deny { reason } => Some(reason.clone()),
                PreToolDecision::Ask { .. } => None,
            };
            if let Some(reason) = denial_reason {
                let result = ToolExecutionResult {
                    content: vec![ContentBlock::Text {
                        text: format!("Error: {reason}"),
                    }],
                    is_error: true,
                    error: Some(ToolFailure {
                        message: reason,
                        info: None,
                    }),
                    value: None,
                    meta: None,
                    additional_contexts: Vec::new(),
                    concludes_turn: false,
                    canonical_token: 0,
                };
                return Preparation::PostResult {
                    run_ctx: Arc::clone(&run_ctx),
                    result: Arc::new(self.mark_canonical(run_ctx.token, result)),
                };
            }
            if self.caller_cancelled(&run_ctx) {
                return Preparation::PostResult {
                    run_ctx: Arc::clone(&run_ctx),
                    result: Arc::new(tool_aborted_before_dispatch_result(None)),
                };
            }
            Preparation::Dispatch {
                run_ctx: Arc::clone(&run_ctx),
            }
        })
        .catch_unwind()
        .await;
        match outcome {
            Ok(preparation) => preparation,
            Err(payload) => {
                let error = tool_error_from_panic(payload);
                let result = tool_error_result(&error.message, error.info.as_ref());
                let result = Arc::new(self.mark_canonical(run_ctx.token, result));
                Preparation::FinalResult { run_ctx, result }
            }
        }
    }

    /// Run only the around-dispatch/body stage (the TS scheduler's
    /// `dispatch` stage).
    pub async fn dispatch_scheduled(
        self: &Arc<Self>,
        run_ctx: Arc<ToolRunContext>,
    ) -> DispatchOutcome {
        let scope_key = run_ctx
            .agent
            .as_ref()
            .map(|agent| agent.scope_key().clone());
        let runtime = Arc::clone(self);
        let outcome = AssertUnwindSafe(async {
            let carrier = scope_target(None, scope_key);
            let dispatch_ctx = self.ctx.with_filter(carrier.filter);
            let args = vec![arc(run_ctx.execution.clone())];
            let value = dispatch_ctx
                .waterfall(
                    "tools/execute",
                    args,
                    Box::pin(Self::dispatch_tool_body(runtime, Arc::clone(&run_ctx))),
                )
                .await;
            let result = downcast_arc::<Arc<ToolExecutionResult>>(&value)
                .map(|arc| arc.as_ref().clone())
                .unwrap_or_else(|| panic!("tools/execute waterfall returned no result"));
            let normalized = self.normalize_dispatch_result(&run_ctx, result);
            let mut normalized = normalized;
            {
                let deferred = &mut run_ctx.state.lock().deferred;
                if !deferred.is_empty() {
                    let contexts = std::mem::take(deferred);
                    let mut merged = contexts;
                    merged.extend(normalized.additional_contexts.clone());
                    let rebuilt = ToolExecutionResult {
                        additional_contexts: merged,
                        ..clone_result(&normalized)
                    };
                    normalized = Arc::new(self.mark_canonical(run_ctx.token, rebuilt));
                }
            }
            let final_result = if self.caller_cancelled(&run_ctx) && !normalized.is_error {
                self.cancellation_result(&run_ctx, Some(Arc::clone(&normalized)))
            } else {
                Arc::clone(&normalized)
            };
            DispatchOutcome::PostResult(final_result)
        })
        .catch_unwind()
        .await;
        match outcome {
            Ok(outcome) => outcome,
            Err(payload) => {
                let error = tool_error_from_panic(payload);
                let result = self.mark_canonical(
                    run_ctx.token,
                    tool_error_result(&error.message, error.info.as_ref()),
                );
                DispatchOutcome::FinalResult(Arc::new(result))
            }
        }
    }

    /// Run post-execute and definition-owned content finalization, then
    /// materialize and notify (the TS scheduler's `finalize` stage).
    pub async fn finalize_scheduled(
        self: &Arc<Self>,
        run_ctx: Arc<ToolRunContext>,
        result: Arc<ToolExecutionResult>,
    ) -> Arc<ToolExecutionResult> {
        let scope_key = run_ctx
            .agent
            .as_ref()
            .map(|agent| agent.scope_key().clone());
        let run_ctx_for_error = Arc::clone(&run_ctx);
        let outcome = AssertUnwindSafe(async move {
            let carrier = scope_target(None, scope_key);
            let dispatch_ctx = self.ctx.with_filter(carrier.filter);
            let args = vec![arc(run_ctx.execution.clone()), arc(Arc::clone(&result))];
            let value = dispatch_ctx
                .waterfall(
                    "tools/post-execute",
                    args,
                    Box::pin(async {
                        arc(PostToolDecision::Accept {
                            content: None,
                            value: None,
                            additional_contexts: None,
                        })
                    }),
                )
                .await;
            let decision = downcast_arc::<PostToolDecision>(&value)
                .unwrap_or_else(|| panic!("tools/post-execute listener returned no decision"));
            let post = self.apply_post_decision(&run_ctx, &result, (*decision).clone());
            let final_result = if self.caller_cancelled(&run_ctx) && !post.is_error {
                self.cancellation_result(&run_ctx, Some(Arc::clone(&post)))
            } else {
                Arc::clone(&post)
            };
            self.finish_scheduled(run_ctx, final_result)
        })
        .catch_unwind()
        .await;
        match outcome {
            Ok(result) => result,
            Err(payload) => {
                let error = tool_error_from_panic(payload);
                let result = self.mark_canonical(
                    run_ctx_for_error.token,
                    tool_error_result(&error.message, error.info.as_ref()),
                );
                self.finish_scheduled(run_ctx_for_error, Arc::new(result))
            }
        }
    }

    /// Run definition-owned content finalization, then materialize and
    /// notify without post-execute (the TS scheduler's `finish` stage).
    pub fn finish_scheduled(
        &self,
        run_ctx: Arc<ToolRunContext>,
        result: Arc<ToolExecutionResult>,
    ) -> Arc<ToolExecutionResult> {
        // materializeFinalResult: Rust values are lossless and owned — identity.
        let final_result = {
            let finalizer = run_ctx.state.lock().finalizer.clone();
            match finalizer {
                Some(finalize_content) => match catch_unwind(AssertUnwindSafe(|| {
                    finalize_content(&run_ctx.execution, &result)
                })) {
                    Ok(Some(content)) => {
                        let rebuilt = ToolExecutionResult {
                            content,
                            ..clone_result(&result)
                        };
                        Arc::new(self.mark_canonical(run_ctx.token, rebuilt))
                    }
                    _ => Arc::clone(&result),
                },
                None => Arc::clone(&result),
            }
        };
        self.notify_result(&run_ctx, Arc::clone(&final_result));
        final_result
    }

    fn apply_post_decision(
        &self,
        run_ctx: &ToolRunContext,
        result: &Arc<ToolExecutionResult>,
        decision: PostToolDecision,
    ) -> Arc<ToolExecutionResult> {
        let decision_contexts = match &decision {
            PostToolDecision::Accept {
                additional_contexts,
                ..
            }
            | PostToolDecision::Block {
                additional_contexts,
                ..
            } => additional_contexts.clone().unwrap_or_default(),
        };
        if let PostToolDecision::Block { feedback, .. } = &decision {
            let message = failure_message_from_content(feedback);
            let rebuilt = ToolExecutionResult {
                content: feedback.clone(),
                is_error: true,
                error: Some(ToolFailure {
                    message,
                    info: None,
                }),
                value: None,
                meta: None,
                additional_contexts: if decision_contexts.is_empty() {
                    Vec::new()
                } else {
                    decision_contexts
                },
                concludes_turn: false,
                canonical_token: 0,
            };
            return Arc::new(self.mark_canonical(run_ctx.token, rebuilt));
        }
        let PostToolDecision::Accept { content, value, .. } = &decision else {
            unreachable!()
        };
        if content.is_some() && value.is_some() {
            panic!("tools/post-execute accept decision cannot replace both value and content");
        }
        let mut additional_contexts = result.additional_contexts.clone();
        additional_contexts.extend(decision_contexts);
        if let Some(value) = value {
            if result.is_error {
                panic!("tools/post-execute cannot replace the value of a failed result");
            }
            let Some(tool) = self.resolve_execution(
                &run_ctx.name,
                run_ctx.agent.as_ref(),
                run_ctx.parent.is_some(),
            ) else {
                let error = ToolNotFoundError::new(&run_ctx.name, None);
                let info = ToolErrorInfo {
                    name: "ToolNotFoundError".to_string(),
                    code: error.code().to_string(),
                };
                let mut failed = tool_error_result(&error.to_string(), Some(&info));
                failed.additional_contexts = additional_contexts;
                return Arc::new(self.mark_canonical(run_ctx.token, failed));
            };
            let replaced = self.create_success_result(run_ctx, &tool, value.clone());
            let rebuilt = ToolExecutionResult {
                additional_contexts: if additional_contexts.is_empty() {
                    replaced.additional_contexts.clone()
                } else {
                    additional_contexts
                },
                ..clone_result(&replaced)
            };
            return Arc::new(self.mark_canonical(run_ctx.token, rebuilt));
        }
        let rebuilt = ToolExecutionResult {
            content: content.clone().unwrap_or_else(|| result.content.clone()),
            additional_contexts: if additional_contexts.is_empty() {
                result.additional_contexts.clone()
            } else {
                additional_contexts
            },
            ..clone_result(result)
        };
        Arc::new(self.mark_canonical(run_ctx.token, rebuilt))
    }

    fn notify_result(&self, run_ctx: &ToolRunContext, result: Arc<ToolExecutionResult>) {
        let carrier = scope_target(
            None,
            run_ctx
                .agent
                .as_ref()
                .map(|agent| agent.scope_key().clone()),
        );
        let dispatch_ctx = self.ctx.with_filter(carrier.filter);
        let args = vec![arc(run_ctx.execution.clone()), arc(Arc::clone(&result))];
        let listeners = dispatch_ctx.collect(DispatchMode::Emit, "tools/result", &args);
        for (listener_ctx, listener) in listeners {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                futures::executor::block_on(listener(&listener_ctx, args.clone()));
            }));
            if let Err(payload) = outcome {
                let message = render_panic(payload);
                self.ctx.named_logger(Some("tools")).warn(vec![arc(format!(
                    "tool \"{}\" ({}): tools/result observer failed: {message}",
                    run_ctx.name,
                    run_ctx.call_id.as_str()
                ))]);
            }
        }
    }

    async fn service_ask(
        &self,
        run_ctx: &ToolRunContext,
        reason: Option<String>,
    ) -> (PreToolDecision, bool) {
        let Some(agent) = run_ctx.agent.clone() else {
            return (
                PreToolDecision::Deny {
                    reason: "approval requires an agent-owned tool call".to_string(),
                },
                false,
            );
        };
        let Some(approval) = self
            .ctx
            .get_typed::<Arc<dsh_user_approval::ApprovalService>>("approval", false)
            .map(|slot| slot.as_ref().clone())
        else {
            return (
                PreToolDecision::Deny {
                    reason: "approval service is unavailable".to_string(),
                },
                false,
            );
        };
        let signal = run_ctx.signal.lock().clone();
        let request = dsh_user_approval::ApprovalRequest {
            agent,
            tool_name: run_ctx.name.clone(),
            call_id: Some(run_ctx.call_id.as_str().to_string()),
            reason: reason.clone(),
            grant_key: Some(format!("tool:{}", run_ctx.name)),
            rememberable: true,
            signal: Some(signal),
        };
        match approval.request(&request).await {
            Ok(
                dsh_user_approval::ApprovalOutcome::AllowedOnce
                | dsh_user_approval::ApprovalOutcome::AllowedAlways,
            ) => (PreToolDecision::Allow, false),
            Ok(dsh_user_approval::ApprovalOutcome::Cancelled) => (
                PreToolDecision::Deny {
                    reason: "approval request was cancelled".to_string(),
                },
                true,
            ),
            Ok(outcome) => (
                PreToolDecision::Deny {
                    reason: reason.unwrap_or_else(|| {
                        format!("approval request resolved {}", outcome.as_str())
                    }),
                },
                false,
            ),
            Err(error) => (PreToolDecision::Deny { reason: error }, false),
        }
    }

    /// Run around-dispatch fallback: the registered body with the original
    /// caller signal fused back into any around-wrapper replacement.
    async fn dispatch_tool_body(self: Arc<Self>, run_ctx: Arc<ToolRunContext>) -> ArcValue {
        let wrapper_signal = run_ctx.signal.lock().clone();
        let caller = run_ctx.state.lock().caller_signal.clone();
        let fused: AbortPredicate = Arc::new(move || wrapper_signal() || caller());
        if fused() {
            return arc(Arc::new(tool_aborted_before_dispatch_result(None)));
        }
        *run_ctx.signal.lock() = fused.clone();
        let outcome = AssertUnwindSafe(async {
            let Some(tool) = self.resolve_execution(
                &run_ctx.name,
                run_ctx.agent.as_ref(),
                run_ctx.parent.is_some(),
            ) else {
                let error = ToolNotFoundError::new(&run_ctx.name, None);
                let info = ToolErrorInfo {
                    name: "ToolNotFoundError".to_string(),
                    code: error.code().to_string(),
                };
                return tool_error_result(&error.to_string(), Some(&info));
            };
            run_ctx.state.lock().body_invoked = true;
            let body = (tool.execute)(&run_ctx.arguments, &run_ctx);
            match AssertUnwindSafe(body).catch_unwind().await {
                Ok(Ok(value)) => {
                    let result = self.create_success_result(&run_ctx, &tool, value);
                    if fused() {
                        tool_aborted_result(Some(&result))
                    } else {
                        result
                    }
                }
                Ok(Err(error)) => tool_error_result(&error.message, error.info.as_ref()),
                Err(payload) => {
                    let error = tool_error_from_panic(payload);
                    tool_error_result(&error.message, error.info.as_ref())
                }
            }
        })
        .catch_unwind()
        .await;
        let result = match outcome {
            Ok(result) => result,
            Err(payload) => {
                let error = tool_error_from_panic(payload);
                tool_error_result(&error.message, error.info.as_ref())
            }
        };
        arc(Arc::new(result))
    }

    fn create_success_result(
        &self,
        run_ctx: &ToolRunContext,
        tool: &ToolDefinition,
        candidate: JsonValue,
    ) -> ToolExecutionResult {
        let violations = validate_json_schema_value(&tool.output.schema, &candidate, "value");
        if !violations.is_empty() {
            std::panic::panic_any(ToolOutputError {
                tool_name: tool.name.clone(),
                violations,
            });
        }
        let rendered = match (tool.output.render)(&run_ctx.arguments, &candidate) {
            Ok(content) => content,
            Err(message) => std::panic::panic_any(ToolOutputError {
                tool_name: tool.name.clone(),
                violations: vec![format!("output.render failed: {message}")],
            }),
        };
        let meta = if run_ctx.parent.is_none() {
            match &tool.output.presentation_meta {
                Some(projector) => match projector(&run_ctx.arguments, &candidate) {
                    Ok(meta) => Some(meta),
                    Err(message) => std::panic::panic_any(ToolOutputError {
                        tool_name: tool.name.clone(),
                        violations: vec![format!("output.presentationMeta failed: {message}")],
                    }),
                },
                None => None,
            }
        } else {
            None
        };
        let concludes_turn = run_ctx.state.lock().concluded;
        self.mark_canonical(
            run_ctx.token,
            ToolExecutionResult {
                is_error: false,
                error: None,
                value: Some(candidate),
                content: rendered,
                meta,
                additional_contexts: Vec::new(),
                concludes_turn,
                canonical_token: 0,
            },
        )
    }

    fn normalize_dispatch_result(
        &self,
        run_ctx: &ToolRunContext,
        result: Arc<ToolExecutionResult>,
    ) -> Arc<ToolExecutionResult> {
        if result.canonical(run_ctx.token) {
            return result;
        }
        if result.is_error {
            let rebuilt = ToolExecutionResult {
                is_error: true,
                error: result.error.clone(),
                content: result.content.clone(),
                value: None,
                meta: result.meta.clone(),
                additional_contexts: result.additional_contexts.clone(),
                concludes_turn: false,
                canonical_token: 0,
            };
            return Arc::new(self.mark_canonical(run_ctx.token, rebuilt));
        }
        let Some(tool) = self.resolve_execution(
            &run_ctx.name,
            run_ctx.agent.as_ref(),
            run_ctx.parent.is_some(),
        ) else {
            std::panic::panic_any(ToolNotFoundError::new(&run_ctx.name, None));
        };
        let normalized = self.create_success_result(
            run_ctx,
            &tool,
            result
                .value
                .clone()
                .expect("successful result carries a value"),
        );
        let rebuilt = ToolExecutionResult {
            additional_contexts: if result.additional_contexts.is_empty() {
                normalized.additional_contexts.clone()
            } else {
                result.additional_contexts.clone()
            },
            ..normalized
        };
        Arc::new(self.mark_canonical(run_ctx.token, rebuilt))
    }

    fn mark_canonical(&self, token: u64, result: ToolExecutionResult) -> ToolExecutionResult {
        ToolExecutionResult {
            canonical_token: token,
            ..result
        }
    }

    // ---- registry views ----

    fn restrictable_names(&self, scope: Option<&ScopeKey>) -> Vec<String> {
        self.view(scope).restrictable_names
    }

    /// Resolve the definition that MAY EXECUTE for a call, applying the
    /// mode collapse at the operation boundary that owns it.
    fn resolve_execution(
        &self,
        name: &str,
        agent: Option<&Arc<dyn Agent>>,
        nested: bool,
    ) -> Option<Arc<ToolDefinition>> {
        let tool = self.get(name, agent.map(|agent| agent.scope_key()))?;
        if self.collapses(name, agent, nested) {
            return None;
        }
        Some(tool)
    }

    /// Whether the `code` mode collapse denies a model-direct call.
    fn collapses(&self, name: &str, agent: Option<&Arc<dyn Agent>>, nested: bool) -> bool {
        !nested
            && self.mode_for(agent.map(|agent| agent.scope_key())) == ToolPresentationMode::Code
            && name != RUN_CODE_NAME
    }

    /// Build one scope's wire schemas and names for prompt-order validation.
    fn wire_schemas(&self, scope: Option<&ScopeKey>) -> ToolProviderResult {
        let view = self.view(scope);
        let mode = self.mode_for(scope);
        let mut schemas = view
            .visible
            .values()
            .map(|definition| self.schema_of(definition))
            .collect::<Vec<_>>();
        let mut known_names = view.known_names;
        if mode != ToolPresentationMode::Native {
            let transport = self.require_code_transport();
            let transport_schema = self.schema_of(&transport);
            if mode == ToolPresentationMode::Code {
                schemas.clear();
                known_names.clear();
            }
            schemas.push(transport_schema);
            known_names.push(RUN_CODE_NAME.to_string());
        }
        ToolProviderResult {
            schemas,
            known_names: Some(known_names),
        }
    }

    fn require_code_transport(&self) -> Arc<ToolDefinition> {
        let mut transport = self.code_transport.lock();
        transport
            .get_or_insert_with(|| {
                crate::code_mode::create_run_code_tool(Arc::downgrade(
                    &self
                        .ctx
                        .get_typed::<Arc<ToolRuntime>>("tools", false)
                        .expect("tools service is installed"),
                ))
            })
            .clone()
    }

    /// Project one definition onto the model-facing schema fields.
    fn schema_of(&self, definition: &ToolDefinition) -> ToolSchema {
        ToolSchema {
            name: definition.name.clone(),
            description: definition.description.clone(),
            parameters: definition.parameters.clone(),
        }
    }

    /// Resolve every registry fact one scope needs in one layer traversal.
    fn view(&self, scope: Option<&ScopeKey>) -> ToolView {
        let layers = self.layers.chain_layers(scope);
        let own = self.layers.peek(scope);
        let mut inherited: Vec<(String, Arc<ToolDefinition>)> = self.layers.global.tools.entries();
        for layer in &layers {
            if let Some(own) = &own
                && Arc::ptr_eq(layer, own)
            {
                continue;
            }
            for (name, definition) in layer.tools.entries() {
                if let Some(existing) = inherited.iter_mut().find(|(key, _)| *key == name) {
                    existing.1 = definition;
                } else {
                    inherited.push((name, definition));
                }
            }
        }
        let mut visible: Vec<(String, Arc<ToolDefinition>)> = Vec::new();
        let mut known_names = Vec::new();
        let mut restrictable_names = Vec::new();
        for (name, definition) in &inherited {
            known_names.push(name.clone());
            restrictable_names.push(name.clone());
            if layers.iter().all(|layer| layer.admits(name)) {
                visible.push((name.clone(), Arc::clone(definition)));
            }
        }
        if let Some(own) = &own {
            for (name, definition) in own.tools.entries() {
                known_names.push(name.clone());
                if let Some(existing) = visible.iter_mut().find(|(key, _)| *key == name) {
                    existing.1 = definition;
                } else {
                    visible.push((name, definition));
                }
            }
        }
        // The reserved run_code transport is inserted when a code runtime
        // milestone lands; native mode inserts nothing.
        ToolView {
            visible: visible.into_iter().collect(),
            known_names,
            restrictable_names,
        }
    }
}

struct ToolView {
    visible: HashMap<String, Arc<ToolDefinition>>,
    known_names: Vec<String>,
    restrictable_names: Vec<String>,
}

fn clone_result(result: &ToolExecutionResult) -> ToolExecutionResult {
    ToolExecutionResult {
        is_error: result.is_error,
        error: result.error.clone(),
        value: result.value.clone(),
        content: result.content.clone(),
        meta: result.meta.clone(),
        additional_contexts: result.additional_contexts.clone(),
        concludes_turn: result.concludes_turn,
        canonical_token: result.canonical_token,
    }
}

/// Derive one failure message from policy feedback without changing its
/// rendered blocks.
fn failure_message_from_content(content: &[ContentBlock]) -> String {
    let text = content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => text.clone(),
            other => format!("[{} content]", other.type_tag()),
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        "tool result blocked by post-execute policy".to_string()
    } else {
        text
    }
}

/// Normalize one thrown tool outcome into a failed result.
fn tool_error_result(message: &str, info: Option<&ToolErrorInfo>) -> ToolExecutionResult {
    ToolExecutionResult {
        content: vec![ContentBlock::Text {
            text: format!("Error: {message}"),
        }],
        is_error: true,
        error: Some(ToolFailure {
            message: message.to_string(),
            info: info.cloned(),
        }),
        value: None,
        meta: None,
        additional_contexts: Vec::new(),
        concludes_turn: false,
        canonical_token: 0,
    }
}

/// Canonical result when cancellation supersedes success after body
/// invocation.
fn tool_aborted_result(prior: Option<&ToolExecutionResult>) -> ToolExecutionResult {
    ToolExecutionResult {
        content: vec![ContentBlock::Text {
            text: "Error: tool call aborted".to_string(),
        }],
        is_error: true,
        error: Some(ToolFailure {
            message: "tool call aborted".to_string(),
            info: Some(ToolErrorInfo {
                name: "AbortError".to_string(),
                code: TOOL_ABORTED.to_string(),
            }),
        }),
        value: None,
        meta: None,
        additional_contexts: prior
            .map(|prior| prior.additional_contexts.clone())
            .unwrap_or_default(),
        concludes_turn: false,
        canonical_token: 0,
    }
}

/// Canonical result when cancellation prevents tool body invocation.
fn tool_aborted_before_dispatch_result(prior: Option<&ToolExecutionResult>) -> ToolExecutionResult {
    ToolExecutionResult {
        content: vec![ContentBlock::Text {
            text: "Error: tool call aborted before dispatch".to_string(),
        }],
        is_error: true,
        error: Some(ToolFailure {
            message: "tool call aborted before dispatch".to_string(),
            info: Some(ToolErrorInfo {
                name: "AbortError".to_string(),
                code: TOOL_ABORTED_BEFORE_DISPATCH.to_string(),
            }),
        }),
        value: None,
        meta: None,
        additional_contexts: prior
            .map(|prior| prior.additional_contexts.clone())
            .unwrap_or_default(),
        concludes_turn: false,
        canonical_token: 0,
    }
}

/// Render a panic payload to a string.
fn render_panic(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return message.to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "tool pipeline panicked".to_string()
}

/// Normalize one panic payload into the tool failure channel, retaining
/// the structured `{ name, code }` of registry-owned error classes (the TS
/// `errorInfo` read from thrown `HarnessError` instances).
fn tool_error_from_panic(payload: Box<dyn std::any::Any + Send>) -> ToolBodyError {
    match payload.downcast::<ToolOutputError>() {
        Ok(error) => {
            ToolBodyError::coded(error.to_string(), "ToolOutputError", "INVALID_TOOL_OUTPUT")
        }
        Err(payload) => match payload.downcast::<ToolNotFoundError>() {
            Ok(error) => {
                ToolBodyError::coded(error.to_string(), "ToolNotFoundError", "UNKNOWN_TOOL")
            }
            Err(payload) => ToolBodyError::plain(render_panic(payload)),
        },
    }
}

/// One registry's global + scoped presentation view. Kept private: the
/// scheduler contract (parallel dispatch staging) belongs to the
/// dsh-agent-loop milestone.
#[allow(dead_code)]
pub(crate) struct ToolRuntimeSchedulerMarker;
