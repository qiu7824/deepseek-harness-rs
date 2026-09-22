use super::*;
#[path = "tests/goal_binding.rs"]
mod goal_binding;
#[path = "tests/migration.rs"]
mod migration;
#[path = "tests/revise.rs"]
mod revise;

#[test]
fn failed_readonly_search_does_not_poison_fresh_content_acceptance() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    runtime.create("owner", "task", spec()).unwrap();
    let mut search = step("search");
    search.tool = "grep".into();
    search.effect = EffectKind::ReadOnly;
    runtime.prepare("owner", "task", search).unwrap();
    runtime.dispatch("owner", "task", "search").unwrap();
    runtime
        .observe(
            "owner",
            "task",
            "search",
            "failed-search",
            false,
            false,
            Some(serde_json::json!({"error":"missing executable"})),
            vec![],
        )
        .unwrap();
    let task = validate_answer(&runtime, br#"{"answer":42}"#);
    assert_eq!(task.steps[0].state, StepState::Failed);
    assert!(
        task.completion_blockers().is_empty(),
        "{:?}",
        task.completion_blockers()
    );
    runtime
        .complete(
            "owner",
            "task",
            "done",
            task.revision,
            &task.output_identities,
        )
        .unwrap();
}

#[test]
fn undispatched_write_is_failed_and_migration_preserves_unknown_effects() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    runtime.create("owner", "task", spec()).unwrap();
    runtime.prepare("owner", "task", step("s1")).unwrap();
    runtime.dispatch("owner", "task", "s1").unwrap();
    let task = runtime
        .observe_not_dispatched(
            "owner",
            "task",
            "s1",
            "denied",
            vec!["registry:no-body".into()],
        )
        .unwrap();
    assert_eq!(task.steps[0].state, StepState::NotDispatched);
    runtime.prepare("owner", "task", step("s2")).unwrap();
    runtime.dispatch("owner", "task", "s2").unwrap();
    let task = runtime.get("owner", "task").unwrap();
    assert!(
        runtime
            .migrate_environment_by_user("owner", "task", "moving", task.revision, "host2")
            .is_err()
    );
    runtime
        .observe(
            "owner",
            "task",
            "s2",
            "uncertain",
            false,
            false,
            None,
            vec![],
        )
        .unwrap();
    let before = validate_answer(&runtime, br#"{"answer":42}"#);
    let after = runtime
        .migrate_environment_by_user("owner", "task", "migrate", before.revision, "host2")
        .unwrap();
    assert_eq!(after.steps, before.steps);
    assert_eq!(after.steps[1].state, StepState::Unknown);
    assert_eq!(after.spec.acceptance_checks, before.spec.acceptance_checks);
    assert_eq!(after.spec.environment_fingerprint, "host2");
    assert!(after.acceptance_results.is_empty());
    assert!(after.validation_identity.is_none());
    assert!(!after.completion_blockers().is_empty());
    assert!(
        runtime
            .migrate_environment_by_user("owner", "task", "stale", before.revision, "host3")
            .is_err()
    );
}

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("dsh-task-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn open(&self) -> TaskRuntime {
        TaskRuntime::open(&self.0.join("state.sqlite")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn spec() -> ContractSpec {
    ContractSpec {
        objective: "Deliver a correct answer".into(),
        goal_id: None,
        constraints: vec![],
        expected_outputs: vec!["answer.json".into()],
        acceptance_checks: vec![AcceptanceCheck {
            id: "answer".into(),
            description: "Answer is correct".into(),
            checker: Checker::Json {
                path: "answer.json".into(),
                assertions: BTreeMap::from([("/answer".into(), Value::from(42))]),
            },
        }],
        environment_fingerprint: "host1:policy2".into(),
        validation_subject: None,
    }
}
fn step(id: &str) -> Step {
    Step {
        id: id.into(),
        execution_id: id.into(),
        idempotency_key: format!("op-{id}"),
        input_identity: "input-sha".into(),
        tool: "pwsh".into(),
        effect: EffectKind::Write,
        state: StepState::Prepared,
        updated_at: 0,
        process: None,
        result_identity: None,
        result: None,
        evidence_refs: vec![],
        failure_reason: None,
    }
}
fn validate_answer(runtime: &TaskRuntime, bytes: &[u8]) -> TaskContract {
    let task = runtime.get("owner", "task").unwrap();
    let results = vec![check_bytes(&task.spec.acceptance_checks[0], bytes)];
    runtime
        .record_validation(
            "owner",
            "task",
            &format!("validate-{}", task.revision),
            task.revision,
            results,
            BTreeMap::from([("answer.json".into(), digest(bytes))]),
        )
        .unwrap()
}

#[test]
fn completed_evidence_refresh_is_audited_and_keeps_human_consent_and_history() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    let mut definition = spec();
    definition.expected_outputs = vec!["out.docx".into()];
    definition.acceptance_checks = vec![
        AcceptanceCheck {
            id: "office".into(),
            description: "Office".into(),
            checker: Checker::OfficePackage {
                path: "out.docx".into(),
                format: "docx".into(),
            },
        },
        AcceptanceCheck {
            id: "manual".into(),
            description: "Layout".into(),
            checker: Checker::Manual {
                reason: "Inspect rendered pages".into(),
            },
        },
    ];
    let created = runtime.create("owner", "task", definition).unwrap();
    let bytes = office::feature_docx().unwrap();
    let outputs = BTreeMap::from([("out.docx".into(), digest(&bytes))]);
    let manual = AcceptanceResult {
        check_id: "manual".into(),
        checker_version: CHECKER_VERSION.into(),
        input_identity: digest(&serde_json::to_vec(&outputs).unwrap()),
        status: AcceptanceStatus::Passed,
        evidence_refs: vec!["user-confirmation:original".into()],
        coverage: "User inspected the pages".into(),
        failure_reason: None,
    };
    let machine = check_bytes(&created.spec.acceptance_checks[0], &bytes);
    let validated = runtime
        .record_validation(
            "owner",
            "task",
            "initial",
            created.revision,
            vec![machine.clone(), manual.clone()],
            outputs.clone(),
        )
        .unwrap();
    let mut historical = runtime
        .complete("owner", "task", "complete", validated.revision, &outputs)
        .unwrap();
    // A completed record written by the previous release, before current gates.
    historical.acceptance_results[0].checker_version = CHECKER_VERSION.into();
    persist(&runtime.db.lock(), &historical).unwrap();
    assert!(
        historical
            .completion_blockers()
            .iter()
            .any(|reason| reason.contains("obsolete"))
    );
    let refreshed = runtime
        .refresh_evidence_by_user(
            "owner",
            "task",
            "refresh-1",
            historical.revision,
            vec![machine.clone(), manual.clone()],
            outputs.clone(),
        )
        .unwrap();
    assert!(refreshed.completion_blockers().is_empty());
    assert_eq!(refreshed.state, TaskState::Completed);
    assert_eq!(refreshed.spec, historical.spec);
    assert_eq!(refreshed.acceptance_results, historical.acceptance_results);
    assert_eq!(refreshed.current_acceptance_results()[1], manual);
    assert_eq!(
        refreshed
            .acceptance_refresh
            .as_ref()
            .unwrap()
            .completed_revision,
        historical.revision
    );
    assert_eq!(
        refreshed.acceptance_refresh.as_ref().unwrap().completed_at,
        historical.updated_at
    );
    let mut failed = machine.clone();
    failed.status = AcceptanceStatus::Failed;
    failed.failure_reason = Some("Current checker rejected input".into());
    let rejected = runtime
        .refresh_evidence_by_user(
            "owner",
            "task",
            "refresh-2",
            refreshed.revision,
            vec![failed.clone(), manual.clone()],
            outputs.clone(),
        )
        .unwrap();
    assert!(!rejected.completion_blockers().is_empty());
    assert_eq!(rejected.acceptance_results, historical.acceptance_results);
    assert_eq!(
        rejected.state,
        TaskState::Completed,
        "historical completion is retained, reuse is blocked"
    );
    let history = runtime.evidence_refresh_history("owner", "task").unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0]["passed"], false);
    assert_eq!(history[1]["passed"], true);
    runtime
        .refresh_evidence_by_user(
            "owner",
            "task",
            "refresh-2",
            refreshed.revision,
            vec![failed, manual.clone()],
            outputs.clone(),
        )
        .unwrap();
    assert_eq!(
        runtime
            .evidence_refresh_history("owner", "task")
            .unwrap()
            .len(),
        2,
        "retry does not append a second audit record"
    );
    let mut changed_outputs = outputs.clone();
    changed_outputs.insert("out.docx".into(), "changed".into());
    assert!(
        runtime
            .refresh_evidence_by_user(
                "owner",
                "task",
                "changed",
                rejected.revision,
                vec![machine.clone(), manual.clone()],
                changed_outputs
            )
            .is_err()
    );
    let mut changed_manual = manual.clone();
    changed_manual.evidence_refs.clear();
    assert!(
        runtime
            .refresh_evidence_by_user(
                "owner",
                "task",
                "manual",
                rejected.revision,
                vec![machine, changed_manual],
                outputs
            )
            .is_err()
    );
    drop(runtime);
    let reopened = fixture.open();
    let durable = reopened.get("owner", "task").unwrap();
    assert!(!durable.completion_blockers().is_empty());
    assert_eq!(durable.acceptance_results, historical.acceptance_results);
    assert_eq!(durable.current_acceptance_results()[1], manual);
    assert_eq!(
        reopened
            .evidence_refresh_history("owner", "task")
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn successful_execution_with_bad_content_cannot_complete() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    runtime.create("owner", "task", spec()).unwrap();
    runtime.prepare("owner", "task", step("s1")).unwrap();
    runtime.dispatch("owner", "task", "s1").unwrap();
    runtime
        .observe(
            "owner",
            "task",
            "s1",
            "notice1",
            true,
            false,
            Some(serde_json::json!({"exitCode":0})),
            vec![],
        )
        .unwrap();
    let task = validate_answer(&runtime, br#"{"answer":41}"#);
    assert_eq!(task.state, TaskState::ValidationFailed);
    assert!(
        runtime
            .complete(
                "owner",
                "task",
                "finish",
                task.revision,
                &task.output_identities
            )
            .is_err()
    );
}

#[test]
fn changed_inputs_invalidate_previously_passed_acceptance() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    runtime.create("owner", "task", spec()).unwrap();
    let task = validate_answer(&runtime, br#"{"answer":42}"#);
    let changed = BTreeMap::from([("answer.json".into(), "changed".into())]);
    assert!(
        runtime
            .complete("owner", "task", "finish", task.revision, &changed)
            .unwrap_err()
            .contains("inputs changed")
    );
    let done = runtime
        .complete(
            "owner",
            "task",
            "finish",
            task.revision,
            &task.output_identities,
        )
        .unwrap();
    assert_eq!(done.state, TaskState::Completed);
}

#[test]
fn restart_preserves_prepared_and_marks_dispatched_unknown_without_replay() {
    let fixture = Fixture::new();
    {
        let runtime = fixture.open();
        runtime.create("owner", "task", spec()).unwrap();
        runtime.prepare("owner", "task", step("prepared")).unwrap();
        runtime
            .prepare("owner", "task", step("dispatched"))
            .unwrap();
        runtime.dispatch("owner", "task", "dispatched").unwrap();
    }
    let runtime = fixture.open();
    let task = runtime.get("owner", "task").unwrap();
    assert_eq!(task.steps[0].state, StepState::Prepared);
    assert_eq!(task.steps[1].state, StepState::Unknown);
    assert_eq!(task.state, TaskState::Blocked);
    assert_eq!(task.recovery()[0].action, "revalidate_before_dispatch");
    assert_eq!(task.recovery()[1].action, "inspect_effects");
    assert_eq!(task.recovery()[1].idempotency_key, "op-dispatched");
    assert!(
        runtime
            .complete("owner", "task", "finish", task.revision, &BTreeMap::new())
            .is_err()
    );
}

#[test]
fn durable_terminal_notifications_are_idempotent_and_cannot_regress() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    runtime.create("owner", "task", spec()).unwrap();
    runtime.prepare("owner", "task", step("s1")).unwrap();
    runtime.dispatch("owner", "task", "s1").unwrap();
    let first = runtime
        .observe("owner", "task", "s1", "done", true, false, None, vec![])
        .unwrap();
    let again = runtime
        .observe("owner", "task", "s1", "done", true, false, None, vec![])
        .unwrap();
    assert_eq!(again.revision, first.revision);
    assert!(
        runtime
            .observe("owner", "task", "s1", "done", false, false, None, vec![])
            .is_err()
    );
    assert!(
        runtime
            .observe("owner", "task", "s1", "late", false, true, None, vec![])
            .is_err()
    );
}

