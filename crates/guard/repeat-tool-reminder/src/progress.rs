//! Bounded result-aware loop guard. A blocked path can be repaired or superseded,
//! while unrelated tools and genuine background polling remain available.
use cordis::{Context, arc, downcast_arc};
use dsh_agent::AgentPreStepPayload;
use dsh_llm::MessageSource;
use dsh_tools::{PreToolDecision, ToolExecution, ToolExecutionResult};
use parking_lot::Mutex;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    hash::{Hash, Hasher},
    sync::Arc,
};

fn hash(value: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}
fn signature(name: &str, args: &Value) -> u64 {
    hash((name, super::json_stringify(&super::sort_json_value(args))))
}
fn poller(name: &str, args: &Value) -> bool {
    matches!(
        name,
        "job_output" | "job_list" | "wait" | "wait_agent" | "terminal_read"
    ) || name.ends_with("_poll")
        || name.ends_with("_get_result")
        || name == "agent_team" && matches!(args["action"].as_str(), Some("wait" | "status"))
}
fn repair(name: &str) -> bool {
    matches!(
        name,
        "write" | "edit" | "write_file" | "patch" | "apply_patch"
    )
}
fn result_signature(result: &ToolExecutionResult) -> Option<u64> {
    // Hash complete bytes; a prefix would mistake long, changing outputs for a loop.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    result.is_error.hash(&mut h);
    if let Some(value) = &result.value {
        let mut value = value.clone();
        if let Some(object) = value.as_object_mut() {
            for field in [
                "durationMs",
                "elapsedMs",
                "checkedAt",
                "timestamp",
                "requestId",
                "callId",
            ] {
                object.remove(field);
            }
        }
        super::json_stringify(&super::sort_json_value(&value)).hash(&mut h);
    } else if let Some(error) = &result.error {
        error.message.hash(&mut h);
    } else {
        let mut any = false;
        for block in &result.content {
            match block {
                dsh_llm::ContentBlock::Text { text } => {
                    text.hash(&mut h);
                    any = true;
                }
                _ => return None, // Images can change without changing their text envelope.
            }
        }
        if !any {
            return None;
        }
    }
    Some(h.finish())
}

#[derive(Default)]
struct History {
    rows: VecDeque<(u64, u64)>,
    blocked: HashSet<u64>,
    last_repair: Option<(u64, u64)>,
    seen_seq: Option<u64>,
}
impl History {
    fn observe(&mut self, sig: u64, result: u64, changed: bool) {
        if changed && self.last_repair != Some((sig, result)) {
            self.rows.clear();
            self.blocked.clear();
            self.last_repair = Some((sig, result));
        }
        if self.rows.len() == 64 {
            self.rows.pop_front();
        }
        self.rows.push_back((sig, result));
        for period in 1..=4 {
            let count = period * 5;
            if self.rows.len() < count {
                continue;
            }
            let offset = self.rows.len() - count;
            if (period..count).all(|i| self.rows[offset + i] == self.rows[offset + i % period]) {
                self.blocked.extend(
                    self.rows
                        .iter()
                        .skip(self.rows.len() - period)
                        .map(|(s, _)| *s),
                );
            }
        }
    }
}
type State = Arc<Mutex<HashMap<String, History>>>;
fn with_history<T>(
    state: &State,
    execution: &ToolExecution,
    action: impl FnOnce(&mut History) -> T,
) -> Option<T> {
    let owner = execution.agent.as_ref()?.id().as_str().to_string();
    let session = execution.agent.as_ref()?.session();
    let mut state = state.lock();
    // Never discard a running owner's blocked history merely to admit a new owner.
    if state.len() >= 512 && !state.contains_key(&owner) {
        return None;
    }
    let history = state.entry(owner).or_default();
    let until = session.seq().get();
    let mut policy_changed = false;
    if session
        .visit_events(history.seen_seq.unwrap_or(until), Some(until), |event| {
            policy_changed |= matches!(
                event.type_.as_str(),
                "sandbox/mode"
                    | "sandbox/roots-revoked"
                    | "permission/preset"
                    | "permission/options"
                    | "terminal/permissions-revoked"
            );
            Ok(true)
        })
        .is_ok()
    {
        if policy_changed {
            *history = History::default();
        }
        history.seen_seq = Some(until);
    }
    Some(action(history))
}

