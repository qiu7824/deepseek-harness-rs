//! Versioned skill candidates. Validation facts come from a trusted task verifier,
//! never from caller-supplied success flags. Candidates remain outside discovery roots.
use async_trait::async_trait;
use cordis::{arc, downcast_arc};
use dsh_skill::{
    SkillCandidate, SkillDefinition, SkillInvocationPolicy, SkillLookupOptions, SkillProvider,
    SkillProviderObservation,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

const MAX_DOCUMENT: usize = 8 * 1024 * 1024;
const MAX_REVISIONS: usize = 128;
const PROVIDER: &str = "validated-skill-revisions";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SampleRef {
    pub task_id: String,
    pub revision: u64,
    pub expected_success: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationEvidence {
    pub subject_identity: String,
    pub environment_fingerprint: String,
    pub project: String,
    pub checker_version: String,
    pub evidence_refs: Vec<String>,
    pub positive_samples: usize,
    pub negative_samples: usize,
    pub all_matched: bool,
}

#[async_trait]
pub trait SkillEvidenceVerifier: Send + Sync {
    async fn verify(
        &self,
        owner: &str,
        subject: &str,
        samples: &[SampleRef],
    ) -> Result<ValidationEvidence, String>;
    async fn environment_fingerprint(
        &self,
        project: &str,
        session: Option<&str>,
    ) -> Result<String, String>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRevision {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub content_hash: String,
    pub project: String,
    pub owner_session_id: String,
    pub source_evidence: Vec<String>,
    pub created_at: u64,
    pub samples: Vec<SampleRef>,
    pub validation: Option<ValidationEvidence>,
    pub withdrawn: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    version: u32,
    revision: u64,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    revisions: Vec<SkillRevision>,
    active: BTreeMap<String, String>,
}

fn enabled_by_default() -> bool {
    true
}

pub struct SkillLifecycle {
    path: PathBuf,
    state: tokio::sync::Mutex<Document>,
    verifier: parking_lot::RwLock<Option<Arc<dyn SkillEvidenceVerifier>>>,
    invalidate: parking_lot::RwLock<Option<Arc<dyn Fn() + Send + Sync>>>,
}

fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn active_key(record: &SkillRevision) -> String {
    format!("{}:{}", SkillLifecycle::hash(&record.project), record.name)
}

fn checked_text(value: &str, limit: usize, label: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > limit || value.contains('\0') {
        return Err(format!("invalid {label}"));
    }
    Ok(())
}

fn project_path(path: &str) -> Result<String, String> {
    let path = Path::new(path);
    if !path.is_absolute() || !path.is_dir() {
        return Err("skill scope must be an existing absolute project directory".into());
    }
    dsh_workspace_resources::checked_path(path)?;
    std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| e.to_string())
}

impl SkillLifecycle {
    pub async fn open(root: &Path) -> Result<Arc<Self>, String> {
        let path = root.join("skill-revisions-v1.json");
        dsh_workspace_resources::checked_path(&path)?;
        let state = match tokio::fs::metadata(&path).await {
            Ok(meta) if meta.len() <= MAX_DOCUMENT as u64 => {
                let value: Document = serde_json::from_slice(
                    &tokio::fs::read(&path).await.map_err(|e| e.to_string())?,
                )
                .map_err(|e| format!("skill revision data: {e}"))?;
                if value.version != 1 || value.revisions.len() > MAX_REVISIONS {
                    return Err("unsupported skill revision data".into());
                }
                for revision in &value.revisions {
                    if revision.content_hash != Self::hash(&revision.content)
                        || !dsh_skill::is_skill_name(&revision.name)
                    {
                        return Err("skill revision integrity check failed".into());
                    }
                }
                value
            }
            Ok(_) => return Err("skill revision data exceeds 8 MiB".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Document {
                version: 1,
                enabled: true,
                ..Default::default()
            },
            Err(e) => return Err(e.to_string()),
        };
        Ok(Arc::new(Self {
            path,
            state: tokio::sync::Mutex::new(state),
            verifier: parking_lot::RwLock::new(None),
            invalidate: parking_lot::RwLock::new(None),
        }))
    }

    pub fn hash(content: &str) -> String {
        format!("{:x}", Sha256::digest(content.as_bytes()))
    }
    pub fn set_verifier(&self, verifier: Arc<dyn SkillEvidenceVerifier>) {
        *self.verifier.write() = Some(verifier);
        self.changed();
    }
    pub fn set_invalidator(&self, invalidate: Arc<dyn Fn() + Send + Sync>) {
        *self.invalidate.write() = Some(invalidate);
    }
    fn changed(&self) {
        if let Some(callback) = self.invalidate.read().as_ref() {
            callback();
        }
    }

    async fn commit(&self, state: &mut Document, mut next: Document) -> Result<(), String> {
        next.revision = state
            .revision
            .checked_add(1)
            .ok_or("skill revision overflow")?;
        let bytes = serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_DOCUMENT {
            return Err(
                "skill revision storage budget reached; withdraw and remove unused candidates"
                    .into(),
            );
        }
        dsh_workspace_resources::checked_path(&self.path)?;
        dsh_atomic_write::write_file_atomic(
            &self.path,
            &bytes,
            dsh_atomic_write::WriteFileAtomicOptions {
                mode: 0o600,
                dir_mode: Some(0o700),
            },
        )
        .await
        .map_err(|e| e.to_string())?;
        *state = next;
        self.changed();
        Ok(())
    }

    fn expect(state: &Document, revision: u64) -> Result<(), String> {
        if state.revision != revision {
            Err("skill revisions changed; reload before updating".into())
        } else {
            Ok(())
        }
    }

    pub async fn list(&self) -> serde_json::Value {
        let state = self.state.lock().await;
        serde_json::json!({"revision":state.revision,"enabled":state.enabled,"active":state.active,"candidates":state.revisions.iter().map(|r| serde_json::json!({
            "id":r.id,"name":r.name,"description":r.description,"contentHash":r.content_hash,"project":r.project,
            "createdAt":r.created_at,"sourceEvidence":r.source_evidence,"samples":r.samples,"validation":r.validation,
            "withdrawn":r.withdrawn,"active":state.active.get(&active_key(r))==Some(&r.id)
        })).collect::<Vec<_>>()})
    }

    pub async fn get(&self, id: &str) -> Result<SkillRevision, String> {
        self.state
            .lock()
            .await
            .revisions
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| "skill revision not found".into())
    }

    pub async fn list_for_owner(&self, owner: &str) -> serde_json::Value {
        let state = self.state.lock().await;
        serde_json::json!({"revision":state.revision,"enabled":state.enabled,"candidates":state.revisions.iter().filter(|r|r.owner_session_id==owner).map(|r|serde_json::json!({
            "id":r.id,"name":r.name,"contentHash":r.content_hash,"project":r.project,"withdrawn":r.withdrawn,
            "validated":r.validation.is_some(),"active":state.active.get(&active_key(r))==Some(&r.id)
        })).collect::<Vec<_>>()})
    }

    pub async fn create(
        &self,
        expected: u64,
        name: &str,
        description: &str,
        content: &str,
        project: &str,
        owner: &str,
        source_evidence: Vec<String>,
    ) -> Result<SkillRevision, String> {
        if !dsh_skill::is_skill_name(name) || name.len() > 80 {
            return Err("invalid skill name".into());
        }
        checked_text(description, 1024, "description")?;
        checked_text(content, 256 * 1024, "content")?;
        checked_text(owner, 128, "owner session")?;
        if source_evidence.is_empty() || source_evidence.len() > 32 {
            return Err("source evidence is required (1-32 references)".into());
        }
        for reference in &source_evidence {
            checked_text(reference, 512, "source evidence")?;
        }
        let project = project_path(project)?;
        let mut state = self.state.lock().await;
        Self::expect(&state, expected)?;
        if !state.enabled {
            return Err("skill candidate improvement is disabled".into());
        }
        if state.revisions.len() >= MAX_REVISIONS {
            return Err("skill revision count limit reached".into());
        }
        let content_hash = Self::hash(content);
        if state.revisions.iter().any(|r| {
            !r.withdrawn && r.name == name && r.project == project && r.content_hash == content_hash
        }) {
            return Err("identical skill candidate already exists".into());
        }
        let candidate = SkillRevision {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            description: description.into(),
            content: content.into(),
            content_hash,
            project,
            owner_session_id: owner.into(),
            source_evidence,
            created_at: timestamp(),
            samples: vec![],
            validation: None,
            withdrawn: false,
        };
        let mut next = state.clone();
        next.revisions.push(candidate.clone());
        self.commit(&mut state, next).await?;
        Ok(candidate)
    }

    async fn validate_record(
        &self,
        record: &SkillRevision,
        samples: &[SampleRef],
    ) -> Result<ValidationEvidence, String> {
        if samples.len() < 2
            || samples.len() > 32
            || !samples.iter().any(|s| s.expected_success)
            || !samples.iter().any(|s| !s.expected_success)
        {
            return Err("validation requires bounded positive and negative samples".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for sample in samples {
            checked_text(&sample.task_id, 128, "sample task")?;
            if !ids.insert(&sample.task_id) {
                return Err("duplicate validation sample".into());
            }
        }
        let verifier = self
            .verifier
            .read()
            .clone()
            .ok_or("trusted skill validation service unavailable")?;
        let evidence = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            verifier.verify(&record.owner_session_id, &record.content_hash, samples),
        )
        .await
        .map_err(|_| "skill validation exceeded its ten-second budget")??;
        let fingerprint = verifier
            .environment_fingerprint(&record.project, Some(&record.owner_session_id))
            .await?;
        if !evidence.all_matched
            || evidence.positive_samples == 0
            || evidence.negative_samples == 0
            || evidence.subject_identity != record.content_hash
            || evidence.project != record.project
            || evidence.environment_fingerprint.is_empty()
            || evidence.environment_fingerprint != fingerprint
            || evidence.evidence_refs.is_empty()
            || evidence.checker_version.is_empty()
        {
            return Err(
                "skill samples are stale, unrelated, or did not match their expected outcomes"
                    .into(),
            );
        }
        Ok(evidence)
    }

    pub async fn validate(
        &self,
        expected: u64,
        id: &str,
        samples: Vec<SampleRef>,
    ) -> Result<ValidationEvidence, String> {
        let record = {
            let state = self.state.lock().await;
            Self::expect(&state, expected)?;
            state
                .revisions
                .iter()
                .find(|r| r.id == id && !r.withdrawn)
                .cloned()
                .ok_or("skill candidate not found")?
        };
        let evidence = self.validate_record(&record, &samples).await?;
        let mut state = self.state.lock().await;
        Self::expect(&state, expected)?;
        let mut next = state.clone();
        let candidate = next
            .revisions
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or("skill candidate not found")?;
        candidate.samples = samples;
        candidate.validation = Some(evidence.clone());
        self.commit(&mut state, next).await?;
        Ok(evidence)
    }

    /// UI-only explicit activation. Callers cannot replace validation with a bool.
    /// Restoring an older revision uses this same fresh-validation gate.
    pub async fn activate(&self, expected: u64, id: &str) -> Result<(), String> {
        let record = {
            let state = self.state.lock().await;
            Self::expect(&state, expected)?;
            state
                .revisions
                .iter()
                .find(|r| r.id == id && !r.withdrawn && r.validation.is_some())
                .cloned()
                .ok_or("a validated skill candidate is required")?
        };
        let evidence = self.validate_record(&record, &record.samples).await?;
        let mut state = self.state.lock().await;
        Self::expect(&state, expected)?;
        if !state.enabled {
            return Err("skill candidate improvement is disabled".into());
        }
        let mut next = state.clone();
        next.active.insert(active_key(&record), record.id.clone());
        next.revisions
            .iter_mut()
            .find(|r| r.id == id)
            .unwrap()
            .validation = Some(evidence);
        self.commit(&mut state, next).await
    }

    pub async fn withdraw(&self, expected: u64, id: &str) -> Result<(), String> {
        let mut state = self.state.lock().await;
        Self::expect(&state, expected)?;
        let mut next = state.clone();
        let candidate = next
            .revisions
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or("skill candidate not found")?;
        candidate.withdrawn = true;
        next.active.retain(|_, active| active != id);
        self.commit(&mut state, next).await
    }

    /// Explicit user restore preserves the immutable content and requires fresh
    /// evidence. Merely withdrawing a revision does not destroy its history.
    pub async fn restore(&self, expected: u64, id: &str) -> Result<(), String> {
        let record = {
            let state = self.state.lock().await;
            Self::expect(&state, expected)?;
            if !state.enabled {
                return Err("skill candidate improvement is disabled".into());
            }
            state
                .revisions
                .iter()
                .find(|r| r.id == id && r.validation.is_some())
                .cloned()
                .ok_or("a previously validated skill revision is required")?
        };
        let evidence = self.validate_record(&record, &record.samples).await?;
        let mut state = self.state.lock().await;
        Self::expect(&state, expected)?;
        let mut next = state.clone();
        let restored = next
            .revisions
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or("skill revision not found")?;
        restored.withdrawn = false;
        restored.validation = Some(evidence);
        next.active.insert(active_key(&record), record.id);
        self.commit(&mut state, next).await
    }

    pub async fn remove(&self, expected: u64, id: &str) -> Result<(), String> {
        let mut state = self.state.lock().await;
        Self::expect(&state, expected)?;
        if state.active.values().any(|active| active == id) {
            return Err("withdraw active skill revision before removing it".into());
        }
        let mut next = state.clone();
        let count = next.revisions.len();
        next.revisions.retain(|r| r.id != id);
        if count == next.revisions.len() {
            return Err("skill candidate not found".into());
        }
        self.commit(&mut state, next).await
    }

    pub async fn set_enabled(&self, expected: u64, enabled: bool) -> Result<(), String> {
        let mut state = self.state.lock().await;
        Self::expect(&state, expected)?;
        let mut next = state.clone();
        next.enabled = enabled;
        self.commit(&mut state, next).await
    }

    async fn applicable(
        &self,
        record: &SkillRevision,
        options: &SkillLookupOptions,
        verify_evidence: bool,
    ) -> bool {
        if !self.state.lock().await.enabled
            || record.withdrawn
            || options.signal.as_ref().is_some_and(|s| s())
        {
            return false;
        }
        let Some(cwd) = options.cwd.as_ref().and_then(|p| project_path(p).ok()) else {
            return false;
        };
        if cwd != record.project {
            return false;
        }
        let Some(session) = options.session_id.as_deref() else {
            return false;
        };
        let Some(verifier) = self.verifier.read().clone() else {
            return false;
        };
        let Ok(current) = verifier
            .environment_fingerprint(&record.project, Some(session))
            .await
        else {
            return false;
        };
        if record
            .validation
            .as_ref()
            .is_none_or(|v| v.environment_fingerprint != current)
        {
            return false;
        }
        // Validation at lookup/get avoids stale registry caches granting applicability.
        !verify_evidence || self.validate_record(record, &record.samples).await.is_ok()
    }
}

/// Model-facing candidate generation is separate from user activation controls.
pub fn install_candidate_tool(
    ctx: &cordis::Context,
    store: Arc<SkillLifecycle>,
) -> Result<(), String> {
    use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .map(|s| s.as_ref().clone())
        .ok_or("skill candidates require tools")?;
    tools.register(ctx,ToolDefinition{
        name:"skill_candidate".into(),
        description:"Create or inspect a task-owned reusable skill candidate from observed evidence. This does not activate instructions or grant permissions. The project and owner are bound to the current session. Use immutable task acceptance evidence with this contentHash for later positive/negative validation; activation is a user control.".into(),
        parameters:serde_json::json!({"type":"object","additionalProperties":false,"properties":{
            "action":{"type":"string","enum":["list","read","create"]},"id":{"type":"string"},"expectedRevision":{"type":"integer","minimum":0},
            "name":{"type":"string"},"description":{"type":"string"},"content":{"type":"string"},"sourceEvidence":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":32}
        },"required":["action"]}),
        output:ToolOutputDefinition{schema:serde_json::json!({"type":"object"}),render:Arc::new(|_,v|Ok(vec![dsh_llm::ContentBlock::Text{text:serde_json::to_string(v).unwrap_or_default()}])),presentation_meta:None},
        timeout_ms:None,is_concurrency_safe:None,execute:Arc::new(move|args,run|{
            let args=args.clone();let store=store.clone();let owner=run.execution.agent.clone();
            Box::pin(async move{
                let owner=owner.ok_or_else(||ToolBodyError::plain("skill candidate requires an initiating session"))?;
                let owner_id=owner.id().as_str();
                let required=|key:&str|args[key].as_str().filter(|s|!s.is_empty()).ok_or_else(||ToolBodyError::plain(format!("missing {key}")));
                match required("action")?{
                    "list"=>Ok(store.list_for_owner(owner_id).await),
                    "read"=>{let item=store.get(required("id")?).await.map_err(ToolBodyError::plain)?;if item.owner_session_id!=owner_id{return Err(ToolBodyError::plain("skill candidate not found"));}serde_json::to_value(item).map_err(|e|ToolBodyError::plain(e.to_string()))},
                    "create"=>{
                        let project=owner.session().header().cwd.clone().ok_or_else(||ToolBodyError::plain("skill candidate requires a project workspace"))?;
                        let expected=args["expectedRevision"].as_u64().ok_or_else(||ToolBodyError::plain("expectedRevision is required"))?;
                        let source=serde_json::from_value(args["sourceEvidence"].clone()).map_err(|e|ToolBodyError::plain(e.to_string()))?;
                        let item=store.create(expected,required("name")?,required("description")?,required("content")?,&project,owner_id,source).await.map_err(ToolBodyError::plain)?;
                        Ok(serde_json::json!({"candidate":item,"state":store.list_for_owner(owner_id).await}))
                    },
                    _=>Err(ToolBodyError::plain("unsupported skill candidate action"))
                }
            })
        }),finalize_content:None,present_call:None,present_result:None,
    })?;
    Ok(())
}

#[async_trait]
impl SkillProvider for SkillLifecycle {
    fn name(&self) -> &str {
        PROVIDER
    }
    async fn list(&self, options: &SkillLookupOptions) -> Result<SkillProviderObservation, String> {
        let records = {
            let state = self.state.lock().await;
            state
                .revisions
                .iter()
                .filter(|r| state.active.get(&active_key(r)) == Some(&r.id))
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut candidates = vec![];
        for record in records {
            if !self.applicable(&record, options, false).await {
                continue;
            }
            candidates.push(SkillCandidate {
                name: record.name.clone(),
                description: record.description.clone(),
                when_to_use: None,
                invocation: SkillInvocationPolicy::BOTH,
                source: "已验证项目技能".into(),
                provider: PROVIDER.into(),
                resource_base: None,
                rank: 350,
                locator: arc(record.id),
                path: None,
                metadata: Some(
                    serde_json::json!({"contentHash":record.content_hash,"project":record.project}),
                ),
            });
        }
        // Environment and verification state can change independently of registration.
        Ok(SkillProviderObservation {
            candidates,
            complete: true,
            volatile: true,
        })
    }
    async fn get(
        &self,
        candidate: &SkillCandidate,
        options: &SkillLookupOptions,
    ) -> Result<Option<SkillDefinition>, String> {
        let id =
            downcast_arc::<String>(&candidate.locator).ok_or("invalid skill revision locator")?;
        let record = self.get(id.as_str()).await?;
        if self.state.lock().await.active.get(&active_key(&record)) != Some(&record.id)
            || !self.applicable(&record, options, true).await
        {
            return Ok(None);
        }
        Ok(Some(SkillDefinition {
            name: record.name,
            description: record.description,
            when_to_use: None,
            invocation: SkillInvocationPolicy::BOTH,
            source: candidate.source.clone(),
            provider: PROVIDER.into(),
            resource_base: None,
            content: record.content,
            path: None,
            metadata: candidate.metadata.clone(),
        }))
    }
}

#[cfg(test)]
#[path = "skill_lifecycle_tests.rs"]
mod tests;
