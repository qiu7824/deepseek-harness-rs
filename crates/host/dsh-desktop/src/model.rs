use crate::markdown::{Block, parse};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
#[derive(Clone, Default)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub group: String,
    pub updated: i64,
    pub running: bool,
    pub values: Value,
    pub cwd: String,
    pub preset: String,
}
#[derive(Clone)]
pub struct Message {
    pub id: String,
    pub role: String,
    pub text: String,
    pub blocks: Vec<Block>,
    pub collapsed: bool,
    pub error: bool,
}
#[derive(Clone, Default)]
pub struct Data {
    pub sessions: Vec<Session>,
    pub selected: String,
    pub messages: Vec<Arc<Message>>,
    pub more: bool,
    pub first: Option<i64>,
    pub error: Option<String>,
}
pub fn str_at(v: &Value, p: &str) -> String {
    v.pointer(p)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}
pub fn rpc(base: &str, method: &str, payload: Value) -> Result<Value, String> {
    let parsed: ureq::http::Uri = base.parse().map_err(|_| "无效服务地址")?;
    if parsed.scheme_str() != Some("http")
        || !matches!(parsed.host(), Some("127.0.0.1" | "localhost" | "[::1]"))
    {
        return Err("桌面客户端只连接本机 Host".into());
    }
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .build()
        .new_agent();
    let result:Value=agent.post(format!("{}/api/{method}",base.trim_end_matches('/')))
  .send_json(json!({"type":"client-request","rpcId":"native-desktop","method":method,"payload":payload})).map_err(|e|e.to_string())?
  .body_mut().with_config().limit(16*1024*1024).read_json().map_err(|e|e.to_string())?;
    if result.pointer("/result/ok").and_then(Value::as_bool) != Some(true) {
        return Err(format!(
            "{method}: {}",
            result
                .pointer("/result/error/message")
                .unwrap_or(&Value::Null)
        ));
    }
    Ok(result
        .pointer("/result/value")
        .cloned()
        .unwrap_or(Value::Null))
}
pub fn sessions(root: &Value) -> Vec<Session> {
    root.as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            let values = s
                .pointer("/projections/values")
                .cloned()
                .unwrap_or(Value::Null);
            let cwd = str_at(s, "/cwd");
            let title = str_at(&values, "/title");
            Session {
                id: str_at(s, "/sessionId"),
                title: if title.is_empty() {
                    "新会话".into()
                } else {
                    title
                },
                group: std::path::Path::new(&cwd)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or("未分组".into()),
                cwd,
                preset: str_at(s, "/agentPreset"),
                values,
                updated: s["updatedAt"].as_i64().unwrap_or(0),
                running: s["running"].as_bool().unwrap_or(false),
            }
        })
        .collect()
}
fn parts(v: &Value, kind: &str) -> String {
    v.as_array()
        .into_iter()
        .flatten()
        .filter(|x| x["type"].as_str() == Some(kind))
        .filter_map(|x| x["text"].as_str().or_else(|| x["reasoning"].as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}
pub fn messages(history: &Value) -> Vec<Arc<Message>> {
    let mut out: Vec<Message> = Vec::new();
    let mut calls: HashMap<String, usize> = HashMap::new();
    for item in history["events"].as_array().into_iter().flatten() {
        let e = &item["event"];
        let d = &e["data"];
        let ty = e["type"].as_str().unwrap_or("");
        let id = format!("seq-{}", e["seq"]);
        let mut push = |role: &str, text: String, collapsed: bool, error: bool| {
            if !text.is_empty() {
                out.push(Message {
                    id: format!("{id}-{role}"),
                    role: role.into(),
                    blocks: parse(&text),
                    text,
                    collapsed,
                    error,
                });
            }
        };
        match ty {
            "user/message" if d.pointer("/source/kind").and_then(Value::as_str) == Some("user") => {
                let mut content = parts(&d["content"], "text");
                for p in d["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|p| p["type"] == "image")
                {
                    let path = str_at(p, "/path");
                    if !path.is_empty() {
                        content.push_str(&format!("\n![附件]({path})"));
                    } else {
                        content.push_str("\n\n图片附件（可在网页版查看）");
                    }
                }
                push("user", content, false, false);
            }
            "assistant/message" => {
                let msg = d.get("message").unwrap_or(d);
                let thought = parts(&msg["content"], "reasoning");
                push("reasoning", thought, true, false);
                push("assistant", parts(&msg["content"], "text"), false, false);
            }
            "tool/call" => {
                let name = str_at(d, "/name");
                let args = str_at(d, "/arguments");
                let idx = out.len();
                out.push(Message {
                    id: id.clone(),
                    role: "tool".into(),
                    blocks: vec![],
                    text: format!("{name}\n{args}"),
                    collapsed: true,
                    error: false,
                });
                calls.insert(str_at(d, "/callId"), idx);
            }
            "tool/result" => {
                let key = str_at(d, "/message/source/callId");
                if let Some(&idx) = calls.get(&key) {
                    out[idx].error = d["isError"].as_bool().unwrap_or(false)
                        || d.get("error").is_some_and(|e| !e.is_null());
                    out[idx]
                        .text
                        .push_str(&format!("\n{}", parts(&d["message"]["content"], "text")));
                }
            }
            "turn/end"
                if !matches!(
                    d.pointer("/reason/kind").and_then(Value::as_str),
                    Some("completed")
                ) =>
            {
                push(
                    "notice",
                    format!("回合结束：{}", str_at(d, "/reason/kind")),
                    false,
                    true,
                )
            }
            _ => {}
        }
    }
    out.into_iter().map(Arc::new).collect()
}
pub fn load(base: &str, selected: Option<&str>, before: Option<i64>) -> Result<Data, String> {
    let listed = rpc(base, "session.list", json!({}))?;
    let sessions = sessions(&listed["items"]);
    let selected = selected
        .map(str::to_owned)
        .or_else(|| sessions.first().map(|s| s.id.clone()))
        .unwrap_or_default();
    let history = if selected.is_empty() {
        json!({"events":[]})
    } else {
        rpc(
            base,
            "session.history",
            json!({"sessionId":selected,"maxMessages":60,"beforeSeq":before}),
        )?
    };
    Ok(Data {
        sessions,
        selected,
        messages: messages(&history),
        more: history["hasMoreBefore"].as_bool().unwrap_or(false),
        first: history["firstSeq"].as_i64(),
        error: None,
    })
}
pub fn fixture(path: &std::path::Path) -> Result<Data, String> {
    let root: Value = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let sessions = sessions(&root["sessions"]);
    let selected = sessions.first().map(|s| s.id.clone()).unwrap_or_default();
    Ok(Data {
        sessions,
        selected,
        messages: messages(&root["history"]),
        ..Data::default()
    })
}
