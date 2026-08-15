//! Service Definition for the code-execution capability seam that runs one
//! model-written program against host async bindings. Runtimes know nothing
//! about tools or sessions; consumers own those concerns. Rust port of
//! `packages/code-runtime/code-runtime/src/index.ts`.

use std::sync::OnceLock;

use cordis::Service;
use futures::future::BoxFuture;

pub use crate::types::{
    CodeAbort, CodeBindingErrorClass, CodeBindingFunction, CodeBindingNamespace, CodeJsonValue,
    CodeRunFailure, CodeRunFailureKind, CodeRunRequest, CodeRunResult,
};

/// Binding globals EVERY backend refuses because SOME backend owns the slot
/// in the program's namespace (TS `RESERVED_BINDING_GLOBALS`).
pub fn reserved_binding_globals() -> &'static std::collections::HashSet<&'static str> {
    static SET: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        [
            "console",
            "__dsh_main__",
            "__builtins__",
            "__name__",
            "__debug__",
        ]
        .into_iter()
        .collect()
    })
}

/// `CodeBindingErrorClass.memberNameProperty` names EVERY backend refuses, as
/// one shared contract (TS `RESERVED_ERROR_MEMBERS`).
pub fn reserved_error_members() -> &'static std::collections::HashSet<&'static str> {
    static SET: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        [
            "name",
            "message",
            "stack",
            "args",
            "with_traceback",
            "add_note",
        ]
        .into_iter()
        .collect()
    })
}

/// Dunder form (`__x__`, non-empty middle): object-protocol slots in Python,
/// refused as error members on every backend (TS `DUNDER_MEMBER`).
pub fn is_dunder_member(name: &str) -> bool {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN
        .get_or_init(|| regex::Regex::new(r"^__.+__$").expect("valid regex"))
        .is_match(name)
}

/// Reserved words of every portable target language (ECMAScript ∪ Python),
/// refused as binding globals / error-class names by all backends (TS
/// `PORTABLE_RESERVED_WORDS`).
pub fn portable_reserved_words() -> &'static std::collections::HashSet<&'static str> {
    static SET: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        [
            // ECMAScript reserved words and reserved-in-strict-mode names.
            "await", "break", "case", "catch", "class", "const", "continue", "debugger",
            "default", "delete", "do", "else", "enum", "export", "extends", "false", "finally",
            "for", "function", "if", "import", "in", "instanceof", "new", "null", "return",
            "super", "switch", "this", "throw", "true", "try", "typeof", "var", "void", "while",
            "with", "yield", "let", "static", "implements", "interface", "package", "private",
            "protected", "public", "arguments", "eval",
            // Python 3.x keywords and soft keywords.
            "False", "None", "True", "and", "as", "assert", "async", "def", "del", "elif",
            "except", "from", "global", "is", "lambda", "nonlocal", "not", "or", "pass", "raise",
            "match", "type", "_",
        ]
        .into_iter()
        .collect()
    })
}

/// Registers one `ctx.codeRuntime` implementation (TS `CodeRuntime`).
/// Program, budget, abort, and substrate failures resolve in
/// [`CodeRunResult`]; only Service Definition contract misuse rejects.
/// Implementations bridge structured-cloneable bindings, materialize each
/// declared namespace rejection class, treat programs as hostile peers,
/// isolate runs from one another, and terminate and await in-flight runs
/// during disposal.
pub trait CodeRuntime: Send + Sync + 'static {
    /// The source language `run` expects `program` to be written in, as a
    /// lowercase identifier. Informational, not gating. Well-known values:
    /// `typescript` and `python`.
    fn language(&self) -> String;

    /// The execution substrate, as a lowercase identifier. Informational,
    /// not gating. Well-known values: `worker-thread`, `process`,
    /// `container`.
    fn isolation(&self) -> String;

    /// Execute one program against the request's bindings and capture what
    /// it emitted. The error is a result field; a `Err` return means Service
    /// Definition contract misuse only.
    fn run(&self, request: CodeRunRequest) -> BoxFuture<'static, Result<CodeRunResult, String>>;
}

impl Service for dyn CodeRuntime {
    fn service_name(&self) -> &'static str {
        "codeRuntime"
    }
}
