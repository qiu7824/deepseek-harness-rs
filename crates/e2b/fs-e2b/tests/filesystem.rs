//! Rust port of the core
//! `packages/e2b/fs-e2b/tests/filesystem.spec.ts` behaviors: remote path
//! identity and canonicalization, strict transport framing, whole/streamed
//! UTF-8 reads with the SDK quirks, guarded atomic writes, literal edits
//! with CRLF restoration, and error mapping.
//!
//! # Deviations
//!
//! - The TS stubs `ctx.e2b` with a plain runtime object; the Rust suite
//!   mounts the real `E2bRuntime` over a fake SDK, so sandbox creation
//!   side effects (`.dsh-e2b` mkdir/chmod) are part of the shared remote.
//! - Command-count assertions use relative offsets.
//! - The abort seam is the shared predicate (an `AtomicBool` closure).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

use base64::Engine;
use cordis::{ArcValue, Context, Plugin, PluginError, arc};
use dsh_e2b::{
    Config, E2bBackgroundOptions, E2bCommandHandle, E2bCommandOptions, E2bCommandResult,
    E2bCreateOptions, E2bEntryInfo, E2bReadStream, E2bRuntime, E2bSandbox, E2bSdk, E2bSdkError,
    E2bSdkErrorKind, FileType,
};
use dsh_fs::{
    AbortPredicate, FileSystem, FsEditGuard, FsEditRequest, FsErrorCode, FsTarget, FsWriteIntent,
    LstatOptions, ResolveOptions,
};
use dsh_fs_e2b::E2bFileSystem;
use futures::future::BoxFuture;
use parking_lot::Mutex;

fn never_abort() -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(|| false)
}

#[derive(Clone)]
struct RemoteNode {
    file_type: FileType,
    data: Vec<u8>,
    mode: u32,
    modified: u64,
    metadata: Option<HashMap<String, String>>,
    symlink_target: Option<String>,
}

enum Competitor {
    File { path: String, data: Vec<u8> },
    Directory { path: String },
}

/// A scripted remote sandbox (TS `FakeRemote`).
struct FakeRemote {
    nodes: Mutex<HashMap<String, RemoteNode>>,
    writes: Mutex<Vec<(String, Vec<u8>, Option<HashMap<String, String>>)>>,
    write_parent_modes: Mutex<Vec<u32>>,
    renames: Mutex<Vec<(String, String)>>,
    links: Mutex<Vec<(String, String)>>,
    removals: Mutex<Vec<String>>,
    commands: Mutex<Vec<String>>,
    reads: Mutex<Vec<(String, String)>>,
    stream_chunks: Mutex<Option<Vec<Vec<u8>>>>,
    stream_keep_open: AtomicBool,
    stream_cancels: Mutex<u32>,
    next_command_error: Mutex<VecDeque<E2bSdkError>>,
    next_make_dir_result: Mutex<Option<bool>>,
    next_info_error: Mutex<VecDeque<E2bSdkError>>,
    next_list_error: Mutex<VecDeque<E2bSdkError>>,
    next_read_error: Mutex<VecDeque<E2bSdkError>>,
    next_rename_error: Mutex<VecDeque<E2bSdkError>>,
    next_remove_error: Mutex<VecDeque<E2bSdkError>>,
    canonical_output: Mutex<Option<String>>,
    abort_after_rename: Mutex<Option<Arc<AtomicBool>>>,
    competitor_before_link: Mutex<Option<Competitor>>,
    guarded_link_output: Mutex<Option<String>>,
    disappear_on_info: Mutex<std::collections::HashSet<String>>,
    clock: Mutex<u64>,
}

impl FakeRemote {
    fn new() -> Arc<Self> {
        let remote = Arc::new(Self {
            nodes: Mutex::new(HashMap::new()),
            writes: Mutex::new(Vec::new()),
            write_parent_modes: Mutex::new(Vec::new()),
            renames: Mutex::new(Vec::new()),
            links: Mutex::new(Vec::new()),
            removals: Mutex::new(Vec::new()),
            commands: Mutex::new(Vec::new()),
            reads: Mutex::new(Vec::new()),
            stream_chunks: Mutex::new(None),
            stream_keep_open: AtomicBool::new(false),
            stream_cancels: Mutex::new(0),
            next_command_error: Mutex::new(VecDeque::new()),
            next_make_dir_result: Mutex::new(None),
            next_info_error: Mutex::new(VecDeque::new()),
            next_list_error: Mutex::new(VecDeque::new()),
            next_read_error: Mutex::new(VecDeque::new()),
            next_rename_error: Mutex::new(VecDeque::new()),
            next_remove_error: Mutex::new(VecDeque::new()),
            canonical_output: Mutex::new(None),
            abort_after_rename: Mutex::new(None),
            competitor_before_link: Mutex::new(None),
            guarded_link_output: Mutex::new(None),
            disappear_on_info: Mutex::new(std::collections::HashSet::new()),
            clock: Mutex::new(1),
        });
        remote.dir("/");
        remote.dir("/workspace");
        remote
    }

    fn next_modified(&self) -> u64 {
        let mut clock = self.clock.lock();
        let value = *clock;
        *clock += 1;
        value
    }

