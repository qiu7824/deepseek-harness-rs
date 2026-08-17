//! `questions` domain contract. The question requested frame is a
//! server-request whose rpcId is the question's stable logical id; the
//! answer is a client-response echoing that rpcId, with no resource id in
//! the payload. Rust port of
//! `packages/host/apiproxy/src/api/questions.ts`.

use dsh_session::SessionId;
use dsh_user_questions::AskUserQuestionAnswer;
use serde::{Deserialize, Serialize};

/// Question answer payload (the result.value slot of a client-response):
/// answers one ask() as a whole batch (one ask, many questions, one answer
/// — never split per question).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionResponsePayload {
    pub session_id: SessionId,
    pub answer: AskUserQuestionAnswer,
}
