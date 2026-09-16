//! Provider-independent progressive tool disclosure. Discovery never changes permissions.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

use cordis::Context;
use dsh_llm::{ContentBlock, ToolSchema};
use dsh_session::{Session, SessionSeq, SessionStore, session_id};
use dsh_system_prompt::{AssembleContext, PromptContext, PromptText, SystemPrompt};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};

const EVENT: &str = "tools/discovery";
const SEARCH: &str = "tool_search";
const DESCRIBE: &str = "tool_describe";
const SCAN_LIMIT: u64 = 8192;
const SESSION_LIMIT: usize = 64;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoveryConfig {
    pub enabled: bool,
    pub eager_limit: usize,
    pub listing_chars: usize,
    pub max_loaded: usize,
    pub max_schema_bytes: usize,
    pub deferred_prefixes: Vec<String>,
    pub eager_tools: Vec<String>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            eager_limit: 32,
            listing_chars: 4096,
            max_loaded: 96,
            max_schema_bytes: 128 * 1024,
            deferred_prefixes: vec!["mcp__".into()],
            eager_tools: [
                "read",
                "read_image",
                "write",
                "edit",
                "glob",
                "grep",
                "bash",
                "pwsh",
                "skill",
                "ask_user_question",
                "todo",
                "todo_write",
                "present",
                "present_files",
                "web_search",
                "web_fetch",
                "spawn_agent",
                "spawn_subagent",
                "send_message",
                "wait_agent",
                "memory",
                "memory_search",
                "memory_get",
                "memory_write",
                "get_goal",
                "create_goal",
                "update_goal",
                "tool_search",
                "tool_describe",
                "environment_probe",
                "environment_validate",
                "execute_native",
                "execute_steps",
                "execute_script",
                "task_execution",
                "run_code",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        }
    }
}

impl DiscoveryConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.eager_limit > 4096
            || !(256..=32768).contains(&self.listing_chars)
            || !(1..=256).contains(&self.max_loaded)
            || !(4096..=1024 * 1024).contains(&self.max_schema_bytes)
            || self.deferred_prefixes.len() > 64
            || self.eager_tools.len() > 256
            || self
                .deferred_prefixes
                .iter()
                .chain(&self.eager_tools)
                .any(|s| s.is_empty() || s.len() > 256)
        {
            return Err("invalid tool discovery budget or name list".into());
        }
        Ok(())
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    loaded: BTreeMap<String, String>,
}
#[derive(Clone, Default)]
struct CachedSession {
    identity: usize,
    next: u64,
    snapshot: Snapshot,
    limited: bool,
}

pub struct ToolDiscovery {
    ctx: Context,
    runtime: Weak<ToolRuntime>,
    config: DiscoveryConfig,
    sessions: Mutex<VecDeque<(String, CachedSession)>>,
    search_requests: AtomicU64,
    describe_requests: AtomicU64,
    search_hits: AtomicU64,
    discovery_micros: AtomicU64,
}

pub fn install(
    ctx: &Context,
    runtime: &Arc<ToolRuntime>,
    config: DiscoveryConfig,
) -> Result<(), String> {
    config.validate()?;
    if !config.enabled {
        return Ok(());
    }
    if runtime.discovery.lock().is_some() {
        return Err("tool discovery already installed".into());
    }
    for name in [SEARCH, DESCRIBE] {
        if runtime.get(name, None).is_some() {
            return Err(format!("tool discovery name already registered: {name}"));
        }
    }
    let prompt = ctx
        .get_typed::<Arc<SystemPrompt>>("systemPrompt", false)
        .ok_or("systemPrompt unavailable")?;
    let discovery = Arc::new(ToolDiscovery {
        ctx: ctx.clone(),
        runtime: Arc::downgrade(runtime),
        config,
        sessions: Mutex::new(VecDeque::new()),
        search_requests: AtomicU64::new(0),
        describe_requests: AtomicU64::new(0),
        search_hits: AtomicU64::new(0),
        discovery_micros: AtomicU64::new(0),
    });
    let search = runtime.prepare_register_arc(ctx, definition(&discovery, false))?;
    let describe = runtime.prepare_register_arc(ctx, definition(&discovery, true))?;
    search.commit(ctx);
    describe.commit(ctx);
    *runtime.discovery.lock() = Some(discovery.clone());
    prompt.context(
        ctx,
        PromptContext {
            name: "tools:discovery".into(),
            order: 80.0,
            text: PromptText::Provider(Arc::new(move |assemble| discovery.manifest(assemble))),
        },
    );
    Ok(())
}

