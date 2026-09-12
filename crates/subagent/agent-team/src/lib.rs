//! Durable named teammates, peer delivery and a compare-and-set task board.

use cordis::Context;
use dsh_agent::{Agent, AgentRegistry, InboxTarget};
use dsh_llm::{ContentBlock, MessageSource};
use dsh_session::{SessionEvent, SessionStore, session_id};
use dsh_session_persistence::SessionPersistenceApi;
use dsh_subagent::{
    ContinuableStartSpec, SubagentFollowupOptions, SubagentRuntime, SubagentStartRequest,
};
use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub id: String,
    pub name: String,
    pub description: String,
    pub provider: String,
    pub context: String,
    pub phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub revision: u64,
    pub subject: String,
    pub description: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    pub blocked_by: Vec<String>,
    pub write_scopes: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mail {
    pub id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub target_id: String,
    pub content: Vec<ContentBlock>,
}
#[derive(Default, Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub team_id: String,
    pub members: BTreeMap<String, Member>,
    pub tasks: BTreeMap<String, Task>,
    pub messages: Vec<Mail>,
    pub delivered: BTreeSet<String>,
}

fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}
fn fold(team: &str, events: &[SessionEvent]) -> Result<Board, String> {
    let mut state = Board {
        team_id: team.into(),
        ..Default::default()
    };
    for event in events {
        if !event.type_.starts_with("team/") || event.data["teamId"] != team {
            continue;
        }
        if event.data["version"] != 1 {
            return Err("unsupported team record version".into());
        }
        match event.type_.as_str() {
            "team/member" => {
                let member: Member = serde_json::from_value(event.data["member"].clone())
                    .map_err(|_| "invalid team member record")?;
                if !name(&member.name)
                    || member.name == "lead"
                    || !matches!(member.phase.as_str(), "provisioning" | "active" | "failed")
                {
                    return Err("invalid team member identity or phase".into());
                }
                if state
                    .members
                    .get(&member.name)
                    .is_some_and(|old| old.id != member.id)
                {
                    return Err("team member identity changed".into());
                }
                if state
                    .members
                    .values()
                    .any(|old| old.name != member.name && old.id == member.id)
                {
                    return Err("duplicate teammate identity".into());
                }
                if state.members.get(&member.name).is_some_and(|old| {
                    old.provider != member.provider
                        || old.context != member.context
                        || (old.phase == "failed" && member.phase != "failed")
                        || (old.phase == "active" && member.phase == "provisioning")
                }) {
                    return Err("invalid teammate lifecycle transition".into());
                }
                state.members.insert(member.name.clone(), member);
            }
            "team/task" => {
                let task: Task = serde_json::from_value(event.data["task"].clone())
                    .map_err(|_| "invalid team task record")?;
                if task.revision != state.tasks.get(&task.id).map_or(1, |old| old.revision + 1) {
                    return Err("noncontiguous team task revision".into());
                }
                state.tasks.insert(task.id.clone(), task);
            }
            "team/message/queued" => {
                let mail: Mail = serde_json::from_value(event.data["message"].clone())
                    .map_err(|_| "invalid team message record")?;
                if state.messages.iter().any(|old| old.id == mail.id) {
                    return Err("duplicate team message identity".into());
                }
                let sender = if mail.sender_id == team {
                    Some("lead")
                } else {
                    state
                        .members
                        .values()
                        .find(|member| member.id == mail.sender_id && member.phase == "active")
                        .map(|member| member.name.as_str())
                };
                let target = mail.target_id == team
                    || state
                        .members
                        .values()
                        .any(|member| member.id == mail.target_id && member.phase == "active");
                if sender != Some(mail.sender_name.as_str()) || !target {
                    return Err("queued message does not belong to this team".into());
                }
                state.messages.push(mail);
            }
            "team/message/delivered" => {
                let id = event.data["messageId"]
                    .as_str()
                    .ok_or("invalid delivery receipt")?;
                if !state
                    .messages
                    .iter()
                    .any(|mail| mail.id == id && event.data["targetId"] == mail.target_id)
                {
                    return Err("delivery receipt has no matching queued target".into());
                }
                state.delivered.insert(id.into());
            }
            _ => {}
        }
    }
    Ok(state)
}

