//! Vocabulary types for the code-execution seam: what a caller hands a
//! [`crate::index::CodeRuntime`] and what it gets back. Rust port of
//! `packages/code-runtime/code-runtime/src/types.ts`.
//!
//! # Deviations
//!
//! - `CodeJsonValue` is `serde_json::Value` (the repo-wide lossless JSON
//!   shape; the TS structural type admits exactly the same members).
//! - `AbortSignal` collapses into the repo-wide cancellation predicate
//!   ([`CodeAbort`]).

use std::sync::Arc;

use futures::future::BoxFuture;

/// The abort/cancellation predicate (the TS `AbortSignal` collapse).
pub type CodeAbort = Arc<dyn Fn() -> bool + Send + Sync>;

/// A lossless JSON value transferable through the dependency-light Service
/// Definition (TS `CodeJsonValue`).
pub type CodeJsonValue = serde_json::Value;

/// One host-side function exposed to the program as an async callable (TS
/// `CodeBindingFunction`). The runtime bridges calls to it (possibly across a
/// serialization boundary), so `args` and the resolution value MUST be
/// lossless JSON. A rejection of this function surfaces inside the program
/// as a rejection of the corresponding call. The Rust collapse: the future
/// panics (the repo-wide rejection channel).
pub type CodeBindingFunction =
    Arc<dyn Fn(CodeJsonValue) -> BoxFuture<'static, CodeJsonValue> + Send + Sync>;

/// Program-visible typed rejection for one binding namespace (TS
/// `CodeBindingErrorClass`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBindingErrorClass {
    /// Constructor global and resulting `Error.name`; same portable
    /// identifier rule as [`CodeBindingNamespace::global`].
    pub name: String,
    /// Non-empty own property for the member name. The portable exclusion
    /// set is [`crate::index::RESERVED_ERROR_MEMBERS`] plus dunder-form
    /// names, enforced identically by every backend.
    pub member_name_property: String,
}

/// A named group of [`CodeBindingFunction`]s the runtime exposes to the
/// program as one global object (e.g. `tools`) (TS `CodeBindingNamespace`).
#[derive(Clone)]
pub struct CodeBindingNamespace {
    /// The global identifier the program sees. Must match the
    /// LANGUAGE-PORTABLE identifier subset `[A-Za-z_][A-Za-z0-9_]*` and no
    /// language's reserved words.
    pub global: String,
    /// The callable members, keyed by the exact name the program calls.
    pub functions: Vec<(String, CodeBindingFunction)>,
    /// Optional program-visible typed rejection contract for this namespace.
    pub error_class: Option<CodeBindingErrorClass>,
}

/// One run: the program source plus everything the runtime acts on (TS
/// `CodeRunRequest`). Defaulting is the implementation's validated config —
/// a request carries no optional tuning knobs.
#[derive(Clone)]
pub struct CodeRunRequest {
    /// The program source, in the runtime's language. It runs as the body of
    /// an async function: top-level `await` and `return` are available, and
    /// the completion value becomes [`CodeRunResult::value`].
    pub program: String,
    /// Host functions exposed to the program, one global object per
    /// namespace.
    pub bindings: Vec<CodeBindingNamespace>,
    /// Abort the run: the runtime stops the program (hard, even mid-loop) and
    /// resolves with a [`CodeRunFailure`] of kind `abort`.
    pub signal: Option<CodeAbort>,
}

/// Why a run failed (TS `CodeRunFailureKind`). The kinds are orthogonal
/// outcomes reported independently: a budget expiry is not an exception, an
/// abort is not a timeout, and a substrate death is neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeRunFailureKind {
    /// The program threw or failed to parse/transform.
    Exception,
    /// An implementation-owned budget expired; the message says which.
    Timeout,
    /// [`CodeRunRequest::signal`] fired.
    Abort,
    /// The execution substrate died without settling (e.g. OOM).
    WorkerExit,
    /// The completion value was not lossless JSON.
    InvalidOutput,
    /// The serialized outer logs/value/diagnostic exceeded the configured
    /// cap.
    OutputLimit,
}

impl CodeRunFailureKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CodeRunFailureKind::Exception => "exception",
            CodeRunFailureKind::Timeout => "timeout",
            CodeRunFailureKind::Abort => "abort",
            CodeRunFailureKind::WorkerExit => "worker-exit",
            CodeRunFailureKind::InvalidOutput => "invalid-output",
            CodeRunFailureKind::OutputLimit => "output-limit",
        }
    }
}

/// One failure fact (TS `CodeRunFailure`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeRunFailure {
    /// The failure class (see [`CodeRunFailureKind`] for each kind's
    /// meaning).
    pub kind: CodeRunFailureKind,
    /// Human-readable detail, suitable for feeding back to a model to
    /// self-correct.
    pub message: String,
}

/// The outcome of one run. An error is a FIELD on a resolved result, never a
/// rejection of `run()` (TS `CodeRunResult`).
#[derive(Debug, Clone, Default)]
pub struct CodeRunResult {
    /// The program's completion value (its top-level `return`), when it ran
    /// to completion and the value crossed the runtime's lossless-JSON
    /// boundary.
    pub value: Option<CodeJsonValue>,
    /// Text the program emitted, in order, bounded only as part of the outer
    /// result.
    pub logs: Vec<String>,
    /// Present iff the run failed.
    pub error: Option<CodeRunFailure>,
}
