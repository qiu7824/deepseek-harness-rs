//! Scoped cancellation receipts for prompts that have not reached admission yet.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use dsh_session::SessionHeader;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

const MAX_CANCEL_REQUESTS: usize = 64;
const MAX_REQUEST_ID_BYTES: usize = 200;
const MARKER: &[u8] = b"cancelled-v1\n";

#[derive(Clone, Debug)]
pub(crate) struct RequestScope([u8; 32]);

impl RequestScope {
    pub fn session(header: &SessionHeader) -> Result<Self, String> {
        if header.origin.as_deref() == Some("subagent") {
            return Err("subagent sessions require their direct-parent cancellation route".into());
        }
        Ok(Self::digest("session", None, header))
    }

    pub fn subagent(parent: &SessionHeader, child: &SessionHeader) -> Result<Self, String> {
        if child.parent_session.as_ref() != Some(&parent.id)
            || child.origin.as_deref() != Some("subagent")
        {
            return Err("subagent cancellation requires the recorded direct parent".into());
        }
        Ok(Self::digest("subagent", Some(parent), child))
    }

    fn digest(kind: &str, parent: Option<&SessionHeader>, child: &SessionHeader) -> Self {
        let mut hash = Sha256::new();
        for field in [
            "dsh-request-cancellation-v1",
            kind,
            parent.map(|p| p.id.as_str()).unwrap_or(""),
            child.id.as_str(),
        ] {
            hash.update((field.len() as u64).to_le_bytes());
            hash.update(field.as_bytes());
        }
        hash.update(parent.map(|p| p.created_at).unwrap_or(0).to_le_bytes());
        hash.update(child.created_at.to_le_bytes());
        Self(hash.finalize().into())
    }

    fn key(&self, request_id: &str) -> Result<RequestKey, String> {
        validate_id(request_id)?;
        Ok(RequestKey {
            scope: self.0,
            request: Sha256::digest(request_id.as_bytes()).into(),
        })
    }
}

fn validate_id(value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > MAX_REQUEST_ID_BYTES {
        return Err("requestId must contain 1 to 200 bytes".into());
    }
    Ok(())
}

pub(crate) fn validate_request_ids(ids: &[String]) -> Result<(), String> {
    if ids.len() > MAX_CANCEL_REQUESTS
        || ids.iter().map(String::len).sum::<usize>() > MAX_CANCEL_REQUESTS * MAX_REQUEST_ID_BYTES
    {
        return Err("Stop accepts at most 64 request IDs and 12800 bytes".into());
    }
    for id in ids {
        validate_id(id)?;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct RequestKey {
    scope: [u8; 32],
    request: [u8; 32],
}

#[derive(Default)]
struct RequestState {
    cancelled: AtomicBool,
    commit: Mutex<()>,
}

struct Active {
    state: Arc<RequestState>,
    leases: usize,
}

#[derive(Default)]
struct Registrations {
    active: HashMap<RequestKey, Active>,
    writing: HashMap<RequestKey, usize>,
}

pub(crate) struct CancelledRequests {
    root: PathBuf,
    registrations: Mutex<Registrations>,
}

pub(crate) struct RequestLease {
    store: Arc<CancelledRequests>,
    key: RequestKey,
    state: Arc<RequestState>,
}

impl RequestLease {
    pub fn cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::SeqCst)
    }
    pub fn signal(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let state = self.state.clone();
        Arc::new(move || state.cancelled.load(Ordering::SeqCst))
    }
    pub fn publish<T>(&self, operation: impl FnOnce(bool) -> T) -> T {
        let _commit = self.state.commit.lock();
        operation(self.cancelled())
    }
}

impl Drop for RequestLease {
    fn drop(&mut self) {
        let mut registrations = self.store.registrations.lock();
        if let Some(active) = registrations.active.get_mut(&self.key) {
            active.leases -= 1;
            if active.leases == 0 {
                registrations.active.remove(&self.key);
            }
        }
    }
}

pub(crate) struct CancellationReservation {
    store: Arc<CancelledRequests>,
    keys: Vec<RequestKey>,
    active: Vec<Arc<RequestState>>,
}