fn ready(state: &Board, task: &Task) -> bool {
    task.blocked_by.iter().all(|id| {
        state
            .tasks
            .get(id)
            .is_some_and(|task| task.status == "completed")
    })
}
fn validate_task(state: &Board, task: &Task) -> Result<(), String> {
    if !name(&task.id)
        || task.subject.trim().is_empty()
        || task.subject.len() > 512
        || task.description.len() > 16_384
        || task.write_scopes.len() > 32
        || task
            .write_scopes
            .iter()
            .any(|scope| scope.is_empty() || scope.len() > 1024)
        || task.blocked_by.len() > 256
    {
        return Err("invalid task identity, text or write scope count".into());
    }
    if !matches!(
        task.status.as_str(),
        "pending" | "in_progress" | "completed" | "deleted"
    ) {
        return Err("invalid task status".into());
    }
    let mut pending = task.blocked_by.clone();
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if id == task.id {
            return Err("task dependency cycle".into());
        }
        if !seen.insert(id.clone()) {
            continue;
        }
        let dependency = state
            .tasks
            .get(&id)
            .ok_or("task dependency does not exist")?;
        pending.extend(dependency.blocked_by.clone());
    }
    if task.status == "in_progress" && !ready(state, task) {
        return Err("task dependencies are not completed".into());
    }
    Ok(())
}

pub struct AgentTeams {
    sessions: Arc<SessionStore>,
    agents: Arc<AgentRegistry>,
    persistence: Arc<dyn SessionPersistenceApi>,
    subagents: Arc<SubagentRuntime>,
    gate: tokio::sync::Mutex<()>,
    max_members: usize,
}
impl cordis::Service for AgentTeams {
    fn service_name(&self) -> &'static str {
        "agentTeams"
    }
}

