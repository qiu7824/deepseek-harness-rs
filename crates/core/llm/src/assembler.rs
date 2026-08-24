//! Incremental chunk-to-message assembler. Rust port of
//! `packages/llm/llm/src/assembler.ts`.
//!
//! This is the single canonical assembly algorithm used by the agent loop to
//! build an assistant message from a chunk stream while logging the raw
//! chunks for replay fidelity.

use indexmap::IndexMap;

use crate::brand::CallId;
use crate::message::{Message, MessageSource, create_message};
use crate::types::{ContentBlock, FinishReason, StreamChunk, TokenUsage};

struct PartialBlock {
    block_type: String,
    text: String,
    tool_call_id: Option<CallId>,
    tool_call_name: Option<String>,
    tool_call_arguments: String,
    /// Set by `block-end` — authoritative, and freezes the partial.
    block: Option<ContentBlock>,
}

/// Incrementally assembles raw [`StreamChunk`]s into complete
/// [`ContentBlock`]s and a final assistant [`Message`].
pub struct BlockAssembler {
    partials: IndexMap<u64, PartialBlock>,
    usage: Option<TokenUsage>,
    finish: Option<FinishReason>,
    replay_state: Option<serde_json::Value>,
}

impl Default for BlockAssembler {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockAssembler {
    pub fn new() -> Self {
        Self {
            partials: IndexMap::new(),
            usage: None,
            finish: None,
            replay_state: None,
        }
    }

    /// Feed one chunk into the assembly state.
    pub fn push(&mut self, chunk: &StreamChunk) {
        match chunk {
            StreamChunk::BlockStart { index, block_type } => {
                if !self.partials.contains_key(index) {
                    self.partials.insert(
                        *index,
                        PartialBlock {
                            block_type: block_type.clone(),
                            text: String::new(),
                            tool_call_id: None,
                            tool_call_name: None,
                            tool_call_arguments: String::new(),
                            block: None,
                        },
                    );
                }
            }
            StreamChunk::TextDelta { index, text } => {
                let partial = self.ensure(*index, "text");
                if partial.block.is_some() {
                    return; // closed by block-end; ignore stragglers
                }
                partial.text.push_str(text);
            }
            StreamChunk::ReasoningDelta { index, text } => {
                let partial = self.ensure(*index, "reasoning");
                if partial.block.is_some() {
                    return;
                }
                partial.text.push_str(text);
            }
            StreamChunk::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            } => {
                let partial = self.ensure(*index, "tool-call");
                if partial.block.is_some() {
                    return;
                }
                partial.tool_call_id = Some(id.clone());
                if let Some(name) = name {
                    partial.tool_call_name = Some(name.clone());
                }
                partial.tool_call_arguments.push_str(arguments_delta);
            }
            StreamChunk::BlockEnd { index, block } => {
                let partial = self.ensure(*index, block.type_tag());
                // First close wins.
                if partial.block.is_some() {
                    return;
                }
                partial.block = Some(block.clone());
            }
            StreamChunk::Usage { usage } => {
                self.usage = Some(usage.clone());
            }
            StreamChunk::Finish {
                reason,
                replay_state,
            } => {
                self.replay_state = Self::align_replay_state(&self.partials, reason, replay_state);
                self.finish = Some(reason.clone());
            }
        }
    }

    fn ensure(&mut self, index: u64, block_type: &str) -> &mut PartialBlock {
        self.partials.entry(index).or_insert_with(|| PartialBlock {
            block_type: block_type.to_string(),
            text: String::new(),
            tool_call_id: None,
            tool_call_name: None,
            tool_call_arguments: String::new(),
            block: None,
        })
    }

    fn assemble(partial: &PartialBlock, index: u64) -> ContentBlock {
        if let Some(block) = &partial.block {
            return block.clone();
        }
        match partial.block_type.as_str() {
            "text" => ContentBlock::Text {
                text: partial.text.clone(),
            },
            "reasoning" => ContentBlock::Reasoning {
                text: partial.text.clone(),
            },
            "tool-call" => ContentBlock::ToolCall {
                id: partial
                    .tool_call_id
                    .clone()
                    .unwrap_or_else(|| crate::brand::call_id(format!("call-{index}"))),
                name: partial.tool_call_name.clone().unwrap_or_default(),
                arguments: partial.tool_call_arguments.clone(),
            },
            other => panic!("cannot assemble incomplete block of type \"{other}\""),
        }
    }

