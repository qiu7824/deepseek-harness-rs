use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cordis::{Context, Service, arc};
use dsh_llm::CallId;
use dsh_system_prompt::{AssembleContext, Config as PromptConfig, SystemPrompt};
use dsh_tool_web::{
    Config, NAME, ToolWebPlugin, WEB_SEARCH_MAX_QUERIES, WEB_SEARCH_MAX_RESULTS, WebSearch,
    WebSearchRequest, WebSearchResult, WebSearchSource, format_search_output, parse_search_args,
};
use dsh_tools::{ToolExecutionInput, ToolRuntime};
use dsh_web::WebError;

#[derive(Default)]
struct StubWeb {
    requests: Mutex<Vec<WebSearchRequest>>,
}

impl Service for StubWeb {
    fn service_name(&self) -> &'static str {
        "web"
    }
}

#[async_trait]
impl WebSearch for StubWeb {
    async fn search(
        &self,
        request: WebSearchRequest,
        _cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<WebSearchResult, WebError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(match request.query.as_str() {
            "one" => WebSearchResult {
                content: Some("answer one".into()),
                sources: vec![
                    source("https://a.test", Some("A")),
                    source("https://shared.test", None),
                ],
                truncated: false,
            },
            "two" => WebSearchResult {
                content: Some("answer two".into()),
                sources: vec![
                    source("https://b.test", Some("B")),
                    source("https://shared.test", None),
                ],
                truncated: false,
            },
            _ => WebSearchResult {
                content: None,
                sources: vec![],
                truncated: false,
            },
        })
    }
}

struct FailingWeb {
    sibling_cancelled: Arc<AtomicBool>,
    sibling_settled: Arc<AtomicBool>,
}

impl Service for FailingWeb {
    fn service_name(&self) -> &'static str {
        "web"
    }
}

#[async_trait]
impl WebSearch for FailingWeb {
    async fn search(
        &self,
        request: WebSearchRequest,
        cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<WebSearchResult, WebError> {
        if request.query == "one" {
            tokio::task::yield_now().await;
            return Err(WebError::new("WEB_PROVIDER_ERROR", "first search failed"));
        }
        while !cancelled() {
            tokio::task::yield_now().await;
        }
        self.sibling_cancelled.store(true, Ordering::SeqCst);
        self.sibling_settled.store(true, Ordering::SeqCst);
        Err(WebError::new("WEB_ABORTED", "sibling search stopped"))
    }
}

fn source(url: &str, title: Option<&str>) -> WebSearchSource {
    WebSearchSource {
        url: url.into(),
        title: title.map(str::to_string),
        snippet: None,
        published_at: None,
    }
}

async fn setup(config: serde_json::Value) -> (Context, Arc<ToolRuntime>, Arc<StubWeb>) {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, PromptConfig::default()).unwrap();
    let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
    let web = Arc::new(StubWeb::default());
    let service: Arc<dyn WebSearch> = web.clone();
    ctx.register_service(service);
    let fiber = ctx.plugin(Arc::new(ToolWebPlugin), arc(config));
    fiber.settle().await.unwrap();
    (ctx, tools, web)
}

fn input(args: serde_json::Value) -> ToolExecutionInput {
    ToolExecutionInput {
        call_id: CallId::new("web-search-test"),
        root_call_id: None,
        name: "web_search".into(),
        arguments: args,
        agent: None,
        parent: None,
        signal: Arc::new(|| false),
    }
}

