//! Registry for ordered system sections, dynamic context, tool schemas, and
//! prompt variables. Rust port of `@deepseek-ai/dsh-system-prompt`.
//!
//! # Deviations
//!
//! - `AssembleContext` carries a `fields` bag instead of TS's open object
//!   type (merge-extensible context fields).
//! - `AssembleContext.signal` (the TS `AbortSignal`) has no wiring yet.
//! - `SystemPrompt::install` takes a validated [`Config`] value; config
//!   validation is manual for now ([`parse_config`]) with a schemastery
//!   schema available for loader integration ([`config_schema`]).
//! - `system-prompt/change` listeners run INLINE during registration and
//!   disposal so a throwing listener rolls the registration back (TS P1-1
//!   synchronous-emit contract), instead of the port's usual fire-and-forget
//!   emit.

pub mod invariant;

use std::collections::HashSet;
use std::sync::Arc;

use cordis::{Context, DispatchMode, Disposer, Service, arc, downcast};
use dsh_llm::{ContextSnapshotSection, ToolSchema};
use dsh_scope::{
    AnonymousEntries, NamedEntries, PreparedRegistration, ScopeKey, ScopeLayer, ScopedLayers,
    scope_target,
};
use indexmap::IndexMap;
use serde_json::{Map, Value as JsonValue};

/// Merge-extensible context for one prompt assembly.
#[derive(Debug, Clone, Default)]
pub struct AssembleContext {
    /// Scope whose providers and waterfall listeners participate. When
    /// absent, only global providers and subject-less listeners participate.
    pub scope: Option<ScopeKey>,
    /// Plugin-defined assembly fields (TS merge-extensible object members).
    pub fields: Map<String, JsonValue>,
}

impl AssembleContext {
    pub fn field(&self, name: &str) -> Option<&JsonValue> {
        self.fields.get(name)
    }

    pub fn field_str(&self, name: &str) -> Option<&str> {
        self.fields.get(name).and_then(|value| value.as_str())
    }
}

/// Static text or a provider evaluated at each assembly.
#[derive(Clone)]
pub enum PromptText {
    Static(String),
    Provider(Arc<dyn Fn(&AssembleContext) -> String + Send + Sync>),
}

impl PromptText {
    fn resolve(&self, context: &AssembleContext) -> String {
        match self {
            PromptText::Static(text) => text.clone(),
            PromptText::Provider(provider) => provider(context),
        }
    }
}

impl From<&str> for PromptText {
    fn from(text: &str) -> Self {
        PromptText::Static(text.to_string())
    }
}

impl From<String> for PromptText {
    fn from(text: String) -> Self {
        PromptText::Static(text)
    }
}

/// One contributed section of the system prompt (registry input).
#[derive(Clone)]
pub struct PromptSection {
    /// Unique name — a duplicate registration throws.
    pub name: String,
    /// Sections are concatenated in ascending order.
    pub order: f64,
    /// Static text or a provider evaluated at each assembly.
    pub text: PromptText,
    /// Treat this contribution as the complete system prompt.
    pub complete: Option<bool>,
}

/// Dynamic model context materialized as a durable user-role snapshot.
#[derive(Clone)]
pub struct PromptContext {
    /// Unique name — a duplicate registration throws.
    pub name: String,
    /// Contexts are joined in ascending order.
    pub order: f64,
    /// Static text or a provider evaluated for each assembly.
    pub text: PromptText,
}

/// One section of an assembly with its text resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct AssembledSection {
    pub name: String,
    /// The resolved (but not yet interpolated) section text.
    pub text: String,
}

/// One resolved dynamic context contribution.
#[derive(Debug, Clone, PartialEq)]
pub struct AssembledContext {
    pub name: String,
    /// The resolved text before variable interpolation.
    pub text: String,
}

/// Tool schemas visible in one assembly and their pre-restriction name set.
#[derive(Debug, Clone, Default)]
pub struct ToolProviderResult {
    /// The schemas this provider contributes to THIS assembly.
    pub schemas: Vec<ToolSchema>,
    /// The pre-restriction name universe (defaults to `schemas`' names).
    pub known_names: Option<Vec<String>>,
}

