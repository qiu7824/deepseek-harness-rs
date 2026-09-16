//! Durable, user-managed application grants. Only adapter-attested targets
//! enter the catalog; the settings API accepts opaque catalog IDs, not paths.
use std::{collections::BTreeMap,path::PathBuf,sync::{Arc,atomic::{AtomicU64,Ordering}},time::{SystemTime,UNIX_EPOCH}};
use cordis::Context;
use dsh_tool_computer_use_command::{AbortPredicate,AdapterError,ComputerPermissionService,ComputerPermissionRequest,ComputerPermissionLease,ComputerTargetIdentity,COMPUTER_PERMISSION_SCOPES};
use parking_lot::Mutex;
use serde::{Serialize,Deserialize};
use serde_json::{Value,json};
use sha2::{Digest,Sha256};

#[derive(Clone,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
struct Grant {id:String,target_id:String,identity:ComputerTargetIdentity,scopes:Vec<String>,scope:String,owner_id:Option<String>,remaining:Option<u64>,created_at:u64,revoked:bool}
#[derive(Clone,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
struct Saved {version:u32,revision:u64,host_id:String,grants:Vec<Grant>}
struct State {saved:Saved,observed:BTreeMap<String,(ComputerTargetIdentity,String)>,generations:BTreeMap<String,Arc<AtomicU64>>}