    fn dir(&self, path: &str) {
        self.nodes.lock().insert(
            path.to_string(),
            RemoteNode {
                file_type: FileType::Dir,
                data: Vec::new(),
                mode: 0o755,
                modified: self.next_modified(),
                metadata: None,
                symlink_target: None,
            },
        );
    }

    fn file(&self, path: &str, data: &[u8], mode: u32) {
        self.nodes.lock().insert(
            path.to_string(),
            RemoteNode {
                file_type: FileType::File,
                data: data.to_vec(),
                mode,
                modified: self.next_modified(),
                metadata: None,
                symlink_target: None,
            },
        );
    }

    fn symlink(&self, path: &str, target: &str) {
        self.nodes.lock().insert(
            path.to_string(),
            RemoteNode {
                file_type: FileType::File,
                data: Vec::new(),
                mode: 0o777,
                modified: self.next_modified(),
                metadata: None,
                symlink_target: Some(target.to_string()),
            },
        );
    }

    fn mutate(&self, path: &str, data: &[u8]) {
        let mut nodes = self.nodes.lock();
        let node = nodes.get_mut(path).expect("mutate target");
        node.data = data.to_vec();
        node.modified = self.next_modified();
    }

    fn required(&self, path: &str) -> RemoteNode {
        self.nodes
            .lock()
            .get(path)
            .cloned()
            .unwrap_or_else(|| panic!("missing: {path}"))
    }

    fn followed(&self, path: &str) -> (String, RemoteNode) {
        let node = self.required(path);
        match &node.symlink_target {
            None => (path.to_string(), node),
            Some(target) => {
                let target_node = self.required(target);
                (target.clone(), target_node)
            }
        }
    }

    fn raw_info(&self, path: &str) -> E2bEntryInfo {
        let node = self.required(path);
        let (followed_path, followed_node) = self.followed(path);
        E2bEntryInfo {
            name: posix_basename(path),
            path: path.to_string(),
            file_type: followed_node.file_type,
            size: followed_node.data.len() as u64,
            mode: followed_node.mode,
            modified_time_ms: Some(followed_node.modified as i64),
            symlink_target: node.symlink_target.clone(),
            metadata: followed_node.metadata.clone(),
        }
        .tap(|_| {
            let _ = followed_path;
        })
    }
}

trait Tap: Sized {
    fn tap(self, f: impl FnOnce(&Self)) -> Self {
        f(&self);
        self
    }
}

impl<T> Tap for T {}

fn posix_basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    trimmed
        .rfind('/')
        .map(|index| trimmed[index + 1..].to_string())
        .unwrap_or_else(|| trimmed.to_string())
}

fn posix_dirname(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(index) if index > 0 => trimmed[..index].to_string(),
        Some(_) => "/".to_string(),
        None => ".".to_string(),
    }
}

/// A scripted remote read stream (TS `ReadableStream`).
struct FakeReadStream {
    chunks: VecDeque<Vec<u8>>,
    keep_open: bool,
    cancels: Arc<Mutex<u32>>,
}

impl E2bReadStream for FakeReadStream {
    fn read(&mut self) -> BoxFuture<'static, Result<Option<Vec<u8>>, E2bSdkError>> {
        if let Some(chunk) = self.chunks.pop_front() {
            return Box::pin(async move { Ok(Some(chunk)) });
        }
        if self.keep_open {
            return Box::pin(std::future::pending());
        }
        Box::pin(async move { Ok(None) })
    }

    fn cancel(&mut self) -> BoxFuture<'static, ()> {
        *self.cancels.lock() += 1;
        Box::pin(async {})
    }
}

#[async_trait::async_trait]
impl E2bSandbox for FakeRemote {
    fn sandbox_id(&self) -> &str {
        "fake"
    }

    async fn make_dir(&self, path: &str) -> Result<bool, E2bSdkError> {
        if let Some(result) = self.next_make_dir_result.lock().take() {
            return Ok(result);
        }
        if self.nodes.lock().contains_key(path) {
            return Ok(false);
        }
        self.dir(path);
        Ok(true)
    }

    async fn get_info(&self, path: &str) -> Result<E2bEntryInfo, E2bSdkError> {
        if let Some(error) = self.next_info_error.lock().pop_front() {
            return Err(error);
        }
        if self.disappear_on_info.lock().remove(path) {
            return Err(E2bSdkError::not_found(format!("missing: {path}")));
        }
        if !self.nodes.lock().contains_key(path) {
            return Err(E2bSdkError::not_found(format!("missing: {path}")));
        }
        Ok(self.raw_info(path))
    }

    async fn read_bytes(&self, path: &str) -> Result<Vec<u8>, E2bSdkError> {
        self.reads
            .lock()
            .push((path.to_string(), "bytes".to_string()));
        if let Some(error) = self.next_read_error.lock().pop_front() {
            return Err(error);
        }
        Ok(self.followed(path).1.data)
    }

