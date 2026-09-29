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
fn execution_signature(execution: &ToolExecution, context: Option<&Value>) -> u64 {
    hash((
        signature(&execution.name, &execution.arguments),
        execution
            .schema
            .as_ref()
            .and_then(|schema| serde_json::to_string(schema).ok()),
        context
            .filter(|value| !value.is_null())
            .map(environment_key),
    ))
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
fn sandbox_path(name: &str) -> bool {
    matches!(
        name,
        "pwsh"
            | "bash"
            | "execute_native"
            | "execute_script"
            | "execute_steps"
            | "run_code"
            | "terminal_open"
            | "terminal_send"
    )
}
fn environment_key(value: &Value) -> u64 {
    let mut value = value.clone();
    if let Some(fields) = value.as_object_mut() {
        fields.remove("fingerprint");
    }
    hash(super::json_stringify(&super::sort_json_value(&value)))
}
fn recovery_key(value: &Value) -> u64 {
    if let Some(identity) = value.get("startupRecoveryIdentity") {
        return environment_key(identity);
    }
    let mut stable = value.clone();
    if let Some(fields) = stable.as_object_mut() {
        for field in [
            "fingerprint",
            "workingDirectory",
            "workdir",
            "cwd",
            "requestedPermissions",
        ] {
            fields.remove(field);
        }
        if let Some(profile) = fields.get_mut("profile").and_then(Value::as_object_mut) {
            profile.remove("contextId");
        }
        if let Some(policy) = fields
            .get_mut("selectedPolicy")
            .and_then(Value::as_object_mut)
        {
            policy.remove("readOnlyRoots");
        }
    }
    environment_key(&stable)
}
fn shared_failure(result: &ToolExecutionResult) -> Option<u64> {
    let receipt = result.meta.as_ref()?.get("executionReceipt")?;
    if !result.is_error || receipt["authority"] != "tool-runtime" {
        return None;
    }
    if !matches!(
        receipt["errorCode"].as_str(),
        Some(
            "SANDBOX_SETUP_FAILED"
                | "SANDBOX_SETUP_REQUIRED"
                | "SANDBOX_SETUP_TIMEOUT"
                | "SANDBOX_UNAVAILABLE"
                | "SANDBOX_QUARANTINED"
                | "SHELL_NOT_FOUND"
        )
    ) {
        return None;
    }
    Some(recovery_key(&receipt["executionContext"]))
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
                "checkId",
                "cacheHit",
            ] {
                object.remove(field);
            }
        }
        super::json_stringify(&super::sort_json_value(&value)).hash(&mut h);
    } else if let Some(error) = &result.error {
        // Cache countdowns are not progress. These typed refusal classes keep
        // the exact call and admission context, but exclude volatile wording.
        match error.info.as_ref().map(|info| info.code.as_str()) {
            Some(
                code @ ("NATIVE_TOOL_UNSUPPORTED"
                | "NATIVE_TOOL_TEMPORARILY_UNAVAILABLE"
                | "AUTHENTICATION_REQUIRED"),
            ) => code.hash(&mut h),
            _ => error.message.hash(&mut h),
        }
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
    environment_failures: HashMap<u64, u8>,
    last_probe: Option<String>,
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
            self.blocked
                .retain(|sig| self.rows.iter().any(|(id, _)| id == sig));
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
            // A permission/path change is not proof that process startup was
            // repaired. Only call-level repetition is scoped to that change.
            history.rows.clear();
            history.blocked.clear();
            history.last_repair = None;
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
                let provider=execution.agent.as_ref().and_then(|agent|agent.ctx().get_typed::<Arc<dsh_tools::receipt::ExecutionEvidenceProvider>>("executionEvidence",false));
                let context=provider.and_then(|provider|(provider.snapshot)(&execution).ok());
                if sandbox_path(&execution.name) && context.as_ref().is_some_and(|context|with_history(&state,&execution,|h|h.environment_failures.get(&recovery_key(context)).copied().unwrap_or(0)>=5)==Some(true)) {
                    return Some(arc(PreToolDecision::Deny { reason:"TOOL_NO_PROGRESS: 当前运行配置已连续五次出现结构化启动故障，此调用未执行。请用 environment_validate refresh=true 获取新的成功启动证据，或在用户明确修改运行配置/发出新指令后重新评估。切换workdir、改写命令或更换执行包装工具不代表环境已修复；本地执行受阻时应直接说明并提供可用的文本结果，不得改走远端终端或浏览器绕过。".into() }));
                }
                if !poller(&execution.name, &execution.arguments) && with_history(&state, &execution,
                    |h| h.blocked.contains(&execution_signature(&execution,context.as_ref()))) == Some(true) {
                    return Some(arc(PreToolDecision::Deny { reason: "TOOL_NO_PROGRESS: 同一调用或调用循环已连续五轮返回相同结果，此调用未执行。请检查错误并修复输入或环境；正常后台查询仍可使用。不得改写包装命令绕过权限拒绝；无法修复时明确报告未完成。".into() }));
                }
            }
            match next { Some(next) => Some(next.call().await), None => Some(arc(PreToolDecision::Allow)) }
        })
    }), Default::default()).await;
    let after = state.clone();
    ctx.on(
        "tools/result",
        Arc::new(move |_, args| {
            let state = after.clone();
            Box::pin(async move {
                let next = args.last().and_then(downcast_arc::<cordis::NextFn>);
                if let (Some(execution), Some(result)) = (
                    args.first().and_then(downcast_arc::<Arc<ToolExecution>>),
                    args.get(1)
                        .and_then(downcast_arc::<Arc<ToolExecutionResult>>),
                ) {
                    with_history(&state, &execution, |history| {
                        if sandbox_path(&execution.name) {
                            if let Some(key) = shared_failure(&result) {
                                if history.environment_failures.len() < 64
                                    || history.environment_failures.contains_key(&key)
                                {
                                    let count =
                                        history.environment_failures.entry(key).or_default();
                                    *count = count.saturating_add(1);
                                }
                            }
                        }
                        if !result.is_error
                            && execution.name == "environment_validate"
                            && execution.arguments["refresh"] == true
                        {
                            if let Some(value) = &result.value {
                                if value["status"] == "ready"
                                    && value["cacheHit"] == false
                                    && value["level"] != "locate"
                                    && value["executionWorld"] == "selected_environment"
                                {
                                    if let Some(id) = value["checkId"]
                                        .as_str()
                                        .filter(|id| history.last_probe.as_deref() != Some(*id))
                                    {
                                        if let Some(context) = result.meta.as_ref().and_then(|m| {
                                            m.pointer("/executionReceipt/executionContext")
                                        }) {
                                            history
                                                .environment_failures
                                                .remove(&recovery_key(context));
                                            history.rows.clear();
                                            history.blocked.clear();
                                            history.last_probe = Some(id.to_owned());
                                        }
                                    }
                                }
                            }
                        }
                    });
                    if !poller(&execution.name, &execution.arguments) {
                        if let Some(result_hash) = result_signature(&result) {
                            with_history(&state, &execution, |history| {
                                history.observe(
                                    execution_signature(
                                        &execution,
                                        result.meta.as_ref().and_then(|meta| {
                                            meta.pointer("/executionReceipt/executionContext")
                                        }),
                                    ),
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
                    let mut state = state.lock();
                    if state
                        .get(payload.agent.id().as_str())
                        .is_some_and(|history| history.environment_failures.is_empty())
                    {
                        state.remove(payload.agent.id().as_str());
                    } else if let Some(history) = state.get_mut(payload.agent.id().as_str()) {
                        history.rows.clear();
                        history.blocked.clear();
                        history.last_repair = None;
                    }
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
