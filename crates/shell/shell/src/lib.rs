//! Bash executor capability seam (`ctx.shell`): foreground runs, background
//! process handles, and the shared exit-status render contract. Rust port of
//! `packages/shell/shell`.

pub mod diagnostics;
pub mod index;
pub mod invariant;
pub mod profiles;
pub mod powershell;
pub mod render;
pub mod types;

pub use diagnostics::{ExecutionDiagnostic, application_diagnostics, validate_native_argv};
pub use index::{CollectedOutput, ShellExecutor, shell_settings_namespace};
pub use profiles::{
    ExecutionProfileResolver, ExecutionValidationRequest, ResolvedExecutionProfile,
};
pub use render::{ParsedExitStatus, parse_exit_status};
pub use types::{
    DSH_ENV_PREFIX, DshEnvironment, DshEnvironmentKey, ShellAbort, ShellExecRequest, ShellExecSpec,
    ShellProcess, ShellProcessRead, ShellProcessStatus, ShellRunResult, ShellSandboxInfo,
};
