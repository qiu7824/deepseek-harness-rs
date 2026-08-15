//! Local filesystem skill provider.
//!
//! This package is one implementation of the `ctx.skills` provider
//! registry. It discovers directory-bundle and flat Markdown skills from
//! project, custom, and user roots, parses YAML frontmatter, and loads
//! bodies through `ctx.fs` when a filesystem service is present.
//! Rust port of `packages/skill/skill-filesystem/src/index.ts`.
//!
//! # Deviations
//!
//! - The watcher is a notify-based recursive watch with a stability
//!   debounce; the TS chokidar ancestor-watch mode (missing roots) and
//!   polling mode are not ported — missing roots are picked up on the next
//!   discovery instead.
//! - The `fs/observed` mutation hook is not wired: the Rust
//!   [`dsh_fs::FsObservationActorHandle`] carries no tool name (the TS
//!   actor's `edit`/`write` names are unrepresentable).
//! - Error messages carry the port's error strings (the TS
//!   `[unrenderable thrown value]` coercion collapse).

pub mod invariant;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cordis::{ArcValue, Context, Disposer, InjectSpec, Plugin, PluginError, arc};
use dsh_fs::{FileSystem, FsDirEntry, FsErrorCode, FsInfoType};
use dsh_skill::{
    BUNDLED_SKILL_RANK, SkillAbort, SkillCandidate, SkillDefinition, SkillInvocationPolicy,
    SkillLookupOptions, SkillProvider, SkillProviderObservation, SkillResourceBase, is_skill_name,
};

pub const NAME: &str = "skill-filesystem";

const PROJECT_DSH_RANK: i64 = 100;
const PROJECT_AGENTS_RANK: i64 = 200;
const CUSTOM_RANK: i64 = 300;
const USER_DSH_RANK: i64 = 400;
const USER_AGENTS_RANK: i64 = 500;
const DEFAULT_WATCH_STABILITY_THRESHOLD_MS: u64 = 200;
const DEFAULT_WATCH_POLL_INTERVAL_MS: u64 = 100;

/// Local filesystem skill provider configuration.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Unique provider name. Defaults to `filesystem`.
    pub provider_name: Option<String>,
    /// Whether project and user roots are included around custom roots.
    pub include_default_roots: Option<bool>,
    /// DeepSeek Harness config root. Defaults to `$DSH_HOME` or `~/.dsh`.
    pub dsh_home: Option<String>,
    /// Shared agent config root. Defaults to `$DSH_AGENTS_HOME` or
    /// `~/.agents`.
    pub agents_home: Option<String>,
    /// Additional skill roots scanned after project roots and before user
    /// roots.
    pub custom_skill_dirs: Option<Vec<String>>,
    /// Whether host-local skill roots are watched for catalog changes.
    pub watch: Option<bool>,
    /// Whether the watcher uses polling instead of native filesystem
    /// events (not ported — accepted, ignored).
    pub watch_use_polling: Option<bool>,
    /// Milliseconds a changed skill entry must remain stable before it is
    /// observed.
    pub watch_stability_threshold_ms: Option<u64>,
    /// Milliseconds between stability probes.
    pub watch_poll_interval_ms: Option<u64>,
    /// Maximum distinct project roots whose skill directories remain
    /// watched.
    pub watch_max_projects: Option<usize>,
    /// Whether watched symbolic links follow their target files (not
    /// ported — accepted, ignored).
    pub watch_follow_symlinks: Option<bool>,
    /// Bundled skill root; defaults to `$DSH_BUNDLED_SKILL_DIR` when
    /// default roots are included, otherwise mounts none.
    pub bundled_skill_dir: Option<String>,
}

#[derive(Clone)]
pub struct SkillRoot {
    pub path: String,
    pub source: String,
    pub rank: i64,
    pub skip_system: bool,
    /// The project root owning project-* roots (the TS watcher groups
    /// owners by it; the port's watcher subset reads it only for fidelity).
    #[allow(dead_code)]
    pub project_root: Option<String>,
    pub trusted_host: bool,
}

