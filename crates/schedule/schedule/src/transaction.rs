//! Agent-scoped serialization for Schedule reads and durable mutations.
//! Rust port of `packages/schedule/schedule/src/transaction.ts`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

type Gate = tokio::sync::Mutex<()>;

/// Per-agent serialization gates (TS per-agent promise tails).
fn tails() -> &'static Mutex<HashMap<usize, Weak<Gate>>> {
    static TAILS: OnceLock<Mutex<HashMap<usize, Weak<Gate>>>> = OnceLock::new();
    TAILS.get_or_init(|| Mutex::new(HashMap::new()))
}

struct GateLease {
    key: usize,
    gate: Arc<Gate>,
}

impl GateLease {
    fn new(key: usize) -> Self {
        let mut tails = tails().lock().expect("tails");
        let gate = tails.get(&key).and_then(Weak::upgrade).unwrap_or_else(|| {
            let gate = Arc::new(Gate::new(()));
            tails.insert(key, Arc::downgrade(&gate));
            gate
        });
        Self { key, gate }
    }
}

impl Drop for GateLease {
    fn drop(&mut self) {
        let mut tails = tails().lock().expect("tails");
        // Admission and last-owner cleanup use the same lock. Pending callers
        // keep their lease even while awaiting the gate, including cancellation.
        if Arc::strong_count(&self.gate) == 1
            && tails
                .get(&self.key)
                .is_some_and(|gate| gate.ptr_eq(&Arc::downgrade(&self.gate)))
        {
            tails.remove(&self.key);
        }
    }
}

/// Run one complete Schedule transaction after its exact Agent's prior
/// transaction.
pub async fn run_schedule_transaction<T, F, Fut>(agent: &dyn dsh_agent::Agent, operation: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let key = (agent as *const dyn dsh_agent::Agent).cast::<()>() as usize;
    let lease = GateLease::new(key);
    let _guard = lease.gate.lock().await;
    operation().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn waiters_share_a_gate_and_last_owner_removes_it() {
        let first = GateLease::new(usize::MAX);
        let second = GateLease::new(usize::MAX);
        assert!(Arc::ptr_eq(&first.gate, &second.gate));
        let guard = first.gate.lock().await;
        assert!(second.gate.try_lock().is_err());
        drop(guard);
        drop(first);
        assert!(tails().lock().unwrap().contains_key(&usize::MAX));
        drop(second);
        assert!(!tails().lock().unwrap().contains_key(&usize::MAX));
    }

    #[tokio::test]
    async fn cancelled_waiter_releases_its_lease_without_splitting_serialization() {
        let key = usize::MAX - 1;
        let owner = GateLease::new(key);
        let guard = owner.gate.lock().await;
        let pending = GateLease::new(key);
        let task = tokio::spawn(async move {
            let _guard = pending.gate.lock().await;
        });
        tokio::task::yield_now().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let next = GateLease::new(key);
        assert!(Arc::ptr_eq(&owner.gate, &next.gate));
        drop(next);
        drop(guard);
        drop(owner);
        assert!(!tails().lock().unwrap().contains_key(&key));
    }
}
