//! Bounded, read-only diagnostics for the optional TypeScript execution tool.
use dsh_subprocess::{
    SubprocessCollect, SubprocessHandle, SubprocessOutputMode, SubprocessRuntime,
    SubprocessSpawnSpec, SubprocessStdinMode, SubprocessStdio,
};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};

const PROBE: &str = r#"const {Worker}=require('node:worker_threads');const {stripTypeScriptTypes}=require('node:module');const typed=typeof stripTypeScriptTypes==='function'&&stripTypeScriptTypes('const value: number = 1',{mode:'strip'}).includes('value');const worker=new Worker('require("node:worker_threads").parentPort.postMessage(true)',{eval:true});worker.once('message',ok=>process.stdout.write(JSON.stringify({permission:!!process.permission,typescriptStrip:typed,worker:ok===true})));worker.once('error',()=>process.exit(2));"#;

pub(super) fn configured_command(directory: &Path) -> String {
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    [directory.join(name), directory.join("bin").join(name)]
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "node".into())
}

struct Guard(Arc<dyn SubprocessHandle>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.terminate();
    }
}

async fn output(
    runtime: &Arc<dyn SubprocessRuntime>,
    path: &str,
    cwd: &Path,
    args: &[&str],
) -> Result<(Option<i32>, String), String> {
    let child = runtime
        .spawn(SubprocessSpawnSpec {
            argv: std::iter::once(path.to_string())
                .chain(args.iter().map(|s| s.to_string()))
                .collect(),
            cwd: cwd.to_string_lossy().into_owned(),
            stdio: SubprocessStdio {
                stdin: SubprocessStdinMode::Ignore,
                stdout: SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: 4096,
                    spill: None,
                }),
                stderr: SubprocessOutputMode::Collect(SubprocessCollect {
                    max_bytes: 1,
                    spill: None,
                }),
            },
            grace_ms: 300,
            signal: None,
            env: Some(vec![("NODE_OPTIONS".into(), None)]),
        })
        .map_err(|_| "Node 运行程序无法启动".to_string())?;
    let guard = Guard(child.clone());
    let result = child
        .done()
        .await
        .map_err(|_| "Node 状态检查失败".to_string())?;
    let read = child
        .collected()
        .stdout
        .map(|r| r.read_from(0))
        .ok_or("Node 未返回状态")?;
    if read.lossy {
        return Err("Node 状态输出超过限制".into());
    }
    drop(guard);
    Ok((result.exit_code, read.text))
}

pub(super) async fn probe(runtime: Arc<dyn SubprocessRuntime>, command: &str, cwd: &Path) -> Value {
    probe_with_timeout(runtime, command, cwd, Duration::from_secs(5)).await
}

async fn probe_with_timeout(
    runtime: Arc<dyn SubprocessRuntime>,
    command: &str,
    cwd: &Path,
    timeout: Duration,
) -> Value {
    let mut result = json!({"command":command,"source":if command=="node"{"path"}else{"environment"},"path":Value::Null,"version":Value::Null,"status":"missing","available":false,"capabilities":Value::Null,"error":Value::Null});
    let work = async {
        let path = runtime
            .resolve_executable(command, None, None)
            .await
            .map_err(|_| "未找到 Node；普通会话可继续使用，代码模式需要兼容的 Node".to_string())?;
        result["path"] = json!(path);
        result["status"] = json!("error");
        let (code, version) = output(&runtime, &path, cwd, &["--version"]).await?;
        let version = version.trim();
        if code != Some(0) || !valid_version(version) {
            return Err("所选程序不是可识别的 Node 运行时".into());
        }
        result["version"] = json!(version);
        let (code, raw) = output(
            &runtime,
            &path,
            cwd,
            &[
                "--permission",
                "--allow-worker",
                "--no-addons",
                "--eval",
                PROBE,
            ],
        )
        .await?;
        result["status"] = json!("incompatible");
        if code != Some(0) {
            return Err("Node 不支持所需权限模型或 TypeScript 执行能力".into());
        }
        let capabilities: Value = serde_json::from_str(raw.trim())
            .map_err(|_| "Node 能力检查未返回有效结果".to_string())?;
        result["capabilities"] = capabilities.clone();
        if ["permission", "typescriptStrip", "worker"]
            .iter()
            .any(|key| capabilities[*key] != true)
        {
            return Err("Node 缺少代码模式所需能力".into());
        }
        result["available"] = json!(true);
        result["status"] = json!("ready");
        Ok::<(), String>(())
    };
    match tokio::time::timeout(timeout, work).await {
        Ok(Ok(())) => (),
        Ok(Err(error)) => result["error"] = json!(error),
        Err(_) => {
            result["status"] = json!("timeout");
            result["error"] = json!("Node 检测超时；普通会话不受影响");
        }
    }
    result
}

fn valid_version(value: &str) -> bool {
    let Some(value) = value.strip_prefix('v') else {
        return false;
    };
    let core = value.split('-').next().unwrap_or("");
    let parts: Vec<_> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
#[path = "runtime_paths_node_tests.rs"]
mod tests;