#[test]
fn cancellation_survives_late_results_and_restart() {
    let fixture = Fixture::new();
    {
        let runtime = fixture.open();
        runtime.create("owner", "task", spec()).unwrap();
        runtime.prepare("owner", "task", step("s1")).unwrap();
        runtime.dispatch("owner", "task", "s1").unwrap();
        runtime.cancel("owner", "task", "cancel").unwrap();
        let late = runtime
            .observe("owner", "task", "s1", "done", true, false, None, vec![])
            .unwrap();
        assert_eq!(late.state, TaskState::Cancelled);
        assert!(runtime.prepare("owner", "task", step("s2")).is_err());
    }
    let runtime = fixture.open();
    let task = runtime.get("owner", "task").unwrap();
    assert_eq!(task.state, TaskState::Cancelled);
    let resumed = runtime
        .resume_by_user("owner", "task", "human-continue", task.revision)
        .unwrap();
    assert_eq!(resumed.state, TaskState::Blocked);
    assert_eq!(
        runtime.cancel("owner", "task", "cancel").unwrap().state,
        TaskState::Blocked,
        "duplicate old cancel returns current state without replay"
    );
}

#[test]
fn owner_isolation_and_immutable_requirements() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    runtime.create("owner", "task", spec()).unwrap();
    assert!(runtime.get("other", "task").is_err());
    assert!(runtime.cancel("other", "task", "cancel").is_err());
    let mut changed = spec();
    changed.acceptance_checks.clear();
    assert!(runtime.create("owner", "task", changed).is_err());
    let mut changed = spec();
    changed.objective = "easier".into();
    assert!(runtime.create("owner", "task", changed).is_err());
}