pub async fn install(ctx: &Context) {
    let state: State = Default::default();
    let before = state.clone();
    ctx.on("tools/pre-execute", Arc::new(move |_, args| {
        let state = before.clone();
        Box::pin(async move {
            let next = args.last().and_then(downcast_arc::<cordis::NextFn>);
            let execution = args.first().and_then(downcast_arc::<Arc<ToolExecution>>);
            if let Some(execution) = execution {
                if !poller(&execution.name, &execution.arguments) && with_history(&state, &execution,
                    |h| h.blocked.contains(&signature(&execution.name, &execution.arguments))) == Some(true) {
                    return Some(arc(PreToolDecision::Deny { reason: "TOOL_NO_PROGRESS: 同一调用或调用循环已连续五轮返回相同结果，此调用未执行。请检查错误并修复输入或环境；正常后台查询仍可使用。不得改写包装命令绕过权限拒绝；无法修复时明确报告未完成。".into() }));
                }
            }
            match next { Some(next) => Some(next.call().await), None => Some(arc(PreToolDecision::Allow)) }
        })
    }), Default::default()).await;
    let after = state.clone();
    ctx.on(
        "tools/post-execute",
        Arc::new(move |_, args| {
            let state = after.clone();
            Box::pin(async move {
                let next = args.last().and_then(downcast_arc::<cordis::NextFn>);
                if let (Some(execution), Some(result)) = (
                    args.first().and_then(downcast_arc::<Arc<ToolExecution>>),
                    args.get(1)
                        .and_then(downcast_arc::<Arc<ToolExecutionResult>>),
                ) {
                    if !poller(&execution.name, &execution.arguments) {
                        if let Some(result_hash) = result_signature(&result) {
                            with_history(&state, &execution, |history| {
                                history.observe(
                                    signature(&execution.name, &execution.arguments),
                                    result_hash,
                                    !result.is_error && repair(&execution.name),
                                )
                            });
                        }
                    }
                }
                match next {
                    Some(next) => Some(next.call().await),
                    None => None,
                }
            })
        }),
        Default::default(),
    )
    .await;
    let reset = state.clone();
    ctx.on(
        "agent/pre-step",
        Arc::new(move |_, args| {
            let state = reset.clone();
            Box::pin(async move {
                if let Some(payload) = args.first().and_then(downcast_arc::<AgentPreStepPayload>) {
                    if payload
                        .messages
                        .iter()
                        .any(|m| matches!(m.source, MessageSource::User { .. }))
                    {
                        state.lock().remove(payload.agent.id().as_str());
                    }
                }
                let next = args.last().and_then(downcast_arc::<cordis::NextFn>);
                match next {
                    Some(next) => Some(next.call().await),
                    None => None,
                }
            })
        }),
        Default::default(),
    )
    .await;
    ctx.on(
        "agent/turn-finished",
        Arc::new(move |_, args| {
            let state = state.clone();
            Box::pin(async move {
                if let Some(payload) = args
                    .first()
                    .and_then(downcast_arc::<dsh_agent::AgentTurnStoppingPayload>)
                {
                    state.lock().remove(payload.agent.id().as_str());
                }
                None
            })
        }),
        Default::default(),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blocks_identical_and_alternating_cycles_but_not_changing_results() {
        let mut identical = History::default();
        for _ in 0..4 {
            identical.observe(1, 1, false);
        }
        assert!(identical.blocked.is_empty());
        identical.observe(1, 1, false);
        assert!(identical.blocked.contains(&1));
        let mut cycle = History::default();
        for _ in 0..5 {
            cycle.observe(1, 1, false);
            cycle.observe(2, 2, false);
        }
        assert!(cycle.blocked.contains(&1) && cycle.blocked.contains(&2));
        let mut changing = History::default();
        for i in 0..100 {
            changing.observe(1, i, false);
        }
        assert!(changing.blocked.is_empty());
        assert_eq!(changing.rows.len(), 64);
        cycle.observe(3, 3, true);
        assert!(cycle.blocked.is_empty());
    }
    #[test]
    fn repeated_identical_writes_do_not_reset_their_own_cycle() {
        let mut history = History::default();
        for _ in 0..5 {
            history.observe(1, 1, true);
        }
        assert!(history.blocked.contains(&1));
        assert!(poller("job_output", &Value::Null));
        assert!(poller("agent_team", &serde_json::json!({"action":"wait"})));
        assert!(!poller(
            "agent_team",
            &serde_json::json!({"action":"create"})
        ));
    }
}
