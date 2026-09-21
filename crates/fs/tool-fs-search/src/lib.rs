//! Model-facing ripgrep-backed `glob` and `grep` tools.

use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, downcast};
use dsh_subprocess::{
    SubprocessAbort, SubprocessCollect, SubprocessOutputMode, SubprocessRuntime,
    SubprocessSpawnSpec, SubprocessStdinMode, SubprocessStdio,
};
use dsh_system_prompt::{PromptSection, SystemPrompt};
use dsh_tools::{ToolBodyError, ToolCallKind, ToolCallView, ToolDefinition, ToolOutputDefinition};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

pub const NAME: &str = "tool-fs-search";
pub const INJECT: [&str; 3] = ["tools", "systemPrompt", "subprocess"];
const VCS: [&str; 6] = [".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];

#[derive(Clone)]
struct Config {
    sample: bool,
    glob_max: usize,
    grep_max: usize,
    line_max: usize,
    raw_max: u64,
    grace_ms: u64,
    stderr_max: u64,
    timeout_ms: u64,
}
impl Config {
    fn parse(value: &ArcValue) -> Result<Self, String> {
        let v = downcast::<serde_json::Value>(value)
            .cloned()
            .unwrap_or_default();
        let sample = v
            .get("sampleOverCapGlobResults")
            .and_then(|v| v.as_bool())
            .ok_or("tool-fs-search: sampleOverCapGlobResults is required")?;
        let positive = |name: &str, default: u64| -> Result<u64, String> {
            match v.get(name) {
                None => Ok(default),
                Some(x) => x
                    .as_u64()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("tool-fs-search: {name} must be a positive integer")),
            }
        };
        Ok(Self {
            sample,
            glob_max: positive("globMaxResults", 100)? as usize,
            grep_max: positive("grepMaxMatches", 1000)? as usize,
            line_max: positive("grepMaxLineBytes", 2000)? as usize,
            raw_max: positive("rawOutputMaxBytes", 20_000_000)?,
            grace_ms: positive("graceMs", 3000)?,
            stderr_max: positive("stderrMaxBytes", 65536)?,
            timeout_ms: positive("timeoutMs", 30000)?,
        })
    }
}

fn output(
    render: impl Fn(
        &serde_json::Value,
        &serde_json::Value,
    ) -> Result<Vec<dsh_llm::ContentBlock>, String>
    + Send
    + Sync
    + 'static,
    schema: serde_json::Value,
) -> ToolOutputDefinition {
    ToolOutputDefinition {
        schema,
        render: Arc::new(render),
        presentation_meta: None,
    }
}
fn err(message: impl Into<String>, code: &str) -> ToolBodyError {
    ToolBodyError {
        receipt: None,
        message: message.into(),
        info: Some(dsh_tools::ToolErrorInfo {
            name: "SearchError".into(),
            code: code.into(),
        }),
    }
}
fn cwd(exec: &dsh_tools::ToolExecution) -> String {
    exec.agent
        .as_ref()
        .and_then(|a| a.session().header().cwd.clone())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .to_string_lossy()
                .into_owned()
        })
}
fn display(path: &str, workdir: &str) -> String {
    let p = Path::new(path);
    if p.is_absolute() {
        if let Ok(rel) = p.strip_prefix(workdir) {
            return if rel.as_os_str().is_empty() {
                ".".into()
            } else {
                rel.to_string_lossy().into_owned()
            };
        }
    }
    path.to_string()
}

