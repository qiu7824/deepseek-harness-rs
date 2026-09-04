use std::collections::BTreeMap;

use dsh_llm::{
    ContentBlock, EMPTY_RESPONSE_CODE, FinishReason, LlmFailure, StreamChunk, TokenUsage, call_id,
};
use serde::Deserialize;

use crate::sse::DONE;

#[cfg(test)]
mod identity_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_and_null_continuations_preserve_interleaved_tool_identities() {
        let mut translator = Translator::new();
        for calls in [
            json!([{"index":0,"id":"call-a","function":{"name":"read","arguments":"{"}}, {"index":1,"id":"call-b","function":{"name":"write","arguments":"{"}}]),
            json!([{"index":1,"id":"","function":{"name":"","arguments":"}"}}, {"index":0,"id":null,"function":{"name":null,"arguments":"}"}}]),
        ] {
            translator
                .consume(&json!({"choices":[{"delta":{"tool_calls":calls}}]}).to_string())
                .unwrap();
        }
        let completed = translator.consume(DONE).unwrap();
        let blocks: Vec<_> = completed
            .iter()
            .filter_map(|chunk| {
                if let StreamChunk::BlockEnd { block, .. } = chunk {
                    Some(serde_json::to_value(block).unwrap())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["id"], "call-a");
        assert_eq!(blocks[0]["name"], "read");
        assert_eq!(blocks[1]["id"], "call-b");
        assert_eq!(blocks[1]["name"], "write");
    }
}

fn failure(message: impl Into<String>, code: impl Into<String>) -> LlmFailure {
    LlmFailure {
        message: message.into(),
        code: code.into(),
        status: None,
        provider_retry_after_ms: None,
        request_id: None,
    }
}

#[derive(Deserialize)]
struct WireChunk {
    #[serde(default)]
    choices: Vec<WireChoice>,
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireChoice {
    delta: Option<WireDelta>,
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct WireDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<WireToolCallDelta>,
}

#[derive(Deserialize)]
struct WireToolCallDelta {
    index: u64,
    id: Option<String>,
    function: Option<WireFunctionDelta>,
}

#[derive(Deserialize)]
struct WireFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct WireUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    prompt_cache_hit_tokens: Option<u64>,
    prompt_tokens_details: Option<WirePromptDetails>,
    completion_tokens_details: Option<WireCompletionDetails>,
}

#[derive(Deserialize)]
struct WirePromptDetails {
    cached_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct WireCompletionDetails {
    reasoning_tokens: Option<u64>,
}

struct OpenBlock {
    index: u64,
    text: String,
    call_id: String,
    name: String,
}

enum BlockKey {
    Text,
    Reasoning,
    Tool(u64),
}

pub(crate) struct Translator {
    next_index: u64,
    text: Option<OpenBlock>,
    reasoning: Option<OpenBlock>,
    tools: BTreeMap<u64, OpenBlock>,
    order: Vec<(u64, BlockKey)>,
    finish: Option<FinishReason>,
    usage: Option<TokenUsage>,
}

impl Translator {
    pub(crate) fn new() -> Self {
        Self {
            next_index: 0,
            text: None,
            reasoning: None,
            tools: BTreeMap::new(),
            order: Vec::new(),
            finish: None,
            usage: None,
        }
    }

    pub(crate) fn close_after_explicit_finish(&mut self) -> Result<Vec<StreamChunk>, LlmFailure> {
        if self.finish.is_none() {
            return Err(failure(
                "SSE stream ended without [DONE] or finish_reason",
                "STREAM_CLOSED",
            ));
        }
        Ok(self.close())
    }

    fn next_block(&mut self, key: BlockKey) -> OpenBlock {
        let index = self.next_index;
        self.next_index += 1;
        self.order.push((index, key));
        OpenBlock {
            index,
            text: String::new(),
            call_id: String::new(),
            name: String::new(),
        }
    }

