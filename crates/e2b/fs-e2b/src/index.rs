//! E2B provider for the filesystem capability seam. Paths, contents, and
//! atomic staging files remain inside the shared remote sandbox. Rust port
//! of `packages/e2b/fs-e2b/src/index.ts`.
//!
//! # Deviations
//!
//! - The per-call sandbox policy argument is ignored (the remote sandbox
//!   boundary is the confinement).
//! - `stream_text` returns a boxed stream of decoded strings with a
//!   hand-rolled incremental UTF-8 decoder (the TS `TextDecoder`
//!   `{ stream: true }` semantics).
//! - Abort predicates are polled at operation boundaries (the repo-wide
//!   `AbortSignal` collapse).

use std::collections::HashMap;
use std::sync::Arc;

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, Service, arc};
use dsh_e2b::{
    E2bCommandOptions, E2bEntryInfo, E2bReadStream, E2bRuntime, E2bSandbox, E2bSdkError,
    E2bSdkErrorKind, FileType, e2b_control_envs, quote_e2b_shell_arg,
};
use dsh_fs::{
    AbortPredicate, FileSystem, FsDirEntry, FsEditGuard, FsEditOutcome, FsEditRequest, FsError,
    FsErrorCode, FsInfo, FsInfoType, FsPathInfo, FsPathInfoType, FsTarget, FsTargetKey, FsVersion,
    FsWriteIntent, FsWriteOutcome, LstatOptions, ResolveOptions, fs_target_key, fs_version,
};
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

/// Cordis plugin name (TS `name`).
pub const FS_E2B_NAME: &str = "fs-e2b";

/// Services required before the plugin can mount the backend.
pub const FS_E2B_INJECT: [&str; 1] = ["e2b"];

const VERSION_METADATA_KEY: &str = "dsh-version";
const BINARY_SAMPLE_BYTES: usize = 8192;

fn assert_not_aborted(signal: Option<&AbortPredicate>, operation: &str) -> Result<(), FsError> {
    if signal.is_some_and(|signal| signal()) {
        return Err(FsError::new(
            format!("{operation} aborted"),
            FsErrorCode::FsAborted,
        ));
    }
    Ok(())
}

fn normalize_line_endings(value: &str) -> String {
    value.replace("\r\n", "\n")
}

fn detects_crlf(value: &str) -> bool {
    let sample: String = value.chars().take(4096).collect();
    let crlf = sample.matches("\r\n").count();
    let lf = sample.matches('\n').count() - crlf;
    crlf > lf
}

fn restore_line_endings(value: &str, crlf: bool) -> String {
    if crlf {
        normalize_line_endings(value).replace('\n', "\r\n")
    } else {
        value.to_string()
    }
}

fn decode_text(bytes: &[u8], display_path: &str) -> Result<String, FsError> {
    if bytes[..bytes.len().min(BINARY_SAMPLE_BYTES)].contains(&0) {
        return Err(FsError::new(
            format!("cannot read \"{display_path}\": binary file"),
            FsErrorCode::FsNotText,
        ));
    }
    std::str::from_utf8(bytes)
        .map(|text| text.to_string())
        .map_err(|_| {
            FsError::new(
                format!("cannot read \"{display_path}\": invalid UTF-8 text"),
                FsErrorCode::FsNotText,
            )
        })
}

fn is_base64(encoded: &str) -> bool {
    if encoded.is_empty() || encoded.len() % 4 != 0 {
        return false;
    }
    encoded
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/' || byte == b'=')
        && encoded[..encoded.len() - 2]
            .bytes()
            .all(|byte| byte != b'=')
}

fn decode_canonical_path(encoded: &str) -> Result<String, String> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    if encoded.is_empty() || !is_base64(encoded) {
        return Err("fs-e2b: canonical path transport returned invalid base64".to_string());
    }
    let framed = STANDARD
        .decode(encoded)
        .map_err(|_| "fs-e2b: canonical path transport returned invalid base64".to_string())?;
    if STANDARD.encode(&framed) != encoded
        || framed.len() < 2
        || *framed.last().expect("length checked") != 0
        || framed[..framed.len() - 1].contains(&0)
    {
        return Err("fs-e2b: canonical path transport returned invalid NUL framing".to_string());
    }
    let path = std::str::from_utf8(&framed[..framed.len() - 1])
        .map_err(|_| "fs-e2b: canonical path is not valid UTF-8".to_string())?;
    if !path.starts_with('/') {
        return Err("fs-e2b: canonical path is not absolute".to_string());
    }
    Ok(path.to_string())
}

