//! Tests for the LOCAL spill backend: Rust port of
//! `packages/spill/spill-local/tests/spill-local.spec.ts` — `saveText` writes
//! a session-scoped file and returns a locator + byte length + retrieval
//! hint, filename sanitization neutralizes traversal, the configured `root`
//! is honored (and the private default when omitted), and a storage failure
//! rejects. The Cordis-free `store` helpers are exercised directly for the
//! naming/encoding edge cases.

use std::sync::Arc;

use cordis::Context;
use dsh_spill::{SaveTextSpill, SpillOwner, SpillSource, SpillStore};
use dsh_spill_local::{
    LocalSpillStore, encode_segment, private_root, save_text_file, session_dir,
};

/// A per-test temp root that cleans itself up.
struct TempRoot(std::path::PathBuf);

impl TempRoot {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "dsh-spill-test-rs-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&base).expect("temp root");
        Self(base)
    }

    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn request() -> SaveTextSpill {
    SaveTextSpill {
        owner: SpillOwner { session_id: dsh_session::session_id("sess-1") },
        source: SpillSource {
            tool_name: "web_fetch".to_string(),
            call_id: dsh_llm::call_id("call-1"),
            label: "result".to_string(),
        },
        suggested_name: "web_fetch.txt".to_string(),
        content: "the full body".to_string(),
    }
}

// ---------------------------------------------------------------------------
// encodeSegment

#[test]
fn encode_segment_cases() {
    // Keeps the safe set literal.
    assert_eq!(encode_segment("web_fetch.txt"), "web_fetch.txt");
    assert_eq!(encode_segment("a-B_9.z"), "a-B_9.z");

    // Escapes separators and tilde (dots are literal except as whole-segment
    // tokens): the traversal defense is that separators escape, keeping the
    // result ONE segment.
    assert_eq!(encode_segment("../etc/passwd"), "..~002Fetc~002Fpasswd");
    assert_eq!(encode_segment("a/b"), "a~002Fb");
    assert_eq!(encode_segment("~"), "~007E");

    // Escapes the whole-segment dot tokens.
    assert_eq!(encode_segment("."), "~002E");
    assert_eq!(encode_segment(".."), "~002E~002E");

    // Encodes the empty string to a non-empty segment.
    assert_eq!(encode_segment(""), "~");
}

// ---------------------------------------------------------------------------
// sessionDir

#[test]
fn session_dir_is_a_stable_per_session_hash_under_the_root() {
    let dir = session_dir("/spill", "sess-1");
    assert_eq!(dir, session_dir("/spill", "sess-1"));
    let path = std::path::Path::new(&dir);
    assert_eq!(path.parent().expect("parent"), std::path::Path::new("/spill"));
    let base = path.file_name().expect("basename").to_string_lossy().into_owned();
    assert!(
        base.len() == 20 && base.starts_with("session-")
            && base[8..].chars().all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()),
        "{base}"
    );
    assert_ne!(session_dir("/spill", "sess-2"), dir);
}

// ---------------------------------------------------------------------------
// saveTextFile

#[tokio::test(flavor = "current_thread")]
async fn save_text_file_writes_the_content_under_the_session_dir_and_reports_bytes() {
    let temp = TempRoot::new();
    let saved = save_text_file(dsh_spill_local::SaveTextOptions {
        root: temp.path(),
        session_id: "sess-1".to_string(),
        suggested_name: "r.txt".to_string(),
        content: "héllo".to_string(),
    })
    .await
    .expect("save");
    assert_eq!(std::fs::read_to_string(&saved.path).expect("read"), "héllo");
    assert_eq!(saved.bytes, "héllo".len() as u64);
    assert_eq!(
        std::path::Path::new(&saved.path).parent().expect("parent").to_string_lossy(),
        session_dir(&temp.path(), "sess-1")
    );
    let base = std::path::Path::new(&saved.path)
        .file_name()
        .expect("basename")
        .to_string_lossy()
        .into_owned();
    assert!(
        base.len() == 18 && base[..12].chars().all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
            && &base[13..] == "r.txt",
        "{base}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn save_text_file_sanitizes_a_traversal_shaped_suggested_name_into_one_segment() {
    let temp = TempRoot::new();
    let saved = save_text_file(dsh_spill_local::SaveTextOptions {
        root: temp.path(),
        session_id: "sess-1".to_string(),
        suggested_name: "../../evil".to_string(),
        content: "x".to_string(),
    })
    .await
    .expect("save");
    // The separators escaped, so the whole name is one leaf under the
    // session dir.
    assert_eq!(
        std::path::Path::new(&saved.path).parent().expect("parent").to_string_lossy(),
        session_dir(&temp.path(), "sess-1")
    );
    assert!(!saved.path.replace('\\', "/").contains("/.."), "{}", saved.path);
}

#[tokio::test(flavor = "current_thread")]
#[cfg(unix)]
async fn save_text_file_creates_the_session_directory_and_file_with_owner_only_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempRoot::new();
    let saved = save_text_file(dsh_spill_local::SaveTextOptions {
        root: temp.path(),
        session_id: "sess-1".to_string(),
        suggested_name: "r.txt".to_string(),
        content: "x".to_string(),
    })
    .await
    .expect("save");
    let directory = std::fs::metadata(std::path::Path::new(&saved.path).parent().expect("parent"))
        .expect("dir stat");
    let file = std::fs::metadata(&saved.path).expect("file stat");
    assert!(directory.is_dir());
    assert!(file.is_file());
    assert_eq!(directory.permissions().mode() & 0o777, 0o700);
    assert_eq!(file.permissions().mode() & 0o777, 0o600);
}

