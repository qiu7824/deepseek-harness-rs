//! Rust port of the core
//! `packages/fs/tool-str-replace-editor/tests/tools.spec.ts` behaviors:
//! the standalone schema and presentation, create/view/replace/insert with
//! the canonical output, view ranges and clipping, directory listing,
//! observation-policy read-before-edit gating, sandbox policy forwarding,
//! and config validation.
//!
//! # Deviations
//!
//! - The TS patches `ctx.fs` methods after load; the Rust backend is a
//!   forwarding wrapper whose overrides are set before the tool mounts.
//! - The "confined fs without policy" case uses a stub whose
//!   `sandbox_mode()` reports confinement (the TS `defineProperty` patch).
//! - Temp roots are per-test directories under the system temp path.

use std::path::PathBuf;
use std::sync::Arc;

use cordis::{ArcValue, Context, FiberCore, Plugin, PluginError, arc};
use dsh_agent::{Agent, AgentOptions, AgentRegistry, AgentStatus, Inbox};
use dsh_fs::{
    AbortPredicate, FileSystem, FsDirEntry, FsEditGuard, FsEditOutcome, FsEditRequest, FsError,
    FsInfo, FsPathInfo, FsTarget, FsWriteIntent, FsWriteOutcome, LstatOptions, ResolveOptions,
    fs_version,
};
use dsh_fs_local::LocalFileSystem;
use dsh_fs_observation_policy;
use dsh_fs_sandbox::SandboxedFileSystem;
use dsh_llm::{ContentBlock, call_id};
use dsh_sandbox::SandboxMode;
use dsh_sandbox_policy::SandboxPolicyService;
use dsh_scope::ScopeKey;
use dsh_session::{Session, SessionHeader, SessionId, session_id};
use dsh_system_prompt::SystemPrompt;
use dsh_tools::{ToolCallView, ToolExecutionInput, ToolExecutionResult, ToolRuntime};
use dsh_tool_str_replace_editor::{Config, ToolStrReplaceEditorPlugin, apply};
use parking_lot::Mutex;

fn never_abort() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| false)
}

fn temp_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "dsh-tool-str-replace-editor-{label}-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).expect("temp root");
    root
}

/// A forwarding filesystem wrapper with settable overrides (the TS
/// post-load `ctx.fs` method patches).
struct MockFs {
    inner: Arc<dyn FileSystem>,
    confined: Mutex<Option<dsh_sandbox::SandboxMode>>,
    list_dir_override: Mutex<
        Option<
            Arc<
                dyn Fn(
                        &FsTarget,
                        Option<AbortPredicate>,
                    )
                        -> futures::future::BoxFuture<'static, Result<Vec<FsDirEntry>, FsError>>
                    + Send
                    + Sync,
            >,
        >,
    >,
    stat_override: Mutex<
        Option<
            Arc<
                dyn Fn(&FsTarget, Option<AbortPredicate>)
                    -> futures::future::BoxFuture<'static, Result<Option<FsInfo>, FsError>>
                    + Send
                    + Sync,
            >,
        >,
    >,
    write_override: Mutex<
        Option<
            Arc<
                dyn Fn(
                        &FsTarget,
                        &str,
                        Option<&FsWriteIntent>,
                        Option<AbortPredicate>,
                        Option<&dsh_sandbox::SandboxExecutionPolicy>,
                    )
                        -> futures::future::BoxFuture<'static, Result<FsWriteOutcome, FsError>>
                    + Send
                    + Sync,
            >,
        >,
    >,
}

impl MockFs {
    fn new(inner: Arc<dyn FileSystem>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            confined: Mutex::new(None),
            list_dir_override: Mutex::new(None),
            stat_override: Mutex::new(None),
            write_override: Mutex::new(None),
        })
    }
}

#[async_trait::async_trait]
impl FileSystem for MockFs {
    fn sandbox_mode(&self) -> Option<dsh_sandbox::SandboxMode> {
        *self.confined.lock()
    }

    async fn resolve(&self, path: &str, opts: Option<&ResolveOptions>) -> Result<FsTarget, FsError> {
        self.inner.resolve(path, opts).await
    }

    fn process_path(&self, target: &FsTarget) -> String {
        self.inner.process_path(target)
    }

    fn file_url(&self, target: &FsTarget) -> String {
        self.inner.file_url(target)
    }

    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool {
        self.inner.contains(parent, child)
    }

    async fn stat(&self, target: &FsTarget, signal: Option<AbortPredicate>) -> Result<Option<FsInfo>, FsError> {
        let override_fn = { self.stat_override.lock().clone() };
        if let Some(override_fn) = override_fn {
            return override_fn(target, signal).await;
        }
        self.inner.stat(target, signal).await
    }

    async fn lstat(
        &self,
        path: &str,
        opts: Option<&LstatOptions>,
        signal: Option<AbortPredicate>,
    ) -> Result<Option<FsPathInfo>, FsError> {
        self.inner.lstat(path, opts, signal).await
    }

