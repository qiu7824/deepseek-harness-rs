//! Registry-owned receipts. Tool payloads cannot supply or replace these facts.
use crate::{ToolExecution, ToolExecutionResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    ReadOnly,
    Write,
    External,
    #[default]
    Unknown,
}

/// A host can provide trusted classifications and a non-secret execution identity.
/// An absent provider stays unknown; remote tool hints are not an authority.
pub struct ExecutionEvidenceProvider {
    pub classify: Arc<dyn Fn(&ToolExecution) -> EffectClass + Send + Sync>,
    pub snapshot: Arc<dyn Fn(&ToolExecution) -> Result<Value, String> + Send + Sync>,
}
impl cordis::Service for ExecutionEvidenceProvider {
    fn service_name(&self) -> &'static str {
        "executionEvidence"
    }
}

/// New same-session attachments may be produced by nested calls. They do not
/// revoke the original read scope; removed roots, modes and runtimes do.
pub fn context_matches(expected: &Value, current: &Value) -> bool {
    let mut expected = expected.clone();
    let mut current = current.clone();
    for value in [&mut expected, &mut current] {
        if let Some(object) = value.as_object_mut() {
            object.remove("fingerprint");
        }
    }
    if let Some(roots) = expected
        .pointer("/selectedPolicy/readOnlyRoots")
        .and_then(Value::as_array)
    {
        let Some(new) = current
            .pointer("/selectedPolicy/readOnlyRoots")
            .and_then(Value::as_array)
        else {
            return false;
        };
        if !roots.iter().all(|root| new.contains(root)) {
            return false;
        }
        expected["selectedPolicy"]["readOnlyRoots"] = json!([]);
        current["selectedPolicy"]["readOnlyRoots"] = json!([]);
    }
    expected == current
}

pub fn failure_stage(code: Option<&str>, body_invoked: bool) -> &'static str {
    match code.unwrap_or("") {
        "TOOL_INPUT_INVALID"
        | "TOOL_ARGUMENTS_INVALID"
        | "TOOL_SCHEMA_INVALID"
        | "INVALID_ARGUMENTS" => "input",
        code if code.starts_with("USER_APPROVAL_") || code.starts_with("APPROVAL_") => {
            "authorization"
        }
        "TOOL_NO_PROGRESS"
        | "TOOL_PREFLIGHT_DENIED"
        | "TOOL_BINDING_CHANGED"
        | "PRUNED_ARGUMENTS" => "preflight",
        "EXECUTION_CONTEXT_CHANGED"
        | "EXECUTION_CONTEXT_UNAVAILABLE"
        | "SANDBOX_SETUP_FAILED"
        | "SANDBOX_SETUP_REQUIRED"
        | "SANDBOX_SETUP_TIMEOUT"
        | "SANDBOX_UNAVAILABLE"
        | "SANDBOX_QUARANTINED"
        | "SANDBOX_BUSY"
        | "ENVIRONMENT_NOT_READY"
        | "PROGRAM_NOT_FOUND"
        | "SHELL_NOT_FOUND"
        | "NATIVE_SANDBOX_UNAVAILABLE" => "environment",
        "NATIVE_TOOL_UNSUPPORTED"
        | "NATIVE_TOOL_TEMPORARILY_UNAVAILABLE"
        | "CREDENTIAL_UNAVAILABLE"
        | "AUTHENTICATION_REQUIRED"
        | "AUTH_TARGET_MISMATCH" => "capability",
        "TOOL_ABORTED"
        | "TOOL_ABORTED_BEFORE_DISPATCH"
        | "SHELL_ABORTED"
        | "CANCELLED"
        | "AUTHENTICATION_REVOKED" => "cancellation",
        code if code.starts_with("TOOL_OUTPUT") => "output",
        _ if !body_invoked => "preflight",
        _ => "execution",
    }
}

