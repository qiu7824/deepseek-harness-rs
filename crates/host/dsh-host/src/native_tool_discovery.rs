//! Discovery and settings expose the same native capability evidence.
use super::task_models::TaskModels;
use cordis::{Context, arc, downcast_arc};
use dsh_tools::{PostToolDecision, ToolExecution, ToolExecutionResult};
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) fn install(ctx: &Context, service: &Arc<TaskModels>) {
    let weak = Arc::downgrade(service);
    futures::executor::block_on(ctx.on("tools/post-execute",Arc::new(move|_,args| {
        let weak=weak.clone();
        Box::pin(async move {
            let next=args.last().and_then(downcast_arc::<cordis::NextFn>);
            let decision=match next {Some(next)=>next.call().await,None=>arc(PostToolDecision::Accept{content:None,value:None,additional_contexts:None})};
            let Some(execution)=args.first().and_then(downcast_arc::<Arc<ToolExecution>>) else{return Some(decision);};
            if !matches!(execution.name.as_str(),"tool_search"|"tool_describe") {return Some(decision);}
            let Some(result)=args.get(1).and_then(downcast_arc::<Arc<ToolExecutionResult>>) else{return Some(decision);};
            if result.is_error {return Some(decision);}
            let Some(PostToolDecision::Accept{content:None,value,additional_contexts})=downcast_arc::<PostToolDecision>(&decision).as_deref().cloned() else{return Some(decision);};
            let Some(mut value)=value.or_else(||result.value.clone()) else{return Some(decision);};
            let Some(rows)=value["tools"].as_array_mut() else{return Some(decision);};
            if !rows.iter().any(|row|matches!(row["name"].as_str(),Some("generate_image"|"web_search"))) {return Some(decision);}
            let (Some(service),Some(agent))=(weak.upgrade(),execution.agent.as_ref()) else{return Some(decision);};
            let Ok(snapshot)=service.snapshot().await else{return Some(decision);};
            for row in rows {
                let role=match row["name"].as_str(){Some("generate_image")=>"image",Some("web_search")=>"search",_=>continue};
                let identity=service.admission_identity(role,agent);
                let provider=[&identity["configured"]["provider"],&identity["current"]["provider"],&identity["fallback"]["provider"],&snapshot["main"]["provider"]].into_iter().filter_map(Value::as_str).find(|s|!s.is_empty());
                let record=snapshot["providers"].as_array().into_iter().flatten().find(|p|p["id"].as_str()==provider).map(|p|p["nativeCapabilities"][role].clone()).unwrap_or(json!({"registered":true,"state":"unconfigured"}));
                row["nativeCapability"]=json!({"role":role,"provider":provider,"capability":record,"settings":"任务分工 / 模型连接","permissions":"checked_per_call"});
            }
            Some(arc(PostToolDecision::Accept{content:None,value:Some(value),additional_contexts}))
        })
    }),Default::default()));
}
