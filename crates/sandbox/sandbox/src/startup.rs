//! Shared startup deadlines. Only runner-owned progress extends initialization;
//! application output is never evidence that a sandbox is ready or progressing.
use crate::SandboxStartup;
use std::time::Duration;

// Cold provisioning and effective-policy preparation each have a 900s
// deadline. Both status probes and a small orchestration margin also fit.
pub const PREPARATION_TIMEOUT: Duration = Duration::from_secs(1890);
pub const STARTUP_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
pub const STARTUP_MAX_TIMEOUT: Duration = Duration::from_secs(1800);
pub const ENVIRONMENT_VALIDATION_TIMEOUT_MS: u64 =
    (PREPARATION_TIMEOUT.as_secs() + STARTUP_MAX_TIMEOUT.as_secs() + 135 + 30) * 1000;
pub const ENVIRONMENT_INITIALIZATION_TIMEOUT_MS: u64 =
    (PREPARATION_TIMEOUT.as_secs() + 30) * 1000;

pub async fn prepare_future(
    preparation: futures::future::BoxFuture<'static, Result<(), String>>,
    signal: Option<std::sync::Arc<dyn Fn() -> bool + Send + Sync>>,
) -> Result<(), String> {
    let cancelled = || signal.as_ref().is_some_and(|signal| signal());
    let cancellation_error = || "[SANDBOX_SETUP_CANCELLED] environment preparation cancelled; requested command not dispatched".to_string();
    if cancelled() { return Err(cancellation_error()); }
    tokio::select! {
        biased;
        _ = async { loop { if cancelled() { break; } tokio::time::sleep(Duration::from_millis(15)).await; } } => Err(cancellation_error()),
        result = tokio::time::timeout(PREPARATION_TIMEOUT, preparation) =>
            result.map_err(|_| "[SANDBOX_SETUP_TIMEOUT] environment preparation deadline exceeded; requested command not dispatched".to_string())?,
    }
}

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

    #[tokio::test(start_paused = true)]
    async fn cancelled_preparation_drops_inflight_work_without_dispatch() {
        struct DropFlag(Arc<AtomicBool>);
        impl Drop for DropFlag { fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); } }
        let cancelled = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let ready = Arc::new(tokio::sync::Notify::new());
        let guard = DropFlag(dropped.clone());
        let entered = ready.clone();
        let cancellation = cancelled.clone();
        let task = tokio::spawn(prepare_future(Box::pin(async move {
            let _guard = guard;
            entered.notify_one();
            std::future::pending().await
        }), Some(Arc::new(move || cancellation.load(Ordering::SeqCst)))));
        ready.notified().await;
        cancelled.store(true, Ordering::SeqCst);
        assert!(task.await.unwrap().unwrap_err().contains("SANDBOX_SETUP_CANCELLED"));
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test(start_paused = true)]
    async fn preparation_deadline_is_finite_and_pre_cancelled_work_is_never_polled() {
        let polled = Arc::new(AtomicBool::new(false));
        let flag = polled.clone();
        let error = prepare_future(Box::pin(async move {
            flag.store(true, Ordering::SeqCst); Ok(())
        }), Some(Arc::new(|| true))).await.unwrap_err();
        assert!(error.contains("SANDBOX_SETUP_CANCELLED"));
        assert!(!polled.load(Ordering::SeqCst));
        let began = tokio::time::Instant::now();
        let error = prepare_future(Box::pin(std::future::pending()), None).await.unwrap_err();
        assert!(error.contains("SANDBOX_SETUP_TIMEOUT"));
        assert_eq!(began.elapsed(), PREPARATION_TIMEOUT);
    }

    #[test]
    fn enclosing_tool_budgets_cover_preparation_startup_probe_and_cleanup() {
        assert!(PREPARATION_TIMEOUT >= Duration::from_secs(900 + 900 + 30 + 30));
        assert!(Duration::from_millis(ENVIRONMENT_INITIALIZATION_TIMEOUT_MS) > PREPARATION_TIMEOUT);
        assert!(Duration::from_millis(ENVIRONMENT_VALIDATION_TIMEOUT_MS)
            > PREPARATION_TIMEOUT + STARTUP_MAX_TIMEOUT + Duration::from_secs(135));
    }

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
