//! Host-owned approval/question pending registries behind the answerable mux
//! frames and `POST /api/respond`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use cordis::{Context, EventOptions, NextFn, arc, downcast, downcast_arc, make_disposer};
use dsh_user_approval::{ApprovalOutcome, ApprovalRequest, ApprovalRequestId, approval_request_id};
use dsh_user_questions::{
    AskUserQuestionAnswer, AskUserQuestionItem, AskUserQuestionRequest, UserQuestionError,
    UserQuestionProvider, UserQuestionService,
};
use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};

use crate::api::approvals::{ApprovalClientOutcome, ApprovalResponsePayload};
use crate::api::questions::QuestionResponsePayload;
use crate::api::rpc::{
    ClientResponse, False, RpcId, RpcReceipt, RpcReceiptReason, True, WireRpcResult, rpc_id,
};
use crate::fetch::handler::FrameRequest;

static NEXT_INTERACTION_ID: AtomicU64 = AtomicU64::new(0);

fn fresh_rpc_id() -> RpcId {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let counter = NEXT_INTERACTION_ID.fetch_add(1, Ordering::SeqCst);
    rpc_id(format!("interaction-{nanos:x}-{counter:x}"))
}

struct PendingQuestion {
    rpc_id: RpcId,
    session_id: dsh_session::SessionId,
    questions: Vec<AskUserQuestionItem>,
    resolve: oneshot::Sender<Result<AskUserQuestionAnswer, UserQuestionError>>,
}

struct PendingApproval {
    rpc_id: RpcId,
    session_id: dsh_session::SessionId,
    approval_id: ApprovalRequestId,
    tool_name: String,
    call_id: Option<String>,
    reason: Option<String>,
    grant_key: Option<String>,
    rememberable: bool,
    resolve: oneshot::Sender<ApprovalOutcome>,
}

#[derive(Default)]
struct InteractionInner {
    pending_questions: HashMap<RpcId, PendingQuestion>,
    pending_approvals: HashMap<RpcId, PendingApproval>,
    mux_queues: HashMap<u64, mpsc::UnboundedSender<FrameRequest>>,
    next_mux_queue: u64,
    disposed: bool,
}

/// Shared pending registry. All ownership transitions happen under `inner`;
/// async waiters and channel delivery happen after the lock is released.
#[derive(Default)]
pub(crate) struct InteractionState {
    inner: Mutex<InteractionInner>,
}

pub(crate) struct MuxSubscription {
    state: Arc<InteractionState>,
    id: u64,
}

/// Resources whose lifetime is exactly one mux stream.
pub(crate) struct MuxResources {
    _subscription: MuxSubscription,
    listener_disposers: Vec<cordis::Disposer>,
}

impl MuxResources {
    pub(crate) fn new(
        subscription: MuxSubscription,
        listener_disposers: Vec<cordis::Disposer>,
    ) -> Self {
        Self {
            _subscription: subscription,
            listener_disposers,
        }
    }
}

impl Drop for MuxResources {
    fn drop(&mut self) {
        for disposer in self.listener_disposers.drain(..).rev() {
            let future = disposer();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                drop(runtime.spawn(future));
            } else {
                futures::executor::block_on(future);
            }
        }
    }
}

impl Drop for MuxSubscription {
    fn drop(&mut self) {
        self.state.inner.lock().mux_queues.remove(&self.id);
    }
}

struct QuestionPendingGuard {
    state: Arc<InteractionState>,
    rpc_id: RpcId,
}

impl Drop for QuestionPendingGuard {
    fn drop(&mut self) {
        self.state.cancel_question(
            &self.rpc_id,
            UserQuestionError::new(
                "ASK_ABORTED",
                "ask_user_question was aborted before the user answered",
            ),
        );
    }
}

struct ApprovalPendingGuard {
    state: Arc<InteractionState>,
    rpc_id: RpcId,
}

impl Drop for ApprovalPendingGuard {
    fn drop(&mut self) {
        let _ = self
            .state
            .settle_approval(&self.rpc_id, ApprovalOutcome::Cancelled);
    }
}

struct WebQuestionProvider {
    state: Arc<InteractionState>,
}