async fn run(
    runtime: &Arc<dyn SubprocessRuntime>,
    tool: &str,
    args: Vec<String>,
    workdir: String,
    signal: SubprocessAbort,
    cfg: &Config,
) -> Result<(String, bool), ToolBodyError> {
    let started = std::time::Instant::now();
    let budget = std::time::Duration::from_millis(cfg.timeout_ms);
    let external = signal.clone();
    let bounded: SubprocessAbort = Arc::new(move || external() || started.elapsed() >= budget);
    let operation = run_command(runtime, tool, args, workdir, bounded, cfg);
    tokio::pin!(operation);
    loop {
        if signal() {
            return Err(err(format!("{tool} search cancelled"), "CANCELLED"));
        }
        if started.elapsed() >= budget {
            return Err(err(
                format!("{tool} search exceeded {} ms", cfg.timeout_ms),
                "SEARCH_TIMEOUT",
            ));
        }
        tokio::select! {
            result = &mut operation => return result,
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }
}

async fn run_command(
    runtime: &Arc<dyn SubprocessRuntime>,
    tool: &str,
    args: Vec<String>,
    workdir: String,
    signal: SubprocessAbort,
    cfg: &Config,
) -> Result<(String, bool), ToolBodyError> {
    let rg = runtime
        .resolve_executable("rg", None, Some(signal.clone()))
        .await
        .map_err(|error| {
            err(
                format!("{tool} could not resolve ripgrep: {error}. Repair the installation if runtime/search/rg is missing."),
                "SEARCH_FAILED",
            )
        })?;
    let handle = runtime
        .spawn(SubprocessSpawnSpec {
            argv: std::iter::once(rg)
                .chain(std::iter::once("--no-config".into()))
                .chain(args)
                .collect(),
            cwd: workdir,
            stdio: SubprocessStdio {
                stdin: SubprocessStdinMode::Ignore,
                stdout: SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: cfg.raw_max,
                    spill: None,
                }),
                stderr: SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: cfg.stderr_max,
                    spill: None,
                }),
            },
            grace_ms: cfg.grace_ms,
            signal: Some(signal),
            env: None,
        })
        .map_err(|error| {
            err(
                format!("{tool} could not spawn ripgrep: {error}"),
                "SEARCH_FAILED",
            )
        })?;
    let outcome = handle.done().await.map_err(|error| {
        err(
            format!("{tool} ripgrep execution failed: {error}"),
            "SEARCH_FAILED",
        )
    })?;
    let collected = handle.collected();
    let stdout = collected
        .stdout
        .ok_or_else(|| {
            err(
                format!("{tool} search command produced no collected output streams"),
                "SEARCH_FAILED",
            )
        })?
        .read_from(0);
    let stderr = collected
        .stderr
        .ok_or_else(|| {
            err(
                format!("{tool} search command produced no collected output streams"),
                "SEARCH_FAILED",
            )
        })?
        .read_from(0);
    if stdout.lossy {
        return Err(err(
            format!(
                "{tool} produced more raw output than retained within the {}-byte cap; narrow pattern, path, or include and retry",
                cfg.raw_max
            ),
            "SEARCH_RAW_OUTPUT_OVERFLOW",
        ));
    }
    match outcome.exit_code {
        Some(0) => Ok((stdout.text, false)),
        Some(1) => Ok((stdout.text, true)),
        Some(code) => {
            let e = stderr.text.trim();
            let invalid = e.contains("regex parse error") || e.contains("error parsing glob");
            Err(err(
                if invalid {
                    format!("{tool} pattern rejected by ripgrep: {e}")
                } else {
                    format!(
                        "{tool} search failed (exit {code}){}",
                        if e.is_empty() {
                            String::new()
                        } else {
                            format!(": {e}")
                        }
                    )
                },
                if invalid {
                    "SEARCH_INVALID_PATTERN"
                } else {
                    "SEARCH_FAILED"
                },
            ))
        }
        None => Err(err(
            format!(
                "{tool} search command was killed by signal {}",
                outcome.signal.unwrap_or_else(|| "(unknown)".into())
            ),
            "SEARCH_FAILED",
        )),
    }
}

