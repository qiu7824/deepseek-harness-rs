//! Model-facing `web_search` over the provider-neutral Rust `web` service.
//! This is the Rust port of `packages/web/tool-web/src/index.ts` and
//! `search.ts` from Node `origin/master`, scoped to the shipped standard
//! preset (`fetch: false`). Provider selection and credentials stay behind
//! [`WebSearch`].

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use cordis::{ArcValue, Context, Disposer, Plugin, PluginError};
use dsh_system_prompt::{PromptSection, PromptText, SystemPrompt};
use dsh_tools::{
    ToolBodyError, ToolCallKind, ToolCallView, ToolDefinition, ToolOutputDefinition, ToolResult,
    ToolResultView, ToolRunContext, ToolRuntime, WebResultView, WebSource,
    validate_json_schema_value,
};
pub use dsh_web::{WebSearch, WebSearchRequest, WebSearchResult, WebSearchSource};
use futures::stream::{FuturesUnordered, StreamExt};

pub const NAME: &str = "tool-web";
pub const INJECT: [&str; 3] = ["tools", "web", "systemPrompt"];
pub const DEFAULT_WEB_TOOL_TIMEOUT_MS: u64 = 30_000;
pub const WEB_SEARCH_MAX_RESULTS: usize = 8;
pub const WEB_SEARCH_MAX_QUERIES: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub search: bool,
    pub fetch: bool,
    pub search_max_results: usize,
    pub search_max_queries: usize,
    pub fetch_timeout_ms: u64,
    pub search_timeout_ms: u64,
    pub fetch_max_output_chars: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            search: true,
            fetch: true,
            search_max_results: WEB_SEARCH_MAX_RESULTS,
            search_max_queries: WEB_SEARCH_MAX_QUERIES,
            fetch_timeout_ms: DEFAULT_WEB_TOOL_TIMEOUT_MS,
            search_timeout_ms: DEFAULT_WEB_TOOL_TIMEOUT_MS,
            fetch_max_output_chars: 200_000,
        }
    }
}

impl Config {
    fn from_json(value: &serde_json::Value) -> Result<Self, String> {
        let mut config = Self::default();
        if let Some(value) = value.get("search") {
            config.search = value
                .as_bool()
                .ok_or_else(|| "tool-web: search must be a boolean".to_string())?;
        }
        if let Some(value) = value.get("fetch") {
            config.fetch = value
                .as_bool()
                .ok_or_else(|| "tool-web: fetch must be a boolean".to_string())?;
        }
        config.search_max_results =
            positive_usize(value, "searchMaxResults", config.search_max_results)?;
        config.search_max_queries =
            positive_usize(value, "searchMaxQueries", config.search_max_queries)?;
        config.fetch_timeout_ms = positive_u64(value, "fetchTimeoutMs", config.fetch_timeout_ms)?;
        config.search_timeout_ms =
            positive_u64(value, "searchTimeoutMs", config.search_timeout_ms)?;
        config.fetch_max_output_chars =
            positive_usize(value, "fetchMaxOutputChars", config.fetch_max_output_chars)?;
        Ok(config)
    }
}

fn positive_u64(value: &serde_json::Value, name: &str, default: u64) -> Result<u64, String> {
    match value.get(name) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .filter(|value| *value > 0)
            .ok_or_else(|| format!("tool-web: {name} must be a positive integer")),
    }
}

fn positive_usize(value: &serde_json::Value, name: &str, default: usize) -> Result<usize, String> {
    positive_u64(value, name, default as u64).and_then(|value| {
        usize::try_from(value).map_err(|_| format!("tool-web: {name} must be a positive integer"))
    })
}

pub fn parse_search_args(queries: &[String], max_queries: usize) -> Result<Vec<String>, String> {
    if queries.is_empty() {
        return Err("queries must contain at least one query".into());
    }
    if queries.len() > max_queries {
        let noun = if max_queries == 1 { "query" } else { "queries" };
        return Err(format!("queries must contain at most {max_queries} {noun}"));
    }
    if queries.iter().any(|query| query.trim().is_empty()) {
        return Err("each query must be a non-empty string".into());
    }
    let mut seen = HashSet::new();
    Ok(queries
        .iter()
        .filter(|query| seen.insert((*query).clone()))
        .cloned()
        .collect())
}

fn source_label(source: &WebSearchSource) -> String {
    if let Some(title) = source.title.as_ref().filter(|title| !title.is_empty()) {
        return title.clone();
    }
    let after_scheme = source
        .url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(&source.url);
    let hostname = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    if hostname.is_empty() {
        source.url.clone()
    } else {
        hostname.to_string()
    }
}