#[tokio::test(flavor = "current_thread")]
async fn save_text_file_gives_distinct_paths_to_two_saves_of_the_same_name() {
    let temp = TempRoot::new();
    let options = || dsh_spill_local::SaveTextOptions {
        root: temp.path(),
        session_id: "sess-1".to_string(),
        suggested_name: "r.txt".to_string(),
        content: "x".to_string(),
    };
    let a = save_text_file(options()).await.expect("save a");
    let b = save_text_file(options()).await.expect("save b");
    assert_ne!(a.path, b.path);
}

// ---------------------------------------------------------------------------
// privateRoot

#[test]
fn private_root_is_a_stable_absolute_directory_under_the_temp_dir() {
    let first = private_root();
    assert!(std::path::Path::new(first).is_absolute());
    assert_eq!(private_root(), first);
    assert_eq!(
        std::path::Path::new(first).parent().expect("parent"),
        std::env::temp_dir()
    );
    assert!(std::fs::metadata(first).expect("exists").is_dir());
}

// ---------------------------------------------------------------------------
// LocalSpillStore service

#[tokio::test(flavor = "current_thread")]
async fn local_store_registers_as_ctx_spill_store_and_saves_under_the_configured_root() {
    let temp = TempRoot::new();
    let ctx = Context::root();
    let store = LocalSpillStore::install(
        &ctx,
        dsh_spill_local::Config { root: Some(temp.path()) },
    )
    .expect("install");
    let reference = store.save_text(&request()).await.expect("save");
    assert_eq!(
        std::path::Path::new(reference.locator.as_str())
            .parent()
            .expect("parent")
            .to_string_lossy(),
        session_dir(&temp.path(), "sess-1")
    );
    assert_eq!(
        std::fs::read_to_string(reference.locator.as_str()).expect("read"),
        "the full body"
    );
    assert_eq!(reference.bytes, "the full body".len() as u64);
    assert_eq!(
        reference.retrieval_hint,
        "Use read with offset/limit, or grep this path to search within it."
    );
}

#[tokio::test(flavor = "current_thread")]
async fn local_store_resolves_a_relative_configured_root_to_absolute() {
    let ctx = Context::root();
    let store = LocalSpillStore::install(
        &ctx,
        dsh_spill_local::Config { root: Some(".".to_string()) },
    )
    .expect("install");
    assert!(std::path::Path::new(store.root()).is_absolute());
}

#[tokio::test(flavor = "current_thread")]
async fn local_store_falls_back_to_the_private_root_when_none_is_configured() {
    let ctx = Context::root();
    let store = LocalSpillStore::install(&ctx, dsh_spill_local::Config::default())
        .expect("install");
    assert_eq!(store.root(), private_root());
}

#[tokio::test(flavor = "current_thread")]
async fn local_store_rejects_when_the_root_is_not_writable() {
    let temp = TempRoot::new();
    // A file (not a dir) as the root makes mkdir under it fail — a real
    // storage error.
    let saved = save_text_file(dsh_spill_local::SaveTextOptions {
        root: temp.path(),
        session_id: "s".to_string(),
        suggested_name: "f".to_string(),
        content: "x".to_string(),
    })
    .await
    .expect("seed file");
    let ctx = Context::root();
    let store = LocalSpillStore::install(
        &ctx,
        dsh_spill_local::Config { root: Some(saved.path.clone()) },
    )
    .expect("install");
    assert!(store.save_text(&request()).await.is_err());
}

// ---------------------------------------------------------------------------
// the erased service handle

#[tokio::test(flavor = "current_thread")]
async fn the_spill_store_service_resolves_through_the_erased_handle() {
    let temp = TempRoot::new();
    let ctx = Context::root();
    let _store = LocalSpillStore::install(
        &ctx,
        dsh_spill_local::Config { root: Some(temp.path()) },
    )
    .expect("install");
    let resolved = ctx
        .get_typed::<Arc<dyn SpillStore>>("spillStore", false)
        .expect("service registered");
    let reference = resolved.save_text(&request()).await.expect("save");
    assert!(reference.locator.as_str().starts_with(&temp.path()));
}