    async fn read_stream(&self, path: &str) -> Result<Box<dyn E2bReadStream>, E2bSdkError> {
        self.reads
            .lock()
            .push((path.to_string(), "stream".to_string()));
        if let Some(error) = self.next_read_error.lock().pop_front() {
            return Err(error);
        }
        let data = self.followed(path).1.data;
        let chunks = match self.stream_chunks.lock().clone() {
            Some(chunks) => chunks,
            None => vec![data],
        };
        Ok(Box::new(FakeReadStream {
            chunks: chunks.into(),
            keep_open: self.stream_keep_open.load(SeqCst),
            cancels: Arc::new(Mutex::new(0)),
        }))
    }

    async fn list(&self, path: &str) -> Result<Vec<E2bEntryInfo>, E2bSdkError> {
        if let Some(error) = self.next_list_error.lock().pop_front() {
            return Err(error);
        }
        self.required(path);
        let candidates: Vec<String> = {
            let nodes = self.nodes.lock();
            nodes
                .keys()
                .filter(|candidate| candidate.as_str() != path && posix_dirname(candidate) == path)
                .cloned()
                .collect()
        };
        let mut entries: Vec<E2bEntryInfo> = candidates
            .iter()
            .map(|candidate| self.raw_info(candidate))
            .collect();
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
    }

    async fn write(
        &self,
        path: &str,
        content: &[u8],
        metadata: Option<HashMap<String, String>>,
    ) -> Result<(), E2bSdkError> {
        let parent = posix_dirname(path);
        {
            let mut nodes = self.nodes.lock();
            if !nodes.contains_key(&parent) {
                let modified = self.next_modified();
                nodes.insert(
                    parent.clone(),
                    RemoteNode {
                        file_type: FileType::Dir,
                        data: Vec::new(),
                        mode: 0o755,
                        modified,
                        metadata: None,
                        symlink_target: None,
                    },
                );
            }
            self.write_parent_modes.lock().push(nodes[&parent].mode);
            nodes.insert(
                path.to_string(),
                RemoteNode {
                    file_type: FileType::File,
                    data: content.to_vec(),
                    mode: 0o644,
                    modified: self.next_modified(),
                    metadata: metadata.clone(),
                    symlink_target: None,
                },
            );
        }
        self.writes
            .lock()
            .push((path.to_string(), content.to_vec(), metadata));
        Ok(())
    }

    async fn rename(&self, from: &str, to: &str) -> Result<E2bEntryInfo, E2bSdkError> {
        if let Some(error) = self.next_rename_error.lock().pop_front() {
            return Err(error);
        }
        let node = self.required(from);
        {
            let mut nodes = self.nodes.lock();
            nodes.remove(from);
            nodes.insert(to.to_string(), node);
        }
        self.renames.lock().push((from.to_string(), to.to_string()));
        if let Some(signal) = self.abort_after_rename.lock().clone() {
            signal.store(true, SeqCst);
        }
        Ok(self.raw_info(to))
    }

    async fn remove(&self, path: &str) -> Result<(), E2bSdkError> {
        self.removals.lock().push(path.to_string());
        if let Some(error) = self.next_remove_error.lock().pop_front() {
            return Err(error);
        }
        self.nodes.lock().retain(|candidate, _| {
            candidate != path && !candidate.starts_with(&format!("{path}/"))
        });
        Ok(())
    }

