//! Per-tool isolation of Host private files from model-controlled paths.
use crate::workspace_resources::Resources;
use dsh_fs::*;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) struct HostPrivateFileSystem {
    local: Arc<dyn FileSystem>,
    roots: Vec<PathBuf>,
    credentials: PathBuf,
    resources: Arc<Resources>,
    agents: Arc<dsh_agent::AgentRegistry>,
    attachments: Arc<dyn dsh_attachment::AttachmentStore>,
    // None is reserved for trusted service use. Some(None) is a tool without
    // an authenticated session, which receives no scratch exception.
    scope: Option<Option<String>>,
}

fn key(path: &str) -> String {
    let key = path.replace('\\', "/").trim_end_matches('/').to_owned();
    if cfg!(windows) {
        let key = key.to_lowercase();
        if let Some(tail) = key.strip_prefix("//?/unc/") {
            format!("//{tail}")
        } else {
            key.strip_prefix("//?/").unwrap_or(&key).to_owned()
        }
    } else {
        key
    }
}
fn under(path: &str, root: &str) -> bool {
    let path = key(path);
    let root = key(root);
    path == root
        || path.starts_with(&format!("{root}/"))
        || cfg!(windows) && path.starts_with(&format!("{root}:"))
}
fn denied() -> FsError {
    FsError::new(
        "Host private files are not available to tools. Use files admitted to this session or its owned scratch resources.",
        FsErrorCode::FsSandboxDenied,
    )
}

impl HostPrivateFileSystem {
    pub fn install(
        ctx: &cordis::Context,
        local: Arc<dyn FileSystem>,
        home: &Path,
        data: &Path,
        credentials: &Path,
        resources: Arc<Resources>,
    ) {
        let agents = ctx
            .get_typed::<Arc<dsh_agent::AgentRegistry>>("agents", false)
            .expect("Host agents installed")
            .as_ref()
            .clone();
        let attachments = ctx
            .get_typed::<Arc<dyn dsh_attachment::AttachmentStore>>("attachments", false)
            .expect("Host attachments installed")
            .as_ref()
            .clone();
        let fs: Arc<dyn FileSystem> = Arc::new(Self {
            local,
            roots: vec![home.into(), data.into()],
            credentials: credentials.into(),
            resources,
            scope: None,
            agents,
            attachments,
        });
        ctx.register_service(fs);
    }

    async fn canonical(&self, path: &Path) -> Result<String, FsError> {
        let target = self.local.resolve(&path.to_string_lossy(), None).await?;
        Ok(self.local.process_path(&target))
    }

    // Resolve the original path again immediately before I/O and delegate the
    // fresh identity, catching aliases retargeted since the initial resolve.
    async fn checked(
        &self,
        target: &FsTarget,
        recursive: bool,
        mutation: bool,
    ) -> Result<FsTarget, FsError> {
        if self.scope.is_none() {
            return Ok(target.clone());
        }
        let fresh = self.local.resolve(&target.display_path, None).await?;
        let path = self.local.process_path(&fresh);
        let credential = self.canonical(&self.credentials).await?;
        if under(&path, &credential) || recursive && under(&credential, &path) {
            return Err(denied());
        }
        let mut protected = Vec::new();
        for root in &self.roots {
            protected.push(self.canonical(root).await?);
        }
        for root in self.resources.private_roots() {
            protected.push(
                self.canonical(&root)
                    .await
                    .unwrap_or_else(|_| root.to_string_lossy().into_owned()),
            );
        }
        let private = protected.iter().any(|root| under(&path, root));
        if private {
            let mut owned = false;
            if let Some(Some(owner)) = &self.scope {
                // Unavailable scratch storage cannot authorize a path, but it
                // must not suppress independent immutable-attachment admission.
                let stores = self.resources.stores().unwrap_or_default();
                for store in &stores {
                    // Store verifies immutable resource identity and rejects
                    // linked resource directories before returning paths.
                    for row in store.list_brief().unwrap_or_default() {
                        if row.owner != *owner
                            || row.path.is_empty()
                            || row.state == "reclaimed"
                            || mutation && row.kind == "trash"
                        {
                            continue;
                        }
                        if dsh_workspace_resources::checked_path(Path::new(&row.path)).is_err() {
                            continue;
                        }
                        let root = self.canonical(Path::new(&row.path)).await?;
                        if under(&path, &root) {
                            owned = true;
                            break;
                        }
                    }
                    if owned {
                        break;
                    }
                }
            }
            if !owned
                && !mutation
                && let Some(Some(owner)) = &self.scope
            {
                if let Some(agent) = self.agents.get(&dsh_session::session_id(owner)) {
                    let mut paths = Vec::new();
                    agent
                        .session()
                        .visit_events(0, None, |event| {
                            for reference in
                                dsh_attachment::file_references_for_event(&event.type_, &event.data)
                            {
                                paths.extend(self.attachments.file_host_path(&reference));
                            }
                            for reference in dsh_attachment::image_references_for_event(
                                &event.type_,
                                &event.data,
                            ) {
                                paths.extend(self.attachments.image_host_path(&reference));
                            }
                            Ok(true)
                        })
                        .map_err(|_| denied())?;
                    for admitted in paths {
                        if key(&path) == key(&self.canonical(&admitted).await?) {
                            owned = true;
                            break;
                        }
                    }
                }
            }
            if !owned {
                return Err(denied());
            }
        }
        // ripgrep runs outside this provider. Reject an ancestor query before
        // it can collect private bytes, rather than filtering only its output.
        if recursive && protected.iter().any(|root| under(root, &path)) {
            return Err(denied());
        }
        Ok(fresh)
    }
}