/// Merge-extensible assembled model input.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PromptAssembly {
    pub sections: Vec<AssembledSection>,
    pub contexts: Vec<AssembledContext>,
    pub tools: Vec<ToolSchema>,
    pub variables: IndexMap<String, Option<String>>,
}

/// The deployment persona's section name and order.
pub const PERSONA_SECTION: &str = "deployment:persona";

/// Prompt order of the persona slot; the first section a model reads.
pub const PERSONA_ORDER: f64 = 0.0;

/// Reserved [`Config::tool_order`] marker for unlisted tools.
pub const TOOL_ORDER_REST: &str = "<unlisted-tools>";

/// The built-in harness identity section text.
pub const HARNESS_IDENTITY: &str = "You are an AI agent powered by DeepSeek Harness.";

/// Valid variable names: how they are written between the braces.
pub(crate) fn is_valid_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

/// Validate duplicate names and the required [`TOOL_ORDER_REST`] marker.
fn validate_tool_order(tool_order: Option<Vec<String>>) -> Result<Option<Vec<String>>, String> {
    let Some(tool_order) = tool_order else {
        return Ok(None);
    };
    let mut seen = HashSet::new();
    for name in &tool_order {
        if !seen.insert(name.clone()) {
            return Err(format!("toolOrder lists \"{name}\" more than once"));
        }
    }
    if !seen.contains(TOOL_ORDER_REST) {
        return Err(format!(
            "toolOrder must contain the \"{TOOL_ORDER_REST}\" rest entry (where unlisted tools are inserted)"
        ));
    }
    Ok(Some(tool_order))
}

/// Apply configured tool order, inserting unlisted tools lexicographically
/// at [`TOOL_ORDER_REST`].
fn order_tools(
    mut tools: Vec<ToolSchema>,
    tool_order: Option<&Vec<String>>,
    known_names: &HashSet<String>,
) -> Result<Vec<ToolSchema>, String> {
    if tools.iter().any(|tool| tool.name == TOOL_ORDER_REST) {
        return Err(format!(
            "tool provider returned reserved tool name \"{TOOL_ORDER_REST}\" (reserved for toolOrder's rest entry)"
        ));
    }
    let Some(tool_order) = tool_order else {
        tools.sort_by(compare_tool_names);
        return Ok(tools);
    };
    let unknown: Vec<&String> = tool_order
        .iter()
        .filter(|name| name.as_str() != TOOL_ORDER_REST && !known_names.contains(*name))
        .collect();
    if !unknown.is_empty() {
        let rendered: Vec<String> = unknown.iter().map(|name| format!("\"{name}\"")).collect();
        let known: Vec<&str> = {
            let mut names: Vec<&str> = known_names.iter().map(String::as_str).collect();
            names.sort_unstable();
            names
        };
        return Err(format!(
            "toolOrder lists unregistered tool{} {}; known tools: {}",
            if unknown.len() > 1 { "s" } else { "" },
            rendered.join(", "),
            if known.is_empty() {
                "(none)".to_string()
            } else {
                known.join(", ")
            }
        ));
    }
    let listed: HashSet<&str> = tool_order.iter().map(String::as_str).collect();
    let mut rest: Vec<ToolSchema> = tools
        .iter()
        .filter(|tool| !listed.contains(tool.name.as_str()))
        .cloned()
        .collect();
    rest.sort_by(compare_tool_names);
    let mut ordered = Vec::new();
    for name in tool_order {
        if name == TOOL_ORDER_REST {
            ordered.extend(rest.clone());
        } else {
            ordered.extend(tools.iter().filter(|tool| &tool.name == name).cloned());
        }
    }
    Ok(ordered)
}

/// Lexicographic (code-unit) name comparison — locale-independent.
fn compare_tool_names(a: &ToolSchema, b: &ToolSchema) -> std::cmp::Ordering {
    a.name.cmp(&b.name)
}

