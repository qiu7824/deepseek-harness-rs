//! Trusted goal requirements and fresh completion proof through the owner's FS.
use super::*;
use dsh_goal::{
    GoalCompletionCommitGuard, GoalCompletionError, GoalCompletionGuard, GoalCompletionPermit,
    GoalRequirementsIdentity,
};
use std::sync::Weak;
#[cfg(test)]
#[path = "task_goal_tests.rs"]
mod tests;

fn binding(identity: &GoalRequirementsIdentity) -> GoalBinding {
    GoalBinding {
        goal_id: identity.goal_id.clone(),
        objective_revision: identity.objective_revision,
    }
}

#[derive(Clone, Debug)]
pub(super) struct GoalFacts {
    pub binding: Option<GoalBinding>,
    pub objective: Option<String>,
    pub unavailable: bool,
}
impl GoalFacts {
    fn from_owned_projection(mut value: Value) -> Self {
        if value.is_null() {
            return Self::from_requirements(None);
        }
        let Some(Value::Object(mut goal)) = value
            .as_object_mut()
            .and_then(|object| object.remove("goal"))
        else {
            return Self::from_requirements(Some(Value::Null));
        };
        let mut requirements = serde_json::Map::new();
        requirements.insert("goalId".into(), goal.remove("id").unwrap_or(Value::Null));
        requirements.insert(
            "objectiveRevision".into(),
            goal.remove("objectiveRevision").unwrap_or(Value::Null),
        );
        requirements.insert(
            "objective".into(),
            goal.remove("objective").unwrap_or(Value::Null),
        );
        Self::from_requirements(Some(Value::Object(requirements)))
    }
    fn from_requirements(value: Option<Value>) -> Self {
        let Some(mut value) = value else {
            return Self {
                binding: None,
                objective: None,
                unavailable: false,
            };
        };
        let valid = matches!((value["goalId"].as_str(),value["objectiveRevision"].as_u64(),value["objective"].as_str()),(Some(id),Some(revision),Some(_)) if !id.is_empty() && revision>0);
        if !valid {
            return Self {
                binding: None,
                objective: None,
                unavailable: true,
            };
        }
        let goal_id = value["goalId"].as_str().unwrap().to_owned();
        let objective_revision = value["objectiveRevision"].as_u64().unwrap();
        let objective = value
            .as_object_mut()
            .unwrap()
            .remove("objective")
            .and_then(|value| {
                if let Value::String(text) = value {
                    Some(text)
                } else {
                    None
                }
            });
        Self {
            binding: Some(GoalBinding {
                goal_id,
                objective_revision,
            }),
            objective,
            unavailable: false,
        }
    }
    fn unavailable() -> Self {
        Self {
            binding: None,
            objective: None,
            unavailable: true,
        }
    }
    pub fn status(&self, task: &TaskContract) -> &'static str {
        self.status_fields(task.spec.goal_id.as_deref(), task.goal_binding.as_ref())
    }
    fn status_fields(&self, goal_id: Option<&str>, binding: Option<&GoalBinding>) -> &'static str {
        if goal_id.is_none() {
            "unlinked"
        } else if self.unavailable {
            "unavailable"
        } else if binding.is_none_or(|value| {
            Some(value.goal_id.as_str()) != goal_id || value.objective_revision == 0
        }) {
            "missing"
        } else if binding == self.binding.as_ref() {
            "current"
        } else {
            "stale"
        }
    }
    pub fn blockers(&self, task: &TaskContract) -> Vec<String> {
        self.blocker_fields(task.spec.goal_id.as_deref(), task.goal_binding.as_ref())
    }
    fn blocker_fields(&self, goal_id: Option<&str>, binding: Option<&GoalBinding>) -> Vec<String> {
        match self.status_fields(goal_id,binding) {
            "unavailable"=>vec!["Current goal requirements are unavailable; acceptance reuse is blocked until they can be read".into()],
            "missing"=>vec!["Linked task has no trusted goal requirements binding; explicitly revise or create a successor before reuse".into()],
            "stale"=>vec!["Goal requirements changed or the linked goal is no longer current; old acceptance does not satisfy the current goal".into()],
            _=>Vec::new(),
        }
    }
    fn into_public_requirements(self) -> Value {
        let Some(binding) = self.binding else {
            return Value::Null;
        };
        let mut value = serde_json::Map::new();
        value.insert("goalId".into(), Value::String(binding.goal_id));
        value.insert(
            "objectiveRevision".into(),
            json!(binding.objective_revision),
        );
        value.insert(
            "objective".into(),
            self.objective.map_or(Value::Null, Value::String),
        );
        Value::Object(value)
    }
}

