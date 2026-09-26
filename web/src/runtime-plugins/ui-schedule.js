window.__ModuleLoader__.load({id:"@deepseek-ai/dsh-client-ui-schedule",factory:require=>{
  const React=require("react"),h=React.createElement;
  const {useState,useEffect,useMemo,useRef,useCallback,useSyncExternalStore}=React;
  const zh={title:"定时任务",subtitle:"到点后把任务作为新消息发送到原会话执行。关闭页面、关闭会话或重启应用后仍会按时运行。",create:"新建任务",refresh:"刷新",all:"全部",enabled:"已启用",inactive:"已停用",search:"搜索任务",empty:"还没有定时任务",emptyHint:"可以在这里新建，也可以在对话中让智能体帮你安排，例如“每个工作日 9 点汇总昨天的提交”。",emptyFiltered:"没有符合条件的任务",loading:"正在读取…",name:"名称",namePlaceholder:"留空则使用任务内容的开头",prompt:"任务内容",promptPlaceholder:"到点后要执行的指令，例如：汇总今天的新闻并列出三条要点",frequency:"频率",once:"单次",every:"每隔",daily:"每天",weekly:"每周",cron:"Cron",at:"执行时间",interval:"间隔",minutes:"分钟",hours:"小时",days:"天",time:"时间",weekdays:"星期",timeZone:"时区",cronHint:"5 个字段：分 时 日 月 周，例如 0 9 * * 1-5 表示工作日 9:00",session:"目标会话",sessionHint:"任务会发送到这个会话，由该会话的智能体执行",noSession:"请先添加工作区或新建一个会话",existing:"已有会话",newSession:"新会话",openSession:"打开会话",save:"保存",saving:"正在保存…",cancel:"取消",close:"关闭",runNow:"立即运行",delete:"删除",confirmDelete:"删除后不再运行，已发送的消息保留在会话中。确定删除？",enable:"启用",rules:"规则",records:"运行记录",next:"下次运行",last:"最近运行",never:"尚未运行",none:"无",delivered:"已发送",failed:"发送失败",manual:"手动",createdByUser:"由你创建",createdByAgent:"由智能体创建",records0:"还没有运行记录",loadMore:"加载更多",pruned:"更早的记录已按保留策略清理（{days} 天 / {records} 条）",copyId:"复制消息 ID",copied:"已复制",statusActive:"运行中",statusInactive:"已停用",unsaved:"有未保存的修改",conflict:"任务已被其他操作修改，已刷新为最新内容",sentHint:"“已发送”表示消息已进入会话，不代表任务已完成。",weekdayNames:"周一,周二,周三,周四,周五,周六,周日",listSeparator:"、",colon:"：",everyUnit:"每 {n} {unit}",dailyLabel:"每天 {time}",weeklyLabel:"每{days} {time}",onceLabel:"单次 · {time}",cronLabel:"Cron · {expr}",inMinutes:"{n} 分钟后",inHours:"{n} 小时后",inDays:"{n} 天后",soon:"即将运行",overdue:"等待投递",storageError:"定时任务存储不可用"};
  const en={title:"Scheduled tasks",subtitle:"When due, the task is sent as a new message into its conversation. Tasks keep running after the page, the conversation or the app is closed.",create:"New task",refresh:"Refresh",all:"All",enabled:"Enabled",inactive:"Inactive",search:"Search tasks",empty:"No scheduled tasks yet",emptyHint:"Create one here or ask the agent in a conversation, e.g. “every workday at 9, summarize yesterday's commits”.",emptyFiltered:"No matching tasks",loading:"Loading…",name:"Name",namePlaceholder:"Defaults to the start of the task",prompt:"Task",promptPlaceholder:"The instruction to run when due, e.g. summarize today's news in three points",frequency:"Frequency",once:"Once",every:"Every",daily:"Daily",weekly:"Weekly",cron:"Cron",at:"Run at",interval:"Interval",minutes:"minutes",hours:"hours",days:"days",time:"Time",weekdays:"Weekdays",timeZone:"Time zone",cronHint:"5 fields: minute hour day month weekday, e.g. 0 9 * * 1-5 for workdays at 9:00",session:"Conversation",sessionHint:"The task is sent to this conversation and run by its agent",noSession:"Add a workspace or start a conversation first",existing:"Existing conversations",newSession:"New conversation",openSession:"Open conversation",save:"Save",saving:"Saving…",cancel:"Cancel",close:"Close",runNow:"Run now",delete:"Delete",confirmDelete:"Delete this task? Future runs stop; delivered messages stay in the conversation.",enable:"Enabled",rules:"Rule",records:"Runs",next:"Next run",last:"Last run",never:"Not run yet",none:"None",delivered:"Sent",failed:"Failed",manual:"Manual",createdByUser:"Created by you",createdByAgent:"Created by the agent",records0:"No runs yet",loadMore:"Load more",pruned:"Earlier runs were removed by retention ({days} days / {records} records)",copyId:"Copy message ID",copied:"Copied",statusActive:"Active",statusInactive:"Inactive",unsaved:"Unsaved changes",conflict:"The task changed elsewhere and was reloaded",sentHint:"“Sent” means the message reached the conversation, not that the task finished.",weekdayNames:"Mon,Tue,Wed,Thu,Fri,Sat,Sun",listSeparator:", ",colon:": ",everyUnit:"Every {n} {unit}",dailyLabel:"Daily {time}",weeklyLabel:"{days} {time}",onceLabel:"Once · {time}",cronLabel:"Cron · {expr}",inMinutes:"in {n} min",inHours:"in {n} h",inDays:"in {n} d",soon:"Due now",overdue:"Waiting to send",storageError:"Scheduled task storage unavailable"};
  const ZONES=["Asia/Shanghai","Asia/Hong_Kong","Asia/Taipei","Asia/Tokyo","Asia/Singapore","Europe/London","Europe/Berlin","America/New_York","America/Los_Angeles","UTC"];
  const browserZone=()=>{try{return Intl.DateTimeFormat().resolvedOptions().timeZone||"UTC"}catch{return "UTC"}};
  const fill=(text,values)=>String(text).replace(/\{(\w+)\}/g,(_,key)=>values[key]??"");
  async function call(operation,payload={},signal){
    const response=await fetch(`/__dsh-schedule/${operation}`,{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(payload),signal});
    let value={};try{value=await response.json()}catch{}
    if(!response.ok){const error=Error(value.error||`HTTP ${response.status}`);error.code=value.code;throw error}
    return value;
  }
  async function rpc(method,payload={}){
    const response=await fetch(`/api/${method}`,{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({type:"client-request",rpcId:`schedule-${Date.now()}-${Math.random().toString(36).slice(2)}`,method,payload})});
    const value=await response.json();if(!value?.result?.ok)throw Error(value?.result?.error?.message||`HTTP ${response.status}`);return value.result.value;
  }
  function formatTime(iso,zone){if(!iso)return "";const date=new Date(iso);if(Number.isNaN(date.getTime()))return iso;const now=new Date();const opts={month:"numeric",day:"numeric",hour:"2-digit",minute:"2-digit",hour12:false};if(date.getFullYear()!==now.getFullYear())opts.year="numeric";if(zone)opts.timeZone=zone;try{return new Intl.DateTimeFormat(undefined,opts).format(date)}catch{return date.toLocaleString()}}
  function relative(iso,t){if(!iso)return "";const delta=new Date(iso).getTime()-Date.now();if(delta<=0)return t("soon");if(delta<3600e3)return fill(t("inMinutes"),{n:Math.max(1,Math.round(delta/60e3))});if(delta<24*3600e3)return fill(t("inHours"),{n:Math.round(delta/3600e3)});return fill(t("inDays"),{n:Math.round(delta/86400e3)})}
  const plainPath=path=>String(path||"").replace(/^\\\\\?\\/,"");
  function ruleLabel(rule,t){
    if(!rule)return "";const names=t("weekdayNames").split(",");const zoneNote=rule.timeZone&&rule.timeZone!==browserZone()?` (${rule.timeZone})`:"";
    switch(rule.kind){
      case "at":return fill(t("onceLabel"),{time:formatTime(rule.at)});
      case "every":{const s=rule.everySeconds;const [n,unit]=s%86400===0?[s/86400,t("days")]:s%3600===0?[s/3600,t("hours")]:[Math.round(s/60),t("minutes")];return fill(t("everyUnit"),{n,unit})}
      case "daily":return fill(t("dailyLabel"),{time:rule.time})+zoneNote;
      case "weekly":{const days=rule.weekdays.map(d=>names[d-1]).join(t("listSeparator"));return rule.weekdays.length===7?fill(t("dailyLabel"),{time:rule.time})+zoneNote:fill(t("weeklyLabel"),{days,time:rule.time})+zoneNote}
      case "cron":return fill(t("cronLabel"),{expr:rule.expression})+zoneNote;
      default:return rule.kind;
    }
  }
  const localInput=date=>{const pad=n=>String(n).padStart(2,"0");return `${date.getFullYear()}-${pad(date.getMonth()+1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`};
  function draftFromRule(rule){
    const zone=rule?.timeZone||browserZone();
    const base={kind:"daily",at:localInput(new Date(Date.now()+3600e3)),interval:"1",unit:"3600",time:"09:00",weekdays:[1,2,3,4,5],expression:"0 9 * * 1-5",timeZone:zone};
    if(!rule)return base;
    switch(rule.kind){
      case "at":return {...base,kind:"at",at:localInput(new Date(rule.at))};
      case "every":{const s=rule.everySeconds;const unit=s%86400===0?"86400":s%3600===0?"3600":"60";return {...base,kind:"every",interval:String(s/Number(unit)),unit}}
      case "daily":return {...base,kind:"daily",time:rule.time.slice(0,5)};
      case "weekly":return {...base,kind:"weekly",time:rule.time.slice(0,5),weekdays:[...rule.weekdays]};
      case "cron":return {...base,kind:"cron",expression:rule.expression};
      default:return base;
    }
  }
  function ruleFromDraft(draft){
    switch(draft.kind){
      case "at":{const date=new Date(draft.at);if(Number.isNaN(date.getTime()))throw Error("invalid time");return {kind:"at",at:date.toISOString()}}
      case "every":return {kind:"every",everySeconds:Math.round(Number(draft.interval)*Number(draft.unit))};
      case "daily":return {kind:"daily",time:draft.time,timeZone:draft.timeZone};
      case "weekly":return {kind:"weekly",time:draft.time,weekdays:[...draft.weekdays].sort((a,b)=>a-b),timeZone:draft.timeZone};
      default:return {kind:"cron",expression:draft.expression.trim(),timeZone:draft.timeZone};
    }
  }
  function Field({label,hint,children}){return h("label",{className:"dshSchedField"},h("span",{className:"dshSchedLabel"},label),children,hint&&h("small",{className:"dshSchedHint"},hint))}
  function ZoneInput({value,onChange,t}){const id=useMemo(()=>"dsh-zone-"+Math.random().toString(36).slice(2),[]);return h(Field,{label:t("timeZone")},h("input",{list:id,value,onChange:e=>onChange(e.target.value),spellCheck:false}),h("datalist",{id},[...new Set([browserZone(),...ZONES])].map(zone=>h("option",{key:zone,value:zone}))))}
  function RuleEditor({draft,onChange,t}){
    const set=patch=>onChange({...draft,...patch});const names=t("weekdayNames").split(",");
    return h("div",{className:"dshSchedRule"},
      h("div",{className:"dshSchedSegment",role:"radiogroup","aria-label":t("frequency")},["at","every","daily","weekly","cron"].map(kind=>h("button",{key:kind,type:"button",role:"radio","aria-checked":draft.kind===kind,"data-active":draft.kind===kind||undefined,onClick:()=>set({kind})},t(kind==="at"?"once":kind)))),
      draft.kind==="at"&&h(Field,{label:t("at")},h("input",{type:"datetime-local",value:draft.at,onChange:e=>set({at:e.target.value})})),
      draft.kind==="every"&&h(Field,{label:t("interval")},h("div",{className:"dshSchedRow"},h("input",{type:"number",min:1,step:1,value:draft.interval,onChange:e=>set({interval:e.target.value}),style:{maxWidth:120}}),h("select",{value:draft.unit,onChange:e=>set({unit:e.target.value})},h("option",{value:"60"},t("minutes")),h("option",{value:"3600"},t("hours")),h("option",{value:"86400"},t("days"))))),
      (draft.kind==="daily"||draft.kind==="weekly")&&h(Field,{label:t("time")},h("input",{type:"time",value:draft.time,onChange:e=>set({time:e.target.value})})),
      draft.kind==="weekly"&&h(Field,{label:t("weekdays")},h("div",{className:"dshSchedDays"},names.map((name,index)=>{const day=index+1,on=draft.weekdays.includes(day);return h("button",{key:day,type:"button","aria-pressed":on,"data-active":on||undefined,onClick:()=>set({weekdays:on?draft.weekdays.filter(d=>d!==day):[...draft.weekdays,day]})},name)}))),
      draft.kind==="cron"&&h(Field,{label:"Cron",hint:t("cronHint")},h("input",{value:draft.expression,onChange:e=>set({expression:e.target.value}),spellCheck:false,style:{fontFamily:"var(--dsw-font-mono,monospace)"}})),
      ["daily","weekly","cron"].includes(draft.kind)&&h(ZoneInput,{value:draft.timeZone,onChange:timeZone=>set({timeZone}),t}));
  }
  function useSessions(sessions){
    const empty=useMemo(()=>({ids:[],byId:{},current:void 0}),[]);
    return useSyncExternalStore(cb=>sessions?.list?.subscribe?.(cb)??(()=>{}),()=>sessions?.list?.getSnapshot?.()??empty);
  }
  function sessionTitle(list,id){const row=list.byId?.[id];return row?.displayTitle||row?.title||id}
  function CreateDialog({t,sessions,onClose,onCreated}){
    const list=useSessions(sessions);
    const choices=useMemo(()=>list.ids.map(id=>list.byId[id]).filter(row=>row&&row.origin!=="subagent"&&(!row.blank||row.id===list.current)),[list]);
    const [workspaces,setWorkspaces]=useState([]);
    useEffect(()=>{rpc("workspace.list").then(value=>setWorkspaces(value.items||[])).catch(()=>{})},[]);
    const [sessionId,setSessionId]=useState(()=>list.current&&list.byId[list.current]?.origin!=="subagent"?list.current:choices[0]?.id||"");
    useEffect(()=>{if(!sessionId&&workspaces[0])setSessionId("new:"+plainPath(workspaces[0].path))},[workspaces,sessionId]);
    const [title,setTitle]=useState(""),[prompt,setPrompt]=useState(""),[draft,setDraft]=useState(()=>draftFromRule(null));
    const [busy,setBusy]=useState(false),[error,setError]=useState(null);
    const dialog=useRef(null);
    useEffect(()=>{dialog.current?.querySelector("textarea")?.focus();const onKey=e=>{if(e.key==="Escape"&&!busy)onClose()};window.addEventListener("keydown",onKey);return()=>window.removeEventListener("keydown",onKey)},[busy,onClose]);
    const submit=async e=>{e.preventDefault();setBusy(true);setError(null);try{const rule=ruleFromDraft(draft);let target=sessionId;if(target.startsWith("new:")){target=await sessions.create({cwd:target.slice(4)});const name=(title.trim()||prompt.trim().split(/\r?\n/)[0]).slice(0,40);try{await sessions.binding?.(target)?.session?.rename?.(`${t("title")} · ${name}`)}catch{}}const value=await call("create",{sessionId:target,title,prompt,rule});onCreated(value.task)}catch(error){setError(error.message)}finally{setBusy(false)}};
    return h("div",{className:"dshSchedOverlay",onMouseDown:e=>{if(e.target===e.currentTarget&&!busy)onClose()}},
      h("form",{ref:dialog,className:"dshSchedDialog dshSched",role:"dialog","aria-modal":true,"aria-label":t("create"),onSubmit:submit},
        h("header",null,h("h2",null,t("create")),h("button",{type:"button",className:"dshSchedIcon","aria-label":t("close"),onClick:onClose,disabled:busy},"×")),
        h(Field,{label:t("prompt")},h("textarea",{required:true,rows:4,maxLength:8000,value:prompt,placeholder:t("promptPlaceholder"),onChange:e=>setPrompt(e.target.value)})),
        h(Field,{label:t("name")},h("input",{maxLength:120,value:title,placeholder:t("namePlaceholder"),onChange:e=>setTitle(e.target.value)})),
        h(Field,{label:t("frequency")},h(RuleEditor,{draft,onChange:setDraft,t})),
        h(Field,{label:t("session"),hint:choices.length||workspaces.length?t("sessionHint"):t("noSession")},h("select",{required:true,value:sessionId,onChange:e=>setSessionId(e.target.value)},
          choices.length>0&&h("optgroup",{label:t("existing")},choices.map(row=>h("option",{key:row.id,value:row.id},row.displayTitle||row.title||row.id))),
          workspaces.length>0&&h("optgroup",{label:t("newSession")},workspaces.map(space=>h("option",{key:space.workspaceId,value:"new:"+plainPath(space.path)},`＋ ${t("newSession")} · ${space.title||plainPath(space.path)}`))))),
        error&&h("p",{className:"dshSchedError",role:"alert"},error),
        h("footer",null,h("button",{type:"button",onClick:onClose,disabled:busy},t("cancel")),h("button",{type:"submit",className:"dshSchedPrimary",disabled:busy||!sessionId||!prompt.trim()},busy?t("saving"):t("create")))));
  }
  function Records({task,t}){
    const [page,setPage]=useState(null),[error,setError]=useState(null),[copied,setCopied]=useState(null);
    const load=useCallback(async(offset=0)=>{try{const value=await call("history",{id:task.id,sessionId:task.sessionId,offset,limit:20});setPage(previous=>offset===0?value:{...value,records:[...(previous?.records||[]),...value.records]});setError(null)}catch(error){setError(error.message)}},[task.id,task.sessionId]);
    useEffect(()=>{setPage(null);void load(0)},[load,task.historyCount,task.lastDelivery?.deliveredAt]);
    if(error)return h("p",{className:"dshSchedError"},error);
    if(!page)return h("p",{className:"dshSchedMuted"},t("loading"));
    if(!page.records.length)return h("p",{className:"dshSchedMuted"},t("records0"));
    return h("div",{className:"dshSchedRecords"},h("p",{className:"dshSchedHint"},t("sentHint")),
      h("ol",null,page.records.map(record=>h("li",{key:record.deliveredAt+record.occurrenceAt,"data-outcome":record.outcome},
        h("div",{className:"dshSchedRow"},h("span",{className:"dshSchedDot","aria-hidden":true}),h("strong",null,record.outcome==="delivered"?t("delivered"):t("failed")),record.manual&&h("span",{className:"dshSchedTag"},t("manual")),h("span",{className:"dshSchedMuted"},formatTime(record.deliveredAt)),record.messageId&&h("button",{type:"button",className:"dshSchedLink",title:record.messageId,onClick:async()=>{try{await navigator.clipboard.writeText(record.messageId);setCopied(record.messageId);setTimeout(()=>setCopied(null),1500)}catch{}}},copied===record.messageId?t("copied"):t("copyId"))),
        record.error&&h("p",{className:"dshSchedError"},record.error),
        h("p",{className:"dshSchedClamp"},record.prompt)))),
      page.hasMore&&h("button",{type:"button",onClick:()=>load(page.records.length)},t("loadMore")),
      page.earlierRecordsPruned&&!page.hasMore&&h("p",{className:"dshSchedHint"},fill(t("pruned"),{days:page.retentionDays,records:page.retentionRecords})));
  }
  function Detail({task,t,sessions,layout,onChanged,onDeleted,onClose}){
    const list=useSessions(sessions);
    const [tab,setTab]=useState("rules");
    const initial=useMemo(()=>({title:task.title,prompt:task.prompt,draft:draftFromRule(task.rule)}),[task.id,task.updatedAt]);
    const [form,setForm]=useState(initial);
    const [busy,setBusy]=useState(null),[error,setError]=useState(null),[notice,setNotice]=useState(null);
    const baseline=useRef(initial);
    const dirty=JSON.stringify(form)!==JSON.stringify(baseline.current);
    useEffect(()=>{if(!dirty){setForm(initial)}baseline.current=initial},[initial]);// keep an unsaved draft across refreshes
    useEffect(()=>{setError(null);setNotice(null)},[task.id]);
    const run=async(name,action)=>{setBusy(name);setError(null);setNotice(null);try{await action()}catch(error){if(error.code==="conflict"){setNotice(t("conflict"));setForm(initial);onChanged()}else setError(error.message)}finally{setBusy(null)}};
    const save=()=>run("save",async()=>{const rule=ruleFromDraft(form.draft);const changedRule=JSON.stringify(rule)!==JSON.stringify(ruleFromDraft(baseline.current.draft));const value=await call("update",{id:task.id,sessionId:task.sessionId,expectedUpdatedAt:task.updatedAt,title:form.title,prompt:form.prompt,...changedRule?{rule}:{}});baseline.current={title:value.task.title,prompt:value.task.prompt,draft:draftFromRule(value.task.rule)};setForm(baseline.current);onChanged(value.task)});
    const toggle=()=>run("toggle",async()=>{const value=await call("setActive",{id:task.id,sessionId:task.sessionId,active:task.status!=="active"});onChanged(value.task)});
    const runNow=()=>run("run",async()=>{await call("runNow",{id:task.id,sessionId:task.sessionId});onChanged();setTab("records")});
    const remove=()=>{if(!window.confirm(t("confirmDelete")))return;void run("delete",async()=>{await call("delete",{id:task.id,sessionId:task.sessionId});onDeleted(task.id)})};
    const open=()=>{sessions?.open?.(task.sessionId);layout?.selectPanel?.(null)};
    const known=Boolean(list.byId?.[task.sessionId]);
    return h("section",{className:"dshSchedDetail","aria-label":task.title},
      h("header",null,
        h("div",{className:"dshSchedDetailTitle"},h("button",{type:"button",className:"dshSchedIcon dshSchedBack","aria-label":t("close"),onClick:onClose},"‹"),h("h2",null,task.title)),
        h("label",{className:"dshSchedSwitch"},h("input",{type:"checkbox",role:"switch",checked:task.status==="active",disabled:Boolean(busy),onChange:toggle}),h("span",null,t("enable")))),
      h("div",{className:"dshSchedMeta"},
        h("span",null,ruleLabel(task.rule,t)),
        h("span",null,t("next"),t("colon"),task.status==="active"&&task.nextRunAt?`${formatTime(task.nextRunAt)}（${relative(task.nextRunAt,t)}）`:t("none")),
        h("span",null,t("last"),t("colon"),task.lastDelivery?`${formatTime(task.lastDelivery.deliveredAt)} · ${task.lastDelivery.outcome==="delivered"?t("delivered"):t("failed")}`:t("never")),
        h("span",null,task.origin==="agent"?t("createdByAgent"):t("createdByUser"))),
      h("div",{className:"dshSchedRow dshSchedSessionRow"},h("span",{className:"dshSchedMuted"},t("session"),"："),h("span",{className:"dshSchedSessionName"},sessionTitle(list,task.sessionId)),known&&h("button",{type:"button",className:"dshSchedLink",onClick:open},t("openSession"))),
      h("div",{className:"dshSchedTabs",role:"tablist"},["rules","records"].map(id=>h("button",{key:id,type:"button",role:"tab","aria-selected":tab===id,"data-active":tab===id||undefined,onClick:()=>setTab(id)},t(id),id==="records"&&task.historyCount?` ${task.historyCount}`:""))),
      tab==="rules"?h("div",{className:"dshSchedForm"},
        h(Field,{label:t("name")},h("input",{maxLength:120,value:form.title,onChange:e=>setForm({...form,title:e.target.value})})),
        h(Field,{label:t("prompt")},h("textarea",{rows:5,maxLength:8000,value:form.prompt,onChange:e=>setForm({...form,prompt:e.target.value})})),
        h(Field,{label:t("frequency")},h(RuleEditor,{draft:form.draft,onChange:draft=>setForm({...form,draft}),t})),
        notice&&h("p",{className:"dshSchedHint",role:"status"},notice),
        error&&h("p",{className:"dshSchedError",role:"alert"},error),
        h("footer",null,
          h("button",{type:"button",className:"dshSchedDanger",onClick:remove,disabled:Boolean(busy)},t("delete")),
          h("span",{style:{flex:1}}),
          dirty&&h("span",{className:"dshSchedMuted"},t("unsaved")),
          h("button",{type:"button",onClick:runNow,disabled:Boolean(busy)},busy==="run"?"…":t("runNow")),
          h("button",{type:"button",onClick:()=>setForm(baseline.current),disabled:!dirty||Boolean(busy)},t("cancel")),
          h("button",{type:"button",className:"dshSchedPrimary",onClick:save,disabled:!dirty||Boolean(busy)||!form.prompt.trim()},busy==="save"?t("saving"):t("save")))):h(Records,{task,t}));
  }
  /** Task a conversation header asked the page to open, consumed on mount. */
  let requestedTask=null;
  /** Clock in a conversation header: the conversation's active tasks, opening the page. */
  function ScheduleHeaderAction({sessionId,t,layout}){
    const [tasks,setTasks]=useState([]),[open,setOpen]=useState(false);
    useEffect(()=>{if(!sessionId)return;const abort=new AbortController();let revision=null;(async()=>{while(!abort.signal.aborted){try{if(revision!==null){const next=await call("wait",{revision},abort.signal);if(next.revision===revision)continue}const value=await call("catalog",{sessionId},abort.signal);revision=value.revision;setTasks((value.tasks||[]).filter(task=>task.status==="active"))}catch{if(abort.signal.aborted)return;await new Promise(resolve=>setTimeout(resolve,5000))}}})();return()=>abort.abort()},[sessionId]);
    if(!tasks.length)return null;
    const show=task=>{requestedTask=task?.id??null;setOpen(false);layout?.selectPanel?.("schedule")};
    const label=`${t("title")} ${tasks.length}`;
    return h("span",{className:"dshSchedHeader dshSched",style:{position:"relative",display:"inline-flex"}},
      h("button",{type:"button",className:"dshSchedHeaderButton","aria-label":label,title:tasks.map(task=>`${task.title} · ${relative(task.nextRunAt,t)}`).join("\n"),"aria-expanded":tasks.length>1?open:undefined,onClick:()=>tasks.length===1?show(tasks[0]):setOpen(value=>!value)},h(ClockIcon,{size:15}),h("span",null,tasks.length)),
      open&&h("ul",{className:"dshSchedHeaderMenu",role:"menu"},tasks.map(task=>h("li",{key:task.id},h("button",{type:"button",role:"menuitem",onClick:()=>show(task)},h("strong",null,task.title),h("small",null,`${ruleLabel(task.rule,t)} · ${relative(task.nextRunAt,t)}`))))));
  }
  function TaskManager({t,sessions,layout}){
    const list=useSessions(sessions);
    const [catalog,setCatalog]=useState(null),[error,setError]=useState(null),[filter,setFilter]=useState("all"),[query,setQuery]=useState(""),[selected,setSelected]=useState(()=>{const id=requestedTask;requestedTask=null;return id}),[creating,setCreating]=useState(false);
    const load=useCallback(async signal=>{try{const value=await call("catalog",{},signal);setCatalog(value);setError(value.error?`${t("storageError")}：${value.error}`:null);return value}catch(error){if(error.name!=="AbortError")setError(error.message)}},[t]);
    useEffect(()=>{const abort=new AbortController();let revision=null;(async()=>{const first=await load(abort.signal);revision=first?.revision??0;while(!abort.signal.aborted){try{const next=await call("wait",{revision},abort.signal);if(next.revision!==revision){revision=next.revision;await load(abort.signal)}}catch{if(abort.signal.aborted)return;await new Promise(resolve=>setTimeout(resolve,3000))}}})();return()=>abort.abort()},[load]);
    const tasks=catalog?.tasks||[];
    const visible=tasks.filter(task=>(filter==="all"||(filter==="active")===(task.status==="active"))&&(!query||`${task.title} ${task.prompt}`.toLowerCase().includes(query.toLowerCase()))).sort((a,b)=>(a.status===b.status?0:a.status==="active"?-1:1)||String(a.nextRunAt||"~").localeCompare(String(b.nextRunAt||"~"))||b.createdAt.localeCompare(a.createdAt));
    const current=tasks.find(task=>task.id===selected)||null;
    const counts={all:tasks.length,active:tasks.filter(task=>task.status==="active").length,inactive:tasks.filter(task=>task.status!=="active").length};
    return h("div",{className:"dshSched dshSchedPage","data-detail":current?"":undefined},
      h("header",{className:"dshSchedHeader"},h("div",null,h("h1",null,t("title")),h("p",{className:"dshSchedMuted"},t("subtitle"))),h("div",{className:"dshSchedRow"},h("button",{type:"button",onClick:()=>load()},t("refresh")),h("button",{type:"button",className:"dshSchedPrimary",onClick:()=>setCreating(true),disabled:Boolean(catalog?.error)},"+ ",t("create")))),
      error&&h("p",{className:"dshSchedError",role:"alert"},error),
      h("div",{className:"dshSchedToolbar"},h("div",{className:"dshSchedSegment",role:"tablist"},[["all","all"],["active","enabled"],["inactive","inactive"]].map(([id,label])=>h("button",{key:id,type:"button",role:"tab","aria-selected":filter===id,"data-active":filter===id||undefined,onClick:()=>setFilter(id)},t(label)," ",h("span",{className:"dshSchedCount"},counts[id])))),h("input",{type:"search",placeholder:t("search"),value:query,onChange:e=>setQuery(e.target.value)})),
      h("div",{className:"dshSchedBody"},
        h("ul",{className:"dshSchedList","aria-label":t("title")},!catalog?h("li",{className:"dshSchedMuted"},t("loading")):visible.length===0?h("li",{className:"dshSchedEmpty"},h("strong",null,tasks.length?t("emptyFiltered"):t("empty")),!tasks.length&&h("p",null,t("emptyHint"))):visible.map(task=>h("li",{key:task.id},h("button",{type:"button",className:"dshSchedCard","aria-current":task.id===selected||undefined,"data-status":task.status,onClick:()=>setSelected(task.id)},
          h("div",{className:"dshSchedRow"},h("span",{className:"dshSchedDot","aria-hidden":true}),h("strong",{className:"dshSchedCardTitle"},task.title),task.lastDelivery?.outcome==="failed"&&h("span",{className:"dshSchedTag dshSchedTagError"},t("failed"))),
          h("div",{className:"dshSchedCardMeta"},h("span",null,ruleLabel(task.rule,t)),h("span",null,task.status==="active"?(task.nextRunAt?relative(task.nextRunAt,t):t("overdue")):t("statusInactive"))),
          h("div",{className:"dshSchedCardSession"},sessionTitle(list,task.sessionId)))))),
        current&&h(Detail,{key:current.id,task:current,t,sessions,layout,onChanged:()=>load(),onDeleted:()=>{setSelected(null);void load()},onClose:()=>setSelected(null)})),
      creating&&h(CreateDialog,{t,sessions,onClose:()=>setCreating(false),onCreated:task=>{setCreating(false);setSelected(task.id);void load()}}));
  }
  const ClockIcon=({size=16})=>h("svg",{width:size,height:size,viewBox:"0 0 24 24",fill:"none",stroke:"currentColor",strokeWidth:1.8,strokeLinecap:"round",strokeLinejoin:"round","aria-hidden":true},h("circle",{cx:12,cy:12,r:9}),h("path",{d:"M12 7v5l3 2"}));
  const css=`.dshSched{--sched-accent:var(--dsw-alias-brand-primary,#4d6bfe);color:var(--dsw-alias-label-primary);font-size:14px;line-height:1.5}.dshSched *{box-sizing:border-box}.dshSchedPage{height:100%;overflow:auto;padding:24px 28px;max-width:1180px;margin:0 auto;display:flex;flex-direction:column;gap:14px}.dshSched h1{font-size:22px;margin:0 0 4px}.dshSched h2{font-size:17px;margin:0;overflow-wrap:anywhere}.dshSchedHeader{display:flex;gap:16px;justify-content:space-between;align-items:flex-start}.dshSchedRow{display:flex;gap:8px;align-items:center;flex-wrap:wrap;min-width:0}.dshSched button,.dshSched input,.dshSched select,.dshSched textarea{font:inherit;color:inherit}.dshSched button{border:1px solid var(--dsw-alias-border-l2);background:transparent;padding:6px 14px;border-radius:18px;cursor:pointer;min-height:32px}.dshSched button:hover:not(:disabled){background:var(--dsw-alias-interactive-bg-hover)}.dshSched button:disabled{opacity:.45;cursor:default}.dshSched :focus-visible{outline:2px solid var(--sched-accent);outline-offset:2px}.dshSched .dshSchedPrimary{background:var(--dsw-alias-label-primary,#111);color:var(--dsw-alias-bg-base,#fff);border-color:transparent}.dshSched .dshSchedPrimary:hover:not(:disabled){background:var(--dsw-alias-label-primary,#111);opacity:.88}.dshSched .dshSchedDanger{color:var(--dsw-alias-state-error-primary,#d93025)}.dshSched input:not([type=checkbox]),.dshSched select,.dshSched textarea{width:100%;min-width:0;padding:7px 10px;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;background:var(--dsw-alias-bg-base)}.dshSched textarea{resize:vertical}.dshSchedMuted{color:var(--dsw-alias-label-tertiary);margin:0}.dshSchedHint{color:var(--dsw-alias-label-tertiary);font-size:12px;margin:0}.dshSchedError{color:var(--dsw-alias-state-error-primary,#d93025);overflow-wrap:anywhere;margin:0}.dshSchedToolbar{display:flex;gap:12px;align-items:center;flex-wrap:wrap}.dshSchedToolbar input{flex:1;min-width:160px;max-width:320px}.dshSchedSegment{display:inline-flex;gap:2px;padding:3px;border-radius:20px;background:var(--dsw-alias-bg-layer-2,#f1f1f4);flex-wrap:wrap}.dshSched .dshSchedSegment button{border:0;min-height:28px;padding:3px 12px}.dshSched .dshSchedSegment button[data-active]{background:var(--dsw-alias-bg-base,#fff);box-shadow:0 1px 3px #0000001a}.dshSchedCount{color:var(--dsw-alias-label-tertiary);font-size:12px}.dshSchedBody{display:grid;grid-template-columns:minmax(260px,380px) minmax(0,1fr);gap:18px;align-items:start;min-height:0}.dshSchedPage:not([data-detail]) .dshSchedBody{grid-template-columns:minmax(0,1fr)}.dshSchedList{list-style:none;margin:0;padding:0;display:grid;gap:8px}.dshSchedPage:not([data-detail]) .dshSchedList{grid-template-columns:repeat(auto-fill,minmax(280px,1fr))}.dshSched .dshSchedCard{display:flex;flex-direction:column;gap:4px;width:100%;text-align:left;border-radius:12px;padding:12px 14px;background:var(--dsw-alias-bg-layer-1,#fff)}.dshSched .dshSchedCard[aria-current]{border-color:var(--sched-accent);box-shadow:0 0 0 1px var(--sched-accent)}.dshSchedCardTitle{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.dshSchedCardMeta{display:flex;justify-content:space-between;gap:8px;color:var(--dsw-alias-label-secondary);font-size:12px}.dshSchedCardSession{color:var(--dsw-alias-label-tertiary);font-size:12px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.dshSchedDot{width:8px;height:8px;border-radius:50%;background:var(--dsw-alias-state-success-primary,#1e9e5a);flex:none}[data-status=inactive] .dshSchedDot{background:var(--dsw-alias-label-quaternary,#bbb)}[data-outcome=failed] .dshSchedDot{background:var(--dsw-alias-state-error-primary,#d93025)}.dshSchedTag{font-size:11px;padding:1px 7px;border-radius:6px;background:var(--dsw-alias-bg-layer-2,#f1f1f4);color:var(--dsw-alias-label-secondary)}.dshSchedTagError{color:var(--dsw-alias-state-error-primary,#d93025)}.dshSchedEmpty{padding:36px 16px;text-align:center;border:1px dashed var(--dsw-alias-border-l2);border-radius:12px;color:var(--dsw-alias-label-secondary)}.dshSchedDetail{border:1px solid var(--dsw-alias-border-l2);border-radius:14px;padding:16px 18px;background:var(--dsw-alias-bg-layer-1,#fff);display:flex;flex-direction:column;gap:12px;min-width:0}.dshSchedDetail>header{display:flex;justify-content:space-between;gap:12px;align-items:center}.dshSchedDetailTitle{display:flex;gap:6px;align-items:center;min-width:0}.dshSched .dshSchedIcon{border:0;min-height:28px;width:28px;padding:0;font-size:18px;line-height:1}.dshSchedBack{display:none}.dshSchedMeta{display:flex;flex-wrap:wrap;gap:6px 16px;color:var(--dsw-alias-label-secondary);font-size:12px}.dshSchedSessionName{overflow-wrap:anywhere}.dshSched .dshSchedLink{border:0;padding:0 4px;min-height:0;color:var(--sched-accent);background:none}.dshSchedTabs{display:flex;gap:18px;border-bottom:1px solid var(--dsw-alias-border-l2)}.dshSched .dshSchedTabs button{border:0;border-radius:0;padding:6px 0;min-height:0;border-bottom:2px solid transparent}.dshSched .dshSchedTabs button[data-active]{border-bottom-color:var(--dsw-alias-label-primary);font-weight:600}.dshSchedForm,.dshSchedDialog{display:flex;flex-direction:column;gap:10px}.dshSchedForm footer,.dshSchedDialog footer{display:flex;gap:8px;align-items:center;flex-wrap:wrap;justify-content:flex-end}.dshSchedField{display:flex;flex-direction:column;gap:5px;min-width:0}.dshSchedLabel{font-size:12px;color:var(--dsw-alias-label-secondary)}.dshSchedRule{display:flex;flex-direction:column;gap:10px}.dshSchedDays{display:flex;gap:6px;flex-wrap:wrap}.dshSched .dshSchedDays button{min-width:40px;padding:4px 10px}.dshSched .dshSchedDays button[data-active]{background:var(--dsw-alias-label-primary,#111);color:var(--dsw-alias-bg-base,#fff);border-color:transparent}.dshSchedSwitch{display:inline-flex;gap:6px;align-items:center;font-size:13px;white-space:nowrap}.dshSchedSwitch input{width:34px;height:20px;appearance:none;border-radius:10px;background:var(--dsw-alias-label-quaternary,#c9c9cf);position:relative;cursor:pointer;margin:0;transition:background .15s}.dshSchedSwitch input::after{content:"";position:absolute;top:2px;left:2px;width:16px;height:16px;border-radius:50%;background:#fff;transition:transform .15s}.dshSchedSwitch input:checked{background:var(--sched-accent)}.dshSchedSwitch input:checked::after{transform:translateX(14px)}.dshSchedRecords ol{list-style:none;margin:0;padding:0;display:grid;gap:10px}.dshSchedRecords li{padding:10px 12px;border-radius:10px;background:var(--dsw-alias-bg-layer-2,#f6f6f8);display:flex;flex-direction:column;gap:4px}.dshSchedClamp{margin:0;display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical;overflow:hidden;color:var(--dsw-alias-label-secondary);font-size:13px;white-space:pre-wrap;overflow-wrap:anywhere}.dshSchedOverlay{position:fixed;inset:0;z-index:1000;background:#0006;display:flex;align-items:center;justify-content:center;padding:16px}.dshSchedDialog{width:min(560px,100%);max-height:calc(100vh - 32px);overflow:auto;background:var(--dsw-alias-bg-base,#fff);border-radius:16px;padding:20px 22px;box-shadow:0 20px 60px #0000003a}.dshSchedDialog>header{display:flex;justify-content:space-between;align-items:center}.dshSched.dshSchedHeader{flex-direction:row}.dshSched .dshSchedHeaderButton{display:inline-flex;gap:4px;align-items:center;border:0;min-height:28px;padding:2px 8px;color:var(--dsw-alias-label-secondary);font-size:12px}.dshSchedHeaderMenu{position:absolute;top:100%;right:0;z-index:50;list-style:none;margin:4px 0 0;padding:4px;min-width:260px;max-width:min(340px,90vw);background:var(--dsw-alias-bg-base,#fff);border:1px solid var(--dsw-alias-border-l2);border-radius:12px;box-shadow:0 8px 24px #0000001f}.dshSched .dshSchedHeaderMenu button{display:flex;flex-direction:column;align-items:flex-start;gap:2px;width:100%;border:0;border-radius:8px;text-align:left;padding:8px 10px}.dshSchedHeaderMenu small{color:var(--dsw-alias-label-tertiary)}@media(max-width:820px){.dshSchedPage{padding:16px 14px}.dshSchedHeader{flex-direction:column}.dshSchedBody{grid-template-columns:minmax(0,1fr)}.dshSchedPage[data-detail] .dshSchedList{display:none}.dshSchedBack{display:inline-block}}`;
  function apply(ctx){
    ctx.effect(()=>ctx.locale.register("schedule",{zh,en}),"schedule locale");
    const t=ctx.locale.bind("schedule");
    const style=document.createElement("style");style.dataset.dshSchedule="";style.textContent=css;document.head.appendChild(style);
    ctx.effect(()=>()=>style.remove(),"schedule style");
    ctx.slots.inject("main",()=>ctx.slots.register({name:"main",key:"schedule"},()=>h(TaskManager,{t,sessions:ctx.sessions,layout:ctx.layout})));
    ctx.slots.inject("conversation.session.header.actions",()=>ctx.slots.register({name:"conversation.session.header.actions",id:"schedule-tasks",order:10,inject:sessionId=>({sessionId})},props=>h(ScheduleHeaderAction,{...props,t,layout:ctx.layout})));
    ctx.slots.inject("sidebar.panellist",()=>ctx.slots.register({name:"sidebar.panellist",id:"schedule",order:20,label:()=>t("title")},props=>h(ClockIcon,{size:props?.size})));
  }
  return {apply,inject:["slots","locale","sessions","layout"],test:{TaskManager,ScheduleHeaderAction,Detail,CreateDialog,RuleEditor,ruleLabel,ruleFromDraft,draftFromRule,zh,en,css}};
}});