/// Plugin config: the deployment-authored fragment of the system prompt.
#[derive(Debug, Clone)]
pub struct Config {
    /// Include the fixed DeepSeek Harness identity (default true).
    pub include_harness_identity: bool,
    /// Include dynamic runtime-context snapshots (default true).
    pub include_runtime_context: bool,
    /// Deployment-wide order-0 persona template.
    pub persona: String,
    /// Model-facing tool names in order, with [`TOOL_ORDER_REST`] exactly
    /// once.
    pub tool_order: Option<Vec<String>>,
}

impl Default for Config {
    /// The TS schema defaults: identity and runtime context are ON by
    /// default (a derived `bool` default of `false` would silently flip
    /// both).
    fn default() -> Self {
        Self {
            include_harness_identity: true,
            include_runtime_context: true,
            persona: String::new(),
            tool_order: None,
        }
    }
}

/// The schemastery config schema (TS `SystemPrompt.Config`), for loader
/// integration.
pub fn config_schema() -> schemastery::Schema {
    use schemastery::{Data, Schema};
    let mut dict = IndexMap::new();
    dict.insert(
        "includeHarnessIdentity".to_string(),
        Schema::boolean().default(Data::Bool(true)),
    );
    dict.insert(
        "includeRuntimeContext".to_string(),
        Schema::boolean().default(Data::Bool(true)),
    );
    dict.insert(
        "persona".to_string(),
        Schema::string().default(Data::String(String::new())),
    );
    dict.insert(
        "toolOrder".to_string(),
        Schema::array(Schema::string()).default(Data::Undefined),
    );
    Schema::object(dict)
}

/// Parse a loader-supplied JSON config with the schema's defaults and the
/// load-time `toolOrder` validation (manual port; unknown fields are
/// ignored — see the module deviation notes).
pub fn parse_config(value: &JsonValue) -> Result<Config, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "system-prompt config must be an object".to_string())?;
    let include_harness_identity = match object.get("includeHarnessIdentity") {
        None | Some(JsonValue::Null) => true,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| "includeHarnessIdentity must be a boolean".to_string())?,
    };
    let include_runtime_context = match object.get("includeRuntimeContext") {
        None | Some(JsonValue::Null) => true,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| "includeRuntimeContext must be a boolean".to_string())?,
    };
    let persona = match object.get("persona") {
        None | Some(JsonValue::Null) => String::new(),
        Some(value) => value
            .as_str()
            .ok_or_else(|| "persona must be a string".to_string())?
            .to_string(),
    };
    let tool_order = match object.get("toolOrder") {
        None => None,
        Some(value) => {
            let entries = value
                .as_array()
                .ok_or_else(|| "toolOrder must be an array of strings".to_string())?;
            let names = entries
                .iter()
                .map(|entry| {
                    entry
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| "toolOrder must be an array of strings".to_string())
                })
                .collect::<Result<Vec<String>, String>>()?;
            Some(names)
        }
    };
    let tool_order = validate_tool_order(tool_order)?;
    Ok(Config {
        include_harness_identity,
        include_runtime_context,
        persona,
        tool_order,
    })
}

/// Interpolate strict `{{variable}}` references, drop empty sections, and
/// join the rest with blank lines (TS `renderPrompt`).
pub fn render_prompt(assembly: &PromptAssembly) -> Result<String, String> {
    let rendered = assembly
        .sections
        .iter()
        .map(|section| interpolate(&section.name, &section.text, &assembly.variables, "section"))
        .collect::<Result<Vec<String>, String>>()?;
    Ok(rendered
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<String>>()
        .join("\n\n"))
}

/// Render the complete dynamic context snapshot (TS
/// `renderContextSnapshot`).
pub fn render_context_snapshot(assembly: &PromptAssembly) -> Result<String, String> {
    Ok(join_context_sections(&render_context_sections(assembly)?))
}

/// The model-facing snapshot text for an already-rendered section list
/// (TS `joinContextSections`).
pub fn join_context_sections(sections: &[ContextSnapshotSection]) -> String {
    let body = sections
        .iter()
        .map(|section| section.text.as_str())
        .collect::<Vec<&str>>()
        .join("\n\n");
    if body.is_empty() {
        return String::new();
    }
    format!(
        "Current runtime context. This snapshot supersedes earlier runtime-context snapshots.\n\n{body}"
    )
}

