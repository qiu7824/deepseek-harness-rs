use super::*;

type RevisionResult<T> = std::result::Result<T, RevisionError>;
fn fingerprint(spec: &ContractSpec, expected: u64, mode: RevisionMode) -> RevisionResult<String> {
    let mut contract = serde_json::to_value(spec).map_err(|e| e.to_string())?;
    // Environment is a server observation, not part of the user's retry payload.
    contract
        .as_object_mut()
        .expect("ContractSpec object")
        .remove("environmentFingerprint");
    Ok(digest(
        &serde_json::to_vec(
            &serde_json::json!({"reviseByUser":contract,"expectedRevision":expected,"mode":mode}),
        )
        .map_err(|e| e.to_string())?,
    ))
}
fn replay(
    db: &Connection,
    owner: &str,
    id: &str,
    key: &str,
    fingerprint: &str,
) -> RevisionResult<Option<TaskContract>> {
    let previous: Option<String> = db
        .query_row(
            "SELECT digest FROM mutations WHERE owner=?1 AND task_id=?2 AND key=?3",
            params![owner, id, key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(previous) = previous {
        if previous != fingerprint {
            return Err(RevisionError::IdempotencyConflict);
        }
        let target: String = db.query_row("SELECT target_task_id FROM contract_revisions WHERE owner=?1 AND task_id=?2 AND key=?3",params![owner,id,key],|row|row.get(0)).map_err(|e|e.to_string())?;
        return Ok(Some(load(db, owner, &target)?));
    }
    Ok(None)
}

impl TaskRuntime {
    /// A retry of an already committed user edit performs no new work.
    pub fn replay_revision(
        &self,
        owner: &str,
        id: &str,
        key: &str,
        expected: u64,
        spec: &ContractSpec,
        mode: RevisionMode,
    ) -> RevisionResult<Option<TaskContract>> {
        if !valid_id(key) {
            return Err(RevisionError::InvalidContract(
                "Invalid idempotency key".into(),
            ));
        }
        let recorded = replay(
            &self.db.lock(),
            owner,
            id,
            key,
            &fingerprint(spec, expected, mode)?,
        )?;
        if recorded.is_none() {
            validate(spec).map_err(RevisionError::InvalidContract)?;
        }
        Ok(recorded)
    }

    /// Caller holds the user-control admission lease and excludes validation work.
    /// Source snapshots and effects remain available even after acceptance is invalidated.
    pub fn revise_by_user(
        &self,
        owner: &str,
        id: &str,
        key: &str,
        expected: u64,
        spec: ContractSpec,
        mode: RevisionMode,
    ) -> RevisionResult<TaskContract> {
        if !valid_id(key) {
            return Err(RevisionError::InvalidContract(
                "Invalid idempotency key".into(),
            ));
        }
        let fingerprint = fingerprint(&spec, expected, mode)?;
        let mut db = self.db.lock();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        if let Some(task) = replay(&tx, owner, id, key, &fingerprint)? {
            return Ok(task);
        }
        validate(&spec).map_err(RevisionError::InvalidContract)?;
        let count: u64 = tx
            .query_row(
                "SELECT COUNT(*) FROM mutations WHERE owner=?1 AND task_id=?2",
                params![owner, id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if count >= 4096 {
            return Err(RevisionError::InvalidContract(
                "Task mutation budget exceeded".into(),
            ));
        }
        let mut task = load(&tx, owner, id)?;
        if task.revision != expected {
            return Err(RevisionError::Conflict);
        }
        if mode == RevisionMode::InPlace
            && task.spec.environment_fingerprint != spec.environment_fingerprint
        {
            return Err(RevisionError::EnvironmentChanged);
        }
        if task.steps.iter().any(|step| {
            matches!(
                step.state,
                StepState::Dispatched | StepState::Running | StepState::EffectObserved
            )
        }) {
            return Err(RevisionError::Busy);
        }
        let terminal = matches!(task.state, TaskState::Completed | TaskState::Cancelled);
        if mode == RevisionMode::Successor && !terminal {
            return Err(RevisionError::InvalidMode);
        }
        if mode == RevisionMode::InPlace && task.state == TaskState::Completed {
            return Err(RevisionError::SuccessorRequired);
        }
        let prior = serde_json::to_string(&task).map_err(|e| e.to_string())?;
        let changed = task.spec != spec;
        if mode == RevisionMode::Successor {
            let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM tasks WHERE owner=?1 AND state NOT IN ('Completed','Cancelled'))",params![owner],|row|row.get(0)).map_err(|e|e.to_string())?;
            if active {
                return Err(RevisionError::ActiveContract);
            }
            task.task_id = format!(
                "task-{}",
                &digest(
                    &serde_json::to_vec(&serde_json::json!([owner, id, key]))
                        .map_err(|e| e.to_string())?
                )[..32]
            );
            if tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM tasks WHERE owner=?1 AND task_id=?2)",
                    params![owner, task.task_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|e| e.to_string())?
            {
                return Err(RevisionError::InvalidContract(
                    "Successor identity is already in use".into(),
                ));
            }
            task.based_on = Some(TaskRevisionOrigin {
                task_id: id.into(),
                revision: expected,
            });
            task.revision = 1;
            task.requirements_revision = 1;
            task.created_at = now();
            task.state = TaskState::Planned;
        } else {
            task.revision += 1;
            if changed {
                task.requirements_revision += 1;
            }
        }
        if changed || mode == RevisionMode::Successor {
            task.spec = spec;
            task.acceptance_results.clear();
            task.acceptance_refresh = None;
            task.validation_identity = None;
            task.output_identities.clear();
            task.validation_subject_evidence = None;
            // Never dispatch an intent prepared against superseded requirements.
            // Its exact prior state remains in the immutable revision snapshot.
            for step in &mut task.steps {
                if step.state == StepState::Prepared {
                    step.state = StepState::NotDispatched;
                    step.updated_at = now();
                    step.failure_reason = Some("Requirements revised before dispatch".into());
                    step.evidence_refs
                        .push(format!("contract-revision:{id}:{expected}"));
                }
            }
            if task.state != TaskState::Cancelled {
                task.state = TaskState::Planned;
            }
        }
        task.updated_at = now();
        persist(&tx, &task)?;
        tx.execute("INSERT INTO contract_revisions(owner,task_id,key,source_revision,target_task_id,target_revision,created_at,prior_body) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![owner,id,key,expected,task.task_id,task.revision,task.updated_at,prior]).map_err(|e|e.to_string())?;
        tx.execute(
            "INSERT INTO mutations(owner,task_id,key,digest,response) VALUES(?1,?2,?3,?4,'')",
            params![owner, id, key, fingerprint],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(task)
    }

    pub fn requirements_history(&self, owner: &str, id: &str) -> Result<Vec<Value>> {
        let db = self.db.lock();
        load(&db, owner, id)?;
        let mut query = db.prepare("SELECT key,source_revision,target_task_id,target_revision,created_at,task_id FROM contract_revisions WHERE owner=?1 AND (task_id=?2 OR target_task_id=?2) ORDER BY rowid DESC LIMIT 64").map_err(|e|e.to_string())?;
        query.query_map(params![owner,id],|row|Ok(serde_json::json!({"idempotencyKey":row.get::<_,String>(0)?,"sourceRevision":row.get::<_,u64>(1)?,"targetTaskId":row.get::<_,String>(2)?,"targetRevision":row.get::<_,u64>(3)?,"createdAt":row.get::<_,u64>(4)?,"sourceTaskId":row.get::<_,String>(5)?})))
            .map_err(|e|e.to_string())?.collect::<std::result::Result<Vec<_>,_>>().map_err(|e|e.to_string())
    }

    pub fn requirements_snapshot(&self, owner: &str, id: &str, key: &str) -> Result<TaskContract> {
        let db = self.db.lock();
        load(&db, owner, id)?;
        let body: String = db.query_row("SELECT prior_body FROM contract_revisions WHERE owner=?1 AND task_id=?2 AND key=?3", params![owner,id,key],|row|row.get(0)).map_err(|e|e.to_string())?;
        serde_json::from_str(&body).map_err(|e| e.to_string())
    }
}
