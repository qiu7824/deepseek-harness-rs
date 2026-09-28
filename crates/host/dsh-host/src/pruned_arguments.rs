//! Refuse lossy context artifacts in writes, with a path back to original evidence.
use cordis::{arc, downcast_arc};
use dsh_tools::{PreToolDecision, ToolErrorInfo, ToolExecution};
use serde_json::{Value, json};
use std::sync::Arc;

fn contains_marker(value: &Value) -> bool {
    match value {
        Value::String(text) => text.contains(dsh_llm::TOOL_RESULT_PRUNE_MARKER.trim()),
        Value::Array(items) => items.iter().any(contains_marker),
        Value::Object(fields) => fields.values().any(contains_marker),
        _ => false,
    }
}
fn proposed_write_contains_marker(name: &str, args: &Value) -> bool {
    if matches!(name, "edit" | "edit_file") {
        let fields = ["new_string", "newString", "replacement", "newText"];
        if !fields.iter().any(|field| args.get(*field).is_some()) {
            return contains_marker(args);
        }
        return fields
            .iter()
            .any(|field| args.get(*field).is_some_and(contains_marker));
    }
    if matches!(name, "apply_patch" | "patch") {
        if let Some(patch) = args["patch"].as_str().or_else(|| args["input"].as_str()) {
            return patch.lines().any(|line| {
                line.starts_with('+') && line.contains(dsh_llm::TOOL_RESULT_PRUNE_MARKER.trim())
            });
        }
    }
    contains_marker(args)
}
fn reason(execution: &ToolExecution) -> Result<Option<Value>, String> {
    if crate::execution_evidence::classify(execution) == dsh_tools::receipt::EffectClass::ReadOnly
        || !proposed_write_contains_marker(&execution.name, &execution.arguments)
    {
        return Ok(None);
    }
    let Some(agent) = &execution.agent else {
        return Ok(None);
    };
    let session = agent.session();
    // Quoting the literal marker is valid when the user actually supplied it.
    if session
        .find_event_rev(|event| {
            event.type_ == "user/message" && event.data["source"]["kind"] == "user"
        })?
        .is_some_and(|event| contains_marker(&event.data["content"]))
    {
        return Ok(None);
    }
    let Some(pruned) = session.find_event_rev(|event| {
        event.type_ == "tool/result"
            && contains_marker(&event.data["message"]["content"])
            && matches!(
                event.surface_op,
                Some(dsh_session::SurfaceOp::Replace { .. })
            )
    })?
    else {
        return Ok(None);
    };
    let Some(sources) = pruned
        .source_event_seqs
        .as_ref()
        .filter(|sources| !sources.is_empty())
    else {
        return Ok(None);
    };
    let Some(provenance) = session.find_event_rev(|event| {
        event.type_ == "compaction/prune"
            && event.seq.get() < pruned.seq.get()
            && event.data["shadowedSeqs"] == json!(sources)
    })?
    else {
        return Ok(None);
    };
    let reference = &provenance.data["contextPrune"];
    let original = reference["originalSeq"].as_u64().unwrap_or(sources[0]);
    let call = pruned.data["message"]["source"]["callId"]
        .as_str()
        .unwrap_or("");
    Ok(Some(
        json!({"sourceEventSeq":original,"sourceCallId":call,"operationStarted":false}),
    ))
}
pub(super) async fn install(ctx: &cordis::Context) {
    ctx.on("tools/pre-execute",Arc::new(move|_,args|Box::pin(async move {
        if let Some(execution)=args.first().and_then(downcast_arc::<Arc<ToolExecution>>) {
            match reason(&execution) {
                Ok(Some(meta))=>return Some(arc(PreToolDecision::DenyWithInfo {
                    reason:format!("PRUNED_ARGUMENTS: 写入内容包含上下文裁剪占位符，操作未执行。请重新读取原文件或取得工具调用 {} 的完整结果后再写入；原始事件序号为 {}。删除旧占位符的编辑可正常执行。",meta["sourceCallId"],meta["sourceEventSeq"]),
                    info:ToolErrorInfo{name:"PrunedArgumentsError".into(),code:"PRUNED_ARGUMENTS".into()},meta:Some(meta),
                })),
                Err(error)=>return Some(arc(PreToolDecision::DenyWithInfo {
                    reason:format!("无法核对裁剪内容来源，操作未执行：{error}"),
                    info:ToolErrorInfo{name:"ContextEvidenceError".into(),code:"CONTEXT_EVIDENCE_UNAVAILABLE".into()},meta:None,
                })),
                _=>{}
            }
        }
        match args.last().and_then(downcast_arc::<cordis::NextFn>){Some(next)=>Some(next.call().await),None=>Some(arc(PreToolDecision::Allow))}
    })),Default::default()).await;
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn guard_requires_durable_prune_provenance_and_allows_reloaded_bytes_or_user_quotes() {
        let ctx = cordis::Context::root();
        dsh_system_prompt::SystemPrompt::install(&ctx, Default::default()).unwrap();
        dsh_tools::ToolRuntime::install(&ctx, Default::default()).unwrap();
        dsh_llm::LlmRuntime::install(&ctx);
        let session =
            dsh_session::Session::create(dsh_session::session_id("prune-owner"), None, None, None)
                .unwrap();
        let agent: Arc<dyn dsh_agent::Agent> = dsh_agent_loop::ReactLoopAgent::new(
            &ctx,
            session.id().clone(),
            Default::default(),
            session.clone(),
        )
        .unwrap();
        let mut execution = ToolExecution {
            schema: None,
            permission_preset: None,
            token: 1,
            call_id: dsh_llm::call_id("write"),
            root_call_id: dsh_llm::call_id("write"),
            name: "write".into(),
            arguments: json!({"content":dsh_llm::TOOL_RESULT_PRUNE_MARKER}),
            agent: Some(agent),
            parent: None,
            signal: parking_lot::Mutex::new(Arc::new(|| false)),
        };
        assert!(
            reason(&execution).unwrap().is_none(),
            "a literal alone is not proof of data loss"
        );
        let data = json!({"turn":1,"message":{"id":"result","role":"tool","content":[{"type":"text","text":"original bytes"}],"source":{"kind":"tool","callId":"read"},"toolCallId":"read"}});
        let original = session
            .append(
                "tool/result",
                data.clone(),
                Some(dsh_session::SurfaceIntent {
                    surface_op: dsh_session::SurfaceOp::Append,
                    source_event_seqs: None,
                }),
            )
            .unwrap()
            .seq
            .get();
        let mut pruned = data;
        pruned["message"]["content"] =
            json!([{"type":"text","text":dsh_llm::TOOL_RESULT_PRUNE_MARKER}]);
        session.append("compaction/prune",json!({"shadowedSeqs":[original],"contextPrune":{"version":1,"originalSeq":original,"callId":"read"}}),None).unwrap();
        session
            .append(
                "tool/result",
                pruned,
                Some(dsh_session::SurfaceIntent {
                    surface_op: dsh_session::SurfaceOp::Replace {
                        start: original,
                        end: original,
                    },
                    source_event_seqs: Some(vec![original]),
                }),
            )
            .unwrap();
        assert_eq!(
            reason(&execution).unwrap().unwrap()["sourceEventSeq"],
            original
        );
        execution.name = "read".into();
        assert!(reason(&execution).unwrap().is_none());
        execution.name = "write".into();
        execution.arguments = json!({"content":"original bytes"});
        assert!(reason(&execution).unwrap().is_none());
        execution.arguments = json!({"content":dsh_llm::TOOL_RESULT_PRUNE_MARKER});
        let cold = dsh_session::Session::from_restore(
            session.id().clone(),
            session.events().to_vec(),
            &session.header(),
            session.inherited_event_count(),
        )
        .unwrap();
        execution.agent = Some(
            dsh_agent_loop::ReactLoopAgent::new(
                &ctx,
                cold.id().clone(),
                Default::default(),
                cold.clone(),
            )
            .unwrap(),
        );
        assert!(
            reason(&execution).unwrap().is_some(),
            "cold restore cannot forget the gap"
        );
        let user = dsh_llm::create_user_message(
            vec![dsh_llm::ContentBlock::Text {
                text: format!(
                    "Document this literal: {}",
                    dsh_llm::TOOL_RESULT_PRUNE_MARKER
                ),
            }],
            dsh_llm::MessageSource::User {
                rpc_id: None,
                client_time_zone: None,
            },
        );
        cold.append(
            "user/message",
            serde_json::to_value(user).unwrap(),
            Some(dsh_session::SurfaceIntent {
                surface_op: dsh_session::SurfaceOp::Append,
                source_event_seqs: None,
            }),
        )
        .unwrap();
        assert!(reason(&execution).unwrap().is_none());
    }
    #[test]
    fn writing_a_gap_is_blocked_but_removing_or_discussing_ordinary_truncation_is_not() {
        let marker = dsh_llm::TOOL_RESULT_PRUNE_MARKER;
        assert!(proposed_write_contains_marker(
            "write",
            &json!({"content":format!("head{marker}tail")})
        ));
        assert!(proposed_write_contains_marker(
            "run_code",
            &json!({"code":format!("save({marker:?})")})
        ));
        assert!(!proposed_write_contains_marker(
            "edit",
            &json!({"old_string":marker,"new_string":"recovered bytes"})
        ));
        assert!(!proposed_write_contains_marker(
            "apply_patch",
            &json!({"patch":format!("-{}\n+recovered",marker.trim())})
        ));
        assert!(!proposed_write_contains_marker(
            "write",
            &json!({"content":"Document how output truncation works."})
        ));
    }
}
