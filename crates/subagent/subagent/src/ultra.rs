//! Root-scoped admission and durable accounting for Ultra delegated work.
use crate::{SubagentError, SubagentRun};
use dsh_agent::Agent;
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Weak},
    time::{SystemTime, UNIX_EPOCH},
};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn enabled(agent: &dyn Agent) -> bool {
    let name = dsh_agent::model_selection::model_selection_service_name(agent.ctx());
    agent
        .ctx()
        .get_typed::<Arc<parking_lot::Mutex<dsh_agent::ModelSelectionRef>>>(&name, false)
        .and_then(|value| {
            let state = value.lock();
            state.assembled.clone().or_else(|| state.resolved_current())
        })
        .map(|s| s.execution_mode == dsh_llm::ExecutionMode::Ultra)
        .unwrap_or(agent.options().execution_mode == dsh_llm::ExecutionMode::Ultra)
}
pub fn child_marker(agent: &dyn Agent) -> bool {
    agent.session().events().iter().any(|e| {
        e.type_ == "execution/ultra-child"
            && e.data["childId"].as_str() == Some(agent.id().as_str())
    })
}
pub fn mark_child(parent: &dyn Agent, child: &dyn Agent) -> Result<(), String> {
    if enabled(parent) && !child_marker(child) {
        child.session().append(
            "execution/ultra-child",
            json!({"childId":child.id().as_str(),"rootId":parent.id().as_str()}),
            None,
        )?;
    }
    Ok(())
}
fn tokens(agent: &dyn Agent) -> u64 {
    tokens_since(agent, 0)
}
fn tokens_since(agent: &dyn Agent, from: u64) -> u64 {
    let events = agent.session().events();
    let own = events
        .iter()
        .rposition(|e| {
            e.type_ == "execution/ultra-child"
                && e.data["childId"].as_str() == Some(agent.id().as_str())
        })
        .map(|n| n + 1)
        .unwrap_or(0);
    events[own..]
        .iter()
        .filter(|e| e.type_ == "assistant/message" && e.seq.get() >= from)
        .map(|e| {
            let value = &e.data["usage"];
            value["totalTokens"].as_u64().unwrap_or_else(|| {
                [
                    "inputTokens",
                    "outputTokens",
                    "cacheReadTokens",
                    "cacheWriteTokens",
                ]
                .iter()
                .filter_map(|key| value[*key].as_u64())
                .fold(0, u64::saturating_add)
            })
        })
        .fold(0, u64::saturating_add)
}
#[derive(Default)]
struct Tree {
    epoch: String,
    owner: Option<Weak<dyn Agent>>,
    active: HashSet<String>,
    closed: bool,
    remote: HashMap<String, Weak<dyn SubagentRun>>,
    local: HashMap<String, (Weak<dyn Agent>, u64)>,
}
#[derive(Default)]
pub struct UltraControl {
    trees: parking_lot::Mutex<HashMap<String, Tree>>,
    changed: tokio::sync::Notify,
}
impl cordis::Service for UltraControl {
    fn service_name(&self) -> &'static str {
        "ultraControl"
    }
}
impl UltraControl {
    pub fn install(ctx: &cordis::Context) -> Arc<Self> {
        let control = Arc::new(Self::default());
        ctx.register_service(control.clone());
        control
    }
    pub fn get(ctx: &cordis::Context) -> Option<Arc<Self>> {
        ctx.get_typed::<Arc<Self>>("ultraControl", false)
            .map(|s| s.as_ref().clone())
    }
    pub async fn admit_wait(
        self: &Arc<Self>,
        parent: &Arc<dyn Agent>,
        child_id: &str,
        signal: &crate::types::SubagentSignal,
    ) -> Result<Option<UltraPermit>, SubagentError> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if signal() {
                return Err(SubagentError::new("CANCELLED", "子任务准入已取消"));
            }
            let notified = self.changed.notified();
            match self.admit(parent, child_id) {
                Err(error) if error.code == "ULTRA_CONCURRENCY_LIMIT" => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(SubagentError::new(
                            "ULTRA_CONCURRENCY_LIMIT",
                            "等待子任务空位超过 30 秒；收取现有结果后再继续",
                        ));
                    }
                    tokio::select! {_=notified=>(),_=tokio::time::sleep(std::time::Duration::from_millis(100))=>()}
                }
                result => return result,
            }
        }
    }
    pub fn admit(
        self: &Arc<Self>,
        parent: &Arc<dyn Agent>,
        child_id: &str,
    ) -> Result<Option<UltraPermit>, SubagentError> {
        if child_marker(parent.as_ref()) {
            return Err(SubagentError::new(
                "ULTRA_DEPTH_LIMIT",
                "Ultra 子任务不能继续创建子任务",
            ));
        }
        if !enabled(parent.as_ref()) {
            return Ok(None);
        }
        let mut trees = self.trees.lock();
        let all_events = parent.session().events();
        let mut epoch = all_events
            .iter()
            .rev()
            .find(|e| e.type_ == "turn/start")
            .map(|e| e.seq.to_string())
            .unwrap_or_else(|| "initial".into());
        let tree = trees.entry(parent.id().to_string()).or_default();
        if !tree.active.is_empty() {
            epoch = tree.epoch.clone();
        }
        if tree.epoch != epoch {
            tree.epoch = epoch.clone();
            tree.closed = false;
        }
        if tree.owner.as_ref().and_then(Weak::upgrade).is_none() {
            tree.owner = Some(Arc::downgrade(parent));
            tree.closed = false;
            tree.active.clear();
        }
        if tree.closed {
            return Err(SubagentError::new(
                "ULTRA_CLOSED",
                "该任务的子任务准入已关闭",
            ));
        }
        if tree.active.contains(child_id) {
            return Ok(None);
        }
        if tree.active.len() >= 3 {
            return Err(SubagentError::new(
                "ULTRA_CONCURRENCY_LIMIT",
                "已有三个子任务运行；等待现有结果后再分配新任务",
            ));
        }
        let events = all_events
            .iter()
            .filter(|e| e.data["epoch"].as_str() == Some(epoch.as_str()))
            .collect::<Vec<_>>();
        let admitted = events
            .iter()
            .filter(|e| e.type_ == "execution/ultra-admitted")
            .count();
        if admitted >= 36 {
            return Err(SubagentError::new(
                "ULTRA_RUN_LIMIT",
                "Ultra 子任务累计运行次数已达到 36 次",
            ));
        }
        let created = events
            .iter()
            .filter(|e| e.type_ == "execution/ultra-admitted")
            .filter_map(|e| e.data["childId"].as_str())
            .collect::<HashSet<_>>();
        if !created.contains(child_id) && created.len() >= 12 {
            return Err(SubagentError::new(
                "ULTRA_CREATE_LIMIT",
                "Ultra 子任务累计创建数已达到 12 个",
            ));
        }
        let start = events
            .iter()
            .find(|e| e.type_ == "execution/ultra-admitted")
            .and_then(|e| e.data["startedAt"].as_u64())
            .unwrap_or_else(now);
        let timeout = std::env::var("DSH_ULTRA_TIMEOUT_SECONDS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(1800);
        if now().saturating_sub(start) >= timeout {
            return Err(SubagentError::new(
                "ULTRA_TIME_LIMIT",
                "Ultra 执行时间预算已耗尽",
            ));
        }
        let child_tokens = events
            .iter()
            .filter(|e| e.type_ == "execution/ultra-settled")
            .filter_map(|e| e.data["tokens"].as_u64())
            .fold(0, u64::saturating_add);
        if let Some(limit) = std::env::var("DSH_ULTRA_TOKEN_BUDGET")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
        {
            if events.iter().any(|event| {
                event.type_ == "execution/ultra-settled" && event.data["tokens"].is_null()
            }) || tree.remote.keys().any(|id| !tree.local.contains_key(id))
            {
                return Err(SubagentError::new(
                    "ULTRA_USAGE_UNAVAILABLE",
                    "子任务用量尚未完整提供，无法继续在 Token 预算内准入",
                ));
            }
            let active = tree
                .local
                .values()
                .filter_map(|(child, baseline)| {
                    child
                        .upgrade()
                        .map(|child| tokens(child.as_ref()).saturating_sub(*baseline))
                })
                .fold(0, u64::saturating_add);
            if tokens_since(parent.as_ref(), epoch.parse().unwrap_or(0))
                .saturating_add(child_tokens)
                .saturating_add(active)
                >= limit
            {
                return Err(SubagentError::new(
                    "ULTRA_TOKEN_LIMIT",
                    "Ultra Token 预算已耗尽",
                ));
            }
        }
        parent.session().append("execution/ultra-admitted",json!({"childId":child_id,"epoch":epoch,"startedAt":start,"deadline":start.saturating_add(timeout)}),None).map_err(|e|SubagentError::new("ULTRA_PERSISTENCE",e))?;
        tree.active.insert(child_id.into());
        Ok(Some(UltraPermit {
            control: self.clone(),
            root: Arc::downgrade(parent),
            root_id: parent.id().to_string(),
            child_id: child_id.into(),
            epoch,
            baseline_tokens: 0,
            deadline: start.saturating_add(timeout),
            child: None,
        }))
    }
    pub async fn close(&self, parent: &Arc<dyn Agent>) {
        let runs = {
            let mut trees = self.trees.lock();
            let Some(tree) = trees.get_mut(parent.id().as_str()) else {
                return;
            };
            tree.closed = true;
            tree.remote
                .values()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>()
        };
        for run in runs {
            let _ = run.dispose().await;
        }
        self.changed.notify_waiters();
    }
    fn token_limit_reached(&self, root_id: &str) -> bool {
        let Some(limit) = std::env::var("DSH_ULTRA_TOKEN_BUDGET")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
        else {
            return false;
        };
        let mut trees = self.trees.lock();
        let Some(tree) = trees.get_mut(root_id) else {
            return false;
        };
        let Some(root) = tree.owner.as_ref().and_then(Weak::upgrade) else {
            return false;
        };
        let epoch = tree.epoch.clone();
        let settled = root
            .session()
            .events()
            .iter()
            .filter(|event| {
                event.type_ == "execution/ultra-settled"
                    && event.data["epoch"].as_str() == Some(&epoch)
            })
            .filter_map(|event| event.data["tokens"].as_u64())
            .fold(0, u64::saturating_add);
        let active = tree
            .local
            .values()
            .filter_map(|(child, baseline)| {
                child
                    .upgrade()
                    .map(|child| tokens(child.as_ref()).saturating_sub(*baseline))
            })
            .fold(0, u64::saturating_add);
        let used = tokens_since(root.as_ref(), epoch.parse().unwrap_or(0))
            .saturating_add(settled)
            .saturating_add(active);
        if used < limit {
            return false;
        }
        let notify = !tree.closed;
        tree.closed = true;
        drop(trees);
        if notify {
            let _ = root.session().append(
                "execution/ultra-budget-exhausted",
                json!({"reason":"tokens","used":used,"limit":limit,"epoch":epoch}),
                None,
            );
            self.changed.notify_waiters();
        }
        true
    }
}
pub struct UltraPermit {
    control: Arc<UltraControl>,
    root: Weak<dyn Agent>,
    root_id: String,
    child_id: String,
    epoch: String,
    baseline_tokens: u64,
    deadline: u64,
    child: Option<Weak<dyn Agent>>,
}
impl UltraPermit {
    pub fn bind(&mut self, child: &Arc<dyn Agent>) {
        self.baseline_tokens = tokens(child.as_ref());
        self.child = Some(Arc::downgrade(child));
        if let Some(tree) = self.control.trees.lock().get_mut(&self.root_id) {
            tree.local.insert(
                self.child_id.clone(),
                (Arc::downgrade(child), self.baseline_tokens),
            );
        }
        let child = Arc::downgrade(child);
        let deadline = self.deadline;
        let control = self.control.clone();
        let root_id = self.root_id.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let Some(child) = child.upgrade() else { break };
                if now() >= deadline || control.token_limit_reached(&root_id) {
                    child.cancel(dsh_session::AgentCancelCause::Parent, None);
                    break;
                }
                if child.status() == dsh_agent::AgentStatus::Idle
                    && child.inbox().next_turn().is_empty()
                {
                    break;
                }
            }
        });
    }
    pub fn watch_run(mut self, run: Arc<dyn SubagentRun>) {
        if let Some(child) = run.local_agent() {
            self.bind(&child)
        }
        if let Some(tree) = self.control.trees.lock().get_mut(&self.root_id) {
            tree.remote
                .insert(self.child_id.clone(), Arc::downgrade(&run));
        }
        let timeout = std::time::Duration::from_secs(self.deadline.saturating_sub(now()).max(1));
        tokio::spawn(async move {
            if tokio::time::timeout(timeout, run.result()).await.is_err() {
                let _ = run.dispose().await;
            }
            drop(self);
        });
    }
}
impl Drop for UltraPermit {
    fn drop(&mut self) {
        let mut trees = self.control.trees.lock();
        if let Some(root) = self.root.upgrade() {
            let count = self
                .child
                .as_ref()
                .and_then(Weak::upgrade)
                .map(|child| tokens(child.as_ref()).saturating_sub(self.baseline_tokens));
            let _ = root.session().append(
                "execution/ultra-settled",
                json!({"childId":self.child_id,"epoch":self.epoch,"tokens":count}),
                None,
            );
        }
        if let Some(tree) = trees.get_mut(&self.root_id) {
            tree.active.remove(&self.child_id);
            tree.remote.remove(&self.child_id);
            tree.local.remove(&self.child_id);
        }
        drop(trees);
        self.control.changed.notify_waiters();
    }
}
