use std::io;
use std::sync::Arc;

use dsh_jsonrpc_runtime::JsonRpcRuntime;
use serde_json::{Value, json};

fn main() {
    let mut runtime = JsonRpcRuntime::new();
    runtime
        .register(
            "echo",
            Arc::new(|params: Value| {
                Ok(json!({
                    "text": params.get("text").and_then(Value::as_str).unwrap_or_default()
                }))
            }),
        )
        .expect("register echo");
    runtime
        .serve(io::BufReader::new(io::stdin().lock()), io::stdout().lock())
        .expect("serve stdio");
}
