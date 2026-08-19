//! ACP stdio service contract. The first tracer exposes protocol handshake and
//! fresh session allocation; prompt driving is layered after Agent ownership.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use dsh_jsonrpc_runtime::{JsonRpcHandlerError, JsonRpcRuntime};
use parking_lot::Mutex;
use serde_json::{Value, json};

pub const PROTOCOL_VERSION: u64 = 1;

#[derive(Default)]
pub struct AcpSessions {
    sessions: Mutex<HashMap<String, String>>,
}

impl AcpSessions {
    pub fn contains(&self, id: &str) -> bool {
        self.sessions.lock().contains_key(id)
    }
}

pub fn runtime(sessions: Arc<AcpSessions>) -> JsonRpcRuntime {
    let mut runtime = JsonRpcRuntime::new();
    runtime
        .register(
            "initialize",
            Arc::new(|_params| {
                Ok(json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "agentInfo": { "name": "deepseek-harness-acp", "version": "0.1.0" },
                    "agentCapabilities": {
                        "promptCapabilities": {
                            "image": false,
                            "audio": false,
                            "embeddedContext": false
                        }
                    },
                    "authMethods": []
                }))
            }),
        )
        .expect("initialize once");
    let sessions_for_new = sessions.clone();
    runtime
        .register(
            "session/new",
            Arc::new(move |params: Value| {
                let cwd = params
                    .get("cwd")
                    .and_then(Value::as_str)
                    .filter(|cwd| Path::new(cwd).is_absolute())
                    .ok_or_else(|| invalid_params("cwd must be an absolute path"))?;
                if params
                    .get("mcpServers")
                    .and_then(Value::as_array)
                    .is_some_and(|servers| !servers.is_empty())
                {
                    return Err(invalid_params("mcpServers is not supported"));
                }
                if params
                    .get("additionalDirectories")
                    .and_then(Value::as_array)
                    .is_some_and(|directories| !directories.is_empty())
                {
                    return Err(invalid_params("additionalDirectories is not supported"));
                }
                let id = uuid::Uuid::new_v4().to_string();
                sessions_for_new
                    .sessions
                    .lock()
                    .insert(id.clone(), cwd.to_string());
                Ok(json!({ "sessionId": id }))
            }),
        )
        .expect("session/new once");
    runtime
}

fn invalid_params(message: &str) -> JsonRpcHandlerError {
    JsonRpcHandlerError {
        code: -32602,
        message: message.to_string(),
        data: None,
    }
}
