//! Bridge skill promotion to immutable task acceptance and the current runtime.
use dsh_host_apiproxy::skill_lifecycle::{SampleRef, SkillEvidenceVerifier, ValidationEvidence};
use dsh_shell::ExecutionProfileResolver;
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};

pub(crate) fn environment_fingerprint(
    ctx: &cordis::Context,
    session: Option<&str>,
    cwd: &str,
) -> Result<String, String> {
    let resolver = ctx
        .get_typed::<Arc<dyn ExecutionProfileResolver>>("executionProfiles", false)
        .ok_or("execution environment service unavailable")?;
    let profile = resolver.resolve(session, cwd)?;
    let policy = ctx
        .get_typed::<Arc<dsh_sandbox_policy::SandboxPolicyService>>("sandboxPolicy", false)
        .ok_or("sandbox policy unavailable")?;
    let session = match session {
        Some(id) => Some(Arc::new(
            ctx.get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
                .and_then(|s| s.get(&dsh_session::session_id(id)))
                .ok_or("restore the validation session before using its evidence")?,
        )),
        None => None,
    };
    let policy = policy.resolve(&dsh_sandbox_policy::SandboxPolicyRequest {
        session,
        mode: None,
    });
    let mut roots = policy.read_only_roots.clone();
    roots.sort();
    roots.dedup();
    let backend = if policy.mode == dsh_sandbox::SandboxMode::DangerFullAccess {
        "unconfined".to_string()
    } else {
        ctx.get_typed::<Arc<dyn dsh_sandbox::SandboxProvider>>("sandbox", false)
            .map(|provider| provider.backend_fingerprint_for(&policy))
            .unwrap_or_else(|| "unavailable".into())
    };
    let identity = serde_json::json!({"version":2,"contextId":profile.context_id,"backend":backend,"mode":policy.mode.as_str(),"workspace":policy.workspace_root,"readOnlyRoots":roots});
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&identity).map_err(|e| e.to_string())?)
    ))
}

struct Verifier {
    ctx: cordis::Context,
    tasks: Arc<crate::task_execution::TaskExecution>,
}
#[async_trait::async_trait]
impl SkillEvidenceVerifier for Verifier {
    async fn verify(
        &self,
        owner: &str,
        subject: &str,
        samples: &[SampleRef],
    ) -> Result<ValidationEvidence, String> {
        let session = self
            .ctx
            .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
            .and_then(|s| s.get(&dsh_session::session_id(owner)))
            .ok_or("validation session is unavailable")?;
        let cwd = session
            .header()
            .cwd
            .clone()
            .ok_or("validation session has no workspace")?;
        let project = std::fs::canonicalize(Path::new(&cwd))
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned();
        let fingerprint = environment_fingerprint(&self.ctx, Some(owner), &cwd)?;
        let mut references = Vec::new();
        let mut positive = 0;
        let mut negative = 0;
        for sample in samples {
            let task = self
                .tasks
                .verified_evidence(
                    owner,
                    &sample.task_id,
                    sample.revision,
                    &cwd,
                    Arc::new(|| false),
                )
                .await?;
            let validation_subject = task
                .spec
                .validation_subject
                .as_ref()
                .ok_or("sample task does not identify the tested skill")?;
            let expected = if sample.expected_success {
                "success"
            } else {
                "failure"
            };
            if validation_subject.kind != "skill"
                || validation_subject.identity != subject
                || validation_subject.expected_outcome != expected
            {
                return Err(
                    "sample task does not match this skill revision and expected outcome".into(),
                );
            }
            if task.spec.environment_fingerprint != fingerprint {
                return Err(
                    "sample environment changed; repeat the relevant verification task".into(),
                );
            }
            if sample.expected_success {
                positive += 1;
            } else {
                negative += 1;
            }
            references.push(format!("task:{}@{}", task.task_id, task.revision));
            references.extend(
                task.acceptance_results
                    .into_iter()
                    .flat_map(|r| r.evidence_refs),
            );
        }
        references.sort();
        references.dedup();
        Ok(ValidationEvidence {
            subject_identity: subject.into(),
            environment_fingerprint: fingerprint,
            project,
            checker_version: dsh_task_runtime::CHECKER_VERSION.into(),
            evidence_refs: references,
            positive_samples: positive,
            negative_samples: negative,
            all_matched: true,
        })
    }
    async fn environment_fingerprint(
        &self,
        project: &str,
        session: Option<&str>,
    ) -> Result<String, String> {
        environment_fingerprint(&self.ctx, session, project)
    }
}

pub(crate) fn install(
    ctx: &cordis::Context,
    capabilities: Arc<dsh_host_apiproxy::capabilities::CapabilityManager>,
    tasks: Arc<crate::task_execution::TaskExecution>,
    _profiles: Arc<crate::execution_profiles::ExecutionProfiles>,
) {
    capabilities
        .skill_lifecycle()
        .set_verifier(Arc::new(Verifier {
            ctx: ctx.clone(),
            tasks,
        }));
}
