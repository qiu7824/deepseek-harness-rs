//! `downloads` domain contract: host-only download surfaces — the
//! GET-download channel family, the mirror of the SSE-stream `events`
//! domain. No wire envelope. Rust port of
//! `packages/host/apiproxy/src/api/downloads.ts`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use dsh_session::SessionId;

use crate::fetch::handler::{AbortSignal, DownloadResponse};

/// The session-log download request (the carrier's GET query boundary).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLogRequest {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_descendants: Option<bool>,
}

/// Host-only download surfaces (no wire envelope; absent from IApiClient).
#[async_trait]
pub trait DownloadsApi: Send + Sync {
    /// Stream one session-log ZIP — the root artifact verbatim plus each
    /// subagent descendant's — as an attachment response. The carrier's GET
    /// route answers this directly; the browser never calls it.
    async fn session_log(
        &self,
        request: SessionLogRequest,
        signal: AbortSignal,
    ) -> DownloadResponse;
}
