//! Canonical selection of a child's final assistant output. Rust port of
//! `packages/subagent/subagent/src/assistant-output.ts`.

use dsh_llm::ContentBlock;
use dsh_session::SessionEvent;

/// Incremental fold of the selection rule, for backends that observe a
/// child's output as it streams.
#[derive(Debug, Clone, Default)]
pub struct AssistantOutputFold {
    message: Option<Vec<ContentBlock>>,
    partial: Vec<String>,
}

impl AssistantOutputFold {
    /// Fold one session event: a non-empty assistant message becomes the
    /// candidate final answer, and a `text-delta` chunk extends the streamed
    /// fallback; every other event contributes nothing.
    pub fn push(&mut self, event: &SessionEvent) {
        if event.type_ == "assistant/message"
            && let Some(content) = event
                .data
                .get("message")
                .and_then(|message| message.get("content"))
            && let Some(blocks) = content.as_array()
            && !blocks.is_empty()
        {
            self.message = serde_json::from_value::<Vec<ContentBlock>>(content.clone()).ok();
        } else if event.type_ == "assistant/chunk"
            && event
                .data
                .get("chunk")
                .and_then(|chunk| chunk.get("type"))
                .and_then(|kind| kind.as_str())
                == Some("text-delta")
            && let Some(text) = event
                .data
                .get("chunk")
                .and_then(|chunk| chunk.get("text"))
                .and_then(|text| text.as_str())
        {
            self.push_text(text);
        }
    }

    /// Extend the streamed fallback with text observed outside session
    /// events.
    pub fn push_text(&mut self, text: &str) {
        if !text.is_empty() {
            self.partial.push(text.to_string());
        }
    }

    /// Select the final output folded so far.
    pub fn collect(&self) -> Option<Vec<ContentBlock>> {
        if let Some(message) = &self.message {
            return Some(message.clone());
        }
        let text = self.partial.join("");
        if text.is_empty() {
            None
        } else {
            Some(vec![ContentBlock::Text { text }])
        }
    }
}

/// Apply the selection rule to one complete child-owned event suffix.
pub fn final_assistant_output(events: &[SessionEvent]) -> Option<Vec<ContentBlock>> {
    let mut fold = AssistantOutputFold::default();
    for event in events {
        fold.push(event);
    }
    fold.collect()
}
