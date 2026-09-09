//! Agent entry points share the same bounded index as the workspace UI.
use cordis::Context;
use dsh_code_graph::{
    BackgroundIndex,
    agent_queries::{AgentQuery, query},
};
use dsh_llm::ContentBlock;
use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

fn scoped_path(root: &Path, value: &str) -> Result<String, String> {
    if value.is_empty() {
        return Ok(String::new());
    }
    let path = PathBuf::from(value);
    let mut ancestor = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    let mut missing = Vec::new();
    while !ancestor.exists() {
        let name = ancestor.file_name().ok_or("invalid code scope")?.to_owned();
        if name == ".." {
            return Err("code scope must remain inside the workspace".into());
        }
        missing.push(name);
        if !ancestor.pop() {
            return Err("code scope does not have an existing parent".into());
        }
    }
    let mut resolved = std::fs::canonicalize(&ancestor)
        .map_err(|error| format!("code scope is unavailable: {error}"))?;
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    let relative = resolved
        .strip_prefix(root)
        .map_err(|_| "code scope is outside this Agent's workspace")?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn validate_arguments(args: &Value) -> Result<(), String> {
    for (key, maximum) in [
        ("query", 1000),
        ("symbol", 4096),
        ("from", 4096),
        ("to", 4096),
        ("path", 4096),
    ] {
        if args
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| value.chars().count() > maximum)
        {
            return Err(format!("{key} exceeds {maximum} characters"));
        }
    }
    if let Some(files) = args.get("files").and_then(Value::as_array) {
        if files.len() > 50
            || files.iter().any(|file| {
                file.as_str()
                    .is_some_and(|name| name.chars().count() > 4096)
            })
        {
            return Err("files accepts at most 50 paths of 4096 characters each".into());
        }
    }
    for (key, maximum) in [("limit", 50), ("depth", 4)] {
        if let Some(value) = args.get(key) {
            if !value
                .as_u64()
                .is_some_and(|number| (1..=maximum).contains(&number))
            {
                return Err(format!("{key} must be an integer between 1 and {maximum}"));
            }
        }
    }
    Ok(())
}

