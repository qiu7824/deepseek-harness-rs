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
    let mut task_mutation = false;
    let mut work_calls = BTreeSet::new();
    session.visit_events(start, None, |event| {
        if event.data.get("turn").is_some() && event.data["turn"] != request.turn {
            return Ok(true);
        }
        if matches!(
            event.type_.as_str(),
            "tool/call" | "tool/ptc-dispatch-start"
        ) {
            let id = event.data["callId"]
                .as_str()
                .or_else(|| event.data["subCallId"].as_str())
                .unwrap_or("");
            let name = event.data["name"].as_str().unwrap_or("");
            calls.insert(format!("call-{}", digest(id.as_bytes())));
            let args = event.data["arguments"]
                .as_str()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .unwrap_or_else(|| event.data["arguments"].clone());
            if name == "task_execution" && args["action"] != "list" {
                task_mutation |= !matches!(
                    args["action"].as_str(),
                    Some(
                        "get"
                            | "recover"
                            | "refresh_history"
                            | "requirements_history"
                            | "requirements_snapshot"
                    )
                );
                task_calls.insert(id.to_owned());
                if let Some(id) = args["taskId"].as_str() {
                    task_ids.insert(id.to_owned());
                }
            }
            if !matches!(
                name,
                "task_execution"
                    | "update_goal"
                    | "update_plan"
                    | "think"
                    | "tool_search"
                    | "job_output"
                    | "job_list"
                    | "wait"
                    | "wait_agent"
                    | "run_code"
            ) {
                work_calls.insert(id.to_owned());
                mutations |= !crate::task_effects::read_only(name, &args);
            }
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
    let tasks = service
        .runtime
        .list(owner)?
        .into_iter()
        .filter(|task| {
            task_ids.contains(&task.task_id)
                || task
                    .steps
                    .iter()
                    .any(|step| calls.contains(&step.execution_id))
        })
        .collect::<Vec<_>>();
    if tasks.is_empty() {
        return Ok(
            (mutations || work_calls.len() > 1).then(|| CompletionAssessment {
                status: "unverified".into(),
                summary: "未建立任务验收，结果尚未验证。".into(),
                task_id: None,
                blockers: vec![],
                follow_up: None,
            }),
        );
    }
    let mut assessments = Vec::new();
    for task in tasks {
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
        let mut current = assessment(&task, verification_error, request.attempts);
        if !task_mutation && !mutations {
            current.follow_up = None;
        }
        assessments.push(current);
    }
    if assessments.len() == 1 {
        return Ok(assessments.pop());
    }
    let rank = |status: &str| match status {
        "blocked" => 4,
        "incomplete" => 3,
        "cancelled" => 2,
        "unverified" => 1,
        _ => 0,
    };
    let highest = assessments
        .iter()
        .map(|a| rank(&a.status))
        .max()
        .unwrap_or(0);
    let status = assessments
        .iter()
        .find(|a| rank(&a.status) == highest)
        .unwrap()
        .status
        .clone();
    let summary = if highest == 0 {
        format!("本回合关联的 {} 项任务均已验收。", assessments.len())
    } else {
        format!(
            "本回合关联 {} 项任务，其中仍有未完成的验收。",
            assessments.len()
        )
    };
    let follow_up = if status == "incomplete" {
        assessments.iter().find_map(|a| a.follow_up.clone())
    } else {
        None
    };
    let blockers = assessments
        .iter()
        .filter(|a| a.status != "verified")
        .map(|a| {
            format!(
                "{}：{} {}",
                a.task_id.as_deref().unwrap_or("任务"),
                a.summary,
                a.blockers.join("；")
            )
        })
        .take(8)
        .map(|s| s.chars().take(600).collect())
        .collect();
    Ok(Some(CompletionAssessment {
        status,
        summary,
        task_id: None,
        blockers,
        follow_up,
    }))
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
    #[tokio::test]
    async fn explicit_status_query_shows_incomplete_without_restarting_the_users_task() {
        let root = std::env::temp_dir().join(format!("completion-query-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let ctx = Context::root();
        dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
        dsh_tools::ToolRuntime::install(&ctx, Default::default()).unwrap();
        dsh_llm::LlmRuntime::install(&ctx);
        let fs = dsh_fs_local::LocalFileSystem::install(
            &ctx,
            dsh_fs_local::Config {
                cwd: Some(root.to_string_lossy().into_owned()),
                diff_basis_max_bytes: None,
            },
        )
        .unwrap();
        let service = TaskExecution {
            runtime: Arc::new(TaskRuntime::open(&root.join("tasks.sqlite")).unwrap()),
            context: ctx.clone(),
            fs,
            resources: None,
            validation_work: Default::default(),
        };
        let session = dsh_session::Session::create(
            dsh_session::session_id("completion-query"),
            None,
            None,
            None,
        )
        .unwrap();
        let agent: Arc<dyn dsh_agent::Agent> = dsh_agent_loop::ReactLoopAgent::new(
            &ctx,
            session.id().clone(),
            Default::default(),
            session.clone(),
        )
        .unwrap();
        service.runtime.create(agent.id().as_str(),"query-task",serde_json::from_value(json!({"objective":"Pending work","acceptanceChecks":[{"id":"manual","description":"Review output","checker":{"kind":"manual","reason":"Review"}}]})).unwrap()).unwrap();
        session
            .append("turn/start", json!({"turn":1}), None)
            .unwrap();
        session.append("tool/call",json!({"turn":1,"callId":"get","name":"task_execution","arguments":{"action":"get","taskId":"query-task"}}),None).unwrap();
        let request = || CompletionRequest {
            agent: agent.clone(),
            turn: 1,
            attempts: 0,
            cancelled: Arc::new(|| false),
        };
        let status = review(&service, request()).await.unwrap().unwrap();
        assert_eq!(status.status, "incomplete");
        assert_eq!(status.task_id.as_deref(), Some("query-task"));
        assert!(status.follow_up.is_none());
        session.append("tool/call",json!({"turn":1,"callId":"validate","name":"task_execution","arguments":{"action":"validate","taskId":"query-task"}}),None).unwrap();
        assert!(
            review(&service, request())
                .await
                .unwrap()
                .unwrap()
                .follow_up
                .is_some()
        );
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }
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