#[async_trait::async_trait]
impl UserQuestionProvider for WebQuestionProvider {
    async fn ask(
        &self,
        request: &AskUserQuestionRequest,
    ) -> Result<AskUserQuestionAnswer, UserQuestionError> {
        let Some(agent) = request.agent.as_ref() else {
            return Err(UserQuestionError::new(
                "ASK_MISSING_AGENT",
                "web user interaction requires an agent-owned session",
            ));
        };
        if request.signal.as_ref().is_some_and(|signal| signal()) {
            return Err(UserQuestionError::new(
                "ASK_ABORTED",
                "ask_user_question was aborted before the user answered",
            ));
        }

        let rpc_id = fresh_rpc_id();
        let (resolve, mut answer) = oneshot::channel();
        self.state.publish_question(PendingQuestion {
            rpc_id: rpc_id.clone(),
            session_id: agent.id().clone(),
            questions: request.questions.clone(),
            resolve,
        })?;
        let _guard = QuestionPendingGuard {
            state: Arc::clone(&self.state),
            rpc_id: rpc_id.clone(),
        };

        let Some(signal) = request.signal.clone() else {
            return answer.await.unwrap_or_else(|_| {
                Err(UserQuestionError::new(
                    "ASK_ABORTED",
                    "web user-questions provider was disposed",
                ))
            });
        };
        loop {
            tokio::select! {
                result = &mut answer => {
                    return result.unwrap_or_else(|_| {
                        Err(UserQuestionError::new(
                            "ASK_ABORTED",
                            "web user-questions provider was disposed",
                        ))
                    });
                }
                _ = tokio::time::sleep(Duration::from_millis(15)) => {
                    if signal() {
                        let error = UserQuestionError::new(
                            "ASK_ABORTED",
                            "ask_user_question was aborted before the user answered",
                        );
                        if self.state.cancel_question(&rpc_id, error.clone()) {
                            return Err(error);
                        }
                        // Another claimant already removed the pending entry.
                        // Its oneshot result is authoritative: in particular,
                        // an accepted response cannot be overwritten by a
                        // cancellation predicate observed at the same time.
                        return answer.await.unwrap_or_else(|_| {
                            Err(UserQuestionError::new(
                                "ASK_ABORTED",
                                "web user-questions provider was disposed",
                            ))
                        });
                    }
                }
            }
        }
    }
}

impl InteractionState {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Install the UI question provider and approval waterfall answerer when
    /// their service seams are already composed. Registration itself is
    /// synchronous, so `ApiProxyService::install` returns ready to answer.
    pub(crate) fn activate(self: &Arc<Self>, ctx: &Context) {
        if let Some(questions) = ctx
            .get_typed::<Arc<UserQuestionService>>("userQuestions", false)
            .map(|slot| slot.as_ref().clone())
        {
            let disposer = questions
                .register_provider(Arc::new(WebQuestionProvider {
                    state: Arc::clone(self),
                }))
                .unwrap_or_else(|error| panic!("api-proxy: {error}"));
            let _ = ctx.fiber.disposables.push(disposer);
        }

        if ctx.get("approval", false).is_some() {
            let state = Arc::clone(self);
            let listener: Arc<cordis::Listener> = Arc::new(
                move |_dispatch_ctx: &Context,
                      args: Vec<cordis::ArcValue>|
                      -> cordis::BoxFuture<'static, Option<cordis::ArcValue>> {
                    let state = Arc::clone(&state);
                    Box::pin(async move {
                        let request = args
                            .first()
                            .and_then(|value| downcast::<ApprovalRequest>(value))
                            .cloned();
                        let next = args.last().and_then(downcast_arc::<NextFn>);
                        let Some(request) = request else {
                            return match next {
                                Some(next) => Some(next.call().await),
                                None => None,
                            };
                        };
                        if request.signal.as_ref().is_some_and(|signal| signal()) {
                            return Some(arc(ApprovalOutcome::Cancelled));
                        }
                        let Some((rpc_id, mut outcome)) = state.publish_approval(&request) else {
                            return match next {
                                Some(next) => Some(next.call().await),
                                None => None,
                            };
                        };
                        let _guard = ApprovalPendingGuard {
                            state: Arc::clone(&state),
                            rpc_id: rpc_id.clone(),
                        };
                        let Some(signal) = request.signal.clone() else {
                            return Some(arc(outcome
                                .await
                                .unwrap_or(ApprovalOutcome::Unavailable)));
                        };
                        loop {
                            tokio::select! {
                                biased;
                                result = &mut outcome => {
                                    return Some(arc(result.unwrap_or(ApprovalOutcome::Unavailable)));
                                }
                                _ = tokio::time::sleep(Duration::from_millis(15)) => {
                                    if signal() {
                                        if state.settle_approval(&rpc_id, ApprovalOutcome::Cancelled) {
                                            return Some(arc(ApprovalOutcome::Cancelled));
                                        }
                                        // A response or teardown already claimed the
                                        // entry. Preserve that claimant's authoritative
                                        // outcome instead of overwriting it with a late
                                        // observation of the cancellation predicate.
                                        return Some(arc(outcome
                                            .await
                                            .unwrap_or(ApprovalOutcome::Unavailable)));
                                    }
                                }
                            }
                        }
                    })
                },
            );
            ctx.events.register(
                ctx,
                "api-proxy: approval answerer",
                "approval/request",
                listener,
                &EventOptions::default(),
            );
        }

