//! Stable identity shared by every attempt in one request-step retry chain.
//! Rust port of `packages/llm/llm-retry/src/brand.ts`.

use dsh_brand::Branded;

#[doc(hidden)]
pub enum RetryIdTag {}
/// Opaque retry-chain identity (TS `RetryId`).
pub type RetryId = Branded<RetryIdTag>;

/// Brand an implementation-minted retry-chain identity (TS `RetryId(id)`).
pub fn retry_id(id: impl Into<String>) -> RetryId {
    Branded::new(id)
}
