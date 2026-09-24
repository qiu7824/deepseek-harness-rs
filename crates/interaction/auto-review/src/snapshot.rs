//! Reviewer authority comes from provenance, never from assistant or tool prose.
use dsh_agent::Agent;
use dsh_session::SessionEvent;
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
    agent.session().with_surface_events(|events, nodes| {
        capture_locked(
            events,
            nodes,
            &cwd,
            meta.parent_session.as_ref().map(|id| id.as_str()),
            meta.origin.as_deref() == Some("subagent"),
            inherited,
            exec,
        )
    })?
}
fn capture_locked(
    events: &[SessionEvent],
    nodes: &[u64],
    cwd: &str,
    parent: Option<&str>,
    child: bool,
    inherited: u64,
    exec: &ToolExecution,
) -> Result<Snapshot, String> {
    let header = &events
        .iter()
        .rev()
        .find(|e| e.type_ == "request/header")
        .ok_or("missing request header")?
        .data["header"];
    let provider = id(&header["config"], "provider")?.to_owned();
    let model = id(&header["config"], "model")?.to_owned();
    let mut native: HashMap<Key, Vec<&SessionEvent>> = HashMap::new();
    let mut starts: HashMap<Key, &SessionEvent> = HashMap::new();
    let mut by_parent: HashMap<Key, Vec<&SessionEvent>> = HashMap::new();
    let mut current = None;
    let mut descriptor_seen = false;
    let mut initial = None;
    for event in events {
        match event.type_.as_str() {
            "turn/start" | "turn/end" | "step/end" => current = None,
            "step/start" => current = Some(step(&event.data)?),
            "tool/call" => native
                .entry(key(step(&event.data)?, id(&event.data, "callId")?))
                .or_default()
                .push(event),
            "tool/ptc-dispatch-start" => {
                let position = current.ok_or("PTC start has no owning step")?;
                let call = key(position, id(&event.data, "subCallId")?);
                if starts.insert(call, event).is_some() {
                    return Err("ambiguous nested call identity".into());
                }
                by_parent
                    .entry(key(position, id(&event.data, "parentCallId")?))
                    .or_default()
                    .push(event);
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
    }
    let current = current.ok_or("pending action has no open step")?;
    let root_key = key(current, exec.root_call_id.as_str());
    let calls = native
        .get(&root_key)
        .ok_or("pending root action is not logged")?;
    if calls.len() != 1 {
        return Err("ambiguous pending root action".into());
    }
    let root = calls[0];
    let nested = if exec.parent.is_some() {
        Some(
            *starts
                .get(&key(current, exec.call_id.as_str()))
                .ok_or("pending nested action is not logged")?,
        )
    } else {
        None
    };
    let mut project = Vec::new();
    let mut history = Vec::new();
    let mut visible = HashSet::new();
    let mut passed = false;
    for &seq in nodes {
        let event = events
            .get(seq as usize)
            .ok_or("invalid surface coordinate")?;
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
            let Some(call) = calls.first() else {
                if position == current && !passed || !children.is_empty() {
                    return Err("unstarted visible action before pending call".into());
                }
                unstarted = true;
                continue;
            };
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
            for nested_call in children {
                if nested.is_some_and(|pending| pending.seq == nested_call.seq) {
                    continue;
                }
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
