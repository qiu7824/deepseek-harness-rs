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

use cordis::{Context, Disposer, DispatchMode, Service, arc, downcast};
use dsh_llm::{ContextSnapshotSection, ToolSchema};
use dsh_scope::{
    AnonymousEntries, NamedEntries, ScopeKey, ScopeLayer, ScopedLayers, scope_target,
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
            if known.is_empty() { "(none)".to_string() } else { known.join(", ") }
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
    Ok(Config { include_harness_identity, include_runtime_context, persona, tool_order })
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
    format!("Current runtime context. This snapshot supersedes earlier runtime-context snapshots.\n\n{body}")
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
        .filter(|section: &Result<ContextSnapshotSection, String>| match section {
            Ok(section) => !section.text.is_empty(),
            Err(_) => true,
        })
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
            sections: NamedEntries::new(move |name| {
                duplicate_error("section", name, scoped)
            }),
            contexts: NamedEntries::new(move |name| {
                duplicate_error("context", name, scoped)
            }),
            runtime_context_suppressors: AnonymousEntries::new(),
            tool_providers: AnonymousEntries::new(),
            variables: NamedEntries::new(move |name| {
                duplicate_error("variable", name, scoped)
            }),
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

fn duplicate_error(kind: &str, name: &str, scoped: bool) -> Box<dyn std::error::Error + Send + Sync> {
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
            layers: ScopedLayers::new(
                |scope| PromptLayer::new(scope),
                move || emit_change_inline(&change_ctx),
            ),
            tool_order,
        });
        ctx.register_service(service.clone());
        // Keep harness-owned openers independent of the selected loop
        // plugin.
        if config.include_harness_identity {
            service.section(ctx, PromptSection {
                name: "harness:identity".to_string(),
                order: -100.0,
                text: PromptText::Static(HARNESS_IDENTITY.to_string()),
                complete: None,
            });
        }
        service.section(ctx, PromptSection {
            name: PERSONA_SECTION.to_string(),
            order: PERSONA_ORDER,
            text: PromptText::Static(config.persona),
            complete: None,
        });
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
        if !section.order.is_finite() {
            panic!("prompt section \"{}\" order must be a finite number", section.name);
        }
        self.layers.effect(
            caller,
            move |layer| layer.sections.insert(&section.name, section.clone()),
            "systemPrompt.section()",
            true,
        )
    }

    /// Register ordered dynamic context in the calling context's scope
    /// (TS `SystemPrompt.context`).
    pub fn context(&self, caller: &Context, context: PromptContext) -> Disposer {
        if !context.order.is_finite() {
            panic!("prompt context \"{}\" order must be a finite number", context.name);
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
    pub async fn assemble(&self, caller: &Context, context: &AssembleContext) -> Result<PromptAssembly, String> {
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
            return Err(format!("multiple complete prompt sections are active: {names}"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use cordis::{Context, EventOptions, NextFn};
    use std::sync::atomic::{AtomicU32, Ordering as MemOrder};

    const BUILT_IN: [&str; 2] = ["harness:identity", "deployment:persona"];
    const IDENTITY: &str = "You are an AI agent powered by DeepSeek Harness.";

    fn contributed(assembly: &PromptAssembly) -> Vec<&AssembledSection> {
        assembly
            .sections
            .iter()
            .filter(|section| !BUILT_IN.contains(&section.name.as_str()))
            .collect()
    }

    fn default_config() -> Config {
        Config::default()
    }

    fn install(ctx: &Context, config: Config) -> Arc<SystemPrompt> {
        SystemPrompt::install(ctx, config).unwrap()
    }

    fn tool(name: &str, description: &str) -> ToolSchema {
        ToolSchema {
            name: name.to_string(),
            description: description.to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    fn names(assembly: &PromptAssembly) -> Vec<String> {
        assembly.tools.iter().map(|tool| tool.name.clone()).collect()
    }

    #[tokio::test]
    async fn scoped_sections_shadow_globals_for_that_scope_only() {
        use dsh_scope::{CreateScopeOptions, ScopeKey, create_scope};

        let ctx = Context::root();
        let service = install(&ctx, default_config());
        service.section(&ctx, PromptSection {
            name: "policy".into(),
            order: 10.0,
            text: "global policy".into(),
            complete: None,
        });

        let key = ScopeKey::new();
        let scope = create_scope(&ctx, key.clone(), &CreateScopeOptions::default());
        service.section(&scope.ctx, PromptSection {
            name: "policy".into(),
            order: 10.0,
            text: "agent policy".into(),
            complete: None,
        });

        // Without the scope: the global text.
        let global = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        let policy = global.sections.iter().find(|s| s.name == "policy").unwrap();
        assert_eq!(policy.text, "global policy");

        // With the scope: the scoped text shadows the global one.
        let mut scoped_context = AssembleContext::default();
        scoped_context.scope = Some(key.clone());
        let scoped = service.assemble(&ctx, &scoped_context).await.unwrap();
        let policy = scoped.sections.iter().find(|s| s.name == "policy").unwrap();
        assert_eq!(policy.text, "agent policy");

        (scope.dispose)().await;
        // After the scope disposes, the shadow is gone.
        let global = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        let policy = global.sections.iter().find(|s| s.name == "policy").unwrap();
        assert_eq!(policy.text, "global policy");
    }

    #[tokio::test]
    async fn built_in_persona_name_is_reserved() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            service.section(&ctx, PromptSection {
                name: PERSONA_SECTION.into(),
                order: 0.0,
                text: "imposter".into(),
                complete: None,
            })
        }));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn built_in_sections_and_persona() {
        let ctx = Context::root();
        let service = install(&ctx, Config { persona: "You are DeepSeek Harness.".into(), ..default_config() });
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(
            assembly.sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["harness:identity", "deployment:persona"]
        );
        assert_eq!(
            render_prompt(&assembly).unwrap(),
            format!("{IDENTITY}\n\nYou are DeepSeek Harness.")
        );
        // Built-in names are reserved.
        let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            service.section(&ctx, PromptSection {
                name: "deployment:persona".into(),
                order: 0.0,
                text: "imposter".into(),
                complete: None,
            })
        }));
        assert!(duplicate.is_err());

        // persona-less deployment renders only the identity
        let ctx2 = Context::root();
        let service2 = install(&ctx2, default_config());
        let assembly2 = service2.assemble(&ctx2, &AssembleContext::default()).await.unwrap();
        assert_eq!(render_prompt(&assembly2).unwrap(), IDENTITY);
    }

    #[tokio::test]
    async fn omit_identity_and_suppress_runtime_context() {
        let ctx = Context::root();
        let service = install(
            &ctx,
            Config {
                include_harness_identity: false,
                persona: "You are a helpful software engineer assistant.".into(),
                ..default_config()
            },
        );
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(
            assembly.sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["deployment:persona"]
        );

        // includeRuntimeContext: false — providers not evaluated, waterfall
        // additions dropped.
        let ctx2 = Context::root();
        let service2 = install(&ctx2, Config { include_runtime_context: false, ..default_config() });
        let provider_calls = Arc::new(AtomicU32::new(0));
        let calls = provider_calls.clone();
        service2.context(&ctx2, PromptContext {
            name: "policy".into(),
            order: 0.0,
            text: PromptText::Provider(Arc::new(move |_ctx| {
                calls.fetch_add(1, MemOrder::SeqCst);
                format!("policy {}", calls.load(MemOrder::SeqCst))
            })),
        });
        ctx2.on(
            "system-prompt/assemble",
            Arc::new(|_ctx, args| {
                Box::pin(async move {
                    let shared = downcast::<SharedAssembly>(&args[0]).expect("assembly arg");
                    shared.0.lock().contexts.push(AssembledContext {
                        name: "late".into(),
                        text: "late context".into(),
                    });
                    let next = downcast::<NextFn>(&args[2]).expect("next");
                    Some(next.call().await)
                })
            }),
            EventOptions::default(),
        )
        .await;
        let assembly2 = service2.assemble(&ctx2, &AssembleContext::default()).await.unwrap();
        assert!(assembly2.contexts.is_empty());
        assert_eq!(provider_calls.load(MemOrder::SeqCst), 0);
    }

    #[tokio::test]
    async fn assembles_in_order_with_resolved_text_and_tools() {
        let ctx = Context::root();
        let service = install(&ctx, Config { persona: "You are DeepSeek Harness.".into(), ..default_config() });

        service.section(&ctx, PromptSection {
            name: "cwd".into(),
            order: 20.0,
            text: PromptText::Provider(Arc::new(|_ctx| "cwd: /tmp".to_string())),
            complete: None,
        });
        service.section(&ctx, PromptSection {
            name: "rules".into(),
            order: 10.0,
            text: "Be precise.".into(),
            complete: None,
        });
        service.context(&ctx, PromptContext {
            name: "later".into(),
            order: 20.0,
            text: PromptText::Provider(Arc::new(|_ctx| "context 2".to_string())),
        });
        service.context(&ctx, PromptContext {
            name: "earlier".into(),
            order: 10.0,
            text: "context 1".into(),
        });
        service.tools(&ctx, Arc::new(|_ctx| ToolProviderResult {
            schemas: vec![tool("echo", "echo back")],
            known_names: None,
        }));

        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(
            assembly.sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["harness:identity", "deployment:persona", "rules", "cwd"]
        );
        assert_eq!(
            assembly.sections.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
            vec![IDENTITY, "You are DeepSeek Harness.", "Be precise.", "cwd: /tmp"]
        );
        assert_eq!(
            assembly.contexts,
            vec![
                AssembledContext { name: "earlier".into(), text: "context 1".into() },
                AssembledContext { name: "later".into(), text: "context 2".into() },
            ]
        );
        assert_eq!(assembly.tools, vec![tool("echo", "echo back")]);
        assert!(assembly.variables.is_empty());
        assert_eq!(
            render_prompt(&assembly).unwrap(),
            format!("{IDENTITY}\n\nYou are DeepSeek Harness.\n\nBe precise.\n\ncwd: /tmp")
        );
        assert_eq!(
            render_context_snapshot(&assembly).unwrap(),
            "Current runtime context. This snapshot supersedes earlier runtime-context snapshots.\n\ncontext 1\n\ncontext 2"
        );
    }

    #[tokio::test]
    async fn resolves_text_providers_against_context_per_call() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_provider = calls.clone();
        service.section(&ctx, PromptSection {
            name: "dynamic".into(),
            order: 0.0,
            text: PromptText::Provider(Arc::new(move |context| {
                let call = calls_for_provider.fetch_add(1, MemOrder::SeqCst) + 1;
                format!(
                    "call {call} for {}",
                    context.field_str("who").unwrap_or("nobody")
                )
            })),
            complete: None,
        });

        let mut context = AssembleContext::default();
        context.fields.insert("who".into(), serde_json::json!("alice"));
        let first = service.assemble(&ctx, &context).await.unwrap();
        assert_eq!(contributed(&first)[0].text, "call 1 for alice");
        let second = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(contributed(&second)[0].text, "call 2 for nobody");
    }

    #[tokio::test]
    async fn fiber_disposal_removes_contributions() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());

        struct Contributor;
        #[async_trait::async_trait]
        impl cordis::Plugin for Contributor {
            fn inject(&self) -> cordis::InjectSpec {
                cordis::InjectSpec::new(["systemPrompt"])
            }

            async fn apply(&self, ctx: &Context, _config: cordis::ArcValue) -> Result<(), cordis::PluginError> {
                let service: Arc<Arc<SystemPrompt>> = ctx.get_typed("systemPrompt", false).unwrap();
                service.section(&ctx, PromptSection {
                    name: "scoped".into(),
                    order: 0.0,
                    text: "scoped section".into(),
                    complete: None,
                });
                service.context(&ctx, PromptContext {
                    name: "scoped-context".into(),
                    order: 0.0,
                    text: "scoped context".into(),
                });
                service.tools(&ctx, Arc::new(|_ctx| ToolProviderResult {
                    schemas: vec![tool("scoped-tool", "")],
                    known_names: None,
                }));
                service.variable(&ctx,"scoped_var", Arc::new(|_ctx| Some("v".to_string())));
                Ok(())
            }
        }

        let fiber = ctx.plugin(Arc::new(Contributor), cordis::arc(()));
        fiber.settle().await.unwrap();

        let before = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(contributed(&before).len(), 1);
        assert_eq!(before.contexts.len(), 1);
        assert_eq!(before.variables.get("scoped_var"), Some(&Some("v".to_string())));

        fiber.dispose().await;
        let after = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(contributed(&after).len(), 0);
        assert_eq!(after.contexts.len(), 0);
        assert_eq!(
            after.sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            BUILT_IN
        );
        assert!(after.tools.is_empty());
        assert!(after.variables.is_empty());
    }

    #[tokio::test]
    async fn duplicate_and_non_finite_registrations_reject_without_leaking() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());
        service.section(&ctx, PromptSection {
            name: "dup".into(),
            order: 0.0,
            text: "first".into(),
            complete: None,
        });
        let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            service.section(&ctx, PromptSection {
                name: "dup".into(),
                order: 1.0,
                text: "second".into(),
                complete: None,
            })
        }));
        assert!(duplicate.is_err());
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(
            contributed(&assembly).iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
            vec!["first"]
        );

        let non_finite = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            service.section(&ctx, PromptSection {
                name: "bad-order".into(),
                order: f64::NAN,
                text: "x".into(),
                complete: None,
            })
        }));
        assert!(non_finite.is_err());

        service.context(&ctx, PromptContext {
            name: "policy".into(),
            order: 1.0,
            text: "first".into(),
        });
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            service.context(&ctx, PromptContext {
                name: "policy".into(),
                order: 2.0,
                text: "second".into(),
            })
        }))
        .is_err());
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            service.context(&ctx, PromptContext {
                name: "bad".into(),
                order: f64::NAN,
                text: "x".into(),
            })
        }))
        .is_err());
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(assembly.contexts, vec![AssembledContext { name: "policy".into(), text: "first".into() }]);
    }

    #[tokio::test]
    async fn change_listener_throw_rolls_back_registration() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());

        let threw = Arc::new(AtomicU32::new(0));
        let threw_for_listener = threw.clone();
        let disposer = ctx
            .on(
                "system-prompt/change",
                Arc::new(move |_ctx, _args| {
                    let threw = threw_for_listener.clone();
                    Box::pin(async move {
                        if threw.fetch_add(1, MemOrder::SeqCst) == 0 {
                            panic!("boom change listener");
                        }
                        None
                    })
                }),
                EventOptions::default(),
            )
            .await;

        let registration = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            service.section(&ctx, PromptSection {
                name: "p".into(),
                order: 0.0,
                text: "persona".into(),
                complete: None,
            })
        }));
        assert!(registration.is_err());
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(contributed(&assembly).len(), 0, "nothing leaked");

        // Subsequent listener-free register contributes exactly once.
        disposer().await;
        service.section(&ctx, PromptSection {
            name: "p".into(),
            order: 0.0,
            text: "persona".into(),
            complete: None,
        });
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(
            contributed(&assembly).iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["p"]
        );
    }

    #[tokio::test]
    async fn tool_provider_membership_snapshots_before_evaluation() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());
        let service_for_provider = service.clone();
        let ctx_for_register = ctx.clone();
        let added = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let added_for_provider = added.clone();
        service.tools(&ctx, Arc::new(move |_assembly_ctx| {
            if !added_for_provider.swap(true, MemOrder::SeqCst) {
                let service = service_for_provider.clone();
                let ctx = ctx_for_register.clone();
                service.tools(&ctx, Arc::new(|_ctx| ToolProviderResult {
                    schemas: vec![tool("late", "")],
                    known_names: None,
                }));
            }
            ToolProviderResult {
                schemas: vec![tool("first", "")],
                known_names: None,
            }
        }));

        let first = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(names(&first), vec!["first"]);
        let second = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(names(&second), vec!["first", "late"]);
    }

    #[tokio::test]
    async fn waterfall_listeners_compose_in_order_and_mutate_the_shared_assembly() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());
        service.section(&ctx, PromptSection {
            name: "base".into(),
            order: 0.0,
            text: "base".into(),
            complete: None,
        });

        ctx.on(
            "system-prompt/assemble",
            Arc::new(|_ctx, args| {
                Box::pin(async move {
                    let shared = downcast::<SharedAssembly>(&args[0]).expect("assembly arg");
                    shared.0.lock().sections.push(AssembledSection {
                        name: "from-a".into(),
                        text: "a".into(),
                    });
                    let next = downcast::<NextFn>(&args[2]).expect("next");
                    Some(next.call().await)
                })
            }),
            EventOptions::default(),
        )
        .await;
        let seen = Arc::new(parking_lot::Mutex::new(Vec::<Vec<String>>::new()));
        let seen_for_listener = seen.clone();
        ctx.on(
            "system-prompt/assemble",
            Arc::new(move |_ctx, args| {
                let seen = seen_for_listener.clone();
                Box::pin(async move {
                    let shared = downcast::<SharedAssembly>(&args[0]).expect("assembly arg");
                    seen.lock().push(
                        shared
                            .0
                            .lock()
                            .sections
                            .iter()
                            .map(|s| s.name.clone())
                            .collect(),
                    );
                    let next = downcast::<NextFn>(&args[2]).expect("next");
                    Some(next.call().await)
                })
            }),
            EventOptions::default(),
        )
        .await;

        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(
            seen.lock()[0],
            vec!["harness:identity", "deployment:persona", "base", "from-a"]
        );
        assert_eq!(
            assembly.sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["harness:identity", "deployment:persona", "base", "from-a"]
        );
    }

    #[tokio::test]
    async fn waterfall_short_circuit_and_complete_section_restore() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());
        service.section(&ctx, PromptSection {
            name: "real".into(),
            order: 0.0,
            text: "real".into(),
            complete: None,
        });
        ctx.on(
            "system-prompt/assemble",
            Arc::new(|_ctx, _args| {
                Box::pin(async move {
                    Some(arc(SharedAssembly::new(PromptAssembly::default())))
                })
            }),
            EventOptions::default(),
        )
        .await;
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert!(assembly.sections.is_empty());

        // complete section restored AFTER the waterfall
        let ctx2 = Context::root();
        let service2 = install(&ctx2, default_config());
        service2.section(&ctx2, PromptSection {
            name: "complete".into(),
            order: 10.0,
            text: "Exact prompt.".into(),
            complete: Some(true),
        });
        service2.section(&ctx2, PromptSection {
            name: "extra".into(),
            order: 20.0,
            text: "extra".into(),
            complete: None,
        });
        ctx2.on(
            "system-prompt/assemble",
            Arc::new(|_ctx, args| {
                Box::pin(async move {
                    let shared = downcast::<SharedAssembly>(&args[0]).expect("assembly arg");
                    let complete_present = shared
                        .0
                        .lock()
                        .sections
                        .iter()
                        .any(|section| section.name == "complete");
                    assert!(complete_present, "complete section missing before waterfall");
                    shared.0.lock().sections.push(AssembledSection {
                        name: "late".into(),
                        text: "late".into(),
                    });
                    let next = downcast::<NextFn>(&args[2]).expect("next");
                    Some(next.call().await)
                })
            }),
            EventOptions::default().prepend(true),
        )
        .await;
        let assembly = service2.assemble(&ctx2, &AssembleContext::default()).await.unwrap();
        assert_eq!(
            assembly.sections,
            vec![AssembledSection { name: "complete".into(), text: "Exact prompt.".into() }]
        );
    }

    #[tokio::test]
    async fn multiple_complete_sections_reject() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());
        service.section(&ctx, PromptSection {
            name: "first".into(),
            order: 10.0,
            text: "first".into(),
            complete: Some(true),
        });
        service.section(&ctx, PromptSection {
            name: "second".into(),
            order: 20.0,
            text: "second".into(),
            complete: Some(true),
        });
        let error = service.assemble(&ctx, &AssembleContext::default()).await.unwrap_err();
        assert_eq!(error, "multiple complete prompt sections are active: \"first\", \"second\"");
    }

    #[tokio::test]
    async fn assemblies_are_snapshots() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());
        service.section(&ctx, PromptSection {
            name: "base".into(),
            order: 0.0,
            text: "base".into(),
            complete: None,
        });
        service.tools(&ctx, Arc::new(|_ctx| ToolProviderResult {
            schemas: vec![tool("t", "tool")],
            known_names: None,
        }));

        let mut first = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        first.sections[0].name = "mutated".into();
        first.sections[0].text = "mutated".into();
        first.contexts.push(AssembledContext { name: "mutated".into(), text: "mutated".into() });
        first.tools[0].description = "mutated".into();

        let second = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(
            second.sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["harness:identity", "deployment:persona", "base"]
        );
        assert_eq!(second.sections[0].text, IDENTITY);
        assert!(second.contexts.is_empty());
        assert_eq!(second.tools, vec![tool("t", "tool")]);
    }

    #[test]
    fn render_prompt_filters_empty_sections() {
        let assembly = PromptAssembly {
            sections: vec![
                AssembledSection { name: "empty".into(), text: "".into() },
                AssembledSection { name: "real".into(), text: "content".into() },
            ],
            ..Default::default()
        };
        assert_eq!(render_prompt(&assembly).unwrap(), "content");
    }

    #[tokio::test]
    async fn context_snapshot_and_variable_interpolation() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());
        service.context(&ctx, PromptContext { name: "empty".into(), order: 0.0, text: "".into() });
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(render_context_snapshot(&assembly).unwrap(), "");

        service.variable(&ctx,"mode", Arc::new(|_ctx| Some("read-only".to_string())));
        service.context(&ctx, PromptContext {
            name: "policy".into(),
            order: 1.0,
            text: "Mode: {{mode}}.".into(),
        });
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(
            render_context_snapshot(&assembly).unwrap(),
            "Current runtime context. This snapshot supersedes earlier runtime-context snapshots.\n\nMode: read-only."
        );
    }

    #[test]
    fn interpolation_errors_are_attributed() {
        let assembly = PromptAssembly {
            sections: vec![],
            contexts: vec![AssembledContext { name: "policy".into(), text: "Mode: {{missing}}.".into() }],
            tools: vec![],
            variables: IndexMap::new(),
        };
        let error = render_context_snapshot(&assembly).unwrap_err();
        assert_eq!(
            error,
            "unknown prompt variable \"{{missing}}\" in context \"policy\"; registered variables: (none)"
        );
    }

    #[tokio::test]
    async fn change_emit_counts_on_register_and_dispose() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());
        let changes = Arc::new(AtomicU32::new(0));
        let changes_for_listener = changes.clone();
        ctx.on(
            "system-prompt/change",
            Arc::new(move |_ctx, _args| {
                let changes = changes_for_listener.clone();
                Box::pin(async move {
                    changes.fetch_add(1, MemOrder::SeqCst);
                    None
                })
            }),
            EventOptions::default(),
        )
        .await;

        let dispose = service.tools(&ctx, Arc::new(|_ctx| ToolProviderResult::default()));
        assert_eq!(changes.load(MemOrder::SeqCst), 1);
        dispose().await;
        assert_eq!(changes.load(MemOrder::SeqCst), 2);

        let dispose = service.context(&ctx, PromptContext { name: "policy".into(), order: 0.0, text: "current".into() });
        assert_eq!(changes.load(MemOrder::SeqCst), 3);
        dispose().await;
        assert_eq!(changes.load(MemOrder::SeqCst), 4);
    }

    #[tokio::test]
    async fn direct_disposer_removes_contributions() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());

        let dispose = service.section(&ctx, PromptSection {
            name: "direct".into(),
            order: 0.0,
            text: "direct section".into(),
            complete: None,
        });
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(contributed(&assembly).len(), 1);
        dispose().await;
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(contributed(&assembly).len(), 0);

        let dispose = service.tools(&ctx, Arc::new(|_ctx| ToolProviderResult {
            schemas: vec![tool("direct-tool", "")],
            known_names: None,
        }));
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(names(&assembly), vec!["direct-tool"]);
        dispose().await;
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert!(assembly.tools.is_empty());
    }

    #[tokio::test]
    async fn variables_resolve_per_assembly_and_live_iterate() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());

        let dispose = service.variable(&ctx,
            "who",
            Arc::new(|context| context.field_str("who").map(str::to_string)),
        );
        let mut context = AssembleContext::default();
        context.fields.insert("who".into(), serde_json::json!("alice"));
        let assembly = service.assemble(&ctx, &context).await.unwrap();
        assert_eq!(assembly.variables.get("who"), Some(&Some("alice".to_string())));
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(assembly.variables.get("who"), Some(&None));
        dispose().await;
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert!(assembly.variables.is_empty());

        // live iteration: a variable registered by an earlier provider
        // participates in the same assembly
        let service_for_provider = service.clone();
        let ctx_for_register = ctx.clone();
        let added = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let added_for_provider = added.clone();
        service.variable(&ctx,
            "first",
            Arc::new(move |_assembly_ctx| {
                if !added_for_provider.swap(true, MemOrder::SeqCst) {
                    let service = service_for_provider.clone();
                    let ctx = ctx_for_register.clone();
                    service.variable(&ctx,"late", Arc::new(|_ctx| Some("second value".to_string())));
                }
                Some("first value".to_string())
            }),
        );
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(assembly.variables.get("first"), Some(&Some("first value".to_string())));
        assert_eq!(assembly.variables.get("late"), Some(&Some("second value".to_string())));
    }

    #[tokio::test]
    async fn variable_name_validation() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());
        service.variable(&ctx,"model", Arc::new(|_ctx| Some("m1".to_string())));
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            service.variable(&ctx,"model", Arc::new(|_ctx| Some("m2".to_string())))
        }))
        .is_err());
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            service.variable(&ctx,"Not Valid", Arc::new(|_ctx| Some("x".to_string())))
        }))
        .is_err());
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(assembly.variables.get("model"), Some(&Some("m1".to_string())));
    }

    #[tokio::test]
    async fn variables_interpolate_in_persona_and_waterfall() {
        let ctx = Context::root();
        let service = install(
            &ctx,
            Config { persona: "You run on {{model}} in {{cwd}}.".into(), ..default_config() },
        );
        service.variable(&ctx,"model", Arc::new(|_ctx| Some("deepseek-v4".to_string())));
        service.variable(&ctx,"cwd", Arc::new(|_ctx| Some("/work".to_string())));
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(
            render_prompt(&assembly).unwrap(),
            format!("{IDENTITY}\n\nYou run on deepseek-v4 in /work.")
        );

        // waterfall listener adds a variable before render
        let ctx2 = Context::root();
        let service2 = install(&ctx2, default_config());
        service2.section(&ctx2, PromptSection {
            name: "s".into(),
            order: 0.0,
            text: "{{extra}}".into(),
            complete: None,
        });
        ctx2.on(
            "system-prompt/assemble",
            Arc::new(|_ctx, args| {
                Box::pin(async move {
                    let shared = downcast::<SharedAssembly>(&args[0]).expect("assembly arg");
                    shared
                        .0
                        .lock()
                        .variables
                        .insert("extra".into(), Some("from-waterfall".into()));
                    let next = downcast::<NextFn>(&args[2]).expect("next");
                    Some(next.call().await)
                })
            }),
            EventOptions::default(),
        )
        .await;
        let assembly = service2.assemble(&ctx2, &AssembleContext::default()).await.unwrap();
        assert_eq!(render_prompt(&assembly).unwrap(), format!("{IDENTITY}\n\nfrom-waterfall"));
    }

    #[tokio::test]
    async fn unknown_variable_errors_list_registered() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());
        service.section(&ctx, PromptSection {
            name: "persona".into(),
            order: 0.0,
            text: "on {{modle}}".into(),
            complete: None,
        });
        service.variable(&ctx,"model", Arc::new(|_ctx| Some("m".to_string())));
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        let error = render_prompt(&assembly).unwrap_err();
        assert_eq!(
            error,
            "unknown prompt variable \"{{modle}}\" in section \"persona\"; registered variables: model"
        );
    }

    #[test]
    fn interpolation_edge_cases() {
        let empty = PromptAssembly::default();
        let error = render_prompt(&PromptAssembly {
            sections: vec![AssembledSection { name: "s".into(), text: "{{x}}".into() }],
            ..empty.clone()
        })
        .unwrap_err();
        assert_eq!(error, "unknown prompt variable \"{{x}}\" in section \"s\"; registered variables: (none)");

        let mut variables = IndexMap::new();
        variables.insert("cwd".into(), None);
        let error = render_prompt(&PromptAssembly {
            sections: vec![AssembledSection { name: "persona".into(), text: "in {{cwd}}".into() }],
            variables,
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!(
            error,
            "prompt variable \"{{cwd}}\" has no value for this assembly (section \"persona\")"
        );

        let mut variables = IndexMap::new();
        variables.insert("model".into(), Some("m".into()));
        let error = render_prompt(&PromptAssembly {
            sections: vec![AssembledSection { name: "s".into(), text: "on {{ model }}".into() }],
            variables,
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!(error, "malformed prompt variable reference \"{{ model }}\" in section \"s\" (variable names match ^[a-z][a-z0-9_]*$)");

        // lone {{ without a later }} stays literal
        let text = render_prompt(&PromptAssembly {
            sections: vec![AssembledSection {
                name: "s".into(),
                text: "shell ${X:-{{fallback} stays".into(),
            }],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(text, "shell ${X:-{{fallback} stays");

        // mangled references with a }} still following throw the preview error
        for text in ["{{{model}}}", "x {{a{b}} y {{model}}"] {
            let mut variables = IndexMap::new();
            variables.insert("model".into(), Some("m".into()));
            let error = render_prompt(&PromptAssembly {
                sections: vec![AssembledSection { name: "s".into(), text: text.into() }],
                variables,
                ..Default::default()
            })
            .unwrap_err();
            assert!(error.contains("malformed prompt variable reference at"), "{error}");
        }

        // prototype properties are unknown variables (no prototype chain in
        // Rust — trivially rejected)
        let error = render_prompt(&PromptAssembly {
            sections: vec![AssembledSection { name: "s".into(), text: "on {{constructor}}".into() }],
            ..Default::default()
        })
        .unwrap_err();
        assert!(error.contains("unknown prompt variable \"{{constructor}}\""), "{error}");

        // substituted values are never re-scanned
        let mut variables = IndexMap::new();
        variables.insert("model".into(), Some("literal {{sneaky}} inside".into()));
        let text = render_prompt(&PromptAssembly {
            sections: vec![AssembledSection { name: "s".into(), text: "v = {{model}}!".into() }],
            variables,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(text, "v = literal {{sneaky}} inside!");
    }

    // ---- tool order ----

    #[tokio::test]
    async fn tool_order_defaults_and_configured() {
        let ctx = Context::root();
        let service = install(&ctx, default_config());
        service.tools(&ctx, Arc::new(|_ctx| ToolProviderResult {
            schemas: vec![tool("charlie", "charlie"), tool("alpha", "alpha")],
            known_names: None,
        }));
        service.tools(&ctx, Arc::new(|_ctx| ToolProviderResult {
            schemas: vec![tool("bravo", "bravo")],
            known_names: None,
        }));
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(names(&assembly), vec!["alpha", "bravo", "charlie"]);

        // configured order with a rest entry
        let ctx2 = Context::root();
        let service2 = install(
            &ctx2,
            Config {
                tool_order: Some(vec![
                    "todo_write".into(),
                    TOOL_ORDER_REST.into(),
                    "bash".into(),
                ]),
                ..default_config()
            },
        );
        service2.tools(&ctx2, Arc::new(|_ctx| ToolProviderResult {
            schemas: vec![
                tool("bash", ""),
                tool("echo_b", ""),
                tool("todo_write", ""),
                tool("echo_a", ""),
            ],
            known_names: None,
        }));
        let assembly = service2.assemble(&ctx2, &AssembleContext::default()).await.unwrap();
        assert_eq!(names(&assembly), vec!["todo_write", "echo_a", "echo_b", "bash"]);
    }

    #[tokio::test]
    async fn tool_order_rejects_unknown_reserved_and_bad_configs() {
        let ctx = Context::root();
        let service = install(
            &ctx,
            Config {
                tool_order: Some(vec![
                    "todo_write".into(),
                    "ghost".into(),
                    TOOL_ORDER_REST.into(),
                    "wraith".into(),
                ]),
                ..default_config()
            },
        );
        service.tools(&ctx, Arc::new(|_ctx| ToolProviderResult {
            schemas: vec![tool("bash", ""), tool("todo_write", "")],
            known_names: None,
        }));
        let error = service.assemble(&ctx, &AssembleContext::default()).await.unwrap_err();
        assert_eq!(
            error,
            "toolOrder lists unregistered tools \"ghost\", \"wraith\"; known tools: bash, todo_write"
        );

        let ctx2 = Context::root();
        let service2 = install(
            &ctx2,
            Config { tool_order: Some(vec!["ghost".into(), TOOL_ORDER_REST.into()]), ..default_config() },
        );
        let error = service2.assemble(&ctx2, &AssembleContext::default()).await.unwrap_err();
        assert_eq!(error, "toolOrder lists unregistered tool \"ghost\"; known tools: (none)");

        // reserved rest name from a provider
        let ctx3 = Context::root();
        let service3 = install(&ctx3, default_config());
        service3.tools(&ctx3, Arc::new(|_ctx| ToolProviderResult {
            schemas: vec![tool(TOOL_ORDER_REST, "")],
            known_names: None,
        }));
        let error = service3.assemble(&ctx3, &AssembleContext::default()).await.unwrap_err();
        assert_eq!(
            error,
            "tool provider returned reserved tool name \"<unlisted-tools>\" (reserved for toolOrder's rest entry)"
        );

        // load-time rejections: empty list / missing rest / duplicates
        let error = parse_config(&serde_json::json!({"toolOrder": []})).unwrap_err();
        assert!(error.contains("must contain the \"<unlisted-tools>\" rest entry"), "{error}");
        let error =
            parse_config(&serde_json::json!({"toolOrder": ["bash", "todo_write"]})).unwrap_err();
        assert!(error.contains("rest entry"), "{error}");
        let error = parse_config(
            &serde_json::json!({"toolOrder": ["bash", "bash", "<unlisted-tools>"]}),
        )
        .unwrap_err();
        assert!(error.contains("more than once"), "{error}");
        let error = parse_config(
            &serde_json::json!({"toolOrder": ["<unlisted-tools>", "bash", "<unlisted-tools>"]}),
        )
        .unwrap_err();
        assert!(error.contains("more than once"), "{error}");
    }

    #[tokio::test]
    async fn tool_order_stable_sort_and_waterfall_timing() {
        // same-name tools keep collection order (stable sort)
        let ctx = Context::root();
        let service = install(&ctx, default_config());
        service.tools(&ctx, Arc::new(|_ctx| ToolProviderResult {
            schemas: vec![
                tool("dup", "first"),
                tool("anchor", "anchor"),
                tool("dup", "second"),
            ],
            known_names: None,
        }));
        let assembly = service.assemble(&ctx, &AssembleContext::default()).await.unwrap();
        assert_eq!(
            assembly.tools.iter().map(|t| t.description.as_str()).collect::<Vec<_>>(),
            vec!["anchor", "first", "second"]
        );

        // canonicalization happens BEFORE the waterfall; listener edits are
        // owned by the listener
        let ctx2 = Context::root();
        let service2 = install(&ctx2, default_config());
        service2.tools(&ctx2, Arc::new(|_ctx| ToolProviderResult {
            schemas: vec![tool("zulu", ""), tool("alpha", "")],
            known_names: None,
        }));
        let seen = Arc::new(parking_lot::Mutex::new(Vec::<Vec<String>>::new()));
        let seen_for_listener = seen.clone();
        ctx2.on(
            "system-prompt/assemble",
            Arc::new(move |_ctx, args| {
                let seen = seen_for_listener.clone();
                Box::pin(async move {
                    let shared = downcast::<SharedAssembly>(&args[0]).expect("assembly arg");
                    {
                        let mut assembly = shared.0.lock();
                        seen.lock().push(names(&assembly));
                        assembly.tools.push(tool("aardvark", ""));
                    }
                    let next = downcast::<NextFn>(&args[2]).expect("next");
                    Some(next.call().await)
                })
            }),
            EventOptions::default(),
        )
        .await;
        let assembly = service2.assemble(&ctx2, &AssembleContext::default()).await.unwrap();
        assert_eq!(seen.lock()[0], vec!["alpha", "zulu"]);
        assert_eq!(names(&assembly), vec!["alpha", "zulu", "aardvark"]);
    }

    #[test]
    fn config_parse_applies_defaults() {
        let config = parse_config(&serde_json::json!({})).unwrap();
        assert!(config.include_harness_identity);
        assert!(config.include_runtime_context);
        assert_eq!(config.persona, "");
        assert!(config.tool_order.is_none());

        let config = parse_config(&serde_json::json!({
            "includeHarnessIdentity": false,
            "persona": "hi",
            "toolOrder": ["a", "<unlisted-tools>"],
        }))
        .unwrap();
        assert!(!config.include_harness_identity);
        assert_eq!(config.persona, "hi");
        assert_eq!(config.tool_order, Some(vec!["a".to_string(), "<unlisted-tools>".to_string()]));

        assert!(parse_config(&serde_json::json!({"includeHarnessIdentity": "yes"})).is_err());
        assert!(parse_config(&serde_json::json!({"toolOrder": [1, 2]})).is_err());
    }

    #[test]
    fn config_schema_builds() {
        let schema = config_schema();
        assert_eq!(schema.type_name(), "object");
    }
}
