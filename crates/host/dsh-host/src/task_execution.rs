//! Host adapters for immutable acceptance requirements and durable effect recovery.
use cordis::{Context, downcast_arc};
use dsh_fs::{FileSystem, ResolveOptions};
use dsh_task_runtime::*;
use dsh_tools::{
    ToolBodyError, ToolDefinition, ToolExecution, ToolExecutionResult, ToolOutputDefinition,
    ToolRuntime,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

pub(crate) struct TaskExecution {
    pub runtime: Arc<TaskRuntime>,
    context: Context,
    fs: Arc<dyn FileSystem>,
    resources: Option<Arc<crate::workspace_resources::Resources>>,
    validation_work: validation_work::Work,
}
const MAX_INPUT_BYTES: u64 = 32 * 1024 * 1024;

#[path = "task_goals.rs"]
mod goals;
#[path = "task_input_snapshot.rs"]
mod input_snapshot;
#[path = "task_requirements.rs"]
mod requirements;
#[path = "task_validation_work.rs"]
mod validation_work;

#[derive(Debug)]
pub(crate) enum TaskActionError {
    Cancelled,
    Revision(RevisionError),
    Failed(String),
}
impl From<RevisionError> for TaskActionError {
    fn from(value: RevisionError) -> Self {
        Self::Revision(value)
    }
}
impl From<String> for TaskActionError {
    fn from(value: String) -> Self {
        Self::Failed(value)
    }
}
impl From<&str> for TaskActionError {
    fn from(value: &str) -> Self {
        Self::Failed(value.into())
    }
}
impl std::fmt::Display for TaskActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Task validation cancelled"),
            Self::Revision(error) => std::fmt::Display::fmt(error, f),
            Self::Failed(message) => f.write_str(message),
        }
    }
}
impl TaskActionError {
    fn tool_error(self) -> ToolBodyError {
        match self {
            Self::Cancelled => {
                ToolBodyError::coded("Task validation cancelled", "TaskCancelled", "CANCELLED")
            }
            Self::Failed(message) => ToolBodyError::plain(message),
            Self::Revision(error) => {
                ToolBodyError::coded(error.to_string(), "TaskRevisionError", error.code())
            }
        }
    }
    fn code(&self) -> &'static str {
        match self {
            Self::Cancelled => "CANCELLED",
            Self::Revision(error) => error.code(),
            Self::Failed(_) => "TASK_EXECUTION_FAILED",
        }
    }
}

fn validation_action(action: &str, user_control: bool) -> bool {
    matches!(action, "validate" | "complete")
        || matches!(action, "reconcile" | "refresh_evidence") && user_control
}

// Journal revisions change after ordinary successful tools. Only facts requiring
// model action belong in the durable prompt, not storage bookkeeping.
fn prompt_task_state(task: &TaskContract) -> Value {
    json!({"taskId":task.task_id,"state":task.state,"requirementsRevision":task.requirements_revision,
        "blockers":task.completion_blockers().into_iter().take(12).collect::<Vec<_>>(),
        "recovery":task.recovery().into_iter().take(8).collect::<Vec<_>>()})
}

fn capabilities() -> Value {
    json!({"revise":true,"requirementsHistory":true,"requirementsSnapshot":true})
}
fn task_response(task: TaskContract) -> Value {
    json!({"recovery":task.recovery(),"blockers":task.completion_blockers(),"task":task,"capabilities":capabilities()})
}

struct UserRevision {
    expected: u64,
    key: String,
    mode: RevisionMode,
    spec: ContractSpec,
    expected_goal_binding: Option<GoalBinding>,
}
impl UserRevision {
    fn parse(args: &Value) -> std::result::Result<Self, RevisionError> {
        let invalid = |message: &str| RevisionError::InvalidContract(message.into());
        let expected = args["expectedRevision"]
            .as_u64()
            .filter(|value| *value > 0)
            .ok_or_else(|| invalid("Missing positive expectedRevision"))?;
        let key = text(args, "idempotencyKey")
            .map_err(RevisionError::InvalidContract)?
            .into();
        let mode =
            serde_json::from_value(args["mode"].clone()).map_err(|_| RevisionError::InvalidMode)?;
        let spec = serde_json::from_value(args["contract"].clone())
            .map_err(|error| invalid(&format!("Invalid contract: {error}")))?;
        let expected_goal_binding = args
            .get("expectedGoalBinding")
            .filter(|value| !value.is_null())
            .map(|value| {
                serde_json::from_value(value.clone())
                    .map_err(|error| invalid(&format!("Invalid expectedGoalBinding: {error}")))
            })
            .transpose()?;
        Ok(Self {
            expected,
            key,
            mode,
            spec,
            expected_goal_binding,
        })
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("Missing {key}"))
}
fn exempt(name: &str) -> bool {
    matches!(
        name,
        "task_execution"
            | "tool_search"
            | "tool_describe"
            | "get_goal"
            | "create_goal"
            | "update_goal"
            | "present"
    )
}
fn effect(name: &str, arguments: &Value) -> EffectKind {
    if matches!(
        name,
        "read"
            | "read_file"
            | "read_image"
            | "read_video"
            | "office_render"
            | "list_directory"
            | "glob"
            | "grep"
            | "environment_probe"
            | "environment_validate"
            | "consult_model"
            | "web_search"
            | "web_fetch"
            | "job_output"
            | "job_list"
    ) || name == "agent_team" && matches!(arguments["action"].as_str(), Some("status" | "wait"))
        || name == "workspace_scratch" && matches!(arguments["action"].as_str(), Some("read" | "list" | "inspect"))
    {
        EffectKind::ReadOnly
    } else {
        EffectKind::Write
    }
}
fn execution_id(execution: &ToolExecution) -> String {
    format!("call-{}", digest(execution.call_id.as_str().as_bytes()))
}
fn outcome_flags(name: &str, value: Option<&Value>, is_error: bool) -> (bool, bool) {
    let Some(value) = value else {
        return (!is_error, false);
    };
    let failed = is_error
        || value["exitCode"].as_i64().is_some_and(|code| code != 0)
        || value["timedOut"] == true
        || value["aborted"] == true
        || matches!(
            value["status"].as_str(),
            Some("failed" | "cancelled" | "unknown")
        )
        || name == "terminal_send" && value["completion"] != "completed";
    let running = value["jobId"].is_string()
        || matches!(
            value["status"].as_str(),
            Some("running" | "pending" | "queued")
        )
        || value["completed"] == false;
    (!failed, running)
}

