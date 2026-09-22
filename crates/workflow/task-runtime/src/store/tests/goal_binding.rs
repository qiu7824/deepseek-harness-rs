use super::*;

fn binding(revision: u64) -> GoalBinding {
    GoalBinding {
        goal_id: "goal-existing".into(),
        objective_revision: revision,
    }
}
fn linked() -> ContractSpec {
    let mut spec = spec();
    spec.goal_id = Some("goal-existing".into());
    spec
}
fn completed(runtime: &TaskRuntime) -> TaskContract {
    runtime
        .create_bound("owner", "task", linked(), Some(binding(1)))
        .unwrap();
    let validated = validate_answer(runtime, br#"{"answer":42}"#);
    runtime
        .complete(
            "owner",
            "task",
            "complete",
            validated.revision,
            &validated.output_identities,
        )
        .unwrap()
}

#[test]
fn linked_create_requires_host_binding_and_model_contract_cannot_supply_it() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    assert!(runtime.create("owner", "task", linked()).is_err());
    let mut foreign = binding(1);
    foreign.goal_id = "different".into();
    assert!(
        runtime
            .create_bound("owner", "task", linked(), Some(foreign))
            .is_err()
    );
    let task = runtime
        .create_bound("owner", "task", linked(), Some(binding(1)))
        .unwrap();
    assert_eq!(task.goal_binding, Some(binding(1)));
    assert!(
        runtime
            .create_bound("owner", "task", linked(), Some(binding(2)))
            .is_err(),
        "create retry must not rebind an old task to edited requirements"
    );
    let mut supplied = serde_json::to_value(linked()).unwrap();
    supplied["goalBinding"] = serde_json::to_value(binding(2)).unwrap();
    assert!(serde_json::from_value::<ContractSpec>(supplied).is_err());
}

#[test]
fn rebind_is_a_requirements_revision_even_when_contract_text_is_unchanged() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    let created = runtime
        .create_bound("owner", "task", linked(), Some(binding(1)))
        .unwrap();
    let before = validate_answer(&runtime, br#"{"answer":42}"#);
    assert!(before.goal_binding_blockers(Some(&binding(1))).is_empty());
    assert!(!before.goal_binding_blockers(Some(&binding(2))).is_empty());
    let stale = runtime
        .revise_bound_by_user(
            "owner",
            "task",
            "stale",
            before.revision,
            before.spec.clone(),
            RevisionMode::InPlace,
            Some(binding(2)),
            Some(&binding(1)),
        )
        .unwrap_err();
    assert_eq!(stale, RevisionError::GoalRequirementsChanged);
    let after = runtime
        .revise_bound_by_user(
            "owner",
            "task",
            "adopt",
            before.revision,
            before.spec.clone(),
            RevisionMode::InPlace,
            Some(binding(2)),
            Some(&binding(2)),
        )
        .unwrap();
    assert_eq!(
        after.requirements_revision,
        created.requirements_revision + 1
    );
    assert_eq!(after.goal_binding, Some(binding(2)));
    assert!(
        after.acceptance_results.is_empty()
            && after.acceptance_refresh.is_none()
            && after.output_identities.is_empty()
            && after.validation_identity.is_none()
    );
    assert_eq!(
        runtime
            .requirements_snapshot("owner", "task", "adopt")
            .unwrap(),
        before
    );
    assert_eq!(
        runtime
            .replay_bound_revision(
                "owner",
                "task",
                "adopt",
                before.revision,
                &before.spec,
                RevisionMode::InPlace,
                Some(&binding(2))
            )
            .unwrap(),
        Some(after)
    );
    assert_eq!(
        runtime
            .replay_bound_revision(
                "owner",
                "task",
                "adopt",
                before.revision,
                &before.spec,
                RevisionMode::InPlace,
                Some(&binding(3))
            )
            .unwrap_err(),
        RevisionError::IdempotencyConflict
    );
}

#[test]
fn evidence_refresh_never_changes_the_original_goal_binding_or_its_applicability() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    let before = completed(&runtime);
    let refreshed = runtime
        .refresh_evidence_by_user(
            "owner",
            "task",
            "refresh",
            before.revision,
            before.acceptance_results.clone(),
            before.output_identities.clone(),
        )
        .unwrap();
    assert_eq!(refreshed.goal_binding, Some(binding(1)));
    assert_eq!(refreshed.acceptance_results, before.acceptance_results);
    assert!(
        !refreshed
            .goal_binding_blockers(Some(&binding(2)))
            .is_empty(),
        "refreshing old checks cannot make them evidence for a new objective"
    );
    assert!(
        refreshed
            .goal_binding_blockers(Some(&binding(1)))
            .is_empty()
    );
    assert!(
        !refreshed.goal_binding_blockers(None).is_empty(),
        "a cleared goal is not an applicable current requirement"
    );
}

#[test]
fn independent_successor_keeps_the_old_link_and_cannot_make_the_goal_look_unassociated() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    let before = completed(&runtime);
    let mut independent = before.spec.clone();
    independent.goal_id = None;
    assert_eq!(
        runtime
            .revise_bound_by_user(
                "owner",
                "task",
                "unlink",
                before.revision,
                independent.clone(),
                RevisionMode::InPlace,
                None,
                None
            )
            .unwrap_err(),
        RevisionError::GoalDetachRequiresSuccessor
    );
    let next = runtime
        .revise_bound_by_user(
            "owner",
            "task",
            "independent",
            before.revision,
            independent,
            RevisionMode::Successor,
            None,
            None,
        )
        .unwrap();
    assert_eq!(next.goal_binding, None);
    assert_eq!(next.spec.goal_id, None);
    assert!(next.acceptance_results.is_empty());
    assert_eq!(runtime.get("owner", "task").unwrap(), before);
    assert_eq!(
        runtime.latest_for_goal("owner", "goal-existing").unwrap(),
        Some(before)
    );
    assert!(
        runtime
            .latest_for_goal("owner", "another-simple-goal")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        runtime.latest("owner").unwrap().unwrap().task_id,
        next.task_id
    );
}

#[test]
fn schema_two_linked_history_is_not_rewritten_or_implicitly_trusted_by_schema_three() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    let mut legacy = completed(&runtime);
    legacy.goal_binding = None;
    persist(&runtime.db.lock(), &legacy).unwrap();
    let body = serde_json::to_string(&legacy).unwrap();
    runtime
        .db
        .lock()
        .pragma_update(None, "user_version", 2)
        .unwrap();
    drop(runtime);
    let reopened = fixture.open();
    let restored = reopened.get("owner", "task").unwrap();
    assert_eq!(restored, legacy);
    assert_eq!(restored.state, TaskState::Completed);
    assert!(!restored.completion_blockers().is_empty());
    assert!(!restored.goal_binding_blockers(Some(&binding(1))).is_empty());
    let db = reopened.db.lock();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        db.query_row(
            "SELECT body FROM tasks WHERE owner='owner' AND task_id='task'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        body
    );
}
