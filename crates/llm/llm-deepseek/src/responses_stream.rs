//! Incremental Responses output reconciliation and explicit terminal outcomes.
use super::failure;
use dsh_llm::{ContentBlock, FinishReason, LlmFailure, StreamChunk, TokenUsage, call_id};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Default)]
struct Item {
    value: Value,
    output_index: Option<u64>,
    complete: bool,
    arguments_done: Option<String>,
    conflicted: bool,
}

fn extends_parts(old: &Value, new: &Value) -> bool {
    let Some(old) = old.as_array() else {
        return true;
    };
    let Some(new) = new.as_array() else {
        return false;
    };
    new.len() >= old.len()
        && old.iter().zip(new).all(|(a, b)| {
            a["type"] == b["type"]
                && ["text", "refusal"].into_iter().all(|key| {
                    a[key]
                        .as_str()
                        .filter(|text| !text.is_empty())
                        .is_none_or(|text| {
                            b[key].as_str().is_some_and(|next| next.starts_with(text))
                        })
                })
        })
}

/// Merge one item/part without replacing unrelated items or erasing a received
/// text suffix with a sparse/shortened completion snapshot.
fn merge_value(previous: &mut Value, next: &Value) {
    if next.is_null() {
        return;
    }
    match (previous, next) {
        (Value::Object(old), Value::Object(new)) => {
            if old
                .get("type")
                .and_then(Value::as_str)
                .zip(new.get("type").and_then(Value::as_str))
                .is_some_and(|(a, b)| a != b)
            {
                *old = new.clone();
                return;
            }
            for (key, value) in new {
                if let Some(prior) = old.get_mut(key) {
                    merge_value(prior, value);
                } else {
                    old.insert(key.clone(), value.clone());
                }
            }
        }
        (Value::Array(old), Value::Array(new)) => {
            for (index, value) in new.iter().enumerate() {
                if let Some(prior) = old.get_mut(index) {
                    merge_value(prior, value);
                } else {
                    old.push(value.clone());
                }
            }
        }
        (Value::String(old), Value::String(new))
            if !old.is_empty() && (new.is_empty() || old.starts_with(new.as_str())) => {}
        (old, new) => *old = new.clone(),
    }
}

fn message_text(item: &Value) -> String {
    if item["type"] != "message" || item["role"] != "assistant" {
        return String::new();
    }
    item["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| match part["type"].as_str() {
            Some("output_text") => part["text"].as_str(),
            Some("refusal") => part["refusal"].as_str(),
            _ => None,
        })
        .collect()
}

#[derive(Default)]
pub(crate) struct ResponsesTranslator {
    next_index: u64,
    text: Option<(u64, String)>,
    reasoning: Option<(u64, String)>,
    tools: BTreeMap<usize, (u64, String, String, String)>,
    items: Vec<Item>,
    ids: BTreeMap<String, usize>,
    indexes: BTreeMap<u64, usize>,
    text_parts: BTreeMap<(usize, u64), String>,
    tool_fragment_seen: bool,
    refusal_seen: bool,
    ended: bool,
    model: Option<String>,
}

impl ResponsesTranslator {
    pub(crate) fn finished(&self) -> bool {
        self.ended
    }

    pub(crate) fn consume_limited(
        &mut self,
        payload: &str,
        remaining: usize,
    ) -> Result<Vec<StreamChunk>, LlmFailure> {
        let mut chunks = self.consume(payload)?;
        if chunks.len() <= remaining {
            return Ok(chunks);
        }
        let error = failure(
            "Responses success response emitted too many chunks",
            "RESPONSE_TOO_LARGE",
        );
        if !self.ended {
            return Ok(self.fail(error));
        }
        // A terminal event may itself exceed the limit. Its safe text must
        // still close with an error; dropping it would leave no terminal event.
        let had_tools = self.tool_fragment_seen
            || self
                .items
                .iter()
                .any(|item| item.value["type"] == "function_call");
        chunks.retain_mut(|chunk| match chunk {
            StreamChunk::BlockEnd { block:ContentBlock::Text {..} | ContentBlock::Reasoning {..}, .. } | StreamChunk::Usage {..} => true,
            StreamChunk::Finish {reason, replay_state} => {
                // Preserve explicit refusals even when a terminal chunk limit
                // is reached, so they can never become a retryable failure.
                if !matches!(reason, FinishReason::Error {failure} if failure.code=="CONTENT_FILTER") {
                    *reason = FinishReason::Error {failure:error.clone()};
                }
                if let Some(state) = replay_state {
                    state["items"] = json!([]);
                    state["truncatedToolCalls"] = json!(had_tools);
                    state["responseStatus"] = json!("failed");
                    state.as_object_mut().unwrap().remove("continuation");
                }
                true
            }
            _ => false,
        });
        Ok(chunks)
    }

