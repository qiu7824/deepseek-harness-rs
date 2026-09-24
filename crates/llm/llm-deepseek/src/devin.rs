//! Devin subscription protocol. Model identifiers come from the account catalog.
use super::devin_wire::{Encoder, Message};
use super::{GenerateOptions, LlmFailure, ResolvedDeepSeekOptions, StreamChunk, failure};
use dsh_llm::{MessageSource, Role, TokenUsage};
use serde_json::{Value, json};
use sha2::Digest;
use std::collections::{HashMap, HashSet};

pub const API: &str = "devin-agent";
pub const BASE_URL: &str = "https://server.codeium.com";
pub const CATALOG_PATH: &str = "/exa.api_server_pb.ApiServerService/GetCliModelConfigs";
pub(crate) const AUTH_PATH: &str = "/exa.auth_pb.AuthService/GetUserJwt";
pub(crate) const CHAT_PATH: &str = "/exa.api_server_pb.ApiServerService/GetChatMessage";

pub(crate) fn metadata(token: &str, user_jwt: &str, discovery: bool) -> Vec<u8> {
    let mut message = Encoder::default();
    // Native protocol identity selects the subscription endpoint surface;
    // the HTTP User-Agent continues to identify this application.
    message.text(1, if discovery { "chisel" } else { "devin-cli" });
    message.text(2, if discovery { "0.0.0-dev" } else { "3000.6.2" });
    message.text(7, if discovery { "0.0.0-dev" } else { "3000.6.2" });
    message.text(12, "chisel");
    if !discovery {
        message.text(28, "chisel");
    }
    let token = if token.starts_with("devin-session-token$") {
        token.to_string()
    } else {
        format!("devin-session-token${token}")
    };
    message.text(3, &token);
    message.text(4, "en");
    message.text(
        5,
        if cfg!(windows) {
            "windows"
        } else if cfg!(target_os = "macos") {
            "darwin"
        } else {
            "linux"
        },
    );
    message.number(6, 1);
    message.text(13, "DeepSeek-Harness-rs");
    message.text(21, user_jwt);
    if discovery {
        message.number(30, 8);
    }
    message.0
}

pub async fn discover_models(base: &str, token: &str) -> Result<Value, String> {
    super::devin_transport::catalog(base, token).await
}

pub(crate) fn catalog_from_bytes(bytes: &[u8]) -> Result<Value, String> {
    let root = Message::parse(bytes)?;
    let mut models = Vec::new();
    let mut seen = HashSet::new();
    for bytes in root.repeated(1)? {
        if models.len() >= 10_000 {
            return Err("Devin model catalog is too large".into());
        }
        let model = Message::parse(bytes)?;
        let info = model.bytes(23)?.map(Message::parse).transpose()?;
        if model.number(4)? != 0
            || info.as_ref().is_some_and(|info| {
                info.number(2).unwrap_or(1) != 0
                    || info.number(25).unwrap_or(1) != 0
                    || matches!(info.number(22).unwrap_or(6), 3 | 4 | 6)
            })
        {
            continue;
        }
        let id = model.text(22)?.trim();
        if id.is_empty() || id.len() > 512 || !seen.insert(id.to_owned()) {
            continue;
        }
        let context = model.number(18)?;
        let maximum = info
            .as_ref()
            .map(|info| info.number(13))
            .transpose()?
            .unwrap_or(0);
        let features = info
            .as_ref()
            .map(|info| info.bytes(6))
            .transpose()?
            .flatten()
            .map(Message::parse)
            .transpose()?;
        let images = if let Some(features) = features {
            features.number(11)? != 0
        } else {
            model.number(5)? != 0
        };
        let mut row = json!({"id":id,"name":if model.text(1)?.trim().is_empty(){id}else{model.text(1)?.trim()},
            "api":API,"context_window":if context>0&&context<=16_777_216{context}else{200_000},
            "max_output_tokens":if maximum>0&&maximum<=1_048_576{maximum}else{64_000},
            "input":if images{vec!["text","image"]}else{vec!["text"]},"available":true});
        let description = model.text(27)?;
        if !description.is_empty() {
            row["description"] = json!(description.chars().take(4096).collect::<String>());
        }
        models.push(row);
    }
    Ok(json!({"data":models}))
}