#[async_trait::async_trait]
impl FileSystem for HostPrivateFileSystem {
    async fn write_bytes(
        &self,
        target: &FsTarget,
        content: &[u8],
        expected: Option<&FsWriteIntent>,
        signal: Option<AbortPredicate>,
        policy: Option<&dsh_sandbox::SandboxExecutionPolicy>,
    ) -> Result<FsBinaryWriteOutcome, FsError> {
        self.local
            .write_bytes(
                &self.checked(target, false, true).await?,
                content,
                expected,
                signal,
                policy,
            )
            .await
    }
    fn for_tool(&self, owner: Option<&str>) -> Option<Arc<dyn FileSystem>> {
        Some(Arc::new(Self {
            local: self.local.clone(),
            roots: self.roots.clone(),
            credentials: self.credentials.clone(),
            resources: self.resources.clone(),
            agents: self.agents.clone(),
            attachments: self.attachments.clone(),
            scope: Some(owner.map(str::to_owned)),
        }))
    }
    fn sandbox_mode(&self) -> Option<dsh_sandbox::SandboxMode> {
        self.local.sandbox_mode()
    }
    async fn authorize_search(&self, target: &FsTarget) -> Result<(), FsError> {
        self.checked(target, true, false).await.map(|_| ())
    }
    async fn authorize_write(&self, target: &FsTarget) -> Result<(), FsError> {
        self.checked(target, false, true).await.map(|_| ())
    }
    async fn resolve(
        &self,
        path: &str,
        opts: Option<&ResolveOptions>,
    ) -> Result<FsTarget, FsError> {
        let target = self.local.resolve(path, opts).await?;
        self.checked(&target, false, false).await
    }
    fn process_path(&self, target: &FsTarget) -> String {
        self.local.process_path(target)
    }
    fn file_url(&self, target: &FsTarget) -> String {
        self.local.file_url(target)
    }
    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool {
        under(
            &self.local.process_path(child),
            &self.local.process_path(parent),
        )
    }
    async fn stat(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<Option<FsInfo>, FsError> {
        self.local
            .stat(&self.checked(target, false, false).await?, signal)
            .await
    }
    async fn lstat(
        &self,
        path: &str,
        opts: Option<&LstatOptions>,
        signal: Option<AbortPredicate>,
    ) -> Result<Option<FsPathInfo>, FsError> {
        self.resolve(
            path,
            Some(&ResolveOptions {
                cwd: opts.and_then(|o| o.cwd.clone()),
                signal: signal.clone(),
            }),
        )
        .await?;
        self.local.lstat(path, opts, signal).await
    }
    async fn read_text(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<String, FsError> {
        self.local
            .read_text(&self.checked(target, false, false).await?, signal)
            .await
    }
    async fn stream_text(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<futures::stream::BoxStream<'static, Result<String, FsError>>, FsError> {
        self.local
            .stream_text(&self.checked(target, false, false).await?, signal)
            .await
    }
    async fn read_bytes(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
        max_bytes: u64,
    ) -> Result<Vec<u8>, FsError> {
        self.local
            .read_bytes(
                &self.checked(target, false, false).await?,
                signal,
                max_bytes,
            )
            .await
    }
    async fn stream_bytes(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
        max_bytes: u64,
    ) -> Result<futures::stream::BoxStream<'static, Result<Vec<u8>, FsError>>, FsError> {
        self.local
            .stream_bytes(
                &self.checked(target, false, false).await?,
                signal,
                max_bytes,
            )
            .await
    }
    async fn list_dir(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<Vec<FsDirEntry>, FsError> {
        let rows = self
            .local
            .list_dir(&self.checked(target, false, false).await?, signal)
            .await?;
        let mut allowed = Vec::new();
        for mut row in rows {
            match self.checked(&row.target, false, false).await {
                Ok(target) => {
                    row.target = target;
                    allowed.push(row);
                }
                Err(error) if error.code == FsErrorCode::FsSandboxDenied => {}
                Err(error) => return Err(error),
            }
        }
        Ok(allowed)
    }
    async fn write_text(
        &self,
        target: &FsTarget,
        content: &str,
        expected: Option<&FsWriteIntent>,
        signal: Option<AbortPredicate>,
        policy: Option<&dsh_sandbox::SandboxExecutionPolicy>,
    ) -> Result<FsWriteOutcome, FsError> {
        self.local
            .write_text(
                &self.checked(target, false, true).await?,
                content,
                expected,
                signal,
                policy,
            )
            .await
    }
    async fn edit_text(
        &self,
        target: &FsTarget,
        edit: &FsEditRequest,
        expected: Option<&FsEditGuard>,
        signal: Option<AbortPredicate>,
        policy: Option<&dsh_sandbox::SandboxExecutionPolicy>,
    ) -> Result<FsEditOutcome, FsError> {
        self.local
            .edit_text(
                &self.checked(target, false, true).await?,
                edit,
                expected,
                signal,
                policy,
            )
            .await
    }
}

#[cfg(test)]
mod tests;