        let state = Arc::clone(self);
        let cleanup = make_disposer(move || {
            let state = Arc::clone(&state);
            Box::pin(async move { state.dispose() })
        });
        let _ = ctx.fiber.disposables.push(cleanup);
    }

    pub(crate) fn subscribe(
        self: &Arc<Self>,
        tx: mpsc::UnboundedSender<FrameRequest>,
    ) -> MuxSubscription {
        let mut inner = self.inner.lock();
        let id = inner.next_mux_queue;
        inner.next_mux_queue = inner.next_mux_queue.wrapping_add(1);
        inner.mux_queues.insert(id, tx.clone());
        for pending in inner.pending_questions.values() {
            let _ = tx.send(question_requested_frame(pending));
        }
        for pending in inner.pending_approvals.values() {
            let _ = tx.send(approval_requested_frame(pending));
        }
        MuxSubscription {
            state: Arc::clone(self),
            id,
        }
    }

    fn publish_question(&self, pending: PendingQuestion) -> Result<(), UserQuestionError> {
        let mut inner = self.inner.lock();
        if inner.disposed {
            return Err(UserQuestionError::new(
                "ASK_ABORTED",
                "web user-questions provider was disposed",
            ));
        }
        let frame = question_requested_frame(&pending);
        inner
            .pending_questions
            .insert(pending.rpc_id.clone(), pending);
        broadcast_locked(&mut inner, frame);
        Ok(())
    }

    fn publish_approval(
        &self,
        request: &ApprovalRequest,
    ) -> Option<(RpcId, oneshot::Receiver<ApprovalOutcome>)> {
        let mut inner = self.inner.lock();
        if inner.disposed {
            return None;
        }
        let claimed: HashSet<String> = inner
            .pending_approvals
            .values()
            .map(|pending| pending.approval_id.as_str().to_string())
            .collect();
        let mut decided = HashSet::new();
        let mut approval_id = None;
        for event in request.agent.session().events().iter().rev() {
            match event.type_.as_str() {
                "approval/decided" => {
                    if let Some(id) = event.data.get("id").and_then(serde_json::Value::as_str) {
                        decided.insert(id.to_string());
                    }
                }
                "approval/asked" => {
                    let Some(id) = event.data.get("id").and_then(serde_json::Value::as_str) else {
                        continue;
                    };
                    if claimed.contains(id) || decided.contains(id) {
                        continue;
                    }
                    let event_call_id =
                        event.data.get("callId").and_then(serde_json::Value::as_str);
                    if request.call_id.as_deref() != event_call_id {
                        continue;
                    }
                    approval_id = Some(approval_request_id(id));
                    break;
                }
                _ => {}
            }
        }
        let approval_id = approval_id?;
        let rpc_id = fresh_rpc_id();
        let (resolve, outcome) = oneshot::channel();
        let pending = PendingApproval {
            rpc_id: rpc_id.clone(),
            session_id: request.agent.session().id().clone(),
            approval_id,
            tool_name: request.tool_name.clone(),
            call_id: request.call_id.clone(),
            reason: request.reason.clone(),
            grant_key: request.grant_key.clone(),
            rememberable: request.rememberable,
            resolve,
        };
        let frame = approval_requested_frame(&pending);
        inner.pending_approvals.insert(rpc_id.clone(), pending);
        broadcast_locked(&mut inner, frame);
        Some((rpc_id, outcome))
    }

    fn cancel_question(&self, rpc_id: &RpcId, error: UserQuestionError) -> bool {
        let pending = {
            let mut inner = self.inner.lock();
            let pending = inner.pending_questions.remove(rpc_id);
            if let Some(pending) = pending.as_ref() {
                let frame =
                    question_resolved_frame(&pending.session_id, &pending.rpc_id, "cancelled");
                broadcast_locked(&mut inner, frame);
            }
            pending
        };
        let claimed = pending.is_some();
        if let Some(pending) = pending {
            let _ = pending.resolve.send(Err(error));
        }
        claimed
    }

    fn settle_approval(&self, rpc_id: &RpcId, outcome: ApprovalOutcome) -> bool {
        let pending = {
            let mut inner = self.inner.lock();
            let pending = inner.pending_approvals.remove(rpc_id);
            if let Some(pending) = pending.as_ref() {
                let frame =
                    approval_resolved_frame(&pending.session_id, &pending.approval_id, outcome);
                broadcast_locked(&mut inner, frame);
            }
            pending
        };
        let claimed = pending.is_some();
        if let Some(pending) = pending {
            let _ = pending.resolve.send(outcome);
        }
        claimed
    }

    pub(crate) fn respond(&self, response: ClientResponse) -> RpcReceipt {
        let rpc_id = response.rpc_id;
        let result = response.result;

        let mut inner = self.inner.lock();
        if let Some(pending) = inner.pending_approvals.get(&rpc_id) {
            let WireRpcResult::Ok {
                value: Some(value), ..
            } = result
            else {
                return bad_response();
            };
            let Ok(payload) = serde_json::from_value::<ApprovalResponsePayload>(value) else {
                return bad_response();
            };
            if payload.session_id != pending.session_id
                || payload.approval_id != pending.approval_id
            {
                return bad_response();
            }
            let outcome = match payload.outcome {
                ApprovalClientOutcome::AllowedOnce => ApprovalOutcome::AllowedOnce,
                ApprovalClientOutcome::AllowedAlways => ApprovalOutcome::AllowedAlways,
                ApprovalClientOutcome::Rejected => ApprovalOutcome::Rejected,
            };
            let pending = inner
                .pending_approvals
                .remove(&rpc_id)
                .expect("pending approval under the same lock");
            let frame = approval_resolved_frame(&pending.session_id, &pending.approval_id, outcome);
            broadcast_locked(&mut inner, frame);
            drop(inner);
            let _ = pending.resolve.send(outcome);
            return accepted();
        }

        let Some(pending) = inner.pending_questions.get(&rpc_id) else {
            return not_pending();
        };
        match result {
            WireRpcResult::Err { error, .. } => {
                if error.code().as_str() != "cancelled" {
                    return bad_response();
                }
                let pending = inner
                    .pending_questions
                    .remove(&rpc_id)
                    .expect("pending question under the same lock");
                let frame =
                    question_resolved_frame(&pending.session_id, &pending.rpc_id, "cancelled");
                broadcast_locked(&mut inner, frame);
                drop(inner);
                let _ = pending.resolve.send(Err(UserQuestionError::new(
                    "ASK_CANCELLED",
                    "the user cancelled ask_user_question",
                )));
                accepted()
            }
            WireRpcResult::Ok {
                value: Some(value), ..
            } => {
                let Ok(payload) = serde_json::from_value::<QuestionResponsePayload>(value) else {
                    return bad_response();
                };
                if !matches_questions(&payload, pending) {
                    return bad_response();
                }
                let pending = inner
                    .pending_questions
                    .remove(&rpc_id)
                    .expect("pending question under the same lock");
                let frame =
                    question_resolved_frame(&pending.session_id, &pending.rpc_id, "answered");
                broadcast_locked(&mut inner, frame);
                drop(inner);
                let _ = pending.resolve.send(Ok(payload.answer));
                accepted()
            }
            WireRpcResult::Ok { value: None, .. } => bad_response(),
        }
    }

    fn dispose(&self) {
        let (questions, approvals) = {
            let mut inner = self.inner.lock();
            if inner.disposed {
                return;
            }
            inner.disposed = true;
            let questions = inner
                .pending_questions
                .drain()
                .map(|(_, value)| value)
                .collect::<Vec<_>>();
            let approvals = inner
                .pending_approvals
                .drain()
                .map(|(_, value)| value)
                .collect::<Vec<_>>();
            for pending in &questions {
                let frame =
                    question_resolved_frame(&pending.session_id, &pending.rpc_id, "cancelled");
                broadcast_locked(&mut inner, frame);
            }
            for pending in &approvals {
                let frame = approval_resolved_frame(
                    &pending.session_id,
                    &pending.approval_id,
                    ApprovalOutcome::Cancelled,
                );
                broadcast_locked(&mut inner, frame);
            }
            (questions, approvals)
        };
        for pending in questions {
            let _ = pending.resolve.send(Err(UserQuestionError::new(
                "ASK_ABORTED",
                "web user-questions provider was disposed",
            )));
        }
        for pending in approvals {
            let _ = pending.resolve.send(ApprovalOutcome::Cancelled);
        }
    }
}

