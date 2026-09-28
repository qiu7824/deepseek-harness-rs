use dsh_llm::{ContentBlock, FinishReason, LlmFailure};
use serde_json::Value;

const EMPTY_NUDGE: &str = "上一条模型响应没有产生可交付的内容。继续处理当前请求，给出有内容的最终答复，或发出当前任务所需的完整工具调用；不要只说明将要继续。保持现有权限和用户约束。";
const DROPPED_TOOL_NUDGE: &str = "上一条响应声明需要调用工具，却没有提供任何工具调用，因此没有执行操作。若仍需操作，请重新发出完整的工具调用；否则给出实际结果。保持现有权限和用户约束。";
const TRUNCATION_NUDGE: &str = "上一条答复因单次输出上限而被截断。继续完成同一答复，从已输出内容的结尾继续，避免重复已有段落；保持原任务范围、权限和用户约束。";
const NO_ANSWER_NUDGE: &str = "上一轮达到单次输出上限，尚未产生可交付答复。请基于已有工作给出简洁的最终结果，明确未完成或无法验证的部分；不要重复长篇推理、重放已执行操作或扩大任务范围。保持现有权限和用户约束。";

/// Recovery is bounded independently from transport retry. Tool progress resets
/// stalled-response counters, but cannot replenish a turn's truncation budget.
#[derive(Default)]
pub(crate) struct ResponseContinuation {
    stalled: u8,
    empty_recoveries: u8,
    truncation_recoveries: u8,
    no_answer_recovery_used: bool,
    intent_recoveries: u8,
    requested_repetition: bool,
}

fn incomplete(code: &str, message: &str) -> LlmFailure {
    LlmFailure {
        offload_images: None,
        message: message.into(),
        code: code.into(),
        status: None,
        provider_retry_after_ms: None,
        request_id: None,
    }
}

impl ResponseContinuation {
    pub(crate) fn observe_user_intent(&mut self, messages: &[dsh_llm::UserMessage]) {
        for message in messages {
            if matches!(message.source, dsh_llm::MessageSource::User { .. }) {
                self.requested_repetition=message.content.iter().any(|block|matches!(block,ContentBlock::Text{text} if crate::repetition::explicitly_requested(text)));
            }
        }
    }
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
        can_act: bool,
    ) -> Result<Option<&'static str>, LlmFailure> {
        let has_text = content
            .iter()
            .any(|block| matches!(block, ContentBlock::Text { text } if !text.trim().is_empty()));
        if !self.requested_repetition
            && matches!(finish, FinishReason::Stop | FinishReason::MaxTokens)
            && content.iter().any(|block| match block {
                ContentBlock::Text { text } => {
                    (*finish == FinishReason::MaxTokens || text.len() >= 16_000)
                        && crate::repetition::dominated(text)
                }
                _ => false,
            })
        {
            return Err(incomplete(
                "OUTPUT_REPETITION",
                "模型输出陷入连续重复，已停止自动续写；收到的文字已保留，任务尚未完成",
            ));
        }
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
                if !self.no_answer_recovery_used && self.truncation_recoveries < 2 {
                    self.no_answer_recovery_used = true;
                    self.truncation_recoveries += 1;
                    return Ok(Some(NO_ANSWER_NUDGE));
                }
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
        let promised_action = can_act
            && *finish == FinishReason::Stop
            && content.iter().any(
                |block| matches!(block, ContentBlock::Text { text } if unfinished_action(text)),
            );
        if promised_action {
            if self.intent_recoveries >= 2 {
                return Err(incomplete(
                    "ACTION_NOT_EXECUTED",
                    "模型连续承诺继续操作，但没有执行；任务尚未完成",
                ));
            }
            self.intent_recoveries += 1;
            return Ok(Some(
                "上一条回复只承诺继续操作，但没有发出工具调用。若当前请求需要操作，请在现有权限内实际执行；否则直接说明结果或无法继续的原因。不要只说稍后继续，也不要扩大任务范围。",
            ));
        }
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

fn unfinished_action(text: &str) -> bool {
    let text = text.trim().trim_end_matches(['。', '.', '！', '!']);
    if text.chars().count() > 120 || text.contains(['\n', '?', '？', ':', '：', '`', '"', '“', '”'])
    {
        return false;
    }
    if [
        "已完成",
        "已经",
        "无法",
        "不能",
        "无需",
        "不需要",
        "don't",
        "cannot",
        "can't",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        return false;
    }
    let lower = text.to_lowercase();
    [
        "我再检查",
        "我先检查",
        "我继续检查",
        "我接下来检查",
        "我先运行",
        "我再运行",
        "我继续修复",
        "我先核对",
        "我继续核对",
        "let me check",
        "let me inspect",
        "let me run",
        "i'll check",
        "i will check",
        "i'll run",
        "i'll inspect",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

#[cfg(test)]
mod intent_tests {
    use super::*;
    #[test]
    fn promises_are_distinct_from_answers_quotes_and_blockers() {
        for text in ["我再检查一下。", "我先运行测试。", "Let me check the file."] {
            assert!(unfinished_action(text), "{text}");
        }
        for text in [
            "已完成检查。",
            "我无法继续检查。",
            "翻译：我再检查一下",
            "我先检查以下内容：\n1.版本",
            "我再检查一下？",
            "结果是 42。",
        ] {
            assert!(!unfinished_action(text), "{text}");
        }
    }
    #[test]
    fn promise_budget_survives_tool_progress_and_requires_tools() {
        let content = vec![ContentBlock::Text {
            text: "我再检查一下。".into(),
        }];
        let mut recovery = ResponseContinuation::default();
        assert!(
            recovery
                .observe(&FinishReason::Stop, None, &content, false, false)
                .unwrap()
                .is_none()
        );
        for _ in 0..2 {
            assert!(
                recovery
                    .observe(&FinishReason::Stop, None, &content, false, true)
                    .unwrap()
                    .is_some()
            );
            recovery.reset();
        }
        assert_eq!(
            recovery
                .observe(&FinishReason::Stop, None, &content, false, true)
                .unwrap_err()
                .code,
            "ACTION_NOT_EXECUTED"
        );
    }
}