pub(super) struct ComputerPermissions {ctx:Context,path:PathBuf,state:Arc<Mutex<State>>}
fn now()->u64{SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()}
fn target_key(target:&ComputerTargetIdentity)->String{
    let identity=json!([target.host_id,target.device_id,target.application_id,target.application_revision,target.origin]);
    format!("app-{:x}",Sha256::digest(identity.to_string().as_bytes()))
}
fn error(code:&str,message:impl Into<String>)->AdapterError{AdapterError::new(code,message)}
fn save(path:&std::path::Path,saved:&Saved)->Result<(),String>{
    let bytes=serde_json::to_vec_pretty(saved).map_err(|error|error.to_string())?;
    if bytes.len()>512*1024{return Err("应用授权记录超过大小限制".into());}
    let parent=path.parent().ok_or("授权目录无效")?;
    std::fs::create_dir_all(parent).map_err(|error|error.to_string())?;
    let temp=parent.join(format!(".computer-permissions-{}.tmp",uuid::Uuid::new_v4()));
    use std::io::Write;
    let mut options=std::fs::OpenOptions::new();options.create_new(true).write(true);
    #[cfg(unix)] {use std::os::unix::fs::OpenOptionsExt;options.mode(0o600);}
    let result=(||{let mut file=options.open(&temp)?;file.write_all(&bytes)?;file.sync_all()?;drop(file);std::fs::rename(&temp,path)})();
    if result.is_err(){let _=std::fs::remove_file(&temp);} result.map_err(|error:std::io::Error|error.to_string())
}
impl ComputerPermissions {
    pub(super) fn install(ctx:&Context,data_root:PathBuf)->Result<Arc<Self>,String>{
        let path=data_root.join("computer-permissions.json");
        let saved=if path.exists(){
            if std::fs::metadata(&path).map_err(|error|error.to_string())?.len()>512*1024{return Err("应用授权文件超过大小限制".into());}
            let saved:Saved=serde_json::from_slice(&std::fs::read(&path).map_err(|error|error.to_string())?).map_err(|error|format!("应用授权文件损坏：{error}"))?;
            if saved.version!=1{return Err("应用授权文件版本不受支持".into());} saved
        }else{Saved{version:1,revision:0,host_id:uuid::Uuid::new_v4().to_string(),grants:vec![]}};
        let service=Arc::new(Self{ctx:ctx.clone(),path,state:Arc::new(Mutex::new(State{saved,observed:BTreeMap::new(),generations:BTreeMap::new()}))});
        let erased:Arc<dyn ComputerPermissionService>=service.clone();ctx.register_service(erased);
        Ok(service)
    }
    pub(super) fn snapshot(&self)->Value{
        let state=self.state.lock();
        json!({"revision":state.saved.revision,"hostId":state.saved.host_id,"scopes":COMPUTER_PERMISSION_SCOPES,"targets":state.observed.iter().map(|(id,(identity,owner))|json!({"id":id,"identity":identity,"ownerSessionId":owner})).collect::<Vec<_>>(),"grants":state.saved.grants,"capabilities":{"nativeDesktop":cfg!(windows),"browser":true,"remoteDesktop":"device_scope_only"}})
    }
    fn mutate(&self,args:&Value)->Result<Value,String>{
        let mut state=self.state.lock();
        if args["expectedRevision"].as_u64()!=Some(state.saved.revision){return Err("授权列表已变化，请刷新后再操作".into());}
        let mut saved=state.saved.clone();
        let changed_key;
        match args["action"].as_str(){
            Some("grant")=>{
                let target_id=args["targetId"].as_str().ok_or("缺少已验证的应用标识")?;
                let (identity,observed_owner)=state.observed.get(target_id).ok_or("应用尚未被控制器验证，请先定位目标应用")?;
                let scope=args["scope"].as_str().filter(|scope|matches!(*scope,"once"|"session"|"persistent")).ok_or("授权范围无效")?;
                let scopes=args["scopes"].as_array().ok_or("缺少操作权限")?.iter().map(|value|value.as_str().filter(|value|COMPUTER_PERMISSION_SCOPES.contains(value)).map(str::to_string).ok_or("操作权限无效")).collect::<Result<Vec<_>,_>>()?;
                if scopes.is_empty()||scopes.len()>COMPUTER_PERMISSION_SCOPES.len(){return Err("请选择至少一项操作权限".into());}
                if saved.grants.len()>=512 {saved.grants.retain(|grant|!grant.revoked&&grant.remaining!=Some(0));}
                if saved.grants.len()>=512{return Err("授权数量已达上限，请撤销不用的规则".into());}
                saved.grants.push(Grant{id:uuid::Uuid::new_v4().to_string(),target_id:target_id.into(),identity:identity.clone(),scopes,scope:scope.into(),owner_id:if scope=="persistent"{None}else{Some(observed_owner.clone())},remaining:if scope=="once"{Some(1)}else{None},created_at:now(),revoked:false});
                changed_key=None;
            }
            Some("revoke")=>{
                let id=args["grantId"].as_str().ok_or("缺少授权标识")?;
                let grant=saved.grants.iter_mut().find(|grant|grant.id==id).ok_or("授权不存在")?;
                grant.revoked=true;changed_key=Some(grant.target_id.clone());
            }
            _=>return Err("未知授权操作".into()),
        }
        saved.revision=saved.revision.saturating_add(1);
        save(&self.path,&saved)?;
        state.saved=saved;
        if let Some(key)=changed_key {state.generations.entry(key).or_insert_with(||Arc::new(AtomicU64::new(0))).fetch_add(1,Ordering::SeqCst);}
        drop(state);Ok(self.snapshot())
    }
    pub(super) fn register(self:&Arc<Self>,server:&Arc<dsh_host_webserver::WebServer>,allow_remote:bool)->dsh_host_webserver::RouteDisposer{
        let service=self.clone();
        server.register(dsh_host_webserver::WebRoute{kind:dsh_host_webserver::WebRouteKind::Exact,path:"/__dsh-computer-permissions".into(),handler:Arc::new(move|request|{
            let service=service.clone();Box::pin(async move{
                let trusted=crate::trusted_web_request(&request,allow_remote);
                let post=request.method()==http::Method::POST&&request.headers().get("content-type").and_then(|value|value.to_str().ok()).is_some_and(|value|value.starts_with("application/json"));
                let result=if !trusted{Err("禁止跨站访问".into())}else if !post{Err("需要 JSON POST 请求".into())}else{
                    match axum::body::to_bytes(axum::body::Body::new(request.into_body()),16*1024).await{
                        Ok(bytes)=>match serde_json::from_slice::<Value>(&bytes){Ok(args)=>if args["action"]=="list"{Ok(service.snapshot())}else{service.mutate(&args)},Err(_)=>Err("无效 JSON".into())},Err(_)=>Err("请求超过大小限制".into())
                    }
                };
                let(status,value)=match result{Ok(value)=>(http::StatusCode::OK,value),Err(error)=>(if trusted{http::StatusCode::BAD_REQUEST}else{http::StatusCode::FORBIDDEN},json!({"error":error}))};
                Ok(http::Response::builder().status(status).header("content-type","application/json").header("cache-control","no-store").body(axum::body::Body::from(value.to_string())).expect("computer permission response"))
            })
        })})
    }
}
impl ComputerPermissionService for ComputerPermissions {
    fn authorize(&self,mut request:ComputerPermissionRequest)->futures::future::BoxFuture<'static,Result<ComputerPermissionLease,AdapterError>>{
        let state=self.state.clone();let path=self.path.clone();let ctx=self.ctx.clone();
        Box::pin(async move{
            if request.signal.as_ref()(){return Err(AdapterError::cancelled());}
            if request.owner_id.is_empty(){return Err(error("COMPUTER_USE_OWNER_REQUIRED","Application authorization requires an owning task"));}
            let(key,generation,stamp,revision,granted)={
                let mut state=state.lock();request.target.host_id=state.saved.host_id.clone();
                let key=target_key(&request.target);
                if !state.observed.contains_key(&key)&&state.observed.len()>=128{let oldest=state.observed.keys().next().cloned().unwrap();state.observed.remove(&oldest);}
                state.observed.insert(key.clone(),(request.target.clone(),request.owner_id.clone()));
                let generation=state.generations.entry(key.clone()).or_insert_with(||Arc::new(AtomicU64::new(0))).clone();
                let stamp=generation.load(Ordering::SeqCst);
                let matched=state.saved.grants.iter().position(|grant|!grant.revoked&&grant.target_id==key&&grant.remaining!=Some(0)&&grant.owner_id.as_ref().is_none_or(|owner|owner==&request.owner_id)&&request.scopes.iter().all(|scope|grant.scopes.contains(scope)));
                if let Some(index)=matched && state.saved.grants[index].remaining==Some(1){
                    let mut saved=state.saved.clone();saved.grants[index].remaining=Some(0);saved.revision+=1;
                    save(&path,&saved).map_err(|message|error("COMPUTER_USE_PERMISSION_STORAGE",message))?;state.saved=saved;
                }
                (key,generation,stamp,state.saved.revision,matched.is_some())
            };
            if !granted{
                let agents=ctx.get_typed::<Arc<dsh_agent::AgentRegistry>>("agents",false).ok_or_else(||error("COMPUTER_USE_PERMISSION_REQUIRED","Application permission is required; open application permissions in settings"))?;
                let owner=agents.get(&dsh_session::session_id(&request.owner_id)).ok_or_else(||error("COMPUTER_USE_OWNER_REQUIRED","The owning task is no longer active"))?;
                let approval=ctx.get_typed::<Arc<dsh_user_approval::ApprovalService>>("approval",false).ok_or_else(||error("COMPUTER_USE_PERMISSION_REQUIRED","No approval service is available"))?;
                let outcome=approval.request(&dsh_user_approval::ApprovalRequest{agent:owner,tool_name:"computer_use".into(),call_id:None,reason:Some(format!("应用：{}\n设备：{}\n来源：{}\n操作：{}\n本次所需权限：{}",request.target.label,request.target.device_id,request.target.origin.as_deref().unwrap_or("桌面应用"),request.action,request.scopes.join(", "))),grant_key:Some(format!("computer:{key}:{}",request.scopes.join(","))),rememberable:false,signal:Some(request.signal.clone())}).await.map_err(|message|error("COMPUTER_USE_PERMISSION_REQUIRED",message))?;
                if !matches!(outcome,dsh_user_approval::ApprovalOutcome::AllowedOnce|dsh_user_approval::ApprovalOutcome::AllowedAlways){return Err(error("COMPUTER_USE_PERMISSION_DENIED","Application action was not authorized; no action was dispatched"));}
            }
            if generation.load(Ordering::SeqCst)!=stamp || request.signal.as_ref()(){return Err(error("COMPUTER_USE_PERMISSION_REVOKED","Application authorization changed while waiting"));}
            let valid:AbortPredicate=Arc::new(move||generation.load(Ordering::SeqCst)==stamp);
            Ok(ComputerPermissionLease{target:request.target,revision,valid})
        })
    }
}
