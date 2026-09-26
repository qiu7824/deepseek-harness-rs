//! Reviewer authority comes from provenance, never from assistant or tool prose.
use dsh_agent::Agent;
use dsh_session::SessionEventReader;
use dsh_tools::ToolExecution;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

type Step = (u64, u64);
type Key = (u64, u64, String);
fn step(data: &Value) -> Result<Step, String> {
    Ok((
        data["turn"].as_u64().ok_or("missing turn")?,
        data["step"].as_u64().ok_or("missing step")?,
    ))
}
fn key(step: Step, id: &str) -> Key {
    (step.0, step.1, id.into())
}
fn id<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing {key}"))
}
fn arguments(raw: &str) -> Value {
    if raw.is_empty() {
        json!({})
    } else {
        serde_json::from_str(raw).unwrap_or_else(|_| json!(raw))
    }
}
fn fact(mode: &str, data: &Value) -> Value {
    json!({"kind":"tool-call","role":"fact","mode":mode,"name":data["name"],"arguments":data["arguments"]})
}

pub(crate) struct Snapshot {
    pub provider: String,
    pub model: String,
    pub text: String,
}
pub(crate) fn capture(agent: &dyn Agent, exec: &ToolExecution) -> Result<Snapshot, String> {
    let meta = agent.session().header().clone();
    let inherited = agent.session().inherited_event_count().get();
    let cwd = meta
        .cwd
        .as_ref()
        .filter(|s| !s.is_empty())
        .ok_or("review requires a working directory")?
        .clone();
    agent.session().with_surface_reader(|reader, nodes| {
        capture_locked(
            reader,
            nodes,
            &cwd,
            meta.parent_session.as_ref().map(|id| id.as_str()),
            meta.origin.as_deref() == Some("subagent"),
            inherited,
            exec,
        )
    })
}
fn capture_locked(
    reader: &SessionEventReader<'_>,
    nodes: &[u64],
    cwd: &str,
    parent: Option<&str>,
    child: bool,
    inherited: u64,
    exec: &ToolExecution,
) -> Result<Snapshot, String> {
    let header_event = reader
        .find_rev(|event| event.type_ == "request/header")?
        .ok_or("missing request header")?;
    let header = &header_event.data["header"];
    let provider = id(&header["config"], "provider")?.to_owned();
    let model = id(&header["config"], "model")?.to_owned();
    let mut native: HashMap<Key, Vec<u64>> = HashMap::new();
    let mut starts: HashMap<Key, u64> = HashMap::new();
    let mut by_parent: HashMap<Key, Vec<u64>> = HashMap::new();
    let mut current = None;
    let mut descriptor_seen = false;
    let mut initial = None;
    reader.visit(0, None, |event| {
        match event.type_.as_str() {
            "turn/start" | "turn/end" | "step/end" => current = None,
            "step/start" => current = Some(step(&event.data)?),
            "tool/call" => native
                .entry(key(step(&event.data)?, id(&event.data, "callId")?))
                .or_default()
                .push(event.seq.get()),
            "tool/ptc-dispatch-start" => {
                let position = current.ok_or("PTC start has no owning step")?;
                let call = key(position, id(&event.data, "subCallId")?);
                if starts.insert(call, event.seq.get()).is_some() {
                    return Err("ambiguous nested call identity".into());
                }
                by_parent
                    .entry(key(position, id(&event.data, "parentCallId")?))
                    .or_default()
                    .push(event.seq.get());
            }
            _ => {}
        }
        if child && event.seq.get() >= inherited {
            if event.type_ == "subagent/descriptor" {
                descriptor_seen = true;
            }
            if descriptor_seen
                && initial.is_none()
                && event.type_ == "user/message"
                && event.data["source"]["kind"] == "user"
                && !event.data["source"]["rpcId"].is_string()
            {
                initial = Some(event.seq.get());
            }
        }
        Ok(true)
    })?;
    let current = current.ok_or("pending action has no open step")?;
    let root_key = key(current, exec.root_call_id.as_str());
    let calls = native
        .get(&root_key)
        .ok_or("pending root action is not logged")?;
    if calls.len() != 1 {
        return Err("ambiguous pending root action".into());
    }
    let root = reader
        .read(calls[0])?
        .ok_or("pending root action is unavailable")?;
    let nested = if exec.parent.is_some() {
        Some(
            reader
                .read(
                    *starts
                        .get(&key(current, exec.call_id.as_str()))
                        .ok_or("pending nested action is not logged")?,
                )?
                .ok_or("pending nested action is unavailable")?,
        )
    } else {
        None
    };
    let mut project = Vec::new();
    let mut history = Vec::new();
    let mut visible = HashSet::new();
    let mut passed = false;
    for &seq in nodes {
        let event = reader.read(seq)?.ok_or("invalid surface coordinate")?;
        if event.type_ == "user/message" {
            let source = &event.data["source"];
            if source["kind"] == "tool" {
                continue;
            }
            let content = event.data["content"]
                .as_array()
                .ok_or("invalid user content")?;
            if source["kind"] == "agent-instructions" {
                if !content.is_empty() {
                    project.push(json!({"kind":"user-message","role":"constraint","source":source,"content":content}));
                }
            } else {
                for block in content {
                    let role = if block["type"] != "text" {
                        "fact"
                    } else if source["kind"] == "user" && source["rpcId"].is_string() {
                        "human-instruction"
                    } else if initial == Some(seq)
                        || (source["kind"] == "agent-message"
                            && parent.is_some()
                            && source["senderSessionId"].as_str() == parent)
                    {
                        "direct-parent-instruction"
                    } else if source["kind"] == "compact-checkpoint" {
                        "checkpoint"
                    } else {
                        "fact"
                    };
                    history.push(json!({"kind":"user-message","role":role,"source":source,"content":[block]}));
                }
            }
            continue;
        }
        if event.type_ != "assistant/message" {
            continue;
        }
        let position = step(&event.data)?;
        let mut unstarted = false;
        for block in event.data["message"]["content"]
            .as_array()
            .ok_or("invalid assistant content")?
        {
            if block["type"] != "tool-call" {
                continue;
            }
            let call_key = key(position, id(block, "id")?);
            let is_current = call_key == root_key;
            if is_current && passed {
                return Err("pending action repeats in surface".into());
            }
            let calls = native.get(&call_key).map(Vec::as_slice).unwrap_or(&[]);
            let children = by_parent.get(&call_key).map(Vec::as_slice).unwrap_or(&[]);
            if calls.len() > 1 {
                return Err("ambiguous native call identity".into());
            }
            let Some(call_seq) = calls.first() else {
                if position == current && !passed || !children.is_empty() {
                    return Err("unstarted visible action before pending call".into());
                }
                unstarted = true;
                continue;
            };
            let call = reader
                .read(*call_seq)?
                .ok_or("started action is unavailable")?;
            if unstarted
                || call.data["name"] != block["name"]
                || call.data["arguments"] != block["arguments"]
            {
                return Err("visible action differs from logged started prefix".into());
            }
            visible.insert(call_key);
            if !is_current || nested.is_some() {
                history.push(fact("native", &call.data));
            }
            for &nested_seq in children {
                if nested
                    .as_ref()
                    .is_some_and(|pending| pending.seq.get() == nested_seq)
                {
                    continue;
                }
                let nested_call = reader
                    .read(nested_seq)?
                    .ok_or("nested action is unavailable")?;
                history.push(fact("ptc-inner", &nested_call.data));
            }
            if is_current {
                passed = true;
            }
        }
    }
    if !passed {
        return Err("pending action is not in the visible surface".into());
    }
    let (mode, schema) = if let Some(start) = nested {
        if !visible.contains(&key(current, id(&start.data, "parentCallId")?))
            || start.data["rootCallId"] != exec.root_call_id.as_str()
            || start.data["name"] != exec.name
            || start.data["arguments"] != exec.arguments
        {
            return Err("nested action identity mismatch".into());
        }
        (
            "ptc-inner",
            exec.schema.clone().ok_or("missing binding schema")?,
        )
    } else {
        if root.data["name"] != exec.name
            || arguments(
                root.data["arguments"]
                    .as_str()
                    .ok_or("missing raw arguments")?,
            ) != exec.arguments
        {
            return Err("native action identity mismatch".into());
        }
        let schemas = header["tools"]
            .as_array()
            .ok_or("missing tool schemas")?
            .iter()
            .filter(|schema| schema["name"] == exec.name)
            .collect::<Vec<_>>();
        if schemas.len() != 1 {
            return Err("missing or ambiguous native schema".into());
        }
        let schema: dsh_llm::ToolSchema =
            serde_json::from_value(schemas[0].clone()).map_err(|_| "invalid native schema")?;
        if exec.schema.as_ref().is_none_or(|bound| {
            bound.name != schema.name
                || bound.description != schema.description
                || bound.parameters != schema.parameters
        }) {
            return Err("native binding differs from advertised schema".into());
        }
        ("native", schema)
    };
    if schema.name != exec.name || !schema.parameters.is_object() {
        return Err("incomplete action schema".into());
    }
    let action = json!({"mode":mode,"name":schema.name,"description":schema.description,"parameters":schema.parameters,"arguments":exec.arguments});
    let text = format!(
        "ENVIRONMENT\n{}\n\nPROJECT_INSTRUCTIONS\n{}\n\nFILTERED_HISTORY\n{}\n\nPENDING_ACTION\n{}",
        json!({"cwd":cwd}),
        serde_json::to_string(&project).map_err(|e| e.to_string())?,
        serde_json::to_string(&history).map_err(|e| e.to_string())?,
        action
    );
    Ok(Snapshot {
        provider,
        model,
        text,
    })
}