impl ToolDiscovery {
    fn session(&self, assemble: &AssembleContext) -> Option<Session> {
        self.ctx
            .get_typed::<Arc<SessionStore>>("sessions", false)?
            .get(&session_id(assemble.field_str("sessionId")?))
    }

    fn state(&self, session: &Session) -> CachedSession {
        let mut sessions = self.sessions.lock();
        let mut state = sessions
            .iter()
            .position(|(id, _)| id == session.id().as_str())
            .and_then(|position| sessions.remove(position))
            .map(|(_, state)| state)
            .unwrap_or_default();
        let end = session.seq().get();
        if state.next > end || state.identity != session.identity() {
            state = CachedSession::default();
        }
        state.identity = session.identity();
        let start = state.next.max(end.saturating_sub(SCAN_LIMIT));
        state.limited |= start > state.next;
        // A snapshot is complete; scanning backwards stops at the newest one.
        for seq in (start..end).rev() {
            let Some(event) = SessionSeq::new(seq)
                .ok()
                .and_then(|seq| session.event_at(seq))
            else {
                continue;
            };
            if event.type_ != EVENT {
                continue;
            }
            if event.data.to_string().len() > 96 * 1024 {
                continue;
            }
            if let Ok(snapshot) = serde_json::from_value::<Snapshot>(event.data) {
                if snapshot.version == 1
                    && snapshot.loaded.len() <= 256
                    && snapshot
                        .loaded
                        .iter()
                        .all(|(name, hash)| name.len() <= 256 && hash.len() == 64)
                {
                    state.snapshot = snapshot;
                    state.limited = false;
                    break;
                }
            }
        }
        state.next = end;
        if sessions.len() >= SESSION_LIMIT {
            sessions.pop_front();
        }
        sessions.push_back((session.id().to_string(), state.clone()));
        state
    }

    fn deferred(&self, tool: &ToolSchema, count: usize) -> bool {
        if [SEARCH, DESCRIBE, "environment_probe", "environment_validate", "execute_native", "execute_steps", "execute_script", "task_execution", "run_code"].contains(&tool.name.as_str())
            || self.config.eager_tools.contains(&tool.name)
        {
            return false;
        }
        // An individually oversized definition must remain reachable without exceeding the loading budget.
        if schema_bytes(tool) > self.config.max_schema_bytes || tool.name.len() > 256 {
            return false;
        }
        count > self.config.eager_limit
            || self
                .config
                .deferred_prefixes
                .iter()
                .any(|prefix| tool.name.starts_with(prefix))
    }

    pub(crate) fn present(
        &self,
        assemble: &AssembleContext,
        schemas: Vec<ToolSchema>,
    ) -> Vec<ToolSchema> {
        // A restricted agent without both discovery entries must not lose its callable tools.
        if ![SEARCH, DESCRIBE]
            .iter()
            .all(|name| schemas.iter().any(|s| &s.name == name))
        {
            return schemas;
        }
        let Some(session) = self.session(assemble) else {
            return schemas;
        };
        let state = self.state(&session);
        let count = schemas.len();
        let mut loaded = 0;
        let mut bytes = 0;
        schemas
            .into_iter()
            .filter(|tool| {
                if !self.deferred(tool, count) {
                    return true;
                }
                let size = schema_bytes(tool);
                if loaded >= self.config.max_loaded || bytes + size > self.config.max_schema_bytes {
                    return false;
                }
                if state
                    .snapshot
                    .loaded
                    .get(&tool.name)
                    .is_some_and(|digest| digest == &fingerprint(tool))
                {
                    loaded += 1;
                    bytes += size;
                    true
                } else {
                    false
                }
            })
            .collect()
    }