pub fn undispatched(call_id: &str, name: &str) -> Value {
    json!({"schemaVersion":1,"authority":"tool-runtime","executionId":call_id,"rootCallId":call_id,
        "tool":name,"owner":null,"bodyInvoked":false,"effectBoundaryEntered":false,"effectClass":"unknown",
        "effects":"none","dispatch":"not_started","outcome":"cancelled","failureStage":"cancellation",
        "errorCode":"TOOL_ABORTED_BEFORE_DISPATCH","executionContext":null,"retry":"after_conditions_change"})
}

/// A durable nested-call intent exists, but no terminal runtime receipt was
/// persisted. Do not infer either dispatch or absence of effects from teardown.
pub(crate) fn interrupted_nested(start: &Value, owner: &str) -> Value {
    json!({"schemaVersion":1,"authority":"tool-runtime","executionId":start["subCallId"],"rootCallId":start["rootCallId"],
        "tool":start["name"],"owner":owner,"bodyInvoked":null,"effectBoundaryEntered":null,"effectClass":"unknown",
        "effects":"unknown","dispatch":"unknown","outcome":"cancelled","failureStage":"cancellation",
        "errorCode":"TOOL_ABORTED","executionContext":null,"retry":"inspect_effects_first"})
}

pub(crate) fn build(
    execution: &ToolExecution,
    result: &ToolExecutionResult,
    body_invoked: bool,
    effects_started: Option<bool>,
    class: EffectClass,
    context: &Value,
    effect_rejection: Option<&str>,
    adapter: Option<&Value>,
) -> Value {
    let adapter_not_started = matches!(
        execution.name.as_str(),
        "pwsh" | "execute_native" | "execute_script" | "execute_steps"
    ) && adapter.is_some_and(|r| {
        r["commandStarted"] == false && r["processState"] == "not_started" && r["effects"] == "none"
    });
    let no_effects = !body_invoked
        || effects_started == Some(false)
        || class == EffectClass::ReadOnly
        || adapter_not_started;
    let code = effect_rejection.or(result
        .error
        .as_ref()
        .and_then(|error| error.info.as_ref())
        .map(|info| info.code.as_str()));
    let phase = if result.is_error {
        failure_stage(code, body_invoked)
    } else {
        "complete"
    };
    let mut receipt = adapter
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    receipt["schemaVersion"] = json!(1);
    receipt["authority"] = json!("tool-runtime");
    receipt["executionId"] = json!(execution.call_id.as_str());
    receipt["rootCallId"] = json!(execution.root_call_id.as_str());
    receipt["tool"] = json!(execution.name);
    receipt["owner"] = json!(execution.agent.as_ref().map(|a| a.id().as_str()));
    receipt["bodyInvoked"] = json!(body_invoked);
    receipt["effectBoundaryEntered"] = json!(effects_started);
    receipt["effectClass"] = json!(class);
    receipt["effects"] = json!(if no_effects {
        "none"
    } else if effects_started == Some(true) {
        "possible"
    } else {
        "unknown"
    });
    receipt["dispatch"] = json!(if !body_invoked
        || adapter_not_started
        || effects_started == Some(false)
    {
        "not_started"
    } else if effects_started == Some(true) {
        "started"
    } else {
        "unknown"
    });
    receipt["outcome"] = json!(if !result.is_error {
        "succeeded"
    } else if phase == "cancellation" {
        "cancelled"
    } else if !body_invoked {
        "blocked"
    } else {
        "failed"
    });
    receipt["failureStage"] = json!(if result.is_error { Some(phase) } else { None });
    receipt["errorCode"] = json!(code);
    receipt["executionContext"] = context.clone();
    receipt["retry"] = json!(if !result.is_error {
        "not_needed"
    } else if no_effects {
        "after_conditions_change"
    } else {
        "inspect_effects_first"
    });
    receipt
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failure_classes_use_codes_not_untrusted_prose() {
        assert_eq!(
            failure_stage(Some("APPROVAL_REJECTED"), true),
            "authorization"
        );
        assert_eq!(
            failure_stage(Some("NATIVE_TOOL_UNSUPPORTED"), true),
            "capability"
        );
        assert_eq!(
            failure_stage(Some("CUSTOM_PERMISSION_DENIED"), true),
            "execution"
        );
        assert_eq!(failure_stage(None, false), "preflight");
    }
}