pub(super) fn projection_definition() -> dsh_session_projection::ProjectionDefinition {
    dsh_session_projection::ProjectionDefinition {
        key: "goal".into(),
        state_version: 1,
        init: Arc::new(|_| cordis::arc(Value::Null)),
        apply: Arc::new(|previous, event| {
            let value = cordis::downcast::<Value>(previous).expect("goal projection JSON state");
            dsh_goal::apply_goal_requirements_projection(value, event)
                .map_or_else(|| previous.clone(), cordis::arc)
        }),
        view: Arc::new(Clone::clone),
        schema: Arc::new(|value| {
            let value = cordis::downcast::<Value>(value).ok_or("Goal projection must be JSON")?;
            let goal = &value["goal"];
            if !value.is_null()
                && !matches!((goal["id"].as_str(),goal["objectiveRevision"].as_u64(),goal["objective"].as_str()),(Some(id),Some(revision),Some(_)) if !id.is_empty() && revision>0)
            {
                return Err("Invalid goal requirements projection".into());
            }
            Ok(value.clone())
        }),
    }
}

pub(super) fn install(service: &Arc<TaskExecution>) -> Result<()> {
    let registry = service
        .context
        .get_typed::<Arc<dsh_session_projection::SessionProjectionRegistry>>(
            "sessionProjections",
            false,
        )
        .ok_or("Session projections unavailable")?;
    registry.register(&service.context, projection_definition())?;
    let guard: Arc<dyn GoalCompletionGuard> = Arc::new(CompletionGuard {
        tasks: Arc::downgrade(service),
    });
    service.context.provide(
        dsh_goal::GOAL_COMPLETION_GUARD_SERVICE,
        Some(cordis::arc(guard)),
    );
    Ok(())
}

impl TaskExecution {
    pub(super) async fn goal_facts(&self, owner: &str) -> GoalFacts {
        let id = dsh_session::session_id(owner);
        if let Some(session) = self
            .context
            .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
            .and_then(|store| store.get(&id))
        {
            if let Some(goals) = self
                .context
                .get_typed::<Arc<dsh_goal::GoalService>>("goals", false)
            {
                return GoalFacts::from_requirements(goals.requirements_view_for_session(&session));
            }
            return GoalFacts::unavailable();
        }
        let Some(persistence) = self
            .context
            .get_typed::<Arc<dyn dsh_session_persistence::SessionPersistenceApi>>(
                "sessionPersistence",
                false,
            )
        else {
            return GoalFacts::unavailable();
        };
        let state = Arc::new(parking_lot::Mutex::new(Value::Null));
        let folded = state.clone();
        let read = persistence
            .visit_goal_events_bounded(
                &id,
                Arc::new(move |events| {
                    let mut state = folded.lock();
                    for event in events {
                        if event.type_ == "goal/change"
                            && dsh_goal::decode_goal_change(&event.data)?.is_none()
                        {
                            return Err("Invalid goal requirements event".into());
                        }
                        if let Some(next) =
                            dsh_goal::apply_goal_requirements_projection(&state, event)
                        {
                            *state = next;
                        }
                    }
                    Ok(true)
                }),
            )
            .await;
        if read.is_err() {
            return GoalFacts::unavailable();
        }
        let value = std::mem::take(&mut *state.lock());
        GoalFacts::from_owned_projection(value)
    }

