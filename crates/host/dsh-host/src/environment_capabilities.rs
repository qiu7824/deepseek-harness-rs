//! Host-owned, bounded runtime diagnostics shared across sessions.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cordis::Context;
use dsh_llm::ContentBlock;
use dsh_subprocess::{
    SubprocessCollect, SubprocessHandle, SubprocessOutputMode, SubprocessRuntime,
    SubprocessSpawnSpec, SubprocessStdinMode, SubprocessStdio,
};
use dsh_system_prompt::{PromptContext, PromptText, SystemPrompt};
use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::runtime_paths::RuntimePaths;

pub(super) const IDS: [&str; 9] = [
    "python", "node", "git", "rg", "pwsh", "ffmpeg", "wps", "rustc", "cargo",
];
const POSITIVE_TTL: u64 = 24 * 60 * 60;
const NEGATIVE_TTL: u64 = 60;
const MAX_CACHE_BYTES: u64 = 128 * 1024;
const PYTHON_PROBE: &str = "import sys,json,importlib.util,sysconfig; print(json.dumps({'version':sys.version.split()[0],'path':sys.executable,'packageRoots':list(set([sysconfig.get_path('purelib'),sysconfig.get_path('platlib')])),'modules':{n:importlib.util.find_spec(n) is not None for n in ['PIL','pypdf','fitz','openpyxl','docx','pptx','reportlab']}}))";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct FileIdentity {
    length: u64,
    modified_nanos: u128,
}
fn identity(path: &str) -> Option<FileIdentity> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    Some(FileIdentity {
        length: metadata.len(),
        modified_nanos: metadata
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos(),
    })
}
fn directory_identity(path: &str) -> Option<FileIdentity> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(FileIdentity {
        length: metadata.len(),
        modified_nanos: metadata
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos(),
    })
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    #[serde(default)]
    check_id: String,
    environment: String,
    checked_at: u64,
    identity: Option<FileIdentity>,
    #[serde(default)]
    dependencies: BTreeMap<String, Option<FileIdentity>>,
    result: Value,
}
impl Record {
    fn valid(&self, environment: &str, now: u64) -> bool {
        let ttl = if matches!(self.result["status"].as_str(), Some("ready" | "located")) {
            POSITIVE_TTL
        } else {
            NEGATIVE_TTL
        };
        self.environment == environment
            && now >= self.checked_at
            && now - self.checked_at < ttl
            && (!matches!(self.result["status"].as_str(), Some("ready" | "located"))
                || self.identity.is_some())
            && self.result["path"].as_str().map(identity).unwrap_or(None) == self.identity
            && self
                .dependencies
                .iter()
                .all(|(path, expected)| &directory_identity(path) == expected)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    records: BTreeMap<String, Record>,
}

pub(super) struct EnvironmentCapabilities {
    runtime: Arc<dyn SubprocessRuntime>,
    paths: Arc<RuntimePaths>,
    cache_path: PathBuf,
    records: Mutex<BTreeMap<String, Record>>,
    gates: BTreeMap<&'static str, tokio::sync::Mutex<()>>,
    persist_gate: tokio::sync::Mutex<()>,
    flights: Mutex<BTreeMap<String, Arc<ProbeFlight>>>,
}

struct ProbeFlight {
    result: Shared<BoxFuture<'static, Result<Value, ToolBodyError>>>,
    waiters: AtomicUsize,
    cancelled: Arc<AtomicBool>,
}
struct FlightWaiter(Arc<ProbeFlight>);
impl Drop for FlightWaiter {
    fn drop(&mut self) {
        if self.0.waiters.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.cancelled.store(true, Ordering::Release);
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl EnvironmentCapabilities {
    pub(super) fn new(runtime: Arc<dyn SubprocessRuntime>, paths: Arc<RuntimePaths>) -> Arc<Self> {
        let cache_path = paths.paths["cacheDirectory"].join("environment-capabilities-v1.json");
        let records = std::fs::metadata(&cache_path)
            .ok()
            .filter(|m| m.len() <= MAX_CACHE_BYTES)
            .and_then(|_| std::fs::read(&cache_path).ok())
            .and_then(|bytes| serde_json::from_slice::<CacheFile>(&bytes).ok())
            .filter(|cache| {
                cache.version == 1
                    && cache.records.len() <= IDS.len()
                    && cache.records.keys().all(|id| IDS.contains(&id.as_str()))
            })
            .map(|cache| cache.records)
            .unwrap_or_default();
        Arc::new(Self {
            runtime,
            paths,
            cache_path,
            records: Mutex::new(records),
            gates: IDS
                .into_iter()
                .map(|id| (id, tokio::sync::Mutex::new(())))
                .collect(),
            persist_gate: tokio::sync::Mutex::new(()),
            flights: Mutex::new(BTreeMap::new()),
        })
    }

    pub(super) fn environment(&self) -> String {
        // Hash only execution identity/configuration. Never persist environment values or credentials.
        let environment: Vec<_> = [
            "PATH",
            "PATHEXT",
            "VIRTUAL_ENV",
            "CONDA_PREFIX",
            "USERPROFILE",
            "HOME",
            "COMPUTERNAME",
            "HOSTNAME",
            "DSH_NODE_COMMAND",
            "DSH_PYTHON_COMMAND",
            "DSH_WPS_COMMAND",
        ]
        .into_iter()
        .map(|key| {
            (
                key,
                std::env::var_os(key).map(|s| s.to_string_lossy().into_owned()),
            )
        })
        .collect();
        let path_directories: Vec<_> =
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .take(256)
                .filter(|path| path.is_absolute())
                .map(|path| {
                    let path = path.to_string_lossy().into_owned();
                    let stamp = directory_identity(&path);
                    (path, stamp)
                })
                .collect();
        format!("{:x}", Sha256::digest(serde_json::to_vec(&json!({"os":std::env::consts::OS,"arch":std::env::consts::ARCH,
            "environment":environment,"pathDirectories":path_directories,"node":self.paths.node_command(),"runtimeDirectory":self.paths.paths["environmentDirectory"]})).unwrap()))
    }

    pub(super) async fn inspect(
        self: &Arc<Self>,
        id: &str,
        refresh: bool,
        signal: dsh_tools::AbortPredicate,
    ) -> Result<Value, ToolBodyError> {
        if signal() {
            return Err(ToolBodyError::coded(
                "environment probe cancelled",
                "AbortError",
                "ABORTED",
            ));
        }
        let key = format!("{id}:{refresh}:{}", self.environment());
        let (flight, coalesced) = {
            let mut flights = self.flights.lock();
            flights.retain(|_, flight| !flight.cancelled.load(Ordering::Acquire));
            let flight = flights
                .entry(key.clone())
                .or_insert_with(|| {
                    let cache = self.clone();
                    let id = id.to_string();
                    let cancelled = Arc::new(AtomicBool::new(false));
                    let flag = cancelled.clone();
                    let result = async move {
                        cache
                            .inspect_inner(
                                &id,
                                refresh,
                                Arc::new(move || flag.load(Ordering::Acquire)),
                            )
                            .await
                    }
                    .boxed()
                    .shared();
                    Arc::new(ProbeFlight {
                        result,
                        waiters: AtomicUsize::new(0),
                        cancelled,
                    })
                })
                .clone();
            let coalesced = flight.waiters.fetch_add(1, Ordering::AcqRel) > 0;
            (flight, coalesced)
        };
        let waiter = FlightWaiter(flight.clone());
        let mut result = tokio::select! {
            result = flight.result.clone() => result,
            _ = cancelled(signal) => Err(ToolBodyError::coded("environment probe cancelled", "AbortError", "ABORTED")),
        };
        if flight.result.peek().is_some() || flight.waiters.load(Ordering::Acquire) == 1 {
            let mut flights = self.flights.lock();
            if flights
                .get(&key)
                .is_some_and(|active| Arc::ptr_eq(active, &flight))
            {
                flights.remove(&key);
            }
        }
        drop(waiter);
        if coalesced {
            if let Ok(value) = &mut result {
                value["cacheHit"] = json!(true);
                value["coalesced"] = json!(true);
            }
        }
        result
    }

    async fn inspect_inner(
        &self,
        id: &str,
        refresh: bool,
        signal: dsh_tools::AbortPredicate,
    ) -> Result<Value, ToolBodyError> {
        if signal() {
            return Err(ToolBodyError::coded(
                "environment probe cancelled",
                "AbortError",
                "ABORTED",
            ));
        }
        let gate = self
            .gates
            .get(id)
            .ok_or_else(|| ToolBodyError::plain("unknown environment capability"))?;
        let started = Instant::now();
        let prior_check = self
            .records
            .lock()
            .get(id)
            .map(|record| record.check_id.clone());
        let work = async {
            let _guard = gate.lock().await;
            let environment = self.environment();
            let refreshed_while_waiting = self
                .records
                .lock()
                .get(id)
                .is_some_and(|record| Some(&record.check_id) != prior_check.as_ref());
            if !refresh || refreshed_while_waiting {
                if let Some(record) = self
                    .records
                    .lock()
                    .get(id)
                    .filter(|r| r.valid(&environment, now()))
                    .cloned()
                {
                    let mut result = record.result;
                    result["cacheHit"] = json!(true);
                    result["checkedAt"] = json!(record.checked_at);
                    return Ok(result);
                }
            }
            let mut result = self.probe(id, signal.clone()).await;
            if signal() {
                return Err(ToolBodyError::coded(
                    "environment probe cancelled",
                    "AbortError",
                    "ABORTED",
                ));
            }
            result["id"] = json!(id);
            result["executionWorld"] = json!("host");
            result["cacheHit"] = json!(false);
            result["elapsedMs"] = json!(started.elapsed().as_millis() as u64);
            let checked_at = now();
            result["checkedAt"] = json!(checked_at);
            let record = Record {
                check_id: uuid::Uuid::new_v4().to_string(),
                environment,
                checked_at,
                identity: result["path"].as_str().and_then(identity),
                dependencies: result["packageRoots"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .take(8)
                    .map(|path| (path.to_string(), directory_identity(path)))
                    .collect(),
                result: result.clone(),
            };
            self.records.lock().insert(id.into(), record);
            if self.persist().await.is_err() {
                result["cacheWarning"] =
                    json!("Persistent cache unavailable; this process still reuses the result.");
            }
            Ok(result)
        };
        tokio::select! {
            result = work => result,
            _ = cancelled(signal.clone()) => Err(ToolBodyError::coded("environment probe cancelled", "AbortError", "ABORTED")),
        }
    }

    async fn persist(&self) -> Result<(), String> {
        let _guard = self.persist_gate.lock().await;
        let records = self.records.lock().clone();
        let bytes = serde_json::to_vec(&CacheFile {
            version: 1,
            records,
        })
        .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_CACHE_BYTES {
            return Err("environment cache budget exceeded".into());
        }
        let path = &self.cache_path;
        std::fs::create_dir_all(path.parent().ok_or("cache parent missing")?)
            .map_err(|e| e.to_string())?;
        let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temp, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temp);
        }
        result.map_err(|e| e.to_string())
    }

    async fn probe(&self, id: &str, signal: dsh_tools::AbortPredicate) -> Value {
        if id == "node" {
            // Reuse the production Node capability validator, including permissions/TypeScript/workers.
            let mut status = self.paths.node_status(self.runtime.clone(), true).await;
            status["capabilityScope"] = json!(
                "PTC permissions, TypeScript stripping and workers; incompatible does not mean ordinary JavaScript is unavailable"
            );
            return status;
        }
        let command = match id {
            "pwsh" => dsh_shell::powershell::locate_powershell().unwrap_or_else(|| "pwsh".into()),
            "python" => std::env::var("DSH_PYTHON_COMMAND")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| {
                    self.paths
                        .python_command()
                        .map(|path| path.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| if cfg!(windows) { "python" } else { "python3" }.into()),
            "wps" => std::env::var("DSH_WPS_COMMAND")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(find_wps)
                .unwrap_or_else(|| "wps".into()),
            _ => id.into(),
        };
        let mut result = json!({"status":"missing","path":Value::Null,"version":Value::Null});
        let task = async {
            let path = self
                .runtime
                .resolve_executable(&command, None, Some(signal.clone()))
                .await
                .map_err(|error| {
                    let lower = error.to_lowercase();
                    (
                        if lower.contains("denied")
                            || lower.contains("permission")
                            || lower.contains("拒绝")
                        {
                            "permission_denied"
                        } else if lower.contains("not found")
                            || lower.contains("missing")
                            || lower.contains("no such")
                        {
                            "missing"
                        } else {
                            "error"
                        },
                        error,
                    )
                })?;
            result["path"] = json!(path);
            if id == "wps" {
                result["status"] = json!("located");
                return Ok(());
            }
            let args: &[&str] = match id {
                "python" => &["-I", "-B", "-c", PYTHON_PROBE],
                "pwsh" => &[
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "$PSVersionTable.PSVersion.ToString()",
                ],
                "ffmpeg" => &["-version"],
                _ => &["--version"],
            };
            let output = self
                .output(&path, args, signal.clone())
                .await
                .map_err(|error| {
                    let lower = error.to_lowercase();
                    (
                        if lower.contains("denied")
                            || lower.contains("permission")
                            || lower.contains("拒绝")
                        {
                            "permission_denied"
                        } else {
                            "error"
                        },
                        error,
                    )
                })?;
            if id == "python" {
                let data: Value = serde_json::from_str(output.trim())
                    .map_err(|_| ("error", "Python returned invalid diagnostic data".into()))?;
                let version = data["version"]
                    .as_str()
                    .filter(|s| s.len() < 64)
                    .ok_or(("error", "Python version missing".into()))?;
                result["version"] = json!(version);
                result["modules"] = data["modules"].clone();
                result["packageRoots"] = data["packageRoots"].clone();
                result["moduleScope"] = json!(
                    "isolated interpreter; workspace and user-site packages are not included"
                );
            } else {
                let version = output.lines().next().unwrap_or_default();
                if version.is_empty() {
                    return Err(("error", "Version probe returned empty output".into()));
                }
                result["version"] = json!(version.chars().take(256).collect::<String>());
            }
            result["status"] = json!("ready");
            Ok::<(), (&str, String)>(())
        };
        match tokio::time::timeout(Duration::from_secs(5), task).await {
            Ok(Ok(())) => (),
            Ok(Err((status, error))) => {
                result["status"] = json!(status);
                result["error"] = json!(error.chars().take(256).collect::<String>());
            }
            Err(_) => {
                result["status"] = json!("timed_out");
                result["error"] = json!("Host probe exceeded five seconds");
            }
        }
        result
    }

    async fn output(
        &self,
        path: &str,
        args: &[&str],
        signal: dsh_tools::AbortPredicate,
    ) -> Result<String, String> {
        let child = self.runtime.spawn(SubprocessSpawnSpec {
            argv: std::iter::once(path.to_string())
                .chain(args.iter().map(|s| s.to_string()))
                .collect(),
            cwd: self.paths.paths["environmentDirectory"]
                .to_string_lossy()
                .into_owned(),
            stdio: SubprocessStdio {
                stdin: SubprocessStdinMode::Ignore,
                stdout: SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: 8192,
                    spill: None,
                }),
                stderr: SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: 1024,
                    spill: None,
                }),
            },
            grace_ms: 200,
            signal: Some(signal),
            env: Some(vec![
                ("NODE_OPTIONS".into(), None),
                ("PYTHONSTARTUP".into(), None),
            ]),
        })?;
        let _guard = ProcessGuard(child.clone());
        let outcome = child.done().await?;
        if outcome.exit_code != Some(0) {
            return Err(format!("Version probe exited with {:?}", outcome.exit_code));
        }
        let read = child
            .collected()
            .stdout
            .ok_or("Probe stdout missing")?
            .read_from(0);
        if read.lossy {
            return Err("Probe output exceeded its byte budget".into());
        }
        Ok(read.text)
    }

    pub(super) fn summary(&self) -> String {
        let environment = self.environment();
        let records = self.records.lock();
        let mut text = String::from(
            "Host environment capabilities: reuse the paths below while valid. Use environment_probe for missing details or refresh after an execution failure; do not repeat shell discovery for known capabilities. These are host diagnostics, not proof of sandbox/remote access. Actual operations must use normal tools and permissions.\n",
        );
        text.push_str("For build tasks discover the needed toolchain (including cargo) once, then validate it in the selected execution context before starting a long/background build. Access denied means unknown accessibility, not missing installation. Do not rotate shell wrappers, guess installation directories, or change global settings after repeated failures; follow the diagnostic recovery and permission flow. Background started means running, not passed. For Office/PDF conversion and visual inspection use office_render: it runs the fixed WPS host bridge and Windows PDF renderer without Python, ffmpeg or LibreOffice. Do not guess WPS command-line switches or retry COM activation through sandboxed shells.\n");
        for (id, record) in records
            .iter()
            .filter(|(_, record)| record.valid(&environment, now()))
        {
            let row = json!({"id":id,"status":record.result["status"],"path":record.result["path"],"version":record.result["version"],"modules":record.result["modules"]});
            text.push_str(&row.to_string());
            text.push('\n');
        }
        if records.is_empty() {
            text.push_str(
                "No verified capabilities cached. Query only capabilities needed for the task.\n",
            );
        }
        text.chars().take(4096).collect()
    }
}

