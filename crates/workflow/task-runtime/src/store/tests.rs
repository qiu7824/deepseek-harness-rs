use super::*;

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
        goal_id: Some("goal1".into()),
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