#[test]
fn manual_acceptance_requires_current_identity_and_user_control() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    let mut spec = spec();
    spec.acceptance_checks.push(AcceptanceCheck {
        id: "layout".into(),
        description: "Layout review".into(),
        checker: Checker::Manual {
            reason: "Layout needs rendering review".into(),
        },
    });
    let task = runtime.create("owner", "task", spec).unwrap();
    let passed = check_bytes(&task.spec.acceptance_checks[0], br#"{"answer":42}"#);
    let manual = AcceptanceResult {
        check_id: "layout".into(),
        checker_version: CHECKER_VERSION.into(),
        input_identity: "layout-v1".into(),
        status: AcceptanceStatus::AwaitingUser,
        evidence_refs: vec![],
        coverage: "Rendered layout".into(),
        failure_reason: None,
    };
    let task = runtime
        .record_validation(
            "owner",
            "task",
            "validation",
            task.revision,
            vec![passed, manual],
            BTreeMap::from([("answer.json".into(), "content-v1".into())]),
        )
        .unwrap();
    assert_eq!(task.state, TaskState::AwaitingUser);
    assert!(
        runtime
            .complete(
                "owner",
                "task",
                "finish",
                task.revision,
                &task.output_identities
            )
            .is_err()
    );
    assert!(
        runtime
            .confirm_manual_by_user(
                "owner",
                "task",
                "confirm",
                task.revision,
                "layout",
                "old-input"
            )
            .is_err()
    );
    let task = runtime
        .confirm_manual_by_user(
            "owner",
            "task",
            "confirm",
            task.revision,
            "layout",
            "layout-v1",
        )
        .unwrap();
    assert_eq!(
        runtime
            .complete(
                "owner",
                "task",
                "finish",
                task.revision,
                &task.output_identities
            )
            .unwrap()
            .state,
        TaskState::Completed
    );
}

