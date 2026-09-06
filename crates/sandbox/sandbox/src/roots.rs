//! The writable-root derivation shared by every enforcement dialect that
//! expresses a mode as a canonical allow-list: `workspace-write` means "the
//! workspace root plus the platform temp areas", and this module is that
//! meaning's one home. Rust port of
//! `packages/sandbox/sandbox/src/roots.ts`. The Seatbelt profile
//! (`dsh-sandbox-local`) and the in-process filesystem fence
//! (`dsh-fs-sandbox`) both derive their allow-list here, so "the write tool
//! cannot write /tmp but bash can" asymmetries cannot arise between them.

use crate::index::SandboxExecutionPolicy;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
type TempProvider = Arc<dyn Fn() -> Vec<String> + Send + Sync>;
static MANAGED_TEMP: OnceLock<Mutex<Vec<(u64, TempProvider)>>> = OnceLock::new();
static NEXT_PROVIDER: AtomicU64 = AtomicU64::new(1);
pub struct ManagedTempRegistration(u64);
impl Drop for ManagedTempRegistration {
    fn drop(&mut self) {
        if let Some(providers) = MANAGED_TEMP.get() {
            providers.lock().unwrap().retain(|(id, _)| *id != self.0);
        }
    }
}
/// Host-owned resource providers extend the normal temporary-write boundary.
/// The callback must return only registered private storage roots.
pub fn register_managed_temp(provider: TempProvider) -> ManagedTempRegistration {
    let id = NEXT_PROVIDER.fetch_add(1, Ordering::Relaxed);
    MANAGED_TEMP
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .push((id, provider));
    ManagedTempRegistration(id)
}
pub fn managed_temp_roots() -> Vec<String> {
    let providers = MANAGED_TEMP
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .clone();
    providers
        .into_iter()
        .flat_map(|(_, provider)| provider())
        .collect()
}

/// Resolve a granted root to the path the enforcement layer actually
/// compares: canonical (symlinks resolved), because both Seatbelt filters
/// and the fs fence's containment check match resolved paths — `/tmp` IS
/// `/private/tmp` on darwin, and an as-spelled grant would match nothing.
///
/// Returns the canonical path, or the spelling as-is when resolution fails
/// (a missing root matches nothing until it exists — the conservative
/// outcome; inventing a fallback would grant a path the caller never named).
pub fn canonical_path(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|resolved| resolved.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string())
}

/// The roots one confined execution may WRITE under — the mode's meaning as
/// a canonical, deduplicated allow-list. `read-only` allows nothing;
/// `workspace-write` allows the policy's workspace root, the host `/tmp`, and
/// the per-user platform temp dir (`env::temp_dir()` — the real temp area
/// for mkstemp-family tools; omitting it would deny what the mode promises).
pub fn writable_roots(policy: &SandboxExecutionPolicy) -> Vec<String> {
    if policy.mode.as_str() != "workspace-write" {
        return Vec::new();
    }
    let candidates = [
        policy.workspace_root.clone(),
        "/tmp".to_string(),
        std::env::temp_dir().to_string_lossy().into_owned(),
    ]
    .into_iter()
    .chain(managed_temp_roots());
    let mut roots: Vec<String> = Vec::new();
    for candidate in candidates {
        let canonical = canonical_path(&candidate);
        if !roots.contains(&canonical) {
            roots.push(canonical);
        }
    }
    roots
}
