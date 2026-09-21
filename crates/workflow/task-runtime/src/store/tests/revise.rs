use super::*;

fn changed(task: &TaskContract) -> ContractSpec {
    let mut spec = task.spec.clone();
    spec.objective = "Deliver the revised answer".into();
    spec.constraints.push("Keep the previous audit".into());
    spec
}

#[test]
fn user_revision_invalidates_machine_and_manual_acceptance_but_preserves_audit() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    let mut definition = spec();
    definition.acceptance_checks.push(AcceptanceCheck {
        id: "human".into(),
        description: "Visual check".into(),
        checker: Checker::Manual {
            reason: "Inspect layout".into(),
        },
    });
    let created = runtime.create("owner", "task", definition).unwrap();
    let outputs = BTreeMap::from([("answer.json".into(), digest(br#"{"answer":42}"#))]);
    let identity = digest(&serde_json::to_vec(&outputs).unwrap());
    let proof = vec![
        check_bytes(&created.spec.acceptance_checks[0], br#"{"answer":42}"#),
        AcceptanceResult {
            check_id: "human".into(),
            checker_version: CHECKER_VERSION.into(),
            input_identity: identity,
            status: AcceptanceStatus::Passed,
            evidence_refs: vec!["user:accepted".into()],
            coverage: "Visual inspection".into(),
            failure_reason: None,
        },
    ];
    let before = runtime
        .record_validation(
            "owner",
            "task",
            "proof",
            created.revision,
            proof.clone(),
            outputs.clone(),
        )
        .unwrap();
    let after = runtime
        .revise_by_user(
            "owner",
            "task",
            "edit",
            before.revision,
            changed(&before),
            RevisionMode::InPlace,
        )
        .unwrap();
    assert_eq!(after.requirements_revision, 2);
    assert_eq!(after.revision, before.revision + 1);
    assert_eq!(after.state, TaskState::Planned);
    assert!(after.acceptance_results.is_empty() && after.acceptance_refresh.is_none());
    assert!(after.output_identities.is_empty() && after.validation_identity.is_none());
    assert!(after.validation_subject_evidence.is_none());
    assert!(!after.completion_blockers().is_empty());
    assert_eq!(
        runtime
            .requirements_snapshot("owner", "task", "edit")
            .unwrap(),
        before
    );
    assert!(
        runtime
            .record_validation(
                "owner",
                "task",
                "late-proof",
                before.revision,
                proof,
                outputs
            )
            .is_err()
    );
    assert!(
        runtime.create("owner", "task", created.spec).is_err(),
        "agent create cannot revert a user edit"
    );
    assert_eq!(runtime.get("owner", "task").unwrap(), after);
    assert!(
        runtime
            .requirements_snapshot("other", "task", "edit")
            .is_err()
    );
}

#[test]
fn user_revision_cas_and_idempotency_are_stable_across_restart_and_later_cancellation() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    let before = runtime.create("owner", "task", spec()).unwrap();
    let revised = changed(&before);
    let after = runtime
        .revise_by_user(
            "owner",
            "task",
            "edit",
            before.revision,
            revised.clone(),
            RevisionMode::InPlace,
        )
        .unwrap();
    assert_eq!(
        runtime
            .revise_by_user(
                "owner",
                "task",
                "edit",
                before.revision,
                revised.clone(),
                RevisionMode::InPlace
            )
            .unwrap(),
        after
    );
    assert_eq!(
        runtime
            .revise_by_user(
                "owner",
                "task",
                "stale",
                before.revision,
                revised.clone(),
                RevisionMode::InPlace
            )
            .unwrap_err(),
        RevisionError::Conflict
    );
    assert_eq!(
        runtime
            .revise_by_user(
                "owner",
                "task",
                "edit",
                before.revision,
                before.spec.clone(),
                RevisionMode::InPlace
            )
            .unwrap_err(),
        RevisionError::IdempotencyConflict
    );
    let mut invalid_reuse = revised.clone();
    invalid_reuse.acceptance_checks.clear();
    assert_eq!(
        runtime
            .replay_revision(
                "owner",
                "task",
                "edit",
                before.revision,
                &invalid_reuse,
                RevisionMode::InPlace
            )
            .unwrap_err(),
        RevisionError::IdempotencyConflict
    );
    assert_eq!(
        runtime
            .revise_by_user(
                "owner",
                "task",
                "edit",
                before.revision,
                invalid_reuse,
                RevisionMode::InPlace
            )
            .unwrap_err(),
        RevisionError::IdempotencyConflict
    );
    let cancelled = runtime.cancel("owner", "task", "cancel").unwrap();
    drop(runtime);
    let reopened = fixture.open();
    let mut observed_elsewhere = revised.clone();
    observed_elsewhere.environment_fingerprint = "new-server-observation".into();
    assert_eq!(
        reopened
            .replay_revision(
                "owner",
                "task",
                "edit",
                before.revision,
                &observed_elsewhere,
                RevisionMode::InPlace
            )
            .unwrap(),
        Some(cancelled)
    );
    assert_eq!(
        reopened
            .requirements_history("owner", "task")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        reopened
            .requirements_snapshot("owner", "task", "edit")
            .unwrap(),
        before
    );
}

#[test]
fn user_revision_retains_unknown_effects_and_cancelled_state_without_replaying_prepared_intents() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    runtime.create("owner", "task", spec()).unwrap();
    runtime.prepare("owner", "task", step("write")).unwrap();
    runtime.dispatch("owner", "task", "write").unwrap();
    runtime
        .observe(
            "owner",
            "task",
            "write",
            "failure",
            false,
            false,
            None,
            vec![],
        )
        .unwrap();
    runtime.prepare("owner", "task", step("pending")).unwrap();
    let before = runtime.cancel("owner", "task", "cancel").unwrap();
    let after = runtime
        .revise_by_user(
            "owner",
            "task",
            "edit",
            before.revision,
            changed(&before),
            RevisionMode::InPlace,
        )
        .unwrap();
    assert_eq!(after.state, TaskState::Cancelled);
    assert_eq!(after.steps[0], before.steps[0]);
    assert_eq!(after.steps[0].state, StepState::Unknown);
    assert_eq!(after.steps[1].state, StepState::NotDispatched);
    assert!(runtime.dispatch("owner", "task", "pending").is_err());
    let resumed = runtime
        .resume_by_user("owner", "task", "resume", after.revision)
        .unwrap();
    assert!(runtime.dispatch("owner", "task", "pending").is_err());
    assert!(
        resumed
            .completion_blockers()
            .iter()
            .any(|value| value.contains("Unknown"))
    );
    assert_eq!(
        runtime
            .requirements_snapshot("owner", "task", "edit")
            .unwrap(),
        before
    );
}

#[test]
fn completed_user_revision_requires_explicit_successor_and_preserves_completed_record() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    runtime.create("owner", "task", spec()).unwrap();
    let proof = validate_answer(&runtime, br#"{"answer":42}"#);
    let completed = runtime
        .complete(
            "owner",
            "task",
            "done",
            proof.revision,
            &proof.output_identities,
        )
        .unwrap();
    assert_eq!(
        runtime
            .revise_by_user(
                "owner",
                "task",
                "edit",
                completed.revision,
                changed(&completed),
                RevisionMode::InPlace
            )
            .unwrap_err(),
        RevisionError::SuccessorRequired
    );
    let mut next_spec = changed(&completed);
    next_spec.environment_fingerprint = "current-host-after-upgrade".into();
    let successor = runtime
        .revise_by_user(
            "owner",
            "task",
            "next",
            completed.revision,
            next_spec,
            RevisionMode::Successor,
        )
        .unwrap();
    assert_eq!(
        successor.spec.environment_fingerprint,
        "current-host-after-upgrade"
    );
    assert_ne!(successor.task_id, completed.task_id);
    assert_eq!(
        successor.based_on,
        Some(TaskRevisionOrigin {
            task_id: "task".into(),
            revision: completed.revision
        })
    );
    assert_eq!(successor.revision, 1);
    assert_eq!(successor.requirements_revision, 1);
    assert_eq!(successor.state, TaskState::Planned);
    assert!(successor.acceptance_results.is_empty());
    assert_eq!(runtime.get("owner", "task").unwrap(), completed);
    assert_eq!(
        runtime
            .requirements_snapshot("owner", "task", "next")
            .unwrap(),
        completed
    );
    let retry = runtime
        .revise_by_user(
            "owner",
            "task",
            "next",
            completed.revision,
            changed(&completed),
            RevisionMode::Successor,
        )
        .unwrap();
    assert_eq!(retry, successor);
    let incoming = runtime
        .requirements_history("owner", &successor.task_id)
        .unwrap();
    assert_eq!(incoming.len(), 1);
    assert_eq!(incoming[0]["sourceTaskId"], "task");
    assert_eq!(incoming[0]["targetTaskId"], successor.task_id);
    assert_eq!(
        runtime
            .requirements_snapshot(
                "owner",
                incoming[0]["sourceTaskId"].as_str().unwrap(),
                incoming[0]["idempotencyKey"].as_str().unwrap()
            )
            .unwrap(),
        completed
    );
    assert_eq!(
        runtime
            .revise_by_user(
                "owner",
                "task",
                "another",
                completed.revision,
                changed(&completed),
                RevisionMode::Successor
            )
            .unwrap_err(),
        RevisionError::ActiveContract
    );
    assert_eq!(runtime.list("owner").unwrap().len(), 2);
}

#[test]
fn cancelled_successor_keeps_unknown_effects_in_the_active_contract() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    runtime.create("owner", "task", spec()).unwrap();
    runtime.prepare("owner", "task", step("write")).unwrap();
    runtime.dispatch("owner", "task", "write").unwrap();
    runtime
        .observe(
            "owner",
            "task",
            "write",
            "failed",
            false,
            false,
            None,
            vec![],
        )
        .unwrap();
    let before = runtime.cancel("owner", "task", "cancel").unwrap();
    let next = runtime
        .revise_by_user(
            "owner",
            "task",
            "next",
            before.revision,
            changed(&before),
            RevisionMode::Successor,
        )
        .unwrap();
    assert_eq!(next.steps, before.steps);
    assert_eq!(next.state, TaskState::Planned);
    assert!(
        next.completion_blockers()
            .iter()
            .any(|value| value.contains("Unknown"))
    );
    assert_eq!(runtime.get("owner", "task").unwrap(), before);
}

#[test]
fn no_op_edit_keeps_acceptance_and_only_actual_requirement_changes_update_requirements_revision() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    runtime.create("owner", "task", spec()).unwrap();
    let before = validate_answer(&runtime, br#"{"answer":42}"#);
    let after = runtime
        .revise_by_user(
            "owner",
            "task",
            "same",
            before.revision,
            before.spec.clone(),
            RevisionMode::InPlace,
        )
        .unwrap();
    assert_eq!(after.requirements_revision, 1);
    assert_eq!(after.acceptance_results, before.acceptance_results);
    assert_eq!(after.validation_identity, before.validation_identity);
    assert_eq!(after.state, before.state);
    let mut raw = serde_json::to_value(&before).unwrap();
    raw.as_object_mut().unwrap().remove("requirementsRevision");
    assert_eq!(
        serde_json::from_value::<TaskContract>(raw)
            .unwrap()
            .requirements_revision,
        1
    );
}

#[test]
fn invalid_busy_and_environment_edits_do_not_append_history_or_change_current_contract() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    let created = runtime.create("owner", "task", spec()).unwrap();
    let mut invalid = changed(&created);
    invalid.acceptance_checks.clear();
    assert!(matches!(
        runtime.revise_by_user(
            "owner",
            "task",
            "bad",
            created.revision,
            invalid,
            RevisionMode::InPlace
        ),
        Err(RevisionError::InvalidContract(_))
    ));
    let mut migrated = changed(&created);
    migrated.environment_fingerprint = "elsewhere".into();
    assert_eq!(
        runtime
            .revise_by_user(
                "owner",
                "task",
                "environment",
                created.revision,
                migrated,
                RevisionMode::InPlace
            )
            .unwrap_err(),
        RevisionError::EnvironmentChanged
    );
    assert_eq!(
        runtime
            .revise_by_user(
                "owner",
                "task",
                "next",
                created.revision,
                changed(&created),
                RevisionMode::Successor
            )
            .unwrap_err(),
        RevisionError::InvalidMode
    );
    runtime.prepare("owner", "task", step("write")).unwrap();
    let busy = runtime.dispatch("owner", "task", "write").unwrap();
    assert_eq!(
        runtime
            .revise_by_user(
                "owner",
                "task",
                "busy",
                busy.revision,
                changed(&busy),
                RevisionMode::InPlace
            )
            .unwrap_err(),
        RevisionError::Busy
    );
    assert_eq!(runtime.get("owner", "task").unwrap(), busy);
    assert!(
        runtime
            .requirements_history("owner", "task")
            .unwrap()
            .is_empty()
    );
}
