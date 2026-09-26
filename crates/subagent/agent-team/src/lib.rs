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
pub mod config;
use config::{Config, Role, SessionConfig};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_request: Option<Value>,
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
    #[serde(default)]
    pub acceptance: String,
    #[serde(default)]
    pub result: String,
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
    pub config: Option<SessionConfig>,
    pub members: BTreeMap<String, Member>,
    pub tasks: BTreeMap<String, Task>,
    pub messages: Vec<Mail>,
    pub delivered: BTreeSet<String>,
    pub cancelled: BTreeSet<String>,
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
        apply_team_event(team, &mut state, event)?;
    }
    Ok(state)
}

fn fold_session(team: &str, session: &dsh_session::Session) -> Result<Board, String> {
    let mut state = Board {
        team_id: team.into(),
        ..Default::default()
    };
    session.visit_events(0, None, |event| {
        apply_team_event(team, &mut state, event)?;
        Ok(true)
    })?;
    Ok(state)
}

fn matching_session_event(
    session: &dsh_session::Session,
    last: bool,
    matches: &impl Fn(&SessionEvent) -> bool,
) -> Result<Option<SessionEvent>, String> {
    if last {
        return session.find_event_rev(matches);
    }
    let mut found = None;
    session.visit_events(0, None, |event| {
        if matches(event) {
            found = Some(event.clone());
        }
        Ok(found.is_none())
    })?;
    Ok(found)
}

fn apply_team_event(team: &str, state: &mut Board, event: &SessionEvent) -> Result<(), String> {
    if !event.type_.starts_with("team/") || event.data["teamId"] != team {
        return Ok(());
    }
    if event.data["version"] != 1 {
        return Err("unsupported team record version".into());
    }
    match event.type_.as_str() {
        "team/config" => {
            let config: SessionConfig = serde_json::from_value(event.data["config"].clone())
                .map_err(|_| "invalid collaboration configuration")?;
            if config.revision != state.config.as_ref().map_or(1, |old| old.revision + 1)
                || !matches!(config.mode.as_str(), "off" | "auto" | "custom")
            {
                return Err("invalid collaboration configuration revision or mode".into());
            }
            if config.mode == "custom" && config.profile.is_none() {
                return Err("custom collaboration profile missing".into());
            }
            if let Some(profile) = &config.profile {
                Config {
                    profiles: vec![profile.clone()],
                    ..Default::default()
                }
                .validate()?;
            }
            state.config = Some(config);
        }
        "team/member" => {
            let member: Member = serde_json::from_value(event.data["member"].clone())
                .map_err(|_| "invalid team member record")?;
            if let Some(role) = &member.role {
                role.validate()?;
            }
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
                    || old.role != member.role
                    || old.creation_request != member.creation_request
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
        "team/message/delivered" | "team/message/cancelled" => {
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
            if event.type_ == "team/message/cancelled" {
                state.cancelled.insert(id.into());
            } else {
                state.delivered.insert(id.into());
            }
        }
        _ => {}
    }
    Ok(())
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
        || task.acceptance.len() > 16_384
        || task.result.len() > 32_768
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
        "pending"
            | "queued"
            | "in_progress"
            | "review"
            | "blocked"
            | "completed"
            | "cancelled"
            | "deleted"
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
    if matches!(
        task.status.as_str(),
        "queued" | "in_progress" | "review" | "completed"
    ) && !ready(state, task)
    {
        return Err("task dependencies are not completed".into());
    }
    Ok(())
}

pub struct AgentTeams {
    sessions: Arc<SessionStore>,
    agents: Arc<AgentRegistry>,
    persistence: Arc<dyn SessionPersistenceApi>,
    subagents: Arc<SubagentRuntime>,
    jobs: Option<Arc<dyn dsh_jobs::JobRegistry>>,
    tools: std::sync::Weak<ToolRuntime>,
    gates: std::sync::Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
    cancellation: Arc<std::sync::Mutex<BTreeMap<String, (u64, bool)>>>,
    config: std::sync::RwLock<Config>,
}
struct StoppingGuard {
    key: String,
    generation: u64,
    state: Arc<std::sync::Mutex<BTreeMap<String, (u64, bool)>>>,
}
impl Drop for StoppingGuard {
    fn drop(&mut self) {
        if let Some(state) = self.state.lock().unwrap().get_mut(&self.key) {
            if state.0 == self.generation {
                state.1 = false;
            }
        }
    }
}
impl cordis::Service for AgentTeams {
    fn service_name(&self) -> &'static str {
        "agentTeams"
    }
}

