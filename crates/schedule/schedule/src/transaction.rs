//! Agent-scoped serialization for Schedule reads and durable mutations.
//! Rust port of `packages/schedule/schedule/src/transaction.ts`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Per-agent serialization gates (TS per-agent promise tails).
fn tails() -> &'static Mutex<HashMap<usize, Arc<tokio::sync::Mutex<()>>>> {
    static TAILS: OnceLock<Mutex<HashMap<usize, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    TAILS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Run one complete Schedule transaction after its exact Agent's prior
/// transaction.
pub async fn run_schedule_transaction<T, F, Fut>(agent: &dyn dsh_agent::Agent, operation: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let key = (agent as *const dyn dsh_agent::Agent).cast::<()>() as usize;
    let gate = tails()
        .lock()
        .expect("tails")
        .entry(key)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = gate.lock().await;
    operation().await
}
