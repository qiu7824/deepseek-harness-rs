window.__ModuleLoader__.load({
  id: "dsh-schedule",
  factory: require => {
    const React = require("react"), h = React.createElement;
    const zh = {
      manager:"提醒", loading:"正在读取提醒…", refresh:"刷新", search:"搜索标题、内容或原会话", all:"全部", active:"待发送", inactive:"已结束", status:"状态", empty:"没有符合条件的提醒。",
      add:"新建提醒", edit:"编辑提醒", title:"标题", prompt:"发送给会话的内容", session:"原会话", chooseSession:"选择接收提醒的会话", openSession:"打开原会话", missingSession:"原会话不可用", archivedSession:"原会话已归档", next:"计划时间", last:"最近写入", save:"保存", cancel:"取消", discard:"放弃草稿", reload:"读取最新记录并保留草稿",
      disabled:"提醒已停用；已保存的提醒和发送记录仍可查看。", enabled:"提醒已启用", enable:"启用提醒", disable:"停用提醒", enableHint:"启用后才能新建或修改提醒。", unavailable:"提醒插件暂不可用。", originalHint:"提醒始终发送到创建时绑定的原会话。", saved:"提醒已保存。", deleted:"提醒已删除。", delete:"删除提醒", confirmDelete:"确认删除", deleteHint:"删除后不再发送此提醒，已有会话消息会保留。", conflict:"提醒已在其他位置修改；草稿已保留。读取最新记录后再保存。", ended:"此提醒已结束，不能修改。", missing:"此提醒已删除或不可用。", retry:"重试待发送提醒", retried:"已请求重新处理到期提醒。", retryHint:"重试仅重新处理已保存的到期提醒。",
      rule:"时间规则", after:"延时一次", at:"指定日期一次", every:"固定间隔", daily:"每天", weekly:"每周", cron:"Cron", seconds:"秒数", date:"日期", time:"时间", zone:"时区（IANA）", zoneHint:"例如 Asia/Shanghai、Europe/London 或 UTC；日期和时间按此时区解释。", weekdays:"星期", cronExpression:"Cron 表达式", cronHint:"五个字段：分 时 日 月 星期。", mon:"周一", tue:"周二", wed:"周三", thu:"周四", fri:"周五", sat:"周六", sun:"周日",
      history:"发送记录", historyHint:"记录表示提醒已写入原会话，不代表模型执行成功。", scheduled:"计划发送", delivered:"写入会话", message:"消息编号", more:"更早记录", noHistory:"暂无发送记录。", historyPruned:"更早的记录已按保留策略清理。", historyUnavailable:"部分更早记录无法读取。", retention:"保留范围", days:"天", records:"条", recordUnavailable:"此记录没有保存发送内容。", pending:"正在保存…",
      required:"请填写标题和发送内容。", invalidSeconds:"秒数须为正安全整数，固定间隔至少为 60 秒。", invalidZone:"请输入有效的 IANA 时区。", invalidTime:"请填写有效日期和时间。", invalidDays:"请至少选择一个星期。", invalidCron:"请填写五字段 Cron 表达式。", failed:"提醒操作失败。", stale:"连接已变化，请重新读取后操作。", invalidResponse:"无法读取提醒响应。", draftConfirm:"放弃尚未保存的草稿？", keepDraft:"保留草稿", leaveDraft:"放弃并继续", timingHint:"修改已创建的一次性提醒时，选择新的指定日期。"
    };
    const en = {
      manager:"Reminders", loading:"Loading reminders…", refresh:"Refresh", search:"Search title, content or original conversation", all:"All", active:"Pending", inactive:"Ended", status:"Status", empty:"No matching reminders.",
      add:"New reminder", edit:"Edit reminder", title:"Title", prompt:"Message for the conversation", session:"Original conversation", chooseSession:"Choose a receiving conversation", openSession:"Open original conversation", missingSession:"Original conversation unavailable", archivedSession:"Original conversation archived", next:"Scheduled time", last:"Last written", save:"Save", cancel:"Cancel", discard:"Discard draft", reload:"Read latest record and keep draft",
      disabled:"Reminders are disabled. Saved reminders and delivery records remain available.", enabled:"Reminders enabled", enable:"Enable reminders", disable:"Disable reminders", enableHint:"Enable reminders to create or edit them.", unavailable:"Reminders are temporarily unavailable.", originalHint:"A reminder always sends to the original conversation it was created for.", saved:"Reminder saved.", deleted:"Reminder deleted.", delete:"Delete reminder", confirmDelete:"Confirm deletion", deleteHint:"Future deliveries stop. Messages already in the conversation remain.", conflict:"This reminder changed elsewhere. Your draft is preserved. Read the latest record before saving again.", ended:"This reminder has ended and cannot be edited.", missing:"This reminder was deleted or is unavailable.", retry:"Retry pending reminders", retried:"Processing of saved due reminders requested.", retryHint:"Retry only processes already saved, due reminders.",
      rule:"Timing rule", after:"Once after a delay", at:"Once on a date", every:"Fixed interval", daily:"Daily", weekly:"Weekly", cron:"Cron", seconds:"Seconds", date:"Date", time:"Time", zone:"Time zone (IANA)", zoneHint:"For example Asia/Shanghai, Europe/London or UTC. Dates and times use this zone.", weekdays:"Weekdays", cronExpression:"Cron expression", cronHint:"Five fields: minute hour day month weekday.", mon:"Monday", tue:"Tuesday", wed:"Wednesday", thu:"Thursday", fri:"Friday", sat:"Saturday", sun:"Sunday",
      history:"Delivery records", historyHint:"A record means the reminder was written to its original conversation, not that the model executed it successfully.", scheduled:"Scheduled", delivered:"Written to conversation", message:"Message ID", more:"Earlier records", noHistory:"No delivery records yet.", historyPruned:"Earlier records were removed under the retention policy.", historyUnavailable:"Some earlier records are unavailable.", retention:"Retention", days:"days", records:"records", recordUnavailable:"Message content was not retained for this record.", pending:"Saving…",
      required:"Enter a title and message.", invalidSeconds:"Use a positive safe integer. Fixed intervals require at least 60 seconds.", invalidZone:"Enter a valid IANA time zone.", invalidTime:"Enter a valid date and time.", invalidDays:"Select at least one weekday.", invalidCron:"Enter a five-field Cron expression.", failed:"Reminder operation failed.", stale:"The connection changed. Refresh before continuing.", invalidResponse:"Unable to read the reminder response.", draftConfirm:"Discard the unsaved draft?", keepDraft:"Keep draft", leaveDraft:"Discard and continue", timingHint:"To reschedule an existing one-time reminder, choose a new date."
    };
    Object.assign(zh,{retentionSettings:"发送记录保留设置",retentionDays:"保留天数",retentionRecords:"每条提醒最多保留记录数",retentionHint:"保留时间和条数任一达到限制时清理较早记录。默认保留 30 天、最多 200 条。",saveConfig:"保存保留设置",reloadConfig:"重新读取配置（保留草稿）",configSaved:"保留设置已保存。",invalidRetention:"保留天数须为 1 至 3650 的整数，记录数须为 1 至 10000 的整数。"});
    Object.assign(en,{retentionSettings:"Delivery record retention",retentionDays:"Days to retain",retentionRecords:"Maximum records per reminder",retentionHint:"Earlier records are removed when either limit is reached. Defaults: 30 days, up to 200 records.",saveConfig:"Save retention settings",reloadConfig:"Reload settings and keep draft",configSaved:"Retention settings saved.",invalidRetention:"Days must be an integer from 1 to 3650; records must be an integer from 1 to 10000."});
    const copy = (lang = document.documentElement.lang || navigator.language || "zh") => (lang.startsWith("zh") ? zh : en);
    const KEY_FIELDS = {
      after:["afterSeconds"], at:[], every:["everySeconds"], daily:["time","timeZone"], weekly:["time","timeZone","weekdays"], cron:["expression","timeZone"]
    };
    const recordOf = row => Object.fromEntries(["kind","id","title","prompt","scheduledAt",...(KEY_FIELDS[row.kind] || [])].map(key => [key,row[key]]));
    const keyOf = row => JSON.stringify([row.sessionId,row.id]);
    const clone = value => JSON.parse(JSON.stringify(value));
    const displayTime = value => {const date = new Date(value);return Number.isFinite(date.valueOf()) ? date.toLocaleString() : String(value || "—");};
    const zoneNow = () => Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
    function newDraft(sessionId = "") {return {sessionId,title:"",prompt:"",kind:"after",seconds:"600",date:"",time:"09:00",timeZone:zoneNow(),weekdays:[1],expression:"0 9 * * *"};}
    function draftOf(row) {
      const draft = {...newDraft(row.sessionId),title:row.title,prompt:row.prompt,kind:row.kind};
      if (row.kind === "after" || row.kind === "at") {
        const instant = new Date(row.scheduledAt);
        // UTC makes an existing instant lossless, including milliseconds and DST folds.
        if (Number.isFinite(instant.valueOf())) {draft.date=instant.toISOString().slice(0,10);draft.time=instant.toISOString().slice(11,23);}
        draft.timeZone="UTC";draft.kind="at";
      }
      if (row.kind === "every") draft.seconds=String(row.everySeconds);
      if (row.time) draft.time=row.time;
      if (row.timeZone) draft.timeZone=row.timeZone;
      if (row.weekdays) draft.weekdays=[...row.weekdays];
      if (row.expression) draft.expression=row.expression;
      return draft;
    }
    function timing(draft,t) {
      const kind=draft.kind;
      if (kind === "after" || kind === "every") {
        const seconds=Number(draft.seconds);
        if(!Number.isSafeInteger(seconds)||seconds<(kind === "every"?60:1))throw Error(t("invalidSeconds"));
        return {[kind === "after"?"after_seconds":"every_seconds"]:seconds};
      }
      const time_zone=draft.timeZone.trim();
      try {if(time_zone!=="UTC"&&!/^[A-Za-z][A-Za-z0-9_+.-]*(?:\/[A-Za-z0-9_+.-]+)+$/.test(time_zone))throw Error();new Intl.DateTimeFormat("en",{timeZone:time_zone}).format();}catch {throw Error(t("invalidZone"));}
      if(kind === "cron") {const expression=draft.expression.trim();if(expression.split(/\s+/).length!==5)throw Error(t("invalidCron"));return {cron:{expression,time_zone}};}
      if(!/^([01]\d|2[0-3]):[0-5]\d(?::[0-5]\d(?:\.\d{1,3})?)?$/.test(draft.time))throw Error(t("invalidTime"));
      const time=draft.time.length===5?`${draft.time}:00`:draft.time;
      if(kind === "at") {if(!/^\d{4}-\d{2}-\d{2}$/.test(draft.date))throw Error(t("invalidTime"));return {at:{date:draft.date,time,time_zone}};}
      if(kind === "daily")return {daily:{time,time_zone}};
      if(kind === "weekly") {
        const weekdays=[...new Set(draft.weekdays)].sort((a,b)=>a-b);
        if(!weekdays.length||weekdays.some(day=>!Number.isInteger(day)||day<1||day>7))throw Error(t("invalidDays"));
        return {weekly:{time,time_zone,weekdays}};
      }
      throw Error(t("invalidResponse"));
    }
    function errorText(error,t) {
      const reason=error?.reason||error?.details?.reason||error?.code;
      const key={schedule_conflict:"conflict",schedule_ended:"ended",schedule_not_found:"missing",schedule_disabled:"enableHint",disabled:"enableHint",session_archived:"archivedSession",session_not_found:"missingSession",session_unauthorized:"missingSession",invalid_time_zone:"invalidZone"}[reason];
      return key?t(key):error?.message||t("failed");
    }
    function validRecord(row) {
      return row&&Object.hasOwn(KEY_FIELDS,row.kind)&&["id","title","prompt","scheduledAt"].every(key=>typeof row[key]==="string")&&KEY_FIELDS[row.kind].every(key=>key==="weekdays"?Array.isArray(row[key])&&(row[key].every(day=>Number.isInteger(day)&&day>=1&&day<=7)):key.endsWith("Seconds")?Number.isSafeInteger(row[key]):typeof row[key]==="string");
    }
    /** One mounted manager owns all requests, drafts and cancellation identities. */
    function createScheduleController(options,publish,t=key=>zh[key]||key) {
      const {connection,sessionStore,subscribeChanged,pluginList,setPluginEnabled}=options;
      const generation=connection.generation.getSnapshot();
      let alive=true,epoch=0,sequence=0,currentSession=sessionStore?.getSnapshot().current,requests=new Map();
      let state={records:[],loading:true,error:"",notice:"",enabled:false,entry:null,selected:null,draft:null,expected:null,baseDraft:null,conflict:false,busy:false,history:null,historyBusy:false,historyError:"",confirmDelete:false,leave:null};
      const live=()=>alive&&connection.generation.getSnapshot()===generation;
      const update=patch=>{if(live()){state={...state,...patch};publish(state);}};
      const cancel=lane=>{const pending=requests.get(lane);pending?.abort.abort();requests.delete(lane);};
      const begin=lane=>{cancel(lane);const request={token:++sequence,epoch,abort:new AbortController()};requests.set(lane,request);return request;};
      const owns=(lane,request)=>live()&&requests.get(lane)===request&&!request.abort.signal.aborted&&(lane==="catalog"||request.epoch===epoch);
      const rpc=async(method,payload,request)=>{
        if(!live()||request.abort.signal.aborted)throw Object.assign(Error(t("stale")),{name:"AbortError"});
        const result=await connection.rpc.call("/api",`schedule.${method}`,payload,request.abort.signal);
        if(!result.ok)throw Object.assign(Error(result.error?.message||t("failed")),result.error,{reason:result.error?.details?.reason});
        return result.value;
      };
      const selectedRow=()=>state.records.find(row=>keyOf(row)===state.selected);
      const dirty=()=>state.draft&&JSON.stringify(state.draft)!==JSON.stringify(state.baseDraft);
      const clearSelection=()=>{epoch++;for(const lane of ["write","history"])cancel(lane);update({selected:null,draft:null,expected:null,baseDraft:null,conflict:false,busy:false,history:null,historyError:"",historyBusy:false,confirmDelete:false,leave:null});};
      const load=async()=>{
        if(!live())return;
        const request=begin("catalog");update({loading:true,error:""});
        try {
          const [records,inventory]=await Promise.all([rpc("catalog",{},request),pluginList(request.abort.signal)]);
          if(!owns("catalog",request))return;
          if(!Array.isArray(records)||records.some(row=>!validRecord(row)||typeof row.sessionId!=="string"||!["active","inactive"].includes(row.status)))throw Error(t("invalidResponse"));
          if(!Array.isArray(inventory?.entries))throw Error(t("invalidResponse"));
          const entry=inventory.entries.find(row=>["dsh-schedule","@deepseek-ai/dsh-schedule"].includes(row.moduleName))||null;
          const patch={records,entry,enabled:entry?.enabled===true,loading:false};
          if(state.expected){const row=records.find(row=>keyOf(row)===state.selected);if(!row||row.status!=="active"||JSON.stringify(recordOf(row))!==JSON.stringify(state.expected))patch.conflict=true;}
          update(patch);return true;
        }catch(error){if(owns("catalog",request))update({loading:false,error:errorText(error,t)});return false;}
      };
      const history=async(more=false)=>{
        const row=selectedRow();if(!live()||!row||state.historyBusy||(more&&!state.history?.nextBefore))return;
        const request=begin("history"),before=more?state.history.nextBefore:undefined;update({historyBusy:true,historyError:""});
        try {
          const value=await rpc("history",{sessionId:row.sessionId,id:row.id,limit:20,...(before?{before}:{})},request);
          if(!owns("history",request))return;
          if(value.code)throw {code:value.code};
          if(value.id!==row.id||!Array.isArray(value.records)||value.records.some(item=>!item||["scheduledAt","deliveredAt","messageId"].some(key=>typeof item[key]!=="string")||(item.prompt!==undefined&&typeof item.prompt!=="string"))||typeof value.earlierRecordsUnavailable!=="boolean"||typeof value.earlierRecordsPruned!=="boolean"||(value.nextBefore!==undefined&&typeof value.nextBefore!=="string")||!Number.isInteger(value.retention?.days)||!Number.isInteger(value.retention?.records))throw Error(t("invalidResponse"));
          const records=more?[...state.history.records,...value.records]:value.records;
          const seen=new Set();update({history:{...value,records:records.filter(item=>{if(seen.has(item.messageId))return false;seen.add(item.messageId);return true;})},historyBusy:false});
        }catch(error){if(owns("history",request))update({historyBusy:false,historyError:errorText(error,t)});}
      };
      const selectNow=row=>{clearSelection();if(row){update({selected:keyOf(row)});void history();}};
      const navigate=action=>{if(dirty()){update({leave:action});return;}action();};
      const api={
        getSnapshot:()=>state,load,history,dirty,
        select:row=>navigate(()=>selectNow(row)),
        add:()=>{if(!state.enabled||state.busy)return;navigate(()=>{clearSelection();const draft=newDraft(currentSession||"");update({draft,baseDraft:clone(draft)});});},
        edit:()=>{const row=selectedRow();if(!row||row.status!=="active"||!state.enabled||state.busy)return;const draft=draftOf(row);update({draft,baseDraft:clone(draft),expected:recordOf(row),conflict:false,error:""});},
        change:(field,value)=>{if(!state.busy&&state.draft)update({draft:{...state.draft,[field]:value},error:"",notice:""});},
        discard:()=>{if(!state.busy)update({draft:null,expected:null,baseDraft:null,conflict:false,error:""});},
        keep:()=>update({leave:null}),leave:()=>{const action=state.leave;update({leave:null});action?.();},
        rebase:async()=>{
          const savedEpoch=epoch,expected=state.expected;if(!expected||state.busy)return;if(!await load())return;
          if(!live()||epoch!==savedEpoch||state.expected!==expected)return;
          const row=selectedRow();
          if(!row||row.status!=="active"){update({error:t(row?"ended":"missing")});return;}
          update({expected:recordOf(row),conflict:false,error:"",baseDraft:draftOf(row)});
        },
        save:async()=>{
          if(!live()||state.busy||!state.draft||!state.enabled||state.conflict)return;
          const draft=clone(state.draft),expected=state.expected;
          let payload;
          try {
            if(!draft.title.trim()||!draft.prompt.trim())throw Error(t("required"));
            if(!draft.sessionId)throw Error(t("chooseSession"));
            const rule=timing(draft,t);
            payload=expected?{sessionId:draft.sessionId,id:expected.id,expected:clone(expected)}:{sessionId:draft.sessionId,title:draft.title.trim(),prompt:draft.prompt.trim(),...rule};
            if(expected){
              if(draft.title.trim()!==expected.title)payload.title=draft.title.trim();
              if(draft.prompt.trim()!==expected.prompt)payload.prompt=draft.prompt.trim();
              const baseline=draftOf({...expected,sessionId:draft.sessionId});
              if(JSON.stringify(timing(baseline,t))!==JSON.stringify(rule))payload.change={kind:draft.kind,...rule};
              if(!Object.hasOwn(payload,"title")&&!Object.hasOwn(payload,"prompt")&&!payload.change)return;
            }
          }catch(error){update({error:errorText(error,t)});return;}
          const request=begin("write");update({busy:true,error:"",notice:""});
          try {
            const result=await rpc(expected?"update":"create",payload,request);
            if(!owns("write",request))return;
            if(result.code){update({busy:false,error:errorText(result,t),conflict:true});void load();return;}
            const record=expected?result.record:result;if(!validRecord(record))throw Error(t("invalidResponse"));
            update({busy:false,draft:null,expected:null,baseDraft:null,conflict:false,selected:keyOf({...record,sessionId:draft.sessionId}),notice:t("saved")});
            await load();if(owns("write",request))void history();
          }catch(error){if(owns("write",request))update({busy:false,error:errorText(error,t)});}
        },
        confirmDelete:()=>{if(!state.busy&&selectedRow())update({confirmDelete:true});},
        cancelDelete:()=>update({confirmDelete:false}),
        remove:async()=>{
          const row=selectedRow();if(!live()||state.busy||!row||!state.confirmDelete)return;
          const request=begin("write");update({busy:true,error:""});
          try {const result=await rpc("delete",{sessionId:row.sessionId,id:row.id},request);if(!owns("write",request))return;if(result.id!==row.id||typeof result.deleted!=="boolean")throw Error(t("invalidResponse"));if(result.code&&result.code!=="schedule_not_found")throw {code:result.code};clearSelection();update({notice:t("deleted")});void load();}
          catch(error){if(owns("write",request))update({busy:false,error:errorText(error,t)});}
        },
        toggle:async()=>{
          if(!live()||state.busy||!state.entry)return;
          const request=begin("write");update({busy:true,error:""});
          try {await setPluginEnabled(state.entry,!state.enabled,request.abort.signal);if(owns("write",request)){update({busy:false});void load();}}
          catch(error){if(owns("write",request))update({busy:false,error:errorText(error,t)});}
        },
        retry:async()=>{
          if(!live()||state.busy||!state.enabled)return;const request=begin("write");update({busy:true,error:""});
          try{await rpc("retry",{},request);if(owns("write",request)){update({busy:false,notice:t("retried")});void load();}}
          catch(error){if(owns("write",request))update({busy:false,error:errorText(error,t)});}
        },
        dispose:()=>{alive=false;epoch++;for(const lane of requests.keys())cancel(lane);stopChanges?.();stopSession?.();stopGeneration?.();}
      };
      const stopChanges=subscribeChanged?.(event=>{
        if(!live())return;
        if(typeof event?.enabled==="boolean")update({enabled:event.enabled});
        void load();if(state.selected) {cancel("history");update({historyBusy:false});void history();}
      });
      const stopSession=sessionStore?.subscribe(()=>{
        const next=sessionStore.getSnapshot().current;if(next===currentSession)return;
        currentSession=next;clearSelection();
      });
      const stopGeneration=connection.generation.subscribe(()=>{if(!live()){epoch++;for(const lane of requests.keys())cancel(lane);}});
      return api;
    }
    const css = `.dshSchedules{box-sizing:border-box;min-width:0;width:100%;padding:16px;color:var(--dsw-alias-label-primary);font-size:14px;line-height:1.6}.dshSchedules *{box-sizing:border-box}.dshSchedules h2{font-size:19px;margin:0}.dshSchedules h3{font-size:16px;margin:0}.dshSchedules header,.dshScheduleBar{display:flex;align-items:center;gap:10px;flex-wrap:wrap;margin-bottom:12px}.dshSchedules header h2{flex:1}.dshSchedules button{font:inherit;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;padding:6px 12px;background:var(--dsw-alias-bg-layer-1);color:inherit;cursor:pointer}.dshSchedules button:disabled{opacity:.5;cursor:default}.dshSchedules :focus-visible{outline:2px solid var(--dsw-alias-brand-primary);outline-offset:2px}.dshSchedules input,.dshSchedules select,.dshSchedules textarea{font:inherit;max-width:100%;min-width:0;color:inherit;background:var(--dsw-alias-bg-base);border:1px solid var(--dsw-alias-border-l2);border-radius:7px;padding:6px 8px}.dshSchedules input[type=search]{flex:1}.dshSchedules label{display:grid;gap:5px;margin:9px 0}.dshSchedules textarea{width:100%;resize:vertical;min-height:100px}.dshScheduleColumns{display:grid;grid-template-columns:minmax(170px,1fr) minmax(0,2fr);gap:18px}.dshScheduleList{padding:0;margin:0;list-style:none;max-height:65vh;overflow:auto}.dshScheduleList button{display:grid;width:100%;text-align:left;gap:3px;margin-bottom:8px;overflow-wrap:anywhere}.dshScheduleList button[aria-pressed=true]{border-color:var(--dsw-alias-brand-primary);background:var(--dsw-alias-bg-layer-2)}.dshScheduleHint,.dshSchedules small{color:var(--dsw-alias-label-tertiary);font-size:12px}.dshSchedules [role=alert]{color:var(--dsw-alias-state-error-primary);overflow-wrap:anywhere}.dshScheduleDetails{min-width:0;overflow-wrap:anywhere}.dshScheduleDetails pre{white-space:pre-wrap;overflow-wrap:anywhere;font:inherit}.dshScheduleDetails dl{display:grid;grid-template-columns:auto minmax(0,1fr);gap:5px 12px}.dshScheduleDetails dd{margin:0}.dshScheduleCard{padding:12px;border:1px solid var(--dsw-alias-border-l2);border-radius:10px;margin:10px 0}.dshScheduleWeek{display:flex;flex-wrap:wrap;gap:12px}.dshScheduleWeek label{display:flex;align-items:center;gap:4px}.dshScheduleWeek input{width:16px;height:16px}.dshSchedules fieldset{border:0;padding:0;min-width:0}.dshSchedules [role=alertdialog]{padding:16px;border:2px solid var(--dsw-alias-brand-primary);border-radius:10px;margin-bottom:14px}.dshScheduleHistory{padding-left:20px}.dshScheduleHistory li{margin:12px 0;overflow-wrap:anywhere}@media(max-width:720px){.dshScheduleColumns{grid-template-columns:minmax(0,1fr)}.dshScheduleList{max-height:240px}.dshSchedules{padding:10px}.dshScheduleDetails dl{grid-template-columns:minmax(0,1fr)}}`;
    function ScheduleForm({state,controller,t,sessions,archived}) {
      const draft=state.draft,editing=!!state.expected;
      const field=(name,label,type="text",extra={})=>h("label",null,t(label),h("input",{...extra,type,value:draft[name],"aria-label":t(label),onChange:event=>controller.change(name,event.target.value)}));
      const calendar=["at","daily","weekly"].includes(draft.kind);
      return h("form",{onSubmit:event=>{event.preventDefault();void controller.save();},"aria-label":t(editing?"edit":"add")},
        h("fieldset",{disabled:state.busy||!state.enabled},h("legend",null,t(editing?"edit":"add")),
          editing?h("p",null,`${t("session")}: ${draft.sessionId}`):h("label",null,t("session"),h("select",{"aria-label":t("session"),value:draft.sessionId,onChange:event=>controller.change("sessionId",event.target.value)},h("option",{value:""},t("chooseSession")),...sessions.filter(row=>!row.parentSessionId&&!archived.includes(row.id)).map(row=>h("option",{key:row.id,value:row.id},row.title||row.id)))),
          field("title","title","text",{required:true,maxLength:300}),h("label",null,t("prompt"),h("textarea",{"aria-label":t("prompt"),required:true,value:draft.prompt,onChange:event=>controller.change("prompt",event.target.value)})),
          h("label",null,t("rule"),h("select",{"aria-label":t("rule"),value:draft.kind,onChange:event=>controller.change("kind",event.target.value)},...["after","at","every","daily","weekly","cron"].filter(kind=>!editing||kind!=="after").map(kind=>h("option",{key:kind,value:kind},t(kind))))),
          editing&&["after","at"].includes(state.expected.kind)&&h("p",{className:"dshScheduleHint"},t("timingHint")),
          ["after","every"].includes(draft.kind)&&field("seconds","seconds","number",{required:true,min:draft.kind==="every"?60:1,step:1}),
          draft.kind==="at"&&field("date","date","date",{required:true}),calendar&&field("time","time","time",{required:true,step:"0.001"}),
          draft.kind==="weekly"&&h("fieldset",null,h("legend",null,t("weekdays")),h("div",{className:"dshScheduleWeek"},...["mon","tue","wed","thu","fri","sat","sun"].map((key,index)=>h("label",{key},h("input",{type:"checkbox",checked:draft.weekdays.includes(index+1),onChange:event=>controller.change("weekdays",event.target.checked?[...draft.weekdays,index+1]:draft.weekdays.filter(day=>day!==index+1))}),t(key))))),
          draft.kind==="cron"&&h(React.Fragment,null,field("expression","cronExpression","text",{required:true}),h("p",{className:"dshScheduleHint"},t("cronHint"))),
          (calendar||draft.kind==="cron")&&h(React.Fragment,null,field("timeZone","zone","text",{required:true,list:"dsh-schedule-zones"}),h("datalist",{id:"dsh-schedule-zones"},...["UTC",zoneNow(),"Asia/Shanghai","Europe/London","America/New_York"].filter((value,index,rows)=>rows.indexOf(value)===index).map(zone=>h("option",{key:zone,value:zone}))),h("p",{className:"dshScheduleHint"},t("zoneHint"))),
          h("button",{type:"submit",disabled:state.conflict},state.busy?t("pending"):t("save"))),
        state.conflict&&h("p",{role:"alert"},t("conflict")),
        h("div",{className:"dshScheduleBar"},editing&&h("button",{type:"button",disabled:state.busy,onClick:()=>void controller.rebase()},t("reload")),h("button",{type:"button",disabled:state.busy,onClick:()=>controller.discard()},t("discard"))));
    }
    function RetentionSettings({connection,entryId,t}) {
      const generation=React.useSyncExternalStore(connection.generation.subscribe,connection.generation.getSnapshot);
      const owner=React.useRef(null),[state,setState]=React.useState({snapshot:null,days:"30",records:"200",busy:true,error:"",notice:""});
      React.useEffect(()=>{
        const current={alive:true,sequence:0,abort:null};owner.current=current;
        const initial={snapshot:null,days:"30",records:"200",busy:false,error:"",notice:""};current.state=initial;setState(initial);
        return()=>{current.alive=false;current.abort?.abort();if(owner.current===current)owner.current=null;};
      },[connection,entryId,generation]);
      const run=async(save)=>{
        const current=owner.current;if(!current||!current.alive||current.state.busy||connection.generation.getSnapshot()!==generation)return;
        const previous=current.state,sequence=++current.sequence;current.abort?.abort();const abort=new AbortController();current.abort=abort;
        const valid=()=>owner.current===current&&current.alive&&current.sequence===sequence&&!abort.signal.aborted&&connection.generation.getSnapshot()===generation;
        const update=patch=>{if(valid()){current.state={...current.state,...patch};setState(current.state);}};
        let config;
        if(save){
          const days=Number(previous.days),records=Number(previous.records);
          if(!Number.isInteger(days)||days<1||days>3650||!Number.isInteger(records)||records<1||records>10000){update({error:t("invalidRetention")});return;}
          if(!previous.snapshot)return;
          if(days===(previous.snapshot.config?.deliveryHistoryDays??30)&&records===(previous.snapshot.config?.deliveryHistoryRecords??200))return;
          config={...previous.snapshot.config,deliveryHistoryDays:days,deliveryHistoryRecords:records};
        }
        update({busy:true,error:"",notice:""});
        try{
          const result=await connection.rpc.call("/api",save?"pluginInventory.setConfig":"pluginInventory.getConfig",save?{entryId,expectedRevision:previous.snapshot.revision,config}:{entryId},abort.signal);
          if(!valid())return;if(!result.ok)throw Error(result.error?.message||t("failed"));
          const value=result.value;if(value?.entryId!==entryId||typeof value.revision!=="string"||(value.config!==undefined&&(!value.config||typeof value.config!=="object"||Array.isArray(value.config))))throw Error(t("invalidResponse"));
          update({snapshot:value,busy:false,...(!previous.snapshot||save?{days:String(value.config?.deliveryHistoryDays??30),records:String(value.config?.deliveryHistoryRecords??200)}:{}),notice:save?t("configSaved"):""});
        }catch(error){if(valid())update({busy:false,error:errorText(error,t)});}
      };
      React.useEffect(()=>{void run(false);},[connection,entryId,generation]);
      const edit=(field,value)=>{const current=owner.current;if(current&&!current.state.busy){current.state={...current.state,[field]:value,error:"",notice:""};setState(current.state);}};
      const dirty=state.snapshot&&(Number(state.days)!==(state.snapshot.config?.deliveryHistoryDays??30)||Number(state.records)!==(state.snapshot.config?.deliveryHistoryRecords??200));
      return h("details",{className:"dshScheduleCard"},h("summary",null,t("retentionSettings")),h("p",{className:"dshScheduleHint"},t("retentionHint")),
        h("label",null,t("retentionDays"),h("input",{type:"number",min:1,max:3650,step:1,value:state.days,disabled:state.busy,"aria-label":t("retentionDays"),onChange:event=>edit("days",event.target.value)})),
        h("label",null,t("retentionRecords"),h("input",{type:"number",min:1,max:10000,step:1,value:state.records,disabled:state.busy,"aria-label":t("retentionRecords"),onChange:event=>edit("records",event.target.value)})),
        state.error&&h("p",{role:"alert"},state.error),state.notice&&h("p",{role:"status"},state.notice),
        h("div",{className:"dshScheduleBar"},h("button",{disabled:state.busy||!dirty,onClick:()=>void run(true)},t("saveConfig")),h("button",{disabled:state.busy,onClick:()=>void run(false)},t("reloadConfig"))));
    }
    function ScheduleManager({services,lang}) {
      const words=copy(lang),t=key=>words[key]||key;
      const generation=React.useSyncExternalStore(services.connection.generation.subscribe,services.connection.generation.getSnapshot);
      const sessionSnapshot=React.useSyncExternalStore(services.sessionStore.subscribe,services.sessionStore.getSnapshot);
      const workspaceSnapshot=React.useSyncExternalStore(services.workspaceStore.subscribe,services.workspaceStore.getSnapshot);
      const [state,setState]=React.useState(null),owner=React.useRef(null),[query,setQuery]=React.useState(""),[filter,setFilter]=React.useState("all");
      React.useEffect(()=>{
        const controller=createScheduleController(services,value=>{if(owner.current===controller)setState(value);},t);
        owner.current=controller;setState(controller.getSnapshot());void controller.load();
        return()=>{controller.dispose();if(owner.current===controller)owner.current=null;};
      },[services,generation,words]);
      const controller=owner.current;
      if(!state||!controller)return h("p",{role:"status"},t("loading"));
      const sessions=Object.values(sessionSnapshot.byId||{}),archived=workspaceSnapshot.archivedSessionIds||[];
      const selected=state.records.find(row=>keyOf(row)===state.selected),needle=query.trim().toLocaleLowerCase();
      const records=state.records.filter(row=>(filter==="all"||row.status===filter)&&`${row.title} ${row.prompt} ${row.sessionId}`.toLocaleLowerCase().includes(needle));
      const sessionMissing=selected&&!sessions.some(row=>row.id===selected.sessionId),sessionArchived=selected&&archived.includes(selected.sessionId);
      const historyView=selected&&h("section",{"aria-label":t("history")},
        h("h3",null,t("history")),h("p",{className:"dshScheduleHint"},t("historyHint")),
        state.historyError&&h("p",{role:"alert"},state.historyError),h("button",{disabled:state.historyBusy,onClick:()=>void controller.history()},t("refresh")),
        state.history&&h(React.Fragment,null,
          !state.history.records.length&&h("p",null,t("noHistory")),
          h("ol",{className:"dshScheduleHistory"},...state.history.records.map(row=>h("li",{key:row.messageId},
            h("div",null,`${t("scheduled")}: ${displayTime(row.scheduledAt)}`),h("div",null,`${t("delivered")}: ${displayTime(row.deliveredAt)}`),
            h("small",null,`${t("message")}: ${row.messageId}`),h("pre",null,row.prompt??t("recordUnavailable"))))),
          state.history.earlierRecordsUnavailable&&h("p",null,t("historyUnavailable")),state.history.earlierRecordsPruned&&h("p",null,t("historyPruned")),
          state.history.retention&&h("p",{className:"dshScheduleHint"},`${t("retention")}: ${state.history.retention.days} ${t("days")} / ${state.history.retention.records} ${t("records")}`),
          state.history.nextBefore&&h("button",{disabled:state.historyBusy,onClick:()=>void controller.history(true)},t("more"))));
      const detailView=selected&&h(React.Fragment,null,
        h("h3",null,selected.title),h("pre",null,selected.prompt),
        h("dl",null,h("dt",null,t("session")),h("dd",null,selected.sessionId),h("dt",null,t("status")),h("dd",null,t(selected.status)),h("dt",null,t("next")),h("dd",null,displayTime(selected.scheduledAt)),
          selected.timeZone&&h(React.Fragment,null,h("dt",null,t("zone")),h("dd",null,selected.timeZone)),
          selected.lastDelivery&&h(React.Fragment,null,h("dt",null,t("last")),h("dd",null,displayTime(selected.lastDelivery.deliveredAt)))),
        h("div",{className:"dshScheduleBar"},h("button",{disabled:!!sessionMissing||!!sessionArchived,onClick:()=>services.openSession(selected.sessionId)},t("openSession")),
          h("button",{disabled:state.busy||!state.enabled||selected.status!=="active",onClick:()=>controller.edit()},t("edit")),h("button",{disabled:state.busy,onClick:()=>controller.confirmDelete()},t("delete"))),
        (sessionMissing||sessionArchived)&&h("p",{className:"dshScheduleHint"},t(sessionArchived?"archivedSession":"missingSession")),
        state.confirmDelete&&h("div",{role:"alertdialog","aria-label":t("confirmDelete")},h("p",null,t("deleteHint")),h("button",{disabled:state.busy,onClick:()=>void controller.remove()},t("confirmDelete")),h("button",{disabled:state.busy,onClick:()=>controller.cancelDelete()},t("cancel"))),historyView);
      return h("section",{className:"dshSchedules","aria-label":t("manager")},h("style",null,css),
        h("header",null,h("h2",null,t("manager")),h("button",{disabled:state.loading,onClick:()=>void controller.load()},t("refresh")),h("button",{disabled:state.busy||!state.enabled,onClick:()=>controller.add()},t("add"))),
        h("div",{className:"dshScheduleCard"},h("p",null,t(state.enabled?"enabled":"disabled")),h("p",{className:"dshScheduleHint"},t("originalHint")),state.entry?h("button",{disabled:state.busy,onClick:()=>void controller.toggle()},t(state.enabled?"disable":"enable")):h("p",{role:"alert"},t("unavailable")),state.enabled&&h("button",{disabled:state.busy,title:t("retryHint"),onClick:()=>void controller.retry()},t("retry"))),
        state.entry&&h(RetentionSettings,{connection:services.connection,entryId:state.entry.entryId,t}),
        state.error&&h("p",{role:"alert"},state.error),state.notice&&h("p",{role:"status"},state.notice),state.loading&&h("p",{role:"status"},t("loading")),
        state.leave&&h("div",{role:"alertdialog","aria-label":t("draftConfirm")},h("p",null,t("draftConfirm")),h("button",{onClick:()=>controller.keep()},t("keepDraft")),h("button",{onClick:()=>controller.leave()},t("leaveDraft"))),
        h("div",{className:"dshScheduleBar"},h("input",{type:"search",value:query,"aria-label":t("search"),placeholder:t("search"),onChange:event=>setQuery(event.target.value)}),h("select",{value:filter,"aria-label":t("status"),onChange:event=>setFilter(event.target.value)},...["all","active","inactive"].map(key=>h("option",{key,value:key},t(key))))),
        h("div",{className:"dshScheduleColumns"},h("ul",{className:"dshScheduleList"},...records.map(row=>h("li",{key:keyOf(row)},h("button",{"aria-pressed":keyOf(row)===state.selected,onClick:()=>controller.select(row)},h("strong",null,row.title),h("small",null,`${t(row.status)} · ${t(row.kind)}`),h("small",null,row.sessionId),h("small",null,displayTime(row.scheduledAt)))))),
          h("div",{className:"dshScheduleDetails"},!records.length&&!state.loading&&h("p",null,t("empty")),state.draft?h(ScheduleForm,{state,controller,t,sessions,archived}):detailView)));
    }
    function servicesFor(ctx,pluginController) {
      const empty={getSnapshot:()=>EMPTY_WORKSPACES,subscribe:()=>()=>{}};
      return {connection:ctx.connection,sessionStore:ctx.sessions.list,workspaceStore:ctx.workspaces?.list||empty,
        subscribeChanged:listener=>ctx.on("connection/host-envelope",envelope=>{if(envelope.payload?.type==="host/schedule-changed")listener(envelope.payload);}),
        pluginList:async signal=>{const result=await ctx.connection.rpc.call("/api","pluginInventory.list",{},signal);if(!result.ok)throw Error(result.error.message);return result.value;},
        setPluginEnabled:async(entry,enabled,signal)=>{if(!pluginController)throw Error(copy().enableHint);const cancel=()=>void pluginController.cancel().catch(()=>{});signal.addEventListener("abort",cancel,{once:true});try {if(signal.aborted)throw Object.assign(Error(copy().stale),{name:"AbortError"});await pluginController.setEnabled(entry,enabled);}finally{signal.removeEventListener("abort",cancel);}},
        openSession:id=>{ctx.layout?.selectPanel(null);ctx.sessions.open(id);}
      };
    }
    const EMPTY_WORKSPACES={archivedSessionIds:[]};
    function apply(ctx) {
      // Management remains available in Settings when the scheduler itself is disabled.
      ctx.slots.inject("conversation.view",()=>ctx.slots.register({name:"conversation.view",id:"reminders",order:17,label:()=>copy().manager},()=>h("div",{className:"dshSchedules"},h("button",{onClick:()=>ctx.emit("schedule/open-manager")},copy().manager))));
    }
    return {name:"schedule",inject:["slots","connection","sessions"],apply,ScheduleManager,RetentionSettings,servicesFor,createScheduleController,recordOf,draftOf,timing,zh,en};
  }
});
