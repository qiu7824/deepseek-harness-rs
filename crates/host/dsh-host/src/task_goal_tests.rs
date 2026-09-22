use super::*;

fn identity(revision: u64) -> GoalRequirementsIdentity {
    GoalRequirementsIdentity {
        goal_id: "goal-a".into(),
        objective_revision: revision,
    }
}
fn task(binding: Option<GoalBinding>) -> TaskContract {
    serde_json::from_value(json!({"version":1,"taskId":"t","owner":"s","revision":1,"requirementsRevision":1,
        "spec":{"objective":"Task","goalId":"goal-a","acceptanceChecks":[]},"state":"completed","steps":[],"acceptanceResults":[],
        "goalBinding":binding,"validationIdentity":null,"outputIdentities":{},"createdAt":1,"updatedAt":1})).unwrap()
}

#[test]
fn model_omission_or_null_cannot_avoid_the_current_noncomplete_goal() {
    let current = identity(2);
    for phase in [
        dsh_goal::GoalPhase::Active,
        dsh_goal::GoalPhase::Paused,
        dsh_goal::GoalPhase::Blocked,
    ] {
        assert_eq!(
            model_binding(None, Some(&current), Some(phase)).unwrap(),
            Some(binding(&current))
        );
        assert_eq!(
            model_binding(Some("goal-a"), Some(&current), Some(phase)).unwrap(),
            Some(binding(&current))
        );
        assert!(matches!(
            model_binding(Some("other"), Some(&current), Some(phase)),
            Err(RevisionError::GoalRequirementsChanged)
        ));
        assert!(model_binding(Some(""), Some(&current), Some(phase)).is_err());
    }
    let null_spec: ContractSpec =
        serde_json::from_value(json!({"objective":"Task","goalId":null,"acceptanceChecks":[]}))
            .unwrap();
    assert!(
        model_binding(
            null_spec.goal_id.as_deref(),
            Some(&current),
            Some(dsh_goal::GoalPhase::Active)
        )
        .unwrap()
        .is_some()
    );
    assert_eq!(model_binding(None, None, None).unwrap(), None);
    assert_eq!(
        model_binding(None, Some(&current), Some(dsh_goal::GoalPhase::Complete)).unwrap(),
        None
    );
    assert!(
        model_binding(
            Some("goal-a"),
            Some(&current),
            Some(dsh_goal::GoalPhase::Complete)
        )
        .is_err()
    );
}

#[test]
fn goal_status_distinguishes_legacy_missing_stale_and_unavailable() {
    let current = GoalFacts {
        binding: Some(binding(&identity(2))),
        objective: Some("Current goal".into()),
        unavailable: false,
    };
    assert_eq!(
        current.status(&task(Some(binding(&identity(2))))),
        "current"
    );
    assert_eq!(current.status(&task(Some(binding(&identity(1))))), "stale");
    assert_eq!(current.status(&task(None)), "missing");
    let cleared = GoalFacts::from_requirements(None);
    assert_eq!(cleared.status(&task(Some(binding(&identity(1))))), "stale");
    let unreadable = GoalFacts::from_requirements(Some(json!({"unavailable":true})));
    assert_eq!(
        unreadable.status(&task(Some(binding(&identity(1))))),
        "unavailable"
    );
    assert!(
        !unreadable
            .blockers(&task(Some(binding(&identity(1)))))
            .is_empty()
    );
    let mut independent = task(None);
    independent.spec.goal_id = None;
    assert_eq!(unreadable.status(&independent), "unlinked");
    assert!(unreadable.blockers(&independent).is_empty());
}

#[test]
fn list_serializes_a_large_current_objective_once_not_once_per_task() {
    let objective = "x".repeat(1024 * 1024);
    let tasks=(0..64).map(|index|json!({"taskId":format!("task-{index}"),"spec":{"goalId":"goal-a"},"goalBinding":binding(&identity(1))})).collect::<Vec<_>>();
    let response = decorate_with_facts(
        json!({"tasks":tasks}),
        GoalFacts {
            binding: Some(binding(&identity(1))),
            objective: Some(objective),
            unavailable: false,
        },
    );
    assert_eq!(
        response["currentGoalRequirements"]["objective"]
            .as_str()
            .unwrap()
            .len(),
        1024 * 1024
    );
    for task in response["tasks"].as_array().unwrap() {
        assert_eq!(task["goalBindingStatus"], "current");
        assert!(task.get("currentGoalRequirements").is_none());
    }
    assert!(
        serde_json::to_vec(&response).unwrap().len() < 1024 * 1024 + 32 * 1024,
        "goal metadata must not amplify a 1 MiB objective into 64 copies"
    );
}

#[test]
fn binding_context_changes_only_for_requirements_or_applicability() {
    let task = task(Some(binding(&identity(1))));
    let facts = GoalFacts {
        binding: Some(binding(&identity(1))),
        objective: Some("never duplicate objective".repeat(1000)),
        unavailable: false,
    };
    let mut initial = prompt_task_state(&task);
    add_summary_facts(&mut initial, &task, &facts);
    let mut journal = task.clone();
    journal.revision += 50;
    journal.updated_at += 999;
    let mut after = prompt_task_state(&journal);
    add_summary_facts(&mut after, &journal, &facts);
    assert_eq!(initial, after);
    assert!(!initial.to_string().contains("never duplicate objective"));
    let updated = GoalFacts {
        binding: Some(binding(&identity(2))),
        objective: None,
        unavailable: false,
    };
    add_summary_facts(&mut after, &journal, &updated);
    assert_ne!(initial, after);
    assert_eq!(after["goalBindingStatus"], "stale");
}

#[test]
fn completion_permit_keeps_stop_validation_across_verification_handoff() {
    use std::sync::{Barrier, mpsc};
    let work = validation_work::Work::default();
    let started = work.clone();
    let barrier = Arc::new(Barrier::new(2));
    let release = barrier.clone();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let guard = started.begin("s", "t", Arc::new(|| false));
        finished_tx.send(digest(b"verified file bytes")).unwrap();
        release.wait();
        CompletionPermit {
            tasks: Weak::new(),
            owner: "s".into(),
            task: Some(("t".into(), 1)),
            goal: binding(&identity(1)),
            work: Some(guard),
        }
    });
    assert!(!finished_rx.recv().unwrap().is_empty());
    work.cancel("s", "t");
    barrier.wait();
    let permit = worker.join().unwrap();
    assert!(matches!(
        permit.commit_guard(),
        Err(GoalCompletionError::Cancelled)
    ));
}