impl AgentTeams {
    async fn events(
        &self,
        id: &str,
    ) -> Result<(dsh_session::SessionHeader, Arc<Vec<SessionEvent>>), String> {
        if let Some(session) = self.sessions.get(&session_id(id)) {
            return Ok((session.header().clone(), session.events()));
        }
        let snapshot = self.persistence.read_from(&session_id(id), 0).await?;
        Ok((snapshot.meta, Arc::new(snapshot.events)))
    }
    pub async fn read(&self, id: &str) -> Result<Board, String> {
        let (header, events) = self.events(id).await?;
        if header.origin.as_deref() != Some("subagent") {
            return fold(id, &events);
        }
        let parent = header
            .parent_session
            .as_ref()
            .ok_or("team member has no parent")?;
        let (_, events) = self.events(parent.as_str()).await?;
        let board = fold(parent.as_str(), &events)?;
        if !board
            .members
            .values()
            .any(|member| member.id == id && member.phase == "active")
        {
            return Err("session is not a registered teammate".into());
        }
        Ok(board)
    }
    pub async fn view(&self, id: &str) -> Result<Value, String> {
        let board = self.read(id).await?;
        let mut view = serde_json::to_value(&board).map_err(|error| error.to_string())?;
        for member in board.members.values() {
            let status = if member.phase != "active" {
                member.phase.clone()
            } else {
                self.agents
                    .get(&session_id(&member.id))
                    .map(|agent| format!("{:?}", agent.status()).to_ascii_lowercase())
                    .unwrap_or("inactive".into())
            };
            view["members"][&member.name]["status"] = json!(status);
        }
        view.as_object_mut().unwrap().remove("messages");
        view.as_object_mut().unwrap().remove("delivered");
        view["pendingMessages"] = json!(
            board
                .messages
                .iter()
                .filter(|mail| !board.delivered.contains(&mail.id))
                .count()
        );
        Ok(view)
    }
    async fn append(
        &self,
        lead: &Arc<dyn Agent>,
        kind: &str,
        mut data: Value,
    ) -> Result<(), String> {
        data["version"] = json!(1);
        data["teamId"] = json!(lead.id().as_str());
        lead.session().append(kind, data, None)?;
        if !self.sessions.flush(lead.session()).await? {
            return Err("team journal has no durability listener".into());
        }
        Ok(())
    }
    fn actor<'a>(&self, board: &'a Board, caller: &Arc<dyn Agent>) -> Result<&'a str, String> {
        if caller.id().as_str() == board.team_id {
            return Ok("lead");
        }
        board
            .members
            .values()
            .find(|member| member.id == caller.id().as_str() && member.phase == "active")
            .map(|member| member.name.as_str())
            .ok_or_else(|| "caller is not an active team member".into())
    }
    fn target<'a>(&self, board: &'a Board, target: &str) -> Result<&'a str, String> {
        if target == "lead" || target == board.team_id {
            return Ok(&board.team_id);
        }
        board
            .members
            .values()
            .find(|member| {
                (member.name == target || member.id == target) && member.phase == "active"
            })
            .map(|member| member.id.as_str())
            .ok_or_else(|| "unknown or inactive teammate".into())
    }
    async fn delivered_to_target(&self, mail: &Mail, team: &str) -> Result<bool, String> {
        let (_, events) = self.events(&mail.target_id).await?;
        Ok(events.iter().any(|event| {
            let messages: Vec<&Value> = if event.type_ == "user/message" {
                vec![&event.data]
            } else if event.type_ == "agent/inbox/spliced" {
                event.data["inserted"]
                    .as_array()
                    .map(|messages| messages.iter().collect())
                    .unwrap_or_default()
            } else {
                vec![]
            };
            messages.into_iter().any(|message| {
                message["source"]["kind"] == "team-message"
                    && message["source"]["teamId"] == team
                    && message["source"]["messageId"] == mail.id
            })
        }))
    }
    async fn dispatch(
        &self,
        lead: &Arc<dyn Agent>,
        mail: &Mail,
        signal: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<(), String> {
        if signal() {
            return Err("team delivery cancelled; message remains queued".into());
        }
        if !self.delivered_to_target(mail, lead.id().as_str()).await? {
            let source = MessageSource::TeamMessage {
                team_id: lead.id().to_string(),
                message_id: mail.id.clone(),
                sender_id: mail.sender_id.clone(),
                sender_name: mail.sender_name.clone(),
            };
            let mut content = vec![ContentBlock::Text {
                text: format!("Team message {} from {}:", mail.id, mail.sender_name),
            }];
            content.extend(mail.content.clone());
            if mail.target_id == lead.id().as_str() {
                lead.send(
                    dsh_llm::create_user_message(content, source),
                    InboxTarget::NextStep,
                    true,
                );
            } else {
                self.subagents
                    .followup(
                        lead.clone(),
                        &session_id(&mail.target_id),
                        &content,
                        SubagentFollowupOptions {
                            source,
                            signal,
                            steer: true,
                        },
                    )
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        if let Some(target) = self.sessions.get(&session_id(&mail.target_id)) {
            self.sessions.flush(&target).await?;
        }
        if !self.delivered_to_target(mail, lead.id().as_str()).await? {
            return Err("target delivery was not durably observed; message remains queued".into());
        }
        self.append(
            lead,
            "team/message/delivered",
            json!({"messageId":mail.id,"targetId":mail.target_id}),
        )
        .await
    }
    async fn recover(
        &self,
        lead: &Arc<dyn Agent>,
        board: &Board,
        signal: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Vec<String>, String> {
        for member in board
            .members
            .values()
            .filter(|member| member.phase == "provisioning")
        {
            let mut updated = member.clone();
            let marker = format!("Team member identity: {}", member.id);
            let accepted = self
                .events(&member.id)
                .await
                .ok()
                .is_some_and(|(header, events)| {
                    header.parent_session.as_ref() == Some(lead.id())
                        && header.origin.as_deref() == Some("subagent")
                        && events.iter().any(|event| {
                            matches!(event.type_.as_str(), "user/message" | "agent/inbox/spliced")
                                && serde_json::to_string(&event.data)
                                    .is_ok_and(|data| data.contains(&marker))
                        })
                });
            updated.phase = if accepted { "active" } else { "failed" }.into();
            if !accepted {
                if let Some(child) = self.agents.get(&session_id(&member.id)) {
                    if child.session().header().parent_session.as_ref() == Some(lead.id()) {
                        self.subagents
                            .interrupt(
                                &session_id(&member.id),
                                &dsh_subagent::SubagentInterruptAuthority::Ancestor {
                                    agent: lead.clone(),
                                },
                            )
                            .map_err(|error| error.to_string())?;
                        tokio::time::timeout(std::time::Duration::from_secs(5), child.when_idle())
                            .await
                            .map_err(|_| "provisioning cleanup did not settle")?;
                    }
                }
                updated.error = Some("initial admission could not be recovered".into());
            }
            self.append(lead, "team/member", json!({"member":updated}))
                .await?;
        }
        let mut errors = Vec::new();
        let mut blocked = BTreeSet::new();
        for mail in board
            .messages
            .iter()
            .filter(|mail| !board.delivered.contains(&mail.id))
        {
            if blocked.contains(&mail.target_id) {
                continue;
            }
            if let Err(error) = self.dispatch(lead, mail, signal.clone()).await {
                blocked.insert(mail.target_id.clone());
                errors.push(format!("{}: {error}", mail.id));
            }
        }
        Ok(errors)
    }
    async fn execute(
        &self,
        caller: Arc<dyn Agent>,
        args: Value,
        signal: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Value, String> {
        let _gate = self.gate.lock().await;
        let mut board = self.read(caller.id().as_str()).await?;
        let actor = self.actor(&board, &caller)?.to_owned();
        let lead = self
            .agents
            .get(&session_id(&board.team_id))
            .ok_or("team lead is not active")?;
        if signal() {
            return Err("team action cancelled".into());
        }
        let pending_errors = self.recover(&lead, &board, signal.clone()).await?;
        let mut receipt = None;
        if signal() {
            return Err("team action cancelled".into());
        }
        board = self.read(caller.id().as_str()).await?;
        match args["action"].as_str().unwrap_or("status") {
            "status" => {}
            "create" => {
                if actor != "lead" {
                    return Err("only the lead may create teammates".into());
                }
                let label = args["name"].as_str().ok_or("teammate name is required")?;
                if !name(label)
                    || label == "lead"
                    || board.members.contains_key(label)
                    || board.members.len() >= self.max_members
                {
                    return Err(
                        "invalid, reserved or duplicate teammate name, or member limit reached"
                            .into(),
                    );
                }
                let prompt = args["prompt"]
                    .as_str()
                    .filter(|p| !p.trim().is_empty() && p.len() <= 32_768)
                    .ok_or("teammate prompt must contain 1 to 32768 bytes")?;
                let context = args["context"].as_str().unwrap_or("fresh");
                let provider = match context {
                    "fresh" => "spawn",
                    "fork" => "fork",
                    _ => return Err("context must be fresh or fork".into()),
                };
                if args["description"]
                    .as_str()
                    .is_some_and(|text| text.len() > 2048)
                {
                    return Err("member description exceeds 2048 bytes".into());
                }
                let id = uuid::Uuid::new_v4().to_string();
                let mut member = Member {
                    id: id.clone(),
                    name: label.into(),
                    description: args["description"].as_str().unwrap_or(label).into(),
                    provider: provider.into(),
                    context: context.into(),
                    phase: "provisioning".into(),
                    error: None,
                };
                self.append(&lead, "team/member", json!({"member":member}))
                    .await?;
                let request = SubagentStartRequest {
                    label: Some(label.into()),
                    prompt: vec![ContentBlock::Text {
                        text: format!(
                            "{prompt}\nTeam member identity: {id}\nYour team lead is {}. Your teammate name is {label}. Use agent_team for the shared task board and peer messages. Task ownership and write scopes do not lock shared files.",
                            lead.id()
                        ),
                    }],
                    parent: lead.clone(),
                    signal: signal.clone(),
                    agent_options: None,
                    output_schema: None,
                    max_depth: Some(3),
                    tool_filter: None,
                    persona: None,
                };
                let result = self
                    .subagents
                    .start_continuable_reserved(
                        ContinuableStartSpec {
                            provider: provider.into(),
                            label: label.into(),
                            request,
                            signal: signal.clone(),
                        },
                        session_id(&id),
                    )
                    .await;
                match result {
                    Ok(_) => {
                        if let Some(child) = self.sessions.get(&session_id(&id)) {
                            self.sessions.flush(&child).await?;
                        }
                        member.phase = "active".into();
                    }
                    Err(error) => {
                        member.phase = "failed".into();
                        member.error = Some(error.to_string());
                    }
                }
                self.append(&lead, "team/member", json!({"member":member}))
                    .await?;
                if member.phase == "failed" {
                    return Err(member.error.unwrap_or_default());
                }
            }
            "message" => {
                let target = self
                    .target(
                        &board,
                        args["target"]
                            .as_str()
                            .ok_or("message target is required")?,
                    )?
                    .to_owned();
                let text = args["message"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty() && text.len() <= 16_384)
                    .ok_or("message must contain 1 to 16384 bytes")?;
                let id = args["messageId"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                if id.is_empty() || id.len() > 200 || id.chars().any(char::is_control) {
                    return Err("invalid message identity".into());
                }
                let content = vec![ContentBlock::Text { text: text.into() }];
                let mail = if let Some(existing) = board.messages.iter().find(|mail| mail.id == id)
                {
                    if existing.sender_id != caller.id().as_str()
                        || existing.target_id != target
                        || existing.content != content
                    {
                        return Err("message identity conflict".into());
                    }
                    existing.clone()
                } else {
                    if board
                        .messages
                        .iter()
                        .filter(|mail| {
                            mail.target_id == target && !board.delivered.contains(&mail.id)
                        })
                        .count()
                        >= 32
                    {
                        return Err("target mailbox is full".into());
                    }
                    let mail = Mail {
                        id: id.clone(),
                        sender_id: caller.id().to_string(),
                        sender_name: actor.clone(),
                        target_id: target.clone(),
                        content,
                    };
                    self.append(&lead, "team/message/queued", json!({"message":mail}))
                        .await?;
                    mail
                };
                if board.delivered.contains(&id) {
                    receipt = Some(json!({"messageId":id,"status":"delivered"}));
                } else if board.messages.iter().any(|previous| {
                    previous.target_id == target
                        && previous.id != id
                        && !board.delivered.contains(&previous.id)
                }) {
                    receipt = Some(json!({"messageId":id,"status":"queued"}));
                } else {
                    receipt = Some(match self.dispatch(&lead, &mail, signal).await {
                        Ok(()) => json!({"messageId":id,"status":"delivered"}),
                        Err(error) => json!({"messageId":id,"status":"queued","error":error}),
                    });
                }
            }
            "task" => {
                let id = args["taskId"].as_str().ok_or("taskId is required")?;
                let old = board.tasks.get(id);
                let expected = args["expectedRevision"]
                    .as_u64()
                    .ok_or("expectedRevision is required (0 creates a task)")?;
                if old.map_or(0, |task| task.revision) != expected {
                    return Err("task revision conflict; read status and retry".into());
                }
                if old.is_some_and(|task| task.status == "deleted") {
                    return Err("deleted tasks cannot be reused".into());
                }
                if actor != "lead"
                    && old.is_some_and(|task| {
                        task.owner_id
                            .as_deref()
                            .is_some_and(|id| id != caller.id().as_str())
                    })
                {
                    return Err("task is owned by another member".into());
                }
                if old.is_none() && board.tasks.len() >= 256 {
                    return Err("team task limit reached".into());
                }
                let owner = match args.get("owner") {
                    Some(Value::Null) => None,
                    Some(Value::String(target)) => Some(self.target(&board, target)?.to_owned()),
                    Some(_) => return Err("owner must be a teammate name or null".into()),
                    None => old.and_then(|task| task.owner_id.clone()),
                };
                if actor != "lead"
                    && owner
                        .as_deref()
                        .is_some_and(|id| id != caller.id().as_str())
                {
                    return Err("teammates can only claim tasks for themselves".into());
                }
                let task = Task {
                    id: id.into(),
                    revision: expected + 1,
                    subject: args["subject"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| old.map(|t| t.subject.clone()))
                        .ok_or("subject is required")?,
                    description: args["description"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| old.map(|t| t.description.clone()))
                        .unwrap_or_default(),
                    status: args["status"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| old.map(|t| t.status.clone()))
                        .unwrap_or("pending".into()),
                    owner_id: owner,
                    blocked_by: strings(&args, "blockedBy")?
                        .unwrap_or_else(|| old.map(|t| t.blocked_by.clone()).unwrap_or_default()),
                    write_scopes: strings(&args, "writeScopes")?
                        .unwrap_or_else(|| old.map(|t| t.write_scopes.clone()).unwrap_or_default()),
                };
                if actor != "lead"
                    && matches!(task.status.as_str(), "in_progress" | "completed")
                    && task.owner_id.as_deref() != Some(caller.id().as_str())
                {
                    return Err("claim the task before starting or completing it".into());
                }
                if actor != "lead" && task.status == "deleted" {
                    return Err("only the lead may delete tasks".into());
                }
                validate_task(&board, &task)?;
                self.append(&lead, "team/task", json!({"task":task}))
                    .await?;
            }
            "interrupt" => {
                if actor != "lead" {
                    return Err("only the lead may interrupt teammates".into());
                }
                let target =
                    self.target(&board, args["target"].as_str().ok_or("target is required")?)?;
                if target == board.team_id {
                    return Err("use the normal session stop action for the lead".into());
                }
                self.subagents
                    .interrupt(
                        &session_id(target),
                        &dsh_subagent::SubagentInterruptAuthority::Ancestor {
                            agent: lead.clone(),
                        },
                    )
                    .map_err(|e| e.to_string())?;
            }
            _ => return Err("unknown team action".into()),
        }
        let state = self.read(caller.id().as_str()).await?;
        let mut output = serde_json::to_value(&state).map_err(|e| e.to_string())?;
        for member in state.members.values() {
            output["members"][&member.name]["status"] = json!(if member.phase != "active" {
                member.phase.clone()
            } else {
                self.agents
                    .get(&session_id(&member.id))
                    .map(|agent| format!("{:?}", agent.status()).to_ascii_lowercase())
                    .unwrap_or("inactive".into())
            });
        }
        if let Some(receipt) = receipt {
            output["receipt"] = receipt;
        }
        if !pending_errors.is_empty() {
            output["pendingErrors"] = json!(pending_errors);
        }
        // Do not replay the whole mailbox into every model response.
        output.as_object_mut().unwrap().remove("messages");
        output.as_object_mut().unwrap().remove("delivered");
        output["pendingMessages"] = json!(
            state
                .messages
                .iter()
                .filter(|mail| !state.delivered.contains(&mail.id))
                .count()
        );
        output["readyTasks"] = json!(
            state
                .tasks
                .values()
                .filter(|task| task.status == "pending" && ready(&state, task))
                .map(|task| &task.id)
                .collect::<Vec<_>>()
        );
        Ok(output)
    }
}

fn strings(args: &Value, key: &str) -> Result<Option<Vec<String>>, String> {
    match args.get(key) {
        None => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|_| format!("{key} must be a string array")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_parameters_use_the_enforced_schema_subset() {
        dsh_tools::assert_object_json_schema(&parameters()).unwrap();
    }

    fn event(seq: u64, kind: &str, team: &str, mut data: Value) -> SessionEvent {
        data["teamId"] = json!(team);
        data["version"] = json!(1);
        serde_json::from_value(json!({"type":kind,"seq":seq,"time":seq+1,"data":data})).unwrap()
    }
    fn task(id: &str) -> Task {
        Task {
            id: id.into(),
            revision: 1,
            subject: id.into(),
            description: String::new(),
            status: "pending".into(),
            owner_id: None,
            blocked_by: vec![],
            write_scopes: vec![],
        }
    }
    #[test]
    fn replay_ignores_inherited_foreign_teams_and_rejects_stale_revisions() {
        let original = event(0, "team/task", "parent", json!({"task":task("inherited")}));
        let current = event(1, "team/task", "fork", json!({"task":task("own")}));
        let state = fold("fork", &[original, current.clone()]).unwrap();
        assert_eq!(state.tasks.keys().cloned().collect::<Vec<_>>(), ["own"]);
        assert!(fold("fork", &[current.clone(), current]).is_err());
    }
    #[test]
    fn task_graph_requires_completed_dependencies_and_rejects_cycles() {
        let mut state = Board::default();
        state.tasks.insert("first".into(), task("first"));
        let mut second = task("second");
        second.blocked_by = vec!["first".into()];
        assert!(!ready(&state, &second));
        second.status = "in_progress".into();
        assert!(validate_task(&state, &second).is_err());
        state.tasks.get_mut("first").unwrap().status = "completed".into();
        assert!(validate_task(&state, &second).is_ok());
        state.tasks.insert("second".into(), second);
        let mut first = task("first");
        first.blocked_by = vec!["second".into()];
        assert!(validate_task(&state, &first).is_err());
    }
    #[test]
    fn member_names_and_mailbox_identity_are_stable() {
        for invalid in ["", "lead/child", "../outside", "Upper", "has space"] {
            assert!(!name(invalid));
        }
        let member = Member {
            id: "child".into(),
            name: "worker".into(),
            description: "check".into(),
            provider: "spawn".into(),
            context: "fresh".into(),
            phase: "provisioning".into(),
            error: None,
        };
        let first = event(0, "team/member", "root", json!({"member":member}));
        let mut changed = member.clone();
        changed.id = "different".into();
        assert!(
            fold(
                "root",
                &[
                    first,
                    event(1, "team/member", "root", json!({"member":changed}))
                ]
            )
            .is_err()
        );
        let mail = Mail {
            id: "mail".into(),
            sender_id: "root".into(),
            sender_name: "lead".into(),
            target_id: "child".into(),
            content: vec![ContentBlock::Text {
                text: "hello".into(),
            }],
        };
        let queued = event(0, "team/message/queued", "root", json!({"message":mail}));
        let delivered = event(
            1,
            "team/message/delivered",
            "root",
            json!({"messageId":"mail","targetId":"child"}),
        );
        let mut active = member.clone();
        active.phase = "active".into();
        let member_event = event(0, "team/member", "root", json!({"member":active}));
        let state = fold("root", &[member_event, queued.clone(), delivered]).unwrap();
        assert!(state.delivered.contains("mail"));
        assert_eq!(state.messages.len(), 1);
        assert!(fold("root", &[queued.clone(), queued]).is_err());
    }
}

fn parameters() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["action"],"properties":{
            "action":{"type":"string","enum":["status","create","message","task","interrupt"]},"name":{"type":"string"},"description":{"type":"string"},"prompt":{"type":"string"},"context":{"type":"string","enum":["fresh","fork"]},"target":{"type":"string"},"message":{"type":"string"},"messageId":{"type":"string","description":"Stable id for retrying the same peer message; reuse it after a queued receipt."},"taskId":{"type":"string"},"expectedRevision":{"type":"integer"},"subject":{"type":"string"},"owner":{"oneOf":[{"type":"string"},{"type":"null"}],"description":"Teammate name or lead; null releases ownership (set status to pending)."},"status":{"type":"string","enum":["pending","in_progress","completed","deleted"]},"blockedBy":{"type":"array","items":{"type":"string"}},"writeScopes":{"type":"array","items":{"type":"string"}}}})
}

pub fn install(ctx: &Context, max_members: usize) -> Result<Arc<AgentTeams>, String> {
    let sessions = ctx
        .get_typed::<Arc<SessionStore>>("sessions", false)
        .ok_or("team sessions service unavailable")?
        .as_ref()
        .clone();
    let agents = ctx
        .get_typed::<Arc<AgentRegistry>>("agents", false)
        .ok_or("team agents service unavailable")?
        .as_ref()
        .clone();
    let persistence = ctx
        .get_typed::<Arc<dyn SessionPersistenceApi>>("sessionPersistence", false)
        .ok_or("team persistence service unavailable")?
        .as_ref()
        .clone();
    let subagents = ctx
        .get_typed::<Arc<SubagentRuntime>>("subagents", false)
        .ok_or("team subagent service unavailable")?
        .as_ref()
        .clone();
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .ok_or("team tools service unavailable")?
        .as_ref()
        .clone();
    let service = Arc::new(AgentTeams {
        sessions,
        agents,
        persistence,
        subagents,
        gate: tokio::sync::Mutex::new(()),
        max_members: max_members.clamp(1, 16),
    });
    let runtime = service.clone();
    tools.register(ctx,ToolDefinition{
        name:"agent_team".into(),description:"Manage an explicitly requested agent team. Use create only when the user asks for a team or teammates. The lead creates named fresh/fork teammates; members share a durable task board and peer mailbox. Read status before task updates and supply expectedRevision (0 for creation). Only claim work for yourself unless you are the lead. Wait for required results before finishing. Task ownership and writeScopes are coordination metadata, not filesystem locks.".into(),
        parameters: parameters(),
        output:ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,value|Ok(vec![ContentBlock::Text{text:value.to_string()}])),presentation_meta:None},
        timeout_ms:Some(60_000),is_concurrency_safe:Some(Arc::new(|_| false)),execute:Arc::new(move|args,exec|{let runtime=runtime.clone();let caller=exec.agent.clone();let signal=exec.signal.lock().clone();let args=args.clone();Box::pin(async move{runtime.execute(caller.ok_or_else(||ToolBodyError::plain("team tools require an agent"))?,args,signal).await.map_err(ToolBodyError::plain)})}),
        finalize_content:None,present_call:None,present_result:None,
    })?;
    ctx.register_service(service.clone());
    Ok(service)
}