#[derive(Clone)]
struct SkillRootEntry {
    name: String,
    kind: EntryKind,
    path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Directory,
    File,
    Other,
}

pub struct ParsedSkill {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub invocation: SkillInvocationPolicy,
    pub metadata: Option<serde_json::Value>,
    pub content: String,
}

#[derive(Clone)]
struct LocalLocator {
    path: String,
    directory: String,
}

/// A minimal debounced watcher: one recursive watcher per existing root;
/// any event invalidates the catalog after the stability threshold.
struct SkillWatcher {
    invalidate: Arc<dyn Fn() + Send + Sync>,
    threshold: std::time::Duration,
    watchers: parking_lot::Mutex<HashMap<String, notify::RecommendedWatcher>>,
    pending: Arc<parking_lot::Mutex<HashMap<String, Arc<tokio::sync::Notify>>>>,
    tasks: parking_lot::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    closing: std::sync::atomic::AtomicBool,
}

impl SkillWatcher {
    fn new(invalidate: Arc<dyn Fn() + Send + Sync>, threshold: std::time::Duration) -> Self {
        Self {
            invalidate,
            threshold,
            watchers: parking_lot::Mutex::new(HashMap::new()),
            pending: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            tasks: parking_lot::Mutex::new(Vec::new()),
            closing: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Watch one existing root; a missing root is skipped (deviation: the
    /// TS ancestor-watch re-discovers it).
    fn observe(&self, root: &str) {
        if self.closing.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        {
            let watchers = self.watchers.lock();
            if watchers.contains_key(root) {
                return;
            }
        }
        use notify::Watcher;
        let pending_for_task = self.pending.clone();
        let pending_for_callback = self.pending.clone();
        let root_owned = root.to_string();
        let mut watcher = match notify::RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| {
                if result.is_ok() {
                    if let Some(notify) = pending_for_callback.lock().get_mut(&root_owned) {
                        notify.notify_waiters();
                    }
                }
            },
            notify::Config::default(),
        ) {
            Ok(watcher) => watcher,
            Err(_) => return,
        };
        if watcher
            .watch(Path::new(root), notify::RecursiveMode::Recursive)
            .is_err()
        {
            return;
        }
        // Debounce: an event arms a timer; a later event within the
        // threshold restarts it; expiry invalidates.
        let root_for_task = root.to_string();
        let invalidate_for_task = self.invalidate.clone();
        let threshold = self.threshold;
        let task = tokio::spawn(async move {
            let pending = pending_for_task;
            loop {
                let notify = {
                    let mut guard = pending.lock();
                    guard
                        .entry(root_for_task.clone())
                        .or_insert_with(|| Arc::new(tokio::sync::Notify::new()))
                        .clone()
                };
                notify.notified().await;
                tokio::time::sleep(threshold).await;
                invalidate_for_task();
            }
        });
        self.tasks.lock().push(task);
        self.watchers.lock().insert(root.to_string(), watcher);
    }

    async fn dispose(&self) {
        self.closing.store(true, std::sync::atomic::Ordering::SeqCst);
        let watchers = {
            let mut guard = self.watchers.lock();
            std::mem::take(&mut *guard)
        };
        drop(watchers);
        let tasks = {
            let mut guard = self.tasks.lock();
            std::mem::take(&mut *guard)
        };
        for task in tasks {
            task.abort();
        }
        // Let aborted tasks unwind.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

/// Register the local filesystem skill provider on `ctx.skills` (TS
/// `apply`).
pub async fn apply(ctx: &Context, config: Config) -> Result<Disposer, String> {
    let skills = ctx
        .get_typed::<Arc<dsh_skill::SkillRegistry>>("skills", false)
        .map(|slot| slot.as_ref().clone())
        .ok_or_else(|| "skill-filesystem requires the skills service".to_string())?;
    let provider_cell: Arc<parking_lot::Mutex<Option<Arc<FileSystemSkillProvider>>>> =
        Arc::new(parking_lot::Mutex::new(None));
    let provider_for_cell = provider_cell.clone();
    let ctx_for_provider = ctx.clone();
    let config_for_provider = config.clone();
    let disposer = skills.register_provider(
        ctx,
        Arc::new(move |control| {
            let provider = Arc::new(FileSystemSkillProvider::new(
                &ctx_for_provider,
                control.invalidate.clone(),
                &config_for_provider,
            ));
            *provider_for_cell.lock() = Some(provider.clone());
            provider
        }),
    );
    let provider_for_teardown = provider_cell;
    Ok(cordis::make_disposer(move || {
        let disposer = disposer.clone();
        let provider = provider_for_teardown.clone();
        Box::pin(async move {
            (disposer)().await;
            let provider = provider.lock().take();
            if let Some(provider) = provider {
                provider.dispose().await;
            }
        })
    }))
}

/// Provider that maps local project/user skill roots into `ctx.skills`.
pub struct FileSystemSkillProvider {
    name: String,
    ctx: Context,
    include_default_roots: bool,
    dsh_home: String,
    agents_home: String,
    custom_skill_dirs: Vec<String>,
    bundled_skill_dir: Option<String>,
    invalidate: Arc<dyn Fn() + Send + Sync>,
    watch_enabled: bool,
    watcher: Arc<SkillWatcher>,
    disposal: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl FileSystemSkillProvider {
    fn new(
        ctx: &Context,
        invalidate: Arc<dyn Fn() + Send + Sync>,
        config: &Config,
    ) -> Self {
        let name = config
            .provider_name
            .clone()
            .unwrap_or_else(|| "filesystem".to_string());
        let include_default_roots = config.include_default_roots.unwrap_or(true);
        let env = |name: &str| std::env::var(name).ok();
        let dsh_home = match &config.dsh_home {
            Some(home) => absolute(home),
            None => dsh_home_paths::resolve_dsh_home(None, &env)
                .to_string_lossy()
                .into_owned(),
        };
        let agents_home = match &config.agents_home {
            Some(home) => absolute(home),
            None => std::env::var("DSH_AGENTS_HOME")
                .map(|home| absolute(&home))
                .unwrap_or_else(|_| {
                    dirs::home_dir()
                        .unwrap_or_else(|| PathBuf::from("."))
                        .join(".agents")
                        .to_string_lossy()
                        .into_owned()
                }),
        };
        let custom_skill_dirs = config
            .custom_skill_dirs
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|root| absolute(&root))
            .collect();
        let bundled_skill_dir = match &config.bundled_skill_dir {
            Some(dir) => Some(absolute(dir)),
            None => {
                if include_default_roots {
                    std::env::var("DSH_BUNDLED_SKILL_DIR").ok().map(|dir| absolute(&dir))
                } else {
                    None
                }
            }
        };
        let watch_enabled = config.watch.unwrap_or(true);
        let threshold_ms = config
            .watch_stability_threshold_ms
            .unwrap_or(DEFAULT_WATCH_STABILITY_THRESHOLD_MS);
        let _ = config.watch_poll_interval_ms.unwrap_or(DEFAULT_WATCH_POLL_INTERVAL_MS);
        let (disposal_tx, _disposal_rx) = tokio::sync::oneshot::channel();
        let watcher = Arc::new(SkillWatcher::new(
            invalidate.clone(),
            std::time::Duration::from_millis(threshold_ms.max(1)),
        ));
        Self {
            name,
            ctx: ctx.clone(),
            include_default_roots,
            dsh_home,
            agents_home,
            custom_skill_dirs,
            bundled_skill_dir,
            invalidate,
            watch_enabled,
            watcher,
            disposal: parking_lot::Mutex::new(Some(disposal_tx)),
        }
    }

    async fn roots(&self, cwd: Option<&str>) -> Vec<SkillRoot> {
        let mut roots: Vec<SkillRoot> = Vec::new();
        if self.include_default_roots {
            if let Some(cwd) = cwd {
                let project_root = find_project_root(
                    &absolute(cwd),
                    self.optional_fs(),
                )
                .await;
                roots.push(SkillRoot {
                    path: join_path(&project_root, ".dsh/skills"),
                    source: "project-dsh".to_string(),
                    rank: PROJECT_DSH_RANK,
                    skip_system: false,
                    project_root: Some(project_root.clone()),
                    trusted_host: false,
                });
                roots.push(SkillRoot {
                    path: join_path(&project_root, ".agents/skills"),
                    source: "project-agents".to_string(),
                    rank: PROJECT_AGENTS_RANK,
                    skip_system: false,
                    project_root: Some(project_root),
                    trusted_host: false,
                });
            }
        }
        roots.extend(self.custom_skill_dirs.iter().map(|path| SkillRoot {
            path: path.clone(),
            source: "custom".to_string(),
            rank: CUSTOM_RANK,
            skip_system: false,
            project_root: None,
            trusted_host: false,
        }));
        if self.include_default_roots {
            roots.push(SkillRoot {
                path: join_path(&self.dsh_home, "skills"),
                source: "user-dsh".to_string(),
                rank: USER_DSH_RANK,
                skip_system: true,
                project_root: None,
                trusted_host: false,
            });
            roots.push(SkillRoot {
                path: join_path(&self.agents_home, "skills"),
                source: "user-agents".to_string(),
                rank: USER_AGENTS_RANK,
                skip_system: false,
                project_root: None,
                trusted_host: false,
            });
        }
        if let Some(bundled) = &self.bundled_skill_dir {
            roots.push(SkillRoot {
                path: bundled.clone(),
                source: "bundled".to_string(),
                rank: BUNDLED_SKILL_RANK,
                skip_system: false,
                project_root: None,
                trusted_host: true,
            });
        }
        roots
    }

    fn optional_fs(&self) -> Option<Arc<dyn FileSystem>> {
        self.ctx
            .get_typed::<Arc<dyn FileSystem>>("fs", false)
            .map(|slot| slot.as_ref().clone())
    }

    /// Host-mutation observation (the TS `observeHostMutation`): a first-
    /// party write under a watched root invalidates immediately.
    pub fn observe_host_mutation(&self, path: &str) {
        let normalized = absolute(path);
        let relevant = self.watched_roots().iter().any(|root| {
            is_potential_skill_path(&SkillRoot {
                path: root.clone(),
                source: String::new(),
                rank: 0,
                skip_system: false,
                project_root: None,
                trusted_host: false,
            }, &normalized)
        });
        if relevant {
            (self.invalidate)();
        }
    }

    fn watched_roots(&self) -> Vec<String> {
        self.watcher.watchers.lock().keys().cloned().collect()
    }

    /// Close every host watcher and contain late callbacks.
    pub async fn dispose(&self) {
        self.watcher.dispose().await;
        if let Some(tx) = self.disposal.lock().take() {
            let _ = tx.send(());
        }
    }
}

#[async_trait::async_trait]
impl SkillProvider for FileSystemSkillProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn list(&self, options: &SkillLookupOptions) -> Result<SkillProviderObservation, String> {
        let roots = self.roots(options.cwd.as_deref()).await;
        if self.watch_enabled {
            // Watch existing roots only (deviation: missing roots are not
            // ancestor-watched).
            for root in &roots {
                self.watcher.observe(&root.path);
            }
        }
        let mut candidates: Vec<SkillCandidate> = Vec::new();
        for root in &roots {
            for skill in discover_root(root, &self.ctx, &self.name).await? {
                candidates.push(skill);
            }
        }
        Ok(SkillProviderObservation {
            candidates,
            complete: true,
        })
    }

    async fn get(
        &self,
        candidate: &SkillCandidate,
        options: &SkillLookupOptions,
    ) -> Result<Option<SkillDefinition>, String> {
        let locator = candidate
            .locator
            .clone()
            .downcast::<LocalLocator>()
            .map_err(|_| "skill locator mismatch".to_string())?;
        let parsed = parse_skill_file(
            &locator.path,
            &self.ctx,
            options.signal.clone(),
            candidate.source == "bundled",
        )
        .await?;
        let Some(parsed) = parsed else {
            return Ok(None);
        };
        Ok(Some(SkillDefinition {
            name: parsed.name,
            description: parsed.description,
            when_to_use: parsed.when_to_use,
            invocation: parsed.invocation,
            source: candidate.source.clone(),
            provider: self.name.clone(),
            resource_base: Some(SkillResourceBase::Directory {
                path: locator.directory.clone(),
            }),
            path: Some(locator.path.clone()),
            metadata: parsed.metadata,
            content: parsed.content,
        }))
    }
}

/// Discover one root's skills (TS `discoverRoot`).
pub async fn discover_root(
    root: &SkillRoot,
    ctx: &Context,
    provider: &str,
) -> Result<Vec<SkillCandidate>, String> {
    let mut skills: Vec<SkillCandidate> = Vec::new();
    let mut entries = list_skill_root_entries(root, ctx).await?;
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    for entry in entries.drain(..) {
        if root.skip_system && entry.name == ".system" {
            continue;
        }
        let locator = match entry.kind {
            EntryKind::Directory => LocalLocator {
                path: join_path(&entry.path, "SKILL.md"),
                directory: entry.path,
            },
            EntryKind::File if entry.name.ends_with(".md") => LocalLocator {
                path: entry.path,
                directory: root.path.clone(),
            },
            _ => continue,
        };
        let parsed = parse_skill_file(&locator.path, ctx, None, root.trusted_host).await?;
        let Some(parsed) = parsed else {
            continue;
        };
        skills.push(SkillCandidate {
            name: parsed.name,
            description: parsed.description,
            when_to_use: parsed.when_to_use,
            invocation: parsed.invocation,
            provider: provider.to_string(),
            source: root.source.clone(),
            resource_base: Some(SkillResourceBase::Directory {
                path: locator.directory.clone(),
            }),
            rank: root.rank,
            locator: arc(locator),
            path: None,
            metadata: parsed.metadata,
        });
    }
    Ok(skills)
}

async fn list_skill_root_entries(
    root: &SkillRoot,
    ctx: &Context,
) -> Result<Vec<SkillRootEntry>, String> {
    if let Some(fs) = ctx
        .get_typed::<Arc<dyn FileSystem>>("fs", false)
        .map(|slot| slot.as_ref().clone())
    {
        if !root.trusted_host {
            return list_entries_from_fs(&fs, &root.path).await;
        }
    }
    list_entries_from_std(&root.path).await
}

async fn list_entries_from_fs(
    fs: &Arc<dyn FileSystem>,
    path: &str,
) -> Result<Vec<SkillRootEntry>, String> {
    let target = match fs.resolve(path, None).await {
        Ok(target) => target,
        Err(error)
            if matches!(error.code, FsErrorCode::FsNotFound | FsErrorCode::FsNotDirectory) =>
        {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error.to_string()),
    };
    let entries = match fs.list_dir(&target, None).await {
        Ok(entries) => entries,
        Err(error)
            if matches!(error.code, FsErrorCode::FsNotFound | FsErrorCode::FsNotDirectory) =>
        {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error.to_string()),
    };
    Ok(entries.into_iter().map(entry_from_fs).collect())
}

fn entry_from_fs(entry: FsDirEntry) -> SkillRootEntry {
    SkillRootEntry {
        name: entry.name,
        kind: match entry.kind {
            FsInfoType::File => EntryKind::File,
            FsInfoType::Directory => EntryKind::Directory,
            FsInfoType::Other => EntryKind::Other,
        },
        path: entry.target.display_path,
    }
}

async fn list_entries_from_std(path: &str) -> Result<Vec<SkillRootEntry>, String> {
    let mut read = match tokio::fs::read_dir(path).await {
        Ok(read) => read,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut entries = Vec::new();
    while let Some(entry) = read.next_entry().await.map_err(|error| error.to_string())? {
        let name = entry.file_name().to_string_lossy().into_owned();
        let full_path = entry.path();
        let kind = match tokio::fs::symlink_metadata(&full_path).await {
            Ok(meta) => {
                let file_type = meta.file_type();
                if file_type.is_dir() {
                    EntryKind::Directory
                } else if file_type.is_file() {
                    EntryKind::File
                } else {
                    EntryKind::Other
                }
            }
            Err(_) => EntryKind::Other,
        };
        entries.push(SkillRootEntry {
            name,
            kind,
            path: full_path.to_string_lossy().into_owned(),
        });
    }
    Ok(entries)
}

/// Parse one skill file's frontmatter and body (TS `parseSkillFile`).
pub async fn parse_skill_file(
    path: &str,
    ctx: &Context,
    signal: Option<SkillAbort>,
    trusted_host: bool,
) -> Result<Option<ParsedSkill>, String> {
    if signal.as_ref().is_some_and(|signal| signal()) {
        return Err(dsh_skill::SKILL_ABORTED_MESSAGE.to_string());
    }
    let raw = read_skill_text(ctx, path, signal.clone(), trusted_host).await?;
    if signal.as_ref().is_some_and(|signal| signal()) {
        return Err(dsh_skill::SKILL_ABORTED_MESSAGE.to_string());
    }
    let Some(raw) = raw else {
        return Ok(None);
    };
    let Some(frontmatter) = parse_frontmatter(&raw) else {
        warn_skill(ctx, path, "missing YAML frontmatter");
        return Ok(None);
    };
    let data: serde_yaml::Mapping = match frontmatter {
        serde_yaml::Value::Mapping(mapping) => mapping,
        _ => {
            warn_skill(ctx, path, "invalid YAML frontmatter");
            return Ok(None);
        }
    };
    let name = string_field(&data, "name");
    let description = string_field(&data, "description");
    let (Some(name), Some(description)) = (name, description) else {
        warn_skill(ctx, path, "frontmatter requires name and description");
        return Ok(None);
    };
    if !is_skill_name(&name) {
        warn_skill(ctx, path, &format!("invalid skill name \"{name}\""));
        return Ok(None);
    }
    let invocation = match parse_invocation_policy(&data) {
        Ok(policy) => policy,
        Err(error) => {
            warn_skill(ctx, path, &format!("invalid invocation frontmatter: {error}"));
            return Ok(None);
        }
    };
    Ok(Some(ParsedSkill {
        name,
        description,
        when_to_use: optional_string(&data, "whenToUse"),
        invocation,
        metadata: optional_metadata(&data),
        content: frontmatter_body(&raw).unwrap_or_default().trim().to_string(),
    }))
}

fn warn_skill(ctx: &Context, path: &str, reason: &str) {
    ctx.named_logger(None)
        .warn(vec![arc(format!("skill file {path} ignored: {reason}"))]);
}

async fn read_skill_text(
    ctx: &Context,
    path: &str,
    signal: Option<SkillAbort>,
    trusted_host: bool,
) -> Result<Option<String>, String> {
    if signal.as_ref().is_some_and(|signal| signal()) {
        return Err(dsh_skill::SKILL_ABORTED_MESSAGE.to_string());
    }
    if let Some(fs) = ctx
        .get_typed::<Arc<dyn FileSystem>>("fs", false)
        .map(|slot| slot.as_ref().clone())
    {
        if !trusted_host {
            return read_skill_text_from_fs(ctx, &fs, path, signal.clone()).await;
        }
    }
    match tokio::fs::read_to_string(path).await {
        Ok(text) => Ok(Some(text)),
        Err(error) => {
            if signal.as_ref().is_some_and(|signal| signal()) {
                return Err(dsh_skill::SKILL_ABORTED_MESSAGE.to_string());
            }
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            Err(error.to_string())
        }
    }
}

async fn read_skill_text_from_fs(
    ctx: &Context,
    fs: &Arc<dyn FileSystem>,
    path: &str,
    signal: Option<SkillAbort>,
) -> Result<Option<String>, String> {
    if signal.as_ref().is_some_and(|signal| signal()) {
        return Err(dsh_skill::SKILL_ABORTED_MESSAGE.to_string());
    }
    let target = match fs.resolve(path, None).await {
        Ok(target) => target,
        Err(error)
            if matches!(error.code, FsErrorCode::FsNotFound | FsErrorCode::FsNotDirectory) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.to_string()),
    };
    if signal.as_ref().is_some_and(|signal| signal()) {
        return Err(dsh_skill::SKILL_ABORTED_MESSAGE.to_string());
    }
    let info = match fs.stat(&target, signal.clone()).await {
        Ok(info) => info,
        Err(error) => {
            if signal.as_ref().is_some_and(|signal| signal()) {
                return Err(dsh_skill::SKILL_ABORTED_MESSAGE.to_string());
            }
            if matches!(error.code, FsErrorCode::FsNotFound | FsErrorCode::FsNotDirectory) {
                return Ok(None);
            }
            return Err(error.to_string());
        }
    };
    if info.as_ref().is_none_or(|info| info.kind != FsInfoType::File) {
        return Ok(None);
    }
    match fs.read_text(&target, signal.clone()).await {
        Ok(text) => Ok(Some(text)),
        Err(error) => {
            if signal.as_ref().is_some_and(|signal| signal()) {
                return Err(dsh_skill::SKILL_ABORTED_MESSAGE.to_string());
            }
            if matches!(error.code, FsErrorCode::FsNotFound | FsErrorCode::FsNotDirectory) {
                return Ok(None);
            }
            if !matches!(error.code, FsErrorCode::FsNotText) {
                return Err(error.to_string());
            }
            warn_skill(
                ctx,
                path,
                &format!("failed to read text file at {}: {}", target.display_path, error),
            );
            Ok(None)
        }
    }
}

fn string_field(data: &serde_yaml::Mapping, key: &str) -> Option<String> {
    match data.get(serde_yaml::Value::String(key.to_string())) {
        Some(serde_yaml::Value::String(value)) if !value.is_empty() => Some(value.clone()),
        _ => None,
    }
}

fn optional_string(data: &serde_yaml::Mapping, key: &str) -> Option<String> {
    string_field(data, key)
}

fn optional_metadata(data: &serde_yaml::Mapping) -> Option<serde_json::Value> {
    match data.get(serde_yaml::Value::String("metadata".to_string())) {
        Some(serde_yaml::Value::Mapping(_)) => {
            serde_json::to_value(data.get(serde_yaml::Value::String("metadata".to_string())))
                .ok()
        }
        _ => None,
    }
}

fn parse_invocation_policy(
    data: &serde_yaml::Mapping,
) -> Result<SkillInvocationPolicy, String> {
    let legacy = [
        ("disableModelInvocation", "disable-model-invocation"),
        ("modelInvocable", "disable-model-invocation"),
        ("userInvocable", "user-invocable"),
    ];
    for (legacy_key, canonical) in legacy {
        if data.contains_key(serde_yaml::Value::String(legacy_key.to_string())) {
            return Err(format!(
                "frontmatter field \"{legacy_key}\" is unsupported; use \"{canonical}\""
            ));
        }
    }
    let disable_model = frontmatter_bool(data, "disable-model-invocation")?;
    let user_invocable = frontmatter_bool(data, "user-invocable")?;
    Ok(SkillInvocationPolicy {
        model_invocable: disable_model != Some(true),
        user_invocable: user_invocable != Some(false),
    })
}

fn frontmatter_bool(
    data: &serde_yaml::Mapping,
    key: &str,
) -> Result<Option<bool>, String> {
    match data.get(serde_yaml::Value::String(key.to_string())) {
        None => Ok(None),
        Some(serde_yaml::Value::Bool(value)) => Ok(Some(*value)),
        Some(serde_yaml::Value::Number(value)) if value.as_i64() == Some(1) => Ok(Some(true)),
        Some(serde_yaml::Value::Number(value)) if value.as_i64() == Some(0) => Ok(Some(false)),
        Some(serde_yaml::Value::String(value)) => match value.to_lowercase().as_str() {
            "true" | "yes" | "on" => Ok(Some(true)),
            "false" | "no" | "off" => Ok(Some(false)),
            _ => Err(format!("frontmatter field \"{key}\" must be a boolean")),
        },
        Some(_) => Err(format!("frontmatter field \"{key}\" must be a boolean")),
    }
}

/// Whether the raw text starts with a `---` frontmatter block.
fn parse_frontmatter(raw: &str) -> Option<serde_yaml::Value> {
    let first_line_end = raw.find('\n')?;
    let first_line = raw[..first_line_end].trim_end_matches('\r');
    if first_line != "---" {
        return None;
    }
    let start = first_line_end + 1;
    let closing = find_closing_frontmatter(raw, start)?;
    let yaml = &raw[start..closing.0];
    serde_yaml::from_str::<serde_yaml::Value>(yaml).ok()
}

fn find_closing_frontmatter(raw: &str, start: usize) -> Option<(usize, usize)> {
    let mut line_start = start;
    while line_start <= raw.len() {
        let next_newline = raw[line_start..].find('\n').map(|offset| line_start + offset);
        let line_end = next_newline.unwrap_or(raw.len());
        let line = raw[line_start..line_end].trim_end_matches('\r');
        if line == "---" {
            let body_start = next_newline.map(|index| index + 1).unwrap_or(raw.len());
            return Some((line_start, body_start));
        }
        let Some(next) = next_newline else {
            return None;
        };
        line_start = next + 1;
    }
    None
}

fn frontmatter_body(raw: &str) -> Option<&str> {
    let first_line_end = raw.find('\n')?;
    let start = first_line_end + 1;
    let (_, body_start) = find_closing_frontmatter(raw, start)?;
    Some(&raw[body_start..])
}

async fn find_project_root(cwd: &str, fs: Option<Arc<dyn FileSystem>>) -> String {
    let mut current = PathBuf::from(cwd);
    loop {
        let git = current.join(".git");
        let exists = match &fs {
            Some(fs) => path_exists_in_fs(fs, &git.to_string_lossy()).await,
            None => tokio::fs::metadata(&git).await.is_ok(),
        };
        if exists {
            return current.to_string_lossy().into_owned();
        }
        let parent = match current.parent() {
            Some(parent) => parent.to_path_buf(),
            None => return cwd.to_string(),
        };
        if parent == current {
            return cwd.to_string();
        }
        current = parent;
    }
}

async fn path_exists_in_fs(fs: &Arc<dyn FileSystem>, path: &str) -> bool {
    let Ok(target) = fs.resolve(path, None).await else {
        return false;
    };
    fs.stat(&target, None).await.is_ok_and(|info| info.is_some())
}

fn is_potential_skill_path(root: &SkillRoot, path: &str) -> bool {
    let Some(segments) = contained_segments(&root.path, path) else {
        return false;
    };
    if segments.is_empty() || segments.len() > 2 {
        return false;
    }
    if root.skip_system && segments[0] == ".system" {
        return false;
    }
    if segments.len() == 1 {
        segments[0].ends_with(".md")
    } else {
        segments[1] == "SKILL.md"
    }
}

fn contained_segments(root: &str, path: &str) -> Option<Vec<String>> {
    let root_path = PathBuf::from(root);
    let child_path = PathBuf::from(path);
    let relative = child_path.strip_prefix(&root_path).ok()?;
    let segments: Vec<String> = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    if segments.iter().any(|segment| segment == "..") {
        return None;
    }
    Some(segments)
}

fn join_path(root: &str, tail: &str) -> String {
    Path::new(root).join(tail).to_string_lossy().into_owned()
}

fn absolute(path: &str) -> String {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_string_lossy().into_owned()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path).to_string_lossy().into_owned(),
            Err(_) => path.to_string_lossy().into_owned(),
        }
    }
}

/// The Cordis plugin form (TS module exports: `name`, `inject`, `Config`,
/// `apply`).
pub struct SkillFilesystemPlugin {
    config: Config,
}

impl SkillFilesystemPlugin {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl Plugin for SkillFilesystemPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(["skills"])
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        let disposer = apply(ctx, self.config.clone())
            .await
            .map_err(|message| PluginError::from(anyhow::anyhow!(message)))?;
        let _ = ctx.effect("skill-filesystem", Box::pin(async move { Some(disposer) }));
        Ok(())
    }
}
