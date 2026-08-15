//! Content-block structure helpers. Rust port of
//! `packages/llm/llm/src/content.ts`.

use crate::types::ContentBlock;

/// True when typed model content contains an image block, walking nested
/// tool-result content (TS `contentHasImage`).
pub fn content_has_image(content: &[ContentBlock]) -> bool {
    content.iter().any(|block| match block {
        ContentBlock::Image { .. } => true,
        ContentBlock::ToolResult { content, .. } => content_has_image(content),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_nested_tool_result_content() {
        assert!(!content_has_image(&[ContentBlock::Text { text: "hi".to_string() }]));
        assert!(content_has_image(&[ContentBlock::ToolResult {
            tool_call_id: crate::brand::call_id("c1"),
            content: vec![ContentBlock::Image {
                attachment: crate::types::ImageAttachmentRef { id: "img".to_string() },
            }],
            is_error: None,
        }]));
    }
}