    async fn read_text(&self, target: &FsTarget, signal: Option<AbortPredicate>) -> Result<String, FsError> {
        self.inner.read_text(target, signal).await
    }

    async fn stream_text(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<futures::stream::BoxStream<'static, Result<String, FsError>>, FsError> {
        self.inner.stream_text(target, signal).await
    }

    async fn read_bytes(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
        max_bytes: u64,
    ) -> Result<Vec<u8>, FsError> {
        self.inner.read_bytes(target, signal, max_bytes).await
    }

    async fn list_dir(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<Vec<FsDirEntry>, FsError> {
        let override_fn = { self.list_dir_override.lock().clone() };
        if let Some(override_fn) = override_fn {
            return override_fn(target, signal).await;
        }
        self.inner.list_dir(target, signal).await
    }

    async fn write_text(
        &self,
        target: &FsTarget,
        content: &str,
        expected: Option<&FsWriteIntent>,
        signal: Option<AbortPredicate>,
        sandbox_policy: Option<&dsh_sandbox::SandboxExecutionPolicy>,
    ) -> Result<FsWriteOutcome, FsError> {
        let override_fn = { self.write_override.lock().clone() };
        if let Some(override_fn) = override_fn {
            return override_fn(target, content, expected, signal, sandbox_policy).await;
        }
        self.inner
            .write_text(target, content, expected, signal, sandbox_policy)
            .await
    }

    async fn edit_text(
        &self,
        target: &FsTarget,
        edit: &FsEditRequest,
        expected: Option<&FsEditGuard>,
        signal: Option<AbortPredicate>,
        sandbox_policy: Option<&dsh_sandbox::SandboxExecutionPolicy>,
    ) -> Result<FsEditOutcome, FsError> {
        self.inner
            .edit_text(target, edit, expected, signal, sandbox_policy)
            .await
    }
}

impl cordis::Service for MockFs {
    fn service_name(&self) -> &'static str {
        "fs"
    }
}

/// A fake owner agent whose session cwd is the test root.
struct StubAgent {
    id: SessionId,
    session: Session,
    inbox: Inbox,
    ctx: Context,
    scope_key: ScopeKey,
    options: AgentOptions,
    status: Mutex<AgentStatus>,
}

impl StubAgent {
    fn new(ctx: &Context, root: &PathBuf) -> (Arc<dyn Agent>, Arc<FiberCore>) {
        let fiber = ctx.plugin(Arc::new(NoopPlugin), arc(()));
        let agent_ctx = fiber.ctx().expect("plugin ctx bound at load");
        let id = session_id(format!("str-replace-editor-owner-{}", uuid::Uuid::new_v4()));
        let header = SessionHeader {
            version: dsh_session::SESSION_FORMAT_VERSION,
            id: id.clone(),
            created_at: 0,
            cwd: Some(root.to_string_lossy().into_owned()),
            parent_session: None,
            seed_length: None,
            origin: None,
            delegation_depth: None,
            agent_preset: None,
        };
        let session = Session::create(id.clone(), None, Some(&header)).expect("session");
        let inbox = Inbox::new(&session, Default::default()).expect("inbox");
        let agent: Arc<dyn Agent> = Arc::new(Self {
            id,
            session,
            inbox,
            ctx: agent_ctx,
            scope_key: ScopeKey::new(),
            options: AgentOptions::default(),
            status: Mutex::new(AgentStatus::Idle),
        });
        (agent, fiber)
    }
}

struct NoopPlugin;

#[async_trait::async_trait]
impl Plugin for NoopPlugin {
    async fn apply(&self, _ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        Ok(())
    }
}

impl Agent for StubAgent {
    fn id(&self) -> &SessionId {
        &self.id
    }

    fn options(&self) -> &AgentOptions {
        &self.options
    }

    fn session(&self) -> &Session {
        &self.session
    }

    fn inbox(&self) -> &Inbox {
        &self.inbox
    }

    fn status(&self) -> AgentStatus {
        *self.status.lock()
    }

    fn ctx(&self) -> &Context {
        &self.ctx
    }

    fn scope_key(&self) -> &ScopeKey {
        &self.scope_key
    }

    fn cancel(&self, _cause: dsh_session::AgentCancelCause, _options: Option<&dsh_agent::CancelOptions>) {}

    fn when_idle(&self) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn run_maintenance(
        &self,
        _task: Arc<dyn Fn() -> cordis::BoxFuture<'static, ()> + Send + Sync>,
    ) -> cordis::BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn send(&self, _message: dsh_session::UserMessage, _target: dsh_agent::InboxTarget, _wakeup: bool) {}

    fn followup(&self, _message: dsh_session::UserMessage) {}

    fn steer(&self, _message: dsh_session::UserMessage) {}

    fn inject(&self, _message: dsh_session::UserMessage) {}
}

/// The assembled test world.
struct World {
    ctx: Context,
    root: PathBuf,
    fiber: Arc<FiberCore>,
    tools: Arc<ToolRuntime>,
    owner: Option<Arc<dyn Agent>>,
    mock: Arc<MockFs>,
}