    fn align_replay_state(
        partials: &IndexMap<u64, PartialBlock>,
        finish: &FinishReason,
        replay_state: &Option<serde_json::Value>,
    ) -> Option<serde_json::Value> {
        let mut replay = replay_state.clone()?;
        let Some(entries) = replay.get("blocks") else {
            return Some(replay);
        };
        let entries = entries.as_array()?;
        let blocks: Vec<ContentBlock> = partials
            .iter()
            .map(|(index, partial)| Self::assemble(partial, *index))
            .collect();
        if entries.len() != blocks.len() {
            return None;
        }
        if !matches!(finish, FinishReason::MaxTokens) {
            return Some(replay);
        }
        let kept: Vec<serde_json::Value> = entries
            .iter()
            .zip(&blocks)
            .filter(|(_, block)| block.type_tag() != "tool-call")
            .map(|(entry, _)| entry.clone())
            .collect();
        replay
            .as_object_mut()?
            .insert("blocks".to_string(), serde_json::Value::Array(kept));
        Some(replay)
    }

    /// Assemble all blocks seen so far, in stream order.
    pub fn blocks(&self) -> Vec<ContentBlock> {
        let blocks: Vec<ContentBlock> = self
            .partials
            .iter()
            .map(|(index, partial)| Self::assemble(partial, *index))
            .collect();
        match self.finish() {
            FinishReason::MaxTokens => blocks
                .into_iter()
                .filter(|block| block.type_tag() != "tool-call")
                .collect(),
            _ => blocks,
        }
    }

    /// Assemble the model-visible prefix that is safe to persist when a
    /// caller interrupts the stream. Only non-blank text and reasoning are
    /// retained; incomplete tool calls are omitted because no result exists.
    pub fn interrupted_blocks(&self) -> Vec<ContentBlock> {
        self.partials
            .iter()
            .filter_map(|(index, partial)| {
                let block_type = partial
                    .block
                    .as_ref()
                    .map(ContentBlock::type_tag)
                    .unwrap_or(partial.block_type.as_str());
                if block_type != "text" && block_type != "reasoning" {
                    return None;
                }
                let block = Self::assemble(partial, *index);
                match &block {
                    ContentBlock::Text { text } | ContentBlock::Reasoning { text }
                        if !text.trim().is_empty() =>
                    {
                        Some(block)
                    }
                    _ => None,
                }
            })
            .collect()
    }

    /// Usage from the `usage` chunk; undefined until one arrives.
    pub fn usage(&self) -> Option<&TokenUsage> {
        self.usage.as_ref()
    }

    /// Finish reason from the `finish` chunk; `{kind: 'stop'}` when the
    /// stream ended without one.
    pub fn finish(&self) -> FinishReason {
        self.finish.clone().unwrap_or(FinishReason::Stop)
    }

    /// Adapter-private replay state from the terminal finish chunk, if any.
    pub fn replay_state(&self) -> Option<&serde_json::Value> {
        self.replay_state.as_ref()
    }

    /// The assembled assistant message.
    pub fn message(&self, source: Option<MessageSource>) -> Message {
        let source = source.unwrap_or(MessageSource::Plugin {
            plugin: "dsh-llm/assembler".to_string(),
            form: None,
            sections: None,
            summary: None,
            compaction_id: None,
            source_command_id: None,
        });
        let mut content = self.blocks();
        let _ = &mut content;
        create_message(crate::message::Role::Assistant, self.blocks(), source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_delta(index: u64, text: &str) -> StreamChunk {
        StreamChunk::TextDelta {
            index,
            text: text.to_string(),
        }
    }

    #[test]
    fn interrupted_blocks_keep_visible_text_and_reasoning_but_drop_tool_calls() {
        let mut assembler = BlockAssembler::new();
        assembler.push(&StreamChunk::TextDelta {
            index: 0,
            text: "visible".to_string(),
        });
        assembler.push(&StreamChunk::ReasoningDelta {
            index: 1,
            text: " thinking ".to_string(),
        });
        assembler.push(&StreamChunk::ToolCallDelta {
            index: 2,
            id: crate::brand::call_id("call-1"),
            name: Some("read".to_string()),
            arguments_delta: "{\"path\":".to_string(),
        });
        assembler.push(&StreamChunk::TextDelta {
            index: 3,
            text: "   ".to_string(),
        });
        assert_eq!(
            assembler.interrupted_blocks(),
            vec![
                ContentBlock::Text {
                    text: "visible".to_string(),
                },
                ContentBlock::Reasoning {
                    text: " thinking ".to_string(),
                },
            ]
        );
    }

    #[test]
    fn assembles_text_and_tool_call_blocks_in_order() {
        let mut assembler = BlockAssembler::new();
        assembler.push(&StreamChunk::BlockStart {
            index: 0,
            block_type: "text".to_string(),
        });
        assembler.push(&text_delta(0, "hel"));
        assembler.push(&text_delta(0, "lo"));
        assembler.push(&StreamChunk::BlockEnd {
            index: 0,
            block: ContentBlock::Text {
                text: "hello".to_string(),
            },
        });
        assembler.push(&StreamChunk::BlockStart {
            index: 1,
            block_type: "tool-call".to_string(),
        });
        assembler.push(&StreamChunk::ToolCallDelta {
            index: 1,
            id: crate::brand::call_id("c1"),
            name: Some("read".to_string()),
            arguments_delta: "{}".to_string(),
        });
        assembler.push(&StreamChunk::BlockEnd {
            index: 1,
            block: ContentBlock::ToolCall {
                id: crate::brand::call_id("c1"),
                name: "read".to_string(),
                arguments: "{}".to_string(),
            },
        });
        assembler.push(&StreamChunk::Usage {
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            },
        });
        assembler.push(&StreamChunk::Finish {
            reason: FinishReason::Stop,
            replay_state: None,
        });

        let blocks = assembler.blocks();
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            blocks[0],
            ContentBlock::Text {
                text: "hello".to_string()
            }
        );
        assert_eq!(assembler.usage().map(|usage| usage.output_tokens), Some(5));
        assert_eq!(assembler.finish(), FinishReason::Stop);
    }