pub(crate) fn scope(
    options: &GenerateOptions,
    connection: &ResolvedDeepSeekOptions,
    token: &str,
) -> String {
    let account = connection
        .account_scope
        .clone()
        .unwrap_or_else(|| format!("{:x}", sha2::Sha256::digest(token.as_bytes())));
    format!(
        "{:x}",
        sha2::Sha256::digest(
            json!([
                options.provider,
                options.model,
                options.session_id,
                connection.base_url,
                account
            ])
            .to_string()
            .as_bytes()
        )
    )
}

fn uuid_key(seed: &str) -> String {
    let digest = sha2::Sha256::digest(seed.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    uuid::Uuid::from_bytes(bytes).to_string()
}

pub(crate) fn prepare_replay(chat: &mut Value, options: &GenerateOptions, scope: &str) -> String {
    let mut cascade = None;
    if let Some(messages) = chat.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages.iter_mut() {
            if let Some(object) = message.as_object_mut() {
                object.remove("devin_state");
            }
        }
        let results = options
            .messages
            .iter()
            .filter_map(dsh_llm::Message::as_tool_result);
        for (wire, (id, _, error)) in messages
            .iter_mut()
            .filter(|message| message["role"] == "tool")
            .zip(results)
        {
            if wire["tool_call_id"].as_str() == Some(id.as_str()) {
                wire["is_error"] = json!(error.unwrap_or(false));
            }
        }
        for (wire, original) in messages
            .iter_mut()
            .filter(|m| m["role"] == "assistant")
            .zip(
                options
                    .messages
                    .iter()
                    .filter(|m| m.role == Role::Assistant),
            )
        {
            if let MessageSource::Model {
                provider,
                model,
                replay_state: Some(state),
                ..
            } = &original.source
                && provider == &options.provider
                && model == &options.model
                && state["protocol"] == API
                && state["scopeHash"].as_str() == Some(scope)
            {
                wire["devin_state"] = state.clone();
                if let Some(id) = state["cascadeId"].as_str() {
                    cascade = Some(id.to_string());
                }
            }
        }
    }
    cascade.unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

fn content(message: &Value) -> Result<(String, Vec<Vec<u8>>), String> {
    let Some(content) = message.get("content") else {
        return Ok(Default::default());
    };
    if let Some(text) = content.as_str() {
        return Ok((text.to_string(), Vec::new()));
    }
    if content.is_null() {
        return Ok(Default::default());
    }
    let mut text = String::new();
    let mut images = Vec::new();
    for part in content.as_array().ok_or("invalid Devin message content")? {
        match part["type"].as_str() {
            Some("text") => text.push_str(part["text"].as_str().ok_or("invalid Devin text part")?),
            Some("image_url") => {
                let url = part
                    .pointer("/image_url/url")
                    .and_then(Value::as_str)
                    .ok_or("invalid Devin image")?;
                let (header, data) = url
                    .strip_prefix("data:")
                    .and_then(|v| v.split_once(","))
                    .ok_or("Devin images require inline prepared data")?;
                let media = header
                    .strip_suffix(";base64")
                    .ok_or("Devin images require base64")?;
                if !matches!(
                    media,
                    "image/png" | "image/jpeg" | "image/webp" | "image/gif"
                ) {
                    return Err("unsupported Devin image type".into());
                }
                let mut image = Encoder::default();
                image.text(1, data);
                image.text(2, media);
                images.push(image.0);
            }
            _ => return Err("unsupported Devin message content".into()),
        }
    }
    Ok((text, images))
}

pub(crate) fn chat_request(
    chat: &Value,
    token: &str,
    jwt: &str,
    cascade: &str,
) -> Result<Vec<u8>, String> {
    let mut request = Encoder::default();
    request.bytes(1, &metadata(token, jwt, false));
    let mut system = Vec::new();
    let mut leading = true;
    for (index, message) in chat["messages"]
        .as_array()
        .ok_or("Devin messages missing")?
        .iter()
        .enumerate()
    {
        let role = message["role"]
            .as_str()
            .ok_or("Devin message role missing")?;
        let (text, images) = content(message)?;
        if leading && role == "system" {
            system.push(text);
            continue;
        }
        leading = false;
        let source = match role {
            "user" | "developer" => 1,
            "assistant" => 2,
            "tool" => 4,
            "system" => 5,
            _ => return Err("unsupported Devin message role".into()),
        };
        let mut prompt = Encoder::default();
        let id = message
            .pointer("/devin_state/messageId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                let id = uuid_key(&format!("{cascade}\0{index}\0{role}"));
                if role == "assistant" {
                    format!("bot-{id}")
                } else {
                    id
                }
            });
        prompt.text(1, &id);
        prompt.number(2, source);
        prompt.text(3, &text);
        prompt.text(7, message["tool_call_id"].as_str().unwrap_or(""));
        prompt.number(9, u64::from(message["is_error"] == true));
        for image in images {
            prompt.bytes(10, &image)
        }
        if role == "assistant" {
            prompt.text(11, message["reasoning_content"].as_str().unwrap_or(""));
            prompt.text(
                12,
                message
                    .pointer("/devin_state/signature")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            );
            prompt.text(
                15,
                message
                    .pointer("/devin_state/outputId")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            );
            for call in message["tool_calls"].as_array().into_iter().flatten() {
                let mut tool = Encoder::default();
                tool.text(1, call["id"].as_str().ok_or("tool call id missing")?);
                tool.text(
                    2,
                    call.pointer("/function/name")
                        .and_then(Value::as_str)
                        .ok_or("tool name missing")?,
                );
                tool.text(
                    3,
                    call.pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .ok_or("tool arguments missing")?,
                );
                prompt.bytes(6, &tool.0);
            }
        }
        request.bytes(3, &prompt.0);
    }
    request.text(2, &system.join("\n\n"));
    request.text(
        21,
        chat["model"]
            .as_str()
            .ok_or("Devin model identifier missing")?,
    );
    request.number(7, 5);
    request.number(20, 1);
    request.number(11, 1);
    request.text(16, cascade);
    request.text(22, &uuid::Uuid::new_v4().to_string());
    let mut choice = Encoder::default();
    choice.text(1, "auto");
    request.bytes(12, &choice.0);
    let mut cache = Encoder::default();
    cache.number(1, 1);
    request.bytes(13, &cache.0);
    let mut configuration = Encoder::default();
    configuration.number(1, 1);
    configuration.number(2, chat["max_tokens"].as_u64().unwrap_or(64000));
    configuration.number(3, 200);
    configuration.double(5, chat["temperature"].as_f64().unwrap_or(0.4));
    configuration.double(6, chat["temperature"].as_f64().unwrap_or(0.4));
    configuration.number(7, 50);
    configuration.double(8, 1.0);
    configuration.double(11, 1.0);
    for pattern in [
        "<|user|>",
        "<|bot|>",
        "<|context_request|>",
        "<|endoftext|>",
        "<|end_of_turn|>",
    ] {
        configuration.text(9, pattern)
    }
    for pattern in chat["stop"].as_array().into_iter().flatten() {
        if let Some(value) = pattern.as_str() {
            configuration.text(9, value)
        }
    }
    request.bytes(8, &configuration.0);
    for definition in chat["tools"].as_array().into_iter().flatten() {
        let definition = &definition["function"];
        let mut tool = Encoder::default();
        tool.text(
            1,
            definition["name"]
                .as_str()
                .ok_or("Devin tool name missing")?,
        );
        tool.text(2, definition["description"].as_str().unwrap_or(""));
        tool.text(3, &definition["parameters"].to_string());
        request.bytes(10, &tool.0);
    }
    if request.0.len() > 64 * 1024 * 1024 {
        return Err("Devin request exceeds 64 MiB".into());
    }
    Ok(request.0)
}

