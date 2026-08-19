//! Newline-delimited JSON-RPC 2.0 dispatcher for stdio runtimes.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::Arc;

use serde_json::{Value, json};

pub type JsonRpcHandler =
    Arc<dyn Fn(Value) -> Result<Value, JsonRpcHandlerError> + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq)]
pub struct JsonRpcHandlerError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl JsonRpcHandlerError {
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
            data: None,
        }
    }
}

#[derive(Default)]
pub struct JsonRpcRuntime {
    handlers: HashMap<String, JsonRpcHandler>,
}

impl JsonRpcRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        method: impl Into<String>,
        handler: JsonRpcHandler,
    ) -> Result<(), String> {
        let method = method.into();
        if method.is_empty() {
            return Err("JSON-RPC method must not be empty".to_string());
        }
        if self.handlers.insert(method.clone(), handler).is_some() {
            return Err(format!("JSON-RPC method {method:?} is already registered"));
        }
        Ok(())
    }

    /// Process one peer frame. Notifications and malformed lines produce no
    /// output; requests always preserve the exact string/number/null id.
    pub fn dispatch_line(&self, line: &str) -> Option<Value> {
        let frame: Value = serde_json::from_str(line).ok()?;
        let object = frame.as_object()?;
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return object
                .get("id")
                .map(|id| error_frame(id.clone(), -32600, "invalid JSON-RPC request", None));
        }
        let method = object.get("method").and_then(Value::as_str)?;
        let id = object.get("id").cloned()?;
        if !matches!(id, Value::String(_) | Value::Number(_) | Value::Null) {
            return Some(error_frame(
                Value::Null,
                -32600,
                "invalid JSON-RPC request id",
                None,
            ));
        }
        let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(handler) = self.handlers.get(method) else {
            return Some(error_frame(
                id,
                -32601,
                &format!("method not found: {method}"),
                None,
            ));
        };
        match handler(params) {
            Ok(result) => Some(json!({ "jsonrpc": "2.0", "id": id, "result": result })),
            Err(error) => Some(error_frame(id, error.code, &error.message, error.data)),
        }
    }

    /// Drain NDJSON until EOF. Every response is emitted as exactly one JSON
    /// line and flushed before reading the next request.
    pub fn serve<R: BufRead, W: Write>(&self, mut input: R, mut output: W) -> std::io::Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            if input.read_line(&mut line)? == 0 {
                return Ok(());
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some(response) = self.dispatch_line(trimmed) else {
                continue;
            };
            serde_json::to_writer(&mut output, &response).map_err(std::io::Error::other)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

fn error_frame(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = serde_json::Map::new();
    error.insert("code".to_string(), json!(code));
    error.insert("message".to_string(), json!(message));
    if let Some(data) = data {
        error.insert("data".to_string(), data);
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}