/// The snapshot as the named contributions it was assembled from
/// (TS `renderContextSections`).
pub fn render_context_sections(
    assembly: &PromptAssembly,
) -> Result<Vec<ContextSnapshotSection>, String> {
    assembly
        .contexts
        .iter()
        .map(|context| {
            Ok(ContextSnapshotSection {
                name: context.name.clone(),
                text: interpolate(&context.name, &context.text, &assembly.variables, "context")?,
            })
        })
        .filter(
            |section: &Result<ContextSnapshotSection, String>| match section {
                Ok(section) => !section.text.is_empty(),
                Err(_) => true,
            },
        )
        .collect()
}

/// Interpolate one section or context and attribute diagnostics to its
/// owning input (TS `interpolate`).
fn interpolate(
    name: &str,
    text: &str,
    variables: &IndexMap<String, Option<String>>,
    kind: &str,
) -> Result<String, String> {
    let mut result = String::new();
    let mut last = 0usize;
    while let Some(open) = text[last..].find("{{").map(|offset| last + offset) {
        let rest = &text[open..];
        let matched: Option<(&str, usize)> = rest.strip_prefix("{{").and_then(|tail| {
            tail.find("}}").and_then(|close_rel| {
                let candidate = &tail[..close_rel];
                if candidate.contains('{') || candidate.contains('}') {
                    None
                } else {
                    Some((candidate, 2 + close_rel + 2))
                }
            })
        });
        match matched {
            None => {
                // A later closing brace makes this malformed; otherwise it
                // is literal prose.
                if text[open + 2..].contains("}}") {
                    let preview: String = text[open..].chars().take(16).collect();
                    return Err(format!(
                        "malformed prompt variable reference at \"{preview}…\" in {kind} \"{name}\" (references are complete simple {{{{name}}}} groups)"
                    ));
                }
                result.push_str(&text[last..open + 2]);
                last = open + 2;
            }
            Some((variable, total_len)) => {
                if !is_valid_variable_name(variable) {
                    return Err(format!(
                        "malformed prompt variable reference \"{{{{{variable}}}}}\" in {kind} \"{name}\" (variable names match ^[a-z][a-z0-9_]*$)"
                    ));
                }
                if !variables.contains_key(variable) {
                    let known: Vec<&str> = variables.keys().map(String::as_str).collect();
                    let list = if known.is_empty() {
                        "(none)".to_string()
                    } else {
                        known.join(", ")
                    };
                    return Err(format!(
                        "unknown prompt variable \"{{{{{variable}}}}}\" in {kind} \"{name}\"; registered variables: {list}"
                    ));
                }
                match variables.get(variable) {
                    Some(Some(value)) => {
                        result.push_str(&text[last..open]);
                        result.push_str(value);
                        last = open + total_len;
                    }
                    _ => {
                        return Err(format!(
                            "prompt variable \"{{{{{variable}}}}}\" has no value for this assembly ({kind} \"{name}\")"
                        ));
                    }
                }
            }
        }
    }
    result.push_str(&text[last..]);
    Ok(result)
}

/// One tool-schema provider stored in a prompt layer.
pub type ToolProvider = Arc<dyn Fn(&AssembleContext) -> ToolProviderResult + Send + Sync>;

/// One prompt-variable provider stored in a prompt layer.
pub type VariableProvider = Arc<dyn Fn(&AssembleContext) -> Option<String> + Send + Sync>;

/// All prompt registrations owned by one global or scoped layer.
pub struct PromptLayer {
    pub sections: NamedEntries<PromptSection>,
    pub contexts: NamedEntries<PromptContext>,
    pub runtime_context_suppressors: AnonymousEntries<bool>,
    pub tool_providers: AnonymousEntries<ToolProvider>,
    pub variables: NamedEntries<VariableProvider>,
}

impl PromptLayer {
    /// Create one prompt layer with diagnostics specific to its ownership
    /// scope.
    pub fn new(scope: Option<&ScopeKey>) -> Self {
        let scoped = scope.is_some();
        Self {
            sections: NamedEntries::new(move |name| duplicate_error("section", name, scoped)),
            contexts: NamedEntries::new(move |name| duplicate_error("context", name, scoped)),
            runtime_context_suppressors: AnonymousEntries::new(),
            tool_providers: AnonymousEntries::new(),
            variables: NamedEntries::new(move |name| duplicate_error("variable", name, scoped)),
        }
    }
}

