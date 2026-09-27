//! Model tools over the local knowledge bases, and the runtime-context note
//! that tells the model which bases exist.

use std::sync::Arc;

use serde_json::{Value, json};

use dsh_system_prompt::{PromptContext, PromptText, SystemPrompt};
use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};

use crate::store::{KnowledgeError, KnowledgeStore};

pub const SEARCH_TOOL: &str = "knowledge_search";
pub const LIST_TOOL: &str = "knowledge_list";

const SEARCH_DESCRIPTION: &str = "Search the user's local knowledge bases (documents they imported: notes, manuals, Markdown, Office files, PDFs) and return the most relevant passages with their document names. Use it before answering questions the listed knowledge bases may cover, and cite the document names you rely on. Query with the key words of the question; Chinese and English both work. Optionally limit to one knowledge base by its name or id.";
const LIST_DESCRIPTION: &str = "List the user's local knowledge bases with their ids, whether they are enabled for search, and document counts. Give a base name or id to also list its documents.";

fn output() -> ToolOutputDefinition {
    ToolOutputDefinition {
        schema: json!({"type": "object"}),
        render: Arc::new(|_, value| {
            Ok(vec![dsh_llm::ContentBlock::Text {
                text: value.to_string(),
            }])
        }),
        presentation_meta: None,
    }
}

fn tool_error(error: KnowledgeError) -> ToolBodyError {
    ToolBodyError::plain(format!("{}: {}", error.code, error.message))
}

pub(crate) fn search_parameters() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["query"],
        "properties": {
            "query": {"type": "string", "minLength": 1, "maxLength": 500},
            "base": {"type": "string", "description": "Knowledge base name or id; omit to search every enabled base"},
            "limit": {"type": "integer", "minimum": 1, "maximum": 20, "description": "Passages to return, default 6"}
        }
    })
}

pub(crate) fn list_parameters() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "base": {"type": "string", "description": "Knowledge base name or id whose documents to list"}
        }
    })
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, KnowledgeError> + Send + 'static,
) -> Result<T, ToolBodyError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| ToolBodyError::plain(format!("知识库任务失败：{error}")))?
        .map_err(tool_error)
}

pub(crate) fn run_search(store: &KnowledgeStore, args: &Value) -> Result<Value, KnowledgeError> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(6)
        .clamp(1, 20) as usize;
    let scope = match args
        .get("base")
        .and_then(Value::as_str)
        .filter(|base| !base.trim().is_empty())
    {
        Some(base) => {
            let base = store.find_base(base)?;
            if !base.enabled {
                return Err(KnowledgeError::new(
                    "disabled",
                    format!("知识库“{}”已停用", base.name),
                ));
            }
            Some(vec![base.id])
        }
        None => None,
    };
    let hits = store.search(query, scope.as_deref(), limit)?;
    let results: Vec<Value> = hits
        .iter()
        .map(|hit| {
            json!({
                "knowledgeBase": hit.base_name,
                "document": hit.document_name,
                "documentId": hit.document_id,
                "chunk": hit.chunk,
                "text": hit.text,
            })
        })
        .collect();
    let mut out = json!({"query": query, "results": results});
    if hits.is_empty() {
        out["note"] = json!(
            "No passage matched. Try other key words, or answer from general knowledge and say the knowledge bases had nothing relevant."
        );
    }
    Ok(out)
}

pub(crate) fn run_list(store: &KnowledgeStore, args: &Value) -> Result<Value, KnowledgeError> {
    let bases = store.bases()?;
    let mut out = json!({"knowledgeBases": bases});
    if let Some(base) = args
        .get("base")
        .and_then(Value::as_str)
        .filter(|base| !base.trim().is_empty())
    {
        let base = store.find_base(base)?;
        let documents = store.documents(&base.id)?;
        out["documents"] = json!(documents
            .iter()
            .map(|document| json!({"id": document.id, "name": document.name, "chunks": document.chunk_count, "chars": document.chars}))
            .collect::<Vec<_>>());
    }
    Ok(out)
}

/// The runtime-context note naming the enabled, non-empty bases, or an empty
/// string when there is nothing to search.
pub fn context_note(store: &KnowledgeStore) -> String {
    let Ok(bases) = store.bases() else {
        return String::new();
    };
    let lines: Vec<String> = bases
        .iter()
        .filter(|base| base.enabled && base.document_count > 0)
        .take(20)
        .map(|base| {
            // `{{` would be read as a prompt variable.
            let clean = |text: &str| text.replace(['{', '}'], "").replace('\n', " ");
            let description = clean(&base.description);
            let description: String = description.chars().take(120).collect();
            if description.is_empty() {
                format!(
                    "- {} ({} documents)",
                    clean(&base.name),
                    base.document_count
                )
            } else {
                format!(
                    "- {} ({} documents): {}",
                    clean(&base.name),
                    base.document_count,
                    description
                )
            }
        })
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "The user has local knowledge bases enabled for search:\n{}\nWhen a question may be covered by them, call `{SEARCH_TOOL}` first and cite the document names you use.",
        lines.join("\n")
    )
}

/// Register the knowledge tools and the runtime-context note.
pub fn register(ctx: &cordis::Context, store: Arc<KnowledgeStore>) -> Result<(), String> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .ok_or("缺少工具运行时")?;
    let search_store = store.clone();
    tools.register(
        ctx,
        ToolDefinition {
            name: SEARCH_TOOL.into(),
            description: SEARCH_DESCRIPTION.into(),
            parameters: search_parameters(),
            output: output(),
            timeout_ms: Some(30_000),
            is_concurrency_safe: Some(Arc::new(|_| true)),
            finalize_content: None,
            present_call: None,
            present_result: None,
            execute: Arc::new(move |args, _run| {
                let store = search_store.clone();
                let args = args.clone();
                Box::pin(async move { blocking(move || run_search(&store, &args)).await })
            }),
        },
    )?;
    let list_store = store.clone();
    tools.register(
        ctx,
        ToolDefinition {
            name: LIST_TOOL.into(),
            description: LIST_DESCRIPTION.into(),
            parameters: list_parameters(),
            output: output(),
            timeout_ms: Some(15_000),
            is_concurrency_safe: Some(Arc::new(|_| true)),
            finalize_content: None,
            present_call: None,
            present_result: None,
            execute: Arc::new(move |args, _run| {
                let store = list_store.clone();
                let args = args.clone();
                Box::pin(async move { blocking(move || run_list(&store, &args)).await })
            }),
        },
    )?;
    if let Some(prompt) = ctx.get_typed::<Arc<SystemPrompt>>("systemPrompt", false) {
        prompt.context(
            ctx,
            PromptContext {
                name: "knowledge:bases".into(),
                order: 86.0,
                text: PromptText::Provider(Arc::new(move |_| context_note(&store))),
            },
        );
    }
    Ok(())
}
