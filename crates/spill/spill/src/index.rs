//! Service Definition for the spill storage capability seam
//! (`ctx.spillStore`): an abstract service defining WHAT a spill backend
//! does — persist a tool's oversized text and return a model-facing locator
//! plus retrieval guidance — without saying HOW. Rust port of
//! `packages/spill/spill/src/index.ts`.
//!
//! Implementations implement [`SpillStore`] and register as the `spillStore`
//! service; `dsh-spill-local` (host filesystem) is the first.
//!
//! The Service Definition is deliberately minimal: `save_text` and nothing
//! else. It owns NO retention policy (that is `dsh-output-retention`), NO
//! tool-result replacement (that is `dsh-spill-policy`), and NO retrieval or
//! search API. The backend supplies the locator and retrieval hint
//! appropriate for its storage substrate.

use cordis::Service;

use crate::types::{SaveTextSpill, SpillRef};

/// Abstract spill storage service (TS `SpillStore`). Semantics every
/// implementation must honor:
///
/// - [`SpillStore::save_text`] persists the FULL `content` verbatim and
///   returns an opaque locator, exact byte length, and model-facing
///   retrieval guidance.
/// - Storage is scoped by the request's owner session; the backend chooses a
///   private (not world-readable) location and a collision-free name derived
///   from — never equal to — the caller's `suggested_name`.
/// - `save_text` REJECTS on a real storage failure (permissions, ENOSPC,
///   backend unavailable); the caller decides how to degrade (the spill
///   policy treats a rejection as best-effort and keeps the inline result).
#[async_trait::async_trait]
pub trait SpillStore: Send + Sync + 'static {
    /// Persist `input.content` to a session-scoped spill artifact.
    /// Returns the saved artifact's [`SpillRef`]; rejects on a storage
    /// failure.
    async fn save_text(&self, input: &SaveTextSpill) -> Result<SpillRef, String>;
}

impl Service for dyn SpillStore {
    fn service_name(&self) -> &'static str {
        "spillStore"
    }
}
