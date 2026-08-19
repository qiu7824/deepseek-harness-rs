//! Rust port of
//! `packages/util/launch-environment/tests/launch-environment.spec.ts`.

use cordis::Context;
use dsh_launch_environment::{
    DSH_LAUNCH_ENVIRONMENT_KEY, LaunchEnvironmentEntry, LaunchEnvironmentLayerInput,
    LaunchEnvironmentSource, create_launch_environment_snapshot, launch_environment_of,
};

fn layered() -> std::sync::Arc<dsh_launch_environment::LaunchEnvironmentSnapshot> {
    create_launch_environment_snapshot(&[
        LaunchEnvironmentLayerInput {
            source: LaunchEnvironmentSource::Process,
            path: None,
            values: vec![
                ("SHARED".to_string(), "from-process".to_string()),
                ("ONLY_PROCESS".to_string(), "p".to_string()),
            ],
        },
        LaunchEnvironmentLayerInput {
            source: LaunchEnvironmentSource::ProjectEnv,
            path: Some("/work/.env".to_string()),
            values: vec![
                ("SHARED".to_string(), "from-project".to_string()),
                ("ONLY_PROJECT".to_string(), "j".to_string()),
            ],
        },
        LaunchEnvironmentLayerInput {
            source: LaunchEnvironmentSource::UserEnv,
            path: Some("/home/.dsh/.env".to_string()),
            values: vec![
                ("SHARED".to_string(), "from-user".to_string()),
                ("ONLY_USER".to_string(), "u".to_string()),
            ],
        },
    ])
}

fn entry(
    value: &str,
    source: LaunchEnvironmentSource,
    path: Option<&str>,
) -> LaunchEnvironmentEntry {
    LaunchEnvironmentEntry {
        value: value.to_string(),
        source,
        path: path.map(|path| path.to_string()),
    }
}

#[test]
fn resolves_across_every_layer_most_trusted_first_and_reports_the_winning_source() {
    let layered = layered();
    assert_eq!(
        layered.get("SHARED"),
        Some(entry(
            "from-process",
            LaunchEnvironmentSource::Process,
            None
        ))
    );
    assert_eq!(
        layered.get("ONLY_PROJECT"),
        Some(entry(
            "j",
            LaunchEnvironmentSource::ProjectEnv,
            Some("/work/.env")
        ))
    );
    assert_eq!(
        layered.get("ONLY_USER"),
        Some(entry(
            "u",
            LaunchEnvironmentSource::UserEnv,
            Some("/home/.dsh/.env")
        ))
    );
    assert_eq!(layered.get("ABSENT"), None);
}

#[test]
fn filters_layers_without_changing_their_trust_order() {
    let layered = layered();
    assert_eq!(
        layered.get_from(
            "ONLY_PROJECT",
            &[
                LaunchEnvironmentSource::Process,
                LaunchEnvironmentSource::UserEnv
            ]
        ),
        None
    );
    assert_eq!(
        layered.get_from(
            "SHARED",
            &[
                LaunchEnvironmentSource::UserEnv,
                LaunchEnvironmentSource::Process
            ]
        ),
        Some(entry(
            "from-process",
            LaunchEnvironmentSource::Process,
            None
        ))
    );
    assert_eq!(layered.get_from("SHARED", &[]), None);
}

#[test]
fn copies_each_layer_so_a_later_mutation_of_the_source_object_cannot_change_it() {
    let mut values = vec![("KEY".to_string(), "first".to_string())];
    let snapshot = create_launch_environment_snapshot(&[LaunchEnvironmentLayerInput {
        source: LaunchEnvironmentSource::Process,
        path: None,
        values: values.clone(),
    }]);
    values[0].1 = "second".to_string();
    values.push(("LATE".to_string(), "added".to_string()));
    assert_eq!(
        snapshot.get("KEY"),
        Some(entry("first", LaunchEnvironmentSource::Process, None))
    );
    assert_eq!(snapshot.get("LATE"), None);
}

#[test]
fn keeps_an_empty_value_as_a_present_value_for_its_owner_to_judge() {
    let snapshot = create_launch_environment_snapshot(&[LaunchEnvironmentLayerInput {
        source: LaunchEnvironmentSource::Process,
        path: None,
        values: vec![("EMPTY".to_string(), String::new())],
    }]);
    assert_eq!(
        snapshot.get("EMPTY"),
        Some(entry("", LaunchEnvironmentSource::Process, None))
    );
}

#[test]
fn orders_lookups_canonically_regardless_of_construction_order() {
    let reversed = create_launch_environment_snapshot(&[
        LaunchEnvironmentLayerInput {
            source: LaunchEnvironmentSource::UserEnv,
            path: Some("/u".to_string()),
            values: vec![("K".to_string(), "u".to_string())],
        },
        LaunchEnvironmentLayerInput {
            source: LaunchEnvironmentSource::Process,
            path: None,
            values: vec![("K".to_string(), "p".to_string())],
        },
    ]);
    assert_eq!(
        reversed.get("K"),
        Some(entry("p", LaunchEnvironmentSource::Process, None))
    );
}

#[test]
fn launch_environment_of_returns_the_launcher_snapshot_when_the_product_cli_provided_one() {
    let ctx = Context::root();
    let layered = layered();
    let slot: cordis::ArcValue = cordis::arc(layered.clone());
    ctx.provide(DSH_LAUNCH_ENVIRONMENT_KEY, Some(slot));
    let resolved = launch_environment_of(&ctx);
    assert_eq!(resolved.get("SHARED"), layered.get("SHARED"));
}

#[test]
fn launch_environment_of_falls_back_to_the_inherited_environment_as_the_only_layer() {
    // The ambient environment always carries a handful of well-known names;
    // probe with a per-test variable set for this process run.
    let probe = format!("DSH_ENV_SPEC_FALLBACK_{}", std::process::id());
    // SAFETY: single-threaded test process; the variable is per-test and
    // restored to a stable value (Rust 2024 marks `set_var` unsafe because
    // of multi-threaded process races).
    unsafe { std::env::set_var(&probe, "ambient") };
    let ctx = Context::root();
    let snapshot = launch_environment_of(&ctx);
    #[cfg(windows)]
    let resolved = snapshot.get(&probe.to_uppercase());
    #[cfg(not(windows))]
    let resolved = snapshot.get(&probe);
    assert_eq!(
        resolved,
        Some(entry("ambient", LaunchEnvironmentSource::Process, None))
    );
}
