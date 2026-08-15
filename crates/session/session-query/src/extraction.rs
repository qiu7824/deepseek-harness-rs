//! First-party semantic text extraction for session-query consumers. Rust
//! port of `packages/session-query/session-query/src/extraction.ts`.

use dsh_session::SessionEvent;

/// Extract searchable semantic text from one first-party session event.
/// Structural boundaries, raw stream chunks, request envelopes, and unknown
/// declaration-merged events contribute no text.
pub fn extract_session_event_text(event: &SessionEvent) -> String {
    match event.type_.as_str() {
        "user/message" => content_text(event.data.get("content")),
        "assistant/message" => {
            let Some(message) = event.data.get("message") else {
                return String::new();
            };
            content_text(message.get("content"))
        }
        "tool/call" => join_text(&[
            event.data.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            event
                .data
                .get("arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        ]),
        "tool/result" => {
            let message = event.data.get("message");
            let content = message.and_then(|m| m.get("content"));
            join_text(&[
                content_text(content),
                event
                    .data
                    .get("error")
                    .and_then(|e| e.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                event
                    .data
                    .get("error")
                    .and_then(|e| e.get("code"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            ])
        }
        "todo/write" => {
            let parts: Vec<String> = event
                .data
                .get("todos")
                .and_then(|todos| todos.as_array())
                .map(|todos| {
                    todos
                        .iter()
                        .flat_map(|todo| {
                            vec![
                                todo.get("status").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                todo.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            ]
                        })
                        .collect()
                })
                .unwrap_or_default();
            join_text(&parts)
        }
        "turn/end" => turn_end_text(
            event
                .data
                .get("reason")
                .unwrap_or(&serde_json::Value::Null),
        ),
        "turn/start" | "step/start" | "step/end" | "assistant/chunk" | "request/header" => {
            String::new()
        }
        // Unknown events remain non-searchable until a concrete first-party
        // consumer defines semantics.
        _ => String::new(),
    }
}

fn turn_end_text(reason: &serde_json::Value) -> String {
    let kind = reason.get("kind").and_then(|value| value.as_str()).unwrap_or("");
    match kind {
        "error" => {
            let message = reason
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(|value| value.as_str())
                .unwrap_or("");
            join_text(&["error".to_string(), message.to_string()])
        }
        "aborted" => "aborted".to_string(),
        "max-tokens" | "interrupted" => kind.to_string(),
        _ => String::new(),
    }
}

fn content_text(content: Option<&serde_json::Value>) -> String {
    let blocks = content.and_then(|content| content.as_array());
    let mut parts: Vec<String> = Vec::new();
    if let Some(blocks) = blocks {
        for block in blocks {
            parts.extend(block_text(block));
        }
    }
    join_text(&parts)
}

fn block_text(block: &serde_json::Value) -> Vec<String> {
    match block.get("type").and_then(|value| value.as_str()) {
        Some("text") => vec![block.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string()],
        Some("reasoning") => Vec::new(),
        Some("tool-call") => vec![
            block.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            block.get("arguments").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        ],
        Some("tool-result") => {
            let mut parts: Vec<String> = Vec::new();
            if let Some(inner) = block.get("content").and_then(|content| content.as_array()) {
                for block in inner {
                    parts.extend(block_text(block));
                }
            }
            parts
        }
        _ => Vec::new(),
    }
}

fn join_text(parts: &[String]) -> String {
    let trimmed: Vec<&str> = parts
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect();
    trimmed.join("\n")
}