struct Service {
    runtime: Arc<dyn SubprocessRuntime>,
    cfg: Config,
}
impl Service {
    fn install(ctx: &Context, cfg: Config) -> Result<Arc<Self>, String> {
        let tools = ctx
            .get_typed::<Arc<dsh_tools::ToolRuntime>>("tools", false)
            .map(|v| v.as_ref().clone())
            .ok_or("dsh-tool-fs-search requires tools")?;
        let prompt = ctx
            .get_typed::<Arc<SystemPrompt>>("systemPrompt", false)
            .map(|v| v.as_ref().clone())
            .ok_or("dsh-tool-fs-search requires systemPrompt")?;
        let runtime = ctx
            .get_typed::<Arc<dyn SubprocessRuntime>>("subprocess", false)
            .map(|v| v.as_ref().clone())
            .ok_or("dsh-tool-fs-search requires subprocess")?;
        let s = Arc::new(Self { runtime, cfg });
        prompt.section(
            ctx,
            PromptSection {
                name: "tool:glob".into(),
                order: 103.0,
                text: dsh_tools::scoped_tool_guidance(
                    ctx,
                    &["glob"],
                    "Use the glob tool — not shell find — to discover files by path pattern.",
                ),
                complete: None,
            },
        );
        prompt.section(
            ctx,
            PromptSection {
                name: "tool:grep".into(),
                order: 104.0,
                text: dsh_tools::scoped_tool_guidance(
                    ctx,
                    &["grep"],
                    "Use the grep tool — not shell grep or rg — to search file contents.",
                ),
                complete: None,
            },
        );
        tools.register(ctx, s.glob())?;
        tools.register(ctx, s.grep())?;
        Ok(s)
    }
    fn glob(self: &Arc<Self>) -> ToolDefinition {
        let s = self.clone();
        let max = s.cfg.glob_max;
        ToolDefinition {
            name: "glob".into(),
            description: format!(
                "Find files whose paths match a glob pattern. Returns at most {max} paths inline."
            ),
            parameters: serde_json::json!({"type":"object","additionalProperties":false,"properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"]}),
            output: output(
                move |_, v| {
                    let paths = v["paths"].as_array().unwrap();
                    Ok(vec![dsh_llm::ContentBlock::Text {
                        text: if paths.is_empty() {
                            "No files found".into()
                        } else {
                            let shown = paths
                                .iter()
                                .take(max)
                                .filter_map(|v| v.as_str())
                                .collect::<Vec<_>>()
                                .join("\n");
                            if paths.len() > max {
                                format!(
                                    "{shown}\n\n(Showing {max} of {} paths. The complete result could not be saved; narrow pattern or path to see more.)",
                                    paths.len()
                                )
                            } else {
                                shown
                            }
                        },
                    }])
                },
                serde_json::json!({"type":"object","additionalProperties":false,"properties":{"root":{"type":"string"},"paths":{"type":"array","items":{"type":"string"}}},"required":["root","paths"]}),
            ),
            timeout_ms: Some(s.cfg.timeout_ms),
            is_concurrency_safe: None,
            execute: Arc::new(move |a, r| {
                let s = s.clone();
                let a = a.clone();
                let e = r.execution.clone();
                Box::pin(async move {
                    let pattern = a["pattern"]
                        .as_str()
                        .ok_or_else(|| ToolBodyError::plain("pattern is required"))?;
                    if pattern.trim().is_empty() {
                        return Err(ToolBodyError::plain("pattern must be a non-empty string"));
                    }
                    let path = a.get("path").and_then(|v| v.as_str());
                    if path.is_some_and(|p| p.trim().is_empty()) {
                        return Err(ToolBodyError::plain(
                            "path must be a non-empty string when given",
                        ));
                    }
                    let mut argv = vec![
                        "--files".into(),
                        format!("--glob={pattern}"),
                        "--sort=modified".into(),
                        "--no-ignore".into(),
                        "--hidden".into(),
                    ];
                    for name in VCS {
                        argv.push(format!("--glob=!**/{name}"));
                        argv.push(format!("--glob=!**/{name}/**"));
                    }
                    if let Some(path) = path {
                        argv.extend(["--".into(), path.into()]);
                    }
                    let workdir = cwd(&e);
                    let paths = match run(
                        &s.runtime,
                        "glob",
                        argv,
                        workdir.clone(),
                        {
                            let signal = e.signal.lock().clone();
                            signal
                        },
                        &s.cfg,
                    )
                    .await
                    {
                        Ok((out, no)) => {
                            if no {
                                vec![]
                            } else {
                                out.lines()
                                    .filter(|x| !x.is_empty())
                                    .map(|x| display(x, &workdir))
                                    .collect()
                            }
                        }
                        Err(error) => return Err(error),
                    };
                    Ok(serde_json::json!({"root":path.unwrap_or("."),"paths":paths}))
                })
            }),
            finalize_content: None,
            present_call: Some(Arc::new(|a| {
                Some(ToolCallView::Generic {
                    title: format!(
                        "Glob {}{}",
                        a["pattern"].as_str().unwrap_or(""),
                        a.get("path")
                            .and_then(|v| v.as_str())
                            .map(|p| format!(" in {p}"))
                            .unwrap_or_default()
                    ),
                    kind: Some(ToolCallKind::Search),
                    raw_input: a.get("pattern").cloned(),
                    content: None,
                    locations: None,
                })
            })),
            present_result: None,
        }
    }
    fn grep(self: &Arc<Self>) -> ToolDefinition {
        let s = self.clone();
        let max = s.cfg.grep_max;
        let line_max = s.cfg.line_max;
        ToolDefinition {
            name: "grep".into(),
            description: format!(
                "Search file contents with a ripgrep regular expression. Returns the first {max} matches inline."
            ),
            parameters: serde_json::json!({"type":"object","additionalProperties":false,"properties":{"pattern":{"type":"string"},"path":{"type":"string"},"include":{"type":"string"}},"required":["pattern"]}),
            output: output(
                move |_, v| {
                    let m = v["matches"].as_array().unwrap();
                    if m.is_empty() {
                        return Ok(vec![dsh_llm::ContentBlock::Text {
                            text: "No matches found".into(),
                        }]);
                    }
                    let kept = &m[..m.len().min(max)];
                    let mut groups: BTreeMap<&str, Vec<String>> = BTreeMap::new();
                    for x in kept {
                        let mut line = x["line"].as_str().unwrap_or("").to_string();
                        if line.len() > line_max {
                            while !line.is_char_boundary(line_max) {
                                line.pop();
                            }
                            line.truncate(line_max);
                            line.push_str(" (line truncated)");
                        }
                        groups
                            .entry(x["path"].as_str().unwrap_or(""))
                            .or_default()
                            .push(format!("Line {}: {line}", x["lineNumber"]));
                    }
                    let body = groups
                        .into_iter()
                        .map(|(p, rows)| format!("{p}\n{}", rows.join("\n")))
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    let header = if m.len() > max {
                        format!("Found {max} of {} matches", m.len())
                    } else {
                        format!(
                            "Found {} {}",
                            m.len(),
                            if m.len() == 1 { "match" } else { "matches" }
                        )
                    };
                    let footer = if m.len() > max {
                        "\n\n(The complete result could not be saved; narrow pattern, path, or include to see more.)"
                    } else {
                        ""
                    };
                    Ok(vec![dsh_llm::ContentBlock::Text {
                        text: format!("{header}\n\n{body}{footer}"),
                    }])
                },
                serde_json::json!({"type":"object","additionalProperties":false,"properties":{"matches":{"type":"array","items":{"type":"object","additionalProperties":false,"properties":{"path":{"type":"string"},"lineNumber":{"type":"integer"},"line":{"type":"string"}},"required":["path","lineNumber","line"]}}},"required":["matches"]}),
            ),
            timeout_ms: Some(s.cfg.timeout_ms),
            is_concurrency_safe: None,
            execute: Arc::new(move |a, r| {
                let s = s.clone();
                let a = a.clone();
                let e = r.execution.clone();
                Box::pin(async move {
                    let pattern = a["pattern"]
                        .as_str()
                        .ok_or_else(|| ToolBodyError::plain("pattern is required"))?;
                    if pattern.is_empty() {
                        return Err(ToolBodyError::plain("pattern must be a non-empty string"));
                    }
                    let path = a.get("path").and_then(|v| v.as_str());
                    let include = a.get("include").and_then(|v| v.as_str());
                    if path.is_some_and(|p| p.trim().is_empty()) {
                        return Err(ToolBodyError::plain("path must be a non-empty string"));
                    }
                    if include.is_some_and(|p| p.trim().is_empty()) {
                        return Err(ToolBodyError::plain("include must be a non-empty glob"));
                    }
                    let mut argv = vec!["--json".into(), format!("--regexp={pattern}")];
                    if let Some(i) = include {
                        argv.push(format!("--glob={i}"));
                    }
                    if let Some(p) = path {
                        argv.extend(["--".into(), p.into()]);
                    }
                    let workdir = cwd(&e);
                    let run_result = run(
                        &s.runtime,
                        "grep",
                        argv,
                        workdir.clone(),
                        {
                            let signal = e.signal.lock().clone();
                            signal
                        },
                        &s.cfg,
                    )
                    .await;
                    let matches = match run_result {
                        Ok((_, true)) => Vec::new(),
                        Err(error) => return Err(error),
                        Ok((out, false)) => {
                            let mut parsed = Vec::new();
                            for line in out.lines() {
                                let v:serde_json::Value=serde_json::from_str(line).map_err(|_|err("grep received malformed ripgrep --json output (a line is not JSON)","SEARCH_FAILED"))?;
                                if v["type"] != "match" {
                                    continue;
                                }
                                let d = &v["data"];
                                let path=d["path"]["text"].as_str().ok_or_else(||err("grep received malformed ripgrep --json output (a match record has no path text)","SEARCH_FAILED"))?;
                                let number=d["line_number"].as_u64().ok_or_else(||err("grep received malformed ripgrep --json output (a match record has no line number)","SEARCH_FAILED"))?;
                                let text = d["lines"]["text"]
                                    .as_str()
                                    .map(|x| x.trim_end_matches(['\r', '\n']).to_string())
                                    .unwrap_or_else(|| "(line is not valid UTF-8)".into());
                                parsed.push(serde_json::json!({"path":display(path,&workdir),"lineNumber":number,"line":text}));
                            }
                            parsed
                        }
                    };
                    Ok(serde_json::json!({"matches":matches}))
                })
            }),
            finalize_content: None,
            present_call: Some(Arc::new(|a| {
                Some(ToolCallView::Generic {
                    title: format!("Grep {}", a["pattern"].as_str().unwrap_or("")),
                    kind: Some(ToolCallKind::Search),
                    raw_input: a.get("pattern").cloned(),
                    content: None,
                    locations: None,
                })
            })),
            present_result: None,
        }
    }
}

