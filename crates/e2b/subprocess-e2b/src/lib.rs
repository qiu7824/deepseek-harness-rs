//! E2B service provider for the subprocess capability seam: each handle
//! starts through the shared sandbox and retains command output/status
//! paths in that remote world. Rust port of
//! `packages/e2b/subprocess-e2b`.
//!
//! # Deviations
//!
//! - `AbortSignal` collapses into the seam-wide cancellation predicate
//!   ([`dsh_subprocess::SubprocessAbort`]); SDK callbacks become owned
//!   `Arc<dyn Fn>` chunk sinks.
//! - Node streams collapse into tokio byte streams (the seam vocabulary).
//! - `spawnTerminal` and the terminal ladder arrive with the terminal
//!   milestone; this crate currently serves `resolveExecutable`/`spawn`
//!   and the ordinary-process teardown ladder.

pub mod environment;
pub mod index;
pub mod output;
pub mod process;
pub mod remote;

pub use index::{Config, E2bSubprocessRuntime};
