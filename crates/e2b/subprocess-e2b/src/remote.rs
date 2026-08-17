//! Shared remote-control helpers for the E2B subprocess adapter: poll
//! ticks and the one tolerant process-group signal used by the teardown
//! ladder. Rust port of `remote.ts`.

use std::collections::HashMap;
use std::sync::Arc;

use dsh_e2b::{E2bCommandOptions, E2bSandbox, E2bSdkError, e2b_control_envs};
use dsh_subprocess::SubprocessAbort;

use crate::environment::tolerated_teardown_error;

/// Wait one poll interval or until the signal aborts (TS `waitTick`).
/// `true` after a full tick, `false` when aborted first.
pub async fn wait_tick(poll_ms: u64, signal: Option<&SubprocessAbort>) -> bool {
    if signal.is_some_and(|signal| signal()) {
        return false;
    }
    let sleep = tokio::time::sleep(std::time::Duration::from_millis(poll_ms));
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            _ = &mut sleep => return true,
            _ = tokio::task::yield_now() => {
                if signal.is_some_and(|signal| signal()) {
                    return false;
                }
            }
        }
    }
}

/// Signal remote process groups, tolerating the shared teardown outcomes:
/// a nonzero `kill` (groups already gone) and a disappeared sandbox
/// (TS `signalRemoteGroups`).
pub async fn signal_remote_groups(
    sandbox: &Arc<dyn E2bSandbox>,
    envs: HashMap<String, String>,
    groups: &[i64],
    signal: &str,
) -> Result<(), E2bSdkError> {
    let targets = groups
        .iter()
        .map(|group| format!("-{group}"))
        .collect::<Vec<_>>()
        .join(" ");
    let result = sandbox
        .run(
            &format!("kill -{signal} -- {targets}"),
            &E2bCommandOptions::with_envs(e2b_control_envs(envs)),
        )
        .await;
    match result {
        Ok(_) => Ok(()),
        Err(error) if tolerated_teardown_error(&error) => Ok(()),
        Err(error) => Err(error),
    }
}