pub struct ToolFsSearchPlugin;

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use std::{
        future::Future,
        pin::Pin,
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant},
    };
    type Fut<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
    struct HungResolver;
    impl SubprocessRuntime for HungResolver {
        fn resolve_executable(
            &self,
            _: &str,
            _: Option<&[(String, String)]>,
            _: Option<SubprocessAbort>,
        ) -> Fut<Result<String, String>> {
            Box::pin(std::future::pending())
        }
        fn spawn(
            &self,
            _: SubprocessSpawnSpec,
        ) -> Result<Arc<dyn dsh_subprocess::SubprocessHandle>, String> {
            panic!("cancelled resolution must not spawn")
        }
        fn spawn_terminal(
            &self,
            _: dsh_subprocess::SubprocessTerminalSpawnSpec,
        ) -> Fut<Result<Arc<dyn dsh_subprocess::SubprocessTerminalHandle>, String>> {
            panic!("search never opens a terminal")
        }
    }
    fn config(timeout_ms: u64) -> Config {
        Config {
            sample: false,
            glob_max: 100,
            grep_max: 100,
            line_max: 2000,
            raw_max: 10000,
            grace_ms: 100,
            stderr_max: 10000,
            timeout_ms,
        }
    }
    #[tokio::test]
    async fn hung_executable_resolution_is_cancelled_promptly() {
        let runtime: Arc<dyn SubprocessRuntime> = Arc::new(HungResolver);
        let flag = Arc::new(AtomicBool::new(false));
        let setter = flag.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(25)).await;
            setter.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let error = run(
            &runtime,
            "grep",
            vec![],
            ".".into(),
            Arc::new(move || flag.load(Ordering::SeqCst)),
            &config(5000),
        )
        .await
        .unwrap_err();
        assert_eq!(error.info.unwrap().code, "CANCELLED");
        assert!(started.elapsed() < Duration::from_secs(1));
    }
    #[tokio::test]
    async fn hung_executable_resolution_has_a_total_deadline() {
        let runtime: Arc<dyn SubprocessRuntime> = Arc::new(HungResolver);
        let error = run(
            &runtime,
            "glob",
            vec![],
            ".".into(),
            Arc::new(|| false),
            &config(30),
        )
        .await
        .unwrap_err();
        assert_eq!(error.info.unwrap().code, "SEARCH_TIMEOUT");
    }
}
#[async_trait::async_trait]
impl Plugin for ToolFsSearchPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }
    fn inject(&self) -> InjectSpec {
        InjectSpec::new(INJECT)
    }
    async fn apply(&self, ctx: &Context, value: ArcValue) -> Result<(), PluginError> {
        let cfg = Config::parse(&value).map_err(|e| PluginError::from(anyhow::anyhow!(e)))?;
        Service::install(ctx, cfg)
            .map(|_| ())
            .map_err(|e| PluginError::from(anyhow::anyhow!(e)))
    }
}