impl ScopeLayer for PromptLayer {
    fn is_empty(&self) -> bool {
        self.sections.is_empty()
            && self.contexts.is_empty()
            && self.runtime_context_suppressors.is_empty()
            && self.tool_providers.is_empty()
            && self.variables.is_empty()
    }
}

fn duplicate_error(
    kind: &str,
    name: &str,
    scoped: bool,
) -> Box<dyn std::error::Error + Send + Sync> {
    let message = if scoped {
        format!("prompt {kind} \"{name}\" is already registered in this scope")
    } else {
        format!(
            "prompt {kind} \"{name}\" is already registered (for a per-agent override, register through that agent's `agent.ctx` instead)"
        )
    };
    Box::<dyn std::error::Error + Send + Sync>::from(message)
}

/// The assembly handle that travels through the `system-prompt/assemble`
/// waterfall: TS listeners mutate the SAME assembly object in place, so the
/// port shares one lock-guarded value instead of passing owned snapshots.
#[derive(Clone)]
pub struct SharedAssembly(pub Arc<parking_lot::Mutex<PromptAssembly>>);

impl SharedAssembly {
    pub fn new(assembly: PromptAssembly) -> Self {
        Self(Arc::new(parking_lot::Mutex::new(assembly)))
    }

    pub fn snapshot(&self) -> PromptAssembly {
        self.0.lock().clone()
    }
}

/// Emit `system-prompt/change` with the TS synchronous-throw contract:
/// listeners run inline, in order; the first panic aborts the emit and
/// propagates (registration/disposal rollback depends on it).
fn emit_change_inline(ctx: &Context) {
    let listeners = ctx
        .events
        .collect(DispatchMode::Emit, Some(ctx), "system-prompt/change", &[]);
    for (listener_ctx, callback) in &listeners {
        let future = callback(listener_ctx, Vec::new());
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            futures::executor::block_on(future)
        })) {
            Ok(_) => {}
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
}

/// Registry service for the prompt inputs assembled before each model step.
pub struct SystemPrompt {
    pub ctx: Context,
    layers: ScopedLayers<PromptLayer>,
    tool_order: Option<Vec<String>>,
}

