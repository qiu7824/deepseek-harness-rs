//! Rust port of `anonymous-user-id.spec.ts` + `invariant.spec.ts`: first-use
//! persistence, whitespace tolerance, corrupt-file recovery, exclusive-create
//! adoption, best-effort failure, process-lifetime memoization, and the
//! invariant companion registration.

use std::path::PathBuf;
use std::sync::Arc;

use dsh_anonymous_user_id::{
    ANONYMOUS_USER_ID_FILE_NAME, AnonymousUserIdOptions, get_or_create_anonymous_user_id,
    invariant::AnonymousUserIdInvariantPlugin,
};

const UUID_PATTERN: &str = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";

fn temp_home() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dsh-userid-{}", uuid::Uuid::new_v4().to_string()));
    std::fs::create_dir_all(&dir).expect("temp home");
    dir
}

fn env_of(home: &PathBuf) -> Arc<dyn Fn(&str) -> Option<String> + Send + Sync> {
    let home = home.to_string_lossy().to_string();
    Arc::new(move |name: &str| (name == "DSH_HOME").then(|| home.clone()))
}

fn file_of(home: &PathBuf) -> PathBuf {
    home.join(ANONYMOUS_USER_ID_FILE_NAME)
}

fn assert_uuid(id: &str) {
    assert!(
        regex::Regex::new(UUID_PATTERN)
            .expect("static pattern")
            .is_match(id),
        "{id}"
    );
}

#[test]
fn creates_persists_and_returns_a_bare_uuid_line_on_first_use() {
    let home = temp_home();
    let id = get_or_create_anonymous_user_id(AnonymousUserIdOptions {
        env: Some(env_of(&home)),
        random_uuid: None,
    });
    assert_uuid(id.as_str());
    let persisted = std::fs::read_to_string(file_of(&home)).expect("persisted");
    assert_eq!(persisted, format!("{id}\n"));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn creates_the_home_directory_when_missing() {
    let root = temp_home();
    let home = root.join("nested").join("home");
    let id = get_or_create_anonymous_user_id(AnonymousUserIdOptions {
        env: Some(env_of(&home)),
        random_uuid: None,
    });
    let persisted = std::fs::read_to_string(file_of(&home)).expect("persisted");
    assert_eq!(persisted, format!("{id}\n"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn returns_the_persisted_id_tolerating_surrounding_whitespace() {
    let home = temp_home();
    let existing = "01234567-89ab-4cde-8f01-23456789abcd";
    std::fs::write(file_of(&home), format!("  {existing}\n\n")).expect("seed");
    let id = get_or_create_anonymous_user_id(AnonymousUserIdOptions {
        env: Some(env_of(&home)),
        random_uuid: None,
    });
    assert_eq!(id.as_str(), existing);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn overwrites_a_corrupt_file_with_a_fresh_id() {
    let home = temp_home();
    std::fs::write(file_of(&home), "not-a-uuid\n").expect("seed");
    let id = get_or_create_anonymous_user_id(AnonymousUserIdOptions {
        env: Some(env_of(&home)),
        random_uuid: None,
    });
    assert_uuid(id.as_str());
    let persisted = std::fs::read_to_string(file_of(&home)).expect("persisted");
    assert_eq!(persisted, format!("{id}\n"));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn adopts_a_concurrent_winner_written_after_the_initial_read() {
    let home = temp_home();
    let winner = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let file = file_of(&home);
    let file_for_hook = file.clone();
    let id = get_or_create_anonymous_user_id(AnonymousUserIdOptions {
        env: Some(env_of(&home)),
        random_uuid: Some(Arc::new(move || {
            // The generator hook runs between the initial read (absent) and
            // the exclusive write, simulating the concurrent first launch.
            std::fs::write(&file_for_hook, format!("{winner}\n")).expect("plant winner");
            "ffffffff-0000-4000-8000-000000000000".to_string()
        })),
    });
    assert_eq!(id.as_str(), winner);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn returns_a_usable_id_when_the_home_cannot_contain_files() {
    let root = temp_home();
    let blocked = root.join("blocked");
    std::fs::write(&blocked, "occupied\n").expect("occupied file");
    let id = get_or_create_anonymous_user_id(AnonymousUserIdOptions {
        env: Some(env_of(&blocked)),
        random_uuid: None,
    });
    assert_uuid(id.as_str());
    assert!(!blocked.join(ANONYMOUS_USER_ID_FILE_NAME).exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn memoizes_per_resolved_home_for_the_process_lifetime() {
    let home = temp_home();
    let first = get_or_create_anonymous_user_id(AnonymousUserIdOptions {
        env: Some(env_of(&home)),
        random_uuid: None,
    });
    std::fs::remove_file(file_of(&home)).expect("delete");
    let second = get_or_create_anonymous_user_id(AnonymousUserIdOptions {
        env: Some(env_of(&home)),
        random_uuid: None,
    });
    assert_eq!(first, second, "deletion-proof process memo");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn keeps_distinct_homes_on_distinct_ids() {
    let a_home = temp_home();
    let b_home = temp_home();
    let a = get_or_create_anonymous_user_id(AnonymousUserIdOptions {
        env: Some(env_of(&a_home)),
        random_uuid: None,
    });
    let b = get_or_create_anonymous_user_id(AnonymousUserIdOptions {
        env: Some(env_of(&b_home)),
        random_uuid: None,
    });
    assert_ne!(a, b);
    let _ = std::fs::remove_dir_all(&a_home);
    let _ = std::fs::remove_dir_all(&b_home);
}

#[test]
fn reads_the_process_env_by_default() {
    let home = temp_home();
    // SAFETY: single-threaded test binary; no other test consults the
    // process env, and the previous value is restored below.
    let previous = std::env::var("DSH_HOME").ok();
    unsafe {
        std::env::set_var("DSH_HOME", home.to_string_lossy().to_string());
    }
    let id = get_or_create_anonymous_user_id(AnonymousUserIdOptions::default());
    let persisted = std::fs::read_to_string(file_of(&home)).expect("persisted");
    assert_eq!(persisted, format!("{id}\n"));
    match previous {
        Some(value) => unsafe {
            std::env::set_var("DSH_HOME", value);
        },
        None => unsafe {
            std::env::remove_var("DSH_HOME");
        },
    }
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test(flavor = "current_thread")]
async fn invariant_companion_registers_with_an_empty_installer() {
    let ctx = cordis::Context::root();
    let _registry = dsh_invariants::InvariantRegistry::new(
        &ctx,
        dsh_invariants::InvariantConfig {
            enabled: true,
            package_allowlist: vec![],
            package_blocklist: vec![],
        },
    );
    let fiber = ctx.plugin(Arc::new(AnonymousUserIdInvariantPlugin), cordis::arc(()));
    fiber.settle().await.expect("settle");
    // Duplicate registration must fail loud (ownership was reserved).
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dsh_anonymous_user_id::invariant::apply(&ctx);
    }));
    assert!(outcome.is_err(), "package name already reserved");
    fiber.dispose().await;
}