fn entry_type(entry: &E2bEntryInfo) -> FsInfoType {
    match entry.file_type {
        FileType::File => FsInfoType::File,
        FileType::Dir => FsInfoType::Directory,
        FileType::Other => FsInfoType::Other,
    }
}

fn entry_version(entry: &E2bEntryInfo) -> FsVersion {
    let facts = serde_json::to_string(&serde_json::json!([
        entry
            .metadata
            .as_ref()
            .and_then(|m| m.get(VERSION_METADATA_KEY)),
        entry.path,
        match entry.file_type {
            FileType::File => "file",
            FileType::Dir => "dir",
            FileType::Other => "other",
        },
        entry.size,
        entry.mode,
        entry.modified_time_ms,
        entry.symlink_target,
    ]))
    .expect("facts json");
    let digest = Sha256::digest(facts.as_bytes());
    fs_version(format!("e2b:{}", hex_encode(&digest)))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn map_error(error: E2bSdkError, operation: &str, display_path: &str) -> FsError {
    match error.kind {
        E2bSdkErrorKind::NotFound => FsError::new(
            format!("cannot {operation} \"{display_path}\": not found"),
            FsErrorCode::FsNotFound,
        ),
        E2bSdkErrorKind::CommandExit { .. } => FsError::new(
            format!(
                "cannot {operation} \"{display_path}\": {}",
                error.stderr.as_deref().unwrap_or(&error.message)
            ),
            FsErrorCode::FsIoError,
        ),
        E2bSdkErrorKind::Other if error.message.eq_ignore_ascii_case("aborted") => {
            FsError::new(format!("{operation} aborted"), FsErrorCode::FsAborted)
        }
        E2bSdkErrorKind::Other
            if error.message.to_lowercase().contains("permission denied")
                || error
                    .message
                    .to_lowercase()
                    .contains("operation not permitted") =>
        {
            FsError::new(
                format!("cannot {operation} \"{display_path}\": permission denied"),
                FsErrorCode::FsPermissionDenied,
            )
        }
        E2bSdkErrorKind::Other => FsError::new(
            format!("cannot {operation} \"{display_path}\": {}", error.message),
            FsErrorCode::FsIoError,
        ),
    }
}

fn literal_edit(
    content: &str,
    request: &FsEditRequest,
    display_path: &str,
) -> Result<String, FsError> {
    let old_string = normalize_line_endings(&request.old_string);
    let new_string = normalize_line_endings(&request.new_string);
    if old_string.is_empty() {
        return Err(FsError::new(
            format!("cannot edit \"{display_path}\": old_string must be non-empty"),
            FsErrorCode::FsEditNotFound,
        ));
    }
    let mut matches = 0usize;
    let mut offset = 0usize;
    loop {
        let Some(found) = content[offset..].find(&old_string) else {
            break;
        };
        matches += 1;
        offset += found + old_string.len();
    }
    if matches == 0 {
        return Err(FsError::new(
            format!("cannot edit \"{display_path}\": old_string was not found"),
            FsErrorCode::FsEditNotFound,
        ));
    }
    if !request.replace_all && matches != 1 {
        return Err(FsError::new(
            format!("cannot edit \"{display_path}\": old_string matched {matches} times"),
            FsErrorCode::FsAmbiguousEdit,
        ));
    }
    if request.replace_all {
        Ok(content
            .split(&old_string)
            .collect::<Vec<_>>()
            .join(&new_string))
    } else {
        Ok(content.replacen(&old_string, &new_string, 1))
    }
}

/// POSIX `path.resolve` (lexical normalization only; the remote canonical
/// round-trip resolves symlinks).
fn posix_resolve(base: &str, path: &str) -> String {
    let joined = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("{base}/{path}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

fn posix_dirname(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(index) if index > 0 => trimmed[..index].to_string(),
        Some(_) => "/".to_string(),
        None => ".".to_string(),
    }
}

fn posix_basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    trimmed
        .rfind('/')
        .map(|index| trimmed[index + 1..].to_string())
        .unwrap_or_else(|| trimmed.to_string())
}

fn posix_relative(parent: &str, child: &str) -> Option<String> {
    let parent_parts: Vec<&str> = parent.split('/').filter(|part| !part.is_empty()).collect();
    let child_parts: Vec<&str> = child.split('/').filter(|part| !part.is_empty()).collect();
    let mut common = 0;
    while common < parent_parts.len()
        && common < child_parts.len()
        && parent_parts[common] == child_parts[common]
    {
        common += 1;
    }
    if common == parent_parts.len() {
        Some(child_parts[common..].join("/"))
    } else {
        None
    }
}

/// Remote filesystem backend sharing the sandbox owned by `ctx.e2b`.
pub struct E2bFileSystem {
    ctx: Context,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Service for E2bFileSystem {
    fn service_name(&self) -> &'static str {
        "fs"
    }
}

impl E2bFileSystem {
    /// Construct, register as `ctx.fs`, and validate the `e2b` owner (TS
    /// constructor collapse).
    pub fn install(ctx: &Context) -> Result<Arc<Self>, String> {
        if ctx.get_typed::<Arc<E2bRuntime>>("e2b", false).is_none() {
            return Err("dsh-fs-e2b requires the e2b service".to_string());
        }
        let backend = Arc::new(Self {
            ctx: ctx.clone(),
            locks: Mutex::new(HashMap::new()),
        });
        let erased: Arc<dyn FileSystem> = backend.clone();
        ctx.register_service(erased);
        Ok(backend)
    }

    fn runtime(&self) -> Arc<E2bRuntime> {
        self.ctx
            .get_typed::<Arc<E2bRuntime>>("e2b", false)
            .map(|slot| slot.as_ref().clone())
            .expect("e2b service")
    }

    async fn with_lock<T, F>(&self, target_key: &str, operation: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        let lock = self
            .locks
            .lock()
            .entry(target_key.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _guard = lock.lock_owned().await;
        operation.await
    }

    async fn canonical_path(
        &self,
        sandbox: &Arc<dyn E2bSandbox>,
        path: &str,
        signal: Option<&AbortPredicate>,
    ) -> Result<String, FsError> {
        let result = sandbox
            .run(
                &format!(
                    "set -o pipefail; realpath -mz -- {} | base64 -w0",
                    quote_e2b_shell_arg(path)
                ),
                &E2bCommandOptions::with_envs(e2b_control_envs(HashMap::new())),
            )
            .await
            .map_err(|error| map_error(error, "resolve", path))?;
        let decoded = decode_canonical_path(&result.stdout)
            .map_err(|message| FsError::new(message, FsErrorCode::FsIoError))?;
        if let Some(signal) = signal {
            if signal() {
                return Err(FsError::new("resolve aborted", FsErrorCode::FsAborted));
            }
        }
        Ok(decoded)
    }

    async fn probe(
        &self,
        path: &str,
        display_path: &str,
        signal: Option<&AbortPredicate>,
    ) -> Result<Option<E2bEntryInfo>, FsError> {
        assert_not_aborted(signal, "stat")?;
        let sandbox = self.runtime().get_sandbox().await.map_err(|message| {
            FsError::new(
                format!("cannot stat \"{display_path}\": {message}"),
                FsErrorCode::FsIoError,
            )
        })?;
        let entry = match sandbox.get_info(path).await {
            Ok(entry) => entry,
            Err(error) if error.is_not_found() => return Ok(None),
            Err(error) => return Err(map_error(error, "stat", display_path)),
        };
        assert_not_aborted(signal, "stat")?;
        Ok(Some(entry))
    }

    async fn require_regular(
        &self,
        target: &FsTarget,
        signal: Option<&AbortPredicate>,
    ) -> Result<FsInfo, FsError> {
        let info = self.stat(target, signal.cloned()).await?.ok_or_else(|| {
            FsError::new(
                format!("cannot read \"{}\": not found", target.display_path),
                FsErrorCode::FsNotFound,
            )
        })?;
        if info.kind != FsInfoType::File {
            return Err(FsError::new(
                format!(
                    "cannot read \"{}\": not a regular file",
                    target.display_path
                ),
                FsErrorCode::FsNotRegularFile,
            ));
        }
        Ok(info)
    }

    fn check_write_intent(
        &self,
        existing: Option<&E2bEntryInfo>,
        expected: Option<&FsWriteIntent>,
        target: &FsTarget,
    ) -> Result<(), FsError> {
        match expected {
            Some(FsWriteIntent::CreateIfAbsent) if existing.is_some() => Err(FsError::new(
                format!(
                    "cannot overwrite existing \"{}\" without reading it first",
                    target.display_path
                ),
                FsErrorCode::FsNotObserved,
            )),
            Some(FsWriteIntent::ReplaceIfVersion { version }) => {
                if existing.is_none_or(|entry| entry_version(entry) != *version) {
                    return Err(FsError::new(
                        format!(
                            "cannot write \"{}\": file changed since it was read",
                            target.display_path
                        ),
                        FsErrorCode::FsStaleVersion,
                    ));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    async fn read_for_diff(
        &self,
        target: &FsTarget,
        signal: Option<&AbortPredicate>,
    ) -> Result<Option<String>, FsError> {
        let sandbox = self.runtime().get_sandbox().await.map_err(|message| {
            FsError::new(
                format!("cannot read \"{}\": {message}", target.display_path),
                FsErrorCode::FsIoError,
            )
        })?;
        let bytes = sandbox
            .read_bytes(target.target_key.as_str())
            .await
            .map_err(|error| map_error(error, "read", &target.display_path))?;
        assert_not_aborted(signal, "read")?;
        match decode_text(&bytes, &target.display_path) {
            Ok(text) => Ok(Some(normalize_line_endings(&text))),
            Err(error) if error.code == FsErrorCode::FsNotText => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn read_for_edit(
        &self,
        target: &FsTarget,
        signal: Option<&AbortPredicate>,
    ) -> Result<String, FsError> {
        let sandbox = self.runtime().get_sandbox().await.map_err(|message| {
            FsError::new(
                format!("cannot edit \"{}\": {message}", target.display_path),
                FsErrorCode::FsIoError,
            )
        })?;
        let bytes = sandbox
            .read_bytes(target.target_key.as_str())
            .await
            .map_err(|error| map_error(error, "edit", &target.display_path))?;
        assert_not_aborted(signal, "edit")?;
        decode_text(&bytes, &target.display_path)
    }

    #[allow(clippy::too_many_arguments)]
    async fn write_atomic(
        &self,
        target: &FsTarget,
        content: &str,
        existing: Option<&E2bEntryInfo>,
        create_if_absent: bool,
        signal: Option<&AbortPredicate>,
    ) -> Result<FsVersion, FsError> {
        assert_not_aborted(signal, "write")?;
        let sandbox = self.runtime().get_sandbox().await.map_err(|message| {
            FsError::new(
                format!("cannot write \"{}\": {message}", target.display_path),
                FsErrorCode::FsIoError,
            )
        })?;
        let target_path = target.target_key.as_str();
        let version_id = uuid::Uuid::new_v4().to_string();
        let staging_directory = format!(
            "{}/.dsh-{}.tmp",
            posix_dirname(target_path),
            uuid::Uuid::new_v4()
        );
        let temporary = format!("{staging_directory}/content");
        let mut staging_created = false;
        let result = async {
            let created = sandbox
                .make_dir(&staging_directory)
                .await
                .map_err(|error| map_error(error, "write", &target.display_path))?;
            if !created {
                return Err(FsError::new(
                    "private staging directory already exists",
                    FsErrorCode::FsIoError,
                ));
            }
            staging_created = true;
            sandbox
                .run(
                    &format!("chmod 700 -- {}", quote_e2b_shell_arg(&staging_directory)),
                    &E2bCommandOptions::with_envs(e2b_control_envs(HashMap::new())),
                )
                .await
                .map_err(|error| map_error(error, "write", &target.display_path))?;
            assert_not_aborted(signal, "write")?;
            let mut metadata = HashMap::new();
            metadata.insert(VERSION_METADATA_KEY.to_string(), version_id.clone());
            sandbox
                .write(&temporary, content.as_bytes(), Some(metadata))
                .await
                .map_err(|error| map_error(error, "write", &target.display_path))?;
            assert_not_aborted(signal, "write")?;
            let mode = match existing {
                None => 0o600u32,
                Some(entry) => entry.mode & 0o777,
            };
            sandbox
                .run(
                    &format!(
                        "chmod {:o} -- {}",
                        mode,
                        quote_e2b_shell_arg(&temporary)
                    ),
                    &E2bCommandOptions::with_envs(e2b_control_envs(HashMap::new())),
                )
                .await
                .map_err(|error| map_error(error, "write", &target.display_path))?;
            assert_not_aborted(signal, "write")?;
            let committed = if create_if_absent {
                let staged = sandbox
                    .get_info(&temporary)
                    .await
                    .map_err(|error| map_error(error, "write", &target.display_path))?;
                assert_not_aborted(signal, "write")?;
                let target_arg = quote_e2b_shell_arg(target_path);
                let publication = sandbox
                    .run(
                        &format!(
                            "if ln -T -- {} {target_arg}; then printf created; elif test -e {target_arg} || test -L {target_arg}; then printf exists; else exit 1; fi",
                            quote_e2b_shell_arg(&temporary)
                        ),
                        &E2bCommandOptions::with_envs(e2b_control_envs(HashMap::new())),
                    )
                    .await
                    .map_err(|error| map_error(error, "write", &target.display_path))?;
                match publication.stdout.as_str() {
                    "exists" => {
                        return Err(FsError::new(
                            format!(
                                "cannot overwrite existing \"{}\" without reading it first",
                                target.display_path
                            ),
                            FsErrorCode::FsNotObserved,
                        ))
                    }
                    "created" => E2bEntryInfo {
                        name: posix_basename(target_path),
                        path: target_path.to_string(),
                        ..staged
                    },
                    _ => {
                        return Err(FsError::new(
                            "guarded create returned an invalid publication result",
                            FsErrorCode::FsIoError,
                        ))
                    }
                }
            } else {
                sandbox
                    .rename(&temporary, target_path)
                    .await
                    .map_err(|error| map_error(error, "write", &target.display_path))?
            };
            let _ = sandbox.remove(&staging_directory).await;
            Ok::<FsVersion, FsError>(entry_version(&committed))
        }
        .await;
        match result {
            Ok(version) => Ok(version),
            Err(error) => {
                if staging_created {
                    let _ = sandbox.remove(&staging_directory).await;
                }
                Err(error)
            }
        }
    }
}

#[async_trait::async_trait]
impl FileSystem for E2bFileSystem {
    async fn resolve(
        &self,
        path: &str,
        opts: Option<&ResolveOptions>,
    ) -> Result<FsTarget, FsError> {
        let signal = opts.and_then(|opts| opts.signal.clone());
        assert_not_aborted(signal.as_ref(), "resolve")?;
        if path.trim().is_empty() {
            return Err(FsError::new(
                "file_path must be a non-empty string",
                FsErrorCode::FsNotFound,
            ));
        }
        let cwd = opts
            .and_then(|opts| opts.cwd.clone())
            .unwrap_or_else(|| self.runtime().cwd().to_string());
        let display_path = posix_resolve(&cwd, path);
        let sandbox = self.runtime().get_sandbox().await.map_err(|message| {
            FsError::new(
                format!("cannot resolve \"{display_path}\": {message}"),
                FsErrorCode::FsIoError,
            )
        })?;
        let target_key = self
            .canonical_path(&sandbox, &display_path, signal.as_ref())
            .await?;
        assert_not_aborted(signal.as_ref(), "resolve")?;
        Ok(FsTarget {
            target_key: fs_target_key(target_key),
            display_path,
        })
    }

    fn process_path(&self, target: &FsTarget) -> String {
        target.target_key.as_str().to_string()
    }

    fn file_url(&self, target: &FsTarget) -> String {
        let path = self.process_path(target);
        if !path.starts_with('/') {
            panic!("fs-e2b: expected an absolute process path: {path:?}");
        }
        format!(
            "file://{}",
            path.split('/')
                .map(percent_encode_segment)
                .collect::<Vec<_>>()
                .join("/")
        )
    }

    fn contains(&self, parent: &FsTarget, child: &FsTarget) -> bool {
        let relative = posix_relative(&self.process_path(parent), &self.process_path(child));
        matches!(relative, Some(_))
    }

    async fn stat(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<Option<FsInfo>, FsError> {
        assert_not_aborted(signal.as_ref(), "stat")?;
        let Some(entry) = self
            .probe(
                target.target_key.as_str(),
                &target.display_path,
                signal.as_ref(),
            )
            .await?
        else {
            return Ok(None);
        };
        let mut info = FsInfo {
            version: entry_version(&entry),
            kind: entry_type(&entry),
            size: None,
        };
        if entry.file_type == FileType::File {
            info.size = Some(entry.size);
        }
        Ok(Some(info))
    }

    async fn lstat(
        &self,
        path: &str,
        opts: Option<&LstatOptions>,
        signal: Option<AbortPredicate>,
    ) -> Result<Option<FsPathInfo>, FsError> {
        assert_not_aborted(signal.as_ref(), "lstat")?;
        if path.trim().is_empty() {
            return Err(FsError::new(
                "file_path must be a non-empty string",
                FsErrorCode::FsNotFound,
            ));
        }
        let cwd = opts
            .and_then(|opts| opts.cwd.clone())
            .unwrap_or_else(|| self.runtime().cwd().to_string());
        let display_path = posix_resolve(&cwd, path);
        let Some(entry) = self
            .probe(&display_path, &display_path, signal.as_ref())
            .await?
        else {
            return Ok(None);
        };
        let kind = if entry.symlink_target.is_some() {
            FsPathInfoType::Symlink
        } else {
            match entry.file_type {
                FileType::File => FsPathInfoType::File,
                FileType::Dir => FsPathInfoType::Directory,
                FileType::Other => FsPathInfoType::Other,
            }
        };
        Ok(Some(FsPathInfo {
            version: entry_version(&entry),
            kind,
            size: (entry.file_type == FileType::File).then_some(entry.size),
        }))
    }

    async fn read_text(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<String, FsError> {
        let sandbox = self.runtime().get_sandbox().await.map_err(|message| {
            FsError::new(
                format!("cannot read \"{}\": {message}", target.display_path),
                FsErrorCode::FsIoError,
            )
        })?;
        self.require_regular(target, signal.as_ref()).await?;
        let bytes = sandbox
            .read_bytes(target.target_key.as_str())
            .await
            .map_err(|error| map_error(error, "read", &target.display_path))?;
        assert_not_aborted(signal.as_ref(), "read")?;
        decode_text(&bytes, &target.display_path)
    }

    async fn stream_text(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<BoxStream<'static, Result<String, FsError>>, FsError> {
        let sandbox = self.runtime().get_sandbox().await.map_err(|message| {
            FsError::new(
                format!("cannot read \"{}\": {message}", target.display_path),
                FsErrorCode::FsIoError,
            )
        })?;
        self.require_regular(target, signal.as_ref()).await?;
        let mut stream = sandbox
            .read_stream(target.target_key.as_str())
            .await
            .map_err(|error| map_error(error, "read", &target.display_path))?;
        let display_path = target.display_path.clone();
        let signal = signal.clone();
        let sampled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let output = async_stream::stream! {
            let mut pending: Vec<u8> = Vec::new();
            loop {
                if signal.as_ref().is_some_and(|s| s()) {
                    let _ = stream.cancel().await;
                    yield Err(FsError::new("read aborted", FsErrorCode::FsAborted));
                    return;
                }
                match stream.read().await {
                    Ok(Some(chunk)) => {
                        let sampled_before = sampled.load(std::sync::atomic::Ordering::SeqCst);
                        if sampled_before < BINARY_SAMPLE_BYTES {
                            let take = (BINARY_SAMPLE_BYTES - sampled_before).min(chunk.len());
                            if chunk[..take].contains(&0) {
                                let _ = stream.cancel().await;
                                yield Err(FsError::new(
                                    format!("cannot read \"{display_path}\": binary file"),
                                    FsErrorCode::FsNotText,
                                ));
                                return;
                            }
                            sampled.store(sampled_before + take, std::sync::atomic::Ordering::SeqCst);
                        }
                        let mut buffer = std::mem::take(&mut pending);
                        buffer.extend_from_slice(&chunk);
                        match decode_incremental(&buffer, true) {
                            Ok((text, tail)) => {
                                pending = tail;
                                if !text.is_empty() {
                                    yield Ok(text);
                                }
                            }
                            Err(()) => {
                                let _ = stream.cancel().await;
                                yield Err(FsError::new(
                                    format!("cannot read \"{display_path}\": invalid UTF-8 text"),
                                    FsErrorCode::FsNotText,
                                ));
                                return;
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = stream.cancel().await;
                        yield Err(map_error(error, "read", &display_path));
                        return;
                    }
                }
            }
            // Final flush: the trailing bytes must complete a character.
            let mut buffer = std::mem::take(&mut pending);
            match decode_incremental(&buffer, false) {
                Ok((text, tail)) => {
                    if !tail.is_empty() {
                        let _ = stream.cancel().await;
                        yield Err(FsError::new(
                            format!("cannot read \"{display_path}\": invalid UTF-8 text"),
                            FsErrorCode::FsNotText,
                        ));
                        return;
                    }
                    if !text.is_empty() {
                        yield Ok(text);
                    }
                }
                Err(()) => {
                    yield Err(FsError::new(
                        format!("cannot read \"{display_path}\": invalid UTF-8 text"),
                        FsErrorCode::FsNotText,
                    ));
                }
            }
        };
        Ok(output.boxed())
    }

    async fn read_bytes(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
        max_bytes: u64,
    ) -> Result<Vec<u8>, FsError> {
        let sandbox = self.runtime().get_sandbox().await.map_err(|message| {
            FsError::new(
                format!("cannot read \"{}\": {message}", target.display_path),
                FsErrorCode::FsIoError,
            )
        })?;
        let info = self.require_regular(target, signal.as_ref()).await?;
        if let Some(size) = info.size {
            if size > max_bytes {
                return Err(FsError::new(
                    format!(
                        "cannot read \"{}\": {size} bytes exceeds the {max_bytes}-byte limit",
                        target.display_path
                    ),
                    FsErrorCode::FsTooLarge,
                ));
            }
        }
        let mut stream = sandbox
            .read_stream(target.target_key.as_str())
            .await
            .map_err(|error| map_error(error, "read", &target.display_path))?;
        let mut whole: Vec<u8> = Vec::new();
        loop {
            assert_not_aborted(signal.as_ref(), "read")?;
            match stream.read().await {
                Ok(Some(chunk)) => {
                    whole.extend_from_slice(&chunk);
                    if whole.len() as u64 > max_bytes {
                        let _ = stream.cancel().await;
                        return Err(FsError::new(
                            format!(
                                "cannot read \"{}\": content exceeds the {max_bytes}-byte limit",
                                target.display_path
                            ),
                            FsErrorCode::FsTooLarge,
                        ));
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = stream.cancel().await;
                    return Err(map_error(error, "read", &target.display_path));
                }
            }
        }
        Ok(whole)
    }

    async fn list_dir(
        &self,
        target: &FsTarget,
        signal: Option<AbortPredicate>,
    ) -> Result<Vec<FsDirEntry>, FsError> {
        let info = self.stat(target, signal.clone()).await?.ok_or_else(|| {
            FsError::new(
                format!("cannot list \"{}\": not found", target.display_path),
                FsErrorCode::FsNotFound,
            )
        })?;
        if info.kind != FsInfoType::Directory {
            return Err(FsError::new(
                format!("cannot list \"{}\": not a directory", target.display_path),
                FsErrorCode::FsNotDirectory,
            ));
        }
        let sandbox = self.runtime().get_sandbox().await.map_err(|message| {
            FsError::new(
                format!("cannot list \"{}\": {message}", target.display_path),
                FsErrorCode::FsIoError,
            )
        })?;
        let listed = sandbox
            .list(target.target_key.as_str())
            .await
            .map_err(|error| map_error(error, "list", &target.display_path))?;
        let mut entries: Vec<FsDirEntry> = Vec::new();
        for entry in listed {
            let display_path = format!("{}/{}", target.display_path, entry.name);
            let canonical = if entry.symlink_target.is_none() {
                entry.path.clone()
            } else {
                self.canonical_path(&sandbox, &entry.path, signal.as_ref())
                    .await?
            };
            let resolved: Option<E2bEntryInfo> = if entry.symlink_target.is_none() {
                Some(entry.clone())
            } else {
                self.probe(&canonical, &display_path, signal.as_ref())
                    .await?
            };
            entries.push(FsDirEntry {
                name: entry.name,
                kind: resolved
                    .as_ref()
                    .map(entry_type)
                    .unwrap_or(FsInfoType::Other),
                target: FsTarget {
                    target_key: fs_target_key(canonical),
                    display_path,
                },
                version: resolved.as_ref().map(entry_version),
                size: resolved
                    .as_ref()
                    .filter(|entry| entry.file_type == FileType::File)
                    .map(|entry| entry.size),
            });
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
    }

    async fn write_text(
        &self,
        target: &FsTarget,
        content: &str,
        expected: Option<&FsWriteIntent>,
        signal: Option<AbortPredicate>,
        _sandbox_policy: Option<&dsh_sandbox::SandboxExecutionPolicy>,
    ) -> Result<FsWriteOutcome, FsError> {
        self.with_lock(target.target_key.as_str(), async {
            let existing = self
                .probe(
                    target.target_key.as_str(),
                    &target.display_path,
                    signal.as_ref(),
                )
                .await?;
            if let Some(entry) = &existing {
                if entry_type(entry) != FsInfoType::File {
                    return Err(FsError::new(
                        format!(
                            "cannot write \"{}\": not a regular file",
                            target.display_path
                        ),
                        FsErrorCode::FsNotRegularFile,
                    ));
                }
            }
            self.check_write_intent(existing.as_ref(), expected, target)?;
            let before = match &existing {
                None => None,
                Some(_) => self.read_for_diff(target, signal.as_ref()).await?,
            };
            let version = self
                .write_atomic(
                    target,
                    content,
                    existing.as_ref(),
                    matches!(expected, Some(FsWriteIntent::CreateIfAbsent)),
                    signal.as_ref(),
                )
                .await?;
            Ok(FsWriteOutcome {
                operation: if existing.is_none() {
                    dsh_fs::FsWriteOperation::Create
                } else {
                    dsh_fs::FsWriteOperation::Update
                },
                version,
                before,
                after: normalize_line_endings(content),
            })
        })
        .await
    }

    async fn edit_text(
        &self,
        target: &FsTarget,
        edit: &FsEditRequest,
        expected: Option<&FsEditGuard>,
        signal: Option<AbortPredicate>,
        _sandbox_policy: Option<&dsh_sandbox::SandboxExecutionPolicy>,
    ) -> Result<FsEditOutcome, FsError> {
        self.with_lock(target.target_key.as_str(), async {
            let existing = self
                .probe(
                    target.target_key.as_str(),
                    &target.display_path,
                    signal.as_ref(),
                )
                .await?;
            let Some(existing) = existing else {
                return Err(FsError::new(
                    format!(
                        "cannot edit \"{}\": file changed since it was read",
                        target.display_path
                    ),
                    FsErrorCode::FsStaleVersion,
                ));
            };
            if entry_type(&existing) != FsInfoType::File {
                return Err(FsError::new(
                    format!(
                        "cannot edit \"{}\": not a regular file",
                        target.display_path
                    ),
                    FsErrorCode::FsNotRegularFile,
                ));
            }
            if expected.is_some_and(|guard| entry_version(&existing) != guard.version) {
                return Err(FsError::new(
                    format!(
                        "cannot edit \"{}\": file changed since it was read",
                        target.display_path
                    ),
                    FsErrorCode::FsStaleVersion,
                ));
            }
            let raw = self.read_for_edit(target, signal.as_ref()).await?;
            let before = normalize_line_endings(&raw);
            let after = literal_edit(&before, edit, &target.display_path)?;
            let storage = restore_line_endings(&after, detects_crlf(&raw));
            let version = self
                .write_atomic(target, &storage, Some(&existing), false, signal.as_ref())
                .await?;
            Ok(FsEditOutcome {
                version,
                before,
                after,
            })
        })
        .await
    }
}

/// Percent-encode one URI path segment (TS `encodeURIComponent` per
/// segment).
fn percent_encode_segment(segment: &str) -> String {
    let mut encoded = String::new();
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => encoded.push(byte as char),
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// Decode as much complete UTF-8 as possible from `buffer`; `streaming`
/// keeps an incomplete trailing sequence for the next chunk. Returns the
/// decoded prefix and the un-decoded tail.
fn decode_incremental(buffer: &[u8], streaming: bool) -> Result<(String, Vec<u8>), ()> {
    match std::str::from_utf8(buffer) {
        Ok(_) => Ok((
            std::str::from_utf8(buffer).expect("utf8").to_string(),
            Vec::new(),
        )),
        Err(error) => {
            let valid_up_to = error.valid_up_to();
            if error.error_len().is_none() && streaming {
                // Incomplete trailing sequence; keep it for the next chunk.
                let text = std::str::from_utf8(&buffer[..valid_up_to]).expect("valid prefix");
                return Ok((text.to_string(), buffer[valid_up_to..].to_vec()));
            }
            Err(())
        }
    }
}

/// The Cordis plugin form (TS mounts the module with `inject = ['e2b']`).
pub struct FsE2bPlugin;

impl FsE2bPlugin {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Plugin for FsE2bPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(FS_E2B_NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new(FS_E2B_INJECT)
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        E2bFileSystem::install(ctx)
            .map(|_| ())
            .map_err(|message| PluginError::from(anyhow::anyhow!(message)))
    }
}
