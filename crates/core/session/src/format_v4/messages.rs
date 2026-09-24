use serde_json::{Map, Value, json};

use crate::SessionEvent;

type Object = Map<String, Value>;

fn object(value: &Value) -> Result<&Object, String> {
    value
        .as_object()
        .ok_or_else(|| "message value must be an object".into())
}

fn object_mut(value: &mut Value) -> Result<&mut Object, String> {
    value
        .as_object_mut()
        .ok_or_else(|| "message value must be an object".into())
}

fn nonempty(value: Option<&Value>, field: &str) -> Result<(), String> {
    if value
        .and_then(Value::as_str)
        .is_some_and(|text| !text.is_empty())
    {
        Ok(())
    } else {
        Err(format!("{field} must be a nonempty string"))
    }
}

fn visit_messages(
    event: &mut SessionEvent,
    mut visit: impl FnMut(&mut Object) -> Result<(), String>,
) -> Result<(), String> {
    match event.type_.as_str() {
        "user/message" => visit(object_mut(&mut event.data)?)?,
        "developer/message" | "system/message" | "assistant/message" | "tool/result" => {
            let message = object_mut(&mut event.data)?
                .get_mut("message")
                .ok_or("missing message")?;
            visit(object_mut(message)?)?;
        }
        "agent/inbox/spliced" | "session/title-llm-request" => {
            let key = if event.type_ == "agent/inbox/spliced" {
                "inserted"
            } else {
                "messages"
            };
            let messages = object_mut(&mut event.data)?
                .get_mut(key)
                .and_then(Value::as_array_mut)
                .ok_or("message array required")?;
            for message in messages {
                visit(object_mut(message)?)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn inspect_message_slots(
    kind: &str,
    data: &Value,
    mut inspect: impl FnMut(&Object) -> Result<(), String>,
) -> Result<(), String> {
    match kind {
        "user/message" => inspect(object(data)?)?,
        "developer/message" | "system/message" | "assistant/message" | "tool/result" => {
            inspect(object(data.get("message").ok_or("missing message")?)?)?;
        }
        "agent/inbox/spliced" | "session/title-llm-request" => {
            let key = if kind == "agent/inbox/spliced" {
                "inserted"
            } else {
                "messages"
            };
            for message in data
                .get(key)
                .and_then(Value::as_array)
                .ok_or("message array required")?
            {
                inspect(object(message)?)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn producer(plugin: &str, role: Option<&str>) -> String {
    match plugin {
        "@deepseek-ai/dsh-system-prompt" if role == Some("system") => "system-prompt".into(),
        "@deepseek-ai/dsh-system-prompt" => "runtime-context".into(),
        "compact" => "compact-checkpoint".into(),
        "tools-code-mode" | "tools-ptc" => "ptc-mode".into(),
        "dsh-compaction-basic" => "compact-basic".into(),
        "agent-instructions"
        | "session-reference"
        | "team-message"
        | "goal"
        | "skill-invocation"
        | "skill-catalog"
        | "coordinator"
        | "subagent-report"
        | "subagent-settled"
        | "webhook"
        | "agent-message"
        | "model-selection"
        | "plan-mode"
        | "time-context"
        | "tmux-context"
        | "user-approval"
        | "repeat-tool-reminder"
        | "tool-cordis"
        | "cordis-host-runner"
        | "tool-goal"
        | "tool-jobs"
        | "hooks-codex"
        | "hooks-claude-code"
        | "schedule"
        | "dsh-session-title-llm" => plugin.into(),
        _ => format!("plugin:{plugin}"),
    }
}

fn rewrite_source(message: &mut Object) -> Result<(), String> {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let source = object_mut(message.get_mut("source").ok_or("missing message source")?)?;
    nonempty(source.get("kind"), "source.kind")?;
    if source["kind"] == "plugin" {
        let plugin = source
            .get("plugin")
            .and_then(Value::as_str)
            .ok_or("plugin source requires a string plugin")?;
        let kind = producer(plugin, role.as_deref());
        source.remove("plugin");
        source.insert("kind".into(), Value::String(kind));
    }
    Ok(())
}

fn lift_tool_result(event: &mut SessionEvent) -> Result<(), String> {
    if event.type_ != "tool/result" {
        return Ok(());
    }
    let data = object_mut(&mut event.data)?;
    let message = object(data.get("message").ok_or("missing tool result message")?)?;
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return Err("V3 tool result must have a user-role wrapper".into());
    }
    nonempty(message.get("id"), "message.id")?;
    let source = object(message.get("source").ok_or("missing tool result source")?)?;
    nonempty(source.get("callId"), "source.callId")?;
    let blocks = message
        .get("content")
        .and_then(Value::as_array)
        .filter(|blocks| blocks.len() == 1)
        .ok_or("tool result requires exactly one wrapper")?;
    let wrapper = object(&blocks[0])?;
    if source.get("kind").and_then(Value::as_str) != Some("tool")
        || wrapper.get("type").and_then(Value::as_str) != Some("tool-result")
        || wrapper.get("toolCallId") != source.get("callId")
    {
        return Err("tool result wrapper must match its tool source".into());
    }
    let content = wrapper
        .get("content")
        .and_then(Value::as_array)
        .ok_or("tool result content must be an array")?;
    if content
        .iter()
        .any(|block| block.get("type").and_then(Value::as_str) == Some("tool-result"))
    {
        return Err("nested historical tool results require an explicit converter; original generation must be retained".into());
    }
    if wrapper
        .get("isError")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err("tool result isError must be boolean when present".into());
    }
    let call_id = source["callId"].clone();
    let Value::Object(mut message) = data.remove("message").expect("validated message") else {
        unreachable!()
    };
    let Value::Array(mut blocks) = message.remove("content").expect("validated content") else {
        unreachable!()
    };
    let Value::Object(mut wrapper) = blocks.pop().expect("one validated wrapper") else {
        unreachable!()
    };
    let mut target = Object::from_iter([
        ("id".into(), message.remove("id").unwrap()),
        ("role".into(), json!("tool")),
        ("source".into(), message.remove("source").unwrap()),
        ("toolCallId".into(), call_id),
        ("content".into(), wrapper.remove("content").unwrap()),
    ]);
    message.remove("role");
    wrapper.remove("type");
    wrapper.remove("toolCallId");
    if let Some(value) = wrapper.remove("isError") {
        target.insert("isError".into(), value);
    }
    for (key, value) in message {
        target.insert(format!("plugin:message:{key}"), value);
    }
    for (key, value) in wrapper {
        target.insert(format!("plugin:result:{key}"), value);
    }
    data.insert("message".into(), Value::Object(target));
    Ok(())
}

fn block_type(value: &str) -> String {
    if [
        "text",
        "reasoning",
        "image",
        "file",
        "tool-call",
        "tool-result",
    ]
    .contains(&value)
    {
        value.into()
    } else {
        format!("plugin:{value}")
    }
}

fn convert_block(value: &mut Value) -> Result<(), String> {
    let block = object_mut(value)?;
    let kind = block
        .get("type")
        .and_then(Value::as_str)
        .ok_or("content requires a string type")?;
    let converted = block_type(kind);
    block.insert("type".into(), json!(converted));
    Ok(())
}

fn convert_content(value: &mut Value) -> Result<(), String> {
    for block in value.as_array_mut().ok_or("content must be an array")? {
        convert_block(block)?;
    }
    Ok(())
}

fn convert_chunk(value: &mut Value) -> Result<(), String> {
    let Some(chunk) = value.as_object_mut() else {
        return Ok(());
    };
    match chunk.get("type").and_then(Value::as_str) {
        Some("block-end") => {
            convert_block(chunk.get_mut("block").ok_or("block-end requires a block")?)?
        }
        Some("block-start") => {
            let original = chunk
                .get("blockType")
                .and_then(Value::as_str)
                .ok_or("block-start requires blockType")?;
            let converted = block_type(original);
            chunk.insert("blockType".into(), json!(converted));
        }
        _ => {}
    }
    Ok(())
}

/// Convert only the declared V3 message/content slots, preserving opaque JSON
/// in tool arguments, schemas, replay state and extension-owned fields.
pub fn convert_v3_event_messages(mut event: SessionEvent) -> Result<SessionEvent, String> {
    visit_messages(&mut event, rewrite_source)?;
    lift_tool_result(&mut event)?;
    visit_messages(&mut event, |message| {
        convert_content(
            message
                .get_mut("content")
                .ok_or("missing message content")?,
        )
    })?;
    let Some(data) = event.data.as_object_mut() else {
        return Ok(event);
    };
    match event.type_.as_str() {
        "compaction/summary" => {
            convert_content(
                data.get_mut("summary")
                    .ok_or("missing compaction summary")?,
            )?;
            if let Some(raw) = data.get_mut("rawOutput") {
                convert_content(raw)?;
            }
        }
        "tool/ptc-dispatch" => {
            convert_content(data.get_mut("content").ok_or("missing PTC content")?)?
        }
        "team/message/queued" => {
            if let Some(message) = data.get_mut("message").and_then(Value::as_object_mut) {
                convert_content(
                    message
                        .get_mut("content")
                        .ok_or("missing team message content")?,
                )?;
            }
        }
        "request/header" => {
            if let Some(tools) = data
                .get("header")
                .and_then(|header| header.get("tools"))
                .and_then(Value::as_array)
            {
                if tools.iter().any(|tool| {
                    tool.as_object()
                        .is_some_and(|tool| tool.contains_key("deferLoading"))
                }) {
                    return Err("V3 tool schema contains V4-only deferLoading; original generation must be retained".into());
                }
            }
        }
        "assistant/chunk" => {
            // The Rust V3 writer stored live chunks as separate durable rows.
            if let Some(chunk) = data.get_mut("chunk") {
                convert_chunk(chunk)?;
            }
        }
        _ => {}
    }
    if ["assistant/message", "assistant/attempt"].contains(&event.type_.as_str()) {
        if let Some(stream) = data.get_mut("stream").and_then(Value::as_array_mut) {
            for entry in stream {
                if entry.get("type").and_then(Value::as_str) == Some("chunk") {
                    if let Some(chunk) = entry.get_mut("chunk") {
                        convert_chunk(chunk)?;
                    }
                }
            }
        }
    }
    Ok(event)
}

fn no_result_wrappers(value: &Value) -> Result<(), String> {
    for block in value.as_array().ok_or("content must be an array")? {
        let block = object(block)?;
        nonempty(block.get("type"), "content.type")?;
        if block["type"] == "tool-result" {
            return Err("V4 content must not contain a retired tool-result wrapper".into());
        }
    }
    Ok(())
}

/// Mandatory native message/source admission before physical recovery may
/// discard a malformed suffix. This is not whole-artifact lifecycle validation.
pub fn validate_v4_message_fields(event: &SessionEvent) -> Result<(), String> {
    validate_v4_message_data(&event.type_, &event.data, event.ignorable)
}

pub(super) fn validate_v4_message_data(
    kind: &str,
    data: &Value,
    ignorable: Option<bool>,
) -> Result<(), String> {
    if ["tool/code-dispatch", "tool/code-dispatch-start"].contains(&kind) && ignorable != Some(true)
    {
        return Err("V4 refuses required retired PTC event tags".into());
    }
    if kind == "request/header" {
        let header = object(
            data.get("header")
                .ok_or("request/header requires a header")?,
        )?;
        if header.contains_key("system") {
            return Err("V4 request/header refuses retired header.system".into());
        }
    }
    inspect_message_slots(kind, data, |message| {
        nonempty(message.get("id"), "message.id")?;
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .ok_or("missing message role")?;
        if !["system", "developer", "user", "assistant", "tool"].contains(&role) {
            return Err("invalid V4 message role".into());
        }
        let expected = match kind {
            "user/message" => Some("user"),
            "system/message" => Some("system"),
            "developer/message" => Some("developer"),
            "assistant/message" => Some("assistant"),
            "tool/result" => Some("tool"),
            _ => None,
        };
        if expected.is_some_and(|expected| expected != role) {
            return Err("message role does not match its event".into());
        }
        let source = object(message.get("source").ok_or("missing message source")?)?;
        nonempty(source.get("kind"), "source.kind")?;
        if source["kind"] == "plugin" {
            return Err("V4 requires a producer-owned source kind".into());
        }
        no_result_wrappers(message.get("content").ok_or("missing message content")?)
    })?;
    match kind {
        "tool/result" => {
            let data = object(data)?;
            let message = object(data.get("message").ok_or("missing tool message")?)?;
            let source = object(message.get("source").ok_or("missing tool source")?)?;
            if message.get("role").and_then(Value::as_str) != Some("tool")
                || source.get("kind").and_then(Value::as_str) != Some("tool")
            {
                return Err("V4 tool/result requires tool role and tool source".into());
            }
            nonempty(message.get("toolCallId"), "message.toolCallId")?;
            if message.get("toolCallId") != source.get("callId") {
                return Err("V4 tool/result call ids disagree".into());
            }
            if message.get("isError").is_some_and(|v| !v.is_boolean()) {
                return Err("isError must be boolean when present".into());
            }
            if data.contains_key("error") && message.get("isError") != Some(&Value::Bool(true)) {
                return Err("error metadata requires isError true".into());
            }
        }
        "compaction/summary" => {
            no_result_wrappers(data.get("summary").ok_or("missing compaction summary")?)?;
            if let Some(raw) = data.get("rawOutput") {
                no_result_wrappers(raw)?;
            }
        }
        "tool/ptc-dispatch" => {
            no_result_wrappers(data.get("content").ok_or("missing PTC content")?)?
        }
        "team/message/queued" => {
            if let Some(message) = data.get("message").and_then(Value::as_object) {
                no_result_wrappers(
                    message
                        .get("content")
                        .ok_or("missing team message content")?,
                )?;
            }
        }
        _ => {}
    }
    let check_chunk = |chunk: &Value| -> Result<(), String> {
        if chunk.get("type").and_then(Value::as_str) == Some("block-start")
            && chunk.get("blockType").and_then(Value::as_str) == Some("tool-result")
            || chunk.get("type").and_then(Value::as_str) == Some("block-end")
                && chunk.pointer("/block/type").and_then(Value::as_str) == Some("tool-result")
        {
            return Err("V4 streams must not carry retired tool-result wrappers".into());
        }
        Ok(())
    };
    if kind == "assistant/chunk" {
        if let Some(chunk) = data.get("chunk") {
            check_chunk(chunk)?;
        }
    }
    if ["assistant/message", "assistant/attempt"].contains(&kind) {
        if let Some(stream) = data.get("stream").and_then(Value::as_array) {
            for entry in stream {
                if entry.get("type").and_then(Value::as_str) == Some("chunk") {
                    if let Some(chunk) = entry.get("chunk") {
                        check_chunk(chunk)?;
                    }
                }
            }
        }
    }
    Ok(())
}