impl SystemPrompt {
    /// Create the service, register it as `systemPrompt`, and install the
    /// built-in sections (TS `SystemPrompt` constructor + registration).
    pub fn install(ctx: &Context, config: Config) -> Result<Arc<Self>, String> {
        let tool_order = validate_tool_order(config.tool_order)?;
        let change_ctx = ctx.clone();
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            layers: ScopedLayers::new(PromptLayer::new, move || emit_change_inline(&change_ctx)),
            tool_order,
        });
        ctx.register_service(service.clone());
        // Keep harness-owned openers independent of the selected loop
        // plugin.
        if config.include_harness_identity {
            service.section(
                ctx,
                PromptSection {
                    name: "harness:identity".to_string(),
                    order: -100.0,
                    text: PromptText::Static(HARNESS_IDENTITY.to_string()),
                    complete: None,
                },
            );
        }
        service.section(
            ctx,
            PromptSection {
                name: PERSONA_SECTION.to_string(),
                order: PERSONA_ORDER,
                text: PromptText::Static(config.persona),
                complete: None,
            },
        );
        if !config.include_runtime_context {
            service.suppress_runtime_context(ctx);
        }
        Ok(service)
    }

    /// Register an ordered prompt section in the calling context's scope
    /// (TS `SystemPrompt.section`). The effect is owned by the CALLER's
    /// fiber — the TS `this.ctx` proxy rebinding contracts onto the
    /// explicit `caller` parameter.
    pub fn section(&self, caller: &Context, section: PromptSection) -> Disposer {
        self.prepare_section(caller, section).commit(caller)
    }

    /// Prepare one section synchronously without binding it to the caller's
    /// fiber yet. Dropping the handle rolls the insertion back.
    pub fn prepare_section(
        &self,
        caller: &Context,
        section: PromptSection,
    ) -> PreparedRegistration {
        self.try_prepare_section(caller, section)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    /// Prepare one section with a normal duplicate error for cross-registry
    /// transactions; listener panics retain the synchronous emit contract.
    pub fn try_prepare_section(
        &self,
        caller: &Context,
        section: PromptSection,
    ) -> Result<PreparedRegistration, String> {
        if !section.order.is_finite() {
            return Err(format!(
                "prompt section \"{}\" order must be a finite number",
                section.name
            ));
        }
        self.layers.try_prepare_named(
            caller,
            |layer| &layer.sections,
            section.name.clone(),
            section,
            "systemPrompt.section()",
            true,
        )
    }

    /// Register ordered dynamic context in the calling context's scope
    /// (TS `SystemPrompt.context`).
    pub fn context(&self, caller: &Context, context: PromptContext) -> Disposer {
        if !context.order.is_finite() {
            panic!(
                "prompt context \"{}\" order must be a finite number",
                context.name
            );
        }
        self.layers.effect(
            caller,
            move |layer| layer.contexts.insert(&context.name, context.clone()),
            "systemPrompt.context()",
            true,
        )
    }

    /// Suppress every dynamic runtime-context contribution in the calling
    /// context's scope (TS `SystemPrompt.suppressRuntimeContext`).
    pub fn suppress_runtime_context(&self, caller: &Context) -> Disposer {
        self.layers.effect(
            caller,
            |layer| layer.runtime_context_suppressors.append(true),
            "systemPrompt.suppressRuntimeContext()",
            true,
        )
    }

    /// Register a tool-schema provider in the calling context's scope
    /// (TS `SystemPrompt.tools`).
    pub fn tools(&self, caller: &Context, provider: ToolProvider) -> Disposer {
        self.layers.effect(
            caller,
            move |layer| layer.tool_providers.append(provider.clone()),
            "systemPrompt.tools()",
            true,
        )
    }

    /// Register a prompt variable in the calling context's scope
    /// (TS `SystemPrompt.variable`).
    pub fn variable(&self, caller: &Context, name: &str, provider: VariableProvider) -> Disposer {
        if !is_valid_variable_name(name) {
            panic!("invalid prompt variable name \"{name}\" (must match ^[a-z][a-z0-9_]*$)");
        }
        let name = name.to_string();
        self.layers.effect(
            caller,
            move |layer| layer.variables.insert(&name, provider.clone()),
            "systemPrompt.variable()",
            true,
        )
    }

    /// Assemble global and scoped providers, detach tool parameters, apply
    /// canonical ordering, then run the assembly waterfall (TS
    /// `SystemPrompt.assemble`). `caller` is the accessing context (the TS
    /// `this.ctx` rebinding): the waterfall dispatches from it.
    pub async fn assemble(
        &self,
        caller: &Context,
        context: &AssembleContext,
    ) -> Result<PromptAssembly, String> {
        let scope = context.scope.clone();
        let scope_layers = self.layers.chain_layers(scope.as_ref());
        let runtime_context_suppressed = !self.layers.global.runtime_context_suppressors.is_empty()
            || scope_layers
                .iter()
                .any(|layer| !layer.runtime_context_suppressors.is_empty());

        // Scoped variables shadow globals. LIVE iteration (TS `Map.entries()`)
        // so a provider may register a variable that participates in the same
        // assembly.
        let mut variables: IndexMap<String, Option<String>> = IndexMap::new();
        live_entries(&self.layers.global.variables, |name, provider| {
            variables.insert(name, provider(context));
        });
        for layer in &scope_layers {
            live_entries(&layer.variables, |name, provider| {
                variables.insert(name, provider(context));
            });
        }

        // Scoped sections/contexts shadow globals (snapshots).
        let section_by_name = self.layers.merge(scope.as_ref(), |layer| &layer.sections);
        let context_by_name = self.layers.merge(scope.as_ref(), |layer| &layer.contexts);

        // Snapshot tool-provider membership BEFORE evaluating any provider.
        let mut providers: Vec<ToolProvider> = self.layers.global.tool_providers.values();
        for layer in &scope_layers {
            providers.extend(layer.tool_providers.values());
        }
        let mut collected: Vec<ToolSchema> = Vec::new();
        let mut known_names: HashSet<String> = HashSet::new();
        for provider in &providers {
            let result = provider(context);
            let schemas: Vec<ToolSchema> = result
                .schemas
                .iter()
                .map(|tool| ToolSchema {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    // Detach (TS `structuredClone(parameters)`).
                    parameters: tool.parameters.clone(),
                })
                .collect();
            let accepted = result
                .known_names
                .unwrap_or_else(|| schemas.iter().map(|tool| tool.name.clone()).collect());
            collected.extend(schemas);
            for name in accepted {
                known_names.insert(name);
            }
        }

        let mut section_definitions: Vec<&PromptSection> = section_by_name.values().collect();
        section_definitions.sort_by(|a, b| {
            a.order
                .partial_cmp(&b.order)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let complete_sections: Vec<&PromptSection> = section_definitions
            .iter()
            .copied()
            .filter(|section| section.complete == Some(true))
            .collect();
        if complete_sections.len() > 1 {
            let names = complete_sections
                .iter()
                .map(|section| serde_json::to_string(&section.name).unwrap_or_default())
                .collect::<Vec<String>>()
                .join(", ");
            return Err(format!(
                "multiple complete prompt sections are active: {names}"
            ));
        }
        let mut complete_section: Option<AssembledSection> = None;
        let sections: Vec<AssembledSection> = section_definitions
            .into_iter()
            .map(|section| {
                let assembled = AssembledSection {
                    name: section.name.clone(),
                    text: section.text.resolve(context),
                };
                if section.complete == Some(true) {
                    complete_section = Some(assembled.clone());
                }
                assembled
            })
            .collect();
        let contexts: Vec<AssembledContext> = if runtime_context_suppressed {
            Vec::new()
        } else {
            let mut entries: Vec<&PromptContext> = context_by_name.values().collect();
            entries.sort_by(|a, b| {
                a.order
                    .partial_cmp(&b.order)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            entries
                .into_iter()
                .map(|entry| AssembledContext {
                    name: entry.name.clone(),
                    text: entry.text.resolve(context),
                })
                .collect()
        };
        let assembly = PromptAssembly {
            sections,
            contexts,
            tools: order_tools(collected, self.tool_order.as_ref(), &known_names)?,
            variables,
        };

        // Expert waterfall over the assembled sections, contexts, tools, and
        // variables. Listeners share the one mutable assembly; the returned
        // value is authoritative.
        let carrier = scope_target(None, scope);
        let dispatch_ctx = caller.with_filter(carrier.filter);
        let shared = SharedAssembly::new(assembly);
        let value = dispatch_ctx
            .waterfall(
                "system-prompt/assemble",
                vec![arc(shared.clone()), arc(context.clone())],
                Box::pin(async move { arc(shared) }),
            )
            .await;
        let transformed = downcast::<SharedAssembly>(&value)
            .expect("system-prompt/assemble waterfall must resolve a PromptAssembly")
            .snapshot();

        if complete_section.is_none() && !runtime_context_suppressed {
            return Ok(transformed);
        }
        Ok(PromptAssembly {
            sections: match &complete_section {
                Some(section) => vec![section.clone()],
                None => transformed.sections,
            },
            contexts: if runtime_context_suppressed {
                Vec::new()
            } else {
                transformed.contexts
            },
            tools: transformed.tools,
            variables: transformed.variables,
        })
    }
}

/// Position-based live iteration over [`NamedEntries`]: entries appended
/// while `f` processes an earlier entry are visited in the same pass (the
/// TS live `Map.entries()` contract). No lock is held across `f`.
fn live_entries<V: Clone + Send + Sync + 'static>(
    entries: &NamedEntries<V>,
    mut f: impl FnMut(String, V),
) {
    let mut position = 0usize;
    loop {
        let item = entries.get_index(position);
        match item {
            Some((name, value)) => {
                position += 1;
                f(name, value);
            }
            None => break,
        }
    }
}

impl Service for SystemPrompt {
    fn service_name(&self) -> &'static str {
        "systemPrompt"
    }
}
