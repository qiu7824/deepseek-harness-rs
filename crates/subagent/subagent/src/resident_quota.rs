//! Slots count residency, including pending construction and awaited disposal.
use crate::error::SubagentError;
use std::{
    collections::HashMap,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

pub const DEFAULT_MAX_ACTIVE_SUBAGENTS: u64 = 8;
pub const DEFAULT_MAX_DEPTH: u64 = 1;

pub fn validate_capacity(value: u64) -> Result<(), SubagentError> {
    if !(1..=9_007_199_254_740_991).contains(&value) {
        return Err(SubagentError::new(
            "INVALID_MAX_ACTIVE_SUBAGENTS",
            "maxActiveSubagents must be a positive safe integer",
        ));
    }
    Ok(())
}
pub fn parse_capacity(value: Option<&serde_json::Value>) -> Result<u64, SubagentError> {
    let Some(value) = value else {
        return Ok(DEFAULT_MAX_ACTIVE_SUBAGENTS);
    };
    let value = value
        .as_u64()
        .or_else(|| {
            value
                .as_f64()
                .filter(|value| {
                    value.is_finite()
                        && value.fract() == 0.0
                        && *value >= 1.0
                        && *value <= 9_007_199_254_740_991.0
                })
                .map(|value| value as u64)
        })
        .ok_or_else(|| {
            SubagentError::new(
                "INVALID_MAX_ACTIVE_SUBAGENTS",
                "maxActiveSubagents must be a positive safe integer",
            )
        })?;
    validate_capacity(value)?;
    Ok(value)
}
#[derive(Default)]
pub(crate) struct Pool {
    used: AtomicU64,
}
pub(crate) struct ResidentPermit {
    pub(crate) pool: Arc<Pool>,
}
impl Drop for ResidentPermit {
    fn drop(&mut self) {
        self.pool.used.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Binding<T: ?Sized> {
    owner: Weak<T>,
    pool: Weak<Pool>,
}
pub(crate) struct ResidentQuota<T: ?Sized> {
    limit: AtomicU64,
    bindings: parking_lot::Mutex<HashMap<usize, Binding<T>>>,
}
impl<T: ?Sized> Default for ResidentQuota<T> {
    fn default() -> Self {
        Self {
            limit: AtomicU64::new(DEFAULT_MAX_ACTIVE_SUBAGENTS),
            bindings: Default::default(),
        }
    }
}
impl<T: ?Sized> ResidentQuota<T> {
    pub(crate) fn set_limit(&self, value: u64) -> Result<(), SubagentError> {
        validate_capacity(value)?;
        self.limit.store(value, Ordering::Release);
        Ok(())
    }
    pub(crate) fn bind(&self, owner: &Arc<T>, pool: &Arc<Pool>) {
        self.bindings.lock().insert(
            Arc::as_ptr(owner).cast::<()>() as usize,
            Binding {
                owner: Arc::downgrade(owner),
                pool: Arc::downgrade(pool),
            },
        );
    }
    pub(crate) fn reserve(&self, parent: &Arc<T>) -> Result<ResidentPermit, SubagentError> {
        let key = Arc::as_ptr(parent).cast::<()>() as usize;
        let pool = {
            let mut bindings = self.bindings.lock();
            bindings
                .retain(|_, value| value.owner.strong_count() > 0 && value.pool.strong_count() > 0);
            match bindings
                .get(&key)
                .filter(|value| {
                    value
                        .owner
                        .upgrade()
                        .is_some_and(|owner| Arc::ptr_eq(&owner, parent))
                })
                .and_then(|value| value.pool.upgrade())
            {
                Some(pool) => pool,
                None => {
                    let pool = Arc::new(Pool::default());
                    bindings.insert(
                        key,
                        Binding {
                            owner: Arc::downgrade(parent),
                            pool: Arc::downgrade(&pool),
                        },
                    );
                    pool
                }
            }
        };
        let limit = self.limit.load(Ordering::Acquire);
        pool.used.fetch_update(Ordering::AcqRel,Ordering::Acquire,|used|(used<limit).then_some(used+1))
            .map_err(|_|SubagentError::new("ACTIVATION_LIMIT_REACHED",format!("subagent limit reached (active child limit: {limit}); wait for an existing child to finish or use the current agents")))?;
        Ok(ResidentPermit { pool })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn continuable_descendants_share_slots_while_one_shot_parents_start_new_pools() {
        let quota = ResidentQuota::<u8>::default();
        quota.set_limit(2).unwrap();
        let root = Arc::new(0);
        let child = Arc::new(1);
        let one_shot = Arc::new(2);
        let first = quota.reserve(&root).unwrap();
        quota.bind(&child, &first.pool);
        let second = quota.reserve(&child).unwrap();
        assert!(quota.reserve(&root).is_err());
        assert!(quota.reserve(&child).is_err());
        let independent = quota.reserve(&one_shot).unwrap();
        assert!(!Arc::ptr_eq(&independent.pool, &first.pool));
        drop(second);
        assert!(quota.reserve(&child).is_ok());
    }
    #[test]
    fn changed_limits_preserve_residents_and_pending_permits_release_only_on_drop() {
        let quota = ResidentQuota::<u8>::default();
        let root = Arc::new(0);
        quota.set_limit(2).unwrap();
        let first = quota.reserve(&root).unwrap();
        let stopping = quota.reserve(&root).unwrap();
        quota.set_limit(1).unwrap();
        assert!(quota.reserve(&root).is_err());
        drop(stopping);
        assert!(quota.reserve(&root).is_err());
        quota.set_limit(3).unwrap();
        let next = quota.reserve(&root).unwrap();
        drop(first);
        drop(next);
        quota.set_limit(1).unwrap();
        assert!(quota.reserve(&root).is_ok());
    }
    #[test]
    fn invalid_limits_are_refused_and_concurrent_reservations_cannot_exceed_capacity() {
        for value in [
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!(9007199254740992_u64),
        ] {
            assert!(parse_capacity(Some(&value)).is_err());
        }
        let quota = Arc::new(ResidentQuota::<u8>::default());
        let owner = Arc::new(0);
        let entered = Arc::new(std::sync::Barrier::new(33));
        let release = Arc::new(std::sync::Barrier::new(33));
        let workers: Vec<_> = (0..32)
            .map(|_| {
                let quota = quota.clone();
                let owner = owner.clone();
                let entered = entered.clone();
                let release = release.clone();
                std::thread::spawn(move || {
                    let permit = quota.reserve(&owner).ok();
                    entered.wait();
                    release.wait();
                    permit.is_some()
                })
            })
            .collect();
        entered.wait();
        assert!(quota.reserve(&owner).is_err());
        release.wait();
        assert_eq!(
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .filter(|admitted| *admitted)
                .count(),
            8
        );
    }
}