struct SetupOptions {
    fs_policy: bool,
    sandbox_mode: Option<SandboxMode>,
    config: serde_json::Value,
}

impl Default for SetupOptions {
    fn default() -> Self {
        Self {
            fs_policy: false,
            sandbox_mode: None,
            config: serde_json::json!({}),
        }
    }
}

async fn setup_with(root: PathBuf, options: SetupOptions) -> World {
    let ctx = Context::root();
    SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("prompt");
    let tools = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    let registry = AgentRegistry::install(&ctx);
    let inner = match options.sandbox_mode {
        None => {
            let backend = LocalFileSystem::build(dsh_fs_local::Config {
                cwd: Some(root.to_string_lossy().into_owned()),
                diff_basis_max_bytes: None,
            })
            .expect("fs-local");
            backend as Arc<dyn FileSystem>
        }
        Some(mode) => {
            SandboxPolicyService::install(
                &ctx,
                dsh_sandbox_policy::Config {
                    mode: Some(mode),
                    workspace_root: Some(root.to_string_lossy().into_owned()),
                },
            );
            let backend = SandboxedFileSystem::build(
                &ctx,
                dsh_fs_sandbox::Config { cwd: Some(root.to_string_lossy().into_owned()), diff_basis_max_bytes: None },
            )
            .expect("fs-sandbox");
            backend as Arc<dyn FileSystem>
        }
    };
    let mock = MockFs::new(inner);
    {
        let erased: Arc<dyn FileSystem> = mock.clone();
        ctx.register_service(erased);
    }
    if options.fs_policy {
        let _ = dsh_fs_observation_policy::apply(&ctx);
    }
    let fiber = ctx.plugin(Arc::new(ToolStrReplaceEditorPlugin::new()), arc(options.config));
    fiber.settle().await.expect("tool-str-replace-editor loads");
    let (owner, _fiber) = StubAgent::new(&ctx, &root);
    registry.enter(owner.clone(), None).expect("enter owner");
    registry.announce(&owner).await.expect("announce owner");
    World { ctx, root, fiber, tools, owner: Some(owner), mock }
}

async fn setup(root: PathBuf) -> World {
    setup_with(root, SetupOptions::default()).await
}

fn text(result: &ToolExecutionResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

async fn call(
    world: &World,
    owner: Option<Arc<dyn Agent>>,
    args: serde_json::Value,
) -> Arc<ToolExecutionResult> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    world
        .tools
        .execute(ToolExecutionInput {
            call_id: call_id(format!(
                "str-replace-editor-{}",
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
            )),
            root_call_id: None,
            name: "str_replace_editor".to_string(),
            arguments: args,
            agent: owner,
            parent: None,
            signal: never_abort(),
        })
        .await
}

fn read_file(root: &PathBuf, name: &str) -> String {
    std::fs::read_to_string(root.join(name)).expect("read file")
}

fn write_file(root: &PathBuf, name: &str, content: &str) {
    std::fs::write(root.join(name), content).expect("write file");
}

fn error_code(result: &ToolExecutionResult) -> String {
    result
        .error
        .as_ref()
        .and_then(|error| error.info.as_ref())
        .map(|info| info.code.clone())
        .unwrap_or_default()
}

