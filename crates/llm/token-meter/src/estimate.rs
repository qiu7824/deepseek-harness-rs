//! Fixed-density heuristic token pricing shared by the meter service and the
//! pure context-breakdown projection. Rust port of
//! `packages/llm/token-meter/src/estimate.ts`.

use dsh_llm::ContentBlock;
use dsh_session::EpochHeader;

/// Fixed text-density estimate used until exact tokenization is needed.
const CHARS_PER_TOKEN: usize = 4;

/// Per-block structural overhead for JSON framing and type tags.
const BLOCK_OVERHEAD: u64 = 4;

/// Role-field framing overhead added to every priced message.
pub const ROLE_OVERHEAD: u64 = 4;

/// Price content blocks recursively under the fixed density heuristic (TS
/// `estimateContent`).
pub fn estimate_content(blocks: &[ContentBlock]) -> u64 {
    let mut tokens = 0u64;
    for block in blocks {
        match block {
            ContentBlock::Text { text } | ContentBlock::Reasoning { text } => {
                tokens += ceil_div(text.chars().count(), CHARS_PER_TOKEN) + BLOCK_OVERHEAD
            }
            ContentBlock::ToolCall {
                name, arguments, ..
            } => {
                tokens += ceil_div(name.chars().count(), CHARS_PER_TOKEN)
                    + ceil_div(arguments.chars().count(), CHARS_PER_TOKEN)
                    + BLOCK_OVERHEAD
            }
            ContentBlock::ToolResult { content, .. } => {
                tokens += estimate_content(content) + BLOCK_OVERHEAD
            }
            other => {
                // Unknown blocks retain a conservative structural JSON price.
                let json = serde_json::to_string(other).unwrap_or_default();
                tokens += BLOCK_OVERHEAD + ceil_div(json.chars().count(), CHARS_PER_TOKEN)
            }
        }
    }
    tokens
}

/// Heuristically price one model-visible message (TS `estimateMessage`).
pub fn estimate_message(message: &dsh_llm::Message) -> u64 {
    estimate_content(&message.content) + ROLE_OVERHEAD
}

/// Price the system-prompt part of a canonical request envelope (TS
/// `estimateSystemTokens`).
pub fn estimate_system_tokens(header: Option<&EpochHeader>) -> u64 {
    match header.and_then(|header| header.system.as_deref()) {
        Some(system) => ceil_div(system.chars().count(), CHARS_PER_TOKEN) + ROLE_OVERHEAD,
        None => 0,
    }
}

/// Price the tool-schema part of a canonical request envelope (TS
/// `estimateToolsTokens`).
pub fn estimate_tools_tokens(header: Option<&EpochHeader>) -> u64 {
    let Some(tools) = header.and_then(|header| header.tools.as_deref()) else {
        return 0;
    };
    if tools.is_empty() {
        return 0;
    }
    let json = serde_json::to_string(tools).unwrap_or_default();
    ceil_div(json.chars().count(), CHARS_PER_TOKEN) + BLOCK_OVERHEAD
}

/// Price the complete non-surface request envelope (TS `estimateHeader`).
pub fn estimate_header(header: Option<&EpochHeader>) -> u64 {
    estimate_system_tokens(header) + estimate_tools_tokens(header)
}

fn ceil_div(numerator: usize, denominator: usize) -> u64 {
    numerator.div_ceil(denominator) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_llm::{ContentBlock, ModelMessageSource};

    #[test]
    fn prices_text_tool_call_and_nested_results() {
        assert_eq!(estimate_content(&[]), 0);
        let message = dsh_llm::create_assistant_message(
            vec![
                ContentBlock::Text {
                    text: "abcd".to_string(),
                }, // 1 + 4
                ContentBlock::ToolCall {
                    id: dsh_llm::call_id("c1"),
                    name: "read".to_string(),    // ceil(4/4)=1
                    arguments: "{}".to_string(), // ceil(2/4)=1
                },
            ],
            ModelMessageSource {
                provider: "p".to_string(),
                model: "m".to_string(),
                replay_state: None,
            },
        );
        // text: 1+4=5; tool-call: 1+1+4=6; role overhead 4 → 15.
        assert_eq!(estimate_message(&message), 15);

        // Nested tool results recurse.
        let nested = dsh_llm::create_tool_result_message(dsh_llm::ToolResultMessageInput {
            call_id: dsh_llm::call_id("c1"),
            content: vec![ContentBlock::Text {
                text: "12345678".to_string(),
            }], // 2+4=6
            is_error: false,
        });
        // inner text 6 + block overhead 4 = 10, plus role overhead 4 = 14.
        assert_eq!(estimate_message(&nested), 14);
    }
}