struct ProcessGuard(Arc<dyn SubprocessHandle>);
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        self.0.terminate();
    }
}
async fn cancelled(signal: dsh_tools::AbortPredicate) {
    loop {
        if signal() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(super) fn find_wps() -> Option<String> {
    if !cfg!(windows) {
        return std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .filter(|p| p.is_absolute())
            .map(|p| p.join("wps"))
            .find(|p| p.is_file())
            .map(|p| p.to_string_lossy().into_owned());
    }
    #[cfg(windows)]
    if let Some(path) = registered_wps() {
        return Some(path);
    }
    for variable in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        let Some(root) = std::env::var_os(variable).map(PathBuf::from) else {
            continue;
        };
        let root = root.join("Kingsoft").join("WPS Office");
        for relative in ["office6/wps.exe", "wps.exe"] {
            let path = root.join(relative);
            if path.is_file() {
                return Some(path.to_string_lossy().into_owned());
            }
        }
        if let Ok(entries) = std::fs::read_dir(root) {
            let mut directories: Vec<_> = entries
                .filter_map(Result::ok)
                .take(64)
                .map(|entry| entry.path())
                .collect();
            directories.sort();
            directories.reverse();
            for directory in directories {
                let path = directory.join("office6/wps.exe");
                if path.is_file() {
                    return Some(path.to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

#[cfg(windows)]
fn registered_wps() -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW,
    };
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for key in [
            "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\wps.exe",
            "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\App Paths\\wps.exe",
        ] {
            let key: Vec<_> = key.encode_utf16().chain(Some(0)).collect();
            let mut data = vec![0u16; 4096];
            let mut bytes = (data.len() * 2) as u32;
            let status = unsafe {
                RegGetValueW(
                    hive,
                    key.as_ptr(),
                    std::ptr::null(),
                    RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
                    std::ptr::null_mut(),
                    data.as_mut_ptr().cast(),
                    &mut bytes,
                )
            };
            if status == 0 {
                let length = data.iter().position(|c| *c == 0).unwrap_or(data.len());
                let path = String::from_utf16_lossy(&data[..length])
                    .trim_matches('"')
                    .to_string();
                if Path::new(&path).is_absolute() && Path::new(&path).is_file() {
                    return Some(path);
                }
            }
        }
    }
    None
}

pub(super) fn install(
    ctx: &Context,
    tools: &Arc<ToolRuntime>,
    prompt: &Arc<SystemPrompt>,
    runtime: Arc<dyn SubprocessRuntime>,
    paths: Arc<RuntimePaths>,
) -> Result<Arc<EnvironmentCapabilities>, String> {
    let capabilities = EnvironmentCapabilities::new(runtime, paths);
    let probe = capabilities.clone();
    tools.register(ctx, ToolDefinition {
        name: "environment_probe".into(),
        description: "Get cached host program paths, versions and supported capabilities. Request only required names: python, node, git, rg, pwsh, ffmpeg, wps, rustc, cargo. Reuse results; refresh only after failure or environment changes. Python module checks use isolated mode. WPS is located without launching Office. This does not establish sandbox or remote permissions; use environment_validate for the selected execution context.".into(),
        parameters: json!({"type":"object","properties":{"names":{"type":"array","minItems":1,"maxItems":9,"items":{"type":"string","enum":IDS}},"refresh":{"type":"boolean"}},"required":["names"],"additionalProperties":false}),
        output: ToolOutputDefinition { schema: json!({"type":"object"}), render: Arc::new(|_, value| Ok(vec![ContentBlock::Text { text: value.to_string() }])), presentation_meta: None },
        timeout_ms: Some(60000), is_concurrency_safe: Some(Arc::new(|_| true)),
        execute: Arc::new(move |args, exec| {
            let probe = probe.clone(); let args = args.clone(); let signal = exec.signal.lock().clone();
            Box::pin(async move {
                let mut results = Vec::new();
                let mut seen = std::collections::BTreeSet::new();
                for id in args["names"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    if seen.insert(id) { results.push(probe.inspect(id, args["refresh"].as_bool().unwrap_or(false), signal.clone()).await?); }
                }
                Ok(json!({"capabilities":results,"executionWorld":"host"}))
            })
        }), finalize_content: None, present_call: None, present_result: None,
    })?;
    let weak_tools = Arc::downgrade(tools);
    let summary_capabilities = capabilities.clone();
    prompt.context(
        ctx,
        PromptContext {
            name: "environment:capabilities".into(),
            order: 81.0,
            text: PromptText::Provider(Arc::new(move |context| {
                if !weak_tools.upgrade().is_some_and(|tools| {
                    tools
                        .get("environment_probe", context.scope.as_ref())
                        .is_some()
                }) {
                    return String::new();
                }
                summary_capabilities.summary()
            })),
        },
    );
    Ok(capabilities)
}

pub(super) fn discovery_config(
    data_root: &Path,
) -> Result<dsh_tools::discovery::DiscoveryConfig, String> {
    let path = data_root.join("tool-discovery.json");
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.len() > 64 * 1024 => {
            Err("tool-discovery.json exceeds 64 KiB".into())
        }
        Ok(_) => serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("tool-discovery.json: {e}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(error) => Err(format!("tool-discovery.json: {error}")),
    }
}

#[cfg(test)]
#[path = "environment_capabilities_tests.rs"]
mod tests;
