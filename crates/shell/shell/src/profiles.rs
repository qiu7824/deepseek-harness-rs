//! Environment choices resolved once at the tool boundary. This service
//! selects executables; it never grants permissions or changes sandbox policy.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct ResolvedExecutionProfile {
    pub context_id: String,
    pub shell_path: Option<String>,
    pub shell_kind: String,
    pub python_path: Option<String>,
    pub toolchain_paths: BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct ExecutionValidationRequest {
    pub session_id: Option<String>,
    pub workdir: String,
    pub capability: String,
    pub executable: Option<String>,
    pub shell_kind: Option<String>,
    pub execution_context_id: Option<String>,
    pub sandbox_policy: Option<dsh_sandbox::SandboxExecutionPolicy>,
    pub signal: Option<crate::ShellAbort>,
}

pub trait ExecutionProfileResolver: Send + Sync + 'static {
    fn resolve(
        &self,
        session_id: Option<&str>,
        workdir: &str,
    ) -> Result<ResolvedExecutionProfile, String>;

    fn validate(
        &self,
        _request: ExecutionValidationRequest,
    ) -> futures::future::BoxFuture<'static, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn report_failure(&self, _context_id: &str, _capability: &str) {}
}

impl cordis::Service for dyn ExecutionProfileResolver {
    fn service_name(&self) -> &'static str {
        "executionProfiles"
    }
}
