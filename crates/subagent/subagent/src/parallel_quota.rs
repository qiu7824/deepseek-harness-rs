//! Running-turn limits are independent of resident child limits.
use dsh_agent::AgentRegistry;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

pub struct ParallelQuota {
    limit: AtomicU64,
    running: parking_lot::Mutex<HashMap<String, u64>>,
}
impl Default for ParallelQuota {
    fn default() -> Self {
        Self {
            limit: AtomicU64::new(10),
            running: Default::default(),
        }
    }
}
pub(crate) struct Permit {
    quota: Arc<ParallelQuota>,
    root: String,
    release_on_drop: bool,
}
impl Drop for Permit {
    fn drop(&mut self) {
        if !self.release_on_drop {
            return;
        }
        let mut running = self.quota.running.lock();
        if let Some(count) = running.get_mut(&self.root) {
            *count -= 1;
            if *count == 0 {
                running.remove(&self.root);
            }
        }
    }
}
impl ParallelQuota {
    pub fn set_limit(&self, limit: u64) -> Result<(), String> {
        if !(1..=64).contains(&limit) {
            return Err("subagent maxParallel must be between 1 and 64".into());
        }
        self.limit.store(limit, Ordering::Release);
        Ok(())
    }
    fn reserve(self: &Arc<Self>, root: String) -> Result<Permit, String> {
        let mut running = self.running.lock();
        let count = running.entry(root.clone()).or_default();
        if *count >= self.limit.load(Ordering::Acquire) {
            return Err(
                "subagent maxParallel exceeded; wait for running children to finish".into(),
            );
        }
        *count += 1;
        Ok(Permit {
            quota: self.clone(),
            root,
            release_on_drop: true,
        })
    }
    pub(crate) fn reserve_external(
        self: &Arc<Self>,
        parent: &Arc<dyn dsh_agent::Agent>,
    ) -> Result<Permit, String> {
        self.reserve(root_for(parent.as_ref())?)
    }
    pub fn install(ctx: &cordis::Context) -> Arc<Self> {
        let quota = Arc::new(Self::default());
        let weak = Arc::downgrade(&quota);
        ctx.register_service(Arc::new(dsh_agent::AgentRunAdmission {
            admit: Arc::new(move |agent| {
                if agent.session().header().origin.as_deref() != Some("subagent") {
                    return Ok(Box::new(()));
                }
                let Some(quota) = weak.upgrade() else {
                    return Err("subagent run admission unavailable".into());
                };
                Ok(Box::new(quota.reserve(root_for(agent)?)?))
            }),
        }));
        quota
    }
}
fn root_for(agent: &dyn dsh_agent::Agent) -> Result<String, String> {
    let registry = agent.ctx().get_typed::<Arc<AgentRegistry>>("agents", false);
    let mut visited = HashSet::new();
    let mut id = agent.id().clone();
    let mut header = agent.session().header().clone();
    loop {
        if !visited.insert(id.to_string()) {
            return Err("cyclic subagent lineage".into());
        }
        if header.origin.as_deref() != Some("subagent") {
            return Ok(id.to_string());
        }
        let parent = header
            .parent_session
            .clone()
            .ok_or("subagent has no parent")?;
        match registry.as_ref().and_then(|registry| registry.get(&parent)) {
            Some(next) => {
                id = next.id().clone();
                header = next.session().header().clone();
            }
            None => return Ok(parent.to_string()),
        }
    }
}

/// The lifecycle observer drives result even when the caller drops its handle.
/// A failed disposal keeps the slot occupied: completion alone cannot prove
/// that an external process has exited.
pub(crate) struct ExternalRun {
    inner: Arc<dyn crate::SubagentRun>,
    permit: parking_lot::Mutex<Option<Permit>>,
    disposal: tokio::sync::OnceCell<Result<(), String>>,
    result: tokio::sync::OnceCell<Result<crate::SubagentResult, String>>,
}
impl ExternalRun {
    pub(crate) fn wrap(
        inner: Arc<dyn crate::SubagentRun>,
        mut permit: Permit,
    ) -> Arc<dyn crate::SubagentRun> {
        permit.release_on_drop = false;
        Arc::new(Self {
            inner,
            permit: parking_lot::Mutex::new(Some(permit)),
            disposal: tokio::sync::OnceCell::new(),
            result: tokio::sync::OnceCell::new(),
        })
    }
}
#[async_trait::async_trait]
impl crate::SubagentRun for ExternalRun {
    fn id(&self) -> &dsh_session::SessionId {
        self.inner.id()
    }
    fn local_agent(&self) -> Option<Arc<dyn dsh_agent::Agent>> {
        self.inner.local_agent()
    }
    async fn result(&self) -> Result<crate::SubagentResult, String> {
        self.result
            .get_or_init(|| async {
                let result = self.inner.result().await;
                self.dispose().await?;
                result
            })
            .await
            .clone()
    }
    async fn dispose(&self) -> Result<(), String> {
        self.disposal
            .get_or_init(|| async {
                self.inner.dispose().await?;
                if let Some(mut permit) = self.permit.lock().take() {
                    permit.release_on_drop = true;
                }
                Ok(())
            })
            .await
            .clone()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_limits_isolate_roots_and_release_after_settlement() {
        let q = Arc::new(ParallelQuota::default());
        q.set_limit(2).unwrap();
        let a = q.reserve("a".into()).unwrap();
        let b = q.reserve("a".into()).unwrap();
        assert!(q.reserve("a".into()).is_err());
        let other = q.reserve("b".into()).unwrap();
        q.set_limit(1).unwrap();
        drop(a);
        assert!(q.reserve("a".into()).is_err());
        drop(b);
        let next = q.reserve("a".into()).unwrap();
        drop(next);
        drop(other);
        assert!(q.running.lock().is_empty());
        assert!(q.set_limit(0).is_err());
    }
}