impl AgentTeams {
    pub fn configure(&self, config: Config) -> Result<(), String> {
        config.validate()?;
        *self.config.write().unwrap() = config;
        Ok(())
    }
    pub fn settings(&self) -> Config {
        self.config.read().unwrap().clone()
    }
    fn gate(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.gates
            .lock()
            .unwrap()
            .entry(id.into())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
    fn effective_config(&self, board: &Board) -> SessionConfig {
        board.config.clone().unwrap_or_else(|| SessionConfig {
            revision: 0,
            mode: "off".into(),
            profile: None,
            explicit: false,
        })
    }
    pub async fn initialize_session(&self, caller: Arc<dyn Agent>) -> Result<(), String> {
        if caller.session().header().origin.as_deref() == Some("subagent") {
            return Ok(());
        }
        let gate = self.gate(caller.id().as_str());
        let _guard = gate.lock().await;
        if self.read(caller.id().as_str()).await?.config.is_some() {
            return Ok(());
        }
        let defaults = self.settings();
        self.append(&caller,"team/config",json!({"config":SessionConfig{revision:1,mode:defaults.default_mode,profile:defaults.profiles.into_iter().find(|profile|profile.id==defaults.default_profile),explicit:false}})).await
    }
    pub fn manages_cancellation(&self, caller: &Arc<dyn Agent>) -> bool {
        let Ok(board) = fold_session(caller.id().as_str(), caller.session()) else {
            return false;
        };
        self.effective_config(&board).mode != "off"
            || board.members.values().any(|member| {
                self.agents
                    .get(&session_id(&member.id))
                    .is_some_and(|agent| {
                        agent.status() == dsh_agent::AgentStatus::Running
                            || self
                                .jobs
                                .as_ref()
                                .is_some_and(|jobs| jobs.has_owner_activity(&agent))
                    })
            })
    }
    /// Resolve controls through the same service as model tools. The HTTP owner
    /// has already resolved a top-level session through the normal API resolver.
    pub async fn control(&self, caller: Arc<dyn Agent>, args: Value) -> Result<Value, String> {
        if caller.session().header().origin.as_deref() == Some("subagent") {
            return Err("member controls require the main conversation".into());
        }
        if args["action"] == "interrupt" || args["action"] == "stopAll" {
            let board = self.read(caller.id().as_str()).await?;
            let all = args["action"] == "stopAll";
            let target = if all {
                board.team_id.as_str()
            } else {
                self.target(&board, args["target"].as_str().ok_or("choose a member")?)?
            };
            if !all && target == board.team_id {
                return Err("use stopAll for the main conversation".into());
            }
            let _stopping = if all {
                let mut cancellation = self.cancellation.lock().unwrap();
                let state = cancellation.entry(board.team_id.clone()).or_default();
                state.0 += 1;
                state.1 = true;
                Some(StoppingGuard {
                    key: board.team_id.clone(),
                    generation: state.0,
                    state: self.cancellation.clone(),
                })
            } else {
                None
            };
            let mut stopped = BTreeSet::from([target.to_owned()]);
            let live = self.agents.list();
            loop {
                let before = stopped.len();
                for agent in &live {
                    if agent
                        .session()
                        .header()
                        .parent_session
                        .as_ref()
                        .is_some_and(|id| stopped.contains(id.as_str()))
                    {
                        stopped.insert(agent.id().to_string());
                    }
                }
                if before == stopped.len() {
                    break;
                }
            }
            let mut suppression_errors = Vec::new();
            if all {
                for id in &stopped {
                    if let Err(error) = self.subagents.suppress_settlement(&session_id(id), &caller)
                    {
                        suppression_errors.push(error.to_string());
                    }
                }
            }
            for agent in &live {
                if stopped.contains(agent.id().as_str()) {
                    agent.cancel(dsh_session::AgentCancelCause::User, None);
                }
            }
            let gate = self.gate(&board.team_id);
            let _guard = gate.lock().await;
            let outcome =
                async {
                    let current = self.read(&board.team_id).await?;
                    for mail in current.messages.iter().filter(|mail| {
                        stopped.contains(&mail.target_id)
                            && !current.delivered.contains(&mail.id)
                            && !current.cancelled.contains(&mail.id)
                    }) {
                        self.append(
                            &caller,
                            "team/message/cancelled",
                            json!({"messageId":mail.id,"targetId":mail.target_id}),
                        )
                        .await?;
                    }
                    for task in current.tasks.values().filter(|task| {
                        task.owner_id
                            .as_ref()
                            .is_some_and(|id| stopped.contains(id))
                            && matches!(task.status.as_str(), "queued" | "in_progress")
                    }) {
                        let mut updated = task.clone();
                        updated.status = "blocked".into();
                        updated.revision += 1;
                        self.append(&caller, "team/task", json!({"task":updated}))
                            .await?;
                    }
                    let mut errors = suppression_errors;
                    for agent in &live {
                        if stopped.contains(agent.id().as_str()) {
                            if tokio::time::timeout(
                                std::time::Duration::from_secs(5),
                                agent.when_idle(),
                            )
                            .await
                            .is_err()
                            {
                                errors.push(format!("{} cancellation did not settle", agent.id()));
                            }
                            if let Some(jobs) = &self.jobs {
                                let owned = jobs
                                    .list(Some(agent))
                                    .into_iter()
                                    .filter(|job| {
                                        job.owner_session.as_ref() == Some(agent.id())
                                            && !job.status.is_terminal()
                                    })
                                    .collect::<Vec<_>>();
                                for job in &owned {
                                    if let Err(error) = jobs.kill(
                                        &job.id,
                                        Some(agent),
                                        Some("collaboration stopped".into()),
                                    ) {
                                        errors.push(error);
                                    }
                                }
                                for job in owned {
                                    match jobs.wait(&job.id, 5000, Some(agent), None).await {
                                        Ok(snapshot) if snapshot.status.is_terminal() => {}
                                        Ok(_) => errors.push(format!(
                                            "{} background work is still stopping",
                                            job.id
                                        )),
                                        Err(error) => errors.push(error),
                                    }
                                }
                            }
                            if self.sessions.get(agent.id()).is_some_and(|session| {
                                session.identity() == agent.session().identity()
                            }) {
                                match self.sessions.flush(agent.session()).await {
                                    Ok(true) => {}
                                    Ok(false) => errors.push(format!(
                                        "{} cancellation has no durability listener",
                                        agent.id()
                                    )),
                                    Err(error) => errors.push(error),
                                }
                            }
                        }
                    }
                    if !errors.is_empty() {
                        return Err(errors.join("; "));
                    }
                    self.view(&board.team_id).await
                }
                .await;
            return outcome;
        }
        if !matches!(
            args["action"].as_str(),
            Some("configure" | "create" | "message" | "task" | "dispatch" | "status")
        ) {
            return Err("unknown collaboration control".into());
        }
        let id = caller.id().to_string();
        let result = self.execute(caller, args, Arc::new(|| false)).await?;
        let mut view = self.view(&id).await?;
        for key in ["receipt", "pendingErrors"] {
            if let Some(value) = result.get(key) {
                view[key] = value.clone();
            }
        }
        Ok(view)
    }
    pub fn prompt_context(&self, id: &str) -> String {
        if !self.settings().enabled {
            return "Collaboration is disabled. Do not create team members.".into();
        }
        let Some(session) = self.sessions.get(&session_id(id)) else {
            return String::new();
        };
        if session.header().origin.as_deref() == Some("subagent") {
            return String::new();
        }
        let Ok(board) = fold_session(id, &session) else {
            return "Collaboration state cannot be read; report the error before delegation."
                .into();
        };
        let config = self.effective_config(&board);
        if config.mode == "off" {
            return "Use the current agent for this conversation. Only create teammates when the user explicitly requests them; an explicitly disabled collaboration configuration rejects new members.".into();
        }
        format!(
            "The user enabled collaboration in this main conversation. You remain responsible for the goal and final acceptance. Use agent_team to maintain one task board, delegate bounded work only when useful, and open fresh member contexts with relevant files and acceptance criteria. Use roleId from this saved profile when present: {}. Do not create a fixed planner/supervisor hierarchy. Reuse member conversations via message, preserve task revisions, await required results using agent_team action wait rather than repeatedly polling models, and inspect evidence before marking work completed. Tools and permission approvals remain enforced by the runtime. Member model and tool settings are enforced at creation. Config revision: {}.",
            serde_json::to_string(&config.profile).unwrap_or_default(),
            config.revision
        )
    }
    async fn matching_event(
        &self,
        id: &str,
        last: bool,
        matches: impl Fn(&SessionEvent) -> bool + Send + Sync + 'static,
    ) -> Result<(dsh_session::SessionHeader, Option<SessionEvent>), String> {
        if let Some(session) = self.sessions.get(&session_id(id)) {
            let found = matching_session_event(&session, last, &matches)?;
            return Ok((session.header().clone(), found));
        }
        let id = session_id(id);
        let before = self
            .persistence
            .read_snapshot(&id)
            .await?
            .ok_or("team session does not exist")?;
        let found = Arc::new(std::sync::Mutex::new(None));
        let for_visit = found.clone();
        self.persistence
            .visit_nonpacked_events(
                &id,
                Arc::new(move |events| {
                    let mut found = for_visit
                        .lock()
                        .map_err(|_| "team history query poisoned")?;
                    for event in events {
                        if matches(event) {
                            *found = Some(event.clone());
                            if !last {
                                return Ok(false);
                            }
                        }
                    }
                    Ok(true)
                }),
            )
            .await?;
        if self.persistence.read_snapshot(&id).await?.as_ref() != Some(&before) {
            return Err("team history changed during query".into());
        }
        let found = found
            .lock()
            .map_err(|_| "team history query poisoned")?
            .take();
        Ok((before.header, found))
    }
    pub async fn read(&self, id: &str) -> Result<Board, String> {
        let header = if let Some(session) = self.sessions.get(&session_id(id)) {
            session.header().clone()
        } else {
            self.persistence
                .read_snapshot(&session_id(id))
                .await?
                .ok_or("team session does not exist")?
                .header
        };
        if header.origin.as_deref() != Some("subagent") {
            return self.read_board(id).await;
        }
        let parent = header
            .parent_session
            .as_ref()
            .ok_or("team member has no parent")?;
        let board = self.read_board(parent.as_str()).await?;
        if !board
            .members
            .values()
            .any(|member| member.id == id && member.phase == "active")
        {
            return Err("session is not a registered teammate".into());
        }
        Ok(board)
    }
    async fn read_board(&self, id: &str) -> Result<Board, String> {
        if let Some(session) = self.sessions.get(&session_id(id)) {
            return fold_session(id, &session);
        }
        let before = self
            .persistence
            .read_snapshot(&session_id(id))
            .await?
            .ok_or("team session does not exist")?;
        let state = Arc::new(std::sync::Mutex::new(Board {
            team_id: id.into(),
            ..Default::default()
        }));
        let for_visit = state.clone();
        let team = id.to_string();
        self.persistence
            .visit_nonpacked_events(
                &session_id(id),
                Arc::new(move |events| {
                    let mut state = for_visit.lock().map_err(|_| "team replay state poisoned")?;
                    for event in events {
                        apply_team_event(&team, &mut state, event)?;
                    }
                    Ok(true)
                }),
            )
            .await?;
        if self
            .persistence
            .read_snapshot(&session_id(id))
            .await?
            .as_ref()
            != Some(&before)
        {
            return Err("team history changed during replay".into());
        }
        let result = state
            .lock()
            .map_err(|_| "team replay state poisoned")?
            .clone();
        Ok(result)
    }
    pub async fn view(&self, id: &str) -> Result<Value, String> {
        let board = self.read(id).await?;
        let mut view = serde_json::to_value(&board).map_err(|error| error.to_string())?;
        view["config"] = json!(self.effective_config(&board));
        view["leadRunning"] = json!(self.agents.get(&session_id(&board.team_id)).is_some_and(
            |a| {
                a.status() == dsh_agent::AgentStatus::Running
                    || self
                        .jobs
                        .as_ref()
                        .is_some_and(|jobs| jobs.has_owner_activity(&a))
            }
        ));
        for member in board.members.values() {
            let status = if member.phase != "active" {
                member.phase.clone()
            } else {
                self.agents
                    .get(&session_id(&member.id))
                    .map(|agent| {
                        if self
                            .jobs
                            .as_ref()
                            .is_some_and(|jobs| jobs.has_owner_activity(&agent))
                        {
                            "running".into()
                        } else {
                            format!("{:?}", agent.status()).to_ascii_lowercase()
                        }
                    })
                    .unwrap_or("inactive".into())
            };
            view["members"][&member.name]["status"] = json!(status);
            view["members"][&member.name]["jobs"] = json!(
                self.jobs
                    .as_ref()
                    .map(|jobs| jobs
                        .list_for_session(&session_id(&member.id))
                        .into_iter()
                        .filter(|job| job
                            .owner_session
                            .as_ref()
                            .is_some_and(|owner| owner.as_str() == member.id))
                        .map(|job| json!({"id":job.id,"status":job.status.as_str()}))
                        .collect::<Vec<_>>())
                    .unwrap_or_default()
            );
            if member.phase == "active" && status != "running" {
                if let Ok((_, Some(end))) = self
                    .matching_event(&member.id, true, |event| event.type_ == "turn/end")
                    .await
                {
                    let reason = &end.data["reason"];
                    view["members"][&member.name]["lastOutcome"] = reason.clone();
                    if reason["kind"] == "error" {
                        view["members"][&member.name]["status"] = json!("failed");
                        view["members"][&member.name]["error"] = reason["error"]["message"].clone();
                    } else if matches!(
                        reason["kind"].as_str(),
                        Some("aborted" | "interrupted" | "blocked" | "max-tokens")
                    ) {
                        view["members"][&member.name]["status"] = json!("blocked");
                    }
                }
            }
            view["members"][&member.name]
                .as_object_mut()
                .unwrap()
                .remove("creationRequest");
        }
        view.as_object_mut().unwrap().remove("messages");
        view["mailbox"]=json!(board.messages.iter().rev().take(50).map(|mail|json!({"id":mail.id,"sender":mail.sender_name,"targetId":mail.target_id,"content":mail.content,"delivered":board.delivered.contains(&mail.id),"cancelled":board.cancelled.contains(&mail.id)})).collect::<Vec<_>>());
        view.as_object_mut().unwrap().remove("delivered");
        view.as_object_mut().unwrap().remove("cancelled");
        view["pendingMessages"] = json!(
            board
                .messages
                .iter()
                .filter(|mail| !board.delivered.contains(&mail.id)
                    && !board.cancelled.contains(&mail.id))
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
        let team = team.to_string();
        let message_id = mail.id.clone();
        let (_, event) = self
            .matching_event(&mail.target_id, false, move |event| {
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
                        && message["source"]["messageId"] == message_id
                })
            })
            .await?;
        Ok(event.is_some())
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
                .matching_event(&member.id, false, move |event| {
                    matches!(event.type_.as_str(), "user/message" | "agent/inbox/spliced")
                        && serde_json::to_string(&event.data)
                            .is_ok_and(|data| data.contains(&marker))
                })
                .await
                .ok()
                .is_some_and(|(header, event)| {
                    header.parent_session.as_ref() == Some(lead.id())
                        && header.origin.as_deref() == Some("subagent")
                        && event.is_some()
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
        for mail in board.messages.iter().filter(|mail| {
            !board.delivered.contains(&mail.id) && !board.cancelled.contains(&mail.id)
        }) {
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
        if args["action"] == "interrupt" {
            return Box::pin(self.control(caller, args)).await;
        }
        if args["action"] == "wait" {
            let board = self.read(caller.id().as_str()).await?;
            self.actor(&board, &caller)?;
            let target = args["target"].as_str().filter(|target| *target != "all");
            let target = target
                .map(|target| self.target(&board, target).map(str::to_owned))
                .transpose()?;
            if target.as_deref() == Some(caller.id().as_str()) {
                return Err("a member cannot wait for itself".into());
            }
            let timeout = args
                .get("timeoutMs")
                .map(|v| {
                    v.as_u64()
                        .filter(|n| (100..=50_000).contains(n))
                        .ok_or("timeoutMs must be 100 to 50000")
                })
                .transpose()?
                .unwrap_or(30_000);
            let members = board
                .members
                .values()
                .filter(|m| {
                    m.id != caller.id().as_str() && target.as_ref().is_none_or(|id| id == &m.id)
                })
                .filter_map(|m| self.agents.get(&session_id(&m.id)))
                .collect::<Vec<_>>();
            let waiting = async {
                for member in members {
                    member.when_idle().await;
                }
            };
            let cancelled = async {
                loop {
                    if signal() {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
            };
            let timed_out = tokio::select! {
                _=cancelled=>return Err("collaboration wait cancelled".into()),
                outcome=tokio::time::timeout(std::time::Duration::from_millis(timeout),waiting)=>outcome.is_err(),
            };
            let mut result = self.view(&board.team_id).await?;
            result.as_object_mut().unwrap().remove("mailbox");
            result["waitTimedOut"] = json!(timed_out);
            return Ok(result);
        }
        let team = self.read(caller.id().as_str()).await?.team_id;
        let (generation, stopping) = self
            .cancellation
            .lock()
            .unwrap()
            .get(&team)
            .copied()
            .unwrap_or_default();
        if stopping {
            return Err("collaboration is stopping; wait before submitting more work".into());
        }
        let epochs = self.cancellation.clone();
        let owner = team.clone();
        let signal: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
            signal()
                || epochs
                    .lock()
                    .unwrap()
                    .get(&owner)
                    .is_some_and(|s| s.0 != generation || s.1)
        });
        let gate = self.gate(&team);
        let _gate = gate.lock().await;
        let mut board = self.read(caller.id().as_str()).await?;
        let actor = self.actor(&board, &caller)?.to_owned();
        let lead = self
            .agents
            .get(&session_id(&board.team_id))
            .ok_or("team lead is not active")?;
        if signal() {
            return Err("team action cancelled".into());
        }
        let settings = self.settings();
        let action = args["action"].as_str().unwrap_or("status");
        if !settings.enabled && matches!(action, "create" | "configure" | "dispatch") {
            return Err("collaboration is disabled in settings".into());
        }
        // A status query must not dispatch queued work, including after an
        // environment change. Explicit communication retains mailbox recovery.
        let pending_errors = if settings.enabled && matches!(action, "message" | "recover") {
            self.recover(&lead, &board, signal.clone()).await?
        } else {
            vec![]
        };
        let mut receipt = None;
        if signal() {
            return Err("team action cancelled".into());
        }
        board = self.read(caller.id().as_str()).await?;
        match args["action"].as_str().unwrap_or("status") {
            "status" | "recover" => {}
            "configure" => {
                if actor != "lead" {
                    return Err("only the main conversation may configure collaboration".into());
                }
                if lead.status() == dsh_agent::AgentStatus::Running
                    || self
                        .jobs
                        .as_ref()
                        .is_some_and(|jobs| jobs.has_owner_activity(&lead))
                    || board.members.values().any(|m| {
                        self.agents.get(&session_id(&m.id)).is_some_and(|a| {
                            a.status() == dsh_agent::AgentStatus::Running
                                || self
                                    .jobs
                                    .as_ref()
                                    .is_some_and(|jobs| jobs.has_owner_activity(&a))
                        })
                    })
                {
                    return Err("请先停止或等待当前执行完成，再更改协作方式。".into());
                }
                let old = board.config.as_ref().map_or(0, |c| c.revision);
                if args["expectedRevision"].as_u64() != Some(old) {
                    return Err("collaboration configuration conflict; refresh and retry".into());
                }
                let mode = args["mode"]
                    .as_str()
                    .filter(|s| matches!(*s, "off" | "auto" | "custom"))
                    .ok_or("invalid collaboration mode")?;
                let profile_id = args["profileId"].as_str().unwrap_or("");
                let profile = if profile_id.is_empty() {
                    None
                } else {
                    Some(
                        settings
                            .profiles
                            .iter()
                            .find(|p| p.id == profile_id)
                            .cloned()
                            .ok_or("collaboration profile no longer exists")?,
                    )
                };
                if mode == "custom" && profile.is_none() {
                    return Err("choose a collaboration profile".into());
                }
                self.append(&lead,"team/config",json!({"config":SessionConfig{revision:old+1,mode:mode.into(),profile,explicit:true}})).await?;
            }
            "create" => {
                if actor != "lead" {
                    return Err("only the lead may create teammates".into());
                }
                let label = args["name"].as_str().ok_or("teammate name is required")?;
                if let Some(existing) = board.members.get(label) {
                    if args["requestId"].as_str().is_some_and(|id| !id.is_empty())
                        && existing.creation_request.as_ref() == Some(&args)
                    {
                        if existing.phase == "failed" {
                            return Err(existing.error.clone().unwrap_or_else(||"member creation failed; revise the request before creating a replacement".into()));
                        }
                        return self.view(lead.id().as_str()).await;
                    }
                }
                if args
                    .get("requestId")
                    .is_some_and(|v| v.as_str().is_none_or(|s| s.is_empty() || s.len() > 200))
                {
                    return Err("invalid create request identity".into());
                }
                if board
                    .config
                    .as_ref()
                    .is_some_and(|c| c.mode == "off" && c.explicit)
                {
                    return Err(
                        "enable collaboration in this conversation before creating members".into(),
                    );
                }
                if !name(label)
                    || label == "lead"
                    || board.members.contains_key(label)
                    || board
                        .members
                        .values()
                        .filter(|m| m.phase != "failed")
                        .count()
                        >= settings.max_members
                    || board.members.len() >= 256
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
                let mut config = self.effective_config(&board);
                if config.mode == "off" && !config.explicit {
                    config.mode = "auto".into();
                }
                let role = match args["roleId"].as_str().filter(|s| !s.is_empty()) {
                    Some(role_id) => Some(
                        config
                            .profile
                            .as_ref()
                            .and_then(|p| p.roles.iter().find(|r| r.id == role_id))
                            .cloned()
                            .ok_or("role is not in the current collaboration profile")?,
                    ),
                    None => None,
                };
                if config.mode == "custom" && role.is_none() {
                    return Err("choose a role from the configured profile".into());
                }
                if let Some(role) = &role {
                    let tools = self.tools.upgrade().ok_or("tool registry is unavailable")?;
                    for tool in &role.allow_tools {
                        if tool == "run_code" || tools.get(tool, Some(lead.scope_key())).is_none() {
                            return Err(format!(
                                "role tool is unavailable in the parent scope: {tool}"
                            ));
                        }
                    }
                }
                if board.config.is_none()
                    || board
                        .config
                        .as_ref()
                        .is_some_and(|previous| previous.mode != config.mode)
                {
                    config.revision = board
                        .config
                        .as_ref()
                        .map_or(1, |previous| previous.revision + 1);
                    self.append(&lead, "team/config", json!({"config":config}))
                        .await?;
                }
                let mut member = Member {
                    id: id.clone(),
                    name: label.into(),
                    description: args["description"].as_str().unwrap_or(label).into(),
                    provider: provider.into(),
                    context: context.into(),
                    phase: "provisioning".into(),
                    error: None,
                    role: role.clone(),
                    creation_request: args.get("requestId").map(|_| args.clone()),
                };
                self.append(&lead, "team/member", json!({"member":member}))
                    .await?;
                let request = SubagentStartRequest {
                    label: Some(label.into()),
                    prompt: vec![ContentBlock::Text {
                        text: format!(
                            "{prompt}\nRole instructions: {}\nTeam member identity: {id}\nYour team lead is {}. Your teammate name is {label}. Use agent_team for the shared task board and peer messages. Task ownership and write scopes do not lock shared files.",
                            role.as_ref().map(|r| r.instructions.as_str()).unwrap_or(""),
                            lead.id()
                        ),
                    }],
                    parent: lead.clone(),
                    signal: signal.clone(),
                    agent_options: role.as_ref().map(Role::agent_options),
                    output_schema: None,
                    max_depth: Some(if role.as_ref().is_none_or(|r| r.can_spawn) {
                        3
                    } else {
                        lead.options().subagent_depth.unwrap_or(0) + 1
                    }),
                    tool_filter: role.as_ref().and_then(Role::tool_filter),
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
                if board.cancelled.contains(&id) {
                    return Err(
                        "message was cancelled; use a new message identity for new work".into(),
                    );
                }
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
                            mail.target_id == target
                                && !board.delivered.contains(&mail.id)
                                && !board.cancelled.contains(&mail.id)
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
                        && !board.cancelled.contains(&previous.id)
                }) {
                    receipt = Some(json!({"messageId":id,"status":"queued"}));
                } else {
                    receipt = Some(match self.dispatch(&lead, &mail, signal).await {
                        Ok(()) => json!({"messageId":id,"status":"delivered"}),
                        Err(error) => json!({"messageId":id,"status":"queued","error":error}),
                    });
                }
            }
            "dispatch" => {
                if actor != "lead" {
                    return Err("only the main conversation may dispatch tasks".into());
                }
                let id = args["taskId"].as_str().ok_or("taskId is required")?;
                let expected = args["expectedRevision"]
                    .as_u64()
                    .ok_or("expectedRevision is required")?;
                let mail_id = format!("dispatch-{id}-{expected}");
                if board.messages.iter().any(|m| m.id == mail_id) {
                    return self.view(&board.team_id).await;
                }
                let mut task = board.tasks.get(id).cloned().ok_or("task does not exist")?;
                if task.revision != expected {
                    return Err("task revision conflict; refresh and retry".into());
                }
                if !matches!(task.status.as_str(), "pending" | "blocked" | "cancelled")
                    || !ready(&board, &task)
                {
                    return Err(
                        "task is already dispatched or its dependencies are incomplete".into(),
                    );
                }
                let target = task
                    .owner_id
                    .as_deref()
                    .ok_or("assign a member before dispatch")?;
                if target == board.team_id {
                    return Err("dispatch requires a member; the main conversation executes its own work directly".into());
                }
                let target = self.target(&board, target)?.to_owned();
                if board.tasks.values().any(|other| {
                    other.id != id
                        && other.owner_id.as_deref() == Some(&target)
                        && matches!(other.status.as_str(), "queued" | "in_progress")
                }) || self.agents.get(&session_id(&target)).is_some_and(|a| {
                    a.status() == dsh_agent::AgentStatus::Running
                        || self
                            .jobs
                            .as_ref()
                            .is_some_and(|jobs| jobs.has_owner_activity(&a))
                }) {
                    return Err(
                        "member is busy; wait or stop its current work before dispatch".into(),
                    );
                }
                task.status = "queued".into();
                task.revision += 1;
                validate_task(&board, &task)?;
                self.append(&lead, "team/task", json!({"task":task}))
                    .await?;
                let mail = Mail {
                    id: mail_id.clone(),
                    sender_id: lead.id().to_string(),
                    sender_name: "lead".into(),
                    target_id: target,
                    content: vec![ContentBlock::Text {
                        text: format!(
                            "Task {}: {}\n{}\nAcceptance: {}\nCoordinate writes within: {} (these are coordination scopes, not permission grants).\nWhen finished, record concrete results with agent_team task, expectedRevision {}, status review. Do not claim final acceptance yourself.",
                            task.id,
                            task.subject,
                            task.description,
                            task.acceptance,
                            task.write_scopes.join(", "),
                            task.revision
                        ),
                    }],
                };
                self.append(&lead, "team/message/queued", json!({"message":mail}))
                    .await?;
                receipt = Some(match self.dispatch(&lead, &mail, signal.clone()).await {
                    Ok(()) => json!({"messageId":mail_id,"status":"delivered"}),
                    Err(error) => json!({"messageId":mail_id,"status":"queued","error":error}),
                });
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
                let mut task = Task {
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
                    acceptance: args["acceptance"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| old.map(|t| t.acceptance.clone()))
                        .unwrap_or_default(),
                    result: args["result"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| old.map(|t| t.result.clone()))
                        .unwrap_or_default(),
                };
                if actor != "lead"
                    && matches!(
                        task.status.as_str(),
                        "queued" | "in_progress" | "review" | "blocked" | "completed" | "cancelled"
                    )
                    && task.owner_id.as_deref() != Some(caller.id().as_str())
                {
                    return Err("claim the task before starting or completing it".into());
                }
                if actor != "lead" && task.status == "deleted" {
                    return Err("only the lead may delete tasks".into());
                }
                if actor != "lead" && task.status == "completed" {
                    task.status = "review".into();
                }
                if old.is_some_and(|old| {
                    matches!(old.status.as_str(), "queued" | "in_progress")
                        && (old.owner_id != task.owner_id
                            || old.subject != task.subject
                            || old.description != task.description
                            || old.acceptance != task.acceptance
                            || old.blocked_by != task.blocked_by
                            || old.write_scopes != task.write_scopes)
                }) {
                    return Err(
                        "stop the assigned member before changing active task requirements".into(),
                    );
                }
                if task.status == "completed"
                    && !task.acceptance.trim().is_empty()
                    && task.result.trim().is_empty()
                {
                    return Err("record acceptance evidence before completing this task".into());
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
        output["config"] = json!(self.effective_config(&state));
        for member in state.members.values() {
            output["members"][&member.name]
                .as_object_mut()
                .unwrap()
                .remove("creationRequest");
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
        output.as_object_mut().unwrap().remove("cancelled");
        output["pendingMessages"] = json!(
            state
                .messages
                .iter()
                .filter(|mail| !state.delivered.contains(&mail.id)
                    && !state.cancelled.contains(&mail.id))
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
        for timeout in [0, 99, 50001, 600000] {
            assert!(
                !dsh_tools::validate_json_schema_value(
                    &parameters(),
                    &json!({"action":"wait","timeoutMs":timeout}),
                    "arguments"
                )
                .is_empty()
            );
        }
        for timeout in [100, 30000, 50000] {
            assert!(
                dsh_tools::validate_json_schema_value(
                    &parameters(),
                    &json!({"action":"wait","timeoutMs":timeout}),
                    "arguments"
                )
                .is_empty()
            );
        }
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
            acceptance: String::new(),
            result: String::new(),
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
    fn archived_board_keeps_team_scope_and_revision_validation() {
        let events = vec![
            event(0, "team/task", "parent", json!({"task":task("inherited")})),
            event(
                1,
                "assistant/chunk",
                "fork",
                json!({"opaque":"x".repeat(128 * 1024)}),
            ),
            event(2, "team/task", "fork", json!({"task":task("own")})),
        ];
        let mut builder =
            dsh_session::event_archive::EventArchiveBuilder::new(&std::env::temp_dir()).unwrap();
        for event in &events {
            builder.push(event).unwrap();
        }
        let header = dsh_session::snapshot_session_header(&session_id("fork"), None).unwrap();
        let session = dsh_session::Session::from_event_archive(
            header.id.clone(),
            builder.finish().unwrap(),
            &header,
            dsh_session::SessionLogOffset::ZERO,
            vec![],
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(fold_session("fork", &session).unwrap()).unwrap(),
            serde_json::to_value(fold("fork", &events).unwrap()).unwrap()
        );
        session
            .append("team/task", events[2].data.clone(), None)
            .unwrap();
        assert!(fold_session("fork", &session).is_err());
    }

    #[test]
    fn archived_queries_keep_first_admission_and_latest_outcome() {
        let events = vec![
            event(0, "agent/inbox/spliced", "child", json!({"marker":"first"})),
            event(
                1,
                "assistant/chunk",
                "child",
                json!({"marker":"noise", "text":"x".repeat(1024 * 1024)}),
            ),
            event(
                2,
                "turn/end",
                "child",
                json!({"turn":1,"reason":{"kind":"error"}}),
            ),
            event(3, "agent/inbox/spliced", "child", json!({"marker":"later"})),
            event(
                4,
                "turn/end",
                "child",
                json!({"turn":2,"reason":{"kind":"stop"}}),
            ),
        ];
        let mut archive =
            dsh_session::event_archive::EventArchiveBuilder::new(&std::env::temp_dir()).unwrap();
        for event in &events {
            archive.push(event).unwrap();
        }
        let header = dsh_session::snapshot_session_header(&session_id("child"), None).unwrap();
        let session = dsh_session::Session::from_event_archive(
            header.id.clone(),
            archive.finish().unwrap(),
            &header,
            dsh_session::SessionLogOffset::ZERO,
            vec![],
        )
        .unwrap();
        assert_eq!(
            matching_session_event(&session, false, &|event| event.type_
                == "agent/inbox/spliced")
            .unwrap(),
            Some(events[0].clone())
        );
        assert_eq!(
            matching_session_event(&session, true, &|event| event.type_ == "turn/end").unwrap(),
            Some(events[4].clone())
        );
        assert!(
            matching_session_event(&session, false, &|event| event.type_ == "model/selection")
                .unwrap()
                .is_none()
        );
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
            role: None,
            creation_request: None,
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
            "action":{"type":"string","enum":["status","create","message","task","dispatch","interrupt","wait","recover"]},"timeoutMs":{"type":"integer","minimum":100,"maximum":50000,"description":"Wait budget in milliseconds, 100–50000; defaults to 30000. Call wait again if members are still running."},"roleId":{"type":"string"},"requestId":{"type":"string"},"acceptance":{"type":"string"},"result":{"type":"string"},"name":{"type":"string"},"description":{"type":"string"},"prompt":{"type":"string"},"context":{"type":"string","enum":["fresh","fork"]},"target":{"type":"string"},"message":{"type":"string"},"messageId":{"type":"string","description":"Stable id for retrying the same peer message; reuse it after a queued receipt."},"taskId":{"type":"string"},"expectedRevision":{"type":"integer"},"subject":{"type":"string"},"owner":{"oneOf":[{"type":"string"},{"type":"null"}],"description":"Teammate name or lead; null releases ownership (set status to pending)."},"status":{"type":"string","enum":["pending","queued","in_progress","review","blocked","completed","cancelled","deleted"]},"blockedBy":{"type":"array","items":{"type":"string"}},"writeScopes":{"type":"array","items":{"type":"string"}}}})
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
        jobs: ctx
            .get_typed::<Arc<dyn dsh_jobs::JobRegistry>>("jobs", false)
            .map(|slot| slot.as_ref().clone()),
        tools: Arc::downgrade(&tools),
        gates: std::sync::Mutex::new(BTreeMap::new()),
        cancellation: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
        config: std::sync::RwLock::new(Config {
            enabled: true,
            max_members: max_members.clamp(1, 16),
            ..Default::default()
        }),
    });
    let runtime = service.clone();
    tools.register(ctx,ToolDefinition{
        name:"agent_team".into(),description:"Manage an explicitly requested agent team. Use create only when the user asks for a team or teammates. The lead creates named fresh/fork teammates; members share a durable task board and peer mailbox. Use recover to retry durable queued deliveries under current permissions; status and wait do not dispatch queued messages. Read status before task updates and supply expectedRevision (0 for creation). Only claim work for yourself unless you are the lead. Wait for required results before finishing. Task ownership and writeScopes are coordination metadata, not filesystem locks.".into(),
        parameters: parameters(),
        output:ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,value|Ok(vec![ContentBlock::Text{text:value.to_string()}])),presentation_meta:None},
        timeout_ms:Some(60_000),is_concurrency_safe:Some(Arc::new(|_| false)),execute:Arc::new(move|args,exec|{let runtime=runtime.clone();let caller=exec.agent.clone();let signal=exec.signal.lock().clone();let args=args.clone();Box::pin(async move{runtime.execute(caller.ok_or_else(||ToolBodyError::plain("team tools require an agent"))?,args,signal).await.map_err(ToolBodyError::plain)})}),
        finalize_content:None,present_call:None,present_result:None,
    })?;
    ctx.register_service(service.clone());
    Ok(service)
}