pub fn install(
    ctx: &Context,
    tools: Arc<ToolRuntime>,
    index: BackgroundIndex,
) -> Result<(), String> {
    for (name, action, description) in [
        (
            "code_context",
            "context",
            "Find implementation candidates by identifier, path or search terms. Returns bounded definitions with source lines and stable symbol IDs. Read the referenced source before changing it.",
        ),
        (
            "code_callers",
            "callers",
            "Find bounded static caller/reference candidates for one symbol. Use the exact symbol id returned by code_context when names are ambiguous.",
        ),
        (
            "code_callees",
            "callees",
            "Find bounded static dependencies and callees of one symbol, with call-site evidence. Name-based resolution is explicitly heuristic.",
        ),
        (
            "code_impact",
            "impact",
            "Inspect likely change impact for one symbol or workspace files: callers, dependent files and test candidates. Refreshes the shared index by default; this is evidence for validation, not proof of complete coverage.",
        ),
        (
            "code_path",
            "path",
            "Find a shortest directed static relationship path between from/to symbol IDs, bounded to 20 hops and 10000 visited nodes.",
        ),
    ] {
        let shared = index.clone();
        let parameters = json!({"type":"object","additionalProperties":false,"properties":{
            "query":{"type":"string","description":"At most 1000 characters."},"symbol":{"type":"string"},
            "from":{"type":"string"},"to":{"type":"string"},
            "path":{"type":"string","description":"Optional workspace-relative file or directory scope."},
            "files":{"type":"array","items":{"type":"string"},"description":"At most 50 workspace paths."},
            "limit":{"type":"integer","description":"Result count, 1 to 50; default 12."},"depth":{"type":"integer","description":"Relationship depth, 1 to 4; default 2."},
            "refresh":{"type":"boolean"}},"required":match action{"context"=>vec!["query"],"callers"|"callees"=>vec!["symbol"],"path"=>vec!["from","to"],_=>vec![]}});
        tools.register(ctx,ToolDefinition{
            name:name.into(),description:description.into(),parameters,
            output:ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,value|Ok(vec![ContentBlock::Text{text:serde_json::to_string(value).map_err(|error|error.to_string())?}])),presentation_meta:None},
            timeout_ms:Some(25_000),is_concurrency_safe:Some(Arc::new(|_|true)),
            finalize_content:None,present_call:None,present_result:None,
            execute:Arc::new(move|arguments,run|{
                let args=arguments.clone();let shared=shared.clone();let agent=run.execution.agent.clone();let signal=run.execution.signal.lock().clone();
                Box::pin(async move{
                    validate_arguments(&args).map_err(ToolBodyError::plain)?;
                    let agent=agent.ok_or_else(||ToolBodyError::plain("code intelligence requires an initiating Agent"))?;
                    let cwd=agent.session().header().cwd.clone().ok_or_else(||ToolBodyError::plain("this Agent has no workspace"))?;
                    let root=std::fs::canonicalize(&cwd).map_err(|error|ToolBodyError::plain(format!("workspace is unavailable: {error}")))?;
                    let string=|key:&str|args.get(key).and_then(Value::as_str).unwrap_or("").to_string();
                    let scope=scoped_path(&root,&string("path")).map_err(ToolBodyError::plain)?;
                    let files=args.get("files").and_then(Value::as_array).into_iter().flatten().map(|file|scoped_path(&root,file.as_str().unwrap_or(""))).collect::<Result<Vec<_>,_>>().map_err(ToolBodyError::plain)?;
                    if action=="impact"&&string("symbol").is_empty()&&files.is_empty(){return Err(ToolBodyError::plain("code_impact requires symbol or files"))}
                    let request=AgentQuery{action:action.into(),query:string("query"),symbol:if action=="path"{string("from")}else{string("symbol")},to:string("to"),path:scope,files,
                        limit:args.get("limit").and_then(Value::as_u64).unwrap_or(12) as usize,depth:args.get("depth").and_then(Value::as_u64).unwrap_or(2) as usize};
                    let mut changes=shared.changes();
                    shared.request(&root,args.get("refresh").and_then(Value::as_bool).unwrap_or(action=="impact"));
                    let deadline=tokio::time::Instant::now()+Duration::from_secs(18);
                    loop{
                        if signal(){return Err(ToolBodyError::coded("code query cancelled","ToolAborted","TOOL_ABORTED"))}
                        let (snapshot,status,stats,error)=shared.inspect(&root);
                        if matches!(status.as_str(),"ready"|"partial"){
                            if let Some(snapshot)=snapshot{
                                let stop=signal.clone();
                                let mut result=tokio::task::spawn_blocking(move||query(&snapshot,&request,stop.as_ref())).await.map_err(|error|ToolBodyError::plain(error.to_string()))?.map_err(ToolBodyError::plain)?;
                                result["partial"]=json!(stats.partial||result["partial"]==true);
                                result["index"]=json!({"status":status,"coverage":stats,"error":error});
                                return Ok(result);
                            }
                        }
                        if matches!(status.as_str(),"failed"|"cancelled")||tokio::time::Instant::now()>=deadline{
                            return Ok(json!({"status":status,"index":{"coverage":stats,"error":error},"guidance":"The shared index is not ready. Read/search specific files directly or retry explicitly; no complete relationship result is available."}));
                        }
                        if status=="queued"{shared.request(&root,true)}
                        // Index changes wake all consumers. The short timer only
                        // checks this call's cancellation predicate while waiting.
                        tokio::select!{_ = changes.changed()=>{},_ = tokio::time::sleep(Duration::from_millis(100))=>{}}
                    }
                })
            }),
        }).map_err(|error|error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_admission_rejects_excessive_work_before_indexing() {
        for args in [
            json!({"limit":0}),
            json!({"depth":5}),
            json!({"limit":-1}),
            json!({"limit":1.5}),
            json!({"query":"x".repeat(1001)}),
            json!({"files":vec!["src/lib.rs";51]}),
        ] {
            assert!(validate_arguments(&args).is_err(), "{args}");
        }
        assert!(validate_arguments(&json!({"limit":50,"depth":4,"query":"图谱"})).is_ok());
    }
}
