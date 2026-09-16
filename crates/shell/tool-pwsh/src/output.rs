use dsh_shell::{CollectedOutput, ShellRunResult};
use serde_json::{Value, json};

fn stream_json(stream: &CollectedOutput, total_bytes: u64) -> Value {
    json!({
        "preview": stream.text,
        "truncated": stream.truncated,
        "totalBytes": total_bytes,
        "retainedRange": if stream.truncated { "tail" } else { "all" },
        "spillPath": stream.spill_path,
        "complete": !stream.truncated || stream.spill_path.is_some(),
        "lostPrefix": stream.truncated && stream.spill_path.is_none(),
    })
}

pub(crate) fn output_text(result: &ShellRunResult) -> String {
    let mut text = result.stdout.text.clone();
    if !result.stderr.text.is_empty() {
        text.push_str("\n[stderr]\n");
        text.push_str(&result.stderr.text);
    }
    for (name, stream) in [("stdout", &result.stdout), ("stderr", &result.stderr)] {
        if stream.truncated {
            match &stream.spill_path {
                Some(path) => text.push_str(&format!(
                    "\n[{name} preview truncated; complete stream: {path}]"
                )),
                None => text.push_str(&format!(
                    "\n[{name} truncated; complete=false; prefix no longer retained]"
                )),
            }
        }
    }
    for diagnostic in dsh_shell::application_diagnostics(result.exit_code, &result.stderr.text) {
        text.push_str(&format!(
            "\n[diagnostic: {} (suspected; application stderr, not a confirmed sandbox decision)]",
            diagnostic.category
        ));
    }
    text
}

pub(crate) fn result_json(result: &ShellRunResult, execution_id: &str) -> Value {
    let completion = if result.aborted
        || result.timed_out
        || result.signal.is_some()
        || result.exit_code.is_some_and(|code| code != 0)
    {
        "failed"
    } else if result.exit_code == Some(0) {
        "succeeded"
    } else {
        "unknown"
    };
    json!({
        "kind": "foreground",
        "executionId": execution_id,
        "executionContextId": result.execution_context_id,
        "executable": result.executable,
        "phase": "finished",
        "processState": if result.aborted { "cancelled" } else if result.timed_out { "timed_out" } else { "exited" },
        "exitCode": result.exit_code,
        "signal": result.signal,
        "commandStarted": true,
        "completion": completion,
        "effects": "possible",
        "retryAdvice": if completion == "succeeded" { "none" } else { "diagnose" },
        "stdout": output_text(result),
        "streams": {
            "stdout": stream_json(&result.stdout, result.stdout_total_bytes),
            "stderr": stream_json(&result.stderr, result.stderr_total_bytes),
        },
        "diagnostics": dsh_shell::application_diagnostics(result.exit_code, &result.stderr.text).iter().map(|entry| json!({
            "category":entry.category, "confidence":entry.confidence, "source":entry.source,
        })).collect::<Vec<_>>(),
    })
}

pub(crate) fn render_result(value: &Value) -> String {
    format!(
        "{}\n[execution: {}; completion: {}; exit: {}; effects: {}]",
        value["stdout"].as_str().unwrap_or_default(),
        value["executionId"].as_str().unwrap_or_default(),
        value["completion"].as_str().unwrap_or("unknown"),
        value["exitCode"],
        value["effects"].as_str().unwrap_or("possible")
    )
}

pub(crate) fn schema() -> Value {
    json!({"type":"object", "properties": {
        "kind":{"type":"string"}, "jobId":{"type":"string"},
        "executionId":{"type":"string"}, "executionContextId":{"oneOf":[{"type":"string"},{"type":"null"}]},
        "executable":{"type":"string"}, "phase":{"type":"string"},
        "processState":{"type":"string"}, "exitCode":{"oneOf":[{"type":"integer"},{"type":"null"}]},
        "signal":{"oneOf":[{"type":"string"},{"type":"null"}]}, "commandStarted":{"type":"boolean"},
        "completion":{"type":"string"}, "effects":{"type":"string"}, "retryAdvice":{"type":"string"},
        "stdout":{"type":"string"}, "streams":{"type":"object"}, "diagnostics":{"type":"array"},
        "steps":{"type":"array"}
    }, "required":["kind"]})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn truncation_reports_recoverability_without_inventing_full_logs() {
        let output = CollectedOutput {
            text: "tail".into(),
            truncated: true,
            spill_path: None,
        };
        assert_eq!(stream_json(&output, 100)["complete"], false);
        let output = CollectedOutput {
            spill_path: Some("private/owned-spill".into()),
            ..output
        };
        assert_eq!(stream_json(&output, 100)["complete"], true);
        assert_eq!(stream_json(&output, 100)["totalBytes"], 100);
    }
}