    pub(super) async fn decorate_response(&self, value: Value, owner: &str) -> Value {
        let facts = self.goal_facts(owner).await;
        decorate_with_facts(value, facts)
    }
}
fn decorate_with_facts(mut value: Value, facts: GoalFacts) -> Value {
    fn decorate(task: &mut Value, facts: &GoalFacts) -> Vec<String> {
        let goal_id = task["spec"]["goalId"].as_str();
        let binding = task
            .get("goalBinding")
            .and_then(|value| serde_json::from_value::<GoalBinding>(value.clone()).ok());
        let status = facts.status_fields(goal_id, binding.as_ref());
        let blockers = facts.blocker_fields(goal_id, binding.as_ref());
        task["goalBindingStatus"] = json!(status);
        blockers
    }
    if let Some(task_value) = value.get_mut("task") {
        let extra = decorate(task_value, &facts);
        if let Some(blockers) = value["blockers"].as_array_mut() {
            for blocker in extra {
                let blocker = json!(blocker);
                if !blockers.contains(&blocker) {
                    blockers.push(blocker);
                }
            }
        }
    }
    if let Some(tasks) = value.get_mut("tasks").and_then(Value::as_array_mut) {
        for task in tasks {
            decorate(task, &facts);
        }
    }
    value["currentGoalRequirements"] = facts.into_public_requirements();
    value
}
impl TaskExecution {
    pub(super) async fn assert_goal_applicable(&self, task: &TaskContract) -> Result<()> {
        if task.spec.goal_id.is_none() {
            return Ok(());
        }
        let blockers = self.goal_facts(&task.owner).await.blockers(task);
        if blockers.is_empty() {
            Ok(())
        } else {
            Err(blockers.join("; "))
        }
    }

    pub(super) fn capture_goal_binding(
        &self,
        owner: &str,
        requested: Option<&str>,
    ) -> std::result::Result<
        (Option<GoalBinding>, Option<dsh_goal::GoalRequirementsLease>),
        TaskActionError,
    > {
        let Some(requested) = requested else {
            return Ok((None, None));
        };
        let agent = self
            .context
            .get_typed::<Arc<dsh_agent::AgentRegistry>>("agents", false)
            .and_then(|registry| registry.get(&dsh_session::session_id(owner)))
            .ok_or_else(|| {
                TaskActionError::Failed("Goal binding requires the exact live session".into())
            })?;
        let goals = self
            .context
            .get_typed::<Arc<dsh_goal::GoalService>>("goals", false)
            .ok_or_else(|| TaskActionError::Failed("Goal service unavailable".into()))?;
        let lease = goals
            .claim_requirements(&agent)
            .map_err(|_| RevisionError::Busy)?;
        let identity = lease
            .identity()
            .filter(|identity| identity.goal_id == requested)
            .ok_or(RevisionError::GoalRequirementsChanged)?;
        Ok((Some(binding(identity)), Some(lease)))
    }

    pub(super) fn capture_created_binding(
        &self,
        owner: &str,
        spec: &mut ContractSpec,
        user_control: bool,
    ) -> std::result::Result<
        (Option<GoalBinding>, Option<dsh_goal::GoalRequirementsLease>),
        TaskActionError,
    > {
        if user_control {
            return self.capture_goal_binding(owner, spec.goal_id.as_deref());
        }
        let Some(goals) = self
            .context
            .get_typed::<Arc<dsh_goal::GoalService>>("goals", false)
        else {
            if spec.goal_id.is_none() {
                return Ok((None, None));
            }
            return Err(RevisionError::GoalRequirementsChanged.into());
        };
        let agent = self
            .context
            .get_typed::<Arc<dsh_agent::AgentRegistry>>("agents", false)
            .and_then(|registry| registry.get(&dsh_session::session_id(owner)))
            .ok_or_else(|| {
                TaskActionError::Failed("Goal binding requires the exact live session".into())
            })?;
        let lease = goals
            .claim_requirements(&agent)
            .map_err(|_| RevisionError::Busy)?;
        let selected = model_binding(spec.goal_id.as_deref(), lease.identity(), lease.phase())?;
        spec.goal_id = selected.as_ref().map(|binding| binding.goal_id.clone());
        Ok((selected, Some(lease)))
    }

