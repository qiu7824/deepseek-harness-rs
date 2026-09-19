window.__ModuleLoader__.load({
  id:"@deepseek-ai/dsh-client-ui-settings-tool-discovery",
  factory:require=>{
    const React=require("react"),{jsx,jsxs}=require("react/jsx-runtime");
    function ToolDiscoverySection(){
      React.useEffect(()=>{const style=document.createElement("style");style.dataset.toolDiscoveryControls="1";style.textContent=".dshToolDiscovery{font-size:14px;line-height:1.6;color:var(--dsw-alias-label-primary);min-width:0}.dshToolDiscovery h2{font-size:18px}.dshToolDiscovery h2,.dshToolDiscovery p{margin:0}.dshToolDiscovery button,.dshToolDiscovery input:not([type=checkbox]){box-sizing:border-box;max-width:100%;min-width:0;min-height:38px;padding:7px 12px;font:inherit;color:inherit;background:var(--dsw-alias-bg-layer-1);border:1px solid var(--dsw-alias-border-l2);border-radius:8px}.dshToolDiscovery button{cursor:pointer}.dshToolDiscovery button:disabled{opacity:.5;cursor:default}.dshToolDiscovery :focus-visible{outline:2px solid var(--dsw-alias-state-business-primary);outline-offset:2px}.dshToolDiscovery input[type=checkbox]{width:16px;height:16px;flex:none;accent-color:var(--dsw-alias-state-business-primary)}.dshDiscoveryToggle{display:flex;gap:8px;align-items:center}.dshDiscoveryBudgets{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:12px;margin-top:12px}.dshDiscoveryBudgetField{display:grid;gap:6px;min-width:0}.dshToolDiscovery summary{cursor:pointer;min-height:28px}.dshDiscoveryActions{display:flex;gap:8px;flex-wrap:wrap}@media(max-width:600px){.dshDiscoveryBudgets{grid-template-columns:1fr}}";document.head.appendChild(style);return()=>style.remove();},[]);
      const [state,setState]=React.useState(null),[draft,setDraft]=React.useState(null),[busy,setBusy]=React.useState(false),[error,setError]=React.useState(""),[notice,setNotice]=React.useState("");
      const generation=React.useRef(0),mounted=React.useRef(true),controller=React.useRef(null);
      const request=async body=>{const response=await fetch("/__dsh-tool-discovery",{method:body?"POST":"GET",credentials:"same-origin",headers:body?{"Content-Type":"application/json"}:undefined,body:body?JSON.stringify(body):undefined,signal:controller.current?.signal});const value=await response.json();if(!response.ok)throw new Error(value.error||"工具发现设置不可用");return value;};
      const load=React.useCallback(async()=>{const current=++generation.current;const value=await request();if(mounted.current&&current===generation.current){setState(value);setDraft(value.configuration);}},[]);
      React.useEffect(()=>{mounted.current=true;controller.current=new AbortController();load().catch(e=>{if(mounted.current)setError(e.message);});return()=>{mounted.current=false;generation.current++;controller.current.abort();};},[load]);
      const save=async event=>{event.preventDefault();if(busy||!draft)return;setBusy(true);setError("");setNotice("");try{const value=await request({configuration:draft,expectedRevision:state.revision});if(mounted.current){setState(value);setDraft(value.configuration);setNotice(value.restartRequired?"设置已保存，重启 Host 后生效。":"设置已保存。");}}catch(e){if(mounted.current)setError(e.message);}finally{if(mounted.current)setBusy(false);}};
      const number=(key,label,min,max)=>jsxs("label",{className:"dshDiscoveryBudgetField",children:[label,jsx("input",{type:"number",min,max,required:true,value:draft[key],onChange:e=>setDraft({...draft,[key]:Number(e.target.value)})})]},key);
      return jsxs("section",{className:"dshToolDiscovery",style:{display:"flex",flexDirection:"column",gap:14,paddingBottom:24},children:[
        jsx("h2",{children:"工具发现"}),jsx("p",{children:"按需展示工具参数，减少重复搜索与上下文占用。运行环境缓存和权限检查始终独立工作。"}),
        !state&&!error&&jsx("p",{role:"status",children:"正在读取工具发现设置…"}),error&&jsx("p",{role:"alert",children:error}),notice&&jsx("p",{role:"status",children:notice}),
        draft&&jsxs("form",{onSubmit:save,style:{display:"flex",flexDirection:"column",gap:12},children:[
          jsxs("label",{className:"dshDiscoveryToggle",children:[jsx("input",{type:"checkbox",role:"switch",checked:draft.enabled,onChange:e=>setDraft({...draft,enabled:e.target.checked})})," 按需加载工具"]}),
          jsx("p",{children:draft.enabled?"核心工具直接可用，其余工具通过搜索加载并保留到后续轮次。":"全部展示原生工具声明；这不会关闭自动环境诊断或权限检查。"}),
          jsxs("details",{children:[jsx("summary",{children:"高级预算"}),jsxs("div",{className:"dshDiscoveryBudgets",children:[number("eagerLimit","直接展示阈值",0,4096),number("listingChars","目录摘要字符预算",256,32768),number("maxLoaded","延迟工具加载上限",1,256),number("maxSchemaBytes","延迟声明字节预算",4096,1048576)]})]}),
          jsxs("div",{className:"dshDiscoveryActions",children:[jsx("button",{type:"submit",disabled:busy,children:busy?"保存中…":"保存"}),jsx("button",{type:"button",disabled:busy,onClick:()=>{setDraft(state.configuration);setError("");},children:"放弃修改"}),jsx("button",{type:"button",disabled:busy,onClick:()=>load().catch(e=>setError(e.message)),children:"刷新状态"})]})
        ]}),
        state&&jsxs("div",{children:[jsx("p",{children:`当前运行方式：${state.runtime.enabled?"按需加载":"全部展示"}${state.restartRequired?"；有待重启生效的设置":""}`}),jsx("p",{children:`本次 Host：搜索 ${state.runtime.searchRequests||0} 次，命中 ${state.runtime.searchHits||0} 项，精确加载 ${state.runtime.describeRequests||0} 次。`}),jsx("small",{children:"统计仅包含本次 Host 的发现操作，不代表模型账单或任务已完成。"})]})
      ]});
    }
    return {inject:["slots"],apply(ctx){ctx.slots.inject("settings.section",()=>ctx.slots.register({name:"settings.section",id:"tool-discovery",order:27,label:()=>"工具发现"},ToolDiscoverySection));},ToolDiscoverySection};
  }
});
