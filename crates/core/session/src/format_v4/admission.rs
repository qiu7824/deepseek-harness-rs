use super::{
    messages::validate_v4_message_data,
    wire::{MAX_SAFE_INTEGER, count, object},
};
use serde_json::Value;

fn text<'a>(value: &'a Value, field: &str, nonempty: bool) -> Result<&'a str, String> {
    value
        .as_str()
        .filter(|s| !nonempty || !s.is_empty())
        .ok_or_else(|| {
            format!(
                "{field} must be a string{}",
                if nonempty { " with content" } else { "" }
            )
        })
}
fn positive(value: &Value, field: &str) -> Result<(), String> {
    if count(value, field)? == 0 {
        Err(format!("{field} must be positive"))
    } else {
        Ok(())
    }
}

fn messages(
    kind: &str,
    data: &Value,
    mut visit: impl FnMut(&Value) -> Result<(), String>,
) -> Result<(), String> {
    match kind {
        "user/message" => visit(data)?,
        "system/message" | "developer/message" | "assistant/message" | "tool/result" => {
            visit(&data["message"])?
        }
        "agent/inbox/spliced" | "session/title-llm-request" => {
            let key = if kind == "agent/inbox/spliced" {
                "inserted"
            } else {
                "messages"
            };
            if let Some(values) = data[key].as_array() {
                for value in values {
                    visit(value)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn tool_changes(content: &Value, developer: bool) -> Result<(), String> {
    if let Some(blocks) = content.as_array() {
        for block in blocks {
            tool_change(block, developer)?;
        }
    }
    Ok(())
}

fn tool_change(block: &Value, developer: bool) -> Result<(), String> {
    if !matches!(
        block["type"].as_str(),
        Some("tool-addition" | "tool-removal")
    ) {
        return Ok(());
    }
    if !developer {
        return Err("tool-change blocks require developer role".into());
    }
    text(&block["toolName"], "toolName", true)?;
    if block["type"] == "tool-addition" && block.get("tool").is_some() {
        return Err("tool-addition must omit inline definitions".into());
    }
    Ok(())
}

fn retired_content(content: &Value) -> Result<(), String> {
    if content
        .as_array()
        .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool-result"))
    {
        return Err("V4 refuses retired tool-result content wrappers".into());
    }
    Ok(())
}

fn image(value: &Value) -> Result<(), String> {
    object(value, "image attachment")?;
    text(&value["attachmentId"], "attachmentId", true)?;
    if !matches!(
        value["mediaType"].as_str(),
        Some("image/png" | "image/jpeg" | "image/webp" | "image/gif")
    ) {
        return Err("image mediaType is unsupported".into());
    }
    count(&value["bytes"], "image bytes")?;
    positive(&value["width"], "image width")?;
    positive(&value["height"], "image height")?;
    if let Some(name) = value.get("name") {
        text(name, "image name", false)?;
    }
    if let Some(original) = value.get("originalDimensions") {
        object(original, "originalDimensions")?;
        positive(&original["width"], "original width")?;
        positive(&original["height"], "original height")?;
    }
    Ok(())
}

fn content_fields(content: &Value) -> Result<(), String> {
    for block in content.as_array().ok_or("content must be an array")? {
        object(block, "content block")?;
        text(&block["type"], "content type", true)?;
        match block["type"].as_str().unwrap() {
            "text" | "reasoning" => {
                text(&block["text"], "content text", false)?;
            }
            "tool-call" => {
                text(&block["id"], "tool-call id", true)?;
                text(&block["name"], "tool-call name", true)?;
                text(&block["arguments"], "tool-call arguments", false)?;
            }
            "image" => image(&block["attachment"])?,
            "tool-result" => return Err("V4 refuses retired tool-result content wrappers".into()),
            _ => {}
        }
    }
    Ok(())
}

fn fork_result(row: &Value) -> Result<(), String> {
    if row["type"] != "tool/result" || row["data"]["error"]["code"] != "TOOL_NOT_STARTED" {
        return Ok(());
    }
    let data = &row["data"];
    let message = &data["message"];
    let Some(id) = message["id"]
        .as_str()
        .filter(|id| id.starts_with("forked-tool-result-"))
    else {
        return Ok(());
    };
    let call = text(&message["source"]["callId"], "fork callId", false)?;
    let prefix = format!("forked-tool-result-{call}-");
    let suffix = id
        .strip_prefix(&prefix)
        .ok_or("fork result identity has a different callId")?;
    let sequence =
        canonical_suffix(suffix).ok_or("fork result requires a canonical sequence suffix")?;
    let seq = count(&row["seq"], "fork result seq")?;
    let replacement = row["surfaceOp"]["op"] == "replace";
    if if replacement {
        sequence >= seq
    } else {
        sequence != seq
    } {
        return Err("fork result identity has the wrong sequence".into());
    }
    if data["error"]["name"] != "ToolNotStartedError"
        || message["role"] != "tool"
        || message["isError"] != true
        || message["toolCallId"] != call
        || message["source"]["kind"] != "tool"
    {
        return Err("invalid fork not-started result".into());
    }
    if replacement {
        let refs = row["sourceEventSeqs"]
            .as_array()
            .ok_or("fork replacement requires source reference")?;
        if refs.len() != 1 || count(&refs[0], "fork source seq")? != sequence {
            return Err("fork replacement must cite its original result".into());
        }
    } else if row["surfaceOp"] != "append" || row.get("sourceEventSeqs").is_some() {
        return Err("fork append must not carry source references".into());
    }
    let content = message["content"]
        .as_array()
        .filter(|c| c.len() == 1)
        .ok_or("fork result requires exactly one text block")?;
    if content[0]["type"] != "text" || !content[0]["text"].is_string() {
        return Err("fork result requires text content".into());
    }
    Ok(())
}

pub(super) fn canonical_suffix(suffix: &str) -> Option<u64> {
    if suffix.is_empty()
        || suffix.len() > 1 && suffix.starts_with('0')
        || !suffix.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    suffix
        .parse::<u64>()
        .ok()
        .filter(|n| *n <= MAX_SAFE_INTEGER)
}

/// Generation-owned fields that cannot be hidden by suffix recovery. Ordinary
/// incomplete message fields are left to the physical decoder's recovery rules.
pub fn admit_v4_structural_fields(row: &Value) -> Result<(), String> {
    let kind = row["type"].as_str().ok_or("event type must be a string")?;
    let data = &row["data"];
    if ["tool/code-dispatch", "tool/code-dispatch-start"].contains(&kind)
        && row["ignorable"] != true
    {
        return Err("V4 refuses retired required PTC tags".into());
    }
    if kind == "request/header" {
        let header = object(&data["header"], "request header")?;
        if header.contains_key("system") {
            return Err("V4 refuses retired header.system".into());
        }
        if let Some(tools) = header.get("tools").and_then(Value::as_array) {
            for tool in tools {
                if tool.get("deferLoading").is_some_and(|v| v != true) {
                    return Err("deferLoading must be true when present".into());
                }
            }
        }
    }
    messages(kind, data, |message| {
        if message["source"]["kind"] == "plugin" {
            return Err("V4 refuses retired plugin source wrappers".into());
        }
        if kind != "developer/message" && message["role"] == "developer" {
            return Err("developer role requires developer/message".into());
        }
        retired_content(&message["content"])?;
        tool_changes(&message["content"], kind == "developer/message")
    })?;
    if kind == "developer/message" || kind == "system/message" {
        validate_v4_message_data(kind, data, row["ignorable"].as_bool())?;
        positive(&data["turn"], "message turn")?;
        positive(&data["step"], "message step")?;
        if kind == "developer/message" {
            let additions = data["message"]["content"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b["type"] == "tool-addition");
            if additions {
                count(&data["headerSeq"], "developer headerSeq")?;
            } else if data.get("headerSeq").is_some() {
                return Err("developer headerSeq requires tool additions".into());
            }
        } else {
            content_fields(&data["message"]["content"])?;
        }
    }
    if kind == "tool/result" {
        validate_v4_message_data(kind, data, row["ignorable"].as_bool())?;
        fork_result(row)?;
    }
    for content in match kind {
        "compaction/summary" => [Some(&data["summary"]), data.get("rawOutput")],
        "tool/ptc-dispatch" => [Some(&data["content"]), None],
        "team/message/queued" => [Some(&data["message"]["content"]), None],
        _ => [None, None],
    }
    .into_iter()
    .flatten()
    {
        retired_content(content)?;
        tool_changes(content, false)?;
    }
    let chunk = |value: &Value| -> Result<(), String> {
        if value["type"] == "block-end" {
            if value["block"]["type"] == "tool-result" {
                return Err("V4 refuses retired tool-result content wrappers".into());
            }
            tool_change(&value["block"], false)?;
        }
        if value["type"] == "block-start"
            && matches!(
                value["blockType"].as_str(),
                Some("tool-result" | "tool-addition" | "tool-removal")
            )
        {
            return Err("stream contains a retired or misplaced tool block".into());
        }
        Ok(())
    };
    if kind == "assistant/chunk" {
        chunk(&data["chunk"])?;
    }
    if matches!(kind, "assistant/message" | "assistant/attempt") {
        if let Some(stream) = data["stream"].as_array() {
            for entry in stream {
                if entry["type"] == "chunk" {
                    chunk(&entry["chunk"])?;
                }
            }
        }
    }
    Ok(())
}

/// Complete message admission for an accepted logical row. This is deliberately
/// separate from structural admission before recoverable physical decoding.
pub fn validate_v4_row_fields(row: &Value) -> Result<(), String> {
    admit_v4_structural_fields(row)?;
    let kind = row["type"].as_str().ok_or("event type must be a string")?;
    let data = &row["data"];
    validate_v4_message_data(kind, data, row["ignorable"].as_bool())?;
    messages(kind, data, |message| content_fields(&message["content"]))?;
    if kind == "assistant/message" {
        if data["message"]["source"]["kind"] != "model" {
            return Err("assistant message requires model source".into());
        }
        text(
            &data["message"]["source"]["provider"],
            "model provider",
            true,
        )?;
        text(&data["message"]["source"]["model"], "model id", true)?;
    }
    Ok(())
}