#[test]
fn pid_reuse_and_owner_mismatch_are_never_signalled() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    runtime.create("owner", "task", spec()).unwrap();
    let mut step = step("s1");
    let identity = ProcessIdentity {
        pid: 42,
        created_identity: "creation-A".into(),
        host_id: "host1".into(),
        owner: "owner".into(),
    };
    step.process = Some(identity.clone());
    let task = runtime.prepare("owner", "task", step).unwrap();
    assert!(task.can_signal_process("s1", &identity));
    let mut reused = identity.clone();
    reused.created_identity = "creation-B".into();
    assert!(!task.can_signal_process("s1", &reused));
    let mut other = identity;
    other.owner = "other".into();
    assert!(!task.can_signal_process("s1", &other));
}

#[test]
fn live_host_cannot_be_recovered_by_a_second_store() {
    let fixture = Fixture::new();
    let _runtime = fixture.open();
    assert!(TaskRuntime::open(&fixture.0.join("state.sqlite")).is_err());
}

#[test]
fn expected_negative_sample_verifies_readonly_failure_but_not_unknown_write_effects() {
    for effect in [EffectKind::ReadOnly, EffectKind::Write] {
        let fixture = Fixture::new();
        let runtime = fixture.open();
        let mut contract = spec();
        contract.expected_outputs.clear();
        contract.validation_subject = Some(ValidationSubject {
            kind: "skill".into(),
            identity: "content-sha".into(),
            expected_outcome: "failure".into(),
        });
        contract.acceptance_checks = vec![AcceptanceCheck {
            id: "expected-denial".into(),
            description: "Missing read input reports not-found".into(),
            checker: Checker::ToolResult {
                step_id: "s1".into(),
                assertions: BTreeMap::from([
                    ("/isError".into(), Value::Bool(true)),
                    ("/error/code".into(), Value::from("FS_NOT_FOUND")),
                ]),
            },
        }];
        runtime.create("owner", "task", contract).unwrap();
        let mut load = step("load");
        load.tool = "skill_candidate".into();
        load.effect = EffectKind::ReadOnly;
        runtime.prepare("owner", "task", load).unwrap();
        runtime.dispatch("owner", "task", "load").unwrap();
        runtime
            .observe(
                "owner",
                "task",
                "load",
                "loaded",
                true,
                false,
                Some(serde_json::json!({"contentHash":"content-sha"})),
                vec![],
            )
            .unwrap();
        runtime
            .mark_subject_loaded("owner", "task", "load", "content-sha")
            .unwrap();
        let mut prepared = step("s1");
        prepared.effect = effect.clone();
        runtime.prepare("owner", "task", prepared).unwrap();
        runtime.dispatch("owner", "task", "s1").unwrap();
        let task = runtime
            .observe(
                "owner",
                "task",
                "s1",
                "failed",
                false,
                false,
                Some(serde_json::json!({"isError":true,"error":{"code":"FS_NOT_FOUND"}})),
                vec![],
            )
            .unwrap();
        let check = check_tool_result(
            &task.spec.acceptance_checks[0],
            task.steps.iter().find(|step| step.id == "s1"),
        );
        let task = runtime
            .record_validation(
                "owner",
                "task",
                "validation",
                task.revision,
                vec![check],
                BTreeMap::new(),
            )
            .unwrap();
        let completed =
            runtime.complete("owner", "task", "finish", task.revision, &BTreeMap::new());
        if effect == EffectKind::ReadOnly {
            assert_eq!(completed.unwrap().state, TaskState::Completed);
        } else {
            assert!(completed.is_err());
            assert_eq!(
                task.steps
                    .iter()
                    .find(|step| step.id == "s1")
                    .unwrap()
                    .state,
                StepState::Unknown
            );
        }
    }
}