fn model_parameters() -> Value {
    let mut schema = json!({"type":"object","properties":{"action":{"type":"string","enum":["create","list","get","validate","complete","recover"]},"taskId":{"type":"string","minLength":1,"description":"Required for get, validate, complete and recover. Optional for create: omitted IDs are generated deterministically and returned; reuse the returned taskId."},"idempotencyKey":{"type":"string","minLength":1,"description":"Optional operation key; the runtime supplies one when omitted. Reuse an explicit key only for an identical retry."},"contract":{"type":"object","properties":{"objective":{"type":"string"},"goalId":{"type":"string"},"constraints":{"type":"array","items":{"type":"string"}},"expectedOutputs":{"type":"array","items":{"type":"string"}},"acceptanceChecks":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"description":{"type":"string"},"checker":{"type":"object","description":"kind=text(path,required,forbidden), json(path,assertions keyed by JSON pointer), image(path,min_width,min_height,channels), office_package(path,format docx/xlsx/pptx), tool_result(step_id exact ID or tool:NAME,assertions), manual(reason)"}},"required":["id","description","checker"]}},"validationSubject":{"type":"object","properties":{"kind":{"type":"string"},"identity":{"type":"string"},"expectedOutcome":{"type":"string"}},"required":["kind","identity","expectedOutcome"]}},"required":["objective","acceptanceChecks"]}},"required":["action"],"additionalProperties":false});
    schema["properties"]["contract"]["description"] = json!("Pass a JSON object with objective and acceptanceChecks; do not quote or stringify the contract object.");
    schema["properties"]["contract"]["properties"]["acceptanceChecks"]["items"]["properties"]["checker"] = json!({"oneOf":[
        {"type":"object","properties":{"kind":{"type":"string","const":"text"},"path":{"type":"string"},"required":{"type":"array","items":{"type":"string"}},"forbidden":{"type":"array","items":{"type":"string"}}},"required":["kind","path","required"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"type":"string","const":"json"},"path":{"type":"string"},"assertions":{"type":"object","description":"JSON Pointer keys, for example /scripts/test. Keys must start with / (or be empty to match the whole result)."}},"required":["kind","path","assertions"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"type":"string","const":"tool_result"},"step_id":{"type":"string","description":"When creating a contract use tool: followed by the tool name, e.g. tool:execute_native or tool:pwsh. Do not invent future step labels."},"assertions":{"type":"object","description":"JSON Pointer keys into the recorded result, e.g. {\"/exitCode\":0}. Plain exitCode is not a JSON Pointer."}},"required":["kind","step_id","assertions"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"type":"string","const":"image"},"path":{"type":"string"},"min_width":{"type":"integer"},"min_height":{"type":"integer"},"channels":{"type":"integer"}},"required":["kind","path","min_width","min_height"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"type":"string","const":"office_package"},"path":{"type":"string"},"format":{"type":"string","enum":["docx","xlsx","pptx"]}},"required":["kind","path","format"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"type":"string","const":"manual"},"reason":{"type":"string"}},"required":["kind","reason"],"additionalProperties":false}
    ]});
    schema
}

