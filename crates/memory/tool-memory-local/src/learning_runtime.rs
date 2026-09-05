//! Observe registry-owned final results without retaining model arguments or tool output.
use crate::learning::{
    FailureObservation, LearningStore, RecoveryObservation, digest, workspace_key,
};
use cordis::{Context, EventOptions, Listener, downcast_arc};
use dsh_tools::{ToolExecution, ToolExecutionResult};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

struct LimitedHash {
    digest: Sha256,
    bytes: usize,
}
impl Write for LimitedHash {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.saturating_add(bytes.len()) > 65536 {
            return Err(std::io::Error::other("fingerprint budget"));
        }
        self.digest.update(bytes);
        self.bytes += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub fn argument_fingerprint(arguments: &Value) -> Option<String> {
    let mut writer = LimitedHash {
        digest: Sha256::new(),
        bytes: 0,
    };
    serde_json::to_writer(&mut writer, arguments).ok()?;
    Some(format!("{:x}", writer.digest.finalize()))
}
fn normalized_path(cwd: &str, path: &str) -> String {
    let path = Path::new(path);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(cwd).join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    workspace_key(&normalized.to_string_lossy())
}
pub fn resource_fingerprint(cwd: &str, tool: &str, arguments: &Value) -> Option<String> {
    let mut parts = vec![tool.to_string()];
    let mut target = false;
    for key in [
        "file_path",
        "path",
        "directory",
        "cwd",
        "command",
        "url",
        "query",
        "pattern",
        "action",
        "operation",
    ] {
        if let Some(value) = arguments.get(key).and_then(Value::as_str) {
            if value.is_empty() || value.len() > 4096 {
                return None;
            }
            if !matches!(key, "action" | "operation") {
                target = true;
            }
            let fingerprint = if matches!(key, "file_path" | "path" | "directory" | "cwd") {
                normalized_path(cwd, value)
            } else {
                digest(value.as_bytes())
            };
            parts.push(format!("{key}:{fingerprint}"));
        }
    }
    target.then(|| digest(parts.join("\0").as_bytes()))
}

pub fn install(ctx: &Context, store: Arc<LearningStore>) -> Result<(), String> {
    store.start_worker()?;
    let shutdown = store.clone();
    let _ = ctx.effect(
        "learning-ledger",
        Box::pin(async move {
            Some(cordis::make_disposer(move || {
                let store = shutdown.clone();
                Box::pin(async move {
                    store.shutdown().await;
                })
            }))
        }),
    );
    let listener: Arc<Listener> = Arc::new(move |_, args| {
        let execution = args
            .first()
            .and_then(|value| downcast_arc::<Arc<ToolExecution>>(value))
            .map(|value| value.as_ref().clone());
        let result = args
            .get(1)
            .and_then(|value| downcast_arc::<Arc<ToolExecutionResult>>(value))
            .map(|value| value.as_ref().clone());
        let store = store.clone();
        Box::pin(async move {
            let (Some(execution), Some(result)) = (execution, result) else {
                return None;
            };
            let Some(agent) = execution.agent.as_ref() else {
                return None;
            };
            let Some(cwd) = agent
                .session()
                .header()
                .cwd
                .as_deref()
                .filter(|cwd| !cwd.trim().is_empty())
            else {
                return None;
            };
            if !store.enabled() {
                return None;
            }
            let argument_fingerprint = argument_fingerprint(&execution.arguments);
            let resource_fingerprint =
                resource_fingerprint(cwd, &execution.name, &execution.arguments);
            if result.is_error {
                let context = agent.session().request_context();
                let provider = context
                    .as_ref()
                    .map(|context| context.provider.clone())
                    .or_else(|| agent.options().provider.clone())
                    .unwrap_or_default();
                let model = context
                    .as_ref()
                    .map(|context| context.model.clone())
                    .or_else(|| agent.options().model.clone())
                    .unwrap_or_default();
                let _ = store.enqueue_failure(FailureObservation {
                    workspace_key: workspace_key(cwd),
                    session_id: agent.id().as_str().into(),
                    provider,
                    model,
                    tool: execution.name.clone(),
                    source: "tool".into(),
                    code: result
                        .error
                        .as_ref()
                        .and_then(|error| error.info.as_ref())
                        .map(|info| info.code.clone())
                        .unwrap_or_else(|| "TOOL_ERROR".into()),
                    message: String::new(),
                    call_id: execution.call_id.as_str().into(),
                    argument_fingerprint,
                    resource_fingerprint,
                });
            } else if let Some(argument_fingerprint) = argument_fingerprint {
                let _ = store.enqueue_recovery(RecoveryObservation {
                    workspace_key: workspace_key(cwd),
                    session_id: agent.id().as_str().into(),
                    tool: execution.name.clone(),
                    call_id: execution.call_id.as_str().into(),
                    argument_fingerprint,
                    resource_fingerprint,
                });
            }
            None
        })
    });
    futures::executor::block_on(ctx.on(
        "tools/result",
        listener,
        EventOptions::default().global(true),
    ));
    Ok(())
}
