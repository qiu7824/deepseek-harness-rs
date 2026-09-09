use dsh_llm::{ContentBlock, FinishReason, LlmFailure};
use serde_json::Value;

const EMPTY_NUDGE: &str = "上一条模型响应没有产生可交付的内容。继续处理当前请求，给出有内容的最终答复，或发出当前任务所需的完整工具调用；不要只说明将要继续。保持现有权限和用户约束。";
const DROPPED_TOOL_NUDGE: &str = "上一条响应声明需要调用工具，却没有提供任何工具调用，因此没有执行操作。若仍需操作，请重新发出完整的工具调用；否则给出实际结果。保持现有权限和用户约束。";
const TRUNCATION_NUDGE: &str = "上一条答复因单次输出上限而被截断。继续完成同一答复，从已输出内容的结尾继续，避免重复已有段落；保持原任务范围、权限和用户约束。";

/// Recovery is bounded independently from transport retry. Tool progress resets
/// stalled-response counters, but cannot replenish a turn's truncation budget.
#[derive(Default)]
pub(crate) struct ResponseContinuation {
    stalled: u8,
    empty_recoveries: u8,
    truncation_recoveries: u8,
}

fn incomplete(code: &str, message: &str) -> LlmFailure {
    LlmFailure {
        message: message.into(),
        code: code.into(),
        status: None,
        provider_retry_after_ms: None,
        request_id: None,
    }
}

impl ResponseContinuation {
    pub(crate) fn reset(&mut self) {
        self.stalled = 0;
        self.empty_recoveries = 0;
    }

    /// None completes the answer; Some requests another step with an optional
    /// short plugin notice. An empty notice preserves a phase-driven preamble.
    pub(crate) fn observe(
        &mut self,
        finish: &FinishReason,
        state: Option<&Value>,
        content: &[ContentBlock],
        saw_tool_call: bool,
    ) -> Result<Option<&'static str>, LlmFailure> {
        let has_text = content
            .iter()
            .any(|block| matches!(block, ContentBlock::Text { text } if !text.trim().is_empty()));
        let has_visible = has_text
            || content
                .iter()
                .any(|block| matches!(block, ContentBlock::Image { .. }));
        if *finish == FinishReason::MaxTokens {
            if saw_tool_call || state.is_some_and(|state| state["truncatedToolCalls"] == true) {
                return Err(incomplete(
                    "TRUNCATED_TOOL_CALL",
                    "输出上限截断了工具调用；该调用未执行，任务尚未完成",
                ));
            }
            if !has_text {
                return Err(incomplete(
                    "OUTPUT_LIMIT_WITHOUT_ANSWER",
                    "模型已达到单次输出上限，但没有产生可交付的答复；任务尚未完成",
                ));
            }
            if self.truncation_recoveries >= 2 {
                return Err(incomplete(
                    "OUTPUT_CONTINUATION_LIMIT",
                    "答复在两次续写后仍达到输出上限；已保留收到的内容，但答复尚未完整",
                ));
            }
            self.truncation_recoveries += 1;
            return Ok(Some(TRUNCATION_NUDGE));
        }
        let commentary = state.is_some_and(|state| {
            state["format"] == "openai-responses-v1" && state["continuation"] == "commentary"
        });
        let missing_final = state.is_some_and(|state| {
            state["format"] == "openai-responses-v1"
                && state.get("hasVisibleFinal") == Some(&Value::Bool(false))
                && state["items"].as_array().is_some_and(|items| {
                    items.iter().any(|item| {
                        item["type"] == "message"
                            && item["role"] == "assistant"
                            && matches!(item["phase"].as_str(), Some("commentary" | "final_answer"))
                    })
                })
        });
        let dropped_tool = *finish == FinishReason::ToolCalls;
        if !commentary && !missing_final && has_visible && !dropped_tool {
            self.reset();
            return Ok(None);
        }
        if self.stalled >= 3 {
            return Err(incomplete(
                "INCOMPLETE_RESPONSE",
                "模型在三次自动续接后仍未给出最终答复或实际工具调用；任务尚未完成",
            ));
        }
        if (!has_visible || dropped_tool || missing_final) && !commentary {
            if self.empty_recoveries >= 2 {
                return Err(incomplete(
                    "INCOMPLETE_RESPONSE",
                    "模型在两次恢复后仍返回空内容或缺失工具调用；任务尚未完成",
                ));
            }
            self.empty_recoveries += 1;
        }
        self.stalled += 1;
        Ok(Some(if commentary {
            ""
        } else if dropped_tool {
            DROPPED_TOOL_NUDGE
        } else {
            EMPTY_NUDGE
        }))
    }
}