    #[test]
    fn delta_only_protocol_and_max_tokens_truncation() {
        let mut assembler = BlockAssembler::new();
        // Delta-only: no block-start; the assembler infers the block type.
        assembler.push(&text_delta(0, "partial"));
        assembler.push(&StreamChunk::ToolCallDelta {
            index: 1,
            id: crate::brand::call_id("c1"),
            name: Some("danger".to_string()),
            arguments_delta: "{}".to_string(),
        });
        assembler.push(&StreamChunk::Finish {
            reason: FinishReason::MaxTokens,
            replay_state: None,
        });

        // max-tokens truncation drops tool calls that cannot execute.
        let blocks = assembler.blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0],
            ContentBlock::Text {
                text: "partial".to_string()
            }
        );
    }

    #[test]
    fn max_tokens_prunes_replay_entries_with_dropped_tool_calls() {
        let mut assembler = BlockAssembler::new();
        assembler.push(&StreamChunk::BlockEnd {
            index: 0,
            block: ContentBlock::Text {
                text: "lead".to_string(),
            },
        });
        assembler.push(&StreamChunk::BlockEnd {
            index: 1,
            block: ContentBlock::ToolCall {
                id: crate::brand::call_id("c1"),
                name: "echo".to_string(),
                arguments: "{\"text\":".to_string(),
            },
        });
        assembler.push(&StreamChunk::BlockEnd {
            index: 2,
            block: ContentBlock::Reasoning {
                text: "tail".to_string(),
            },
        });
        assembler.push(&StreamChunk::Finish {
            reason: FinishReason::MaxTokens,
            replay_state: Some(serde_json::json!({
                "response": {"responseId": "resp-1"},
                "blocks": ["meta-0", "meta-1", "meta-2"]
            })),
        });

        assert_eq!(
            assembler.blocks(),
            vec![
                ContentBlock::Text {
                    text: "lead".to_string(),
                },
                ContentBlock::Reasoning {
                    text: "tail".to_string(),
                },
            ]
        );
        assert_eq!(
            assembler.replay_state(),
            Some(&serde_json::json!({
                "response": {"responseId": "resp-1"},
                "blocks": ["meta-0", "meta-2"]
            }))
        );
    }

    #[test]
    fn misaligned_replay_entries_drop_the_envelope() {
        let mut assembler = BlockAssembler::new();
        assembler.push(&StreamChunk::BlockEnd {
            index: 0,
            block: ContentBlock::Text {
                text: "one".to_string(),
            },
        });
        assembler.push(&StreamChunk::BlockEnd {
            index: 1,
            block: ContentBlock::Text {
                text: "two".to_string(),
            },
        });
        assembler.push(&StreamChunk::Finish {
            reason: FinishReason::Stop,
            replay_state: Some(serde_json::json!({
                "response": {"responseId": "resp-1"},
                "blocks": ["meta-0"]
            })),
        });

        assert_eq!(assembler.blocks().len(), 2);
        assert_eq!(assembler.replay_state(), None);
    }

    #[test]
    fn stragglers_after_block_end_are_ignored() {
        let mut assembler = BlockAssembler::new();
        assembler.push(&StreamChunk::BlockStart {
            index: 0,
            block_type: "text".to_string(),
        });
        assembler.push(&StreamChunk::BlockEnd {
            index: 0,
            block: ContentBlock::Text {
                text: "done".to_string(),
            },
        });
        assembler.push(&text_delta(0, "STRAY"));
        let blocks = assembler.blocks();
        assert_eq!(
            blocks,
            vec![ContentBlock::Text {
                text: "done".to_string()
            }]
        );
    }
}