pub fn format_search_output(result: &WebSearchResult) -> String {
    let mut parts = Vec::new();
    if let Some(content) = result
        .content
        .as_ref()
        .filter(|content| !content.is_empty())
    {
        parts.push(content.clone());
    }
    if result.sources.is_empty() {
        if result.content.as_ref().is_none_or(String::is_empty) {
            parts.push("No results found.".into());
        }
    } else {
        let lines = result
            .sources
            .iter()
            .map(|source| {
                let mut meta = Vec::new();
                if let Some(snippet) = source
                    .snippet
                    .as_ref()
                    .filter(|snippet| !snippet.is_empty())
                {
                    meta.push(snippet.clone());
                }
                if let Some(date) = source.published_at.as_ref().filter(|date| !date.is_empty()) {
                    meta.push(format!("({date})"));
                }
                let suffix = if meta.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", meta.join(" "))
                };
                format!("- [{}]({}){suffix}", source_label(source), source.url)
            })
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!("Sources:\n{lines}"));
    }
    if result.truncated {
        parts.push(format!(
            "(Showing the first {} sources. Refine the query for more.)",
            result.sources.len()
        ));
    }
    parts.push("Cite the relevant URLs above as markdown links in your answer.".into());
    parts.join("\n\n")
}

fn parameters_schema(max_queries: usize) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "queries": {
                "type": "array",
                "items": { "type": "string" },
                "description": format!("Required search queries; accepts 1–{max_queries} items and merges their results.")
            }
        },
        "required": ["queries"]
    })
}

fn output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "content": { "type": "string" },
            "sources": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "url": { "type": "string" },
                        "title": { "type": "string" },
                        "snippet": { "type": "string" },
                        "publishedAt": { "type": "string" }
                    },
                    "required": ["url"]
                }
            },
            "truncated": { "type": "boolean" }
        },
        "required": ["sources", "truncated"]
    })
}

fn result_to_value(result: &WebSearchResult) -> serde_json::Value {
    serde_json::to_value(result).expect("web search result")
}

fn value_to_result(value: &serde_json::Value) -> Result<WebSearchResult, String> {
    serde_json::from_value(value.clone()).map_err(|error| error.to_string())
}

fn merge_search_results(
    queries: &[String],
    results: &[WebSearchResult],
    max_results: usize,
) -> WebSearchResult {
    let mut seen = HashSet::new();
    let mut sources = Vec::new();
    let ranks = results
        .iter()
        .map(|result| result.sources.len())
        .max()
        .unwrap_or(0);
    let mut dropped = false;
    'merge: for rank in 0..ranks {
        for result in results {
            if let Some(source) = result.sources.get(rank) {
                if seen.insert(source.url.clone()) {
                    if sources.len() == max_results {
                        dropped = true;
                        break 'merge;
                    }
                    sources.push(source.clone());
                }
            }
        }
    }
    let contents = results
        .iter()
        .enumerate()
        .filter_map(|(index, result)| {
            result
                .content
                .as_ref()
                .filter(|content| !content.is_empty())
                .map(|content| format!("### {}\n\n{content}", queries[index]))
        })
        .collect::<Vec<_>>();
    WebSearchResult {
        content: (!contents.is_empty()).then(|| contents.join("\n\n")),
        sources,
        truncated: dropped || results.iter().any(|result| result.truncated),
    }
}

async fn run_search_queries(
    web: Arc<dyn WebSearch>,
    queries: Vec<String>,
    max_results: usize,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<WebSearchResult, String> {
    if queries.len() == 1 {
        return web
            .search(
                WebSearchRequest {
                    query: queries[0].clone(),
                    max_results: Some(max_results),
                },
                cancelled,
            )
            .await
            .map_err(|error| error.to_string());
    }
    let batch_cancelled = Arc::new(AtomicBool::new(false));
    let mut futures = queries
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, query)| {
            let web = web.clone();
            let caller_cancelled = cancelled.clone();
            let batch_cancelled = batch_cancelled.clone();
            async move {
                let fused =
                    Arc::new(move || caller_cancelled() || batch_cancelled.load(Ordering::SeqCst));
                (
                    index,
                    web.search(
                        WebSearchRequest {
                            query,
                            max_results: Some(max_results),
                        },
                        fused,
                    )
                    .await
                    .map_err(|error| error.to_string()),
                )
            }
        })
        .collect::<FuturesUnordered<_>>();
    let mut results = vec![None; queries.len()];
    let mut first_failure = None;
    while let Some((index, result)) = futures.next().await {
        match result {
            Ok(result) => results[index] = Some(result),
            Err(error) => {
                if first_failure.is_none() {
                    first_failure = Some(error);
                    batch_cancelled.store(true, Ordering::SeqCst);
                }
            }
        }
    }
    if let Some(error) = first_failure {
        return Err(error);
    }
    let results = results.into_iter().map(Option::unwrap).collect::<Vec<_>>();
    Ok(merge_search_results(&queries, &results, max_results))
}