    async fn run(
        &self,
        command: &str,
        options: &E2bCommandOptions,
    ) -> Result<E2bCommandResult, E2bSdkError> {
        let home = options
            .envs
            .as_ref()
            .and_then(|envs| envs.get("HOME"))
            .cloned();
        assert!(
            home.as_deref()
                .is_some_and(|home| home.starts_with("/.dsh-e2b-control-")),
            "{command}"
        );
        self.commands.lock().push(command.to_string());
        if let Some(error) = self.next_command_error.lock().pop_front() {
            return Err(error);
        }
        let realpath_prefix = "set -o pipefail; realpath -mz -- ";
        let realpath_suffix = " | base64 -w0";
        if command.starts_with(realpath_prefix) && command.ends_with(realpath_suffix) {
            let quoted = &command[realpath_prefix.len()..command.len() - realpath_suffix.len()];
            let input = quoted[1..quoted.len() - 1].replace("'\"'\"'", "'");
            let canonical = {
                let node = self.nodes.lock().get(&input).cloned();
                format!(
                    "{}\0",
                    node.and_then(|node| node.symlink_target).unwrap_or(input)
                )
            };
            let stdout = match self.canonical_output.lock().take() {
                Some(output) => output,
                None => base64::engine::general_purpose::STANDARD.encode(canonical.as_bytes()),
            };
            return Ok(E2bCommandResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            });
        }
        if let Some(rest) = command.strip_prefix("chmod ") {
            if let Some((mode, quoted)) = rest.split_once(" -- '") {
                let mode = u32::from_str_radix(mode, 8).ok();
                let path = quoted.trim_end_matches('\'');
                if let (Some(mode), Some(node)) = (mode, self.nodes.lock().get_mut(path)) {
                    node.mode = mode;
                }
            }
        }
        if let Some(rest) = command.strip_prefix("if ln -T -- '") {
            let (from, rest) = rest.split_once("' '").expect("guarded link quote pair");
            let to = rest
                .split_once('\'')
                .map(|(to, _)| to.to_string())
                .unwrap_or_default();
            if let Some(output) = self.guarded_link_output.lock().take() {
                return Ok(E2bCommandResult {
                    exit_code: 0,
                    stdout: output,
                    stderr: String::new(),
                });
            }
            if let Some(competitor) = self.competitor_before_link.lock().take() {
                match competitor {
                    Competitor::File { path, data } => self.file(&path, &data, 0o644),
                    Competitor::Directory { path } => self.dir(&path),
                }
            }
            if self.nodes.lock().contains_key(&to) {
                return Ok(E2bCommandResult {
                    exit_code: 0,
                    stdout: "exists".to_string(),
                    stderr: String::new(),
                });
            }
            let node = self.required(&from);
            self.nodes.lock().insert(to.clone(), node);
            self.links.lock().push((from.to_string(), to));
            if let Some(signal) = self.abort_after_rename.lock().clone() {
                signal.store(true, SeqCst);
            }
            return Ok(E2bCommandResult {
                exit_code: 0,
                stdout: "created".to_string(),
                stderr: String::new(),
            });
        }
        if let Some(rest) = command.strip_prefix("mv -f -- '") {
            if let Some((from, to)) = rest.split_once("' '") {
                let from = from.to_string();
                let to = to.trim_end_matches('\'').to_string();
                if let Some(error) = self.next_rename_error.lock().pop_front() {
                    return Err(error);
                }
                let node = self.required(&from);
                {
                    let mut nodes = self.nodes.lock();
                    nodes.remove(&from);
                    nodes.insert(to.clone(), node);
                }
                self.renames.lock().push((from, to));
                if let Some(signal) = self.abort_after_rename.lock().clone() {
                    signal.store(true, SeqCst);
                }
            }
        }
        Ok(E2bCommandResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    async fn run_background(
        &self,
        _command: &str,
        _options: &E2bBackgroundOptions,
    ) -> Result<Arc<dyn E2bCommandHandle>, E2bSdkError> {
        Err(E2bSdkError::other(
            "background commands are unsupported in this fake",
        ))
    }

    async fn kill(&self) -> Result<(), E2bSdkError> {
        Ok(())
    }
}

/// A fake SDK serving one scripted remote.
struct FakeSdk {
    remote: Arc<FakeRemote>,
}

impl E2bSdk for FakeSdk {
    fn create(
        &self,
        _options: &E2bCreateOptions,
    ) -> BoxFuture<'static, Result<Arc<dyn E2bSandbox>, E2bSdkError>> {
        let remote = self.remote.clone();
        Box::pin(async move { Ok(remote as Arc<dyn E2bSandbox>) })
    }
}

async fn setup(remote: Arc<FakeRemote>) -> (Context, Arc<E2bFileSystem>) {
    let ctx = Context::root();
    let sdk: Arc<dyn E2bSdk> = Arc::new(FakeSdk {
        remote: remote.clone(),
    });
    let runtime = E2bRuntime::install(
        &ctx,
        sdk,
        Config {
            api_key: Some("test-key".to_string()),
            cwd: Some("/workspace".to_string()),
            timeout_ms: None,
        },
        Arc::new(|_: &str| None),
    )
    .expect("e2b runtime");
    // Pre-open the shared sandbox so creation-window side effects (the
    // runtime root + its chmod) happen before tests inject scripted errors.
    runtime.get_sandbox().await.expect("open");
    let fs = E2bFileSystem::install(&ctx).expect("fs-e2b");
    (ctx, fs)
}

async fn resolve(fs: &Arc<E2bFileSystem>, path: &str) -> FsTarget {
    fs.resolve(
        path,
        Some(&ResolveOptions {
            cwd: None,
            signal: None,
        }),
    )
    .await
    .expect("resolve")
}

async fn expect_code<F, T>(future: F, code: FsErrorCode)
where
    F: std::future::Future<Output = Result<T, dsh_fs::FsError>>,
{
    let error = future.await.err().expect("error");
    assert_eq!(error.code, code, "{}", error);
}

fn abort(signal: &Arc<AtomicBool>) -> AbortPredicate {
    let signal = signal.clone();
    Arc::new(move || signal.load(SeqCst))
}