impl CancellationReservation {
    /// Wait only for synchronous publications already past their cutoff.
    pub fn settle_publications(&self) {
        for state in &self.active {
            let _committed = state.commit.lock();
        }
    }

    pub async fn persist(&self) -> Result<(), String> {
        for key in &self.keys {
            let path = self.store.path(*key);
            if read_marker(&path).await? {
                continue;
            }
            dsh_atomic_write::write_file_atomic(
                &path,
                MARKER,
                dsh_atomic_write::WriteFileAtomicOptions {
                    mode: 0o600,
                    dir_mode: Some(0o700),
                },
            )
            .await
            .map_err(|error| format!("cannot persist the stopped request: {error}"))?;
        }
        Ok(())
    }
}

impl Drop for CancellationReservation {
    fn drop(&mut self) {
        let mut registrations = self.store.registrations.lock();
        for key in &self.keys {
            if let Some(writing) = registrations.writing.get_mut(key) {
                *writing -= 1;
                if *writing == 0 {
                    registrations.writing.remove(key);
                }
            }
        }
    }
}

impl CancelledRequests {
    pub fn new(data_home: &Path) -> Arc<Self> {
        Arc::new(Self {
            root: data_home.join("control").join("cancelled-prompts-v1"),
            registrations: Mutex::new(Registrations::default()),
        })
    }

    fn path(&self, key: RequestKey) -> PathBuf {
        let mut digest = Sha256::new();
        digest.update(key.scope);
        digest.update(key.request);
        let request = hex(&digest.finalize());
        self.root
            .join(&request[..2])
            .join(format!("{request}.receipt"))
    }

    pub async fn begin(
        self: &Arc<Self>,
        scope: &RequestScope,
        request_id: &str,
    ) -> Result<RequestLease, String> {
        let key = scope.key(request_id)?;
        let state = {
            let mut registrations = self.registrations.lock();
            let writing = registrations.writing.contains_key(&key);
            let active = registrations.active.entry(key).or_insert_with(|| Active {
                state: Arc::new(RequestState::default()),
                leases: 0,
            });
            active.leases += 1;
            if writing {
                active.state.cancelled.store(true, Ordering::SeqCst);
            }
            active.state.clone()
        };
        let lease = RequestLease {
            store: self.clone(),
            key,
            state,
        };
        if read_marker(&self.path(key)).await? {
            lease.state.cancelled.store(true, Ordering::SeqCst);
        }
        Ok(lease)
    }