fn text(result: &dsh_tools::ToolExecutionResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            dsh_llm::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

#[test]
fn validates_and_deduplicates_queries_after_the_bound_check() {
    assert_eq!(
        parse_search_args(&["one".into(), "one".into(), " two ".into()], 4).unwrap(),
        vec!["one", " two "]
    );
    assert!(
        parse_search_args(&[], 4)
            .unwrap_err()
            .contains("at least one query")
    );
    assert!(
        parse_search_args(&["one".into(), "two".into()], 1)
            .unwrap_err()
            .contains("at most 1 query")
    );
    assert!(
        parse_search_args(&["ok".into(), " ".into()], 4)
            .unwrap_err()
            .contains("non-empty string")
    );
}

#[test]
fn formats_the_node_search_output_contract() {
    let output = format_search_output(&WebSearchResult {
        content: Some("an answer".into()),
        sources: vec![
            WebSearchSource {
                url: "https://a.test/x".into(),
                title: Some("A".into()),
                snippet: Some("about a".into()),
                published_at: Some("2026-01-01".into()),
            },
            source("https://b.test/y", None),
        ],
        truncated: true,
    });
    assert!(output.contains("an answer"));
    assert!(output.contains("[A](https://a.test/x) — about a (2026-01-01)"));
    assert!(output.contains("[b.test](https://b.test/y)"));
    assert!(output.contains("Showing the first 2 sources"));
    assert!(output.contains("Cite the relevant URLs above as markdown links"));
}

#[tokio::test(flavor = "current_thread")]
async fn standard_registers_only_real_web_search_with_the_60_second_budget() {
    let (ctx, tools, _) =
        setup(serde_json::json!({ "fetch": false, "searchTimeoutMs": 60000 })).await;
    let search = tools.get("web_search", None).expect("web_search");
    assert_eq!(search.timeout_ms, Some(60_000));
    assert!(tools.get("web_fetch", None).is_none());
    assert_eq!(NAME, "tool-web");
    let schema = tools
        .schemas(None)
        .into_iter()
        .find(|schema| schema.name == "web_search")
        .unwrap();
    assert_eq!(
        schema.parameters["required"],
        serde_json::json!(["queries"])
    );
    assert_eq!(
        schema.parameters["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["queries"]
    );
    let prompt = ctx
        .get_typed::<Arc<SystemPrompt>>("systemPrompt", false)
        .unwrap();
    let assembled = prompt
        .assemble(&ctx, &AssembleContext::default())
        .await
        .unwrap();
    let joined = assembled
        .sections
        .iter()
        .map(|section| section.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("accepts 1–4 non-empty search queries"));
    assert!(joined.contains("Use the returned source snippets when available"));
    assert!(!joined.contains("Follow up with web_fetch"));
}

#[tokio::test(flavor = "current_thread")]
async fn configured_query_cap_is_exposed_in_the_schema_and_enforced_before_search() {
    let (_ctx, tools, web) = setup(serde_json::json!({
        "fetch": false,
        "searchMaxQueries": 2
    }))
    .await;
    let schema = tools
        .schemas(None)
        .into_iter()
        .find(|schema| schema.name == "web_search")
        .unwrap();
    assert!(schema.description.contains("1–2 queries"));
    assert!(
        schema.parameters["properties"]["queries"]["description"]
            .as_str()
            .unwrap()
            .contains("1–2 items")
    );
    let result = tools
        .execute(input(
            serde_json::json!({ "queries": ["one", "two", "three"] }),
        ))
        .await;
    assert!(result.is_error);
    assert!(text(&result).contains("at most 2 queries"));
    assert!(web.requests.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn executes_single_and_multi_query_searches_through_the_service() {
    let (_ctx, tools, web) =
        setup(serde_json::json!({ "fetch": false, "searchTimeoutMs": 60000 })).await;
    let single = tools
        .execute(input(serde_json::json!({ "queries": ["one"] })))
        .await;
    assert!(!single.is_error, "{}", text(&single));
    assert_eq!(
        single.value.as_ref().unwrap()["sources"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let merged = tools
        .execute(input(
            serde_json::json!({ "queries": ["one", "one", "two"] }),
        ))
        .await;
    assert!(!merged.is_error, "{}", text(&merged));
    assert_eq!(
        merged.value.as_ref().unwrap(),
        &serde_json::json!({
            "content": "### one\n\nanswer one\n\n### two\n\nanswer two",
            "sources": [
                { "url": "https://a.test", "title": "A" },
                { "url": "https://b.test", "title": "B" },
                { "url": "https://shared.test" }
            ],
            "truncated": false
        })
    );
    assert_eq!(
        web.requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| (&request.query, request.max_results))
            .collect::<Vec<_>>(),
        vec![
            (&"one".to_string(), Some(WEB_SEARCH_MAX_RESULTS)),
            (&"one".to_string(), Some(WEB_SEARCH_MAX_RESULTS)),
            (&"two".to_string(), Some(WEB_SEARCH_MAX_RESULTS)),
        ]
    );
    assert_eq!(WEB_SEARCH_MAX_QUERIES, 4);
}

#[tokio::test(flavor = "current_thread")]
async fn failed_multi_query_search_cancels_siblings_and_waits_for_settlement() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, PromptConfig::default()).unwrap();
    let tools = ToolRuntime::install(&ctx, Default::default()).unwrap();
    let sibling_cancelled = Arc::new(AtomicBool::new(false));
    let sibling_settled = Arc::new(AtomicBool::new(false));
    let web: Arc<dyn WebSearch> = Arc::new(FailingWeb {
        sibling_cancelled: sibling_cancelled.clone(),
        sibling_settled: sibling_settled.clone(),
    });
    ctx.register_service(web);
    let fiber = ctx.plugin(
        Arc::new(ToolWebPlugin),
        arc(serde_json::json!({ "fetch": false })),
    );
    fiber.settle().await.unwrap();

    let result = tools
        .execute(input(serde_json::json!({ "queries": ["one", "two"] })))
        .await;
    assert!(result.is_error);
    assert!(text(&result).contains("first search failed"));
    assert!(sibling_cancelled.load(Ordering::SeqCst));
    assert!(sibling_settled.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "current_thread")]
async fn caps_merged_sources_and_projects_replayable_web_presentation() {
    let (_ctx, tools, _) = setup(serde_json::json!({
        "fetch": false,
        "searchMaxResults": 2,
        "searchTimeoutMs": 60000
    }))
    .await;
    let result = tools
        .execute(input(serde_json::json!({ "queries": ["one", "two"] })))
        .await;
    assert!(!result.is_error, "{}", text(&result));
    assert_eq!(
        result.value.as_ref().unwrap(),
        &serde_json::json!({
            "content": "### one\n\nanswer one\n\n### two\n\nanswer two",
            "sources": [
                { "url": "https://a.test", "title": "A" },
                { "url": "https://b.test", "title": "B" }
            ],
            "truncated": true
        })
    );
    assert!(text(&result).contains("Showing the first 2 sources"));
    assert_eq!(
        result.meta.as_ref().unwrap(),
        &serde_json::json!({
            "answer": "### one\n\nanswer one\n\n### two\n\nanswer two",
            "sources": [
                { "url": "https://a.test", "title": "A" },
                { "url": "https://b.test", "title": "B" }
            ],
            "truncated": true
        })
    );
    let definition = tools.get("web_search", None).unwrap();
    let view = (definition.present_result.as_ref().unwrap())(
        &serde_json::json!({ "queries": ["one", "two"] }),
        &dsh_tools::ToolResult {
            content: result.content.clone(),
            is_error: false,
            meta: result.meta.clone(),
        },
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(view).unwrap(),
        serde_json::json!({
            "card": "web",
            "kind": "search",
            "title": "one, two",
            "answer": "### one\n\nanswer one\n\n### two\n\nanswer two",
            "sources": [
                { "url": "https://a.test", "title": "A" },
                { "url": "https://b.test", "title": "B" }
            ],
            "truncated": true
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn standard_meta_omits_absent_answer_and_invalid_args_never_reach_web() {
    let (_ctx, tools, web) = setup(serde_json::json!({ "fetch": false })).await;
    let empty = tools
        .execute(input(serde_json::json!({ "queries": ["empty"] })))
        .await;
    assert_eq!(
        empty.meta.as_ref().unwrap(),
        &serde_json::json!({ "sources": [], "truncated": false })
    );
    let before = web.requests.lock().unwrap().len();
    let invalid = tools
        .execute(input(serde_json::json!({ "queries": [] })))
        .await;
    assert!(invalid.is_error);
    assert!(text(&invalid).contains("at least one query"));
    assert_eq!(web.requests.lock().unwrap().len(), before);
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_invalid_config_at_plugin_load() {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, PromptConfig::default()).unwrap();
    ToolRuntime::install(&ctx, Default::default()).unwrap();
    let web: Arc<dyn WebSearch> = Arc::new(StubWeb::default());
    ctx.register_service(web);
    let fiber = ctx.plugin(
        Arc::new(ToolWebPlugin),
        arc(serde_json::json!({ "searchMaxQueries": 0 })),
    );
    let error = fiber.settle().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("searchMaxQueries must be a positive integer")
    );
}

#[test]
fn typed_config_defaults_match_node_origin_master() {
    assert_eq!(Config::default().search_max_results, 8);
    assert_eq!(Config::default().search_max_queries, 4);
    assert_eq!(Config::default().search_timeout_ms, 30_000);
    assert!(Config::default().search);
    assert!(Config::default().fetch);
}