pub fn apply(ctx: &Context, config: &Config) -> Result<Disposer, String> {
    if config.fetch {
        return Err(
            "tool-web: web_fetch is not implemented in this Rust migration; set fetch: false"
                .into(),
        );
    }
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-web requires the tools service".to_string())?;
    let web = ctx
        .get_typed::<Arc<dyn WebSearch>>("web", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-web requires the web service".to_string())?;
    let prompt = ctx
        .get_typed::<Arc<SystemPrompt>>("systemPrompt", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "tool-web requires the systemPrompt service".to_string())?;
    if !config.search {
        return Ok(cordis::make_disposer(|| Box::pin(async {})));
    }

    let max_queries = config.search_max_queries;
    let max_results = config.search_max_results;
    let prompt_disposer = prompt.section(ctx, PromptSection {
        name: "tool:web_search".into(),
        order: 110.0,
        text: PromptText::Static(format!(
            "Use the web_search tool to discover current information on the web. The required queries array accepts 1–{max_queries} non-empty search queries; use a one-item array for a single search. It returns an optional answer plus a list of source URLs. Use the returned source snippets when available, and cite the relevant URLs as markdown links."
        )),
        complete: None,
    });
    let definition = ToolDefinition {
        name: "web_search".into(),
        description: format!(
            "Search the web for current information. Provide 1–{max_queries} queries in the required queries array. Returns an optional summary answer and a list of source URLs."
        ),
        parameters: parameters_schema(max_queries),
        output: ToolOutputDefinition {
            schema: output_schema(),
            render: Arc::new(|_, value| {
                Ok(vec![dsh_llm::ContentBlock::Text {
                    text: format_search_output(&value_to_result(value)?),
                }])
            }),
            presentation_meta: Some(Arc::new(|_, value| {
                let result = value_to_result(value)?;
                let mut meta = serde_json::json!({
                    "sources": result.sources,
                    "truncated": result.truncated,
                });
                if let Some(answer) = result.content {
                    meta.as_object_mut()
                        .expect("search meta object")
                        .insert("answer".into(), serde_json::Value::String(answer));
                }
                Ok(meta)
            })),
        },
        timeout_ms: Some(config.search_timeout_ms),
        is_concurrency_safe: Some(Arc::new(|_| true)),
        execute: Arc::new(move |args, exec: &ToolRunContext| {
            let args = args.clone();
            let web = web.clone();
            let cancelled = exec.signal.lock().clone();
            Box::pin(async move {
                let violations =
                    validate_json_schema_value(&parameters_schema(max_queries), &args, "arguments");
                if !violations.is_empty() {
                    return Err(ToolBodyError::plain(violations.join("; ")));
                }
                let raw = args["queries"]
                    .as_array()
                    .expect("schema validated queries");
                let queries = raw
                    .iter()
                    .map(|value| value.as_str().expect("schema string").to_string())
                    .collect::<Vec<_>>();
                let queries =
                    parse_search_args(&queries, max_queries).map_err(ToolBodyError::plain)?;
                let result = run_search_queries(web, queries, max_results, cancelled)
                    .await
                    .map_err(ToolBodyError::plain)?;
                Ok(result_to_value(&result))
            })
        }),
        finalize_content: None,
        present_call: Some(Arc::new(|args| {
            let title = args
                .get("queries")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            Some(ToolCallView::Generic {
                title: title.clone(),
                kind: Some(ToolCallKind::Search),
                raw_input: Some(serde_json::Value::String(title)),
                content: None,
                locations: None,
            })
        })),
        present_result: Some(Arc::new(|args, result: &ToolResult| {
            if result.is_error {
                return None;
            }
            let meta = result.meta.as_ref()?;
            let sources: Vec<WebSource> =
                serde_json::from_value(meta.get("sources")?.clone()).ok()?;
            let title = args
                .get("queries")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                });
            Some(ToolResultView::Web(WebResultView::Search {
                title,
                sources,
                answer: meta
                    .get("answer")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                truncated: meta.get("truncated").and_then(serde_json::Value::as_bool)?,
            }))
        })),
    };
    let tool_disposer = tools
        .register(ctx, definition)
        .map_err(|error| format!("tool-web: {error}"))?;
    Ok(cordis::make_disposer(move || {
        let tool_disposer = tool_disposer.clone();
        let prompt_disposer = prompt_disposer.clone();
        Box::pin(async move {
            tool_disposer().await;
            prompt_disposer().await;
        })
    }))
}

pub struct ToolWebPlugin;

#[async_trait]
impl Plugin for ToolWebPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }
    fn inject(&self) -> cordis::InjectSpec {
        cordis::InjectSpec::new(INJECT)
    }
    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let config = if let Some(config) = config.downcast_ref::<Config>() {
            config.clone()
        } else if let Some(value) = config.downcast_ref::<serde_json::Value>() {
            Config::from_json(value).map_err(|error| PluginError::from(anyhow::anyhow!(error)))?
        } else {
            Config::default()
        };
        let disposer =
            apply(ctx, &config).map_err(|error| PluginError::from(anyhow::anyhow!(error)))?;
        let _ = ctx.effect("tool-web", Box::pin(async move { Some(disposer) }));
        Ok(())
    }
}