pub(crate) struct NativeTranslator {
    inner: super::translate::Translator,
    calls: HashMap<String, usize>,
    active: Option<usize>,
    arguments: HashMap<usize, String>,
    message_id: String,
    signature: String,
    output_id: String,
    stop: u64,
    usage: Option<TokenUsage>,
}
impl NativeTranslator {
    pub(crate) fn new() -> Self {
        Self {
            inner: super::translate::Translator::new(),
            calls: HashMap::new(),
            active: None,
            arguments: HashMap::new(),
            message_id: String::new(),
            signature: String::new(),
            output_id: String::new(),
            stop: 0,
            usage: None,
        }
    }
    pub(crate) fn consume(&mut self, bytes: &[u8]) -> Result<Vec<StreamChunk>, LlmFailure> {
        self.consume_inner(bytes)
            .map_err(|error| failure(error, "MALFORMED_RESPONSE"))
    }
    fn consume_inner(&mut self, bytes: &[u8]) -> Result<Vec<StreamChunk>, String> {
        let message = Message::parse(bytes)?;
        if message.number(8)? != 0 {
            return Err("Devin redacted the response".into());
        }
        let id = message.text(1)?;
        if !id.is_empty() {
            self.message_id = id.into()
        }
        self.signature.push_str(message.text(10)?);
        if self.signature.len() > 1024 * 1024 {
            return Err("Devin signature exceeds its limit".into());
        }
        let output_id = message.text(15)?;
        if !output_id.is_empty() {
            self.output_id = output_id.into()
        }
        let stop = message.number(5)?;
        if stop != 0 {
            self.stop = stop
        }
        let mut delta = json!({"content":message.text(3)?,"reasoning_content":message.text(9)?});
        let mut tools = Vec::new();
        for bytes in message.repeated(6)? {
            let tool = Message::parse(bytes)?;
            let id = tool.text(1)?;
            let index = if id.is_empty() {
                self.active.ok_or("Devin tool delta has no identity")?
            } else {
                let next = self.calls.len();
                *self.calls.entry(id.into()).or_insert(next)
            };
            if self.calls.len() > 256 {
                return Err("Devin returned too many tool calls".into());
            }
            self.active = Some(index);
            let incoming = tool.text(3)?;
            let previous = self.arguments.entry(index).or_default();
            let addition = if incoming.starts_with(previous.as_str()) {
                incoming[previous.len()..].to_string()
            } else {
                incoming.to_string()
            };
            previous.push_str(&addition);
            tools.push(json!({"index":index,"id":if id.is_empty(){None}else{Some(id)},"function":{"name":if tool.text(2)?.is_empty(){None}else{Some(tool.text(2)?)},"arguments":addition}}));
        }
        if !tools.is_empty() {
            delta["tool_calls"] = json!(tools)
        }
        let out = self
            .inner
            .consume(&json!({"choices":[{"delta":delta}]}).to_string())
            .map_err(|error| error.message)?;
        if let Some(bytes) = message.bytes(7)? {
            let usage = Message::parse(bytes)?;
            self.usage = Some(TokenUsage {
                input_tokens: usage.number(2)?,
                output_tokens: usage.number(3)?,
                cache_read_tokens: Some(usage.number(5)?),
                cache_write_tokens: Some(usage.number(4)?),
                reasoning_tokens: None,
            });
        }
        Ok(out)
    }
    pub(crate) fn finish(
        mut self,
        scope: &str,
        cascade: &str,
    ) -> Result<Vec<StreamChunk>, LlmFailure> {
        let reason = match self.stop {
            3 => "length",
            10 => "tool_calls",
            11 => "content_filter",
            13 => "error",
            1 | 9 => {
                return Err(failure(
                    "Devin ended with an incomplete response",
                    "TRUNCATED_RESPONSE",
                ));
            }
            _ => {
                if self.calls.is_empty() {
                    "stop"
                } else {
                    "tool_calls"
                }
            }
        };
        self.inner
            .consume(&json!({"choices":[{"finish_reason":reason}]}).to_string())?;
        let mut out = self.inner.consume(super::sse::DONE)?;
        if let Some(usage) = self.usage.take() {
            let index = out
                .iter()
                .position(|chunk| matches!(chunk, StreamChunk::Finish { .. }))
                .unwrap_or(out.len());
            out.insert(index, StreamChunk::Usage { usage });
        }
        for chunk in &mut out {
            if let StreamChunk::Finish { replay_state, .. } = chunk {
                *replay_state = Some(
                    json!({"protocol":API,"scopeHash":scope,"cascadeId":cascade,"messageId":self.message_id,"signature":self.signature,"outputId":self.output_id}),
                );
            }
        }
        Ok(out)
    }
}