    pub(super) fn claim_task_goal(
        &self,
        task: &TaskContract,
    ) -> Result<Option<dsh_goal::GoalRequirementsLease>> {
        let (current, lease) = self
            .capture_goal_binding(&task.owner, task.spec.goal_id.as_deref())
            .map_err(|error| error.to_string())?;
        let blockers = task.goal_binding_blockers(current.as_ref());
        if !blockers.is_empty() {
            return Err(blockers.join("; "));
        }
        Ok(lease)
    }
}

fn model_binding(
    requested: Option<&str>,
    current: Option<&GoalRequirementsIdentity>,
    phase: Option<dsh_goal::GoalPhase>,
) -> std::result::Result<Option<GoalBinding>, RevisionError> {
    let active =
        current.filter(|_| phase.is_some_and(|phase| phase != dsh_goal::GoalPhase::Complete));
    match (requested, active) {
        (None, Some(identity)) => Ok(Some(binding(identity))),
        (Some(id), Some(identity)) if id == identity.goal_id => Ok(Some(binding(identity))),
        (None, None) => Ok(None),
        _ => Err(RevisionError::GoalRequirementsChanged),
    }
}

pub(super) fn add_prompt_binding(summary: &mut Value, task: &TaskContract, ctx: &Context) {
    if task.spec.goal_id.is_none() {
        return;
    }
    let id = dsh_session::session_id(&task.owner);
    let current = ctx
        .get_typed::<Arc<dsh_session::SessionStore>>("sessions", false)
        .and_then(|store| store.get(&id))
        .and_then(|session| {
            ctx.get_typed::<Arc<dsh_goal::GoalService>>("goals", false)
                .map(|goals| goals.requirements_identity_for_session(&session))
        });
    let facts = GoalFacts {
        binding: current
            .as_ref()
            .and_then(|identity| identity.as_ref())
            .map(binding),
        objective: None,
        unavailable: current.is_none(),
    };
    add_summary_facts(summary, task, &facts);
}
fn add_summary_facts(summary: &mut Value, task: &TaskContract, facts: &GoalFacts) {
    summary["goalBindingStatus"] = json!(facts.status(task));
    summary["goalBinding"] = json!(task.goal_binding);
    // No objective/phase/round counters in this context key: ordinary usage
    // does not trigger reinjection, and long goal text is not duplicated.
    summary["currentGoalRequirements"] = json!(facts.binding);
    if let Some(blockers) = summary["blockers"].as_array_mut() {
        for blocker in facts.blockers(task) {
            let blocker = json!(blocker);
            if !blockers.contains(&blocker) {
                blockers.push(blocker);
            }
        }
    }
}