    /// Reserve all identities synchronously before any disk await.
    pub fn reserve(
        self: &Arc<Self>,
        scope: &RequestScope,
        ids: &[String],
    ) -> Result<CancellationReservation, String> {
        validate_request_ids(ids)?;
        let mut keys: Vec<_> = ids
            .iter()
            .map(|id| scope.key(id))
            .collect::<Result<_, _>>()?;
        keys.sort_by_key(|key| key.request);
        keys.dedup();
        let mut active = Vec::new();
        let mut registrations = self.registrations.lock();
        for key in &keys {
            *registrations.writing.entry(*key).or_default() += 1;
            if let Some(entry) = registrations.active.get(key) {
                entry.state.cancelled.store(true, Ordering::SeqCst);
                active.push(entry.state.clone());
            }
        }
        drop(registrations);
        Ok(CancellationReservation {
            store: self.clone(),
            keys,
            active,
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(DIGITS[(byte >> 4) as usize] as char);
        value.push(DIGITS[(byte & 15) as usize] as char);
    }
    value
}

async fn read_marker(path: &Path) -> Result<bool, String> {
    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("cannot inspect a stopped request: {error}")),
    };
    let mut bytes = [0u8; MARKER.len()];
    file.read_exact(&mut bytes)
        .await
        .map_err(|error| format!("invalid stopped-request receipt: {error}"))?;
    let mut extra = [0u8; 1];
    if &bytes != MARKER || file.read(&mut extra).await.map_err(|e| e.to_string())? != 0 {
        return Err("invalid stopped-request receipt".into());
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(id: &str, parent: Option<&str>, created_at: u64) -> SessionHeader {
        SessionHeader {
            version: dsh_session::SESSION_FORMAT_VERSION,
            id: dsh_session::session_id(id),
            created_at,
            cwd: None,
            parent_session: parent.map(dsh_session::session_id),
            is_seeded: false,
            origin: parent.map(|_| "subagent".into()),
            delegation_depth: None,
            agent_preset: None,
        }
    }

    fn temp() -> PathBuf {
        std::env::temp_dir().join(format!("dsh-cancellation-{}", uuid::Uuid::new_v4()))
    }

    async fn cleanup(root: PathBuf) {
        let temporary = std::env::temp_dir().canonicalize().unwrap();
        let resolved = root.canonicalize().unwrap();
        assert_eq!(resolved.parent(), Some(temporary.as_path()));
        assert!(
            resolved
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("dsh-cancellation-")
        );
        tokio::fs::remove_dir_all(resolved).await.unwrap();
    }

    #[tokio::test]
    async fn prearrival_and_restart_retries_stay_cancelled_without_history_or_resident_entries() {
        let root = temp();
        let scope = RequestScope::session(&header("main", None, 1)).unwrap();
        let store = CancelledRequests::new(&root);
        let stopped = store.reserve(&scope, &["pending-request".into()]).unwrap();
        let before_write = store.begin(&scope, "pending-request").await.unwrap();
        assert!(
            before_write.cancelled(),
            "a request arriving during the stop write observes the reservation"
        );
        stopped.persist().await.unwrap();
        drop(stopped);
        drop(before_write);
        assert!(store.registrations.lock().active.is_empty());
        assert!(store.registrations.lock().writing.is_empty());
        drop(store);
        let restarted = CancelledRequests::new(&root);
        let retry = restarted.begin(&scope, "pending-request").await.unwrap();
        assert!(retry.cancelled());
        assert!(retry.publish(|cancelled| cancelled));
        assert!(
            !restarted
                .begin(&scope, "explicit-new-request")
                .await
                .unwrap()
                .cancelled()
        );
        assert!(
            !restarted
                .begin(
                    &RequestScope::session(&header("main", None, 2)).unwrap(),
                    "pending-request"
                )
                .await
                .unwrap()
                .cancelled(),
            "a recreated session is a different authority"
        );
        drop(retry);
        assert!(restarted.registrations.lock().active.is_empty());
        cleanup(root).await;
    }

    #[tokio::test]
    async fn cancellation_is_exact_to_request_and_direct_parent_scope() {
        let root = temp();
        let store = CancelledRequests::new(&root);
        let parent = header("parent", None, 1);
        let child = header("child", Some("parent"), 2);
        let scope = RequestScope::subagent(&parent, &child).unwrap();
        assert!(RequestScope::subagent(&header("other", None, 1), &child).is_err());
        assert!(RequestScope::session(&child).is_err());
        let first = store.begin(&scope, "stopped").await.unwrap();
        let sibling = store.begin(&scope, "keep").await.unwrap();
        let stop = store.reserve(&scope, &["stopped".into()]).unwrap();
        stop.settle_publications();
        stop.persist().await.unwrap();
        assert!(first.cancelled());
        assert!(!sibling.cancelled());
        let other =
            RequestScope::subagent(&parent, &header("other-child", Some("parent"), 3)).unwrap();
        assert!(!store.begin(&other, "stopped").await.unwrap().cancelled());
        drop((first, sibling, stop));
        assert!(store.registrations.lock().active.is_empty());
        assert!(store.registrations.lock().writing.is_empty());
        cleanup(root).await;
    }

    #[test]
    fn request_input_is_bounded_and_never_appears_in_a_path() {
        let store = CancelledRequests::new(Path::new("test-root"));
        let scope = RequestScope::session(&header("../session", None, 1)).unwrap();
        let path = store.path(scope.key("../../escape\\name").unwrap());
        assert!(path.starts_with(&store.root));
        assert!(!path.to_string_lossy().contains("escape"));
        assert_eq!(path.file_name().unwrap().to_string_lossy().len(), 72);
        assert!(validate_request_ids(&vec!["id".into(); 65]).is_err());
        assert!(validate_request_ids(&["x".repeat(201)]).is_err());
        assert!(validate_request_ids(&[" ".into()]).is_err());
    }
}
