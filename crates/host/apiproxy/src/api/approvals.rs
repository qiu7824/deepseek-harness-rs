//! `approvals` domain contract. The approval requested frame is a
//! server-request (stable rpcId); the answer is a client-response echoing
//! that rpcId (not a unary method, not in RpcMethodMap, mints no new id).
//! Rust port of `packages/host/apiproxy/src/api/approvals.ts`.

use dsh_session::SessionId;
use dsh_user_approval::ApprovalRequestId;
use serde::{Deserialize, Serialize};

/// The outcome values a client can give (cancelled/unavailable are
/// host-side outcomes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalClientOutcome {
    AllowedOnce,
    AllowedAlways,
    Rejected,
}

/// Approval answer payload (the result.value slot of a client-response).
/// `approvalId` is the core audit correlation; wire correlation is governed
/// by the echoed rpcId.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalResponsePayload {
    pub session_id: SessionId,
    pub approval_id: ApprovalRequestId,
    pub outcome: ApprovalClientOutcome,
}