struct CompletionGuard {
    tasks: Weak<TaskExecution>,
}
struct CompletionPermit {
    tasks: Weak<TaskExecution>,
    owner: String,
    task: Option<(String, u64)>,
    goal: GoalBinding,
    work: Option<validation_work::Guard>,
}
enum CompletionCommit<'a> {
    Verified(validation_work::CommitGuard<'a>),
    Unlinked,
}
impl GoalCompletionCommitGuard for CompletionCommit<'_> {}
impl CompletionPermit {
    fn commit_guard(&self) -> std::result::Result<CompletionCommit<'_>, GoalCompletionError> {
        match &self.work {
            Some(work) => work
                .commit()
                .map(CompletionCommit::Verified)
                .map_err(|_| GoalCompletionError::Cancelled),
            None => Ok(CompletionCommit::Unlinked),
        }
    }
}
impl GoalCompletionPermit for CompletionPermit {
    fn check<'a>(
        &'a self,
        agent: &Arc<dyn dsh_agent::Agent>,
        identity: &GoalRequirementsIdentity,
    ) -> std::result::Result<Box<dyn GoalCompletionCommitGuard + 'a>, GoalCompletionError> {
        if agent.id().as_str() != self.owner || binding(identity) != self.goal {
            return Err(GoalCompletionError::Blocked(
                "Goal acceptance belongs to a different owner or requirements".into(),
            ));
        }
        let commit = self.commit_guard()?;
        let tasks = self.tasks.upgrade().ok_or_else(|| {
            GoalCompletionError::Blocked("Task acceptance service unavailable".into())
        })?;
        let current = tasks
            .runtime
            .latest_for_goal(&self.owner, &self.goal.goal_id)?;
        match (&self.task, current) {
            (None, None) => {}
            (Some((id, revision)), Some(task))
                if &task.task_id == id
                    && task.revision == *revision
                    && task.state == TaskState::Completed
                    && task.goal_binding_blockers(Some(&self.goal)).is_empty()
                    && task.completion_blockers().is_empty() => {}
            _ => return Err(GoalCompletionError::Blocked(
                "The current goal has no unchanged completed contract satisfying its requirements"
                    .into(),
            )),
        }
        if self.work.as_ref().is_some_and(|work| (work.signal)()) {
            return Err(GoalCompletionError::Cancelled);
        }
        Ok(Box::new(commit))
    }
}
#[async_trait::async_trait]
impl GoalCompletionGuard for CompletionGuard {
    async fn prepare(
        &self,
        agent: &Arc<dyn dsh_agent::Agent>,
        identity: &GoalRequirementsIdentity,
    ) -> std::result::Result<Box<dyn GoalCompletionPermit>, GoalCompletionError> {
        let tasks = self.tasks.upgrade().ok_or_else(|| {
            GoalCompletionError::Blocked("Task acceptance service unavailable".into())
        })?;
        let owner = agent.id().as_str();
        let goal = binding(identity);
        let Some(task) = tasks.runtime.latest_for_goal(owner, &goal.goal_id)? else {
            return Ok(Box::new(CompletionPermit {
                tasks: Arc::downgrade(&tasks),
                owner: owner.into(),
                task: None,
                goal,
                work: None,
            }));
        };
        if task.state != TaskState::Completed
            || !task.goal_binding_blockers(Some(&goal)).is_empty()
            || !task.completion_blockers().is_empty()
        {
            return Err(GoalCompletionError::Blocked("Complete a contract verified for the current goal requirements before completing the goal".into()));
        }
        let cwd = agent
            .session()
            .header()
            .cwd
            .as_deref()
            .ok_or_else(|| GoalCompletionError::Blocked("Goal workspace unavailable".into()))?;
        let generation = agent.cancellation_generation().ok_or_else(|| {
            GoalCompletionError::Blocked(
                "Goal completion requires cancellation-aware execution".into(),
            )
        })?;
        let weak = Arc::downgrade(agent);
        let signal: dsh_tools::AbortPredicate = Arc::new(move || {
            weak.upgrade()
                .is_none_or(|agent| agent.cancellation_generation() != Some(generation))
        });
        let (verified, work) = tasks
            .verified_evidence_guarded(owner, &task.task_id, task.revision, cwd, signal)
            .await
            .map_err(|error| match error {
                TaskActionError::Cancelled => GoalCompletionError::Cancelled,
                other => GoalCompletionError::Blocked(other.to_string()),
            })?;
        if (work.signal)() {
            return Err(GoalCompletionError::Cancelled);
        }
        // No new begin: StopValidation and supersession remain attached to this permit.
        Ok(Box::new(CompletionPermit {
            tasks: Arc::downgrade(&tasks),
            owner: owner.into(),
            task: Some((verified.task_id, verified.revision)),
            goal,
            work: Some(work),
        }))
    }
}