    fn manifest(&self, assemble: &AssembleContext) -> String {
        let Some(runtime) = self.runtime.upgrade() else {
            return String::new();
        };
        let tools = runtime.schemas(assemble.scope.as_ref());
        if ![SEARCH, DESCRIBE]
            .iter()
            .all(|name| tools.iter().any(|s| &s.name == name))
        {
            return String::new();
        }
        let deferred: Vec<_> = tools
            .iter()
            .filter(|s| self.deferred(s, tools.len()))
            .collect();
        if deferred.is_empty() {
            return String::new();
        }
        let mut text = String::from(
            "Use tool_search to find additional tools or tool_describe for exact names. Results load schemas for later calls; reuse them until definitions change. Normal permissions always apply.\n",
        );
        if runtime
            .get(crate::RUN_CODE_NAME, assemble.scope.as_ref())
            .is_some()
        {
            text.push_str("In code mode, invoke tools.tool_search or tools.tool_describe inside run_code, then tools.<name> with the returned parameters.\n");
        }
        let mut sources: BTreeMap<String, Vec<&ToolSchema>> = BTreeMap::new();
        for tool in deferred {
            sources.entry(source(&tool.name)).or_default().push(tool);
        }
        let mut omitted = 0;
        for (source, group) in sources {
            let detailed = group
                .iter()
                .map(|tool| {
                    format!(
                        "{}: {}",
                        tool.name,
                        clip(&tool.description.replace(['\n', '\r'], " "), 100)
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            let names = group
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let remaining = self
                .config
                .listing_chars
                .saturating_sub(text.chars().count() + 100);
            let line = [
                format!("{source}: {detailed}\n"),
                format!("{source}: {names}\n"),
                format!("{source}: {} tools (search to discover)\n", group.len()),
            ]
            .into_iter()
            .find(|line| line.chars().count() <= remaining);
            if let Some(line) = line {
                text.push_str(&line);
            } else {
                omitted += group.len();
            }
        }
        if omitted > 0 {
            text.push_str(&format!(
                "{omitted} further tools omitted from this listing; tool_search includes them.\n"
            ));
        }
        if self
            .session(assemble)
            .is_some_and(|s| self.state(&s).limited)
        {
            text.push_str(
                "Older discovery state exceeded the restore window; use exact names to reload.\n",
            );
        }
        clip(&text, self.config.listing_chars)
    }

    fn load(
        &self,
        session: &Session,
        visible: &[ToolSchema],
        selected: &[String],
        release: &[String],
    ) -> Result<Value, ToolBodyError> {
        let state = self.state(session);
        let mut snapshot = state.snapshot;
        let original = snapshot.loaded.clone();
        snapshot.version = 1;
        snapshot.loaded.retain(|name, digest| {
            !release.contains(name)
                && visible
                    .iter()
                    .any(|tool| &tool.name == name && &fingerprint(tool) == digest)
        });
        let mut bytes = 0;
        let mut loaded = 0;
        snapshot.loaded.retain(|name, _| {
            let size = visible
                .iter()
                .find(|tool| &tool.name == name)
                .map(schema_bytes)
                .unwrap_or(0);
            if loaded >= self.config.max_loaded || bytes + size > self.config.max_schema_bytes {
                return false;
            }
            loaded += 1;
            bytes += size;
            true
        });
        let mut schemas = Vec::new();
        let mut unavailable = Vec::new();
        let mut over_budget = Vec::new();
        let mut seen = BTreeSet::new();
        for name in selected {
            if !seen.insert(name) {
                continue;
            }
            let Some(tool) = visible.iter().find(|tool| &tool.name == name) else {
                unavailable.push(name.clone());
                continue;
            };
            if self.deferred(tool, visible.len()) && !snapshot.loaded.contains_key(name) {
                let added = schema_bytes(tool);
                if snapshot.loaded.len() >= self.config.max_loaded
                    || bytes + added > self.config.max_schema_bytes
                {
                    over_budget.push(name.clone());
                    continue;
                }
                snapshot.loaded.insert(name.clone(), fingerprint(tool));
                bytes += added;
            }
            schemas.push(json!({"name": tool.name, "description": tool.description, "parameters": tool.parameters}));
        }
        if original != snapshot.loaded {
            session
                .append(
                    EVENT,
                    serde_json::to_value(&snapshot)
                        .map_err(|e| ToolBodyError::plain(e.to_string()))?,
                    None,
                )
                .map_err(ToolBodyError::plain)?;
        }
        Ok(
            json!({"tools":schemas, "unavailable":unavailable, "overBudget":over_budget,
            "loadedCount":snapshot.loaded.len(), "loadedSchemaBytes":bytes,
            "catalogCount":visible.len(), "budgetHint":"Use tool_describe release to release unused definitions if overBudget is nonempty."}),
        )
    }
}

fn definition(discovery: &Arc<ToolDiscovery>, exact: bool) -> Arc<ToolDefinition> {
    let owner = Arc::downgrade(discovery);
    Arc::new(ToolDefinition {
        name: if exact { DESCRIBE } else { SEARCH }.into(),
        description: if exact { "Load tools by exact names for future calls without searching. Optionally release unused loaded names to free the schema budget. Use after a tool definition changes." }
            else { "Search available tool names, sources and descriptions. Returns and loads matching full schemas for future calls; do not repeat for tools already loaded. Search failure is not proof a capability is absent; inspect availableSources or use exact names." }.into(),
        parameters: if exact { json!({"type":"object","properties":{"names":{"type":"array","items":{"type":"string","maxLength":256},"maxItems":16},"release":{"type":"array","items":{"type":"string","maxLength":256},"maxItems":256}},"additionalProperties":false}) }
            else { json!({"type":"object","properties":{"query":{"type":"string","minLength":1,"maxLength":512},"limit":{"type":"integer","minimum":1,"maximum":16}},"required":["query"],"additionalProperties":false}) },
        output: ToolOutputDefinition { schema: json!({"type":"object"}), render: Arc::new(|_, value| Ok(vec![ContentBlock::Text { text: value.to_string() }])), presentation_meta: None },
        timeout_ms: None, is_concurrency_safe: None,
        execute: Arc::new(move |args, exec| {
            let owner = owner.clone(); let args = args.clone(); let agent = exec.agent.clone();
            Box::pin(async move {
                let owner = owner.upgrade().ok_or_else(|| ToolBodyError::plain("tool discovery unavailable"))?;
                let started = std::time::Instant::now();
                if exact {owner.describe_requests.fetch_add(1,Ordering::Relaxed);}else{owner.search_requests.fetch_add(1,Ordering::Relaxed);}
                let runtime = owner.runtime.upgrade().ok_or_else(|| ToolBodyError::plain("tool runtime unavailable"))?;
                let agent = agent.ok_or_else(|| ToolBodyError::plain("tool discovery requires an agent session"))?;
                let visible = runtime.schemas(Some(agent.scope_key()));
                let release = names(&args["release"]);
                let selected = if exact { names(&args["names"]) } else {
                    let query = args["query"].as_str().unwrap_or_default().trim();
                    if query.is_empty() { return Err(ToolBodyError::plain("query must not be empty")); }
                    search(&visible, query, args["limit"].as_u64().unwrap_or(5).min(16) as usize)
                };
                let mut value = owner.load(agent.session(), &visible, &selected, &release)?;
                if !exact {owner.search_hits.fetch_add(selected.len() as u64,Ordering::Relaxed);}
                owner.discovery_micros.fetch_add(started.elapsed().as_micros().min(u64::MAX as u128) as u64,Ordering::Relaxed);
                let mut sources = BTreeMap::<String, usize>::new();
                for tool in &visible { *sources.entry(source(&tool.name)).or_default() += 1; }
                let source_count = sources.len();
                value["availableSources"] = json!(sources.into_iter().take(64).collect::<BTreeMap<_,_>>());
                value["omittedSources"] = json!(source_count.saturating_sub(64));
                Ok(value)
            })
        }),
        finalize_content: None, present_call: None, present_result: None,
    })
}

impl ToolRuntime {
    /// Host diagnostics only; no tool names, search text or private schemas leave their scope.
    pub fn discovery_diagnostics(&self) -> Value {
        let service = self.discovery.lock().clone();
        match service {
            Some(service) => json!({"enabled":true,"effectiveConfig":service.config,
                "cachedSessions":service.sessions.lock().len(),"sessionLimit":SESSION_LIMIT,"restoreScanLimit":SCAN_LIMIT,
                "searchRequests":service.search_requests.load(Ordering::Relaxed),
                "describeRequests":service.describe_requests.load(Ordering::Relaxed),
                "searchHits":service.search_hits.load(Ordering::Relaxed),
                "discoveryMicros":service.discovery_micros.load(Ordering::Relaxed)}),
            None => json!({"enabled":false,"cachedSessions":0,"searchRequests":0,"describeRequests":0,"searchHits":0,"discoveryMicros":0}),
        }
    }
}

fn names(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}
fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
fn schema_bytes(tool: &ToolSchema) -> usize {
    json!({"name":tool.name,"description":tool.description,"parameters":tool.parameters})
        .to_string()
        .len()
}
fn fingerprint(tool: &ToolSchema) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&json!([tool.name, tool.description, tool.parameters])).unwrap()
        )
    )
}
fn source(name: &str) -> String {
    name.strip_prefix("mcp__")
        .and_then(|name| name.split_once("__"))
        .map(|(server, _)| format!("mcp:{server}"))
        .unwrap_or_else(|| "host".into())
}
fn tokens(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut tokens: Vec<String> = lower
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect();
    let chars: Vec<char> = lower.chars().collect();
    for pair in chars.windows(2) {
        if pair.iter().all(|ch| !ch.is_ascii() && ch.is_alphanumeric()) {
            tokens.push(pair.iter().collect());
        }
    }
    tokens
}

