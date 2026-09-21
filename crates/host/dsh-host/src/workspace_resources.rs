//! Host integration for managed execution resources and bounded tool results.
use cordis::{Context, make_disposer};
use dsh_schemastery::{Data, Schema};
use dsh_settings::{SettingsProvider, SettingsRegisterOptions, settings_namespace};
use dsh_workspace_resources::{Policy, Resource, Store, digest, persist_json};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(test)]
#[path = "workspace_resources_contract_tests.rs"]
mod contract_tests;

fn scratch_parameters() -> Value {
    let mut schema = json!({
        "type":"object",
        "properties":{
            "action":{"type":"string","enum":["allocate","prepare_copy","inspect","promote","write","read","list","pin","release"],"description":"write requires id,path,content; read requires id,path; inspect requires target; promote requires id,path,target,expectedSha256; pin/release require id."},
            "id":{"type":"string","minLength":1},
            "kind":{"type":"string","enum":["script","candidate","log","copy"]},
            "label":{"type":"string"},
            "path":{"type":"string","minLength":1},
            "content":{"type":"string"},
            "offset":{"type":"integer","minimum":0,"description":"Non-negative character offset"},
            "limit":{"type":"integer","minimum":1,"maximum":32000,"description":"1 to 32000 characters"},
            "pinned":{"type":"boolean"},
            "target":{"type":"string","minLength":1},
            "expectedSha256":{"oneOf":[{"type":"string"},{"type":"null"}],"description":"Required for promote: use inspect's sha256, or null only when the target does not exist."},
            "files":{"type":"array","items":{"type":"string"}}
        },
        "required":["action"],"additionalProperties":false
    });
    let variants: &[(&[&str], &[&str])] = &[
        (&["allocate", "prepare_copy", "list"], &["action"]),
        (&["inspect"], &["action", "target"]),
        (
            &["promote"],
            &["action", "id", "path", "target", "expectedSha256"],
        ),
        (&["write"], &["action", "id", "path", "content"]),
        (&["read"], &["action", "id", "path"]),
        (&["pin", "release"], &["action", "id"]),
    ];
    schema["oneOf"] = Value::Array(
        variants
            .iter()
            .map(|(actions, required)| {
                // Root properties supply the types; each disjoint branch contributes
                // the fields that its action requires without duplicating the schema.
                let mut properties = serde_json::Map::new();
                for field in *required {
                    properties.insert((*field).into(), json!({}));
                }
                properties.insert("action".into(), json!({"type":"string","enum":actions}));
                json!({"type":"object","properties":properties,"required":required})
            })
            .collect(),
    );
    schema
}

fn validate_location(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("垃圾槽位置必须为绝对路径".into());
    }
    dsh_workspace_resources::checked_path(path)?;
    if path.is_dir()
        && !path.join(".dsh-resources").is_file()
        && std::fs::read_dir(path)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
    {
        return Err("请选择空目录或已有受管垃圾槽目录".into());
    }
    Ok(())
}

