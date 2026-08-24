//! Per-harness-home anonymous user id shared by telemetry and feedback.
//! Rust port of `packages/identity/anonymous-user-id/src/index.ts`.
//!
//! The id is a random UUID persisted as a bare line in `.anonymous-user-id`
//! inside the harness home resolved by [`dsh_home_paths::resolve_dsh_home`]
//! (`$DSH_HOME` > `~/.dsh`), and never derived from any identifying source.
//! It is scoped to the harness home, not the machine. The result is
//! memoized per resolved file path for the process lifetime.

pub mod invariant;

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use dsh_brand::Branded;
use parking_lot::Mutex;

/// The brand marker for [`AnonymousUserId`].
#[doc(hidden)]
pub enum AnonymousUserIdTag {}

/// A harness-home-scoped anonymous user id (random UUID v4).
pub type AnonymousUserId = Branded<AnonymousUserIdTag>;

/// File inside the harness home storing the id: a bare UUID line, no wrapper
/// format.
pub const ANONYMOUS_USER_ID_FILE_NAME: &str = ".anonymous-user-id";

/// The TS `UUID_PATTERN` (hyphenated 8-4-4-4-12 hex).
const UUID_PATTERN: &str = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";

/// Ambient hooks for locating and generating the id; every field has a
/// default.
pub type EnvironmentReader = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

#[derive(Default)]
pub struct AnonymousUserIdOptions {
    /// Environment consulted for `DSH_HOME`; defaults to the process env.
    pub env: Option<EnvironmentReader>,
    /// UUID generator; defaults to a random v4 (test hook).
    pub random_uuid: Option<Arc<dyn Fn() -> String + Send + Sync>>,
}

/// Process-lifetime memo keyed by resolved file path, so distinct test homes
/// never share an id.
static MEMO: std::sync::OnceLock<Mutex<HashMap<String, AnonymousUserId>>> =
    std::sync::OnceLock::new();

fn memo() -> &'static Mutex<HashMap<String, AnonymousUserId>> {
    MEMO.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Read a valid persisted id from the file, or `None` when absent/corrupt
/// (TS `readPersistedId`).
fn read_persisted_id(file: &PathBuf) -> Option<AnonymousUserId> {
    let text = std::fs::read_to_string(file).ok()?;
    let value = text.trim();
    let pattern = regex::Regex::new(UUID_PATTERN).expect("static pattern");
    pattern.is_match(value).then(|| AnonymousUserId::new(value))
}

/// Return the harness home's anonymous user id, creating and persisting one
/// on first use. A concurrent first launch is settled by an exclusive-create
/// write: the loser rereads the winner's id. Persistence is best-effort — a
/// write failure (read-only home) still returns a usable id for the current
/// run so feedback and telemetry are never blocked (TS
/// `getOrCreateAnonymousUserId`).
pub fn get_or_create_anonymous_user_id(options: AnonymousUserIdOptions) -> AnonymousUserId {
    let env = options
        .env
        .unwrap_or_else(|| Arc::new(|name: &str| std::env::var(name).ok()));
    let file = dsh_home_paths::resolve_dsh_home(None, &*env).join(ANONYMOUS_USER_ID_FILE_NAME);
    let key = file.to_string_lossy().to_string();
    if let Some(cached) = memo().lock().get(&key).cloned() {
        return cached;
    }

    let mut id = read_persisted_id(&file);
    if id.is_none() {
        let generate = options
            .random_uuid
            .unwrap_or_else(|| Arc::new(|| uuid::Uuid::new_v4().to_string()));
        let created = AnonymousUserId::new(generate());
        let exclusive = (|| -> std::io::Result<()> {
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut handle = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)?;
            writeln!(handle, "{created}")?;
            Ok(())
        })();
        match exclusive {
            Ok(()) => id = Some(created.clone()),
            Err(_) => {
                // A create_new refusal covers both a concurrent winner and a
                // pre-existing corrupt file: the reread adopts a valid
                // winner, and an invalid reread falls through to the
                // overwrite path. Non-refusal failures (read-only home) land
                // there too, accepted best-effort below.
                id = read_persisted_id(&file);
                if id.is_none() {
                    let _ = std::fs::write(&file, format!("{created}\n"));
                    id = Some(created.clone());
                }
            }
        }
    }
    let id = id.expect("fresh id");
    memo().lock().insert(key, id.clone());
    id
}