    fn upsert(&mut self, value: &Value, output_index: Option<u64>, complete: bool) -> usize {
        let id = value["id"].as_str().filter(|id| !id.is_empty());
        let known = id.and_then(|id| self.ids.get(id).copied()).or_else(|| {
            output_index
                .and_then(|index| self.indexes.get(&index).copied())
                .filter(|index| {
                    id.is_none()
                        || self.items[*index].value["id"]
                            .as_str()
                            .is_none_or(|old| Some(old) == id)
                })
        });
        let index = known.unwrap_or_else(|| {
            let index = self.items.len();
            self.items.push(Item::default());
            index
        });
        let item = &mut self.items[index];
        let mut incoming = value.clone();
        if item.complete {
            for key in ["content", "summary"] {
                if incoming.get(key).is_some() && !extends_parts(&item.value[key], &incoming[key]) {
                    let refusal = incoming[key]
                        .as_array()
                        .is_some_and(|parts| parts.iter().any(|part| part["type"] == "refusal"));
                    if !refusal {
                        incoming.as_object_mut().unwrap().remove(key);
                    }
                }
            }
            if item.value["type"] == "function_call" {
                for key in ["name", "call_id", "arguments"] {
                    if let (Some(old), Some(next)) =
                        (item.value[key].as_str(), incoming[key].as_str())
                    {
                        if !old.is_empty() && !next.is_empty() && old != next {
                            item.conflicted = true;
                        }
                    }
                }
            }
        }
        merge_value(&mut item.value, &incoming);
        if item.output_index.is_none() {
            item.output_index = output_index;
        }
        if complete {
            item.complete = !matches!(
                value["status"].as_str(),
                Some("in_progress" | "incomplete" | "failed")
            );
            if item.complete
                && value.get("status").is_none()
                && item.value["status"] == "in_progress"
            {
                item.value["status"] = json!("completed");
            }
        }
        if let Some(id) = id {
            self.ids.insert(id.into(), index);
        }
        if let Some(output_index) = output_index {
            self.indexes.entry(output_index).or_insert(index);
        }
        index
    }

    fn event_item(&mut self, event: &Value, kind: &str) -> Option<usize> {
        let id = event["item_id"].as_str().filter(|id| !id.is_empty());
        let output_index = event["output_index"].as_u64();
        if id.is_none() && output_index.is_none() {
            return None;
        }
        let mut item = json!({"type":kind});
        if kind == "message" {
            item["role"] = json!("assistant");
        }
        if let Some(id) = id {
            item["id"] = json!(id);
        }
        Some(self.upsert(&item, output_index, false))
    }

    fn text_delta(&mut self, text: &str, out: &mut Vec<StreamChunk>) {
        if self.text.is_none() {
            let index = self.next_index;
            self.next_index += 1;
            self.text = Some((index, String::new()));
            out.push(StreamChunk::BlockStart {
                index,
                block_type: "text".into(),
            });
        }
        let (index, known) = self.text.as_mut().unwrap();
        known.push_str(text);
        out.push(StreamChunk::TextDelta {
            index: *index,
            text: text.into(),
        });
    }

