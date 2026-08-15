//! Subprocess capability seam (`ctx.subprocess`). Rust port of
//! `@deepseek-ai/dsh-subprocess`.

pub mod index;
pub mod invariant;
pub mod types;

pub use index::{SubprocessRuntime, scrubbed_parent_env, sensitive_env_pattern};
pub use types::{
    CollectedOutput, DSH_ENV_PREFIX, DshEnvironment, DshEnvironmentKey, SubprocessAbort,
    SubprocessCollect, SubprocessCollectedOutputs, SubprocessHandle, SubprocessOutcome,
    SubprocessOutputMode, SubprocessOutputRead, SubprocessOutputReader, SubprocessSpawnSpec,
    SubprocessSpill, SubprocessStdinMode, SubprocessStdio, SubprocessTerminalForeground,
    SubprocessTerminalHandle, SubprocessTerminalSignal, SubprocessTerminalSpawnSpec,
};