#[test]
fn known_failed_process_needs_matching_success_and_fresh_content_validation() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    runtime.create("owner", "task", spec()).unwrap();
    let receipt = |code, context| serde_json::json!({"kind":"foreground","processState":"exited","commandStarted":true,"exitCode":code,"signal":null,"completion":if code==0{"succeeded"}else{"failed"},"retryContext":{"command":"cargo test --offline","workdir":"project","policy":context},"stdout":"x".repeat(20000)});
    for (id, code, context) in [("failed", 101, "restricted"), ("other", 0, "full-access")] {
        runtime.prepare("owner", "task", step(id)).unwrap();
        runtime.dispatch("owner", "task", id).unwrap();
        runtime
            .observe(
                "owner",
                "task",
                id,
                id,
                code == 0,
                false,
                Some(receipt(code, context)),
                vec![id.into()],
            )
            .unwrap();
    }
    let task = validate_answer(&runtime, br#"{"answer":42}"#);
    assert_eq!(task.steps[0].state, StepState::Failed);
    assert!(
        task.completion_blockers()
            .iter()
            .any(|b| b.contains("failed"))
    );
    runtime.prepare("owner", "task", step("retry")).unwrap();
    runtime.dispatch("owner", "task", "retry").unwrap();
    runtime
        .observe(
            "owner",
            "task",
            "retry",
            "retry",
            true,
            false,
            Some(receipt(0, "restricted")),
            vec!["retry".into()],
        )
        .unwrap();
    let task = validate_answer(&runtime, br#"{"answer":42}"#);
    assert!(
        task.completion_blockers().is_empty(),
        "{:?}",
        task.completion_blockers()
    );
    runtime
        .complete(
            "owner",
            "task",
            "finish",
            task.revision,
            &task.output_identities,
        )
        .unwrap();
}

#[test]
fn unknown_process_is_never_cleared_by_successful_retry() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    runtime.create("owner", "task", spec()).unwrap();
    for (id, code, state) in [("unknown", None, "cancelled"), ("retry", Some(0), "exited")] {
        runtime.prepare("owner", "task", step(id)).unwrap();
        runtime.dispatch("owner", "task", id).unwrap();
        runtime.observe("owner","task",id,id,code==Some(0),false,Some(serde_json::json!({"kind":"foreground","processState":state,"commandStarted":true,"exitCode":code,"completion":"succeeded","retryContext":{"command":"same"}})),vec![]).unwrap();
    }
    let task = validate_answer(&runtime, br#"{"answer":42}"#);
    assert_eq!(task.steps[0].state, StepState::Unknown);
    assert!(
        task.completion_blockers()
            .iter()
            .any(|b| b.contains("Unknown"))
    );
}
