use super::*;

#[test]
fn legacy_database_migrates_atomically_without_rewriting_tasks_and_reopens_at_current_schema() {
    let fixture = Fixture::new();
    let runtime = fixture.open();
    runtime.create("owner", "task", spec()).unwrap();
    let before = validate_answer(&runtime, br#"{"answer":42}"#);
    let mut legacy = serde_json::to_value(&before).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("requirementsRevision");
    legacy.as_object_mut().unwrap().remove("basedOn");
    let body = serde_json::to_string(&legacy).unwrap();
    {
        let db = runtime.db.lock();
        db.execute(
            "UPDATE tasks SET body=?1 WHERE owner='owner' AND task_id='task'",
            params![body],
        )
        .unwrap();
        db.execute_batch("DROP TABLE contract_revisions; PRAGMA user_version=1;")
            .unwrap();
    }
    drop(runtime);
    let upgraded = fixture.open();
    assert_eq!(upgraded.get("owner", "task").unwrap(), before);
    {
        let db = upgraded.db.lock();
        let version: u32 = db
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, DATABASE_VERSION);
        assert!(
            version > 2,
            "legacy Hosts reject the schema instead of dropping revision metadata"
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
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM mutations", [], |row| row
                .get::<_, u32>(0))
                .unwrap(),
            1
        );
    }
    drop(upgraded);
    let reopened = fixture.open();
    assert_eq!(
        reopened.get("owner", "task").unwrap().version,
        FORMAT_VERSION
    );
    assert!(
        reopened
            .requirements_history("owner", "task")
            .unwrap()
            .is_empty()
    );
    let mut spec = before.spec.clone();
    spec.objective = "Revised after upgrade".into();
    let revised = reopened
        .revise_by_user(
            "owner",
            "task",
            "edit",
            before.revision,
            spec,
            RevisionMode::InPlace,
        )
        .unwrap();
    assert_eq!(revised.requirements_revision, 2);
    assert_eq!(
        reopened
            .requirements_snapshot("owner", "task", "edit")
            .unwrap(),
        before
    );
}

#[test]
fn failed_schema_migration_rolls_back_and_future_versions_are_never_downgraded() {
    let fixture = Fixture::new();
    let path = fixture.0.join("state.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE contract_revisions(broken INTEGER); PRAGMA user_version=1;")
        .unwrap();
    drop(db);
    assert!(TaskRuntime::open(&path).is_err());
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tasks'",
            [],
            |row| row.get::<_, u32>(0)
        )
        .unwrap(),
        0,
        "all new tables roll back with the version marker"
    );
    db.pragma_update(None, "user_version", DATABASE_VERSION + 1)
        .unwrap();
    drop(db);
    assert!(
        TaskRuntime::open(&path)
            .err()
            .unwrap()
            .contains("newer version")
    );
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        DATABASE_VERSION + 1
    );
}