#[cfg(test)]
mod archive_tests {
    use super::*;
    use dsh_session::{Session, SurfaceIntent, SurfaceOp};
    use std::sync::Arc;

    #[test]
    fn archived_review_snapshot_keeps_exact_bytes_order_and_parent_authority() {
        let session =
            Session::create(dsh_session::session_id("review-archive"), None, None, None).unwrap();
        let schema = dsh_llm::ToolSchema {
            name: "effect".into(),
            description: "Scoped effect".into(),
            parameters: json!({"type":"object","additionalProperties":false}),
            defer_loading: None,
        };
        session
            .append(
                "subagent/descriptor",
                json!({"version":5,"provider":"spawn","mode":"continuable","label":"child"}),
                None,
            )
            .unwrap();
        session
            .append("turn/start", json!({"turn":1}), None)
            .unwrap();
        session
            .append("step/start", json!({"turn":1,"step":1}), None)
            .unwrap();
        session.append("request/header", json!({"header":{"config":{"provider":"fixture","model":"model"},"tools":[schema.clone()]},"reason":"initial"}), None).unwrap();
        for (name, source, text) in [
            ("initial", json!({"kind":"user"}), "Parent task"),
            (
                "parent",
                json!({"kind":"agent-message","senderSessionId":"parent-session"}),
                "Parent continuation",
            ),
            (
                "human",
                json!({"kind":"user","rpcId":"human-input"}),
                "Human instruction",
            ),
        ] {
            session.append("user/message", json!({"id":name,"role":"user","content":[{"type":"text","text":text}],"source":source}), Some(SurfaceIntent {
                surface_op:SurfaceOp::Append, source_event_seqs:None,
            })).unwrap();
        }
        session.append("assistant/message", json!({"turn":1,"step":1,"message":{"id":"assistant","role":"assistant","content":[{"type":"text","text":"ASSISTANT_NOT_AUTHORITY"},{"type":"tool-call","id":"root","name":"effect","arguments":"{}"}],"source":{"kind":"model","provider":"fixture","model":"model"}}}), Some(SurfaceIntent {
            surface_op:SurfaceOp::Append, source_event_seqs:None,
        })).unwrap();
        session
            .append(
                "tool/call",
                json!({"turn":1,"step":1,"callId":"root","name":"effect","arguments":"{}"}),
                None,
            )
            .unwrap();
        let exec = ToolExecution {
            schema: Some(schema),
            permission_preset: Some("auto".into()),
            token: 1,
            call_id: dsh_llm::call_id("root"),
            root_call_id: dsh_llm::call_id("root"),
            name: "effect".into(),
            arguments: json!({}),
            agent: None,
            parent: None,
            signal: parking_lot::Mutex::new(Arc::new(|| false)),
        };
        let snapshot = |session: &Session| {
            session
                .with_surface_reader(|reader, nodes| {
                    capture_locked(
                        reader,
                        nodes,
                        "workspace",
                        Some("parent-session"),
                        true,
                        0,
                        &exec,
                    )
                })
                .unwrap()
        };
        let before = snapshot(&session);
        let mut builder =
            dsh_session::event_archive::EventArchiveBuilder::new(&std::env::temp_dir()).unwrap();
        session
            .visit_events(0, None, |event| {
                builder.push(event)?;
                Ok(true)
            })
            .unwrap();
        let archived = Session::from_event_archive(
            session.id().clone(),
            builder.finish().unwrap(),
            session.header(),
            session.inherited_event_count(),
            vec![],
        )
        .unwrap();
        let after = snapshot(&archived);
        assert_eq!(after.provider, before.provider);
        assert_eq!(after.model, before.model);
        assert_eq!(after.text.as_bytes(), before.text.as_bytes());
        assert!(after.text.contains("direct-parent-instruction"));
        assert!(after.text.contains("human-instruction"));
        assert!(!after.text.contains("ASSISTANT_NOT_AUTHORITY"));
        assert!(
            after.text.find("Parent task").unwrap()
                < after.text.find("Parent continuation").unwrap()
        );
        assert!(
            after.text.find("Parent continuation").unwrap()
                < after.text.find("Human instruction").unwrap()
        );
    }
}