fn validate_model_contract(spec: &ContractSpec) -> Result<()> {
    let mut errors = Vec::new();
    for check in &spec.acceptance_checks {
        if let Checker::ToolResult { step_id, .. } = &check.checker {
            if !step_id
                .strip_prefix("tool:")
                .is_some_and(|name| !name.is_empty() && !name.chars().any(char::is_whitespace))
            {
                errors.push(format!("{}: use a tool selector such as tool:execute_native; future step labels cannot be referenced",check.id));
            }
        }
        if let Checker::ToolResult { assertions, .. } | Checker::Json { assertions, .. } =
            &check.checker
        {
            for pointer in assertions.keys() {
                let mut chars = pointer.chars();
                let mut valid = pointer.is_empty() || pointer.starts_with('/');
                while let Some(ch) = chars.next() {
                    if ch == '~' && !matches!(chars.next(), Some('0' | '1')) {
                        valid = false;
                    }
                }
                if !valid {
                    errors.push(format!("{}: assertion key {pointer:?} must be a valid JSON Pointer, e.g. /exitCode",check.id));
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("Contract was not created: {}", errors.join("; ")))
    }
}

impl TaskExecution {
    fn environment(&self, owner: &str, cwd: &str) -> Result<String> {
        crate::skill_validation::environment_fingerprint(&self.context, Some(owner), cwd)
    }
    async fn preflight(&self, execution: &ToolExecution) -> Result<()> {
        let Some(agent) = &execution.agent else {
            return Ok(());
        };
        let owner = agent.id().as_str();
        let cwd = agent
            .session()
            .header()
            .cwd
            .as_deref()
            .ok_or("Missing workspace")?;
        if execution.name == "present" {
            if let Some(task) = self.runtime.latest(owner)? {
                let signal = execution.signal.lock().clone();
                self.verified_evidence(owner, &task.task_id, task.revision, cwd, signal)
                    .await?;
            }
            return Ok(());
        }
        let Some(task) = self.runtime.active(owner)? else {
            return Ok(());
        };
        if !exempt(&execution.name)
            && effect(&execution.name, &execution.arguments) != EffectKind::ReadOnly
        {
            self.assert_goal_applicable(&task).await?;
        }
        if !exempt(&execution.name)
            && effect(&execution.name, &execution.arguments) != EffectKind::ReadOnly
            && self.environment(owner, cwd)? != task.spec.environment_fingerprint
        {
            return Err("TASK_ENVIRONMENT_CHANGED: 当前权限或运行环境与任务契约不一致。请在本会话的「任务验收」中选择「切换到当前环境」并确认，然后继续；旧验收证据将失效，任务要求和未知效果记录保留。不要重试写入、换工具或重建同一契约；running 状态不表示环境已迁移。".into());
        }
        if execution.name != "workspace_scratch" || execution.arguments["action"] != "promote" {
            return Ok(());
        }
        let resources = self
            .resources
            .as_ref()
            .ok_or("Managed resources unavailable")?;
        let id = text(&execution.arguments, "id")?;
        let store = resources.assert_owner(owner, id)?;
        let candidate = store.path(id, text(&execution.arguments, "path")?)?;
        let validation =
            self.validation_work
                .begin(owner, &task.task_id, execution.signal.lock().clone());
        let signal = validation.signal.clone();
        let input = self
            .input(
                owner,
                cwd,
                &candidate.to_string_lossy(),
                signal.clone(),
                true,
            )
            .await?;
        let options = ResolveOptions {
            cwd: Some(cwd.into()),
            signal: Some(signal),
        };
        let target = self
            .fs
            .resolve(text(&execution.arguments, "target")?, Some(&options))
            .await
            .map_err(|e| e.to_string())?;
        let mut matched = false;
        for check in &task.spec.acceptance_checks {
            let Some(path) = check.checker.path() else {
                continue;
            };
            let expected = self
                .fs
                .resolve(path, Some(&options))
                .await
                .map_err(|e| e.to_string())?;
            if expected.target_key != target.target_key {
                continue;
            }
            matched = true;
            let checking_contract = check.clone();
            let mut file = input.reader()?;
            let checking = options.signal.clone().unwrap_or_else(|| Arc::new(|| false));
            let result = tokio::task::spawn_blocking(move || {
                check_file(&checking_contract, &mut file, Some(checking.as_ref()))
            })
            .await
            .map_err(|e| e.to_string())?;
            if result.status != AcceptanceStatus::Passed {
                return Err(format!(
                    "Candidate failed acceptance {}: {}",
                    check.id,
                    result.failure_reason.unwrap_or_default()
                ));
            }
        }
        if !matched {
            return Err("Add the intended target content checks when creating the contract; a candidate cannot be promoted without a matching checker".into());
        }
        if options.signal.as_ref().is_some_and(|signal| signal()) {
            return Err("Task validation cancelled".into());
        }
        resources.seal_promotion(execution.token, owner, &execution.arguments, input.identity);
        Ok(())
    }
    /// Export only a completed, currently matching validation snapshot.
    /// Consumers such as skill promotion must not accept model-provided pass flags.
    pub async fn verified_evidence(
        &self,
        owner: &str,
        task_id: &str,
        revision: u64,
        cwd: &str,
        signal: dsh_tools::AbortPredicate,
    ) -> Result<TaskContract> {
        self.verified_evidence_guarded(owner, task_id, revision, cwd, signal)
            .await
            .map(|(task, _)| task)
            .map_err(|error| error.to_string())
    }
    /// Return the original guard, not a new registration after the final await.
    async fn verified_evidence_guarded(
        &self,
        owner: &str,
        task_id: &str,
        revision: u64,
        cwd: &str,
        signal: dsh_tools::AbortPredicate,
    ) -> std::result::Result<(TaskContract, validation_work::Guard), TaskActionError> {
        let validation = self.validation_work.begin(owner, task_id, signal);
        let outcome: Result<TaskContract> = async {
            if (validation.signal)() {
                return Err("Task validation cancelled".into());
            }
            let task = self.runtime.get(owner, task_id)?;
            if task.revision != revision || task.state != TaskState::Completed {
                return Err(
                    "Acceptance evidence must reference an exact completed revision".into(),
                );
            }
            if !task.completion_blockers().is_empty() {
                return Err("Acceptance evidence is incomplete".into());
            }
            self.assert_goal_applicable(&task).await?;
            if (validation.signal)() {
                return Err("Task validation cancelled".into());
            }
            if self.environment(owner, cwd)? != task.spec.environment_fingerprint {
                return Err(
                    "Acceptance evidence belongs to a different execution environment".into(),
                );
            }
            let (_, current) = self
                .inputs(&task, cwd, validation.signal.clone(), false)
                .await?;
            if (validation.signal)() {
                return Err("Task validation cancelled".into());
            }
            if current != task.output_identities {
                return Err("Acceptance inputs changed since the verified task".into());
            }
            self.assert_goal_applicable(&task).await?;
            Ok(task)
        }
        .await;
        // Inspect the same authoritative signal before Guard::drop can set it.
        if (validation.signal)() {
            return Err(TaskActionError::Cancelled);
        }
        Ok((outcome?, validation))
    }
    async fn input(
        &self,
        owner: &str,
        cwd: &str,
        path: &str,
        signal: dsh_tools::AbortPredicate,
        retain: bool,
    ) -> Result<input_snapshot::Snapshot> {
        validation_work::until_cancelled(
            self.input_inner(owner, cwd, path, signal.clone(), retain),
            signal,
        )
        .await
    }
    async fn input_inner(
        &self,
        owner: &str,
        cwd: &str,
        path: &str,
        signal: dsh_tools::AbortPredicate,
        retain: bool,
    ) -> Result<input_snapshot::Snapshot> {
        if dsh_tools::path_is_sensitive(Path::new(path)) {
            return Err("Sensitive acceptance inputs require the approved read tool; use its recorded result checker instead".into());
        }
        let options = ResolveOptions {
            cwd: Some(cwd.into()),
            signal: Some(signal.clone()),
        };
        let target = self
            .fs
            .resolve(path, Some(&options))
            .await
            .map_err(|e| e.to_string())?;
        if dsh_tools::path_is_sensitive(Path::new(&target.display_path)) {
            return Err(
                "Resolved acceptance input is sensitive; the ordinary read approval policy applies"
                    .into(),
            );
        }
        let root = self
            .fs
            .resolve(cwd, Some(&options))
            .await
            .map_err(|e| e.to_string())?;
        let mut allowed = self.fs.contains(&root, &target);
        if !allowed && let Some(resources) = &self.resources {
            // Reuse resource ownership instead of treating all scratch directories as readable.
            for resource in resources
                .list(Some(owner))?
                .iter()
                .filter(|resource| resource.owner == owner)
            {
                if resource.path.is_empty() {
                    continue;
                }
                let root = self
                    .fs
                    .resolve(&resource.path, Some(&options))
                    .await
                    .map_err(|e| e.to_string())?;
                if self.fs.contains(&root, &target) {
                    allowed = true;
                    break;
                }
            }
        }
        if !allowed {
            return Err(
                "Acceptance input is outside the session workspace and owned scratch resources"
                    .into(),
            );
        }
        let stream = self
            .fs
            .stream_bytes(&target, Some(signal.clone()), MAX_INPUT_BYTES)
            .await
            .map_err(|e| e.to_string())?;
        input_snapshot::capture(stream, signal, retain, MAX_INPUT_BYTES).await
    }
    async fn inputs(
        &self,
        task: &TaskContract,
        cwd: &str,
        signal: dsh_tools::AbortPredicate,
        retain: bool,
    ) -> Result<(
        BTreeMap<String, input_snapshot::Snapshot>,
        BTreeMap<String, String>,
    )> {
        let mut paths = task
            .spec
            .expected_outputs
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        paths.extend(
            task.spec
                .acceptance_checks
                .iter()
                .filter_map(|check| check.checker.path().map(str::to_owned)),
        );
        let mut bytes = BTreeMap::new();
        let mut identities = BTreeMap::new();
        let mut total = 0u64;
        for path in paths {
            if signal() {
                return Err("Task validation cancelled".into());
            }
            let data = self
                .input(&task.owner, cwd, &path, signal.clone(), retain)
                .await?;
            total = total.saturating_add(data.bytes);
            if total > 128 * 1024 * 1024 {
                return Err("Task validation exceeds the total input budget".into());
            }
            identities.insert(path.clone(), data.identity.clone());
            if retain {
                bytes.insert(path, data);
            }
        }
        Ok((bytes, identities))
    }
    async fn validate(
        &self,
        owner: &str,
        id: &str,
        key: &str,
        cwd: &str,
        signal: dsh_tools::AbortPredicate,
        refresh_revision: Option<u64>,
    ) -> Result<TaskContract> {
        let task = self.runtime.get(owner, id)?;
        if let Some(revision) = refresh_revision {
            if task.state != TaskState::Completed || revision != task.revision {
                return Err("Evidence refresh needs the current completed task revision".into());
            }
        } else if matches!(task.state, TaskState::Completed | TaskState::Cancelled) {
            return Err("Terminal task cannot change validation".into());
        }
        if self.environment(owner, cwd)? != task.spec.environment_fingerprint {
            return Err("Task environment changed; previous evidence is invalid".into());
        }
        self.assert_goal_applicable(&task).await?;
        let (inputs, identities) = self.inputs(&task, cwd, signal.clone(), true).await?;
        if refresh_revision.is_some() && task.output_identities != identities {
            return Err(
                "Completed inputs changed; prior manual confirmation cannot be reused".into(),
            );
        }
        let identity = digest(&serde_json::to_vec(&identities).map_err(|e| e.to_string())?);
        let mut results = Vec::new();
        for check in &task.spec.acceptance_checks {
            if signal() {
                return Err("Task validation cancelled".into());
            }
            let result = match &check.checker {
                Checker::Manual { reason } => {
                    if let Some(previous) = task.acceptance_results.iter().find(|r| {
                        r.check_id == check.id
                            && r.input_identity == identity
                            && r.status == AcceptanceStatus::Passed
                    }) {
                        previous.clone()
                    } else {
                        if refresh_revision.is_some() {
                            return Err(
                                "Evidence refresh requires unchanged existing human confirmation"
                                    .into(),
                            );
                        }
                        AcceptanceResult {
                            check_id: check.id.clone(),
                            checker_version: CHECKER_VERSION.into(),
                            input_identity: identity.clone(),
                            status: AcceptanceStatus::AwaitingUser,
                            evidence_refs: vec![],
                            coverage: reason.clone(),
                            failure_reason: None,
                        }
                    }
                }
                Checker::ToolResult { step_id, .. } => check_tool_result(
                    check,
                    task.steps.iter().rev().find(|s| {
                        s.id == *step_id
                            || step_id
                                .strip_prefix("tool:")
                                .is_some_and(|name| s.tool == name)
                    }),
                ),
                _ => {
                    let check = check.clone();
                    let mut file = inputs[check.checker.path().unwrap()].reader()?;
                    let signal = signal.clone();
                    tokio::task::spawn_blocking(move || {
                        check_file(&check, &mut file, Some(signal.as_ref()))
                    })
                    .await
                    .map_err(|e| e.to_string())?
                }
            };
            results.push(result);
        }
        if signal() {
            return Err("Task validation cancelled".into());
        }
        let _goal_claim = self.claim_task_goal(&task)?;
        if refresh_revision.is_some() {
            self.runtime.refresh_evidence_by_user(
                owner,
                id,
                key,
                task.revision,
                results,
                identities,
            )
        } else {
            self.runtime
                .record_validation(owner, id, key, task.revision, results, identities)
        }
    }
    pub async fn action(
        &self,
        owner: &str,
        cwd: &str,
        args: &Value,
        signal: dsh_tools::AbortPredicate,
        user_control: bool,
    ) -> std::result::Result<Value, TaskActionError> {
        self.action_with_work(owner, cwd, args, signal, user_control, None)
            .await
    }
    async fn action_with_work(
        &self,
        owner: &str,
        cwd: &str,
        args: &Value,
        signal: dsh_tools::AbortPredicate,
        user_control: bool,
        prepared_work: Option<validation_work::Guard>,
    ) -> std::result::Result<Value, TaskActionError> {
        let signal = prepared_work
            .as_ref()
            .map_or(signal, |guard| guard.signal.clone());
        if signal() {
            return Err(TaskActionError::Cancelled);
        }
        let action = text(args, "action")?;
        if action == "list" {
            return Ok(self
                .decorate_response(
                    json!({"tasks":self.runtime.list(owner)?,"capabilities":capabilities()}),
                    owner,
                )
                .await);
        }
        let generated_id = format!(
            "task-{}",
            &digest(
                &serde_json::to_vec(&json!({"owner":owner,"cwd":cwd,"contract":args["contract"]}))
                    .map_err(|e| e.to_string())?
            )[..24]
        );
        let id = if action == "create" && args.get("taskId").is_none() {
            generated_id.as_str()
        } else {
            text(args, "taskId")?
        };
        if action == "get" || action == "recover" {
            let task = self.runtime.get(owner, id)?;
            let environment_changed =
                self.environment(owner, cwd)? != task.spec.environment_fingerprint;
            return Ok(
                self.decorate_response(json!({"environmentChanged":environment_changed,"requiredUserAction":if environment_changed {Some("任务验收 → 切换到当前环境 → 确认；停止重复尝试其他写入工具")} else {None},"recovery":task.recovery(),"blockers":task.completion_blockers(),"task":task,"capabilities":capabilities()}),owner).await,
            );
        }
        if action == "refresh_history" && user_control {
            return Ok(json!({"history":self.runtime.evidence_refresh_history(owner,id)?}));
        }
        let generated_key = format!("operation-{}", uuid::Uuid::new_v4());
        let key = if !user_control && args.get("idempotencyKey").is_none() {
            generated_key.as_str()
        } else {
            text(args, "idempotencyKey")?
        };
        let _validation = prepared_work.or_else(|| {
            validation_action(action, user_control)
                .then(|| self.validation_work.begin(owner, id, signal.clone()))
        });
        let signal = _validation
            .as_ref()
            .map_or(signal, |guard| guard.signal.clone());
        if matches!(
            action,
            "validate" | "refresh_evidence" | "complete" | "confirm"
        ) {
            let current = self.runtime.get(owner, id)?;
            if self.assert_goal_applicable(&current).await.is_err() {
                if signal() {
                    return Err(TaskActionError::Cancelled);
                }
                return Err(RevisionError::GoalRequirementsChanged.into());
            }
        }
        let outcome: std::result::Result<TaskContract, TaskActionError> = async {
            Ok(match action {
                "create" => {
                    let raw_contract = args["contract"].clone();
                    if !raw_contract.is_object() {
                        return Err("contract must be an object containing objective and acceptanceChecks; do not stringify the object".into());
                    }
                    let mut spec: ContractSpec = serde_json::from_value(raw_contract)
                        .map_err(|e| e.to_string())?;
                    if !user_control {
                        validate_model_contract(&spec)?;
                    }
                    spec.environment_fingerprint = self.environment(owner, cwd)?;
                    let (binding, _claim) =
                        self.capture_created_binding(owner, &mut spec, user_control)?;
                    // A model may retry a create after losing the response and
                    // omit the generated task id. Reuse the live contract when
                    // its immutable requirements and Goal binding are identical;
                    // a genuinely different contract still requires an explicit
                    // finish/cancel or successor transition.
                    if let Some(active) = self.runtime.active(owner)? {
                        if active.spec == spec && active.goal_binding == binding {
                            return Ok(active);
                        }
                    }
                    self.runtime.create_bound(owner, id, spec, binding)?
                }
                "validate" => {
                    self.validate(owner, id, key, cwd, signal.clone(), None)
                        .await?
                }
                "refresh_evidence" if user_control => {
                    self.validate(
                        owner,
                        id,
                        key,
                        cwd,
                        signal.clone(),
                        Some(args["revision"].as_u64().ok_or("Missing revision")?),
                    )
                    .await?
                }
                "stop_validation" if user_control => {
                    let current = self.runtime.get(owner, id)?;
                    self.validation_work.cancel(owner, id);
                    current
                }
                "complete" => {
                    let current = self.runtime.get(owner, id)?;
                    if self.environment(owner, cwd)? != current.spec.environment_fingerprint {
                        return Err(
                            "Task environment changed; migrate and revalidate before completion"
                                .into(),
                        );
                    }
                    let (_, identities) = self.inputs(&current, cwd, signal.clone(), false).await?;
                    if signal() {
                        return Err("Task validation cancelled".into());
                    }
                    let _goal_claim = self.claim_task_goal(&current)?;
                    self.runtime
                        .complete(owner, id, key, current.revision, &identities)?
                }
                "cancel" if user_control => {
                    self.validation_work.cancel(owner, id);
                    self.runtime.cancel(owner, id, key)?
                }
                "migrate_environment" if user_control => {
                    self.validation_work.cancel(owner, id);
                    self.runtime.migrate_environment_by_user(
                        owner,
                        id,
                        key,
                        args["revision"].as_u64().ok_or("Missing revision")?,
                        &self.environment(owner, cwd)?,
                    )?
                }
                "confirm" if user_control => {
                    let task = self.runtime.get(owner, id)?;
                    let _goal_claim = self.claim_task_goal(&task)?;
                    self.runtime.confirm_manual_by_user(
                        owner,
                        id,
                        key,
                        args["revision"].as_u64().ok_or("Missing revision")?,
                        text(args, "checkId")?,
                        text(args, "inputIdentity")?,
                    )?
                }
                "resume" if user_control => self.runtime.resume_by_user(
                    owner,
                    id,
                    key,
                    args["revision"].as_u64().ok_or("Missing revision")?,
                )?,
                "reconcile" if user_control => {
                    let current = self.runtime.get(owner, id)?;
                    let check = current
                        .spec
                        .acceptance_checks
                        .iter()
                        .find(|check| check.id == args["checkId"].as_str().unwrap_or_default())
                        .ok_or("Unknown acceptance check")?;
                    let path = check
                        .checker
                        .path()
                        .ok_or("Effect reconciliation requires a file-content checker")?;
                    let input = self.input(owner, cwd, path, signal.clone(), true).await?;
                    let mut file = input.reader()?;
                    let check = check.clone();
                    let check_id = check.id.clone();
                    let checking = signal.clone();
                    let observed = tokio::task::spawn_blocking(move || {
                        check_file(&check, &mut file, Some(checking.as_ref()))
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                    if signal() {
                        return Err("Task validation cancelled".into());
                    }
                    if observed.status != AcceptanceStatus::Passed {
                        return Err(observed
                            .failure_reason
                            .unwrap_or("Effect not verified".into())
                            .into());
                    }
                    self.runtime.reconcile(
                        owner,
                        id,
                        text(args, "stepId")?,
                        key,
                        &format!("acceptance:{}:{}", check_id, observed.input_identity),
                        &observed.input_identity,
                    )?
                }
                _ => return Err("Unknown action or user-only operation".into()),
            })
        }
        .await;
        if signal() {
            return Err(TaskActionError::Cancelled);
        }
        let task = outcome?;
        Ok(self.decorate_response(task_response(task), owner).await)
    }
}

pub(crate) async fn install(
    ctx: &Context,
    tools: &Arc<ToolRuntime>,
    prompt: &Arc<dsh_system_prompt::SystemPrompt>,
    fs: Arc<dyn FileSystem>,
    resources: Option<Arc<crate::workspace_resources::Resources>>,
    data_root: &Path,
) -> Result<Arc<TaskExecution>> {
    let service = Arc::new(TaskExecution {
        runtime: Arc::new(TaskRuntime::open(
            &data_root.join("task-execution-v1.sqlite"),
        )?),
        context: ctx.clone(),
        fs,
        resources,
        validation_work: Default::default(),
    });
    goals::install(&service)?;
    let for_preflight = service.clone();
    ctx.on(
        "tools/pre-execute",
        Arc::new(move |_, args| {
            let service = for_preflight.clone();
            let execution = args
                .first()
                .and_then(downcast_arc::<Arc<ToolExecution>>)
                .map(|v| v.as_ref().clone());
            let next = args.last().and_then(downcast_arc::<cordis::NextFn>);
            Box::pin(async move {
                if let Some(execution) = execution
                    && let Err(reason) = service.preflight(&execution).await
                {
                    return Some(cordis::arc(dsh_tools::PreToolDecision::Deny { reason }));
                }
                match next {
                    Some(next) => Some(next.call().await),
                    None => Some(cordis::arc(dsh_tools::PreToolDecision::Allow)),
                }
            })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;
    let runtime = service.runtime.clone();
    tools.guard(ctx,Arc::new(move|execution|{
        let owner=execution.agent.as_ref()?.id().as_str().to_owned();
        let task=match runtime.active(&owner) {Ok(Some(task))=>task,Ok(None)=>return None,Err(error)=>return Some(error)};
        if execution.name=="update_goal"&&execution.arguments["action"]=="complete" || execution.name=="present" {
            if task.state!=TaskState::Completed {return Some("Complete task_execution acceptance before final delivery or marking the goal complete".into());}
        }
        if exempt(&execution.name) {return None;}
        let id=execution_id(execution);
        if task.steps.iter().any(|step|step.execution_id==id) {return Some("Execution identity is already durable; inspect its status rather than replaying it".into());}
        let input_identity=digest(&serde_json::to_vec(&execution.arguments).unwrap_or_default());
        if task.steps.iter().any(|step|step.effect!=EffectKind::ReadOnly && matches!(step.state,StepState::Unknown|StepState::Running|StepState::Dispatched) && step.tool==execution.name && step.input_identity==input_identity) {
            return Some("An identical operation may already have effects or still be running; inspect its execution before retrying".into());
        }
        let step=Step{id:id.clone(),execution_id:id.clone(),idempotency_key:id.clone(),input_identity:digest(&serde_json::to_vec(&execution.arguments).unwrap_or_default()),tool:execution.name.clone(),effect:effect(&execution.name, &execution.arguments),state:StepState::Prepared,updated_at:now(),process:None,result_identity:None,result:None,evidence_refs:vec![],failure_reason:None};
        runtime.prepare(&owner,&task.task_id,step).and_then(|_|runtime.dispatch(&owner,&task.task_id,&id)).err()
    }))?;
    let runtime = service.runtime.clone();
    ctx.on(
        "tools/result",
        Arc::new(move |listener_ctx, args| {
            let ctx = listener_ctx.clone();
            let runtime = runtime.clone();
            let execution = args
                .first()
                .and_then(downcast_arc::<Arc<ToolExecution>>)
                .map(|v| v.as_ref().clone());
            let result = args
                .get(1)
                .and_then(downcast_arc::<Arc<ToolExecutionResult>>)
                .map(|v| v.as_ref().clone());
            let body_invoked = args.get(2).and_then(downcast_arc::<bool>).map(|value| *value);
            let effects_started=args.get(3).and_then(downcast_arc::<Option<bool>>).and_then(|value|*value);
            Box::pin(async move {
                if let (Some(execution), Some(result)) = (execution, result)
                    && !exempt(&execution.name)
                    && let Some(agent) = &execution.agent
                {
                    let owner = agent.id().as_str();
                    let id = execution_id(&execution);
                    // Include cancelled contracts: late notifications must not reactivate them.
                    if let Ok(tasks) = runtime.list(owner)
                        && let Some(task) = tasks
                            .iter()
                            .find(|t| t.steps.iter().any(|s| s.execution_id == id))
                    {
                        let value = result.value.clone().or_else(|| result.meta.as_ref().and_then(|meta|meta.get("executionReceipt")).cloned()).or_else(|| result.error.as_ref().map(|error|json!({"isError":true,"error":{"message":error.message,"code":error.info.as_ref().map(|info|info.code.clone())}})));
                        let (success, running) = outcome_flags(&execution.name, value.as_ref(), result.is_error);
                        let evidence = vec![format!(
                            "session:{owner}:call:{}",
                            execution.call_id.as_str()
                        )];
                        let adapter_not_dispatched = matches!(execution.name.as_str(), "pwsh" | "execute_native" | "execute_steps" | "execute_script")
                            && result.meta.as_ref().and_then(|meta| meta.get("executionReceipt")).is_some_and(|receipt| receipt["commandStarted"] == false && receipt["effects"] == "none" && receipt["processState"] == "not_started");
                        let observed = if result.is_error && (body_invoked == Some(false)||effects_started==Some(false)||adapter_not_dispatched) {
                            runtime.observe_not_dispatched(owner, &task.task_id, &id, &format!("result-{}", digest(id.as_bytes())), evidence)
                        } else { runtime.observe(
                            owner,
                            &task.task_id,
                            &id,
                            &format!("result-{}", digest(id.as_bytes())),
                            success,
                            running,
                            value.clone(),
                            evidence,
                        ) };
                        if let Err(error) = observed {
                            ctx.named_logger(Some("task-execution"))
                                .warn(vec![cordis::arc(format!(
                                    "Durable task result could not be recorded: {error}"
                                ))]);
                        }
                        if execution.name=="skill_candidate" && execution.arguments["action"]=="read" && !result.is_error
                            && let Some(hash)=value.as_ref().and_then(|value|value["contentHash"].as_str())
                            && task.spec.validation_subject.as_ref().is_some_and(|subject|subject.kind=="skill"&&subject.identity==hash) {
                            if let Err(error)=runtime.mark_subject_loaded(owner,&task.task_id,&id,hash) {
                                ctx.named_logger(Some("task-execution")).warn(vec![cordis::arc(format!("Skill subject read not recorded: {error}"))]);
                            }
                        }
                        if execution.name == "job_output" && !result.is_error && let Some(value) = value {
                            let job = &value["job"];
                            // A PTY send job ending proves only the observation ended.
                            if job["kind"] != "pty-send" && matches!(job["status"].as_str(),Some("completed"|"failed"|"killed")) {
                                for prior in &task.steps {
                                    if matches!(prior.state,StepState::Running|StepState::Unknown)
                                        && prior.result.as_ref().is_some_and(|result|result["jobId"]==job["id"] && job["id"].is_string()) {
                                        let key=format!("job-{}",digest(format!("{}:{}",prior.id,job["status"]).as_bytes()));
                                        if let Err(error)=runtime.observe(owner,&task.task_id,&prior.id,&key,job["status"]=="completed",false,Some(value.clone()),vec![format!("job:{}",job["id"].as_str().unwrap_or_default())]) {
                                            ctx.named_logger(Some("task-execution")).warn(vec![cordis::arc(format!("Job effect state not recorded: {error}"))]);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                None
            })
        }),
        cordis::EventOptions::default().global(true),
    )
    .await;
    let action = service.clone();
    tools.register(ctx,ToolDefinition{
        name:"task_execution".into(),
        description:"Create and inspect a durable task acceptance contract for multi-step work. create requires contract; taskId is optional and generated when omitted. get/validate/complete/recover require the returned taskId. idempotencyKey is optional and generated by the runtime when omitted. Requirements cannot be weakened after creation. Other tool executions are journaled automatically. validate runs real content checkers; complete rechecks input identities and blocks unfinished or unknown effects. recover only inspects and never replays. Manual confirmation, effect reconciliation, cancellation and resume require user controls. Declare content checks against final target paths: workspace_scratch promote checks candidate bytes against those requirements before version-checked delivery.".into(),
        parameters:model_parameters(),
        output:ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,value|Ok(vec![dsh_llm::ContentBlock::Text{text:value.to_string()}])),presentation_meta:None},
        timeout_ms:Some(60_000),is_concurrency_safe:None,finalize_content:None,present_call:None,present_result:None,
        execute:Arc::new(move|args,run|{let service=action.clone();let args=args.clone();let agent=run.agent.clone();let signal=run.signal.lock().clone();Box::pin(async move{
            let agent=agent.ok_or_else(||ToolBodyError::plain("Task contracts require an owning session"))?;
            let cwd=agent.session().header().cwd.as_deref().ok_or_else(||ToolBodyError::plain("Task contract requires a workspace"))?;
            service.action(agent.id().as_str(),cwd,&args,signal,false).await.map_err(TaskActionError::tool_error)
        })}),
    })?;
    prompt.section(ctx,dsh_system_prompt::PromptSection{name:"task:acceptance".into(),order:108.0,complete:None,text:dsh_tools::scoped_tool_guidance(ctx,&["task_execution"],"For multi-step implementation or artifact tasks, create task_execution with the user's objective, constraints and explicit content acceptance checks before execution. Do not weaken requirements. Use its durable recovery state after a restart; unknown effects must be inspected before retrying. A successful process alone is not business acceptance. Validate all final inputs and complete the contract before present/update_goal complete. Office package checks only prove structural readability: use office_render to obtain actual WPS PDF page images and inspect all pages when layout is required. A missing Python/ffmpeg/LibreOffice does not block this Windows-native renderer. Never infer missing installation from sandbox denial, or claim visual acceptance from XML, text or package checks. Manual confirmation and resuming cancelled tasks require direct user controls.")});
    let runtime = service.runtime.clone();
    let environment_service = Arc::downgrade(&service);
    prompt.context(ctx,dsh_system_prompt::PromptContext{name:"task:durable-state".into(),order:82.0,text:dsh_system_prompt::PromptText::Provider(Arc::new(move|context|{
        let Some(owner)=context.field_str("sessionId") else {return String::new()};
        match runtime.latest(owner) {
            Ok(Some(task))=>{
                let changed = environment_service.upgrade().and_then(|service| {
                    let store = service.context.get_typed::<Arc<dsh_session::SessionStore>>("sessions",false)?;
                    let session = store.get(&dsh_session::session_id(owner))?;
                    let cwd = session.header().cwd.as_deref()?;
                    service.environment(owner,cwd).ok().map(|current|current!=task.spec.environment_fingerprint)
                });
                let mut summary=prompt_task_state(&task);
                if let Some(service)=environment_service.upgrade() {goals::add_prompt_binding(&mut summary,&task,&service.context);}
                format!("Durable task acceptance state (read task_execution.get when requirementsRevision or currentGoalRequirements changes before continuing; stale/missing goal bindings require explicit user revision; never replay unknown effects): {}{}",summary.to_string().chars().take(6000).collect::<String>(),if changed==Some(true) {"\nTASK_ENVIRONMENT_CHANGED: Stop issuing write or execution tools. Ask the user to use 任务验收 → 切换到当前环境 and confirm, or cancel the old task when its objective is no longer relevant. Do not retry with other tools, reinterpret running as migrated, or claim an environment probe changed permissions. Migration preserves requirements and effect history and invalidates old acceptance."} else {""})
            }
            Ok(None)=>String::new(),
            Err(error)=>format!("Durable task state unavailable: {error}; do not infer completion from chat history."),
        }
    }))});
    Ok(service)
}

pub(crate) fn register_route(
    server: &Arc<dsh_host_webserver::WebServer>,
    service: Arc<TaskExecution>,
    api: Arc<dsh_host_apiproxy::proxy::ApiProxyService>,
    allow_remote_host: bool,
) -> dsh_host_webserver::RouteDisposer {
    use axum::body::{Body, to_bytes};
    use http::{Method, Response, StatusCode, header};
    server.register(dsh_host_webserver::WebRoute {
        kind: dsh_host_webserver::WebRouteKind::Prefix,
        path: "/__dsh-task-execution".into(),
        handler: Arc::new(move |request| {
            let service = service.clone();
            let api = api.clone();
            Box::pin(async move {
                let trusted = request
                    .headers()
                    .get(header::HOST)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|host| super::allowed_web_authority(host, allow_remote_host))
                    && super::trusted_web_request(&request, allow_remote_host);
                let (status, value) = if !trusted {
                    (StatusCode::FORBIDDEN, json!({"error":"forbidden"}))
                } else if request.method() != Method::POST {
                    (
                        StatusCode::METHOD_NOT_ALLOWED,
                        json!({"error":"POST required"}),
                    )
                } else {
                    let result = async {
                        let bytes = to_bytes(Body::new(request.into_body()), 256 * 1024)
                            .await
                            .map_err(|e| e.to_string())?;
                        let args: Value =
                            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                        let owner = text(&args, "sessionId")?;
                        // Reading durable contracts must not resurrect an idle
                        // Agent just to populate a recovery panel.
                        if args["action"]=="list" {return Ok::<_,TaskActionError>(service.decorate_response(json!({"tasks":service.runtime.list(owner)?,"capabilities":capabilities()}),owner).await);}
                        if args["action"]=="refresh_history" {return Ok(json!({"history":service.runtime.evidence_refresh_history(owner,text(&args,"taskId")?)?}));}
                        if args["action"]=="requirements_history" {return Ok(json!({"history":service.runtime.requirements_history(owner,text(&args,"taskId")?)?,"capabilities":capabilities()}));}
                        if args["action"]=="requirements_snapshot" {return Ok(json!({"snapshot":service.runtime.requirements_snapshot(owner,text(&args,"taskId")?,text(&args,"idempotencyKey")?)?,"capabilities":capabilities()}));}
                        if matches!(args["action"].as_str(),Some("get"|"recover")){
                            let task=service.runtime.get(owner,text(&args,"taskId")?)?;
                            return Ok(service.decorate_response(task_response(task),owner).await);
                        }
                        let requested_action = args["action"].as_str().unwrap_or("");
                        let revision = if requested_action == "revise" {
                            let revision = UserRevision::parse(&args)?;
                            if let Some(task) = service.runtime.replay_bound_revision(owner,text(&args,"taskId")?,&revision.key,revision.expected,&revision.spec,revision.mode,revision.expected_goal_binding.as_ref())? {
                                return Ok(service.decorate_response(task_response(task),owner).await);
                            }
                            Some(revision)
                        } else { None };
                        let prepared_work = if validation_action(requested_action, true) {
                            let id = text(&args,"taskId")?;
                            service.runtime.get(owner,id)?;
                            Some(service.validation_work.begin(owner,id,Arc::new(|| false)))
                        } else { None };
                        if matches!(requested_action,"cancel" | "stop_validation" | "migrate_environment") {
                            let id = text(&args,"taskId")?;
                            service.runtime.get(owner,id)?;
                            service.validation_work.cancel(owner,id);
                        }
                        let lease = if let Some(work) = &prepared_work {
                            validation_work::until_cancelled(api.resolve_control_agent(owner),work.signal.clone()).await
                                .map_err(|error| if (work.signal)() {TaskActionError::Cancelled} else {TaskActionError::Failed(error)})?
                        } else if revision.is_some() {
                            api.try_resolve_control_agent(owner).await?.ok_or(RevisionError::Busy)?
                        } else { api.resolve_control_agent(owner).await? };
                        let cwd = lease
                            .agent
                            .session()
                            .header()
                            .cwd
                            .as_deref()
                            .ok_or("Session workspace unavailable")?;
                        if let Some(revision) = revision {
                            let value=service.revise_with_control(&lease.agent,cwd,text(&args,"taskId")?,revision)?;
                            return Ok(service.decorate_response(value,owner).await);
                        }
                        let value = service
                            .action_with_work(
                                lease.agent.id().as_str(),
                                cwd,
                                &args,
                                Arc::new(|| false),
                                true,
                                prepared_work,
                            )
                            .await?;
                        if args["action"] == "cancel" && value["task"]["state"] == "cancelled" {
                            lease.agent.cancel(dsh_agent::AgentCancelCause::User, None);
                        }
                        Ok::<_, TaskActionError>(value)
                    }
                    .await;
                    match result {
                        Ok(value) => (StatusCode::OK, value),
                        Err(error) => (StatusCode::BAD_REQUEST, json!({"error":error.to_string(),"code":error.code()})),
                    }
                };
                Ok(Response::builder()
                    .status(status)
                    .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                    .header(header::CACHE_CONTROL, "no-store")
                    .body(Body::from(value.to_string()))
                    .expect("task execution response"))
            })
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_contract_schema_rejects_stringified_objects_before_dispatch() {
        let contract = json!({"objective":"Check result","acceptanceChecks":[{"id":"exists","description":"Expected output","checker":{"kind":"text","path":"result.txt","required":["done"]}}]});
        let schema = model_parameters();
        assert!(dsh_tools::validate_json_schema_value(&schema, &json!({"action":"create","contract":contract}), "arguments").is_empty());
        for bad in [json!(contract.to_string()), json!("{broken"), json!([]), json!(null)] {
            let errors = dsh_tools::validate_json_schema_value(&schema, &json!({"action":"create","contract":bad}), "arguments");
            assert!(errors.iter().any(|error|error.contains("arguments.contract") && error.contains("object")), "{errors:?}");
        }
    }
    #[test]
    fn cancellation_classification_never_trusts_error_message_text() {
        assert_eq!(TaskActionError::Cancelled.code(), "CANCELLED");
        assert_eq!(
            TaskActionError::from("Task validation cancelled").code(),
            "TASK_EXECUTION_FAILED"
        );
        assert!(validation_action("refresh_evidence", true));
        assert!(!validation_action("refresh_evidence", false));
        assert_eq!(
            TaskActionError::Revision(RevisionError::Conflict).code(),
            "TASK_REVISION_CONFLICT"
        );
        assert_eq!(
            TaskActionError::from("TASK_REVISION_CONFLICT").code(),
            "TASK_EXECUTION_FAILED"
        );
    }
    #[test]
    fn user_revision_requires_explicit_mode_cas_and_key_and_is_absent_from_model_actions() {
        let request = json!({"expectedRevision":3,"idempotencyKey":"edit-1","mode":"in_place","contract":{"objective":"Revised task","acceptanceChecks":[]}});
        let parsed = UserRevision::parse(&request).unwrap();
        assert_eq!(parsed.expected, 3);
        assert_eq!(parsed.key, "edit-1");
        assert_eq!(parsed.mode, RevisionMode::InPlace);
        for key in ["expectedRevision", "idempotencyKey", "mode", "contract"] {
            let mut missing = request.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(UserRevision::parse(&missing).is_err(), "{key}");
        }
        let schema = model_parameters();
        let actions = schema["properties"]["action"]["enum"].as_array().unwrap();
        for action in ["revise", "requirements_history", "requirements_snapshot"] {
            assert!(!actions.contains(&json!(action)));
        }
        assert_eq!(
            capabilities(),
            json!({"revise":true,"requirementsHistory":true,"requirementsSnapshot":true})
        );
    }
    #[test]
    fn successful_steps_do_not_reinject_task_context_but_unknown_effects_do() {
        let mut task:TaskContract=serde_json::from_value(json!({"version":1,"taskId":"t","owner":"s","revision":1,"spec":{"objective":"Check","acceptanceChecks":[]},"state":"running","steps":[],"acceptanceResults":[],"validationIdentity":null,"outputIdentities":{},"createdAt":1,"updatedAt":1})).unwrap();
        let before = prompt_task_state(&task);
        task.revision = 42;
        task.updated_at = 999;
        let mut step:Step=serde_json::from_value(json!({"id":"read","executionId":"e","idempotencyKey":"k","inputIdentity":"i","tool":"read","effect":"read_only","state":"verified","updatedAt":999,"process":null,"resultIdentity":null,"result":null,"evidenceRefs":[],"failureReason":null})).unwrap();
        task.steps.push(step.clone());
        assert_eq!(before, prompt_task_state(&task));
        task.requirements_revision += 1;
        assert_ne!(
            before,
            prompt_task_state(&task),
            "user edits change the next model context"
        );
        task.requirements_revision -= 1;
        step.id = "write".into();
        step.effect = EffectKind::Write;
        step.state = StepState::Unknown;
        task.steps.push(step);
        let unresolved = prompt_task_state(&task);
        assert_ne!(before, unresolved);
        assert!(unresolved["blockers"].to_string().contains("Unknown"));
        assert_eq!(unresolved["recovery"][0]["action"], "inspect_effects");
        task.state = TaskState::Cancelled;
        assert_ne!(unresolved, prompt_task_state(&task));
    }

    #[test]
    fn team_observation_does_not_require_environment_migration() {
        for action in ["status", "wait"] {
            assert_eq!(
                effect("agent_team", &json!({"action":action})),
                EffectKind::ReadOnly
            );
        }
        for action in [
            "create",
            "message",
            "task",
            "dispatch",
            "interrupt",
            "unknown",
            "",
        ] {
            assert_eq!(
                effect("agent_team", &json!({"action":action})),
                EffectKind::Write
            );
        }
        assert_eq!(effect("agent_team", &json!({})), EffectKind::Write);
    }
    #[test]
    fn invalid_acceptance_references_are_rejected_before_contract_creation() {
        let mut spec: ContractSpec=serde_json::from_value(json!({"objective":"Check build","acceptanceChecks":[{"id":"test","description":"test succeeds","checker":{"kind":"tool_result","step_id":"invented-test-step","assertions":{"exitCode":0}}}]})).unwrap();
        let error = validate_model_contract(&spec).unwrap_err();
        assert!(error.contains("tool:execute_native") && error.contains("/exitCode"));
        spec.acceptance_checks[0].checker = Checker::ToolResult {
            step_id: "tool:execute_native".into(),
            assertions: BTreeMap::from([("/exitCode".into(), json!(0))]),
        };
        validate_model_contract(&spec).unwrap();
        dsh_tools::assert_object_json_schema(&model_parameters()).unwrap();
    }
    #[test]
    fn silent_terminal_observation_is_not_command_success() {
        assert!(matches!(
            effect("consult_model", &json!({})),
            EffectKind::ReadOnly
        ));
        assert!(matches!(
            effect("computer_use_js", &json!({})),
            EffectKind::Write
        ));
        assert_eq!(
            outcome_flags(
                "terminal_send",
                Some(&json!({"completion":"unknown","terminalState":"running"})),
                false
            ),
            (false, false)
        );
        assert_eq!(
            outcome_flags(
                "pwsh",
                Some(&json!({"jobId":"job1","kind":"background"})),
                false
            ),
            (true, true)
        );
        assert_eq!(
            outcome_flags(
                "job_output",
                Some(&json!({"job":{"status":"running"}})),
                false
            ),
            (true, false)
        );
    }
    #[test]
    fn native_nonzero_and_cancellation_do_not_turn_into_success() {
        assert_eq!(
            outcome_flags("execute_native", Some(&json!({"exitCode":1})), false),
            (false, false)
        );
        assert_eq!(
            outcome_flags("pwsh", Some(&json!({"exitCode":0,"aborted":true})), false),
            (false, false)
        );
        assert_eq!(
            outcome_flags("pwsh", Some(&json!({"exitCode":0})), false),
            (true, false)
        );
    }
}