    pub(crate) fn consume(&mut self, payload: &str) -> Result<Vec<StreamChunk>, LlmFailure> {
        if payload == DONE {
            return Ok(self.close());
        }
        let wire: WireChunk = serde_json::from_str(payload).map_err(|_| {
            failure(
                format!(
                    "malformed SSE payload: {}",
                    payload.chars().take(120).collect::<String>()
                ),
                "MALFORMED_RESPONSE",
            )
        })?;
        let mut output = Vec::new();
        for choice in wire.choices {
            let delta = choice.delta.unwrap_or_default();
            if let Some(fragment) = delta.reasoning_content.filter(|text| !text.is_empty()) {
                if self.reasoning.is_none() {
                    let block = self.next_block(BlockKey::Reasoning);
                    output.push(StreamChunk::BlockStart {
                        index: block.index,
                        block_type: "reasoning".to_string(),
                    });
                    self.reasoning = Some(block);
                }
                let block = self.reasoning.as_mut().expect("reasoning block");
                block.text.push_str(&fragment);
                output.push(StreamChunk::ReasoningDelta {
                    index: block.index,
                    text: fragment,
                });
            }
            if let Some(fragment) = delta.content.filter(|text| !text.is_empty()) {
                if self.text.is_none() {
                    let block = self.next_block(BlockKey::Text);
                    output.push(StreamChunk::BlockStart {
                        index: block.index,
                        block_type: "text".to_string(),
                    });
                    self.text = Some(block);
                }
                let block = self.text.as_mut().expect("text block");
                block.text.push_str(&fragment);
                output.push(StreamChunk::TextDelta {
                    index: block.index,
                    text: fragment,
                });
            }
            for call in delta.tool_calls {
                if !self.tools.contains_key(&call.index) {
                    let block = self.next_block(BlockKey::Tool(call.index));
                    output.push(StreamChunk::BlockStart {
                        index: block.index,
                        block_type: "tool-call".to_string(),
                    });
                    self.tools.insert(call.index, block);
                }
                let block = self.tools.get_mut(&call.index).expect("tool block");
                if let Some(id) = call.id.filter(|value| !value.is_empty()) {
                    block.call_id = id;
                }
                let mut arguments = String::new();
                if let Some(function) = call.function {
                    if let Some(name) = function.name.filter(|value| !value.is_empty()) {
                        block.name = name;
                    }
                    arguments = function.arguments.unwrap_or_default();
                    block.text.push_str(&arguments);
                }
                output.push(StreamChunk::ToolCallDelta {
                    index: block.index,
                    id: call_id(&block.call_id),
                    name: (!block.name.is_empty()).then(|| block.name.clone()),
                    arguments_delta: arguments,
                });
            }
            if let Some(reason) = choice.finish_reason {
                self.finish = Some(match reason.as_str() {
                    "stop" => FinishReason::Stop,
                    "tool_calls" => FinishReason::ToolCalls,
                    "length" => FinishReason::MaxTokens,
                    _ => FinishReason::Error {
                        failure: failure(
                            format!("model stopped: {reason}"),
                            reason.to_ascii_uppercase(),
                        ),
                    },
                });
            }
        }
        if let Some(usage) = wire.usage {
            let cached = usage
                .prompt_tokens_details
                .and_then(|details| details.cached_tokens)
                .or(usage.prompt_cache_hit_tokens);
            self.usage = Some(TokenUsage {
                input_tokens: usage.prompt_tokens.saturating_sub(cached.unwrap_or(0)),
                output_tokens: usage.completion_tokens,
                cache_read_tokens: cached,
                cache_write_tokens: None,
                reasoning_tokens: usage
                    .completion_tokens_details
                    .and_then(|details| details.reasoning_tokens),
            });
        }
        Ok(output)
    }

    fn close(&mut self) -> Vec<StreamChunk> {
        let mut output = Vec::new();
        for (index, key) in std::mem::take(&mut self.order) {
            let block = match key {
                BlockKey::Text => ContentBlock::Text {
                    text: self.text.take().expect("ordered text").text,
                },
                BlockKey::Reasoning => ContentBlock::Reasoning {
                    text: self.reasoning.take().expect("ordered reasoning").text,
                },
                BlockKey::Tool(wire_index) => {
                    let block = self.tools.remove(&wire_index).expect("ordered tool");
                    ContentBlock::ToolCall {
                        id: call_id(block.call_id),
                        name: block.name,
                        arguments: block.text,
                    }
                }
            };
            output.push(StreamChunk::BlockEnd { index, block });
        }
        if let Some(usage) = self.usage.take() {
            output.push(StreamChunk::Usage { usage });
        }
        let reason = self.finish.take().unwrap_or(FinishReason::Stop);
        let empty = output
            .iter()
            .all(|chunk| !matches!(chunk, StreamChunk::BlockEnd { .. }));
        output.push(StreamChunk::Finish {
            reason: if matches!(reason, FinishReason::Stop) && empty {
                FinishReason::Error {
                    failure: failure(
                        "model returned a completed response with no content",
                        EMPTY_RESPONSE_CODE,
                    ),
                }
            } else {
                reason
            },
            replay_state: None,
        });
        output
    }
}
