window.__ModuleLoader__.load({
  id: "@deepseek-ai/dsh-client-ui-settings-skill-revisions",
  factory: require => {
    const React = require("react"), { jsx, jsxs } = require("react/jsx-runtime");
    const style = ".dshSkillVersions{display:flex;flex-direction:column;gap:14px;padding:4px 0 24px;color:var(--dsw-alias-label-primary)}.dshSkillVersions h2,.dshSkillVersions p{margin:0}.dshSkillVersions p{line-height:1.6}.dshSkillVersions article,.dshSkillVersions form{display:flex;flex-direction:column;gap:10px;border:1px solid var(--dsw-alias-border-l2);border-radius:10px;padding:14px}.dshSkillVersions label{display:flex;flex-direction:column;gap:5px}.dshSkillVersions input,.dshSkillVersions textarea,.dshSkillVersions button{font:inherit;color:inherit;background:var(--dsw-alias-bg-layer-1);border:1px solid var(--dsw-alias-border-l2);border-radius:7px;padding:7px 10px;box-sizing:border-box}.dshSkillVersions textarea{min-height:120px;width:100%;resize:vertical}.dshSkillVersions button{cursor:pointer}.dshSkillVersions button:disabled{opacity:.5;cursor:default}.dshSkillVersions input:focus-visible,.dshSkillVersions textarea:focus-visible,.dshSkillVersions button:focus-visible{outline:2px solid var(--dsw-alias-state-business-primary)}.dshSkillVersions small{color:var(--dsw-alias-label-tertiary);overflow-wrap:anywhere}.dshSkillVersions .actions{display:flex;gap:8px;flex-wrap:wrap}.dshSkillVersions [role=alert]{color:var(--dsw-alias-state-error-primary)}.dshSkillVersions pre{white-space:pre-wrap;overflow-wrap:anywhere;max-height:320px;overflow:auto}";
    if (typeof document !== "undefined" && !document.querySelector("style[data-skill-revisions]")) {
      const element = document.createElement("style"); element.dataset.skillRevisions = "1"; element.textContent = style; document.head.appendChild(element);
    }
    const empty = () => ({ name:"", description:"", project:"", ownerSessionId:"", sourceEvidence:"", content:"" });
    function SkillRevisionSection({rpc}) {
      const [state,setState]=React.useState(null),[draft,setDraft]=React.useState(null),[busy,setBusy]=React.useState(false),[error,setError]=React.useState(""),[notice,setNotice]=React.useState("");
      const [sampleDraft,setSampleDraft]=React.useState(null),[view,setView]=React.useState(null),[confirm,setConfirm]=React.useState(null);
      const live=React.useRef(true),request=React.useRef(0);
      const load=React.useCallback(async()=>{const id=++request.current;const value=await rpc("capabilities.skillRevisionList",{});if(live.current&&id===request.current)setState(value);},[rpc]);
      React.useEffect(()=>{live.current=true;load().catch(e=>setError(e.message));return()=>{live.current=false;request.current++;};},[load]);
      const act=async action=>{if(busy)return;setBusy(true);setError("");setNotice("");try{await action();await load();}catch(e){if(live.current)setError(e.message||String(e));}finally{if(live.current)setBusy(false);}};
      const mutate=(suffix,payload)=>rpc("capabilities.skillRevision"+suffix,{...payload,expectedRevision:state.revision});
      const field=(name,label,multiline=false)=>jsx("label",{children:jsxs(React.Fragment,{children:[label,jsx(multiline?"textarea":"input",{value:draft[name],required:true,onChange:e=>setDraft({...draft,[name]:e.target.value})})]})},name);
      return jsxs("section",{className:"dshSkillVersions",children:[
        jsx("h2",{children:"技能候选与版本"}),
        jsx("p",{children:"候选不会自动加入模型上下文。通过正向、反向任务验证后，可在适用项目中启用；旧版本恢复也会重新核验。"}),
        state&&jsxs("label",{children:[jsx("input",{type:"checkbox",role:"switch",checked:state.enabled!==false,disabled:busy,onChange:e=>{const enabled=e.target.checked;act(async()=>{await mutate("Toggle",{enabled});setNotice(enabled?"技能候选已启用。":"已停用技能候选加载，历史与手动技能保持可用。");});}}),"启用技能候选与已验证版本"]}),
        jsxs("div",{className:"actions",children:[jsx("button",{disabled:busy||state?.enabled===false,onClick:()=>setDraft(empty()),children:"创建候选"}),jsx("button",{disabled:busy,onClick:()=>act(load),children:"刷新"})]}),
        error&&jsx("p",{role:"alert",children:error}),notice&&jsx("p",{role:"status",children:notice}),
        draft&&jsxs("form",{onSubmit:e=>{e.preventDefault();act(async()=>{await mutate("Create",{...draft,sourceEvidence:draft.sourceEvidence.split(/\r?\n/).map(s=>s.trim()).filter(Boolean)});setDraft(null);setNotice("候选已保存，尚未启用。");});},children:[
          field("name","名称"),field("description","适用场景"),field("project","适用项目绝对路径"),field("ownerSessionId","验证任务所属会话"),field("sourceEvidence","来源证据（每行一项）",true),field("content","技能正文",true),
          jsxs("div",{className:"actions",children:[jsx("button",{type:"submit",disabled:busy||!state,children:"保存候选"}),jsx("button",{type:"button",onClick:()=>setDraft(null),children:"取消"})]})
        ]}),
        state&&state.candidates.length===0&&jsx("p",{children:"尚无技能候选。现有手动技能仍在“技能与 MCP”中管理。"}),
        ...(state?.candidates||[]).map(item=>jsxs("article",{children:[
          jsxs("strong",{children:[item.name," · ",item.active?"已启用":item.withdrawn?"已撤回":item.validation?"验证记录已保存":"待验证"]}),
          jsx("p",{children:item.description}),jsx("small",{children:item.project}),jsx("small",{children:"内容标识："+item.contentHash}),
          item.validation&&jsx("small",{children:`正向 ${item.validation.positiveSamples} 项，反向 ${item.validation.negativeSamples} 项；使用前会核对环境及证据是否仍有效。`}),
          jsxs("div",{className:"actions",children:[
            jsx("button",{disabled:busy,onClick:()=>act(async()=>setView(await rpc("capabilities.skillRevisionRead",{id:item.id}))),children:"查看正文"}),
            !item.withdrawn&&jsx("button",{disabled:busy,onClick:()=>setSampleDraft({id:item.id,positive:"",positiveRevision:"1",negative:"",negativeRevision:"1"}),children:"验证任务"}),
            !item.withdrawn&&!item.active&&jsx("button",{disabled:busy||!item.validation||state.enabled===false,onClick:()=>setConfirm({id:item.id,action:"Activate"}),children:"启用此版本"}),
            item.withdrawn&&jsx("button",{disabled:busy||!item.validation||state.enabled===false,onClick:()=>setConfirm({id:item.id,action:"Restore"}),children:"重新验证并恢复"}),
            !item.withdrawn&&jsx("button",{disabled:busy,onClick:()=>setConfirm({id:item.id,action:"Withdraw"}),children:"撤回"}),
            !item.active&&jsx("button",{disabled:busy,onClick:()=>setConfirm({id:item.id,action:"Remove"}),children:"删除候选"})
          ]}),
          confirm?.id===item.id&&jsxs("div",{className:"actions",children:[jsx("span",{children:["Activate","Restore"].includes(confirm.action)?"启用前将重新检查验证结果和适用环境。":confirm.action==="Withdraw"?"撤回后不会继续加载此版本，历史仍保留。":"删除此候选记录？"}),jsx("button",{disabled:busy,onClick:()=>act(async()=>{await mutate(confirm.action,{id:item.id});setConfirm(null);setNotice("版本状态已更新。");}),children:"确认"}),jsx("button",{onClick:()=>setConfirm(null),children:"取消"})]}),
          sampleDraft?.id===item.id&&jsxs("form",{onSubmit:e=>{e.preventDefault();act(async()=>{await mutate("Validate",{id:item.id,samples:[{taskId:sampleDraft.positive,revision:Number(sampleDraft.positiveRevision),expectedSuccess:true},{taskId:sampleDraft.negative,revision:Number(sampleDraft.negativeRevision),expectedSuccess:false}]});setSampleDraft(null);setNotice("验证完成，候选尚未自动启用。");});},children:[
            jsx("p",{children:"测试任务必须绑定此技能内容标识，并保存独立验收结果；任务失败的反向样本也需完成验收。"}),
            ...[["positive","正向任务 ID"],["positiveRevision","正向任务版本"],["negative","反向任务 ID"],["negativeRevision","反向任务版本"]].map(([name,label])=>jsxs("label",{children:[label,jsx("input",{required:true,type:name.endsWith("Revision")?"number":"text",min:1,value:sampleDraft[name],onChange:e=>setSampleDraft({...sampleDraft,[name]:e.target.value})})]},name)),
            jsxs("div",{className:"actions",children:[jsx("button",{type:"submit",disabled:busy,children:"开始验证"}),jsx("button",{type:"button",onClick:()=>setSampleDraft(null),children:"取消"})]})
          ]}),
          view?.id===item.id&&jsxs(React.Fragment,{children:[jsx("pre",{children:view.content}),jsx("button",{onClick:()=>setView(null),children:"收起正文"})]})
        ]},item.id))
      ]});
    }
    return {inject:["slots","connection"],apply(ctx){const connection=ctx.get("connection");const rpc=async(method,payload)=>{const response=await connection.rpc.call("/api",method,payload);const result=response.result||response;if(!result.ok)throw new Error(result.error?.message||"请求失败");return result.value;};ctx.slots.inject("settings.section",()=>ctx.slots.register({name:"settings.section",id:"skill-revisions",order:26,label:()=>"技能候选与版本",inject:()=>({rpc})},SkillRevisionSection));},SkillRevisionSection};
  }
});