fn matches_questions(payload: &QuestionResponsePayload, pending: &PendingQuestion) -> bool {
    if payload.session_id != pending.session_id
        || payload.answer.answers.len() != pending.questions.len()
    {
        return false;
    }
    payload
        .answer
        .answers
        .iter()
        .zip(&pending.questions)
        .all(|(answer, question)| {
            if answer.id != question.id {
                return false;
            }
            let selected: HashSet<&str> = answer.selected.iter().map(String::as_str).collect();
            if selected.len() != answer.selected.len() {
                return false;
            }
            let custom = answer.custom.as_deref().map(str::trim);
            if custom.is_some_and(str::is_empty) {
                return false;
            }
            if question.multi_select != Some(true)
                && (custom.is_some() && !answer.selected.is_empty() || answer.selected.len() > 1)
            {
                return false;
            }
            let labels: HashSet<&str> = question
                .options
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|option| option.label.as_str())
                .collect();
            answer
                .selected
                .iter()
                .all(|label| labels.contains(label.as_str()))
        })
}

fn question_requested_frame(pending: &PendingQuestion) -> FrameRequest {
    FrameRequest {
        rpc_id: pending.rpc_id.clone(),
        payload: serde_json::json!({
            "type": "question/requested",
            "sessionId": pending.session_id,
            "questions": pending.questions,
        }),
    }
}

