//! Compaction Service Definition (`ctx.compaction`): providers decide when
//! to compact and replace a history range with one summary node.
//! Rust port of `packages/compaction/compaction/src/index.ts`
//! (+ `brand.ts`, `checkpoint.ts`, `types.ts`).
//!
//! # Deviations
//!
//! - `CompactionEngine` is a trait on `Arc<dyn CompactionEngine>` (the TS
//!   abstract class), registered erased as the `compaction` service.
//! - The abort seam is a predicate; aborted requests surface as
//!   `ManualCompactionError { code: cancelled }`.

pub mod basic;
pub mod invariant;
pub mod tool_pairing;
pub use basic::BasicCompactionEngine;

use std::sync::Arc;

use dsh_brand::Branded;
use dsh_commands::CommandId;
use dsh_llm::{ContentBlock, MessageSource, TokenUsage};
use dsh_session::Session;

/// The brand marker for [`CompactionId`].
#[doc(hidden)]
pub enum CompactionIdTag {}

/// Stable identity shared by one compaction's complete durable lifecycle.
pub type CompactionId = Branded<CompactionIdTag>;

/// Brand a minted compaction id (TS `CompactionId`).
pub fn compaction_id(value: impl Into<String>) -> CompactionId {
    CompactionId::new(value)
}

/// Why automatic policy is asking a backend to consider compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionTrigger {
    Pressure,
    ContextOverflow,
}

impl CompactionTrigger {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompactionTrigger::Pressure => "pressure",
            CompactionTrigger::ContextOverflow => "context-overflow",
        }
    }
}

/// Expected failure classes for an explicit idle-session compaction request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualCompactionErrorCode {
    Busy,
    Cancelled,
    Changed,
    Summary,
    Commit,
    Persistence,
}

impl ManualCompactionErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ManualCompactionErrorCode::Busy => "busy",
            ManualCompactionErrorCode::Cancelled => "cancelled",
            ManualCompactionErrorCode::Changed => "changed",
            ManualCompactionErrorCode::Summary => "summary",
            ManualCompactionErrorCode::Commit => "commit",
            ManualCompactionErrorCode::Persistence => "persistence",
        }
    }
}

/// Expected manual-compaction failure suitable for a direct human-command
/// result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualCompactionError {
    pub code: ManualCompactionErrorCode,
    pub message: String,
}

impl ManualCompactionError {
    pub fn new(code: ManualCompactionErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ManualCompactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ManualCompactionError {}

/// Minimal agent context compaction needs without depending on the agent
/// package.
#[derive(Clone)]
pub struct CompactionAgentContext {
    pub session: Session,
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// Agent capability required to serialize an explicit idle-session
/// compaction against driver turns.
#[derive(Clone)]
pub struct ManualCompactAgentContext {
    pub session: Session,
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// Result of a successful compaction operation.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionResult {
    pub compaction_id: CompactionId,
    pub source_command_id: Option<CommandId>,
    pub start_seq: u64,
    pub summary_seq: u64,
    pub end_seq: u64,
    pub summary: Vec<ContentBlock>,
    /// Surface-boundary pair that was shadowed (a surface-POSITION span).
    pub shadowed_range: (u64, u64),
    /// The seqs of all shadowed surface nodes, in surface order.
    pub shadowed_seqs: Vec<u64>,
    pub shadowed_token_count: u64,
}

/// The cancellation seam (TS `AbortSignal`).
pub type CompactionAbort = Arc<dyn Fn() -> bool + Send + Sync>;

/// Create checkpoint provenance correlated with one compaction transaction
/// (TS `compactCheckpointSource`).
pub fn compact_checkpoint_source(
    compaction_id: &CompactionId,
    source_command_id: Option<&CommandId>,
) -> MessageSource {
    MessageSource::Plugin {
        plugin: "compact".to_string(),
        form: None,
        sections: None,
        summary: None,
        compaction_id: Some(compaction_id.as_str().to_string()),
        source_command_id: source_command_id.map(|id| id.as_str().to_string()),
    }
}

/// Test whether a persisted message source identifies a compaction
/// checkpoint (TS `isCompactCheckpointSource`).
pub fn is_compact_checkpoint_source(source: &MessageSource) -> bool {
    matches!(source, MessageSource::Plugin { plugin, .. } if plugin == "compact")
}

/// Abstract compaction service (TS `CompactionEngine`).
#[async_trait::async_trait]
pub trait CompactionEngine: Send + Sync + 'static {
    /// Consider automatic compaction for one explicit trigger.
    async fn compact_if_needed(
        &self,
        agent: &CompactionAgentContext,
        trigger: CompactionTrigger,
        signal: Option<&CompactionAbort>,
    ) -> Result<Option<CompactionResult>, ManualCompactionError>;

    /// Explicitly compact useful history even below automatic pressure
    /// thresholds.
    async fn compact_now(
        &self,
        agent: &ManualCompactAgentContext,
        signal: Option<&CompactionAbort>,
        source_command_id: Option<&CommandId>,
    ) -> Result<Option<CompactionResult>, ManualCompactionError>;

    /// Forcibly compact a range of surface nodes into a single summary node.
    async fn compact_region(
        &self,
        start: u64,
        end: u64,
        agent: &CompactionAgentContext,
        signal: Option<&CompactionAbort>,
    ) -> Result<CompactionResult, ManualCompactionError>;
}

impl cordis::Service for dyn CompactionEngine {
    fn service_name(&self) -> &'static str {
        "compaction"
    }
}

// Vocabulary anchors used by the seam's consumers.
pub use dsh_llm::TokenUsage as CompactionTokenUsageType;

/// The `compaction/summary` shadow-price facts (TS event data subset).
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionSummaryData {
    pub compaction_id: CompactionId,
    pub source_command_id: Option<CommandId>,
    pub summary: Vec<ContentBlock>,
    pub shadowed_range: (u64, u64),
    pub shadowed_seqs: Vec<u64>,
    pub shadowed_token_count: u64,
    pub provider: String,
    pub model: String,
    pub max_tokens: Option<u64>,
    pub usage: Option<TokenUsage>,
}

// Re-export anchors.
pub use crate::tool_pairing::{tool_pairing_balanced_after, tool_pairing_balanced_before};
