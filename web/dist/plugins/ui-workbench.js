window.__ModuleLoader__.load({
 id:"@deepseek-ai/dsh-client-ui-workbench",
 factory:(require)=>{
  const React=require("react"),jsx=require("react/jsx-runtime");
  const css=`.dsh-env{max-width:840px;padding:4px 2px 32px;color:var(--dsw-alias-label-primary);display:flex;flex-direction:column;gap:18px}.dsh-env h2{margin:0;font-size:20px;font-weight:600}.dsh-env-intro{margin:0;color:var(--dsw-alias-label-tertiary);font-size:13px;line-height:21px}.dsh-env-section{border:1px solid var(--dsw-alias-border-l2);border-radius:14px;background:var(--dsw-alias-bg-layer-1);overflow:hidden}.dsh-env-head{padding:14px 16px;border-bottom:1px solid var(--dsw-alias-border-l2);font-size:14px;font-weight:600}.dsh-env-grid{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:1px;background:var(--dsw-alias-border-l2)}.dsh-env-stat{padding:14px 16px;background:var(--dsw-alias-bg-layer-1)}.dsh-env-k{font-size:11px;color:var(--dsw-alias-label-tertiary);margin-bottom:5px}.dsh-env-v{font-size:14px;font-weight:550;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.dsh-env-row{display:grid;grid-template-columns:140px minmax(0,1fr) auto;align-items:center;gap:12px;padding:12px 16px;border-bottom:1px solid var(--dsw-alias-border-l2)}.dsh-env-row:last-child{border-bottom:0}.dsh-env-label{font-size:13px;color:var(--dsw-alias-label-secondary)}.dsh-env-path{font:12px/20px ui-monospace,SFMono-Regular,Consolas,monospace;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.dsh-env-actions{display:flex;gap:6px}.dsh-env button{height:30px;padding:0 11px;border:1px solid var(--dsw-alias-border-l2);border-radius:7px;background:var(--dsw-alias-bg-layer-1);color:inherit;font:12px inherit;cursor:pointer}.dsh-env button:hover{background:var(--dsw-alias-interactive-bg-hover)}.dsh-env button[data-primary=true]{background:var(--dsw-alias-brand-primary);color:white;border-color:transparent}.dsh-env button:disabled{opacity:.45;cursor:default}.dsh-env-note{padding:11px 16px;color:var(--dsw-alias-label-tertiary);font-size:12px;line-height:19px}.dsh-env-status{font-size:12px;color:var(--dsw-alias-state-success-primary)}.dsh-env-status[data-error=true]{color:var(--dsw-alias-state-error-primary)}@media(max-width:760px){.dsh-env-grid{grid-template-columns:1fr}.dsh-env-row{grid-template-columns:1fr}.dsh-env-actions{justify-content:flex-start}}`;
  function installCss(){if(document.querySelector("style[data-dsh-env-settings]"))return;const s=document.createElement("style");s.dataset.dshEnvSettings="";s.textContent=css;document.head.appendChild(s)}
  function humanPath(value){return String(value||"").replace(/^\\\\\?\\UNC\\/i,"\\\\").replace(/^\\\\\?\\/,"")}
  function ContextSettings({api,renderSlot}) {
   const [state,setState]=React.useState({loading:true,host:null,paths:{},runtime:null,revision:0,workspaces:[],error:""});
   const [draft,setDraft]=React.useState({}),[picking,setPicking]=React.useState(null),[status,setStatus]=React.useState(""),[busy,setBusy]=React.useState(false);
   const busyRef=React.useRef(false);
   const load=React.useCallback(async()=>{
    try {
     const [h,s,w,response]=await Promise.all([api.host.describe({}),api.settings.describe({}),api.workspace.list({}),fetch("/__dsh-runtime")]);
     for(const reply of [h,s,w])if(!reply.result.ok)throw new Error(reply.result.error.message);
     const runtime=await response.json();if(!response.ok)throw new Error(runtime.error||"无法读取运行环境");
     const section=s.result.value.namespaces.find(x=>x.ns==="storage-paths");
     setDraft(section?.value??{});setState({loading:false,host:h.result.value,paths:section?.value??{},revision:section?.revision??0,runtime,workspaces:w.result.value.items??[],error:runtime.migrationError?"迁移未全部完成："+runtime.migrationError+"；当前目录可继续使用。":""});
     return {runtime,paths:section?.value??{}};
    }catch(error){setState(x=>({...x,loading:false,error:error instanceof Error?error.message:String(error)}));throw error}
   },[api]);
   React.useEffect(()=>{load().catch(()=>{})},[load]);
   const act=async action=>{if(busyRef.current)return;busyRef.current=true;setBusy(true);setStatus("");try{await action()}catch(error){setStatus("操作失败："+(error instanceof Error?error.message:String(error)))}finally{busyRef.current=false;setBusy(false)}};
   const copy=value=>act(async()=>{await navigator.clipboard.writeText(humanPath(value));setStatus("路径已复制")});
   const open=value=>act(async()=>{const reply=await api.host.openPath({path:value});if(!reply.result.ok)throw new Error(reply.result.error.message)});
   const save=()=>act(async()=>{
    const reply=await api.settings.mutate({ns:"storage-paths",ops:Object.entries(draft).map(([name,value])=>({op:"set",path:[name],value:value.trim()})),expectedRevision:state.revision});
    if(!reply.result.ok)throw new Error(reply.result.error.message);
    await load();setStatus("目录设置已保存，重启时自动迁移；原目录中的数据将保留。");
   });
   const restart=()=>act(async()=>{
    const response=await fetch("/__dsh-runtime",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({action:"restart"})});
    const result=await response.json();if(!response.ok)throw new Error(result.error||"重启失败");
    const previousInstance=result.instanceId??state.runtime?.instanceId;
    if(!previousInstance)throw new Error("Host 未提供重启实例标识，请使用启动器重启");
    setStatus("正在重启并迁移数据…");
    await new Promise(resolve=>setTimeout(resolve,1200));
    for(let i=0;i<120;i++){
     await new Promise(resolve=>setTimeout(resolve,1000));
     let runtime;
     try{const response=await fetch("/__dsh-runtime",{cache:"no-store",signal:AbortSignal.timeout(5000)});if(response.ok)runtime=await response.json()}catch{}
     if(runtime?.restartError)throw new Error(runtime.restartError);
     if(runtime?.instanceId&&runtime.instanceId!==previousInstance){
      const loaded=await load();
      if(loaded.runtime.migrationError)throw new Error("迁移未完成，已继续使用原目录："+loaded.runtime.migrationError);
      const mismatched=Object.keys(loaded.paths).some(name=>humanPath(loaded.paths[name])!==humanPath(loaded.runtime.paths?.[name]));
      if(mismatched)throw new Error("服务已重启，但目录设置尚未全部生效");
      setStatus("重启完成，当前目录已更新。");return;
     }
    }
    throw new Error("重启尚未完成，请通过启动器查看运行日志；原数据仍保留。");
   });
   const pathRow=(label,value,field)=>jsx.jsxs("div",{className:"dsh-env-row",children:[
    jsx.jsx("label",{className:"dsh-env-label",htmlFor:field?"dsh-path-"+field:void 0,children:label}),
    field?jsx.jsxs("div",{children:[jsx.jsx("input",{id:"dsh-path-"+field,className:"dsh-env-path",style:{width:"100%",boxSizing:"border-box",background:"var(--dsw-alias-bg-layer-1)",color:"inherit",border:"1px solid var(--dsw-alias-border-l2)",borderRadius:6,padding:6},value:humanPath(draft[field]),disabled:busy,onChange:e=>setDraft(x=>({...x,[field]:e.target.value}))}),state.runtime?.paths?.[field]&&humanPath(state.runtime.paths[field])!==humanPath(draft[field])&&jsx.jsx("div",{className:"dsh-env-k",children:"当前使用："+humanPath(state.runtime.paths[field])})]}):jsx.jsx("span",{className:"dsh-env-path",title:humanPath(value),children:humanPath(value)||"未配置"}),
    jsx.jsxs("span",{className:"dsh-env-actions",children:[field&&jsx.jsx("button",{disabled:busy,onClick:()=>setPicking(field),children:"选择"}),value&&jsx.jsx("button",{disabled:busy,onClick:()=>copy(value),children:"复制"}),value&&state.host?.canOpenPath&&jsx.jsx("button",{disabled:busy,onClick:()=>open(value),children:"打开"})]})]},field||label);
   const flow=picking?renderSlot("settings.paths.directoryFlow",{open:true,busy,onPicked:path=>{setDraft(x=>({...x,[picking]:path}));setPicking(null)},onCancel:()=>setPicking(null),onError:error=>{setStatus(String(error));setPicking(null)}}):null;
   const changed=Object.keys(draft).some(k=>humanPath(draft[k])!==humanPath(state.paths[k]));
   if(state.loading)return jsx.jsx("section",{className:"dsh-env",children:"正在读取运行环境…"});
   return jsx.jsxs("section",{className:"dsh-env",children:[
    jsx.jsx("h2",{children:"目录与运行环境"}),
    jsx.jsx("p",{className:"dsh-env-intro",children:"目录更改在重启后生效。迁移会校验文件并保留原数据；目标目录必须为空，运行中的任务需要先完成。"}),
    state.error&&jsx.jsx("div",{className:"dsh-env-status","data-error":true,role:"alert",children:state.error}),
    jsx.jsxs("div",{className:"dsh-env-section",children:[jsx.jsx("div",{className:"dsh-env-head",children:"运行概览"}),jsx.jsx("div",{className:"dsh-env-grid",children:[["Host 版本",state.host?.version],["活动会话",String(state.host?.attachedSessions??0)],["Node 运行程序",state.runtime?.nodeCommand||"node"],["Provider",state.host?.provider??"未设置"],["模型",state.host?.model??"未设置"],["工作区",String(state.workspaces.length)]].map(([k,v])=>jsx.jsxs("div",{className:"dsh-env-stat",children:[jsx.jsx("div",{className:"dsh-env-k",children:k}),jsx.jsx("div",{className:"dsh-env-v",title:v,children:v})]},k))})]}),
    jsx.jsxs("div",{className:"dsh-env-section",children:[jsx.jsx("div",{className:"dsh-env-head",children:"存储目录"}),pathRow("Host 启动目录",state.host?.cwd),...[["正式数据根","dataDirectory"],["缓存目录","cacheDirectory"],["运行环境目录","environmentDirectory"],["测试运行目录","testDirectory"]].map(([label,field])=>pathRow(label,state.runtime?.paths?.[field],field)),jsx.jsxs("div",{className:"dsh-env-note",children:[jsx.jsx("button",{"data-primary":true,disabled:busy||!changed||Object.values(draft).some(value=>!String(value).trim()),onClick:save,children:busy?"处理中…":"保存目录设置"})," ",jsx.jsx("button",{disabled:busy||changed||!state.runtime?.restartSupported,onClick:restart,children:"重启并应用"}),jsx.jsx("p",{children:"运行环境目录中的 node 或 bin/node（Windows 为 node.exe）优先用于代码运行；未配置时使用系统 Node。工作区文件保持在各自项目目录。"})]})]}),
    status&&jsx.jsx("div",{className:"dsh-env-status","data-error":status.startsWith("操作失败"),role:status.startsWith("操作失败")?"alert":"status",children:status}),
    jsx.jsxs("div",{className:"dsh-env-section",children:[jsx.jsx("div",{className:"dsh-env-head",children:"工作区"}),...state.workspaces.map(w=>pathRow(w.title,w.path)),state.workspaces.length===0&&jsx.jsx("div",{className:"dsh-env-note",children:"暂无工作区"})]}),flow
   ]});
  }
  function apply(ctx){installCss();ctx.slots.inject("settings.section",()=>ctx.slots.register({name:"settings.section",id:"workbench-context",order:15,label:"目录与运行环境",children:{"settings.paths.directoryFlow":{kind:"single",scope:"root"}}},({renderSlot})=>jsx.jsx(ContextSettings,{api:ctx.get("connection").api,renderSlot})))}
  return {apply,inject:["slots","connection"]}
 }
});
