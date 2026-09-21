use crate::*;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    fs::{File, OpenOptions},
    path::Path,
    time::Duration,
};

const MAX_CONTRACT: usize = 2 * 1024 * 1024;
const MAX_STEPS: usize = 512;
// Record format remains v1; older Hosts must not overwrite revision metadata.
const DATABASE_VERSION: u32 = 2;
mod revise;
pub struct TaskRuntime {
    db: Mutex<Connection>,
    _lease: File,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b':'))
}
fn validate(spec: &ContractSpec) -> Result<()> {
    if spec.objective.trim().is_empty()
        || spec.objective.len() > 20_000
        || spec.acceptance_checks.is_empty()
        || spec.acceptance_checks.len() > 64
        || spec.expected_outputs.len() > 64
        || spec.constraints.len() > 64
    {
        return Err("Invalid or oversized task contract".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    for check in &spec.acceptance_checks {
        if !valid_id(&check.id) || !ids.insert(&check.id) || check.description.len() > 4096 {
            return Err("Invalid or duplicate acceptance check".into());
        }
        if check
            .checker
            .path()
            .is_some_and(|p| p.is_empty() || p.len() > 4096 || p.chars().any(char::is_control))
        {
            return Err("Invalid acceptance path".into());
        }
        match &check.checker {
            Checker::Text {
                required,
                forbidden,
                ..
            } if required.is_empty() && forbidden.is_empty() => {
                return Err("Text checks require content assertions".into());
            }
            Checker::Json { assertions, .. } | Checker::ToolResult { assertions, .. }
                if assertions.is_empty() =>
            {
                return Err("JSON/tool checks require content assertions".into());
            }
            Checker::Manual { reason } if reason.trim().is_empty() => {
                return Err("Manual acceptance requires a coverage explanation".into());
            }
            _ => {}
        }
    }
    if serde_json::to_vec(spec).map_err(|e| e.to_string())?.len() > 128 * 1024 {
        return Err("Task specification exceeds 128 KiB".into());
    }
    Ok(())
}

fn load(db: &Connection, owner: &str, id: &str) -> Result<TaskContract> {
    let text: Option<String> = db
        .query_row(
            "SELECT body FROM tasks WHERE owner=?1 AND task_id=?2",
            params![owner, id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let task: TaskContract = serde_json::from_str(&text.ok_or("Task not found for this owner")?)
        .map_err(|e| format!("Invalid task record: {e}"))?;
    if task.version != FORMAT_VERSION || task.owner != owner || task.task_id != id {
        return Err("Task identity or format mismatch".into());
    }
    Ok(task)
}
fn persist(db: &Connection, task: &TaskContract) -> Result<()> {
    let body = serde_json::to_string(task).map_err(|e| e.to_string())?;
    if body.len() > MAX_CONTRACT {
        return Err("Task record exceeds durable budget".into());
    }
    db.execute("INSERT INTO tasks(owner,task_id,state,body) VALUES(?1,?2,?3,?4) ON CONFLICT(owner,task_id) DO UPDATE SET state=excluded.state,body=excluded.body",params![task.owner,task.task_id,format!("{:?}",task.state),body]).map_err(|e|e.to_string())?;
    Ok(())
}

impl TaskRuntime {
    /// OS lock prevents a second live Host from recovering the first Host's executions.
    /// SQLite FULL synchronous transactions persist intent before dispatch and are crash-safe.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let lease = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lease"))
            .map_err(|e| e.to_string())?;
        lease
            .try_lock()
            .map_err(|e| format!("Task runtime is already owned by another Host: {e}"))?;
        let mut db = Connection::open(path).map_err(|e| e.to_string())?;
        let version: u32 = db
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|e| e.to_string())?;
        if version > DATABASE_VERSION {
            return Err("Task runtime database was created by a newer version".into());
        }
        db.busy_timeout(Duration::from_secs(2))
            .map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
            .map_err(|e| e.to_string())?;
        let migration = db.transaction().map_err(|e| e.to_string())?;
        migration.execute_batch("CREATE TABLE IF NOT EXISTS tasks(owner TEXT NOT NULL,task_id TEXT NOT NULL,state TEXT NOT NULL,body TEXT NOT NULL,PRIMARY KEY(owner,task_id)); CREATE TABLE IF NOT EXISTS mutations(owner TEXT NOT NULL,task_id TEXT NOT NULL,key TEXT NOT NULL,digest TEXT NOT NULL,response TEXT NOT NULL,PRIMARY KEY(owner,task_id,key));").map_err(|e|e.to_string())?;
        migration.execute_batch("CREATE TABLE IF NOT EXISTS acceptance_refreshes(owner TEXT NOT NULL,task_id TEXT NOT NULL,revision INTEGER NOT NULL,key TEXT NOT NULL,created_at INTEGER NOT NULL,passed INTEGER NOT NULL,body TEXT NOT NULL,PRIMARY KEY(owner,task_id,revision));").map_err(|e|e.to_string())?;
        migration.execute_batch("CREATE TABLE IF NOT EXISTS contract_revisions(owner TEXT NOT NULL,task_id TEXT NOT NULL,key TEXT NOT NULL,source_revision INTEGER NOT NULL,target_task_id TEXT NOT NULL,target_revision INTEGER NOT NULL,created_at INTEGER NOT NULL,prior_body TEXT NOT NULL,PRIMARY KEY(owner,task_id,key)); CREATE INDEX IF NOT EXISTS contract_revisions_target ON contract_revisions(owner,target_task_id);").map_err(|e|e.to_string())?;
        migration
            .pragma_update(None, "user_version", DATABASE_VERSION)
            .map_err(|e| e.to_string())?;
        migration.commit().map_err(|e| e.to_string())?;
        let store = Self {
            db: Mutex::new(db),
            _lease: lease,
        };
        store.recover_after_restart()?;
        Ok(store)
    }
    pub fn get(&self, owner: &str, id: &str) -> Result<TaskContract> {
        load(&self.db.lock(), owner, id)
    }
    pub fn active(&self, owner: &str) -> Result<Option<TaskContract>> {
        let db = self.db.lock();
        let id:Option<String>=db.query_row("SELECT task_id FROM tasks WHERE owner=?1 AND state NOT IN ('Completed','Cancelled') ORDER BY rowid DESC LIMIT 1",params![owner],|row|row.get(0)).optional().map_err(|e|e.to_string())?;
        id.map(|id| load(&db, owner, &id)).transpose()
    }
    pub fn latest(&self, owner: &str) -> Result<Option<TaskContract>> {
        let db = self.db.lock();
        let id: Option<String> = db
            .query_row(
                "SELECT task_id FROM tasks WHERE owner=?1 ORDER BY rowid DESC LIMIT 1",
                params![owner],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        id.map(|id| load(&db, owner, &id)).transpose()
    }
    pub fn list(&self, owner: &str) -> Result<Vec<TaskContract>> {
        let db = self.db.lock();
        let mut stmt = db
            .prepare("SELECT task_id FROM tasks WHERE owner=?1 ORDER BY rowid DESC LIMIT 64")
            .map_err(|e| e.to_string())?;
        let ids = stmt
            .query_map(params![owner], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        ids.iter().map(|id| load(&db, owner, id)).collect()
    }
    pub fn create(&self, owner: &str, id: &str, spec: ContractSpec) -> Result<TaskContract> {
        if !valid_id(owner) || !valid_id(id) {
            return Err("Invalid task/owner identity".into());
        }
        validate(&spec)?;
        let mut db = self.db.lock();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        if let Ok(existing) = load(&tx, owner, id) {
            if existing.spec == spec {
                return Ok(existing);
            }
            return Err("Task already exists with different immutable requirements".into());
        }
        let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM tasks WHERE owner=?1 AND state NOT IN ('Completed','Cancelled'))",params![owner],|row|row.get(0)).map_err(|e|e.to_string())?;
        if active {
            return Err("Finish or cancel the current contract before creating another".into());
        }
        let task = TaskContract {
            version: FORMAT_VERSION,
            task_id: id.into(),
            owner: owner.into(),
            revision: 1,
            requirements_revision: 1,
            based_on: None,
            spec,
            state: TaskState::Planned,
            steps: vec![],
            acceptance_results: vec![],
            acceptance_refresh: None,
            validation_identity: None,
            output_identities: BTreeMap::new(),
            validation_subject_evidence: None,
            created_at: now(),
            updated_at: now(),
        };
        persist(&tx, &task)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(task)
    }
    fn mutate(
        &self,
        owner: &str,
        id: &str,
        key: &str,
        input: &Value,
        expected: Option<u64>,
        operation: impl FnOnce(&mut TaskContract) -> Result<()>,
    ) -> Result<TaskContract> {
        self.mutate_audited(owner, id, key, input, expected, None, operation)
    }
    fn mutate_audited(
        &self,
        owner: &str,
        id: &str,
        key: &str,
        input: &Value,
        expected: Option<u64>,
        audit: Option<&Value>,
        operation: impl FnOnce(&mut TaskContract) -> Result<()>,
    ) -> Result<TaskContract> {
        if !valid_id(key) {
            return Err("Invalid idempotency key".into());
        }
        let fingerprint = digest(&serde_json::to_vec(input).map_err(|e| e.to_string())?);
        let mut db = self.db.lock();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let previous: Option<(String, String)> = tx
            .query_row(
                "SELECT digest,response FROM mutations WHERE owner=?1 AND task_id=?2 AND key=?3",
                params![owner, id, key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some((identity, response)) = previous {
            if identity != fingerprint {
                return Err("Idempotency key was reused with different input".into());
            }
            // Return current state, never a historical response that resurrects a cancelled task.
            let _ = response;
            return load(&tx, owner, id);
        }
        let count: u64 = tx
            .query_row(
                "SELECT COUNT(*) FROM mutations WHERE owner=?1 AND task_id=?2",
                params![owner, id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if count >= 4096 {
            return Err("Task mutation budget exceeded".into());
        }
        let mut task = load(&tx, owner, id)?;
        if expected.is_some_and(|revision| revision != task.revision) {
            return Err("Task revision conflict; reload before changing it".into());
        }
        operation(&mut task)?;
        task.revision += 1;
        task.updated_at = now();
        persist(&tx, &task)?;
        if let Some(audit) = audit {
            tx.execute("INSERT INTO acceptance_refreshes(owner,task_id,revision,key,created_at,passed,body) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![owner,id,task.revision,key,task.updated_at,task.completion_blockers().is_empty(),serde_json::to_string(audit).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        }
        tx.execute(
            "INSERT INTO mutations(owner,task_id,key,digest,response) VALUES(?1,?2,?3,?4,'')",
            params![owner, id, key, fingerprint],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(task)
    }
    pub fn prepare(&self, owner: &str, id: &str, step: Step) -> Result<TaskContract> {
        if !valid_id(&step.id)
            || !valid_id(&step.execution_id)
            || !valid_id(&step.idempotency_key)
            || step.input_identity.is_empty()
            || step.state != StepState::Prepared
        {
            return Err("Invalid prepared step".into());
        }
        let key = format!("prepare-{}", digest(step.id.as_bytes()));
        let input = serde_json::to_value(&step).map_err(|e| e.to_string())?;
        self.mutate(owner, id, &key, &input, None, |task| {
            if matches!(task.state, TaskState::Completed | TaskState::Cancelled) {
                return Err("Terminal task cannot dispatch more work".into());
            }
            if task.steps.len() >= MAX_STEPS {
                return Err("Task step budget exceeded".into());
            }
            if task
                .steps
                .iter()
                .any(|s| s.id == step.id || s.execution_id == step.execution_id)
            {
                return Err("Execution already recorded; do not replay it".into());
            }
            if task.steps.iter().any(|prior|prior.idempotency_key==step.idempotency_key) {
                return Err("Logical operation key is already recorded; query or reconcile the original execution".into());
            }
            task.steps.push(step);
            task.validation_identity = None;
            task.state = TaskState::Running;
            Ok(())
        })
    }
    pub fn dispatch(&self, owner: &str, id: &str, step_id: &str) -> Result<TaskContract> {
        let key = format!("dispatch-{}", digest(step_id.as_bytes()));
        self.mutate(
            owner,
            id,
            &key,
            &serde_json::json!({"dispatch":step_id}),
            None,
            |task| {
                if task.state == TaskState::Cancelled {
                    return Err("Task cancelled before dispatch".into());
                }
                let step = task
                    .steps
                    .iter_mut()
                    .find(|s| s.id == step_id)
                    .ok_or("Step not found")?;
                if step.state != StepState::Prepared {
                    return Err("Only a prepared intent may be dispatched".into());
                }
                step.state = StepState::Dispatched;
                step.updated_at = now();
                Ok(())
            },
        )
    }
    /// Runtime-owned notification; model-facing tools never accept a success boolean.
    pub fn observe(
        &self,
        owner: &str,
        id: &str,
        step_id: &str,
        key: &str,
        success: bool,
        running: bool,
        result: Option<Value>,
        evidence: Vec<String>,
    ) -> Result<TaskContract> {
        let input = serde_json::json!({"step":step_id,"success":success,"running":running,"result":result,"evidence":evidence});
        self.mutate(owner, id, key, &input, None, |task| {
            let cancelled = task.state == TaskState::Cancelled;
            let step = task
                .steps
                .iter_mut()
                .find(|s| s.id == step_id)
                .ok_or("Step not found")?;
            if matches!(
                step.state,
                StepState::Verified
                    | StepState::Committed
                    | StepState::Failed
                    | StepState::NotDispatched
            ) {
                return Err("A terminal execution result cannot be overwritten".into());
            }
            if step.state == StepState::Prepared {
                return Err("Undispatched execution cannot have a result".into());
            }
            step.updated_at = now();
            step.result_identity = result
                .as_ref()
                .map(|v| digest(&serde_json::to_vec(v).unwrap_or_default()));
            step.result = result.and_then(|mut value| {
                if serde_json::to_vec(&value).is_ok_and(|v| v.len() > 16_384)
                    && value["kind"] == "foreground"
                {
                    if let Some(object) = value.as_object_mut() {
                        object.remove("streams");
                        object.remove("steps");
                        if let Some(text) = object.get("stdout").and_then(Value::as_str) {
                            let tail: String = text
                                .chars()
                                .rev()
                                .take(2000)
                                .collect::<String>()
                                .chars()
                                .rev()
                                .collect();
                            object.insert("stdout".into(), Value::String(tail));
                            object.insert("journalOutputTruncated".into(), Value::Bool(true));
                        }
                    }
                }
                serde_json::to_vec(&value)
                    .is_ok_and(|v| v.len() <= 16_384)
                    .then_some(value)
            });
            step.evidence_refs = evidence;
            step.state = if running {
                StepState::Running
            } else if success {
                StepState::Verified
            } else if matches!(step.effect, EffectKind::ReadOnly) || step.known_process_exit() {
                StepState::Failed
            } else {
                StepState::Unknown
            };
            if !success {
                step.failure_reason =
                    Some("Execution failed; any partial effects require verification".into());
            }
            if !cancelled {
                task.state = if success {
                    TaskState::Running
                } else {
                    TaskState::ValidationFailed
                };
            }
            task.validation_identity = None;
            Ok(())
        })
    }
    /// Registry/adapter evidence that the requested operation was never dispatched.
    pub fn observe_not_dispatched(
        &self,
        owner: &str,
        id: &str,
        step_id: &str,
        key: &str,
        evidence: Vec<String>,
    ) -> Result<TaskContract> {
        self.mutate(
            owner,
            id,
            key,
            &serde_json::json!({"notDispatched":step_id,"evidence":evidence}),
            None,
            |task| {
                let step = task
                    .steps
                    .iter_mut()
                    .find(|step| step.id == step_id)
                    .ok_or("Step not found")?;
                if !matches!(step.state, StepState::Prepared | StepState::Dispatched) {
                    return Err("Execution may already have effects".into());
                }
                step.state = StepState::NotDispatched;
                step.updated_at = now();
                step.failure_reason = Some(
                    "Registry or adapter verified that the requested operation was not dispatched"
                        .into(),
                );
                step.evidence_refs = evidence;
                if task.state != TaskState::Cancelled {
                    task.state = TaskState::ValidationFailed;
                }
                task.validation_identity = None;
                Ok(())
            },
        )
    }

    /// A user-approved environment transition preserves requirements and effect history.
    pub fn migrate_environment_by_user(
        &self,
        owner: &str,
        id: &str,
        key: &str,
        revision: u64,
        fingerprint: &str,
    ) -> Result<TaskContract> {
        self.mutate(
            owner,
            id,
            key,
            &serde_json::json!({"environment":fingerprint,"revision":revision}),
            Some(revision),
            |task| {
                if fingerprint.is_empty()
                    || matches!(task.state, TaskState::Completed | TaskState::Cancelled)
                {
                    return Err("Only an active task may change environment".into());
                }
                if task
                    .steps
                    .iter()
                    .any(|step| matches!(step.state, StepState::Dispatched | StepState::Running))
                {
                    return Err(
                        "Wait for in-flight operations before migrating the environment".into(),
                    );
                }
                task.spec.environment_fingerprint = fingerprint.into();
                task.acceptance_results.clear();
                task.acceptance_refresh = None;
                task.validation_identity = None;
                task.output_identities.clear();
                task.validation_subject_evidence = None;
                task.state = TaskState::Blocked;
                Ok(())
            },
        )
    }

    /// Trusted reconciliation only after an adapter inspected the result identity/effects.
    /// It preserves the original operation key and never replays a command.
    pub fn reconcile(
        &self,
        owner: &str,
        id: &str,
        step_id: &str,
        key: &str,
        evidence_ref: &str,
        input_identity: &str,
    ) -> Result<TaskContract> {
        self.mutate(owner,id,key,&serde_json::json!({"reconcile":step_id,"evidence":evidence_ref,"identity":input_identity}),None,|task|{
            if evidence_ref.is_empty()||input_identity.is_empty() {return Err("Reconciliation requires observed effect evidence".into());}
            let step=task.steps.iter_mut().find(|s|s.id==step_id).ok_or("Step not found")?;
            if !matches!(step.state,StepState::Unknown|StepState::Failed|StepState::Running|StepState::EffectObserved) {return Err("Step does not require reconciliation".into());}
            step.state=StepState::Verified;step.result_identity=Some(input_identity.into());step.evidence_refs.push(evidence_ref.into());step.updated_at=now();
            task.validation_identity=None;Ok(())
        })
    }
    pub fn record_validation(
        &self,
        owner: &str,
        id: &str,
        key: &str,
        revision: u64,
        results: Vec<AcceptanceResult>,
        outputs: BTreeMap<String, String>,
    ) -> Result<TaskContract> {
        let input = serde_json::json!({"validation":results,"outputs":outputs});
        self.mutate(owner, id, key, &input, Some(revision), |task| {
            if matches!(task.state, TaskState::Cancelled | TaskState::Completed) {
                return Err("Terminal task cannot change validation".into());
            }
            if results.len() != task.spec.acceptance_checks.len()
                || task
                    .spec
                    .acceptance_checks
                    .iter()
                    .any(|c| results.iter().filter(|r| r.check_id == c.id).count() != 1)
            {
                return Err(
                    "Acceptance results must cover every immutable check exactly once".into(),
                );
            }
            let failed = results.iter().any(|r| {
                matches!(
                    r.status,
                    AcceptanceStatus::Failed | AcceptanceStatus::Unverified
                )
            });
            if task.spec.acceptance_checks.iter().any(|check| {
                results.iter().any(|result| {
                    result.check_id == check.id && !check.checker.evidence_current(result)
                })
            }) {
                return Err(
                    "Machine acceptance results must use the current checker version".into(),
                );
            }
            let pending = results
                .iter()
                .any(|r| r.status == AcceptanceStatus::AwaitingUser);
            task.state = if failed {
                TaskState::ValidationFailed
            } else if pending {
                TaskState::AwaitingUser
            } else {
                TaskState::Validating
            };
            task.validation_identity = Some(digest(
                &serde_json::to_vec(&input).map_err(|e| e.to_string())?,
            ));
            task.acceptance_results = results;
            task.acceptance_refresh = None;
            task.output_identities = outputs;
            Ok(())
        })
    }
    pub fn complete(
        &self,
        owner: &str,
        id: &str,
        key: &str,
        revision: u64,
        current_outputs: &BTreeMap<String, String>,
    ) -> Result<TaskContract> {
        self.mutate(
            owner,
            id,
            key,
            &serde_json::json!({"complete":current_outputs}),
            Some(revision),
            |task| {
                let blockers = task.completion_blockers();
                if !blockers.is_empty() {
                    return Err(blockers.join("; "));
                }
                if &task.output_identities != current_outputs {
                    return Err(
                        "Acceptance inputs changed; rerun validation before completion".into(),
                    );
                }
                task.state = TaskState::Completed;
                Ok(())
            },
        )
    }
    /// Append a current-checker receipt without replacing the historical
    /// completion, immutable contract, output hashes, or human approval.
    pub fn refresh_evidence_by_user(
        &self,
        owner: &str,
        id: &str,
        key: &str,
        revision: u64,
        results: Vec<AcceptanceResult>,
        outputs: BTreeMap<String, String>,
    ) -> Result<TaskContract> {
        let input = serde_json::json!({"refreshEvidence":results,"outputs":outputs});
        self.mutate_audited(owner,id,key,&input,Some(revision),Some(&input),|task| {
            if task.state != TaskState::Completed { return Err("Evidence refresh requires a completed task".into()); }
            if task.output_identities != outputs { return Err("Completed inputs changed; create a new validation task instead of reusing prior consent".into()); }
            if results.len() != task.spec.acceptance_checks.len() { return Err("Refresh must cover every immutable check".into()); }
            for check in &task.spec.acceptance_checks {
                let matches = results.iter().filter(|result| result.check_id == check.id).collect::<Vec<_>>();
                if matches.len() != 1 { return Err("Refresh must cover every immutable check exactly once".into()); }
                let result = matches[0];
                if matches!(check.checker, Checker::Manual { .. }) {
                    if !task.acceptance_results.iter().any(|original| original == result && original.status == AcceptanceStatus::Passed) {
                        return Err("Evidence refresh cannot change existing human confirmation".into());
                    }
                } else if !check.checker.evidence_current(result) || !matches!(result.status, AcceptanceStatus::Passed | AcceptanceStatus::Failed) {
                    return Err("Refresh needs current machine checker results".into());
                }
            }
            let (completed_revision, completed_at) = task.acceptance_refresh.as_ref()
                .map(|refresh| (refresh.completed_revision, refresh.completed_at)).unwrap_or((task.revision, task.updated_at));
            task.acceptance_refresh = Some(AcceptanceRefresh {
                completed_revision, completed_at, refreshed_at:now(),
                input_identity:digest(&serde_json::to_vec(&outputs).map_err(|e|e.to_string())?), results,
            });
            Ok(())
        })
    }

    pub fn evidence_refresh_history(&self, owner: &str, id: &str) -> Result<Vec<Value>> {
        let db = self.db.lock();
        load(&db, owner, id)?;
        let mut query = db.prepare("SELECT revision,key,created_at,passed FROM acceptance_refreshes WHERE owner=?1 AND task_id=?2 ORDER BY revision DESC LIMIT 64").map_err(|e|e.to_string())?;
        query.query_map(params![owner,id],|row| Ok(serde_json::json!({"revision":row.get::<_,u64>(0)?,"idempotencyKey":row.get::<_,String>(1)?,"recordedAt":row.get::<_,u64>(2)?,"passed":row.get::<_,bool>(3)?})))
            .map_err(|e|e.to_string())?.collect::<std::result::Result<Vec<_>,_>>().map_err(|e|e.to_string())
    }

    pub fn cancel(&self, owner: &str, id: &str, key: &str) -> Result<TaskContract> {
        self.mutate(
            owner,
            id,
            key,
            &serde_json::json!({"cancel":true}),
            None,
            |task| {
                if task.state == TaskState::Completed {
                    return Err("Completed task cannot be cancelled".into());
                }
                task.state = TaskState::Cancelled;
                Ok(())
            },
        )
    }
    /// Host-only evidence derived from an actual successful skill_candidate read.
    pub fn mark_subject_loaded(
        &self,
        owner: &str,
        id: &str,
        execution_id: &str,
        content_hash: &str,
    ) -> Result<TaskContract> {
        self.mutate(
            owner,
            id,
            &format!("subject-{}", digest(execution_id.as_bytes())),
            &serde_json::json!({"subjectLoaded":content_hash,"executionId":execution_id}),
            None,
            |task| {
                if matches!(task.state, TaskState::Cancelled | TaskState::Completed) {
                    return Err("Terminal task cannot load a new validation subject".into());
                }
                let subject = task
                    .spec
                    .validation_subject
                    .as_ref()
                    .ok_or("Task does not declare a validation subject")?;
                if subject.kind != "skill" || subject.identity != content_hash {
                    return Err("Loaded skill differs from the declared validation subject".into());
                }
                let step = task
                    .steps
                    .iter()
                    .find(|step| step.execution_id == execution_id)
                    .ok_or("Skill read was not recorded")?;
                if step.tool != "skill_candidate" || step.state != StepState::Verified {
                    return Err("Skill load requires a successful recorded read".into());
                }
                task.validation_subject_evidence = Some(SubjectEvidence {
                    identity: content_hash.into(),
                    execution_id: execution_id.into(),
                    loaded_at_revision: task.revision + 1,
                });
                task.validation_identity = None;
                Ok(())
            },
        )
    }
    /// Exposed only through the user control API; model callbacks cannot resume cancellation.
    pub fn resume_by_user(
        &self,
        owner: &str,
        id: &str,
        key: &str,
        revision: u64,
    ) -> Result<TaskContract> {
        self.mutate(
            owner,
            id,
            key,
            &serde_json::json!({"resumeByUser":true}),
            Some(revision),
            |task| {
                if task.state != TaskState::Cancelled {
                    return Err("Only a cancelled task needs explicit continuation".into());
                }
                task.state = TaskState::Blocked;
                task.validation_identity = None;
                Ok(())
            },
        )
    }
    pub fn confirm_manual_by_user(
        &self,
        owner: &str,
        id: &str,
        key: &str,
        revision: u64,
        check_id: &str,
        input_identity: &str,
    ) -> Result<TaskContract> {
        self.mutate(
            owner,
            id,
            key,
            &serde_json::json!({"manual":check_id,"input":input_identity}),
            Some(revision),
            |task| {
                if matches!(task.state, TaskState::Cancelled | TaskState::Completed) {
                    return Err("Task is terminal".into());
                }
                if !task
                    .spec
                    .acceptance_checks
                    .iter()
                    .any(|c| c.id == check_id && matches!(c.checker, Checker::Manual { .. }))
                {
                    return Err("Not a manual acceptance requirement".into());
                }
                let result = task
                    .acceptance_results
                    .iter_mut()
                    .find(|r| r.check_id == check_id)
                    .ok_or("Validate current inputs before confirmation")?;
                if result.input_identity != input_identity {
                    return Err("Manual acceptance input changed".into());
                }
                result.status = AcceptanceStatus::Passed;
                result
                    .evidence_refs
                    .push(format!("user-confirmation:{key}"));
                task.state = TaskState::Validating;
                Ok(())
            },
        )
    }
    fn recover_after_restart(&self) -> Result<()> {
        let mut db = self.db.lock();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let rows = {
            let mut stmt = tx
                .prepare("SELECT owner,task_id FROM tasks")
                .map_err(|e| e.to_string())?;
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(|e| e.to_string())?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        for (owner, id) in rows {
            let mut task = load(&tx, &owner, &id)?;
            let mut changed = false;
            for step in &mut task.steps {
                if matches!(
                    step.state,
                    StepState::Dispatched | StepState::Running | StepState::EffectObserved
                ) {
                    step.state = StepState::Unknown;
                    step.failure_reason=Some("Host restarted before a durable terminal result; inspect effects before retry".into());
                    step.updated_at = now();
                    changed = true;
                }
            }
            if changed {
                if task.state != TaskState::Cancelled {
                    task.state = TaskState::Blocked;
                }
                task.validation_identity = None;
                task.revision += 1;
                task.updated_at = now();
                persist(&tx, &task)?;
            }
        }
        tx.commit().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests;