// ---- identity, metadata, and reads ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolves_remote_paths_reports_symlinks_and_lists_direct_children_in_stable_order() {
    let remote = FakeRemote::new();
    remote.file("/workspace/z.txt", b"z", 0o644);
    remote.file("/workspace/a.txt", b"a", 0o644);
    remote.dir("/workspace/dir");
    remote.symlink("/workspace/link.txt", "/workspace/a.txt");
    let (_ctx, fs) = setup(remote.clone()).await;

    let link = resolve(&fs, "link.txt").await;
    assert_eq!(link.target_key.as_str(), "/workspace/a.txt");
    assert_eq!(link.display_path, "/workspace/link.txt");
    let lstat = fs
        .lstat("link.txt", Some(&LstatOptions { cwd: None }), None)
        .await
        .expect("lstat")
        .expect("present");
    assert_eq!(lstat.kind, dsh_fs::FsPathInfoType::Symlink);
    assert_eq!(lstat.size, Some(1));
    let stat = fs.stat(&link, None).await.expect("stat").expect("present");
    assert_eq!(stat.kind, dsh_fs::FsInfoType::File);
    assert_eq!(stat.size, Some(1));

    let directory = resolve(&fs, ".").await;
    // The Rust suite mounts the real runtime, whose creation window adds
    // the reserved runtime root; the TS suite stubs `ctx.e2b` and never
    // sees it.
    remote.nodes.lock().remove("/workspace/.dsh-e2b");
    let listed = fs.list_dir(&directory, None).await.expect("list");
    assert_eq!(
        listed
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["a.txt", "dir", "link.txt", "z.txt"]
    );
    let link_entry = listed
        .iter()
        .find(|entry| entry.name == "link.txt")
        .expect("link");
    assert_eq!(link_entry.kind, dsh_fs::FsInfoType::File);
    assert_eq!(link_entry.target.target_key.as_str(), "/workspace/a.txt");
    assert_eq!(link_entry.target.display_path, "/workspace/link.txt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projects_canonical_process_paths_file_urls_and_containment() {
    let remote = FakeRemote::new();
    remote.dir("/workspace/nested");
    remote.file("/workspace/nested/multibyte # file.ts", b"text", 0o644);
    remote.file("/outside.ts", b"outside", 0o644);
    let (_ctx, fs) = setup(remote.clone()).await;
    let workspace = resolve(&fs, "/workspace").await;
    let nested = resolve(&fs, "/workspace/nested/multibyte # file.ts").await;
    let outside = resolve(&fs, "/outside.ts").await;

    assert_eq!(
        fs.process_path(&nested),
        "/workspace/nested/multibyte # file.ts"
    );
    assert_eq!(
        fs.file_url(&nested),
        "file:///workspace/nested/multibyte%20%23%20file.ts"
    );
    assert!(fs.contains(&workspace, &workspace));
    assert!(fs.contains(&workspace, &nested));
    assert!(!fs.contains(&nested, &workspace));
    assert!(!fs.contains(&workspace, &outside));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preserves_newline_and_multibyte_canonical_paths_through_strict_ascii_framing() {
    let remote = FakeRemote::new();
    let path = "/workspace/你好\nfile.ts";
    remote.file(path, b"text", 0o644);
    let (_ctx, fs) = setup(remote.clone()).await;
    let target = resolve(&fs, path).await;
    assert_eq!(target.target_key.as_str(), path);
    assert_eq!(target.display_path, path);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_invalid_canonical_path_transport_frames() {
    let frames = [
        "!!!!".to_string(),
        base64::engine::general_purpose::STANDARD.encode(b"/workspace/file"),
        base64::engine::general_purpose::STANDARD.encode(b"/workspace/file\0/other\0"),
        base64::engine::general_purpose::STANDARD.encode([47u8, 0xff, 0]),
        base64::engine::general_purpose::STANDARD.encode(b"workspace/file\0"),
    ];
    for output in frames {
        let remote = FakeRemote::new();
        *remote.canonical_output.lock() = Some(output);
        let (_ctx, fs) = setup(remote.clone()).await;
        expect_code(
            fs.resolve(
                "file",
                Some(&ResolveOptions {
                    cwd: None,
                    signal: None,
                }),
            ),
            FsErrorCode::FsIoError,
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_whole_and_streamed_utf8_across_chunk_boundaries() {
    let remote = FakeRemote::new();
    remote.file("/workspace/text.txt", "A€B".as_bytes(), 0o644);
    *remote.stream_chunks.lock() = Some(vec![vec![65, 0xe2], vec![0x82, 0xac, 66]]);
    let (_ctx, fs) = setup(remote.clone()).await;
    let target = resolve(&fs, "text.txt").await;

    assert_eq!(fs.read_text(&target, None).await.expect("read"), "A€B");
    let mut streamed = String::new();
    let mut stream = fs.stream_text(&target, None).await.expect("stream");
    while let Some(chunk) = futures::StreamExt::next(&mut stream).await {
        streamed.push_str(&chunk.expect("chunk"));
    }
    assert_eq!(streamed, "A€B");

    *remote.stream_chunks.lock() = Some(vec![vec![0xe2], vec![0x82, 0xac]]);
    let mut buffered = String::new();
    let mut stream = fs.stream_text(&target, None).await.expect("stream");
    while let Some(chunk) = futures::StreamExt::next(&mut stream).await {
        buffered.push_str(&chunk.expect("chunk"));
    }
    assert_eq!(buffered, "€");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streams_an_empty_file_through_the_sdk_quirk() {
    let remote = FakeRemote::new();
    remote.file("/workspace/empty.txt", b"", 0o644);
    let (_ctx, fs) = setup(remote.clone()).await;
    let target = resolve(&fs, "empty.txt").await;
    let mut streamed = String::new();
    let mut stream = fs.stream_text(&target, None).await.expect("stream");
    while let Some(chunk) = futures::StreamExt::next(&mut stream).await {
        streamed.push_str(&chunk.expect("chunk"));
    }
    assert_eq!(streamed, "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn maps_binary_invalid_missing_and_non_regular_read_failures() {
    let remote = FakeRemote::new();
    remote.file("/workspace/binary", &[0, 1], 0o644);
    remote.file("/workspace/invalid", &[0xff], 0o644);
    remote.dir("/workspace/directory");
    let (_ctx, fs) = setup(remote.clone()).await;
    expect_code(
        fs.read_text(&resolve(&fs, "binary").await, None),
        FsErrorCode::FsNotText,
    )
    .await;
    expect_code(
        fs.read_text(&resolve(&fs, "invalid").await, None),
        FsErrorCode::FsNotText,
    )
    .await;
    expect_code(
        fs.read_text(&resolve(&fs, "missing").await, None),
        FsErrorCode::FsNotFound,
    )
    .await;
    expect_code(
        fs.read_text(&resolve(&fs, "directory").await, None),
        FsErrorCode::FsNotRegularFile,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_bytes_returns_raw_content_enforces_the_cap_and_maps_failures() {
    let remote = FakeRemote::new();
    remote.file("/workspace/img.bin", &[0x89, 0, 0xff, 0x47], 0o644);
    remote.dir("/workspace/directory");
    let (_ctx, fs) = setup(remote.clone()).await;
    let target = resolve(&fs, "img.bin").await;
    assert_eq!(
        fs.read_bytes(&target, None, 4).await.expect("bytes"),
        vec![0x89, 0, 0xff, 0x47]
    );
    expect_code(fs.read_bytes(&target, None, 3), FsErrorCode::FsTooLarge).await;
    expect_code(
        fs.read_bytes(&resolve(&fs, "missing").await, None, 4),
        FsErrorCode::FsNotFound,
    )
    .await;
    expect_code(
        fs.read_bytes(&resolve(&fs, "directory").await, None, 4),
        FsErrorCode::FsNotRegularFile,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn honors_aborts_before_remote_reads() {
    let remote = FakeRemote::new();
    remote.file("/workspace/a", b"a", 0o644);
    let (_ctx, fs) = setup(remote.clone()).await;
    let aborted = Arc::new(AtomicBool::new(true));
    expect_code(
        fs.resolve(
            "a",
            Some(&ResolveOptions {
                cwd: None,
                signal: Some(abort(&aborted)),
            }),
        ),
        FsErrorCode::FsAborted,
    )
    .await;
    expect_code(
        fs.stat(&resolve(&fs, "a").await, Some(abort(&aborted))),
        FsErrorCode::FsAborted,
    )
    .await;
}

// ---- atomic writes and edits ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn creates_owner_only_files_with_metadata_after_the_committed_move() {
    let remote = FakeRemote::new();
    let (_ctx, fs) = setup(remote.clone()).await;
    let target = resolve(&fs, "new.txt").await;
    let outcome = fs
        .write_text(
            &target,
            "one\r\ntwo\rthree",
            Some(&FsWriteIntent::CreateIfAbsent),
            None,
            None,
        )
        .await
        .expect("write");
    assert_eq!(outcome.operation, dsh_fs::FsWriteOperation::Create);
    assert_eq!(outcome.before, None);
    assert_eq!(outcome.after, "one\ntwo\rthree");
    let node = remote.required("/workspace/new.txt");
    assert_eq!(node.mode, 0o600);
    assert!(
        node.metadata
            .as_ref()
            .and_then(|m| m.get("dsh-version"))
            .is_some()
    );
    assert_eq!(remote.links.lock().len(), 1);
    let staging = posix_dirname(&remote.writes.lock()[0].0);
    assert_eq!(posix_dirname(&staging), "/workspace");
    assert!(remote.removals.lock().contains(&staging));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preserves_replacement_mode_normalizes_crlf_for_diffs_and_changes_version() {
    let remote = FakeRemote::new();
    remote.file("/workspace/file.txt", b"old\r\nline\rlone", 0o640);
    let (_ctx, fs) = setup(remote.clone()).await;
    let target = resolve(&fs, "file.txt").await;
    let before = fs
        .stat(&target, None)
        .await
        .expect("stat")
        .expect("present")
        .version;
    let outcome = fs
        .write_text(
            &target,
            "new",
            Some(&FsWriteIntent::ReplaceIfVersion { version: before }),
            None,
            None,
        )
        .await
        .expect("write");
    assert_eq!(outcome.operation, dsh_fs::FsWriteOperation::Update);
    assert_eq!(outcome.before.as_deref(), Some("old\nline\rlone"));
    assert_eq!(outcome.after, "new");
    assert_eq!(remote.required("/workspace/file.txt").mode, 0o640);
    let committed = outcome.version;
    remote.mutate("/workspace/file.txt", b"external");
    let version = fs
        .stat(&target, None)
        .await
        .expect("stat")
        .expect("present")
        .version;
    assert_ne!(version, committed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enforces_create_and_version_intents_before_publication() {
    let remote = FakeRemote::new();
    remote.file("/workspace/file.txt", b"v1", 0o644);
    remote.dir("/workspace/dir");
    let (_ctx, fs) = setup(remote.clone()).await;
    let target = resolve(&fs, "file.txt").await;
    let version = fs
        .stat(&target, None)
        .await
        .expect("stat")
        .expect("present")
        .version;
    expect_code(
        fs.write_text(
            &target,
            "blind",
            Some(&FsWriteIntent::CreateIfAbsent),
            None,
            None,
        ),
        FsErrorCode::FsNotObserved,
    )
    .await;
    remote.mutate("/workspace/file.txt", b"v2");
    expect_code(
        fs.write_text(
            &target,
            "stale",
            Some(&FsWriteIntent::ReplaceIfVersion {
                version: version.clone(),
            }),
            None,
            None,
        ),
        FsErrorCode::FsStaleVersion,
    )
    .await;
    let missing = resolve(&fs, "missing").await;
    expect_code(
        fs.write_text(
            &missing,
            "stale",
            Some(&FsWriteIntent::ReplaceIfVersion { version }),
            None,
            None,
        ),
        FsErrorCode::FsStaleVersion,
    )
    .await;
    expect_code(
        fs.write_text(&resolve(&fs, "dir").await, "x", None, None, None),
        FsErrorCode::FsNotRegularFile,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preserves_a_competitor_created_after_the_guarded_create_probe() {
    let remote = FakeRemote::new();
    *remote.competitor_before_link.lock() = Some(Competitor::File {
        path: "/workspace/race.txt".to_string(),
        data: b"competitor".to_vec(),
    });
    let (_ctx, fs) = setup(remote.clone()).await;
    expect_code(
        fs.write_text(
            &resolve(&fs, "race.txt").await,
            "ours",
            Some(&FsWriteIntent::CreateIfAbsent),
            None,
            None,
        ),
        FsErrorCode::FsNotObserved,
    )
    .await;
    assert_eq!(remote.required("/workspace/race.txt").data, b"competitor");
    assert!(remote.links.lock().is_empty());
    assert_eq!(remote.removals.lock().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn applies_literal_edits_atomically_and_restores_the_detected_crlf_style() {
    let remote = FakeRemote::new();
    remote.file("/workspace/file.txt", b"one\r\ntwo\r\nthree\n", 0o644);
    let (_ctx, fs) = setup(remote.clone()).await;
    let target = resolve(&fs, "file.txt").await;
    let version = fs
        .stat(&target, None)
        .await
        .expect("stat")
        .expect("present")
        .version;
    let outcome = fs
        .edit_text(
            &target,
            &FsEditRequest {
                old_string: "two\r\n".to_string(),
                new_string: "TWO\r\n".to_string(),
                replace_all: false,
            },
            Some(&FsEditGuard { version }),
            None,
            None,
        )
        .await
        .expect("edit");
    assert_eq!(outcome.before, "one\ntwo\nthree\n");
    assert_eq!(outcome.after, "one\nTWO\nthree\n");
    assert_eq!(
        String::from_utf8_lossy(&remote.required("/workspace/file.txt").data),
        "one\r\nTWO\r\nthree\r\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reports_stale_and_literal_match_failures_with_stable_codes() {
    let remote = FakeRemote::new();
    remote.file("/workspace/file.txt", b"a a", 0o644);
    remote.dir("/workspace/dir");
    let (_ctx, fs) = setup(remote.clone()).await;
    let target = resolve(&fs, "file.txt").await;
    for request in [
        FsEditRequest {
            old_string: String::new(),
            new_string: "x".to_string(),
            replace_all: false,
        },
        FsEditRequest {
            old_string: "z".to_string(),
            new_string: "x".to_string(),
            replace_all: false,
        },
    ] {
        expect_code(
            fs.edit_text(&target, &request, None, None, None),
            FsErrorCode::FsEditNotFound,
        )
        .await;
    }
    expect_code(
        fs.edit_text(
            &target,
            &FsEditRequest {
                old_string: "a".to_string(),
                new_string: "x".to_string(),
                replace_all: false,
            },
            None,
            None,
            None,
        ),
        FsErrorCode::FsAmbiguousEdit,
    )
    .await;
    let outcome = fs
        .edit_text(
            &target,
            &FsEditRequest {
                old_string: "a".to_string(),
                new_string: "x".to_string(),
                replace_all: true,
            },
            None,
            None,
            None,
        )
        .await
        .expect("replace all");
    assert_eq!(outcome.after, "x x");
    expect_code(
        fs.edit_text(
            &target,
            &FsEditRequest {
                old_string: "x".to_string(),
                new_string: "y".to_string(),
                replace_all: false,
            },
            Some(&FsEditGuard {
                version: dsh_fs::fs_version("stale"),
            }),
            None,
            None,
        ),
        FsErrorCode::FsStaleVersion,
    )
    .await;
    expect_code(
        fs.edit_text(
            &resolve(&fs, "missing").await,
            &FsEditRequest {
                old_string: "x".to_string(),
                new_string: "y".to_string(),
                replace_all: false,
            },
            None,
            None,
            None,
        ),
        FsErrorCode::FsStaleVersion,
    )
    .await;
    expect_code(
        fs.edit_text(
            &resolve(&fs, "dir").await,
            &FsEditRequest {
                old_string: "x".to_string(),
                new_string: "y".to_string(),
                replace_all: false,
            },
            None,
            None,
            None,
        ),
        FsErrorCode::FsNotRegularFile,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serializes_guarded_mutations_so_only_one_stale_version_can_win() {
    let remote = FakeRemote::new();
    remote.file("/workspace/file.txt", b"base", 0o644);
    let (_ctx, fs) = setup(remote.clone()).await;
    let target = resolve(&fs, "file.txt").await;
    let version = fs
        .stat(&target, None)
        .await
        .expect("stat")
        .expect("present")
        .version;
    let (write_fs, edit_fs) = (fs.clone(), fs.clone());
    let (write_target, edit_target) = (target.clone(), target.clone());
    let write_version = version.clone();
    let write = tokio::spawn(async move {
        write_fs
            .write_text(
                &write_target,
                "one",
                Some(&FsWriteIntent::ReplaceIfVersion {
                    version: write_version,
                }),
                None,
                None,
            )
            .await
    });
    let edit = tokio::spawn(async move {
        edit_fs
            .edit_text(
                &edit_target,
                &FsEditRequest {
                    old_string: "base".to_string(),
                    new_string: "two".to_string(),
                    replace_all: false,
                },
                Some(&FsEditGuard { version }),
                None,
                None,
            )
            .await
    });
    let (write_result, edit_result) = (write.await.expect("write"), edit.await.expect("edit"));
    let fulfilled = [write_result.is_ok(), edit_result.is_ok()]
        .iter()
        .filter(|ok| **ok)
        .count();
    assert_eq!(fulfilled, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cleans_staging_files_and_maps_command_and_permission_failures() {
    let remote = FakeRemote::new();
    let (_ctx, fs) = setup(remote.clone()).await;
    let command_target = resolve(&fs, "command").await;
    remote
        .next_command_error
        .lock()
        .push_back(E2bSdkError::command_exit(1, "chmod failed"));
    expect_code(
        fs.write_text(&command_target, "x", None, None, None),
        FsErrorCode::FsIoError,
    )
    .await;
    assert_eq!(remote.removals.lock().len(), 1);

    remote
        .next_rename_error
        .lock()
        .push_back(E2bSdkError::other("permission denied"));
    expect_code(
        fs.write_text(&resolve(&fs, "permission").await, "x", None, None, None),
        FsErrorCode::FsPermissionDenied,
    )
    .await;

    let removals_before = remote.removals.lock().len();
    *remote.next_make_dir_result.lock() = Some(false);
    expect_code(
        fs.write_text(&resolve(&fs, "collision").await, "x", None, None, None),
        FsErrorCode::FsIoError,
    )
    .await;
    assert_eq!(remote.removals.lock().len(), removals_before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn maps_canonicalization_and_provider_failures() {
    let remote = FakeRemote::new();
    let (_ctx, fs) = setup(remote.clone()).await;
    remote
        .next_command_error
        .lock()
        .push_back(E2bSdkError::command_exit(1, "not a directory"));
    expect_code(
        fs.resolve(
            "bad",
            Some(&ResolveOptions {
                cwd: None,
                signal: None,
            }),
        ),
        FsErrorCode::FsIoError,
    )
    .await;
    remote
        .next_command_error
        .lock()
        .push_back(E2bSdkError::other("canonical transport failed"));
    expect_code(
        fs.resolve(
            "bad-transport",
            Some(&ResolveOptions {
                cwd: None,
                signal: None,
            }),
        ),
        FsErrorCode::FsIoError,
    )
    .await;
    remote.file("/workspace/a", b"a", 0o644);
    let target = resolve(&fs, "a").await;
    remote
        .next_info_error
        .lock()
        .push_back(E2bSdkError::other("metadata transport failed"));
    expect_code(fs.stat(&target, None), FsErrorCode::FsIoError).await;
    remote
        .next_read_error
        .lock()
        .push_back(E2bSdkError::other("operation not permitted"));
    expect_code(fs.read_text(&target, None), FsErrorCode::FsPermissionDenied).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uses_listing_metadata_directly_and_canonicalizes_only_symbolic_links() {
    let remote = FakeRemote::new();
    remote.file("/workspace/a", b"a", 0o644);
    remote.file("/workspace/target", b"target", 0o644);
    remote.file("/workspace/gone", b"gone", 0o644);
    remote.symlink("/workspace/link", "/workspace/target");
    remote.symlink("/workspace/vanished-link", "/workspace/gone");
    remote
        .disappear_on_info
        .lock()
        .insert("/workspace/gone".to_string());
    let (_ctx, fs) = setup(remote.clone()).await;
    let directory = resolve(&fs, "/workspace").await;

    let listed = fs.list_dir(&directory, None).await.expect("list");
    let a = listed.iter().find(|entry| entry.name == "a").expect("a");
    assert_eq!(a.kind, dsh_fs::FsInfoType::File);
    assert_eq!(a.target.target_key.as_str(), "/workspace/a");
    assert_eq!(a.size, Some(1));
    let link = listed
        .iter()
        .find(|entry| entry.name == "link")
        .expect("link");
    assert_eq!(link.kind, dsh_fs::FsInfoType::File);
    assert_eq!(link.target.target_key.as_str(), "/workspace/target");
    assert_eq!(link.size, Some(6));
    let vanished = listed
        .iter()
        .find(|entry| entry.name == "vanished-link")
        .expect("vanished");
    assert_eq!(vanished.kind, dsh_fs::FsInfoType::Other);
    assert_eq!(vanished.target.target_key.as_str(), "/workspace/gone");
    assert_eq!(vanished.target.display_path, "/workspace/vanished-link");
}