fn approval_requested_frame(pending: &PendingApproval) -> FrameRequest {
    let mut payload = serde_json::json!({
        "type": "approval/requested",
        "sessionId": pending.session_id,
        "approvalId": pending.approval_id,
        "toolName": pending.tool_name,
        "rememberable": pending.rememberable,
    });
    if let Some(call_id) = &pending.call_id {
        payload["callId"] = serde_json::json!(call_id);
    }
    if let Some(reason) = &pending.reason {
        payload["reason"] = serde_json::json!(reason);
    }
    if let Some(grant_key) = &pending.grant_key {
        payload["grantKey"] = serde_json::json!(grant_key);
    }
    FrameRequest {
        rpc_id: pending.rpc_id.clone(),
        payload,
    }
}

fn question_resolved_frame(
    session_id: &dsh_session::SessionId,
    question_rpc_id: &RpcId,
    outcome: &str,
) -> FrameRequest {
    FrameRequest {
        rpc_id: fresh_rpc_id(),
        payload: serde_json::json!({
            "type": "question/resolved",
            "sessionId": session_id,
            "questionRpcId": question_rpc_id,
            "outcome": outcome,
        }),
    }
}

fn approval_resolved_frame(
    session_id: &dsh_session::SessionId,
    approval_id: &ApprovalRequestId,
    outcome: ApprovalOutcome,
) -> FrameRequest {
    FrameRequest {
        rpc_id: fresh_rpc_id(),
        payload: serde_json::json!({
            "type": "approval/resolved",
            "sessionId": session_id,
            "approvalId": approval_id,
            "outcome": outcome,
        }),
    }
}

fn broadcast_locked(inner: &mut InteractionInner, frame: FrameRequest) {
    inner
        .mux_queues
        .retain(|_, queue| queue.send(frame.clone()).is_ok());
}

fn accepted() -> RpcReceipt {
    RpcReceipt::Accepted { accepted: True }
}

fn not_pending() -> RpcReceipt {
    RpcReceipt::Rejected {
        accepted: False,
        reason: RpcReceiptReason::NotPending,
    }
}

fn bad_response() -> RpcReceipt {
    RpcReceipt::Rejected {
        accepted: False,
        reason: RpcReceiptReason::BadResponse,
    }
}