/// BM25 with name weighting and exact/substring fallback, including CJK bigrams.
fn search(tools: &[ToolSchema], query: &str, limit: usize) -> Vec<String> {
    let query_terms: BTreeSet<_> = tokens(query).into_iter().collect();
    let documents: Vec<_> = tools
        .iter()
        .map(|tool| {
            tokens(&format!(
                "{} {} {} {} {}",
                tool.name,
                tool.name,
                tool.name,
                source(&tool.name),
                clip(&tool.description, 2048)
            ))
        })
        .collect();
    let average =
        (documents.iter().map(Vec::len).sum::<usize>() as f64 / tools.len().max(1) as f64).max(1.0);
    let frequencies: BTreeMap<_, _> = query_terms
        .iter()
        .map(|term| {
            (
                term,
                documents.iter().filter(|doc| doc.contains(term)).count() as f64,
            )
        })
        .collect();
    let lower = query.to_lowercase();
    let mut scored = Vec::new();
    for (tool, document) in tools.iter().zip(&documents) {
        if [SEARCH, DESCRIBE].contains(&tool.name.as_str()) {
            continue;
        }
        let mut score = 0.0;
        for term in &query_terms {
            let tf = document.iter().filter(|word| *word == term).count() as f64;
            if tf == 0.0 {
                continue;
            }
            let df = frequencies[term];
            let idf = (1.0 + (tools.len() as f64 - df + 0.5) / (df + 0.5)).ln();
            score += idf * tf * 2.2 / (tf + 1.2 * (0.25 + 0.75 * document.len() as f64 / average));
        }
        if tool.name.to_lowercase() == lower {
            score += 1000.0;
        } else if tool.name.to_lowercase().contains(&lower) {
            score += 20.0;
        }
        if score > 0.0 {
            scored.push((score, tool.name.clone()));
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, name)| name)
        .collect()
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