    fn text_part(&mut self, event: &Value, text: &str, delta: bool, out: &mut Vec<StreamChunk>) {
        let suffix = if let Some(item) = self.event_item(event, "message") {
            let known = self
                .text_parts
                .entry((item, event["content_index"].as_u64().unwrap_or(0)))
                .or_default();
            if delta {
                known.push_str(text);
                text.to_string()
            } else if let Some(suffix) = text.strip_prefix(known.as_str()) {
                let suffix = suffix.to_string();
                *known = text.to_string();
                suffix
            } else {
                String::new()
            }
        } else if delta {
            text.to_string()
        } else {
            let known = self.text.as_ref().map_or("", |(_, text)| text.as_str());
            text.strip_prefix(known)
                .unwrap_or_else(|| if known.contains(text) { "" } else { text })
                .to_string()
        };
        if !suffix.is_empty() {
            self.text_delta(&suffix, out);
        }
    }

    pub(crate) fn consume(&mut self, payload: &str) -> Result<Vec<StreamChunk>, LlmFailure> {
        if self.ended || payload == "[DONE]" {
            return Ok(Vec::new());
        }
        let event: Value = serde_json::from_str(payload).map_err(|error| {
            failure(
                format!("malformed Responses SSE payload: {error}"),
                "MALFORMED_RESPONSE",
            )
        })?;
        if let Some(model) = event
            .pointer("/response/model")
            .and_then(Value::as_str)
            .filter(|model| !model.is_empty())
        {
            self.model = Some(model.into());
        }
        let mut out = Vec::new();
        match event["type"].as_str().unwrap_or_default() {
            "response.output_text.delta" | "response.output_text.done" => {
                let delta = event["type"] == "response.output_text.delta";
                let text = event[if delta { "delta" } else { "text" }]
                    .as_str()
                    .unwrap_or_default();
                self.text_part(&event, text, delta, &mut out);
            }
            "response.refusal.delta" | "response.refusal.done" => {
                self.refusal_seen = true;
                let text = event["delta"]
                    .as_str()
                    .or_else(|| event["refusal"].as_str())
                    .unwrap_or_default();
                self.text_part(
                    &event,
                    text,
                    event["type"] == "response.refusal.delta",
                    &mut out,
                );
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let delta = event["delta"].as_str().unwrap_or_default();
                if self.reasoning.is_none() {
                    let index = self.next_index;
                    self.next_index += 1;
                    self.reasoning = Some((index, String::new()));
                    out.push(StreamChunk::BlockStart {
                        index,
                        block_type: "reasoning".into(),
                    });
                }
                let (index, text) = self.reasoning.as_mut().unwrap();
                text.push_str(delta);
                out.push(StreamChunk::ReasoningDelta {
                    index: *index,
                    text: delta.into(),
                });
            }
            "response.output_item.added" | "response.output_item.done" => {
                if let Some(value) = event.get("item").filter(|value| value["type"].is_string()) {
                    let done = event["type"] == "response.output_item.done";
                    let index = self.upsert(value, event["output_index"].as_u64(), done);
                    if value["type"] == "function_call" {
                        self.tool_fragment_seen = true;
                        if !done && !self.tools.contains_key(&index) {
                            let block = self.next_index;
                            self.next_index += 1;
                            let call = value["call_id"].as_str().unwrap_or_default().to_string();
                            let name = value["name"].as_str().unwrap_or_default().to_string();
                            let args = value["arguments"].as_str().unwrap_or_default().to_string();
                            self.tools
                                .insert(index, (block, call.clone(), name.clone(), args.clone()));
                            out.push(StreamChunk::BlockStart {
                                index: block,
                                block_type: "tool-call".into(),
                            });
                            out.push(StreamChunk::ToolCallDelta {
                                index: block,
                                id: call_id(call),
                                name: Some(name),
                                arguments_delta: args,
                            });
                        }
                    }
                }
            }
            "response.function_call_arguments.delta" | "response.function_call_arguments.done" => {
                self.tool_fragment_seen = true;
                if let Some(item) = self.event_item(&event, "function_call") {
                    if event["type"] == "response.function_call_arguments.done" {
                        if let Some(arguments) = event["arguments"].as_str() {
                            self.items[item].arguments_done = Some(arguments.into());
                        }
                    } else if let Some((index, call, _, args)) = self.tools.get_mut(&item) {
                        let delta = event["delta"].as_str().unwrap_or_default();
                        args.push_str(delta);
                        out.push(StreamChunk::ToolCallDelta {
                            index: *index,
                            id: call_id(call.clone()),
                            name: None,
                            arguments_delta: delta.into(),
                        });
                    }
                }
            }
            "response.completed" | "response.incomplete" | "response.failed" => {
                let kind = event["type"].as_str().unwrap();
                let status = if kind == "response.incomplete" {
                    "incomplete"
                } else if kind == "response.failed" {
                    "failed"
                } else {
                    event
                        .pointer("/response/status")
                        .and_then(Value::as_str)
                        .unwrap_or("completed")
                };
                out.extend(self.terminal(event.get("response"), status, None));
            }
            "error" => {
                let message = event
                    .pointer("/error/message")
                    .or_else(|| event.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Responses request failed");
                let code = event
                    .pointer("/error/code")
                    .or_else(|| event.get("code"))
                    .and_then(Value::as_str);
                let code = if code.is_some_and(|code| {
                    matches!(
                        code,
                        "content_filter" | "content_policy_violation" | "safety_refusal"
                    )
                }) {
                    "CONTENT_FILTER"
                } else {
                    "PROVIDER_ERROR"
                };
                out.extend(self.terminal(None, "failed", Some(failure(message, code))));
            }
            _ => {}
        }
        Ok(out)
    }

    fn terminal(
        &mut self,
        response: Option<&Value>,
        status: &str,
        transport_error: Option<LlmFailure>,
    ) -> Vec<StreamChunk> {
        if self.ended {
            return Vec::new();
        }
        if let Some(items) = response.and_then(|response| response["output"].as_array()) {
            for (index, item) in items
                .iter()
                .enumerate()
                .filter(|(_, item)| item["type"].is_string())
            {
                self.upsert(item, Some(index as u64), true);
            }
        }
        // A coalesced/blank terminal part may not erase text already received
        // for that same item and content index. Sparse indices stay map keys.
        for (item_index, tracked) in self.items.iter_mut().enumerate() {
            let value = &mut tracked.value;
            if value["type"] != "message" {
                continue;
            }
            let mut parts: BTreeMap<u64, Value> = value["content"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(index, value)| (index as u64, value.clone()))
                .collect();
            for ((_, index), text) in self
                .text_parts
                .range((item_index, 0)..=(item_index, u64::MAX))
            {
                let part = parts
                    .entry(*index)
                    .or_insert_with(|| json!({"type":"output_text","text":""}));
                if part["type"] == "output_text" {
                    let old = part["text"].as_str().unwrap_or_default();
                    if old.is_empty() || text.starts_with(old) {
                        part["text"] = json!(text);
                    }
                }
            }
            if !parts.is_empty() {
                value["content"] = Value::Array(parts.into_values().collect());
            }
        }
        let mut order: (Vec<usize>, bool) = (
            (0..self.items.len()).collect(),
            self.items.iter().all(|item| item.output_index.is_some()),
        );
        if order.1 {
            order.0.sort_by_key(|index| self.items[*index].output_index);
        }
        let mut items = order
            .0
            .iter()
            .map(|index| self.items[*index].value.clone())
            .collect::<Vec<_>>();
        let had_tools =
            self.tool_fragment_seen || items.iter().any(|item| item["type"] == "function_call");
        let mut safe_tools = Vec::new();
        let mut invalid_tool = false;
        for index in &order.0 {
            let tracked = &self.items[*index];
            if tracked.value["type"] != "function_call" {
                continue;
            }
            let streamed = self.tools.get(index);
            let call = tracked.value["call_id"]
                .as_str()
                .or_else(|| streamed.map(|tool| tool.1.as_str()))
                .unwrap_or_default();
            let name = tracked.value["name"]
                .as_str()
                .or_else(|| streamed.map(|tool| tool.2.as_str()))
                .unwrap_or_default();
            let complete = (tracked.complete || tracked.arguments_done.is_some())
                && !tracked.conflicted
                && !matches!(
                    tracked.value["status"].as_str(),
                    Some("incomplete" | "failed")
                );
            let arguments = {
                tracked
                    .complete
                    .then(|| tracked.value["arguments"].as_str())
                    .flatten()
            }
            .or_else(|| tracked.arguments_done.as_deref())
            .or_else(|| streamed.map(|tool| tool.3.as_str()))
            .or_else(|| tracked.value["arguments"].as_str())
            .unwrap_or_default();
            let arguments = if complete && arguments.trim().is_empty() {
                "{}"
            } else {
                arguments
            };
            if !complete
                || call.is_empty()
                || name.is_empty()
                || !serde_json::from_str::<Value>(arguments).is_ok_and(|value| value.is_object())
            {
                invalid_tool = true;
                continue;
            }
            safe_tools.push((
                *index,
                call.to_string(),
                name.to_string(),
                arguments.to_string(),
            ));
        }
        if had_tools && safe_tools.is_empty() {
            invalid_tool = true;
        }
        let has_refusal = self.refusal_seen
            || items
                .iter()
                .flat_map(|item| item["content"].as_array().into_iter().flatten())
                .any(|part| part["type"] == "refusal");
        let incomplete_reason = response
            .and_then(|response| response.pointer("/incomplete_details/reason"))
            .and_then(Value::as_str);
        let reason = if has_refusal
            || incomplete_reason == Some("content_filter")
            || response
                .and_then(|response| response.pointer("/error/code"))
                .and_then(Value::as_str)
                .is_some_and(|code| {
                    matches!(
                        code,
                        "content_filter" | "content_policy_violation" | "safety_refusal"
                    )
                }) {
            FinishReason::Error {
                failure: failure("Provider returned a safety refusal", "CONTENT_FILTER"),
            }
        } else if let Some(error) = transport_error {
            FinishReason::Error { failure: error }
        } else if status == "incomplete" && incomplete_reason == Some("max_output_tokens") {
            FinishReason::MaxTokens
        } else if status == "incomplete" {
            FinishReason::Error {
                failure: failure(
                    format!(
                        "Responses response is incomplete: {}",
                        incomplete_reason.unwrap_or("reason unavailable")
                    ),
                    "INCOMPLETE_RESPONSE",
                ),
            }
        } else if status != "completed" {
            FinishReason::Error {
                failure: failure(
                    response
                        .and_then(|response| response.pointer("/error/message"))
                        .and_then(Value::as_str)
                        .unwrap_or("Responses request did not complete"),
                    "PROVIDER_ERROR",
                ),
            }
        } else if invalid_tool {
            FinishReason::Error {
                failure: failure(
                    "Responses response contains an unfinished or invalid tool call",
                    "INCOMPLETE_TOOL_CALL",
                ),
            }
        } else if !safe_tools.is_empty() {
            FinishReason::ToolCalls
        } else {
            FinishReason::Stop
        };
        let successful = matches!(reason, FinishReason::Stop | FinishReason::ToolCalls);
        let mut out = Vec::new();
        let text = items.iter().map(message_text).collect::<String>();
        if !text.is_empty() {
            if let Some((_, old)) = &mut self.text {
                if !old.contains(&text) {
                    *old = text;
                }
            } else {
                let index = self.next_index;
                self.next_index += 1;
                out.push(StreamChunk::BlockStart {
                    index,
                    block_type: "text".into(),
                });
                self.text = Some((index, text));
            }
        }
        let summaries = items
            .iter()
            .filter(|item| item["type"] == "reasoning")
            .flat_map(|item| item["summary"].as_array().into_iter().flatten())
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        if !summaries.is_empty() {
            if let Some((_, old)) = &mut self.reasoning {
                if !old.starts_with(&summaries) {
                    *old = summaries;
                }
            } else {
                let index = self.next_index;
                self.next_index += 1;
                out.push(StreamChunk::BlockStart {
                    index,
                    block_type: "reasoning".into(),
                });
                self.reasoning = Some((index, summaries));
            }
        }
        let messages = items
            .iter()
            .filter(|item| item["type"] == "message" && item["role"] == "assistant")
            .collect::<Vec<_>>();
        let has_visible_final = messages.iter().any(|item| {
            !matches!(item["phase"].as_str(), Some("commentary" | "analysis"))
                && !message_text(item).trim().is_empty()
        }) || messages.is_empty()
            && self
                .text
                .as_ref()
                .is_some_and(|(_, text)| !text.trim().is_empty());
        let commentary =
            !messages.is_empty() && messages.iter().all(|item| item["phase"] == "commentary");
        if let Some((index, text)) = self.reasoning.take() {
            out.push(StreamChunk::BlockEnd {
                index,
                block: ContentBlock::Reasoning { text },
            });
        }
        if let Some((index, text)) = self.text.take() {
            out.push(StreamChunk::BlockEnd {
                index,
                block: ContentBlock::Text { text },
            });
        }
        if successful {
            for (item, call, name, arguments) in safe_tools {
                let index = if let Some(tool) = self.tools.get(&item) {
                    tool.0
                } else {
                    let index = self.next_index;
                    self.next_index += 1;
                    out.push(StreamChunk::BlockStart {
                        index,
                        block_type: "tool-call".into(),
                    });
                    index
                };
                if let Some(position) = order.0.iter().position(|index| *index == item) {
                    items[position]["arguments"] = json!(arguments);
                    if items[position]["status"] == "in_progress" {
                        items[position]["status"] = json!("completed");
                    }
                }
                out.push(StreamChunk::BlockEnd {
                    index,
                    block: ContentBlock::ToolCall {
                        id: call_id(call),
                        name,
                        arguments,
                    },
                });
            }
        }
        if let Some(usage) = response.and_then(|response| response.get("usage")) {
            let cache_read = usage
                .pointer("/input_tokens_details/cached_tokens")
                .and_then(Value::as_u64);
            let cache_write = usage
                .pointer("/input_tokens_details/cache_write_tokens")
                .and_then(Value::as_u64);
            out.push(StreamChunk::Usage {
                usage: TokenUsage {
                    input_tokens: usage["input_tokens"]
                        .as_u64()
                        .unwrap_or(0)
                        .saturating_sub(cache_read.unwrap_or(0))
                        .saturating_sub(cache_write.unwrap_or(0)),
                    output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
                    cache_read_tokens: cache_read,
                    cache_write_tokens: cache_write,
                    reasoning_tokens: usage
                        .pointer("/output_tokens_details/reasoning_tokens")
                        .and_then(Value::as_u64),
                },
            });
        }
        let mut replay = json!({"format":"openai-responses-v1","items":if successful{items}else{Vec::<Value>::new()},"usageAccounting":"disjoint","responseStatus":status,"hasVisibleFinal":has_visible_final,"truncatedToolCalls":had_tools&&!successful});
        if let Some(reason) = incomplete_reason {
            replay["incompleteReason"] = json!(reason);
        }
        if successful && !had_tools && commentary {
            replay["continuation"] = json!("commentary");
        }
        if let Some(model) = self.model.take() {
            replay["responseModel"] = json!(model);
        }
        self.ended = true;
        out.push(StreamChunk::Finish {
            reason,
            replay_state: Some(replay),
        });
        out
    }

    pub(crate) fn fail(&mut self, error: LlmFailure) -> Vec<StreamChunk> {
        self.terminal(None, "failed", Some(error))
    }
    pub(crate) fn finish(&self) -> Result<(), LlmFailure> {
        if self.ended {
            Ok(())
        } else {
            Err(failure(
                "Responses stream ended before a terminal response",
                "STREAM_CLOSED",
            ))
        }
    }
}

#[cfg(test)]
#[path = "responses_stream_tests.rs"]
mod tests;
