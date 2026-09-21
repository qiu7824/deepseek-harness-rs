window.__ModuleLoader__.load({
  id:"@deepseek-ai/dsh-client-ui-workbench-previews",
  factory:require=>{
    const React=require("react"),h=React.createElement;
    const {MarkdownText}=require("@deepseek-ai/dsh-client-ui-primitives");
    const {trackPromptRequest,pendingPromptRequestIds,cancelPromptRequests,pendingPromptStop,stopPromptRequests}=require("@deepseek-ai/dsh-client-runtime/client");
    const plans=new Map();
    const drafts=new Map(),pendingSends=new Map(),fileIdentities=new WeakMap();
    const fileIdentity=file=>{let id=fileIdentities.get(file);if(!id){id=crypto.randomUUID();fileIdentities.set(file,id);}return id;};
    function childState(history){
      let status="inactive",stream="",complete=!history?.hasMore;const queues={"next-turn":[],"next-step":[]};
      for(const row of history?.events||[]){const event=row.event||row,data=event.data||{};
        if(event.type==="turn/start")status="running";
        if(event.type==="step/start")stream="";
        if(event.type==="assistant/chunk"&&data.chunk?.type==="text-delta")stream=(stream+(data.chunk.text||"")).slice(-65536);
        if(event.type==="assistant/message")stream="";
        if(event.type==="turn/end"){status=data.reason?.kind||"inactive";stream="";}
        if(event.type==="agent/inbox/spliced"&&queues[data.target]){const queue=queues[data.target],start=Number(data.start)||0,removed=Number(data.removedCount)||0;if(start>queue.length||removed>queue.length-start)complete=false;queue.splice(start,removed,...(data.inserted||[]));}
      }
      return {status,stream,complete,queue:[...queues["next-step"],...queues["next-turn"]].slice(0,20)};
    }
    function lineCounts(before,after){
      const a=(before||"").split(/\r?\n/),b=(after||"").split(/\r?\n/);if(!before)a.length=0;if(!after)b.length=0;
      if(a.at(-1)==="")a.pop();if(b.at(-1)==="")b.pop();
      if(a.length*b.length>1000000)return null;
      let row=new Uint32Array(b.length+1);
      for(const line of a){const next=new Uint32Array(b.length+1);for(let j=0;j<b.length;j++)next[j+1]=line===b[j]?row[j]+1:Math.max(row[j+1],next[j]);row=next;}
      return {added:b.length-row[b.length],deleted:a.length-row[b.length]};
    }
    function eventText(event){
      const data=event.data||{},content=data.message?.content||data.content||data.result?.content||[];
      if(Array.isArray(content))return content.filter(part=>part.type==="text").map(part=>part.text).join("\n");
      return typeof data.text==="string"?data.text:"";
    }
    function SideConversation(props){
      const address=props.tab.meta?.address;
      return h(SideConversationView,{...props,key:JSON.stringify(address?[address.parentSessionId,address.childSessionId,address.mode]:null)});
    }
    function SideConversationView({tab,rpc,onMain}){
      const address=tab.meta?.address,key=address?JSON.stringify(address):"";
      const [view,setView]=React.useState(null),[text,setText]=React.useState(()=>drafts.get(key)||""),[files,setFiles]=React.useState([]),[error,setError]=React.useState(""),[busy,setBusy]=React.useState(false),[stopping,setStopping]=React.useState(false),[delivery,setDelivery]=React.useState("queue"),[receipt,setReceipt]=React.useState(null);
      const active=React.useRef(false),generation=React.useRef(0),request=React.useRef(null),draft=React.useRef(text),sendTask=React.useRef(null),sendEpoch=React.useRef(0),sendDispatched=React.useRef(false),stopTask=React.useRef(null),stopRetryIds=React.useRef([]);draft.current=text;
      React.useEffect(()=>{
        if(!address)return;active.current=true;const epoch=++generation.current,abort=new AbortController();let timer,alive=true;
        const current=()=>alive&&active.current&&generation.current===epoch;
        const poll=async()=>{try{const next=await rpc("subagent.history",{...address,maxMessages:40},abort.signal);if(current())setView(next);}catch(e){if(current()&&e.name!=="AbortError")setError(e.message);}finally{if(current())timer=setTimeout(poll,document.hidden?10000:1800);}};
        void poll();return()=>{alive=false;active.current=false;generation.current++;abort.abort();clearTimeout(timer);if(draft.current.trim()){while(drafts.size>=16&&!drafts.has(key))drafts.delete(drafts.keys().next().value);drafts.set(key,draft.current.slice(0,65536));}else drafts.delete(key);};
      },[key,rpc]);
      const send=()=>{
        if(sendTask.current||stopTask.current||(!text.trim()&&!files.length))return;setBusy(true);setError("");
        const epoch=generation.current,attempt=++sendEpoch.current,current=()=>active.current&&generation.current===epoch&&sendEpoch.current===attempt,abort=new AbortController();request.current=abort;sendDispatched.current=false;
        const operation=(async()=>{
          let admission,ticket;
          try{const identity=JSON.stringify({text,delivery,files:files.map(file=>[fileIdentity(file),file.name,file.size,file.lastModified])});let pending=pendingSends.get(key);if(pending?.identity!==identity){while(pendingSends.size>=16&&!pendingSends.has(key))pendingSends.delete(pendingSends.keys().next().value);pending={identity,requestId:crypto.randomUUID()};pendingSends.set(key,pending);}admission=pending;ticket=trackPromptRequest(address,pending.requestId);
            const content=text.trim()?[{type:"text",text}]:[];for(const file of files){const data=await new Promise((resolve,reject)=>{const reader=new FileReader();reader.onload=()=>resolve(String(reader.result).split(",")[1]);reader.onerror=()=>reject(new Error("附件读取失败"));reader.readAsDataURL(file)});if(abort.signal.aborted||ticket.cancelled())return;content.push({type:["image/png","image/jpeg","image/webp","image/gif"].includes(file.type)?"image":"file",name:file.name,data,mediaType:file.type||"application/octet-stream"});}
            let stopping;while((stopping=pendingPromptStop(address))){const result=await stopping;if(!result.ok)throw Object.assign(new Error(result.error.message),result.error);}
            if(abort.signal.aborted||ticket.cancelled())return;
            sendDispatched.current=true;
            const value=await rpc("subagent.prompt",{...address,content,requestId:pending.requestId,delivery,clientTimeZone:Intl.DateTimeFormat().resolvedOptions().timeZone},abort.signal);
            if(ticket.cancelled())return;
            if(current()&&pendingSends.get(key)===pending)pendingSends.delete(key);
            if(current()){if(draft.current===text){draft.current="";setText("");drafts.delete(key);}setFiles([]);if(!stopTask.current)setReceipt(value);}
          }catch(e){if(e.code&&!["internal","transport-error"].includes(e.code)&&pendingSends.get(key)===admission)pendingSends.delete(key);if(current()&&!ticket?.cancelled()&&e.name!=="AbortError")setError(e.message);return {cancelled:e.code==="cancelled"};}finally{if(ticket?.cancelled()&&pendingSends.get(key)===admission)pendingSends.delete(key);ticket?.release();if(current())setBusy(false)}
        })();
        sendTask.current=operation;void operation.finally(()=>{if(sendTask.current===operation)sendTask.current=null;});
      };
      const stop=()=>{
        setStopping(true);setError("");
        const epoch=generation.current,current=()=>active.current&&generation.current===epoch,pending=sendTask.current,abort=request.current,attempt=sendEpoch.current,requestId=pendingSends.get(key)?.requestId,requestIds=[...new Set([...stopRetryIds.current,...pendingPromptRequestIds(address),...(requestId?[requestId]:[])])];
        cancelPromptRequests(address,requestIds);stopRetryIds.current=requestIds;
        if(pending&&!sendDispatched.current)abort?.abort();
        const stopping=stopPromptRequests(address,requestIds,async ids=>({ok:true,value:await rpc("subagent.interrupt",{...address,requestIds:ids})}));
        const operation=stopping.then(result=>{
          if(!result.ok)throw Object.assign(new Error(result.error.message),result.error);
          stopRetryIds.current=stopRetryIds.current.filter(id=>!requestIds.includes(id));
          if(requestId&&pendingSends.get(key)?.requestId===requestId)pendingSends.delete(key);
          if(sendTask.current===pending&&sendEpoch.current===attempt){sendEpoch.current++;sendTask.current=null;abort?.abort();if(request.current===abort)request.current=null;if(current())setBusy(false);}
          if(current())setReceipt({status:"停止请求已发送"});
        }).catch(e=>{if(current())setError(e.message)}).finally(()=>{if(stopTask.current===operation){stopTask.current=null;if(current())setStopping(false)}});
        stopTask.current=operation;return operation;
      };
      if(!address)return h("p",{role:"alert"},"子代理地址已失效，请从成员目录重新打开。");
      const messages=(view?.events||[]).map(row=>row.event||row).filter(event=>["user/message","assistant/message","tool/result"].includes(event.type));
      const progress=childState(view),status={running:"运行中",completed:"已完成",error:"失败",blocked:"等待处理",aborted:"已停止",interrupted:"已停止",inactive:"空闲"}[progress.status]||progress.status;
      return h("section",{className:"dshPreviewPanel","aria-label":"子代理对话"},
        h("header",null,h("strong",null,tab.title),h("button",{onClick:()=>onMain(address)},"在主区打开"),h("button",{disabled:address.mode!=="continuable",onClick:stop},stopping?"正在停止…":"停止")),
        h("small",null,`父会话：${address.parentSessionId} · ${address.mode==="continuable"?"可续聊":"只读记录"} · ${status}`),
        error&&h("p",{role:"alert"},error),!view&&h("p",{role:"status"},"正在加载子代理对话…"),
        view?.hasMore&&h("p",null,"显示最近 40 条消息；完整历史可在主区查看。"),
        ...messages.map(event=>h("article",{key:event.seq,"data-event-seq":event.seq},h("strong",null,event.type==="user/message"?"用户":event.type==="assistant/message"?"助手":"工具"),h(MarkdownText,{text:eventText(event),codeLabels:{copyLabel:"复制",copiedLabel:"已复制"}}))),
        progress.stream&&h("article",{"aria-label":"助手正在输出"},h(MarkdownText,{text:progress.stream,codeLabels:{copyLabel:"复制",copiedLabel:"已复制"}})),
        progress.queue.length>0&&h("details",null,h("summary",null,progress.complete?`待处理消息 ${progress.queue.length}`:"最近页中的队列记录"),...progress.queue.map((message,index)=>h("p",{key:message.id||index},(message.content||[]).filter(part=>part.type==="text").map(part=>part.text).join("\n")))),
        receipt&&h("p",{role:"status"},receipt.status||"消息已提交；执行状态以子代理响应为准。"),
        address.mode==="continuable"&&h("form",{onSubmit:event=>{event.preventDefault();send()}},
          h("textarea",{value:text,maxLength:65536,"aria-label":"发送给子代理",placeholder:"继续子代理对话",onChange:event=>setText(event.target.value),style:{width:"100%",minHeight:100,boxSizing:"border-box"},onKeyDown:event=>{if(event.key==="Enter"&&(event.ctrlKey||event.metaKey)&&!event.nativeEvent?.isComposing){event.preventDefault();send();}}}),
          h("label",null,"附件",h("input",{type:"file",multiple:true,disabled:busy||stopping,onChange:event=>{const chosen=Array.from(event.target.files||[]);if(chosen.length>8||chosen.reduce((sum,file)=>sum+file.size,0)>16*1024*1024){setError("附件最多 8 个，总大小不超过 16 MiB");return;}setFiles(chosen);}})),
          files.length>0&&h("p",null,files.map(file=>file.name).join("、")),
          h("select",{value:delivery,"aria-label":"消息投递方式",onChange:event=>setDelivery(event.target.value)},h("option",{value:"queue"},"加入队列"),h("option",{value:"steer"},"引导当前回合")),
          h("button",{type:"submit",disabled:busy||stopping||(!text.trim()&&!files.length)},busy?"提交中…":"发送"),
          h("p",null,"需要审批时，可在主区打开此子代理处理；父会话保持独立。")));
    }
    function planFromBlock(block){
      try{const call=block.call||block,args=JSON.parse(call.argsRaw||"{}");return typeof args.plan==="string"?{text:args.plan,id:String(call.callId||call.id||call.seq||""),settled:"kind" in block}:null;}catch{return null;}
    }
    function PlanCard({block,openPlan}){
      const plan=planFromBlock(block),[error,setError]=React.useState("");if(!plan)return null;
      return h("div",{className:"dshPreviewCard"},h("strong",null,plan.text.split("\n")[0].replace(/^#+\s*/,"")||"计划"),h("button",{type:"button",onClick:()=>{try{openPlan(plan)}catch(e){setError(e.message)}}},"预览计划"),h("small",null,"批准与拒绝在原计划审批卡中处理。"),error&&h("p",{role:"alert"},error));
    }
    function PlanPreview({tab,scope,onSource}){
      const plan=plans.get(tab.meta?.key),[notice,setNotice]=React.useState("");
      if(!plan)return h("section",{className:"dshPreviewPanel"},h("p",{role:"status"},"此计划预览已失效，请从原计划卡重新打开。"),h("button",{onClick:()=>onSource(scope.sessionId)},"返回来源"));
      return h("section",{className:"dshPreviewPanel"},h("header",null,h("strong",null,tab.title),h("button",{onClick:()=>navigator.clipboard.writeText(plan.text).then(()=>setNotice("已复制"),e=>setNotice(e.message))},"复制"),h("button",{onClick:()=>onSource(scope.sessionId)},"返回来源")),h("p",{role:"status"},notice||"计划内容快照；审批状态以原计划卡为准。"),h(MarkdownText,{text:plan.text,codeLabels:{copyLabel:"复制",copiedLabel:"已复制"}}));
    }
    function apply(ctx){
      const style=document.createElement("style");style.textContent=".dshPreviewPanel{box-sizing:border-box;overflow:auto;height:100%;padding:16px;color:var(--dsw-alias-label-primary)}.dshPreviewPanel header,.dshPreviewCard{display:flex;gap:10px;align-items:center;flex-wrap:wrap}.dshPreviewPanel button,.dshPreviewCard button{font:inherit;color:inherit;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;padding:6px 10px;background:var(--dsw-alias-bg-layer-1);cursor:pointer}.dshPreviewCard small{color:var(--dsw-alias-label-tertiary)}.dshPreviewPanel :focus-visible,.dshPreviewCard :focus-visible{outline:2px solid var(--dsw-alias-state-business-primary);outline-offset:2px}";style.textContent+=".dshPreviewPanel{font:inherit;line-height:1.6}.dshPreviewPanel :is(input:not([type=checkbox]),select,textarea){box-sizing:border-box;max-width:100%;min-width:0;min-height:38px;padding:8px 10px;font:inherit;color:inherit;background:var(--dsw-alias-bg-layer-1);border:1px solid var(--dsw-alias-border-l2);border-radius:8px}.dshPreviewPanel :is(button,input,select,textarea):disabled{opacity:.5;cursor:default}.dshPreviewPanel input[type=file]{padding:5px}.dshPreviewPanel input[type=file]::file-selector-button{font:inherit;color:inherit;background:var(--dsw-alias-bg-layer-2);border:0;border-radius:6px;padding:6px 10px;margin-right:8px;cursor:pointer}";document.head.appendChild(style);ctx.effect(()=>()=>{style.remove();plans.clear()},"workbench preview lifecycle");
      ctx.inject(["betterSidebar","sessions"],owner=>{
        const sidebar=owner.get("betterSidebar");
        const rpc=async(method,payload,signal)=>{const result=await owner.get("connection").rpc.call("/api",method,payload,signal);if(!result.ok){const error=new Error(result.error?.message||"请求失败");error.code=result.error?.code;throw error;}return result.value;};
        const removeChild=sidebar.registerTab({id:"suite:child-chat",title:"子代理对话",order:61,component:props=>h(SideConversation,{...props,key:JSON.stringify(props.tab.meta?.address),rpc,onMain:address=>owner.sessions.openSubagent(address)})});
        owner.effect(()=>()=>{removeChild();drafts.clear();pendingSends.clear()},"child conversation tab");
        const remove=sidebar.registerTab({id:"suite:plan-preview",title:"计划",order:60,component:props=>h(PlanPreview,{...props,onSource:id=>owner.sessions.open(id)}),onClose:tab=>plans.delete(tab.meta?.key)});
        owner.effect(()=>remove,"plan preview tab");
        owner.slots.inject("tool.call.toolview",()=>owner.slots.register({name:"tool.call.toolview",key:"exit_plan_mode",inject:sessionId=>({openPlan:plan=>{
          if(plan.text.length>512*1024)throw new Error("计划超过预览上限，请在原计划卡中查看。 ");
          const key=sessionId+":"+crypto.randomUUID();
          while(plans.size>=8)plans.delete(plans.keys().next().value);
          plans.set(key,plan);
          sidebar.openTab({type:"suite:plan-preview",id:key,title:plan.text.split("\n")[0].replace(/^#+\s*/,"").slice(0,120)||"计划",meta:{key,callId:plan.id}},{sessionId});
        }})},PlanCard));
      });
    }
    return {apply,inject:["slots","connection"],test:{planFromBlock,eventText,lineCounts,childState,PlanCard,PlanPreview,SideConversation}};
  }
});
