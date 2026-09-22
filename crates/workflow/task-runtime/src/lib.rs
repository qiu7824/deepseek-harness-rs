//! Durable business acceptance and effect facts, independent of session event durability.
//! Callers own authorization and supply file bytes through their filesystem provider.
//! Agents cannot change requirements; explicit user revisions retain their audit history.
mod checks;
pub mod evaluation;
pub mod images;
pub mod office;
mod store;
pub use checks::{check_bytes, check_file, check_tool_result, digest};
pub use store::TaskRuntime;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const FORMAT_VERSION: u32 = 1;
pub const CHECKER_VERSION: &str = "dsh-acceptance-v1";
pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Planned,
    Running,
    Validating,
    ValidationFailed,
    AwaitingUser,
    Completed,
    Cancelled,
    Blocked,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Prepared,
    Dispatched,
    Running,
    EffectObserved,
    Verified,
    Committed,
    Failed,
    Unknown,
    NotDispatched,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    ReadOnly,
    Idempotent,
    Write,
    External,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceStatus {
    Passed,
    Failed,
    Unverified,
    AwaitingUser,
}

/// Every path is interpreted by the selected filesystem provider, not host std::fs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Checker {
    Text {
        path: String,
        required: Vec<String>,
        #[serde(default)]
        forbidden: Vec<String>,
    },
    Json {
        path: String,
        assertions: BTreeMap<String, Value>,
    },
    Image {
        path: String,
        min_width: u32,
        min_height: u32,
        #[serde(default)]
        channels: Option<u8>,
    },
    OfficePackage {
        path: String,
        format: String,
    },
    /// Runtime-captured result + assertions; exit success alone is never acceptance.
    ToolResult {
        step_id: String,
        assertions: BTreeMap<String, Value>,
    },
    Manual {
        reason: String,
    },
}
impl Checker {
    pub fn version(&self) -> &'static str {
        match self {
            Self::OfficePackage { .. } => "dsh-acceptance-office-v2",
            _ => CHECKER_VERSION,
        }
    }

    fn evidence_current(&self, result: &AcceptanceResult) -> bool {
        // Human confirmation belongs to its input identity, not to parser code.
        matches!(self, Self::Manual { .. }) || result.checker_version == self.version()
    }
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Text { path, .. }
            | Self::Json { path, .. }
            | Self::Image { path, .. }
            | Self::OfficePackage { path, .. } => Some(path),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptanceCheck {
    pub id: String,
    pub description: String,
    pub checker: Checker,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContractSpec {
    pub objective: String,
    #[serde(default)]
    pub goal_id: Option<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub expected_outputs: Vec<String>,
    pub acceptance_checks: Vec<AcceptanceCheck>,
    /// Describes the execution environment; host sets it, not a model claim.
    #[serde(default)]
    pub environment_fingerprint: String,
    #[serde(default)]
    pub validation_subject: Option<ValidationSubject>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValidationSubject {
    pub kind: String,
    pub identity: String,
    pub expected_outcome: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceResult {
    pub check_id: String,
    pub checker_version: String,
    pub input_identity: String,
    pub status: AcceptanceStatus,
    pub evidence_refs: Vec<String>,
    pub coverage: String,
    pub failure_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceRefresh {
    pub completed_revision: u64,
    pub completed_at: u64,
    pub refreshed_at: u64,
    pub input_identity: String,
    pub results: Vec<AcceptanceResult>,
}

fn initial_requirements_revision() -> u64 {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskRevisionOrigin {
    pub task_id: String,
    pub revision: u64,
}

/// Host-captured goal requirements, separate from model-supplied contract fields.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalBinding {
    pub goal_id: String,
    pub objective_revision: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RevisionMode {
    InPlace,
    Successor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevisionError {
    InvalidContract(String),
    Conflict,
    IdempotencyConflict,
    Busy,
    EnvironmentChanged,
    SuccessorRequired,
    InvalidMode,
    ActiveContract,
    GoalRequirementsChanged,
    GoalDetachRequiresSuccessor,
    Storage(String),
}
impl RevisionError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidContract(_) => "TASK_INVALID_CONTRACT",
            Self::Conflict => "TASK_REVISION_CONFLICT",
            Self::IdempotencyConflict => "TASK_IDEMPOTENCY_CONFLICT",
            Self::Busy => "TASK_BUSY",
            Self::EnvironmentChanged => "TASK_ENVIRONMENT_CHANGED",
            Self::SuccessorRequired => "TASK_SUCCESSOR_REQUIRED",
            Self::InvalidMode => "TASK_INVALID_REVISION_MODE",
            Self::ActiveContract => "TASK_ACTIVE_CONTRACT",
            Self::GoalRequirementsChanged => "TASK_GOAL_REQUIREMENTS_CHANGED",
            Self::GoalDetachRequiresSuccessor => "TASK_GOAL_DETACH_REQUIRES_SUCCESSOR",
            Self::Storage(_) => "TASK_EXECUTION_FAILED",
        }
    }
}
impl From<String> for RevisionError {
    fn from(value: String) -> Self {
        Self::Storage(value)
    }
}
impl std::fmt::Display for RevisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidContract(message) | Self::Storage(message) => message,
            Self::Conflict => "Task revision conflict; reload before changing it",
            Self::IdempotencyConflict => "Idempotency key was reused with different input",
            Self::Busy => "Stop in-flight executions and validation before editing requirements",
            Self::EnvironmentChanged => {
                "Task environment changed; migrate it explicitly before editing requirements"
            }
            Self::SuccessorRequired => {
                "Completed history is immutable; explicitly create a successor task"
            }
            Self::InvalidMode => "A successor requires a completed or cancelled source task",
            Self::ActiveContract => {
                "Finish or cancel the current contract before creating a successor"
            }
            Self::GoalRequirementsChanged => {
                "Review the current goal requirements and explicitly bind this contract before reusing acceptance"
            }
            Self::GoalDetachRequiresSuccessor => {
                "Keep the original goal association in history; cancel the task if needed and explicitly create an independent successor"
            }
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessIdentity {
    pub pid: u32,
    pub created_identity: String,
    pub host_id: String,
    pub owner: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub id: String,
    pub execution_id: String,
    pub idempotency_key: String,
    pub input_identity: String,
    pub tool: String,
    pub effect: EffectKind,
    pub state: StepState,
    pub updated_at: u64,
    pub process: Option<ProcessIdentity>,
    pub result_identity: Option<String>,
    /// Bounded canonical result, never raw stdout logs.
    pub result: Option<Value>,
    pub evidence_refs: Vec<String>,
    pub failure_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskContract {
    pub version: u32,
    pub task_id: String,
    pub owner: String,
    pub revision: u64,
    #[serde(default = "initial_requirements_revision")]
    pub requirements_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub based_on: Option<TaskRevisionOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_binding: Option<GoalBinding>,
    pub spec: ContractSpec,
    pub state: TaskState,
    pub steps: Vec<Step>,
    pub acceptance_results: Vec<AcceptanceResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_refresh: Option<AcceptanceRefresh>,
    pub validation_identity: Option<String>,
    pub output_identities: BTreeMap<String, String>,
    #[serde(default)]
    pub validation_subject_evidence: Option<SubjectEvidence>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SubjectEvidence {
    pub identity: String,
    pub execution_id: String,
    pub loaded_at_revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryItem {
    pub step_id: String,
    pub execution_id: String,
    pub idempotency_key: String,
    pub action: String,
    pub reason: String,
}

impl TaskContract {
    pub fn goal_binding_blockers(&self, current: Option<&GoalBinding>) -> Vec<String> {
        let Some(goal_id) = &self.spec.goal_id else {
            return Vec::new();
        };
        if self
            .goal_binding
            .as_ref()
            .is_none_or(|binding| &binding.goal_id != goal_id || binding.objective_revision == 0)
        {
            return vec!["Linked task has no trusted goal requirements binding; explicitly revise or create a successor before reuse".into()];
        }
        if self.goal_binding.as_ref() != current {
            return vec!["Goal requirements changed or the linked goal is no longer current; old acceptance does not satisfy the current goal".into()];
        }
        Vec::new()
    }
    pub fn current_acceptance_results(&self) -> &[AcceptanceResult] {
        self.acceptance_refresh
            .as_ref()
            .map_or(self.acceptance_results.as_slice(), |refresh| {
                refresh.results.as_slice()
            })
    }
    pub fn recovery(&self) -> Vec<RecoveryItem> {
        self.steps.iter().filter(|step| matches!(step.state, StepState::Prepared|StepState::Unknown|StepState::Dispatched|StepState::Running)).map(|step| {
            let (action, reason) = if step.state==StepState::Prepared {
                ("revalidate_before_dispatch","The intent was persisted but never dispatched; revalidate inputs and permissions before a new execution.")
            } else {match step.effect {
                EffectKind::ReadOnly => ("recheck_then_retry", "Verify current environment, permissions and input identity before retrying this read-only operation."),
                EffectKind::Idempotent => ("query_then_retry_same_key", "Query execution identity first; retry only after adapter confirms idempotency with the original key."),
                _ => ("inspect_effects", "The operation may already have written or submitted data. Inspect effects; automatic replay is forbidden."),
            }};
            RecoveryItem{step_id:step.id.clone(),execution_id:step.execution_id.clone(),idempotency_key:step.idempotency_key.clone(),action:action.into(),reason:reason.into()}
        }).collect()
    }
    pub fn can_signal_process(&self, step_id: &str, observed: &ProcessIdentity) -> bool {
        self.steps
            .iter()
            .find(|s| s.id == step_id)
            .is_some_and(|step| {
                observed.owner == self.owner
                    && !observed.created_identity.is_empty()
                    && step.process.as_ref() == Some(observed)
            })
    }
    pub fn completion_blockers(&self) -> Vec<String> {
        let mut failures = Vec::new();
        if self.spec.goal_id.is_some() && self.goal_binding.is_none() {
            failures.push("Linked task has no trusted goal requirements binding".into());
        }
        if self.state == TaskState::Cancelled {
            failures.push(
                "Task was cancelled; only an explicit user continuation may resume it.".into(),
            );
        }
        for step in &self.steps {
            let safely_superseded = (step.state == StepState::Prepared
                || step.effect == EffectKind::ReadOnly
                    && matches!(step.state, StepState::Failed | StepState::Unknown))
                && self.steps.iter().any(|later| {
                    later.id != step.id
                        && later.tool == step.tool
                        && later.input_identity == step.input_identity
                        && matches!(later.state, StepState::Verified | StepState::Committed)
                });
            let recovered_process_failure = step.state == StepState::Failed
                && step.known_process_exit()
                && step
                    .result
                    .as_ref()
                    .and_then(|r| r.get("retryContext"))
                    .is_some_and(|identity| {
                        self.steps
                            .iter()
                            .skip_while(|s| s.id != step.id)
                            .skip(1)
                            .any(|later| {
                                later.tool == step.tool
                                    && matches!(
                                        later.state,
                                        StepState::Verified | StepState::Committed
                                    )
                                    && later.known_process_exit()
                                    && later.result.as_ref().and_then(|r| r.get("retryContext"))
                                        == Some(identity)
                            })
                    });
            let expected_readonly_failure = step.state == StepState::Failed
                && step.effect == EffectKind::ReadOnly
                && self.spec.acceptance_checks.iter().any(|check| {
                    let Checker::ToolResult { step_id, .. } = &check.checker else {
                        return false;
                    };
                    let selected = self.steps.iter().rev().find(|candidate| {
                        candidate.id == *step_id
                            || step_id
                                .strip_prefix("tool:")
                                .is_some_and(|name| candidate.tool == name)
                    });
                    selected.is_some_and(|selected| selected.id == step.id)
                        && self.current_acceptance_results().iter().any(|result| {
                            result.check_id == check.id
                                && result.status == AcceptanceStatus::Passed
                                && check.checker.evidence_current(result)
                        })
                });
            if !matches!(
                step.state,
                StepState::Verified | StepState::Committed | StepState::NotDispatched
            ) && !expected_readonly_failure
                && !(step.effect == EffectKind::ReadOnly && step.state == StepState::Failed)
                && !safely_superseded
                && !recovered_process_failure
            {
                failures.push(format!("Step {} is {:?}", step.id, step.state));
            }
        }
        for check in &self.spec.acceptance_checks {
            if !self.current_acceptance_results().iter().any(|r| {
                r.check_id == check.id
                    && r.status == AcceptanceStatus::Passed
                    && check.checker.evidence_current(r)
            }) {
                if self.current_acceptance_results().iter().any(|r| {
                    r.check_id == check.id
                        && r.status == AcceptanceStatus::Passed
                        && !check.checker.evidence_current(r)
                }) {
                    failures.push(format!("Acceptance {} uses an obsolete checker; revalidate with {} before reusing this evidence", check.id, check.checker.version()));
                } else {
                    failures.push(format!("Acceptance {} has not passed", check.id));
                }
            }
        }
        if self.validation_identity.is_none() {
            failures.push("Current inputs have not been validated".into());
        }
        if let Some(refresh) = &self.acceptance_refresh {
            let identity = digest(
                &serde_json::to_vec(&self.output_identities).expect("input identities JSON"),
            );
            if refresh.input_identity != identity {
                failures.push(
                    "Refreshed evidence does not match the completed input identities".into(),
                );
            }
        }
        if let Some(subject) = &self.spec.validation_subject
            && subject.kind == "skill"
            && !self
                .validation_subject_evidence
                .as_ref()
                .is_some_and(|evidence| evidence.identity == subject.identity)
        {
            failures.push(
                "The tested skill revision was not loaded by a recorded skill_candidate read"
                    .into(),
            );
        }
        for path in &self.spec.expected_outputs {
            if !self.output_identities.contains_key(path) {
                failures.push(format!("Output {path} has no verified identity"));
            }
        }
        failures
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Step {
    pub(crate) fn known_process_exit(&self) -> bool {
        self.result.as_ref().is_some_and(|r| {
            r["kind"] == "foreground"
                && r["processState"] == "exited"
                && r["commandStarted"] == true
                && r["exitCode"]
                    .as_i64()
                    .is_some_and(|code| code != 124 && code != 125)
                && r["signal"].is_null()
                && matches!(r["completion"].as_str(), Some("failed" | "succeeded"))
        })
    }
}
