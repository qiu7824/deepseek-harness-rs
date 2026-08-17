//! Shared ownership of one E2B sandbox. Capability adapters await the same
//! SDK handle, so filesystem and process operations inhabit one remote
//! Linux world. Rust port of `packages/e2b/e2b`.
//!
//! # Deviations
//!
//! - The `e2b` npm SDK boundary collapses into the [`E2bSdk`]/
//!   [`E2bSandbox`] traits; no real HTTP backend exists yet.
//! - `SandboxNotFoundError` and the SDK error kinds collapse into
//!   [`E2bSdkError`]; consumers match the `not_found` kind.
//! - The API-key environment lookup is injectable (testable without
//!   process-global `E2B_API_KEY` mutation).

pub mod index;
pub mod invariant;

pub use index::{
    Config, E2B_INJECT, E2B_NAME, E2bBackgroundOptions, E2bCommandAbort, E2bCommandHandle,
    E2bCommandOptions, E2bCommandResult, E2bCreateOptions, E2bEntryInfo, E2bPlugin,
    E2bReadStream, E2bRuntime, E2bSandbox, E2bSdk, E2bSdkError, E2bSdkErrorKind, FileType,
    SandboxNotFoundError, e2b_control_envs, quote_e2b_shell_arg,
};
pub use invariant::{E2bInvariantPlugin, PACKAGE_NAME as E2B_INVARIANT_PACKAGE_NAME};
