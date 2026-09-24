//! Headless argument contract and bounded committed-event projection.
use serde_json::{Value, json};
use std::io::Write;

pub const HELP: &str = "Usage: dsh --profile headless [--json] [--session-id <id>] [task...]\nAnswer one task and exit. Use - or piped stdin for the task.\n--json writes NDJSON; --session-id resumes an existing session.\n";
#[derive(Debug, Default, PartialEq)]
pub struct Options {
    pub task: Option<String>,
    pub session_id: Option<String>,
    pub json: bool,
    pub help: bool,
}
pub fn json_requested(args: &[String]) -> bool {
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        match arg.as_str() {
            "--" => break,
            "--json" => return true,
            "--session-id" => i += 1,
            _ => {}
        }
        i += 1;
    }
    false
}
pub fn parse(args: &[String], tty: bool) -> Result<Options, String> {
    let mut options = Options::default();
    let mut words = Vec::new();
    let mut flags = true;
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        match arg.as_str() {
            "--" if flags => flags = false,
            "--json" if flags => options.json = true,
            "--help" | "-h" if flags => options.help = true,
            "--session-id" if flags => {
                i += 1;
                let value = args.get(i).ok_or("dsh: --session-id needs a value")?;
                if value.trim().is_empty() {
                    return Err("dsh: --session-id requires a non-empty session id".into());
                }
                if options.session_id.replace(value.clone()).is_some() {
                    return Err("dsh: --session-id may only be specified once".into());
                }
            }
            _ if flags && arg.starts_with("--session-id=") => {
                let value = &arg[13..];
                if value.trim().is_empty() {
                    return Err("dsh: --session-id requires a non-empty session id".into());
                }
                if options.session_id.replace(value.into()).is_some() {
                    return Err("dsh: --session-id may only be specified once".into());
                }
            }
            _ if flags && arg.starts_with('-') && arg != "-" => {
                return Err(format!("dsh: unknown headless option {arg}"));
            }
            _ => words.push(arg.clone()),
        }
        i += 1;
    }
    if options.help {
        return Ok(options);
    }
    if words.len() > 1 && words.iter().any(|word| word == "-") {
        return Err("dsh: `-` must be the only task argument".into());
    }
    if words.is_empty() || words == ["-"] {
        if words.is_empty() && tty {
            return Err("dsh: a headless task is required".into());
        }
    } else {
        let task = words.join(" ");
        if task.trim().is_empty() {
            return Err("dsh: a headless task is required".into());
        }
        options.task = Some(task);
    }
    Ok(options)
}
fn text_bound(text: &str, truncated: &mut bool) -> String {
    let mut end = text.len().min(8192);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    *truncated |= end < text.len();
    text[..end].to_owned()
}
fn bound(value: &Value, depth: usize, truncated: &mut bool) -> Value {
    match value {
        Value::String(s) => Value::String(text_bound(s, truncated)),
        Value::Array(_) | Value::Object(_) if depth >= 64 => {
            *truncated = true;
            json!("[truncated: depth]")
        }
        Value::Array(a) => Value::Array(a.iter().map(|v| bound(v, depth + 1, truncated)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (text_bound(k, truncated), bound(v, depth + 1, truncated)))
                .collect(),
        ),
        _ => value.clone(),
    }
}
pub fn line(value: &Value) -> String {
    let mut truncated = false;
    let mut value = bound(value, 0, &mut truncated);
    if truncated {
        value["truncated"] = json!(true);
    }
    let full = value.to_string();
    if full.len() < 32768 {
        return full;
    }
    let mut scalar: serde_json::Map<String, Value> = value
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, v)| !v.is_object() && !v.is_array())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    scalar.insert("truncated".into(), json!(true));
    let short = Value::Object(scalar).to_string();
    if short.len() < 32768 {
        short
    } else {
        json!({"type":value["type"],"truncated":true}).to_string()
    }
}
pub fn write_event(value: &Value, lossless: bool) -> Result<(), String> {
    let text = if lossless {
        value.to_string()
    } else {
        line(value)
    };
    let stdout = std::io::stdout();
    let mut sink = stdout.lock();
    writeln!(sink, "{text}")
        .and_then(|_| sink.flush())
        .map_err(|e| format!("dsh: stdout: {e}"))
}
#[derive(Default)]
pub struct Projection {
    usage: Option<Value>,
    usage_incomplete: bool,
    attempt_usage: Option<Value>,
    attempt_open: bool,
    finished_attempt: bool,
}
impl Projection {
    fn usage(&mut self, next: Option<&Value>) {
        let Some(next) = next.filter(|v| v.is_object()) else {
            self.usage_incomplete = true;
            return;
        };
        if let Some(total) = &mut self.usage {
            let Some(object) = total.as_object_mut() else {
                return;
            };
            object.retain(|key, value| match (value.as_u64(), next[key].as_u64()) {
                (Some(a), Some(b)) => {
                    if let Some(sum) = a.checked_add(b) {
                        *value = json!(sum);
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            });
        } else {
            self.usage = Some(next.clone());
        }
    }
    pub fn project(&mut self, event: &dsh_session::SessionEvent) -> Vec<Value> {
        let d = &event.data;
        match event.type_.as_str() {
            "turn/start" => vec![json!({"type":"status","phase":"turn_start","turn":d["turn"]})],
            "step/start" => {
                *self = Self::default();
                vec![
                    json!({"type":"status","phase":"step_start","turn":d["turn"],"step":d["step"]}),
                ]
            }
            "assistant/chunk" => {
                let chunk = &d["chunk"];
                if chunk["type"] == "finish" {
                    let usage = self.attempt_usage.take();
                    self.usage(usage.as_ref());
                    self.attempt_open = false;
                    self.finished_attempt = true;
                } else {
                    self.attempt_open = true;
                    if chunk["type"] == "usage" {
                        self.attempt_usage = chunk.get("usage").cloned();
                    }
                }
                vec![]
            }
            "assistant/attempt" => {
                self.usage(d.get("usage"));
                vec![]
            }
            "assistant/message" => {
                if self.attempt_open || !self.finished_attempt {
                    self.usage(d.get("usage"));
                }
                self.attempt_open = false;
                self.attempt_usage = None;
                d["message"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|b| {
                        let kind = match b["type"].as_str()? {
                            "text" => "text",
                            "reasoning" => "thinking",
                            _ => return None,
                        };
                        Some(json!({"type":kind,"text":b["text"]}))
                    })
                    .collect()
            }
            "step/end" => {
                if self.attempt_open {
                    self.usage_incomplete = true;
                }
                let mut v =
                    json!({"type":"status","phase":"step_end","turn":d["turn"],"step":d["step"]});
                if !self.usage_incomplete {
                    if let Some(usage) = self.usage.take() {
                        v["usage"] = usage;
                    }
                }
                vec![v]
            }
            "turn/end" => vec![
                json!({"type":"status","phase":"turn_end","turn":d["turn"],"reason":d["reason"]}),
            ],
            "tool/call" => {
                let raw = d["arguments"].as_str().unwrap_or("");
                let input = if raw.is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(raw).unwrap_or_else(|_| json!(raw))
                };
                vec![
                    json!({"type":"tool_call","callId":d["callId"],"tool":d["name"],"input":input}),
                ]
            }
            "tool/result" if event.surface_op == Some(dsh_session::SurfaceOp::Append) => {
                let m = &d["message"];
                let text = m["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect::<String>();
                vec![
                    json!({"type":"tool_result","callId":m["toolCallId"],"status":if m["isError"]==true{"error"}else{"completed"},"result":text}),
                ]
            }
            _ => vec![],
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn argument_contract_and_literal_json() {
        assert_eq!(
            parse(&args(&["one", "two"]), true).unwrap().task.as_deref(),
            Some("one two")
        );
        assert!(parse(&[], true).is_err());
        assert_eq!(parse(&[], false).unwrap().task, None);
        assert!(parse(&args(&["-", "two"]), false).is_err());
        assert!(parse(&args(&[" "]), false).is_err());
        assert_eq!(
            parse(&args(&["--session-id", " x ", "task"]), true)
                .unwrap()
                .session_id
                .as_deref(),
            Some(" x ")
        );
        assert!(!json_requested(&args(&["--session-id", "--json", "task"])));
        assert!(!json_requested(&args(&["--", "--json"])));
        assert!(json_requested(&args(&["--json", "--", "--json"])));
        assert!(parse(&args(&["--help"]), true).unwrap().help);
    }
    fn event(kind: &str, data: Value) -> dsh_session::SessionEvent {
        serde_json::from_value(json!({"type":kind,"seq":0,"time":0,"data":data})).unwrap()
    }
    #[test]
    fn only_committed_text_is_projected_and_partial_usage_is_not_reported() {
        let mut projection = Projection::default();
        projection.project(&event("step/start", json!({"turn":1,"step":1})));
        assert!(
            projection
                .project(&event(
                    "assistant/chunk",
                    json!({"text":"discarded attempt"})
                ))
                .is_empty()
        );
        assert!(
            projection
                .project(&event("assistant/attempt", json!({})))
                .is_empty()
        );
        let lines=projection.project(&event("assistant/message",json!({"message":{"content":[{"type":"reasoning","text":"committed reason"},{"type":"text","text":"committed answer"}]},"usage":{"inputTokens":3,"outputTokens":4}})));
        assert_eq!(lines[0]["type"], "thinking");
        assert_eq!(lines[1]["text"], "committed answer");
        assert!(
            projection.project(&event("step/end", json!({"turn":1,"step":1})))[0]
                .get("usage")
                .is_none()
        );
        projection.project(&event("step/start", json!({"turn":1,"step":2})));
        projection.project(&event(
            "assistant/attempt",
            json!({"usage":{"inputTokens":2,"outputTokens":1,"cacheReadTokens":1}}),
        ));
        projection.project(&event(
            "assistant/message",
            json!({"usage":{"inputTokens":3,"outputTokens":4}}),
        ));
        let end = projection.project(&event("step/end", json!({"turn":1,"step":2})));
        assert_eq!(end[0]["usage"], json!({"inputTokens":5,"outputTokens":5}));
        assert!(
            projection
                .project(&event("tool/result", json!({"message":{"content":[]}})))
                .is_empty()
        );
    }
    #[test]
    fn native_retry_chunks_sum_usage_without_leaking_or_double_counting_text() {
        let mut p = Projection::default();
        for _ in 0..2 {
            assert!(
                p.project(&event(
                    "assistant/chunk",
                    json!({"chunk":{"type":"text-delta","text":"attempt fragment"}})
                ))
                .is_empty()
            );
            p.project(&event(
                "assistant/chunk",
                json!({"chunk":{"type":"usage","usage":{"inputTokens":3,"outputTokens":2}}}),
            ));
            p.project(&event(
                "assistant/chunk",
                json!({"chunk":{"type":"finish"}}),
            ));
        }
        p.project(&event(
            "assistant/message",
            json!({"message":{"content":[]},"usage":{"inputTokens":3,"outputTokens":2}}),
        ));
        assert_eq!(
            p.project(&event("step/end", json!({})))[0]["usage"],
            json!({"inputTokens":6,"outputTokens":4})
        );
    }
    #[test]
    fn bounded_unicode_depth_and_whole_line() {
        let event = json!({"type":"text","text":"中".repeat(6000)});
        let output = line(&event);
        let v: Value = serde_json::from_str(&output).unwrap();
        assert!(output.len() + 1 <= 32768);
        assert_eq!(v["truncated"], true);
        assert!(v["text"].as_str().unwrap().len() <= 8192);
        let event = json!({"type":"tool_call","input":vec!["x".repeat(8000);20]});
        assert!(line(&event).len() + 1 <= 32768);
        let mut deep = json!(0);
        for _ in 0..100 {
            deep = json!([deep]);
        }
        assert!(line(&json!({"type":"tool_call","input":deep})).contains("truncated"));
    }
}