// ---- the tool surface ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registers_the_standalone_schema_and_configurable_description() {
    let root = temp_root("schema");
    let ctx = Context::root();
    SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("prompt");
    let tools = ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    AgentRegistry::install(&ctx);
    let backend = LocalFileSystem::build(dsh_fs_local::Config {
        cwd: Some(root.to_string_lossy().into_owned()),
        diff_basis_max_bytes: None,
    })
    .expect("fs-local");
    let mock = MockFs::new(backend);
    {
        let erased: Arc<dyn FileSystem> = mock.clone();
        ctx.register_service(erased);
    }
    let fiber = ctx.plugin(
        Arc::new(ToolStrReplaceEditorPlugin::new()),
        arc(serde_json::json!({ "description": "custom editor description" })),
    );
    fiber.settle().await.expect("loads");

    let schemas = tools.schemas(None);
    assert_eq!(
        schemas.iter().map(|schema| schema.name.as_str()).collect::<Vec<_>>(),
        vec!["str_replace_editor"]
    );
    assert_eq!(schemas[0].description, "custom editor description");
    let properties = &schemas[0].parameters["properties"];
    assert!(properties.get("replace_all").is_none());
    assert_eq!(properties["insert_line"]["type"], "integer");
    assert_eq!(properties["view_range"]["items"]["type"], "integer");

    let definition = tools.get("str_replace_editor", None).expect("registered");
    let present = definition.present_call.as_ref().expect("present");
    let view = present(&serde_json::json!({ "command": "view", "path": "/workspace/a.txt" }));
    assert_eq!(
        view,
        Some(ToolCallView::Generic {
            title: "view /workspace/a.txt".to_string(),
            kind: Some(dsh_tools::ToolCallKind::Read),
            raw_input: None,
            content: None,
            locations: Some(vec![dsh_tools::FileLocation { path: "/workspace/a.txt".to_string(), line: None }]),
        })
    );
    let create = present(&serde_json::json!({ "command": "create", "path": "/workspace/a.txt", "file_text": "hello" }));
    assert_eq!(
        create,
        Some(ToolCallView::Diff {
            title: "create /workspace/a.txt".to_string(),
            diffs: vec![dsh_tools::FileDiff {
                path: "/workspace/a.txt".to_string(),
                old_text: None,
                new_text: "hello".to_string(),
            }],
            locations: Some(vec![dsh_tools::FileLocation { path: "/workspace/a.txt".to_string(), line: None }]),
        })
    );
    let replace = present(&serde_json::json!({ "command": "str_replace", "path": "/workspace/a.txt", "old_str": "old", "new_str": "new" }));
    assert_eq!(
        replace,
        Some(ToolCallView::Diff {
            title: "str_replace /workspace/a.txt".to_string(),
            diffs: vec![dsh_tools::FileDiff {
                path: "/workspace/a.txt".to_string(),
                old_text: Some("old".to_string()),
                new_text: "new".to_string(),
            }],
            locations: Some(vec![dsh_tools::FileLocation { path: "/workspace/a.txt".to_string(), line: None }]),
        })
    );
    let insert = present(&serde_json::json!({ "command": "insert", "path": "/workspace/a.txt", "insert_line": 0, "new_str": "x" }));
    assert_eq!(
        insert,
        Some(ToolCallView::Generic {
            title: "insert /workspace/a.txt".to_string(),
            kind: Some(dsh_tools::ToolCallKind::Edit),
            raw_input: None,
            content: None,
            locations: Some(vec![dsh_tools::FileLocation { path: "/workspace/a.txt".to_string(), line: Some(1) }]),
        })
    );
    let create_empty = present(&serde_json::json!({ "command": "create", "path": "/workspace/empty.txt" }));
    match create_empty {
        Some(ToolCallView::Diff { diffs, .. }) => {
            assert_eq!(diffs[0].new_text, "");
        }
        other => panic!("{other:?}"),
    }
    let replace_empty = present(&serde_json::json!({ "command": "str_replace", "path": "/workspace/a.txt" }));
    match replace_empty {
        Some(ToolCallView::Diff { diffs, .. }) => {
            assert_eq!(diffs[0].old_text, None);
            assert_eq!(diffs[0].new_text, "");
        }
        other => panic!("{other:?}"),
    }
    let insert_bare = present(&serde_json::json!({ "command": "insert", "path": "/workspace/a.txt" }));
    match insert_bare {
        Some(ToolCallView::Generic { locations, .. }) => {
            assert_eq!(
                locations,
                Some(vec![dsh_tools::FileLocation { path: "/workspace/a.txt".to_string(), line: None }])
            );
        }
        other => panic!("{other:?}"),
    }

    fiber.dispose().await;
    assert!(tools.schemas(None).is_empty());
    assert!(tools.get("str_replace_editor", None).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn creates_views_replaces_and_inserts_with_the_canonical_model_facing_output() {
    let root = temp_root("canonical");
    let world = setup(root.clone()).await;
    let owner = world.owner.clone();

    let sample = root.join("sample.txt");
    let created = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "create", "path": sample.to_string_lossy(), "file_text": "one\ntwo\nthree\n" }),
    )
    .await;
    assert_eq!(
        text(&created),
        format!("New file created successfully at: {}", sample.display())
    );

    let viewed = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "view", "path": sample.to_string_lossy(), "view_range": [2, -1] }),
    )
    .await;
    assert_eq!(
        text(&viewed),
        [
            format!("Here's the content of {} with line numbers (which has a total of 4 lines) with view_range=[2, -1]:", sample.display()),
            "     2  two".to_string(),
            "     3  three".to_string(),
            "     4  ".to_string(),
            String::new(),
        ]
        .join("\n")
    );

    let replaced = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "str_replace", "path": sample.to_string_lossy(), "old_str": "two", "new_str": "TWO" }),
    )
    .await;
    assert_eq!(text(&replaced), format!("The file {} has been edited successfully.", sample.display()));
    let cleared = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "str_replace", "path": sample.to_string_lossy(), "old_str": "TWO" }),
    )
    .await;
    assert_eq!(text(&cleared), format!("The file {} has been edited successfully.", sample.display()));
    let inserted = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "insert", "path": sample.to_string_lossy(), "insert_line": 1, "new_str": "between" }),
    )
    .await;
    assert_eq!(text(&inserted), format!("The file {} has been edited successfully.", sample.display()));
    assert_eq!(read_file(&root, "sample.txt"), "one\nbetween\n\nthree\n");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_view_records_absence_so_create_can_recover_after_external_deletion() {
    let root = temp_root("absence");
    let world = setup_with(
        root.clone(),
        SetupOptions { fs_policy: true, ..Default::default() },
    )
    .await;
    let owner = world.owner.clone();
    write_file(&root, "deleted.txt", "original");

    let first = call(&world, owner.clone(), serde_json::json!({ "command": "view", "path": root.join("deleted.txt").to_string_lossy() })).await;
    assert!(!first.is_error);
    std::fs::remove_file(root.join("deleted.txt")).expect("rm");

    let missing = call(&world, owner.clone(), serde_json::json!({ "command": "view", "path": root.join("deleted.txt").to_string_lossy() })).await;
    assert!(missing.is_error);
    assert_eq!(error_code(&missing), "FS_NOT_FOUND");

    let edit = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "str_replace", "path": root.join("deleted.txt").to_string_lossy(), "old_str": "original", "new_str": "edited" }),
    )
    .await;
    assert!(edit.is_error);
    assert_eq!(error_code(&edit), "FS_NOT_FOUND");

    let created = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "create", "path": root.join("deleted.txt").to_string_lossy(), "file_text": "fresh" }),
    )
    .await;
    assert!(!created.is_error);
    assert_eq!(read_file(&root, "deleted.txt"), "fresh");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn writes_replacement_text_literally() {
    let root = temp_root("literal");
    let world = setup(root.clone()).await;
    let owner = world.owner.clone();
    let replacement = "$&|$`|$'|$$";
    write_file(&root, "literal.txt", "before OLD after");

    let result = call(
        &world,
        owner,
        serde_json::json!({ "command": "str_replace", "path": root.join("literal.txt").to_string_lossy(), "old_str": "OLD", "new_str": replacement }),
    )
    .await;
    assert!(!result.is_error);
    assert_eq!(
        read_file(&root, "literal.txt"),
        format!("before {replacement} after")
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lists_visible_entries_to_depth_two_and_clips_at_the_configured_view_limit() {
    let root = temp_root("listing");
    let world = setup_with(
        root.clone(),
        SetupOptions { config: serde_json::json!({ "maxOutputChars": 10_000 }), ..Default::default() },
    )
    .await;
    let owner = world.owner.clone();
    std::fs::create_dir_all(root.join("dir").join("nested").join("third")).expect("dirs");
    std::fs::create_dir_all(root.join("dir").join("node_modules").join("pkg")).expect("dirs");
    std::fs::create_dir_all(root.join("dir").join("node_modules_old")).expect("dirs");
    std::fs::create_dir_all(root.join("dir").join("__pycache__")).expect("dirs");
    std::fs::create_dir_all(root.join("dir").join("__pycache__backup")).expect("dirs");
    write_file(&root, "dir/visible.txt", "ok");
    write_file(&root, "dir/.hidden", "hidden");
    write_file(&root, "dir/nested/child.txt", "child");
    write_file(&root, "dir/nested/third/too-deep.txt", "deep");
    write_file(&root, "dir/node_modules/pkg/index.js", "hidden dependency");
    write_file(&root, "dir/node_modules_old/kept.js", "visible source");
    write_file(&root, "dir/__pycache__/module.pyc", "cache");
    write_file(&root, "dir/__pycache__backup/kept.py", "visible source");

    let listing = text(
        call(&world, owner, serde_json::json!({ "command": "view", "path": root.join("dir").to_string_lossy() }))
            .await
            .as_ref(),
    );
    assert!(!listing.contains(".hidden"), "{listing}");
    assert!(!listing.contains("too-deep.txt"), "{listing}");
    assert!(!listing.contains("index.js"), "{listing}");
    assert!(!listing.contains("module.pyc"), "{listing}");
    assert!(listing.contains("node_modules_old"), "{listing}");
    assert!(listing.contains("kept.js"), "{listing}");
    assert!(listing.contains("__pycache__backup"), "{listing}");
    assert!(listing.contains("kept.py"), "{listing}");

    let clipped_root = temp_root("clipped");
    let clipped = setup_with(
        clipped_root.clone(),
        SetupOptions { config: serde_json::json!({ "maxOutputChars": 10 }), ..Default::default() },
    )
    .await;
    write_file(&clipped_root, "large.txt", &"x".repeat(100));
    let view = call(
        &clipped,
        clipped.owner.clone(),
        serde_json::json!({ "command": "view", "path": clipped_root.join("large.txt").to_string_lossy() }),
    )
    .await;
    assert!(text(&view).contains("<response clipped>"), "{}", text(&view));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&clipped_root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn matches_canonical_empty_line_range_and_end_insert_behavior() {
    let root = temp_root("canonical-empty");
    let world = setup(root.clone()).await;
    let owner = world.owner.clone();
    write_file(&root, "empty.txt", "");
    write_file(&root, "newline.txt", "\n");
    write_file(&root, "plain.txt", "one\ntwo");

    let empty_view = call(&world, owner.clone(), serde_json::json!({ "command": "view", "path": root.join("empty.txt").to_string_lossy() })).await;
    assert!(
        text(&empty_view).contains("(which has a total of 1 lines):\n     1  \n"),
        "{}",
        text(&empty_view)
    );
    let newline_view = call(&world, owner.clone(), serde_json::json!({ "command": "view", "path": root.join("newline.txt").to_string_lossy() })).await;
    assert!(
        text(&newline_view).contains("(which has a total of 2 lines):\n     1  \n     2  \n"),
        "{}",
        text(&newline_view)
    );
    let range_view = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "view", "path": root.join("plain.txt").to_string_lossy(), "view_range": [1, 2] }),
    )
    .await;
    assert!(text(&range_view).contains("     2  two"));
    let ownerless_view = call(&world, None, serde_json::json!({ "command": "view", "path": root.join("plain.txt").to_string_lossy() })).await;
    assert!(text(&ownerless_view).contains("     1  one"));
    let ownerless_create = call(
        &world,
        None,
        serde_json::json!({ "command": "create", "path": root.join("ownerless.txt").to_string_lossy(), "file_text": "ownerless" }),
    )
    .await;
    assert!(!ownerless_create.is_error);

    let _ = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "insert", "path": root.join("plain.txt").to_string_lossy(), "insert_line": 2, "new_str": "three" }),
    )
    .await;
    assert_eq!(read_file(&root, "plain.txt"), "one\ntwo\nthree");

    write_file(&root, "newline.txt", "one\n");
    let _ = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "insert", "path": root.join("newline.txt").to_string_lossy(), "insert_line": 2, "new_str": "three" }),
    )
    .await;
    assert_eq!(read_file(&root, "newline.txt"), "one\n\nthree");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uses_old_str_only_replacement_failures_and_rejects_relative_paths() {
    let root = temp_root("failures");
    let world = setup(root.clone()).await;
    let owner = world.owner.clone();
    write_file(&root, "ambiguous.txt", "same\nother\nsame");

    let missing = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "str_replace", "path": root.join("ambiguous.txt").to_string_lossy(), "old_str": "absent", "new_str": "x" }),
    )
    .await;
    assert!(missing.is_error);
    assert!(text(&missing).contains("old_str `absent` did not appear verbatim in"));
    assert!(!text(&missing).contains("old_string"));

    let repeated = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "str_replace", "path": root.join("ambiguous.txt").to_string_lossy(), "old_str": "same", "new_str": "x" }),
    )
    .await;
    assert!(repeated.is_error);
    assert!(text(&repeated).contains("Multiple occurrences of old_str `same` in lines [1, 3]"));
    assert!(!text(&repeated).contains("replace_all"));

    write_file(&root, "ambiguous.txt", "alpha\nbeta\nmiddle\nalpha\nbeta");
    let repeated_multiline = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "str_replace", "path": root.join("ambiguous.txt").to_string_lossy(), "old_str": "alpha\nbeta", "new_str": "x" }),
    )
    .await;
    assert!(text(&repeated_multiline)
        .contains("Multiple occurrences of old_str `alpha\nbeta` in lines [1, 4]"));

    write_file(&root, "mixed-eol.txt", "alpha\r\nbeta\nmiddle\nalpha\nbeta");
    let mixed = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "str_replace", "path": root.join("mixed-eol.txt").to_string_lossy(), "old_str": "alpha\r\nbeta", "new_str": "replaced" }),
    )
    .await;
    assert!(!mixed.is_error);
    assert_eq!(read_file(&root, "mixed-eol.txt"), "replaced\nmiddle\nalpha\nbeta");

    let relative = call(&world, owner, serde_json::json!({ "command": "view", "path": "ambiguous.txt" })).await;
    assert!(relative.is_error);
    assert!(text(&relative).contains("is not an absolute path"));
    assert_eq!(read_file(&root, "ambiguous.txt"), "alpha\nbeta\nmiddle\nalpha\nbeta");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reports_invalid_commands_or_arguments_without_mutating_files() {
    let root = temp_root("invalid");
    let world = setup(root.clone()).await;
    let owner = world.owner.clone();
    write_file(&root, "ambiguous.txt", "same same");
    write_file(&root, "empty.txt", "");
    write_file(&root, "trailing-newline.txt", "one\n");
    write_file(&root, "three-lines.txt", "one\ntwo\nthree");
    std::fs::create_dir_all(root.join("directory")).expect("dir");

    let cases: Vec<serde_json::Value> = vec![
        serde_json::json!({ "command": "view", "path": "" }),
        serde_json::json!({ "command": "view", "path": root.join("missing.txt").to_string_lossy() }),
        serde_json::json!({ "command": "view", "path": root.join("ambiguous.txt").to_string_lossy(), "view_range": [1] }),
        serde_json::json!({ "command": "view", "path": root.join("ambiguous.txt").to_string_lossy(), "view_range": [0, 1] }),
        serde_json::json!({ "command": "view", "path": root.join("ambiguous.txt").to_string_lossy(), "view_range": [1.5, 2] }),
        serde_json::json!({ "command": "view", "path": root.join("three-lines.txt").to_string_lossy(), "view_range": [1, 99] }),
        serde_json::json!({ "command": "view", "path": root.join("three-lines.txt").to_string_lossy(), "view_range": [2, 1] }),
        serde_json::json!({ "command": "view", "path": root.join("directory").to_string_lossy(), "view_range": [1, 1] }),
        serde_json::json!({ "command": "create", "path": root.join("new.txt").to_string_lossy() }),
        serde_json::json!({ "command": "create", "path": root.join("ambiguous.txt").to_string_lossy(), "file_text": "overwrite" }),
        serde_json::json!({ "command": "str_replace", "path": root.join("ambiguous.txt").to_string_lossy(), "new_str": "x" }),
        serde_json::json!({ "command": "str_replace", "path": root.join("ambiguous.txt").to_string_lossy(), "old_str": "", "new_str": "x" }),
        serde_json::json!({ "command": "insert", "path": root.join("ambiguous.txt").to_string_lossy(), "new_str": "x" }),
        serde_json::json!({ "command": "insert", "path": root.join("ambiguous.txt").to_string_lossy(), "insert_line": -1, "new_str": "x" }),
        serde_json::json!({ "command": "insert", "path": root.join("ambiguous.txt").to_string_lossy(), "insert_line": 1.5, "new_str": "x" }),
        serde_json::json!({ "command": "insert", "path": root.join("ambiguous.txt").to_string_lossy(), "insert_line": 99, "new_str": "x" }),
        serde_json::json!({ "command": "insert", "path": root.join("empty.txt").to_string_lossy(), "insert_line": 2, "new_str": "x" }),
        serde_json::json!({ "command": "insert", "path": root.join("directory").to_string_lossy(), "insert_line": 0, "new_str": "x" }),
    ];
    for case in cases {
        let result = call(&world, owner.clone(), case).await;
        assert!(result.is_error, "{:?}", result.error);
    }
    assert_eq!(read_file(&root, "ambiguous.txt"), "same same");

    // A `stat` reporting a special (non-file, non-directory) entry is
    // rejected for view and every mutation.
    let special_target = futures::executor::block_on(
        world
            .mock
            .inner
            .resolve(&root.join("special").to_string_lossy().into_owned(), None),
    )
    .expect("resolve special");
    let special_stat = FsInfo {
        version: fs_version("special"),
        kind: dsh_fs::FsInfoType::Other,
        size: None,
    };
    *world.mock.stat_override.lock() = Some(Arc::new(move |_target, _signal| {
        let special_stat = special_stat.clone();
        Box::pin(async move { Ok(Some(special_stat)) })
    }));
    let special = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "view", "path": root.join("special").to_string_lossy() }),
    )
    .await;
    assert!(special.is_error);
    assert_eq!(error_code(&special), "FS_NOT_REGULAR_FILE");
    assert_eq!(
        world.mock.inner.contains(
            &world
                .mock
                .inner
                .resolve(&root.to_string_lossy().into_owned(), None)
                .await
                .expect("root target"),
            &special_target,
        ),
        true,
        "special target resolves under the root"
    );
    let replace_special = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "str_replace", "path": root.join("special").to_string_lossy(), "old_str": "x", "new_str": "y" }),
    )
    .await;
    assert_eq!(error_code(&replace_special), "FS_NOT_REGULAR_FILE");
    let insert_special = call(
        &world,
        owner,
        serde_json::json!({ "command": "insert", "path": root.join("special").to_string_lossy(), "insert_line": 0, "new_str": "x" }),
    )
    .await;
    assert_eq!(error_code(&insert_special), "FS_NOT_REGULAR_FILE");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegates_read_before_edit_decisions_to_fs_observation_policy() {
    let root = temp_root("observed");
    let world = setup_with(
        root.clone(),
        SetupOptions { fs_policy: true, ..Default::default() },
    )
    .await;
    let owner = world.owner.clone();
    write_file(&root, "existing.txt", "before");

    let blind_edit = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "str_replace", "path": root.join("existing.txt").to_string_lossy(), "old_str": "before", "new_str": "after" }),
    )
    .await;
    assert!(blind_edit.is_error);
    assert_eq!(error_code(&blind_edit), "FS_NOT_OBSERVED");
    assert_eq!(read_file(&root, "existing.txt"), "before");

    let _ = call(&world, owner.clone(), serde_json::json!({ "command": "view", "path": root.join("existing.txt").to_string_lossy() })).await;
    let after_view = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "str_replace", "path": root.join("existing.txt").to_string_lossy(), "old_str": "before", "new_str": "after" }),
    )
    .await;
    assert!(!after_view.is_error);
    assert_eq!(read_file(&root, "existing.txt"), "after");

    let inserted = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "insert", "path": root.join("existing.txt").to_string_lossy(), "insert_line": 1, "new_str": "tail" }),
    )
    .await;
    assert!(!inserted.is_error);
    assert_eq!(read_file(&root, "existing.txt"), "after\ntail");

    let created = call(
        &world,
        owner,
        serde_json::json!({ "command": "create", "path": root.join("created.txt").to_string_lossy(), "file_text": "new" }),
    )
    .await;
    assert!(!created.is_error);
    assert_eq!(read_file(&root, "created.txt"), "new");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn passes_the_session_sandbox_policy_to_every_mutation() {
    let root = temp_root("sandboxed");
    let world = setup_with(
        root.clone(),
        SetupOptions { sandbox_mode: Some(SandboxMode::ReadOnly), ..Default::default() },
    )
    .await;
    let owner = world.owner.clone();
    let result = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "create", "path": root.join("blocked.txt").to_string_lossy(), "file_text": "blocked" }),
    )
    .await;
    assert!(result.is_error);
    assert_eq!(error_code(&result), "FS_SANDBOX_DENIED");
    assert!(text(&result).contains("[sandbox: file access denied under read-only mode]"), "{}", text(&result));

    let ownerless = call(
        &world,
        None,
        serde_json::json!({ "command": "create", "path": root.join("ownerless-blocked.txt").to_string_lossy(), "file_text": "blocked" }),
    )
    .await;
    assert!(ownerless.is_error);
    assert_eq!(error_code(&ownerless), "FS_SANDBOX_DENIED");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preserves_tabs_outside_the_edited_region() {
    let root = temp_root("tabs");
    let world = setup(root.clone()).await;
    let owner = world.owner.clone();
    write_file(&root, "Makefile", "target:\n\told\nremove\n");

    let view = call(&world, owner.clone(), serde_json::json!({ "command": "view", "path": root.join("Makefile").to_string_lossy() })).await;
    assert!(text(&view).contains("     2  \told"), "{}", text(&view));
    let _ = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "str_replace", "path": root.join("Makefile").to_string_lossy(), "old_str": "\told", "new_str": "\tnew" }),
    )
    .await;
    let _ = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "str_replace", "path": root.join("Makefile").to_string_lossy(), "old_str": "remove\n" }),
    )
    .await;
    let _ = call(
        &world,
        owner,
        serde_json::json!({ "command": "insert", "path": root.join("Makefile").to_string_lossy(), "insert_line": 1, "new_str": "\tkept" }),
    )
    .await;
    assert_eq!(read_file(&root, "Makefile"), "target:\n\tkept\n\tnew\n");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reports_missing_sandbox_policy_composition_during_plugin_startup() {
    let root = temp_root("missing-policy");
    let ctx = Context::root();
    SystemPrompt::install(&ctx, dsh_system_prompt::Config::default()).expect("prompt");
    ToolRuntime::install(&ctx, dsh_tools::Config::default()).expect("tools");
    AgentRegistry::install(&ctx);
    let backend = LocalFileSystem::build(dsh_fs_local::Config {
        cwd: Some(root.to_string_lossy().into_owned()),
        diff_basis_max_bytes: None,
    })
    .expect("fs-local");
    let mock = MockFs::new(backend);
    *mock.confined.lock() = Some(SandboxMode::ReadOnly);
    {
        let erased: Arc<dyn FileSystem> = mock.clone();
        ctx.register_service(erased);
    }
    let fiber = ctx.plugin(Arc::new(ToolStrReplaceEditorPlugin::new()), arc(serde_json::json!({})));
    let error = fiber.settle().await.err().expect("missing policy rejected");
    assert!(
        error.message().contains("the mounted filesystem confines but ctx.sandboxPolicy is missing"),
        "{error}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn maps_unexpected_backend_write_failures_for_replace_and_insert() {
    let root = temp_root("backend-error");
    let world = setup(root.clone()).await;
    let owner = world.owner.clone();
    write_file(&root, "backend-error.txt", "old\n");
    *world.mock.write_override.lock() = Some(Arc::new(|_target, _content, _expected, _signal, _policy| {
        Box::pin(async move {
            Err(dsh_fs::FsError::new(
                "backend write failed",
                dsh_fs::FsErrorCode::FsIoError,
            ))
        })
    }));

    let replace = call(
        &world,
        owner.clone(),
        serde_json::json!({ "command": "str_replace", "path": root.join("backend-error.txt").to_string_lossy(), "old_str": "old", "new_str": "new" }),
    )
    .await;
    assert!(replace.is_error);
    assert!(text(&replace).contains("backend write failed"), "{}", text(&replace));

    let insert = call(
        &world,
        owner,
        serde_json::json!({ "command": "insert", "path": root.join("backend-error.txt").to_string_lossy(), "insert_line": 1, "new_str": "new" }),
    )
    .await;
    assert!(insert.is_error);
    assert!(text(&insert).contains("backend write failed"), "{}", text(&insert));
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_invalid_plugin_config() {
    let error = apply(
        &Context::root(),
        Config { max_output_chars: Some(0), description: None },
    )
    .err()
    .expect("maxOutputChars");
    assert!(
        error.contains("maxOutputChars must be a positive safe integer"),
        "{error}"
    );
    let error = apply(
        &Context::root(),
        Config { max_output_chars: None, description: Some(" ".to_string()) },
    )
    .err()
    .expect("description");
    assert!(error.contains("description must be non-empty"), "{error}");
}
