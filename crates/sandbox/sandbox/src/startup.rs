//! Shared startup deadlines. Only runner-owned progress extends initialization;
//! application output is never evidence that a sandbox is ready or progressing.
use crate::SandboxStartup;
use std::time::Duration;

// Includes the native setup helper's 900s budget and both 30s status probes.
pub const PREPARATION_TIMEOUT: Duration = Duration::from_secs(1200);
pub const STARTUP_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
pub const STARTUP_MAX_TIMEOUT: Duration = Duration::from_secs(1800);

#[derive(Default)]
pub struct StartupDeadline {
    last_progress: Duration,
}

impl StartupDeadline {
    /// `elapsed` uses the consumer's clock, including Tokio's test clock.
    pub fn check(&mut self, startup: &SandboxStartup, elapsed: Duration) -> Result<(), String> {
        if startup
            .is_ready()
            .map_err(|error| format!("[SANDBOX_SETUP_FAILED] {error}"))?
        {
            return Ok(());
        }
        if startup
            .take_progress()
            .map_err(|error| format!("[SANDBOX_SETUP_FAILED] {error}"))?
        {
            self.last_progress = elapsed;
        }
        let phase = startup.phase();
        // A shared ACL lease can be occupied by another healthy migration.
        // Its wait is bounded independently, and is not command execution.
        let idle_limit = if phase == "acl_lock" {
            PREPARATION_TIMEOUT
        } else {
            STARTUP_IDLE_TIMEOUT
        };
        let idle = elapsed.saturating_sub(self.last_progress);
        if elapsed >= STARTUP_MAX_TIMEOUT || idle >= idle_limit {
            return Err(format!(
                "[SANDBOX_SETUP_TIMEOUT] phase={phase}; startup elapsed={}s, no progress={}s; limits: idle={}s, total={}s; command readiness not confirmed; durable permission migration can resume on retry",
                elapsed.as_secs(),
                idle.as_secs(),
                idle_limit.as_secs(),
                STARTUP_MAX_TIMEOUT.as_secs()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[test]
    fn healthy_progress_outlives_the_old_timeout_but_stalls_and_total_budget_are_bounded() {
        let progress = Arc::new(AtomicBool::new(false));
        let signal = progress.clone();
        let startup = SandboxStartup::new(|| Ok(false), || Ok(false))
            .with_phase(|| Ok("private_permissions".into()))
            .with_progress(move || Ok(signal.swap(false, Ordering::SeqCst)));
        let mut deadline = StartupDeadline::default();
        for seconds in [60, 120, 180, 240] {
            progress.store(true, Ordering::SeqCst);
            assert!(
                deadline
                    .check(&startup, Duration::from_secs(seconds))
                    .is_ok()
            );
        }
        assert!(deadline.check(&startup, Duration::from_secs(359)).is_ok());
        assert!(
            deadline
                .check(&startup, Duration::from_secs(360))
                .unwrap_err()
                .contains("private_permissions")
        );
        progress.store(true, Ordering::SeqCst);
        assert!(deadline.check(&startup, STARTUP_MAX_TIMEOUT).is_err());
    }

    #[test]
    fn legacy_runner_without_progress_keeps_the_short_stall_deadline() {
        let startup = SandboxStartup::new(|| Ok(false), || Ok(false));
        let mut deadline = StartupDeadline::default();
        assert!(deadline.check(&startup, Duration::from_secs(119)).is_ok());
        assert!(deadline.check(&startup, Duration::from_secs(120)).is_err());
    }

    #[test]
    fn acl_lease_wait_has_a_separate_finite_budget() {
        let startup =
            SandboxStartup::new(|| Ok(false), || Ok(false)).with_phase(|| Ok("acl_lock".into()));
        let mut deadline = StartupDeadline::default();
        assert!(deadline.check(&startup, Duration::from_secs(899)).is_ok());
        assert!(deadline.check(&startup, PREPARATION_TIMEOUT).is_err());
    }
}
