//! Immutable project skill revisions activated explicitly through user controls.
//! Stored revision history is independent of task or session execution databases.
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
pub(crate) const PROVIDER: &str = "manual-skill-revisions";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRevision {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub content_hash: String,
    pub project: String,
    #[serde(default)]
    pub owner_session_id: String,
    #[serde(default)]
    pub source_evidence: Vec<String>,
    pub created_at: u64,
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
            invalidate: parking_lot::RwLock::new(None),
        }))
    }

    pub fn hash(content: &str) -> String {
        format!("{:x}", Sha256::digest(content.as_bytes()))
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
            "createdAt":r.created_at,"sourceEvidence":r.source_evidence,"activationMode":"manual",
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
        if !owner.is_empty() {
            checked_text(owner, 128, "owner session")?;
        }
        if source_evidence.len() > 32 {
            return Err("at most 32 source references are supported".into());
        }
        for reference in &source_evidence {
            checked_text(reference, 512, "source evidence")?;
        }
        let project = project_path(project)?;
        let mut state = self.state.lock().await;
        Self::expect(&state, expected)?;
        if !state.enabled {
            return Err("project skill revisions are disabled".into());
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
            withdrawn: false,
        };
        let mut next = state.clone();
        next.revisions.push(candidate.clone());
        self.commit(&mut state, next).await?;
        Ok(candidate)
    }

    /// Explicit user activation uses immutable content and project identity.
    pub async fn activate(&self, expected: u64, id: &str) -> Result<(), String> {
        let mut state = self.state.lock().await;
        Self::expect(&state, expected)?;
        if !state.enabled {
            return Err("project skill revisions are disabled".into());
        }
        let record = state
            .revisions
            .iter()
            .find(|r| r.id == id && !r.withdrawn)
            .cloned()
            .ok_or("skill candidate not found")?;
        Self::check_activation(&record)?;
        let mut next = state.clone();
        next.active.insert(active_key(&record), record.id);
        self.commit(&mut state, next).await
    }

    fn check_activation(record: &SkillRevision) -> Result<(), String> {
        if record.content_hash != Self::hash(&record.content)
            || project_path(&record.project)? != record.project
        {
            return Err("skill revision identity changed".into());
        }
        Ok(())
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

    /// Restore and activate a retained immutable revision by explicit user action.
    pub async fn restore(&self, expected: u64, id: &str) -> Result<(), String> {
        let mut state = self.state.lock().await;
        Self::expect(&state, expected)?;
        if !state.enabled {
            return Err("project skill revisions are disabled".into());
        }
        let record = state
            .revisions
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or("skill revision not found")?;
        Self::check_activation(&record)?;
        let mut next = state.clone();
        next.revisions
            .iter_mut()
            .find(|r| r.id == id)
            .unwrap()
            .withdrawn = false;
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

    async fn applicable(&self, record: &SkillRevision, options: &SkillLookupOptions) -> bool {
        if !self.state.lock().await.enabled
            || record.withdrawn
            || record.content_hash != Self::hash(&record.content)
            || options.signal.as_ref().is_some_and(|s| s())
        {
            return false;
        }
        options
            .cwd
            .as_ref()
            .and_then(|p| project_path(p).ok())
            .is_some_and(|cwd| cwd == record.project)
    }
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
            if !self.applicable(&record, options).await {
                continue;
            }
            candidates.push(SkillCandidate {
                name: record.name.clone(),
                description: record.description.clone(),
                when_to_use: None,
                invocation: SkillInvocationPolicy::BOTH,
                source: "手动项目技能".into(),
                provider: PROVIDER.into(),
                resource_base: None,
                rank: 350,
                locator: arc(record.id),
                path: None,
                metadata: Some(
                    serde_json::json!({"contentHash":record.content_hash,"project":record.project,"activationMode":"manual"}),
                ),
            });
        }
        // Project availability and explicit enablement can change independently of registration.
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
            || !self.applicable(&record, options).await
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
