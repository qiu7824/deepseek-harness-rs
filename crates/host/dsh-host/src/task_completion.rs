//! Completion is derived from durable acceptance, never from the assistant's wording.
use super::*;
use dsh_agent::{CompletionAssessment, CompletionRequest, CompletionReview};

fn assessment(
    task: &TaskContract,
    verification_error: Option<String>,
    attempts: u8,
) -> CompletionAssessment {
    let mut blockers = task.completion_blockers();
    let stale = verification_error.is_some();
    if let Some(error) = verification_error {
        blockers.insert(0, error);
    }
    let (status, summary) = match task.state {
        TaskState::Completed if !stale && blockers.is_empty() => ("verified", "任务已验收完成。"),
        TaskState::Completed => ("incomplete", "原验收证据已失效，任务尚未重新验收。"),
        TaskState::Cancelled => ("cancelled", "任务已取消。"),
        TaskState::Blocked => ("blocked", "任务存在待核实的执行效果，尚未完成。"),
        TaskState::AwaitingUser => ("blocked", "任务等待人工验收，尚未完成。"),
        TaskState::ValidationFailed => ("incomplete", "任务验收未通过，尚未完成。"),
        _ => ("incomplete", "任务尚未完成验收。"),
    };
    let uncertain = task.steps.iter().any(|s| {
        s.effect != EffectKind::ReadOnly
            && matches!(
                s.state,
                StepState::Unknown | StepState::Running | StepState::Dispatched
            )
    });
    let follow_up = (attempts < 2 && status == "incomplete" && !uncertain).then(|| format!(
        "任务 {} 尚未通过完成检查：{} {}。若可在当前权限内修复，请处理后调用 task_execution validate/complete；否则明确报告未完成与阻塞原因。不得削弱验收要求、重复未知效果操作或宣称完成。",
        task.task_id, summary, blockers.iter().take(4).map(String::as_str).collect::<Vec<_>>().join("；")
    ));
    CompletionAssessment {
        status: status.into(),
        summary: summary.into(),
        task_id: Some(task.task_id.clone()),
        blockers: blockers
            .into_iter()
            .take(8)
            .map(|s| s.chars().take(600).collect())
            .collect(),
        follow_up,
    }
}

async fn review(
    service: &TaskExecution,
    request: CompletionRequest,
) -> Result<Option<CompletionAssessment>> {
    let session = request.agent.session();
    let start = session
        .find_event_rev(|e| e.type_ == "turn/start" && e.data["turn"] == request.turn)?
        .map(|e| e.seq.get())
        .unwrap_or(0);
    let mut calls = BTreeSet::new();
    let mut task_calls = BTreeSet::new();
    let mut task_ids = BTreeSet::new();
    let mut mutations = false;
    session.visit_events(start, None, |event| {
        if event.data["turn"] != request.turn {
            return Ok(true);
        }
        if event.type_ == "tool/call" {
            let id = event.data["callId"].as_str().unwrap_or("");
            let name = event.data["name"].as_str().unwrap_or("");
            calls.insert(format!("call-{}", digest(id.as_bytes())));
            let args = event.data["arguments"]
                .as_str()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .unwrap_or_else(|| event.data["arguments"].clone());
            if name == "task_execution" && !matches!(args["action"].as_str(), Some("get" | "list"))
            {
                task_calls.insert(id.to_owned());
                if let Some(id) = args["taskId"].as_str() {
                    task_ids.insert(id.to_owned());
                }
            }
            mutations |= matches!(
                name,
                "write" | "edit" | "write_file" | "patch" | "generate_image"
            ) || name == "workspace_scratch" && args["action"] == "promote";
        } else if event.type_ == "tool/result"
            && task_calls.contains(
                event.data["message"]["source"]["callId"]
                    .as_str()
                    .unwrap_or(""),
            )
        {
            if let Some(blocks) = event.data["message"]["content"].as_array() {
                for block in blocks {
                    if let Some(value) = block["text"]
                        .as_str()
                        .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    {
                        if let Some(id) = value["task"]["taskId"].as_str() {
                            task_ids.insert(id.to_owned());
                        }
                    }
                }
            }
        }
        Ok(true)
    })?;
    let owner = request.agent.id().as_str();
    let task = service.runtime.list(owner)?.into_iter().find(|task| {
        task_ids.contains(&task.task_id)
            || task
                .steps
                .iter()
                .any(|step| calls.contains(&step.execution_id))
    });
    let Some(task) = task else {
        return Ok(mutations.then(|| CompletionAssessment {
            status: "unverified".into(),
            summary: "未建立任务验收，结果尚未验证。".into(),
            task_id: None,
            blockers: vec![],
            follow_up: None,
        }));
    };
    let verification_error = if task.state == TaskState::Completed {
        match session.header().cwd.as_deref() {
            Some(cwd) => service
                .verified_evidence(
                    owner,
                    &task.task_id,
                    task.revision,
                    cwd,
                    request.cancelled.clone(),
                )
                .await
                .err(),
            None => Some("任务工作区不可用".into()),
        }
    } else {
        None
    };
    Ok(Some(assessment(
        &task,
        verification_error,
        request.attempts,
    )))
}

pub(super) fn install(service: &Arc<TaskExecution>) {
    let weak = Arc::downgrade(service);
    service.context.register_service(Arc::new(CompletionReview {
        review: Arc::new(move |request| {
            let service = weak.upgrade();
            Box::pin(async move {
                let service = service.ok_or("Task acceptance service unavailable")?;
                review(&service, request).await
            })
        }),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn task(state: &str) -> TaskContract {
        serde_json::from_value(json!({"version":1,"taskId":"t","owner":"s","revision":1,
            "spec":{"objective":"Check","acceptanceChecks":[]},"state":state,"steps":[],
            "acceptanceResults":[],"validationIdentity":"ok","outputIdentities":{},"createdAt":1,"updatedAt":1})).unwrap()
    }
    #[test]
    fn failed_acceptance_never_becomes_verified_and_retries_are_bounded() {
        let failed = task("validation_failed");
        assert_eq!(assessment(&failed, None, 0).status, "incomplete");
        assert!(assessment(&failed, None, 0).follow_up.is_some());
        assert!(assessment(&failed, None, 2).follow_up.is_none());
        for state in ["blocked", "awaiting_user", "cancelled"] {
            assert!(assessment(&task(state), None, 0).follow_up.is_none());
        }
    }
    #[test]
    fn completed_evidence_must_still_match() {
        let done = task("completed");
        assert_eq!(assessment(&done, None, 0).status, "verified");
        assert_eq!(
            assessment(&done, Some("output changed".into()), 0).status,
            "incomplete"
        );
    }
}
