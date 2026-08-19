//! Service Definition for the user-questions capability seam
//! (`ctx.userQuestions`): a UI-backed service for pausing an agent tool call
//! until the human answers a question.
//! Rust port of `packages/interaction/user-questions/src/index.ts`
//! (+ `types.ts`).
//!
//! # Deviations
//!
//! - The abort seam is a predicate; an aborted request surfaces as
//!   `ASK_ABORTED`.

use std::sync::Arc;

use dsh_agent::Agent;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// The cancellation seam (TS `AbortSignal`).
pub type QuestionAbort = Arc<dyn Fn() -> bool + Send + Sync>;

/// One selectable answer offered to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserQuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A caller-declared presentation intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserQuestionIntent {
    pub kind: String,
    /// The option label that approves the plan.
    pub approve: String,
}

/// One question in a user-questions request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskUserQuestionItem {
    pub id: String,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<AskUserQuestionOption>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "multiSelect"
    )]
    pub multi_select: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<AskUserQuestionIntent>,
}

/// Answer to one question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserQuestionAnswerItem {
    pub id: String,
    pub selected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

/// The human's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserQuestionAnswer {
    pub answers: Vec<AskUserQuestionAnswerItem>,
}

/// Request for a human answer.
#[derive(Clone)]
pub struct AskUserQuestionRequest {
    pub questions: Vec<AskUserQuestionItem>,
    /// Exact live calling agent, when the request came from an agent tool
    /// call.
    pub agent: Option<Arc<dyn Agent>>,
    pub signal: Option<QuestionAbort>,
}

/// Stable error taxonomy for user-questions failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserQuestionError {
    pub code: String,
    pub message: String,
}

impl UserQuestionError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for UserQuestionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for UserQuestionError {}

/// UI-side provider for user questions.
#[async_trait::async_trait]
pub trait UserQuestionProvider: Send + Sync + 'static {
    async fn ask(
        &self,
        request: &AskUserQuestionRequest,
    ) -> Result<AskUserQuestionAnswer, UserQuestionError>;
}

/// `ctx.userQuestions`: one active UI provider plus an `ask()` API.
pub struct UserQuestionService {
    ctx: cordis::Context,
    provider: Arc<Mutex<Option<Arc<dyn UserQuestionProvider>>>>,
}

impl UserQuestionService {
    /// Create the service and register it as `userQuestions`.
    pub fn install(ctx: &cordis::Context) -> Arc<Self> {
        let service = Arc::new(Self {
            ctx: ctx.clone(),
            provider: Arc::new(Mutex::new(None)),
        });
        ctx.register_service(service.clone());
        service
    }

    /// Register the UI provider. Only one provider may be active in a
    /// context.
    pub fn register_provider(
        &self,
        provider: Arc<dyn UserQuestionProvider>,
    ) -> Result<cordis::Disposer, UserQuestionError> {
        {
            let mut slot = self.provider.lock();
            if slot.is_some() {
                return Err(UserQuestionError::new(
                    "DUPLICATE_PROVIDER",
                    "a user-questions provider is already registered",
                ));
            }
            *slot = Some(provider);
        }
        let service = self.provider.clone();
        Ok(cordis::make_disposer(move || {
            let service = service.clone();
            Box::pin(async move {
                service.lock().take();
            })
        }))
    }

    /// Ask the active UI provider and wait for the user's answer.
    pub async fn ask(
        &self,
        request: &AskUserQuestionRequest,
    ) -> Result<AskUserQuestionAnswer, UserQuestionError> {
        if request.signal.as_ref().is_some_and(|signal| signal()) {
            return Err(UserQuestionError::new(
                "ASK_ABORTED",
                "ask_user_question was aborted before the user answered",
            ));
        }
        if request.questions.is_empty() {
            return Err(UserQuestionError::new(
                "EMPTY_QUESTIONS",
                "ask_user_question requires at least one question",
            ));
        }
        if let Some(agent) = &request.agent {
            let agents = self
                .ctx
                .get_typed::<Arc<dsh_agent::AgentRegistry>>("agents", false)
                .map(|slot| slot.as_ref().clone());
            let live = agents
                .as_ref()
                .and_then(|registry| registry.get(agent.id()))
                .is_some_and(|live| Arc::ptr_eq(&live, agent));
            if !live {
                return Err(UserQuestionError::new(
                    "CALLER_NOT_LIVE",
                    "human interaction requires the exact live calling agent when an agent is supplied",
                ));
            }
            let rooted = agents.as_ref().is_some_and(|registry| {
                registry.roots().iter().any(|root| Arc::ptr_eq(root, agent))
            });
            if !rooted {
                return Err(UserQuestionError::new(
                    "DELEGATED_CALLER",
                    "human interaction is unavailable while the calling agent is owned by another live agent; include the unresolved question or decision in the child agent's final result",
                ));
            }
        }
        for question in &request.questions {
            let Some(intent) = &question.intent else {
                continue;
            };
            let offered = question
                .options
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .any(|option| option.label == intent.approve);
            if !offered {
                return Err(UserQuestionError::new(
                    "BAD_INTENT",
                    format!(
                        "question {} declares intent {} whose approve label {} names none of its options",
                        question.id,
                        intent.kind,
                        serde_json::to_string(&intent.approve).expect("label")
                    ),
                ));
            }
            if question.detail.is_none() {
                return Err(UserQuestionError::new(
                    "BAD_INTENT",
                    format!(
                        "question {} declares intent {} without the detail it reviews",
                        question.id, intent.kind
                    ),
                ));
            }
        }
        let provider = self.provider.lock().clone();
        let Some(provider) = provider else {
            return Err(UserQuestionError::new(
                "NO_PROVIDER",
                "no user-questions provider is registered",
            ));
        };
        provider.ask(request).await
    }
}

impl cordis::Service for UserQuestionService {
    fn service_name(&self) -> &'static str {
        "userQuestions"
    }
}