pub(crate) struct Resources {
    settings: Arc<SettingsProvider>,
    default_root: PathBuf,
    roots_file: PathBuf,
    stores: parking_lot::Mutex<BTreeMap<String, Arc<Store>>>,
    agents: Arc<dsh_agent::AgentRegistry>,
    size_jobs: Arc<parking_lot::Mutex<BTreeMap<String, (u64, bool)>>>,
    /// Runtime-token-bound acceptance receipts; never accepted as model arguments.
    validated_promotions: parking_lot::Mutex<BTreeMap<u64, (String, String, String)>>,
}
impl Resources {
    pub(crate) fn seal_promotion(
        &self,
        token: u64,
        owner: &str,
        arguments: &Value,
        sha256: String,
    ) {
        let mut receipts = self.validated_promotions.lock();
        if receipts.len() >= 256
            && let Some(oldest) = receipts.keys().next().copied()
        {
            receipts.remove(&oldest);
        }
        receipts.insert(
            token,
            (
                owner.into(),
                digest(arguments.to_string().as_bytes()),
                sha256,
            ),
        );
    }
    fn take_promotion(
        &self,
        token: u64,
        owner: &str,
        arguments: &Value,
    ) -> Result<Option<String>, String> {
        let Some((expected_owner, input, sha256)) = self.validated_promotions.lock().remove(&token)
        else {
            return Ok(None);
        };
        if expected_owner != owner || input != digest(arguments.to_string().as_bytes()) {
            return Err("验收凭据与当前执行身份不匹配".into());
        }
        Ok(Some(sha256))
    }
    pub fn policy(&self) -> Policy {
        self.settings
            .get(&settings_namespace("workspace-scratch").unwrap())
            .and_then(|value| value.to_json())
            .and_then(|value| Policy::from_json(value).ok())
            .unwrap_or_default()
    }
    pub fn current(&self) -> Result<Arc<Store>, String> {
        let policy = self.policy();
        let root = if policy.location.trim().is_empty() {
            self.default_root.clone()
        } else {
            PathBuf::from(policy.location)
        };
        self.open_store(root)
    }
    pub fn current_for(&self, project: &str) -> Result<Arc<Store>, String> {
        let project = std::fs::canonicalize(project).map_err(|e| e.to_string())?;
        let key = digest(project.to_string_lossy().as_bytes());
        let location = self
            .settings
            .get(&settings_namespace("workspace-scratch-paths")?)
            .and_then(|value| value.to_json())
            .and_then(|value| value["locations"][&key].as_str().map(str::to_owned));
        match location.filter(|value| !value.is_empty()) {
            Some(location) => self.open_store(PathBuf::from(location)),
            None => self.current(),
        }
    }
    pub async fn workspace_location(
        &self,
        path: &str,
        location: Option<&str>,
    ) -> Result<Value, String> {
        if !Path::new(path).is_absolute() {
            return Err("工作区必须使用绝对路径".into());
        }
        let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
        if !path.is_dir() {
            return Err("工作区必须是目录".into());
        }
        let key = digest(path.to_string_lossy().as_bytes());
        let ns = settings_namespace("workspace-scratch-paths")?;
        if let Some(location) = location {
            if !location.is_empty() {
                let root = PathBuf::from(location);
                validate_location(&root)?;
                self.open_store(root)?;
            }
            self.settings
                .update(&ns, json!({"locations":{key.clone():location}}), None)
                .await?;
        }
        let stored = self
            .settings
            .get(&ns)
            .and_then(|value| value.to_json())
            .and_then(|value| value["locations"][&key].as_str().map(str::to_owned))
            .unwrap_or_default();
        Ok(
            json!({"path":path,"location":stored,"effectiveLocation":self.current_for(&path.to_string_lossy())?.root()}),
        )
    }
    fn open_store(&self, root: PathBuf) -> Result<Arc<Store>, String> {
        if !root.is_absolute() {
            return Err("垃圾槽存储位置必须是绝对路径".into());
        }
        let key = std::fs::canonicalize(&root)
            .unwrap_or_else(|_| root.clone())
            .to_string_lossy()
            .into_owned();
        let mut stores = self.stores.lock();
        if let Some(store) = stores.get(&key) {
            return Ok(store.clone());
        }
        let store = Store::open(&root)?;
        let key = std::fs::canonicalize(store.root())
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned();
        if let Some(existing) = stores.get(&key) {
            return Ok(existing.clone());
        }
        let mut locations = stores.keys().cloned().collect::<Vec<_>>();
        locations.push(key.clone());
        persist_json(&self.roots_file, &locations)?;
        stores.insert(key, store.clone());
        Ok(store)
    }
    pub fn stores(&self) -> Result<Vec<Arc<Store>>, String> {
        let current = self.current();
        let stores = self.stores.lock().values().cloned().collect::<Vec<_>>();
        if stores.is_empty() {
            current?;
        }
        Ok(stores)
    }
    pub fn location_error(&self) -> Option<String> {
        self.current().err()
    }
    pub fn locate(&self, id: &str) -> Result<Arc<Store>, String> {
        let matches = self
            .stores()?
            .into_iter()
            .filter(|store| store.get(id).is_ok())
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err("资源不存在或标识不唯一".into());
        }
        Ok(matches[0].clone())
    }
    pub fn list(&self, owner: Option<&str>) -> Result<Vec<Resource>, String> {
        let mut rows = Vec::new();
        for store in self.stores()? {
            rows.extend(store.list_brief()?);
            let key = store.root().to_string_lossy().into_owned();
            let now = dsh_workspace_resources::now();
            let mut jobs = self.size_jobs.lock();
            if jobs
                .get(&key)
                .is_none_or(|(last, busy)| !busy && now.saturating_sub(*last) >= 60)
            {
                jobs.insert(key.clone(), (now, true));
                let jobs = self.size_jobs.clone();
                tokio::task::spawn_blocking(move || {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        store.refresh_sizes()
                    }));
                    jobs.lock()
                        .insert(key, (dsh_workspace_resources::now(), false));
                });
            }
        }
        if let Some(owner) = owner {
            let projects = rows
                .iter()
                .filter(|row| row.owner == owner)
                .map(|row| row.project.clone())
                .collect::<BTreeSet<_>>();
            rows.retain(|row| {
                row.owner == owner || row.owner == "shared" && projects.contains(&row.project)
            });
        }
        rows.retain(|row| row.state != "reclaimed");
        rows.sort_by_key(|row| std::cmp::Reverse(row.updated_at));
        Ok(rows)
    }
    pub fn collect(&self, manual: bool) -> Result<Vec<String>, String> {
        let policy = self.policy();
        if !manual && !policy.auto_clean {
            return Ok(Vec::new());
        }
        // Session scopes, background jobs and subagents keep their owner registered.
        let active = self
            .agents
            .list()
            .into_iter()
            .map(|agent| agent.id().as_str().to_string())
            .collect::<BTreeSet<_>>();
        let mut cleaned = Vec::new();
        for store in self.stores()? {
            cleaned.extend(store.collect_protected(
                &policy,
                dsh_workspace_resources::now(),
                manual,
                &active,
            )?);
        }
        Ok(cleaned)
    }
    pub fn assert_owner(&self, owner: &str, id: &str) -> Result<Arc<Store>, String> {
        let store = self.locate(id)?;
        if store.get(id)?.owner != owner {
            return Err("不能操作其他任务的资源".into());
        }
        Ok(store)
    }
    pub fn quarantine(&self, id: &str) -> Result<(), String> {
        let store = self.locate(id)?;
        let row = store.get(id)?;
        if row.kind == "cache"
            && self
                .list(None)?
                .iter()
                .any(|run| run.kind == "run" && run.project == row.project && run.busy)
        {
            return Err("缓存仍被运行中的任务使用".into());
        }
        if self
            .agents
            .get(&dsh_session::session_id(&row.owner))
            .is_some()
        {
            return Err("任务仍在运行，请结束运行后再清理".into());
        }
        store.quarantine(id)
    }
    pub fn install(
        ctx: &Context,
        settings: Arc<SettingsProvider>,
        data_root: &Path,
        subprocess: &Arc<dsh_subprocess_local::LocalSubprocessRuntime>,
        agents: Arc<dsh_agent::AgentRegistry>,
    ) -> Result<Arc<Self>, String> {
        settings.register(
            ctx,
            settings_namespace("workspace-scratch-paths")?,
            Schema::object(indexmap::IndexMap::from([(
                "locations".into(),
                Schema::dict(Schema::string(), None).default(Data::Object(Default::default())),
            )])),
            SettingsRegisterOptions {
                validate: Some(Arc::new(|value| {
                    let json = value.to_json().ok_or("工作区垃圾槽设置无效")?;
                    if let Some(locations) = json["locations"].as_object() {
                        for location in locations
                            .values()
                            .filter_map(Value::as_str)
                            .filter(|s| !s.is_empty())
                        {
                            validate_location(Path::new(location))?;
                        }
                    }
                    Ok(())
                })),
                ..Default::default()
            },
        )?;
        let mut fields = indexmap::IndexMap::new();
        for name in ["enabled", "autoClean", "reduceContext"] {
            fields.insert(name.into(), Schema::boolean().default(Data::Bool(true)));
        }
        for (name, value, max) in [
            ("keepDays", 7.0, 365.0),
            ("failedDays", 14.0, 365.0),
            ("recoveryDays", 3.0, 30.0),
            ("softLimitGib", 20.0, 4096.0),
        ] {
            fields.insert(
                name.into(),
                Schema::number()
                    .min(1.0)
                    .max(max)
                    .step(1.0)
                    .default(Data::Number(value)),
            );
        }
        fields.insert(
            "location".into(),
            Schema::string()
                .max(8192.0)
                .default(Data::String(String::new())),
        );
        settings.register(
            ctx,
            settings_namespace("workspace-scratch")?,
            Schema::object(fields),
            SettingsRegisterOptions {
                validate: Some(Arc::new(|value| {
                    let policy = Policy::from_json(value.to_json().ok_or("垃圾槽设置格式无效")?)?;
                    if !policy.location.is_empty() {
                        if !Path::new(&policy.location).is_absolute() {
                            return Err("存储位置必须为绝对路径".into());
                        }
                        let location = Path::new(&policy.location);
                        if location.is_dir()
                            && !location.join(".dsh-resources").is_file()
                            && std::fs::read_dir(location)
                                .map_err(|e| e.to_string())?
                                .next()
                                .is_some()
                        {
                            return Err("请选择新的空目录或已有受管垃圾槽目录".into());
                        }
                    }
                    Ok(())
                })),
                ..Default::default()
            },
        )?;
        let roots_file = data_root.join("resource-roots.json");
        let known: Vec<String> = match std::fs::read(&roots_file) {
            Ok(bytes) if bytes.len() <= 65536 => {
                serde_json::from_slice(&bytes).map_err(|e| format!("垃圾槽位置记录无效：{e}"))?
            }
            Ok(_) => return Err("垃圾槽位置记录过大".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.to_string()),
        };
        let manager = Arc::new(Self {
            settings,
            default_root: data_root.join("scratch"),
            roots_file,
            stores: parking_lot::Mutex::new(BTreeMap::new()),
            agents,
            size_jobs: Default::default(),
            validated_promotions: Default::default(),
        });
        for root in known {
            if Path::new(&root).join(".dsh-resources").is_file() {
                manager
                    .stores
                    .lock()
                    .insert(root.clone(), Store::open(&root)?);
            }
        }
        // Keep settings and recovery UI available when a storage volume is offline.
        // Execution still fails at the resource provider instead of writing into the project.
        let _ = manager.current();
        let managed: Arc<dyn dsh_workspace_resources::ManagedWorkspaces> = manager.clone();
        ctx.register_service(managed);
        let weak = Arc::downgrade(&manager);
        subprocess.set_resource_provider(Arc::new(move |cwd, env| {
            let Some(manager) = weak.upgrade() else {
                return Err("资源管理器已关闭".into());
            };
            if manager.policy().enabled {
                let project = env
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case("DSH_PROJECT_ROOT"))
                    .map(|(_, value)| value.as_str())
                    .unwrap_or(cwd);
                manager.current_for(project).map(Some)
            } else {
                Ok(None)
            }
        }));
        let weak = Arc::downgrade(&manager);
        let grant = dsh_sandbox::roots::register_managed_temp(Arc::new(move || {
            weak.upgrade()
                .and_then(|manager| manager.stores().ok())
                .unwrap_or_default()
                .into_iter()
                .map(|store| store.writable_root().to_string_lossy().into_owned())
                .collect()
        }));
        let weak = Arc::downgrade(&manager);
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(900));
            interval.tick().await;
            loop {
                interval.tick().await;
                let Some(manager) = weak.upgrade() else { break };
                let _ = tokio::task::spawn_blocking(move || manager.collect(false)).await;
            }
        });
        let grant = parking_lot::Mutex::new(Some(grant));
        let _ = ctx.effect(
            "managed resource collection",
            Box::pin(async move {
                Some(make_disposer(move || {
                    task.abort();
                    grant.lock().take();
                    Box::pin(async {})
                }))
            }),
        );
        Ok(manager)
    }
    pub fn install_tools(
        self: &Arc<Self>,
        ctx: &Context,
        tools: &Arc<dsh_tools::ToolRuntime>,
        prompt: &Arc<dsh_system_prompt::SystemPrompt>,
    ) -> Result<(), String> {
        let manager = self.clone();
        let _=prompt.section(ctx,dsh_system_prompt::PromptSection{name:"app:workspace-resources".into(),order:-97.0,text:dsh_system_prompt::PromptText::Provider(Arc::new(move |_|{
            if !manager.policy().enabled {return String::new()}
            "Keep formal source, regression tests, configuration and deliverables in the project. Use workspace_scratch to allocate and write one-off scripts, intermediate reports, screenshots and candidate deliverables outside the project. Shell and terminal TEMP/TMP and compatible Cargo target caches are managed automatically; explicit paths are preserved. Use the returned absolute script path to execute it. For commands that write in place, prepare_copy and pass its workdir to pwsh; this confines writes to the copy while keeping the source project read-only. Use inspect and promote to deliver individual validated files with target-version checks. Keep candidates protected until validated and delivered. Read full tool logs by resource ID with offset/limit only when needed. Do not copy whole execution trees back to the project or place writable hard links to project files. Never treat user input, permanent source or unregistered files as disposable scratch. On Windows, guard optional diagnostics with Get-Command before invoking them; a missing py launcher is not a Python failure. PowerShell keeps a native command's nonzero LASTEXITCODE through later semicolon-separated commands, so use an explicit guard or set allow_nonzero:true for a read-only probe when its exitCode should be reported as data.".into()
        })),complete:None});
        let manager = self.clone();
        tools.register(ctx,dsh_tools::ToolDefinition {
            name:"workspace_scratch".into(),description:"Manage task-owned scratch files outside the project. prepare_copy creates a Git worktree including local changes or a bounded input copy (optional files list); use its workdir for tools that write in place. allocate creates script/candidate/log storage; write(id,path,content), read(id,path,offset,limit), list, pin and release manage it. inspect(target relative to project) returns the current sha256 or null for a new target. promote(id,path,target,expectedSha256) delivers one verified file, rejecting target version conflicts. Candidates stay protected until release. read is never spilled again.".into(),
            parameters:scratch_parameters(),
            output:dsh_tools::ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,value|Ok(vec![dsh_llm::ContentBlock::Text{text:value.to_string()}])),presentation_meta:None},
            timeout_ms:Some(30000),is_concurrency_safe:None,finalize_content:None,present_call:None,present_result:None,
            execute:Arc::new(move |args,run| {
                let args=args.clone();let manager=manager.clone();let signal=run.signal.lock().clone();let token=run.token;let owner=run.agent.as_ref().map(|agent|(agent.id().as_str().to_string(),agent.session().header().cwd.clone().unwrap_or_default()));
                Box::pin(async move {let (owner,project)=owner.ok_or_else(||dsh_tools::ToolBodyError::plain("临时资源必须归属于任务"))?;
                    let validated_source=manager.take_promotion(token,&owner,&args).map_err(dsh_tools::ToolBodyError::plain)?;
                    tokio::task::spawn_blocking(move ||manager.tool_action(&owner,&project,&args,validated_source.as_deref(),signal)).await.map_err(|e|dsh_tools::ToolBodyError::plain(e.to_string()))?.map_err(dsh_tools::ToolBodyError::plain)
                })
            }),
        }).map(|_|())?;
        let erased: Arc<dyn dsh_spill::SpillStore> = self.clone();
        ctx.register_service(erased);
        let dispose = dsh_spill_policy::apply(
            ctx,
            dsh_spill_policy::Config {
                max_inline_bytes: Some(12000),
            },
        )?;
        let _ = ctx.effect(
            "managed tool output retention",
            Box::pin(async move { Some(dispose) }),
        );
        Ok(())
    }
    fn tool_action(
        &self,
        owner: &str,
        project: &str,
        args: &Value,
        validated_source: Option<&str>,
        signal: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Value, String> {
        if args
            .get("offset")
            .is_some_and(|value| value.as_u64().is_none())
            || args.get("limit").is_some_and(|value| {
                value
                    .as_u64()
                    .is_none_or(|limit| limit == 0 || limit > 32000)
            })
        {
            return Err("offset 必须为非负整数，limit 必须为 1–32000 的整数".into());
        }
        let string = |name: &str| {
            args.get(name)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("缺少 {name}"))
        };
        match string("action")? {
            "prepare_copy" => {
                if !self.policy().enabled {
                    return Err("垃圾槽已关闭".into());
                }
                super::workspace_copy::prepare(
                    &self.current_for(project)?,
                    owner,
                    project,
                    args.get("files"),
                    signal,
                )
            }
            "inspect" => super::workspace_copy::inspect(project, string("target")?),
            "promote" => {
                let id = string("id")?;
                let store = self.assert_owner(owner, id)?;
                super::workspace_copy::promote_validated(
                    &store,
                    id,
                    string("path")?,
                    project,
                    string("target")?,
                    args.get("expectedSha256")
                        .ok_or("交付前请 inspect 目标并提供 expectedSha256")?,
                    validated_source,
                    signal,
                )
            }
            "allocate" => {
                if !self.policy().enabled {
                    return Err("垃圾槽已关闭".into());
                }
                let store = self.current_for(project)?;
                let kind = args.get("kind").and_then(Value::as_str).unwrap_or("script");
                let mut lease = store.allocate(
                    owner,
                    project,
                    kind,
                    args.get("label")
                        .and_then(Value::as_str)
                        .unwrap_or("临时材料"),
                )?;
                let id = lease.id().to_string();
                let path = lease.path();
                lease.finish(true)?;
                store.retain(&id, false, true)?;
                Ok(json!({"id":id,"path":path,"protected":true}))
            }
            "list" => Ok(json!({"entries":self.list(Some(owner))?})),
            action => {
                let id = string("id")?;
                let store = self.assert_owner(owner, id)?;
                match action {
                    "write" => {
                        let path = store.write_text(id, string("path")?, string("content")?)?;
                        let saved = std::fs::read(&path).map_err(|error| error.to_string())?;
                        Ok(json!({"id":id,"path":path,"bytes":saved.len(),"sha256":digest(&saved)}))
                    }
                    "read" => Ok(
                        json!({"text":store.read_text(id,string("path")?,args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,args.get("limit").and_then(Value::as_u64).unwrap_or(12000) as usize)?}),
                    ),
                    "pin" => {
                        let row = store.get(id)?;
                        store.retain(
                            id,
                            args.get("pinned").and_then(Value::as_bool).unwrap_or(true),
                            row.state == "candidate",
                        )?;
                        Ok(json!({"id":id}))
                    }
                    "release" => {
                        store.retain(id, store.get(id)?.pinned, false)?;
                        Ok(json!({"id":id,"state":"retained"}))
                    }
                    _ => Err("未知垃圾槽操作".into()),
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl dsh_spill::SpillStore for Resources {
    fn enabled(&self) -> bool {
        self.policy().reduce_context
    }
    async fn save_text(
        &self,
        input: &dsh_spill::SaveTextSpill,
    ) -> Result<dsh_spill::SpillRef, String> {
        let owner = input.owner.session_id.as_str().to_string();
        let project = self
            .agents
            .get(&input.owner.session_id)
            .and_then(|agent| agent.session().header().cwd.clone());
        let store = match project {
            Some(project) => self.current_for(&project)?,
            None => self.current()?,
        };
        let text = input.content.clone();
        let bytes = text.len() as u64;
        let (label, file) = match &input.source {
            dsh_spill::SpillSource::SessionReference { .. } => {
                ("会话完整引用", "session-reference.json")
            }
            _ => ("工具完整输出", "output.txt"),
        };
        let id = tokio::task::spawn_blocking(move || {
            let mut lease = store.allocate(&owner, "", "log", label)?;
            let id = lease.id().to_string();
            store.write_text(&id, file, &text)?;
            lease.finish(true)?;
            Ok::<_, String>(id)
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok(dsh_spill::SpillRef {
            locator: dsh_spill::spill_locator(format!("scratch:{id}")),
            bytes,
            retrieval_hint: format!(
                "Use workspace_scratch action=read id={id} path={file} with offset and limit (characters)."
            ),
        })
    }
}

impl dsh_workspace_resources::ManagedWorkspaces for Resources {
    fn resolve(
        &self,
        owner: &str,
        workdir: &str,
    ) -> Result<Option<dsh_workspace_resources::ExecutionWorkspace>, String> {
        let path = std::fs::canonicalize(workdir).map_err(|e| e.to_string())?;
        for store in self.stores()? {
            let base = std::fs::canonicalize(store.writable_root()).map_err(|e| e.to_string())?;
            let Ok(relative) = path.strip_prefix(&base) else {
                continue;
            };
            let Some(id) = relative
                .components()
                .next()
                .and_then(|part| part.as_os_str().to_str())
            else {
                continue;
            };
            let Ok(row) = store.get(id) else { continue };
            if row.kind != "copy" || row.owner != owner {
                continue;
            }
            let root =
                std::fs::canonicalize(store.path(id, "worktree")?).map_err(|e| e.to_string())?;
            if !path.starts_with(&root) {
                continue;
            }
            let mut read_only_roots = vec![row.project.clone()];
            if let Some(common) = row
                .origin
                .as_ref()
                .and_then(|origin| origin.get("gitCommon"))
                .and_then(Value::as_str)
            {
                read_only_roots.push(common.into());
            }
            return Ok(Some(dsh_workspace_resources::ExecutionWorkspace {
                root: root.to_string_lossy().into_owned(),
                project: row.project,
                read_only_roots,
            }));
        }
        Ok(None)
    }
}
