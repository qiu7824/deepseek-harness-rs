window.__ModuleLoader__.load({
  id: "@deepseek-ai/dsh-client-ui-task-execution",
  factory: require => {
    const React = require("react"), h = React.createElement;
    const labels = {
      planned:"待执行",running:"执行中",validating:"验收中",validation_failed:"验收未通过",awaiting_user:"待人工验收",completed:"已验收完成",cancelled:"已取消",blocked:"待恢复核实",
      prepared:"已保存执行意图",dispatched:"已分派",effect_observed:"已观察到效果",verified:"执行结果已核实",committed:"已交付",failed:"执行失败",unknown:"效果未知",
      passed:"通过",unverified:"尚未验证",not_dispatched:"请求操作未启动"
    };
    const css = ".dshTaskExecution{max-width:1080px;margin:auto;padding:20px 24px;display:grid;gap:14px;color:var(--dsw-alias-label-primary);font-size:14px;line-height:1.6}.dshTaskExecution h2,.dshTaskExecution h3,.dshTaskExecution p{margin:0}.dshTaskExecution h2{font-size:18px;font-weight:600;line-height:1.45}.dshTaskExecution h3{font-size:15px}.dshTaskExecution header,.dshTaskExecution .actions{display:flex;gap:10px;align-items:center;flex-wrap:wrap}.dshTaskExecution header{justify-content:space-between}.dshTaskExecution article,.dshTaskExecution fieldset{border:1px solid var(--dsw-alias-border-l2);border-radius:10px;padding:14px;display:grid;gap:10px;min-width:0}.dshTaskExecution button,.dshTaskExecution select{font:inherit;background:var(--dsw-alias-bg-layer-1);color:inherit;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;padding:7px 12px;max-width:100%}.dshTaskExecution button{cursor:pointer}.dshTaskExecution button:disabled{opacity:.45;cursor:default}.dshTaskExecution :focus-visible{outline:2px solid var(--dsw-alias-state-business-primary);outline-offset:2px}.dshTaskExecution small{color:var(--dsw-alias-label-tertiary)}.dshTaskExecution code,.dshTaskExecution small,.dshTaskExecution p{overflow-wrap:anywhere}.dshTaskExecution [role=alert]{color:var(--dsw-alias-state-error-primary)}.dshTaskExecution [data-state=unknown],.dshTaskExecution [data-state=blocked],.dshTaskExecution [data-state=awaiting_user]{border-color:var(--dsw-alias-state-warning-primary,#b88929)}.dshTaskExecution ul{padding-left:22px;margin:0}.dshTaskExecution pre{white-space:pre-wrap;overflow-wrap:anywhere;max-height:220px;overflow:auto}.dshTaskExecution label{display:grid;gap:5px}.dshTaskExecution details summary{cursor:pointer}.dshTaskExecution .badge{font-size:12px;padding:2px 7px;border-radius:5px;background:var(--dsw-alias-bg-layer-2)}@media(max-width:640px){.dshTaskExecution{padding:14px 12px}}";
    async function request(sessionId, payload, signal) {
      const response = await fetch("/__dsh-task-execution", {method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({sessionId,...payload}),signal});
      let text;
      try { text = await response.text(); }
      catch (e) { if(e.name === "AbortError") throw e; throw new Error(`任务接口响应中断（HTTP ${response.status}）；操作结果未知，请先刷新核实。`); }
      if (!text.trim()) throw new Error(`任务接口返回空响应（HTTP ${response.status}）；操作结果未知，请先刷新核实。`);
      let value;
      try { value = JSON.parse(text); }
      catch { throw new Error(`任务接口返回无效或不完整 JSON（HTTP ${response.status}）；操作结果未知，请先刷新核实。`); }
      if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`任务接口响应格式错误（HTTP ${response.status}）`);
      if (!response.ok) { const failure=new Error(typeof value.error==="string"?value.error:value.error?.message||`HTTP ${response.status}`);failure.code=value.code||value.error?.code;throw failure; }
      return value;
    }
    const checkerNames={manual:"人工验收",text:"文本内容",json:"JSON 内容",image:"图片尺寸与通道",office_package:"Office 文件结构",tool_result:"执行结果"};
    const draftPrefix="dsh:task-requirements:v2:",volatileDrafts=new Map();
    let draftSequence=0;
    const clone=value=>JSON.parse(JSON.stringify(value));
    function draftKey(sessionId,taskId){return draftPrefix+encodeURIComponent(sessionId)+":"+encodeURIComponent(taskId)+":";}
    function recordKey(value){return draftKey(value.sessionId,value.taskId)+"draft:"+value.writer+":"+value.token;}
    function freshDraft(value,writer){return {...value,version:2,kind:"draft",writer,writerSequence:++draftSequence,savedAt:Date.now(),token:crypto.randomUUID()};}
    function storedDraftEntries(sessionId,taskId){
      const prefix=draftKey(sessionId,taskId),records=new Map();
      try{for(let i=0;i<window.localStorage.length;i++){const key=window.localStorage.key(i);if(key?.startsWith(prefix))records.set(key,window.localStorage.getItem(key));}}catch{}
      for(const [key,raw] of volatileDrafts)if(key.startsWith(prefix))records.set(key,raw);
      const result=[];
      for(const [key,raw] of records){try{const value=JSON.parse(raw);if(value.version===2&&value.sessionId===sessionId&&value.taskId===taskId)result.push({key,raw,value});}catch{}}
      return result;
    }
    function draftRecords(sessionId,taskId){
      const entries=storedDraftEntries(sessionId,taskId),completed=new Set(),rejected=new Set(),retired=new Set(),writers=new Map();
      for(const {value} of entries){if(value.kind==="receipt"){completed.add(value.submissionId);for(const key of value.records??[])retired.add(key);}else if(value.kind==="rejected")rejected.add(value.submissionId);}
      for(const row of entries){
        const value=row.value;
        if(value.kind!=="draft"||row.key!==recordKey(value)||!value.contract||!Number.isSafeInteger(value.baseRevision)||retired.has(row.key)||completed.has(value.pendingSave?.idempotencyKey))continue;
        const current=writers.get(value.writer);
        if(!current||value.writerSequence>current.value.writerSequence)writers.set(value.writer,rejected.has(value.pendingSave?.idempotencyKey)?{...row,value:{...value,pendingSave:null}}:row);
      }
      return [...writers.values()].sort((a,b)=>b.value.savedAt-a.value.savedAt||b.value.writerSequence-a.value.writerSequence);
    }
    function removeDraft(record){
      // Record keys include a never-reused token; another editor cannot replace
      // this key with a newer draft between inspection and deletion.
      volatileDrafts.delete(record.key);
      try{window.localStorage.removeItem(record.key);}catch{}
    }
    function persistDraft(value){
      const key=recordKey(value),raw=JSON.stringify(value);
      try{
        const previous=window.localStorage.getItem(key);
        if(previous!==null&&previous!==raw)throw new Error("Immutable draft identity was reused");
        window.localStorage.setItem(key,raw);volatileDrafts.delete(key);
        for(const record of storedDraftEntries(value.sessionId,value.taskId))if(record.value.kind==="draft"&&record.value.writer===value.writer&&record.value.writerSequence<value.writerSequence)removeDraft(record);
        return {ok:true,error:""};
      }catch{
        volatileDrafts.set(key,raw);
        for(const [otherKey,otherRaw] of volatileDrafts){if(otherKey===key)continue;try{const older=JSON.parse(otherRaw);if(older.kind==="draft"&&older.writer===value.writer&&older.writerSequence<value.writerSequence)volatileDrafts.delete(otherKey);}catch{}}
        return {ok:false,error:"草稿目前只保留在此窗口；本地存储不可用，请保持窗口打开。保存请求会在操作标识持久保存后发出。"};
      }
    }
    function sameDraftContent(left,right){
      if(left.baseRevision!==right.baseRevision||left.mode!==right.mode)return false;
      try{return JSON.stringify(savedContract(left.contract))===JSON.stringify(savedContract(right.contract));}catch{return false;}
    }
    function completeDraft(sending,targetTaskId){
      const committed=draftRecords(sending.sessionId,sending.taskId).filter(row=>row.value.pendingSave?.idempotencyKey===sending.pendingSave.idempotencyKey||(!row.value.pendingSave&&sameDraftContent(row.value,sending)));
      const committedWriters=new Map(committed.map(row=>[row.value.writer,row.value.writerSequence]));
      const records=storedDraftEntries(sending.sessionId,sending.taskId).filter(row=>row.value.kind==="draft"&&row.key===recordKey(row.value)&&row.value.writerSequence<=(committedWriters.get(row.value.writer)??-1));
      const receipt={version:2,kind:"receipt",sessionId:sending.sessionId,taskId:sending.taskId,submissionId:sending.pendingSave.idempotencyKey,targetTaskId,records:records.map(row=>row.key)};
      const key=draftKey(sending.sessionId,sending.taskId)+"receipt:"+crypto.randomUUID(),raw=JSON.stringify(receipt);
      let persisted=false;
      try{window.localStorage.setItem(key,raw);persisted=true;}catch{volatileDrafts.set(key,raw);}
      // Preserve the complete durable intent until its receipt is durable too.
      // Different submissions/content in another window are retained.
      if(persisted)for(const row of records)removeDraft(row);
      return persisted?"":"要求已保存；本地回执未能持久保存，重新打开后可用原保存标识再次核实。";
    }
    function rejectDraft(sending,code){
      const value={version:2,kind:"rejected",sessionId:sending.sessionId,taskId:sending.taskId,submissionId:sending.pendingSave.idempotencyKey,code};
      const key=draftKey(sending.sessionId,sending.taskId)+"rejected:"+crypto.randomUUID(),raw=JSON.stringify(value);
      try{window.localStorage.setItem(key,raw);return true;}catch{return false;}
    }
    function jsonDraft(value){
      if(value===null)return {type:"null"};
      if(Array.isArray(value))return {type:"array",items:value.map(jsonDraft)};
      if(typeof value==="object")return {type:"object",items:Object.entries(value).map(([key,value])=>({key,value:jsonDraft(value)}))};
      return {type:typeof value,value:String(value)};
    }
    function jsonValue(node){
      if(node.type==="null")return null;
      if(node.type==="string")return node.value??"";
      if(node.type==="boolean")return node.value==="true";
      if(node.type==="number"){const raw=(node.value??"").trim();if(!/^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?$/.test(raw)||!Number.isFinite(Number(raw)))throw new Error("预期数值必须是有效的有限数字。");if(Number.isInteger(Number(raw))&&!Number.isSafeInteger(Number(raw)))throw new Error("预期整数超出浏览器精确表示范围，不能静默舍入后保存。");return Number(raw);}
      if(node.type==="array")return node.items.map(jsonValue);
      const entries=node.items.map(row=>[row.key,jsonValue(row.value)]);
      if(new Set(entries.map(([key])=>key)).size!==entries.length)throw new Error("预期对象包含重复字段名。");
      return Object.fromEntries(entries);
    }
    function editorContract(spec){
      const value=clone(spec);
      for(const check of value.acceptanceChecks){if(["json","tool_result"].includes(check.checker.kind)){check.checker.assertionRows=Object.entries(check.checker.assertions??{}).map(([path,value])=>({path,value:jsonDraft(value)}));delete check.checker.assertions;}}
      return value;
    }
    function savedContract(draft){
      const contract=clone(draft);delete contract.environmentFingerprint;
      if(!contract.objective?.trim())throw new Error("请填写任务目标。");
      if(!contract.acceptanceChecks?.length)throw new Error("至少保留一项验收要求。");
      for(const check of contract.acceptanceChecks){
        if(!check.description.trim())throw new Error("请填写每项验收要求的说明。");
        const checker=check.checker;
        if(["json","tool_result"].includes(checker.kind)){
          const rows=checker.assertionRows??[];
          if(!rows.length)throw new Error("JSON 或执行结果检查至少需要一条预期值。");
          if(new Set(rows.map(row=>row.path)).size!==rows.length)throw new Error("同一检查内不能有重复的 JSON 路径。");
          if(rows.some(row=>row.path!==""&&!row.path.startsWith("/")))throw new Error("JSON 路径须以 / 开头；留空表示整个值。");
          checker.assertions=Object.fromEntries(rows.map(row=>[row.path,jsonValue(row.value)]));delete checker.assertionRows;
        }
        if(checker.kind==="image")for(const key of ["min_width","min_height","channels"]){if(key==="channels"&&(checker[key]===""||checker[key]==null)){delete checker[key];continue;}const value=Number(checker[key]);if(!Number.isInteger(value)||value<0||value>(key==="channels"?255:4294967295))throw new Error("图片尺寸和通道数须填写有效的非负整数。");checker[key]=value;}
      }
      if(new TextEncoder().encode(JSON.stringify(contract)).length>128*1024)throw new Error("任务要求超过 128 KiB，请缩短内容后保存。");
      return contract;
    }
    function RequirementField({label,value,onChange,multiline=false,disabled=false,...props}){
      return h("label",null,h("span",null,label),h(multiline?"textarea":"input",{...props,value:value??"",disabled,onChange:event=>onChange(event.target.value),...(multiline?{rows:3}:{} )}));
    }
    function StringRequirements({label,values,onChange,disabled,multiline=false}){
      return h("fieldset",null,h("legend",null,label),...values.map((value,index)=>h("div",{className:"requirement-row",key:index},h(RequirementField,{label:`${label} ${index+1}`,value,multiline,disabled,onChange:next=>onChange(values.map((v,i)=>i===index?next:v))}),!disabled&&h("button",{type:"button",onClick:()=>onChange(values.filter((_,i)=>i!==index)),"aria-label":`删除${label} ${index+1}`} ,"删除"))),!disabled&&h("button",{type:"button",disabled:values.length>=64,onClick:()=>onChange([...values,""])},`添加${label}`));
    }
    function JsonRequirementValue({node,onChange,disabled,label="预期值"}){
      const [expanded,setExpanded]=React.useState(false),complex=["array","object"].includes(node.type);
      const changeType=type=>onChange(type==="null"?{type}:complexDefaults(type));
      function complexDefaults(type){return ["array","object"].includes(type)?{type,items:[]}:{type,value:type==="boolean"?"false":type==="number"?"0":""};}
      return h("div",{className:"requirement-value"},h("label",null,label+"类型",h("select",{value:node.type,disabled,onChange:e=>changeType(e.target.value)},...[['string','文本'],['number','数字'],['boolean','布尔'],['null','空值'],['object','对象'],['array','数组']].map(([value,label])=>h("option",{key:value,value},label)))),node.type==="boolean"?h("label",null,label,h("select",{value:node.value,disabled,onChange:e=>onChange({...node,value:e.target.value})},h("option",{value:"true"},"true"),h("option",{value:"false"},"false"))):!complex&&node.type!=="null"?h(RequirementField,{label,value:node.value,disabled,onChange:value=>onChange({...node,value})}):null,
        complex&&h("details",{open:expanded,onToggle:e=>setExpanded(e.currentTarget.open)},h("summary",null,`${label} · ${node.items.length} 项`),expanded&&h("div",null,...node.items.map((row,index)=>h("fieldset",{key:index},h("legend",null,`第 ${index+1} 项`),node.type==="object"&&h(RequirementField,{label:"字段名",value:row.key,disabled,onChange:key=>onChange({...node,items:node.items.map((v,i)=>i===index?{...v,key}:v)})}),h(JsonRequirementValue,{node:node.type==="object"?row.value:row,disabled,onChange:value=>onChange({...node,items:node.items.map((v,i)=>i===index?(node.type==="object"?{...v,value}:value):v)})}),!disabled&&h("button",{type:"button",onClick:()=>onChange({...node,items:node.items.filter((_,i)=>i!==index)})},"删除此值"))),!disabled&&h("button",{type:"button",onClick:()=>onChange({...node,items:[...node.items,node.type==="object"?{key:"",value:jsonDraft("")}:jsonDraft("")]})},"添加值"))));
    }
    function blankChecker(kind){return kind==="text"?{kind,path:"",required:[""],forbidden:[]}:kind==="json"?{kind,path:"",assertionRows:[{path:"",value:jsonDraft("")}]}:kind==="tool_result"?{kind,step_id:"",assertionRows:[{path:"",value:jsonDraft("")}]}:kind==="image"?{kind,path:"",min_width:1,min_height:1}:kind==="office_package"?{kind,path:"",format:"docx"}:{kind:"manual",reason:""};}
    function CheckerRequirements({check,onChange,disabled}){
      const checker=check.checker,change=(key,value)=>onChange({...check,checker:{...checker,[key]:value}});
      return h(React.Fragment,null,h(RequirementField,{label:"验收说明",value:check.description,multiline:true,disabled,onChange:description=>onChange({...check,description})}),h("label",null,"检查方式",h("select",{value:checker.kind,disabled,onChange:e=>onChange({...check,checker:blankChecker(e.target.value)})},...Object.entries(checkerNames).map(([value,name])=>h("option",{key:value,value},name)))),
        ["text","json","image","office_package"].includes(checker.kind)&&h(RequirementField,{label:"文件路径",value:checker.path,disabled,onChange:value=>change("path",value)}),
        checker.kind==="text"&&h(React.Fragment,null,h(StringRequirements,{label:"必须包含的内容",values:checker.required??[],multiline:true,disabled,onChange:value=>change("required",value)}),h(StringRequirements,{label:"不得包含的内容",values:checker.forbidden??[],multiline:true,disabled,onChange:value=>change("forbidden",value)})),
        checker.kind==="manual"&&h(RequirementField,{label:"人工检查范围",value:checker.reason,multiline:true,disabled,onChange:value=>change("reason",value)}),
        checker.kind==="office_package"&&h("label",null,"文档格式",h("select",{value:checker.format,disabled,onChange:e=>change("format",e.target.value)},...['docx','xlsx','pptx'].map(value=>h("option",{key:value,value},value)))),
        checker.kind==="image"&&h("div",{className:"requirement-grid"},...[['min_width','最小宽度'],['min_height','最小高度'],['channels','通道数（可留空）']].map(([key,label])=>h(RequirementField,{key,label,value:checker[key],inputMode:"numeric",disabled,onChange:value=>change(key,value)}))),
        checker.kind==="tool_result"&&h(RequirementField,{label:"步骤标识或 tool:工具名",value:checker.step_id,disabled,onChange:value=>change("step_id",value)}),
        ["json","tool_result"].includes(checker.kind)&&h("fieldset",null,h("legend",null,"内容预期值"),...(checker.assertionRows??[]).map((row,index)=>h("fieldset",{key:index},h("legend",null,`预期值 ${index+1}`),h(RequirementField,{label:"JSON 路径（留空表示整个值）",value:row.path,disabled,onChange:path=>change("assertionRows",checker.assertionRows.map((v,i)=>i===index?{...v,path}:v))}),h(JsonRequirementValue,{node:row.value,disabled,onChange:value=>change("assertionRows",checker.assertionRows.map((v,i)=>i===index?{...v,value}:v))}),!disabled&&h("button",{type:"button",onClick:()=>change("assertionRows",checker.assertionRows.filter((_,i)=>i!==index))},"删除预期值"))),!disabled&&h("button",{type:"button",onClick:()=>change("assertionRows",[...(checker.assertionRows??[]),{path:"",value:jsonDraft("")}])},"添加预期值")));
    }
    function ContractRequirementsForm({contract,onChange,disabled=false}){
      const field=(key,value)=>onChange({...contract,[key]:value});
      return h("div",{className:"requirement-form"},h(RequirementField,{label:"任务目标",value:contract.objective,multiline:true,disabled,onChange:value=>field("objective",value)}),h(StringRequirements,{label:"约束",values:contract.constraints??[],multiline:true,disabled,onChange:value=>field("constraints",value)}),h(StringRequirements,{label:"预期产物",values:contract.expectedOutputs??[],disabled,onChange:value=>field("expectedOutputs",value)}),
        h("fieldset",null,h("legend",null,"验收要求"),...contract.acceptanceChecks.map((check,index)=>h("fieldset",{key:check.id},h("legend",null,`验收项 ${index+1}`),h(CheckerRequirements,{check,disabled,onChange:value=>field("acceptanceChecks",contract.acceptanceChecks.map((v,i)=>i===index?value:v))}),!disabled&&h("button",{type:"button",onClick:()=>field("acceptanceChecks",contract.acceptanceChecks.filter((_,i)=>i!==index))},"删除验收项"))),!disabled&&h("button",{type:"button",disabled:contract.acceptanceChecks.length>=64,onClick:()=>field("acceptanceChecks",[...contract.acceptanceChecks,{id:"check-"+crypto.randomUUID(),description:"",checker:blankChecker("manual")}])},"添加验收项")),
        contract.goalId&&h("small",null,`关联目标：${contract.goalId}`),h("details",null,h("summary",null,"验收对象"),h("label",{className:"requirement-checkbox"},h("input",{type:"checkbox",checked:!!contract.validationSubject,disabled,onChange:e=>field("validationSubject",e.target.checked?{kind:"",identity:"",expectedOutcome:""}:null)}),"指定验收对象"),contract.validationSubject&&h(React.Fragment,null,...[['kind','对象类型'],['identity','对象标识'],['expectedOutcome','预期结果']].map(([key,label])=>h(RequirementField,{key,label,value:contract.validationSubject[key],disabled,onChange:value=>field("validationSubject",{...contract.validationSubject,[key]:value})})))));
    }
    function revisionError(error){return ({TASK_BUSY:"任务仍在执行或验收，请先停止工作，再保存要求。",TASK_REVISION_CONFLICT:"任务版本已变化；草稿已保留，请核对最新要求。",TASK_IDEMPOTENCY_CONFLICT:"该保存标识已有不同内容，请先核对历史记录。",TASK_ENVIRONMENT_CHANGED:"执行环境已变化，请先完成环境切换，再保存要求。",TASK_SUCCESSOR_REQUIRED:"此任务已完成，请创建后续任务以保留历史。",TASK_ACTIVE_CONTRACT:"当前会话还有未结束的任务，暂时无法创建后续任务。"})[error.code]||error.message||String(error);}
    function TaskRequirementsEditor({sessionId,task,onSaved,onClose,onReload}){
      const writer=React.useRef(null),initial=React.useRef(null);if(!writer.current)writer.current=crypto.randomUUID();
      if(!initial.current){const records=draftRecords(sessionId,task.taskId),existing=records.find(row=>row.value.pendingSave)??records.find(row=>row.value.baseRevision===task.revision);initial.current={record:existing,records,value:freshDraft(existing?.value??{sessionId,taskId:task.taskId,baseRevision:task.revision,mode:task.state==="completed"?"successor":"in_place",contract:editorContract(task.spec)},writer.current)};}
      const [draft,setDraft]=React.useState(initial.current.value),[saving,setSaving]=React.useState(false),[error,setError]=React.useState(""),[storageError,setStorageError]=React.useState(""),[copies,setCopies]=React.useState(initial.current.records);
      const latest=React.useRef(draft),active=React.useRef(true),inFlight=React.useRef(false),origin=React.useRef(initial.current.record);latest.current=draft;
      const key=recordKey(draft);const store=next=>{latest.current=next;const result=persistDraft(next);if(active.current)setStorageError(result.error);return result;};
      const update=changes=>{const next=freshDraft({...latest.current,...changes},writer.current);store(next);setDraft(next);setError("");};
      React.useEffect(()=>{active.current=true;const sync=()=>setCopies(draftRecords(sessionId,task.taskId));window.addEventListener?.("storage",sync);return()=>{active.current=false;window.removeEventListener?.("storage",sync);};},[sessionId,task.taskId]);
      const conflict=task.revision!==draft.baseRevision,waiting=!!draft.pendingSave;
      const save=async()=>{
        if(inFlight.current)return;
        let payload=draft.pendingSave;
        try{if(!payload)payload={action:"revise",taskId:task.taskId,expectedRevision:draft.baseRevision,idempotencyKey:crypto.randomUUID(),mode:draft.mode,contract:savedContract(draft.contract)};}catch(error){setError(error.message);return;}
        const sending=freshDraft({...latest.current,pendingSave:payload},writer.current);const persisted=store(sending);setDraft(sending);if(!persisted.ok){setError("本次保存尚未发送：无法持久保存操作标识。");return;}setSaving(true);setError("");inFlight.current=true;
        try{
          const result=await request(sessionId,payload);
          if(!result.task?.taskId)throw new Error("保存回执缺少任务标识，请使用相同标识重试核实。");
          const warning=completeDraft(sending,result.task.taskId);
          if(active.current)await onSaved(result.task.taskId,warning);
        }catch(error){if(active.current){setError(revisionError(error));if(["TASK_REVISION_CONFLICT","TASK_ENVIRONMENT_CHANGED","TASK_SUCCESSOR_REQUIRED","TASK_INVALID_REVISION_MODE","TASK_ACTIVE_CONTRACT","TASK_INVALID_CONTRACT"].includes(error.code)&&rejectDraft(sending,error.code)){const next=freshDraft({...latest.current,pendingSave:null},writer.current);store(next);setDraft(next);if(error.code==="TASK_REVISION_CONFLICT")void onReload();}}}
        finally{inFlight.current=false;if(active.current)setSaving(false);}
      };
      const preserveCurrent=()=>{const copy=freshDraft({...latest.current},crypto.randomUUID());persistDraft(copy);setCopies(draftRecords(sessionId,task.taskId));};
      const restore=row=>{preserveCurrent();origin.current=row;update({...clone(row.value),sessionId,taskId:task.taskId});};
      return h("article",{className:"requirement-editor","aria-label":"编辑任务要求"},h("h3",null,"编辑任务要求"),h("p",null,"要求变更后需要重新验收；执行记录、未知效果和历史版本会保留。保存不会自动启动任务。"),error&&h("p",{role:"alert"},error),storageError&&h("p",{role:"alert"},storageError),
        copies.filter(row=>row.key!==key&&row.key!==origin.current?.key).length>0&&h("details",null,h("summary",null,"其他窗口或先前保留的草稿"),...copies.filter(row=>row.key!==key).map(row=>h("div",{className:"actions",key:row.key},h("span",null,`${new Date(row.value.savedAt).toLocaleString()} · ${row.value.contract.objective||"未填写目标"}`),h("button",{type:"button",disabled:saving||waiting,onClick:()=>restore(row)},"载入此草稿")))),
        conflict&&!waiting&&h("fieldset",null,h("legend",null,"版本已变化"),h("p",null,`草稿基于版本 ${draft.baseRevision}，当前为版本 ${task.revision}。`),h("details",null,h("summary",null,"查看当前已保存要求"),h(ContractRequirementsForm,{contract:editorContract(task.spec),disabled:true})),h("div",{className:"actions"},h("button",{type:"button",disabled:saving,onClick:()=>update({baseRevision:task.revision})},"保留我的内容并使用当前版本"),h("button",{type:"button",disabled:saving,onClick:()=>{preserveCurrent();origin.current=null;update({baseRevision:task.revision,contract:editorContract(task.spec),mode:task.state==="completed"?"successor":"in_place"});}},"使用当前已保存要求"))),
        h("label",null,"保存方式",h("select",{value:draft.mode,disabled:saving||waiting,onChange:e=>update({mode:e.target.value})},h("option",{value:"in_place",disabled:task.state==="completed"},task.state==="cancelled"?"更新此任务，保持已取消":"更新此任务"),h("option",{value:"successor",disabled:!["completed","cancelled"].includes(task.state)},"创建后续任务，保留原任务"))),
        task.state==="completed"&&h("p",null,"原任务保持已完成；保存将创建关联原版本的新任务。"),h(ContractRequirementsForm,{contract:draft.contract,disabled:saving||waiting,onChange:contract=>update({contract})}),
        waiting&&h("p",{role:"status"},saving?"正在保存要求…":"上次保存结果尚未确认；草稿及保存标识已保留，重试不会重复创建任务。"),h("div",{className:"actions"},h("button",{type:"button",disabled:saving||(!waiting&&conflict)||(!waiting&&task.state==="completed"&&draft.mode!=="successor"),onClick:save},waiting?"重试保存并核实":draft.mode==="successor"?"保存为后续任务":"保存任务要求"),h("button",{type:"button",onClick:()=>{store(latest.current);onClose();}},"收起并保留草稿")));
    }
    function RequirementsHistory({sessionId,taskId}){
      const [history,setHistory]=React.useState(null),[snapshot,setSnapshot]=React.useState(null),[error,setError]=React.useState(""),[busy,setBusy]=React.useState(false),generation=React.useRef(0);
      React.useEffect(()=>{const abort=new AbortController();setHistory(null);setSnapshot(null);setError("");request(sessionId,{action:"requirements_history",taskId},abort.signal).then(value=>{if(!abort.signal.aborted)setHistory(value.history);}).catch(error=>{if(!abort.signal.aborted)setError(error.message);});return()=>{abort.abort();generation.current++;};},[sessionId,taskId]);
      const inspect=async row=>{const current=++generation.current;setBusy(true);setError("");try{const value=await request(sessionId,{action:"requirements_snapshot",taskId:row.sourceTaskId??taskId,idempotencyKey:row.idempotencyKey});if(current===generation.current)setSnapshot(value.snapshot);}catch(error){if(current===generation.current)setError(error.message);}finally{if(current===generation.current)setBusy(false);}};
      return h("article",null,h("h3",null,"要求版本历史"),error&&h("p",{role:"alert"},error),history===null?h("p",null,"正在读取历史…"):!history.length?h("p",null,"尚无要求修订记录。"):h("ul",null,...history.map(row=>h("li",{key:(row.sourceTaskId??taskId)+":"+row.idempotencyKey},h("div",{className:"actions"},h("span",null,`版本 ${row.sourceRevision} → ${row.targetRevision}${row.targetTaskId!==taskId?" · 后续任务":""}`),h("button",{type:"button",disabled:busy,onClick:()=>inspect(row)},`查看原版本 ${row.sourceRevision}`))))),snapshot&&h("details",{open:true},h("summary",null,`历史任务 ${snapshot.taskId} · 版本 ${snapshot.revision}`),h(ContractRequirementsForm,{contract:editorContract(snapshot.spec),disabled:true})));
    }
    function TaskExecutionView({sessionId}) {
      const [editing,setEditing]=React.useState(false),[historyOpen,setHistoryOpen]=React.useState(false);
      React.useEffect(()=>{setEditing(false);setHistoryOpen(false);},[sessionId]);
      const [tasks,setTasks]=React.useState([]),[selected,setSelected]=React.useState(""),[detail,setDetail]=React.useState(null),[error,setError]=React.useState(""),[busy,setBusy]=React.useState(false),[pending,setPending]=React.useState(null),[confirmation,setConfirmation]=React.useState(null),[effectCheck,setEffectCheck]=React.useState({}),[notice,setNotice]=React.useState(""),[stopping,setStopping]=React.useState(false);
      const live=React.useRef(false),generation=React.useRef(0),controller=React.useRef(null),operation=React.useRef(0),operationBusy=React.useRef(false),scope=React.useRef({sessionId,epoch:0});
      if(scope.current.sessionId!==sessionId)scope.current={sessionId,epoch:scope.current.epoch+1};
      const load=React.useCallback(async(id="")=>{
        const current=++generation.current,epoch=scope.current.epoch;
        controller.current?.abort();const abort=new AbortController();controller.current=abort;
        try {
          const value=await request(sessionId,{action:"list"},abort.signal);
          const choice=id||value.tasks[0]?.taskId||"";
          const view=choice?await request(sessionId,{action:"get",taskId:choice},abort.signal):null;
          if(live.current&&scope.current.epoch===epoch&&current===generation.current){setTasks(value.tasks);setSelected(choice);setDetail(view);}
        } catch(error) {
          if(abort.signal.aborted||!live.current||scope.current.epoch!==epoch||current!==generation.current)return;
          throw error;
        }
      },[sessionId]);
      React.useEffect(()=>{live.current=true;const epoch=scope.current.epoch;setTasks([]);setDetail(null);setSelected("");setPending(null);setConfirmation(null);setError("");setBusy(true);setStopping(false);setNotice("");setEffectCheck({});++operation.current;operationBusy.current=false;load().catch(e=>{if(live.current&&scope.current.epoch===epoch&&e.name!=="AbortError")setError(e.message);}).finally(()=>{if(live.current&&scope.current.epoch===epoch&&!operationBusy.current)setBusy(false);});return()=>{live.current=false;generation.current++;operation.current++;controller.current?.abort();};},[load]);
      const currentOperation=(epoch,token)=>live.current&&scope.current.epoch===epoch&&operation.current===token;
      const perform=async payload=>{
        if(operationBusy.current||busy||stopping)return;operationBusy.current=true;const epoch=scope.current.epoch,token=++operation.current;setBusy(true);setError("");setNotice("");setPending(payload);
        try{await request(sessionId,payload);if(currentOperation(epoch,token)){setPending(null);setConfirmation(null);await load(payload.taskId);}}
        catch(e){if(currentOperation(epoch,token)){if(e.code==="CANCELLED"){setPending(null);setNotice("验收已停止。");}else setError(e.message||String(e));}}
        finally{if(currentOperation(epoch,token)){operationBusy.current=false;setBusy(false);}}
      };
      const stopChecking=async()=>{
        if(stopping||!pending)return;const epoch=scope.current.epoch;let accepted=false;setStopping(true);setError("");
        try{await request(sessionId,{action:"stop_validation",taskId:pending.taskId,idempotencyKey:crypto.randomUUID()});accepted=true;if(live.current&&scope.current.epoch===epoch){++operation.current;setPending(null);setNotice("停止请求已处理。");await load(pending.taskId);}}
        catch(e){if(live.current&&scope.current.epoch===epoch)setError(e.message||String(e));}
        finally{if(live.current&&scope.current.epoch===epoch){setStopping(false);if(accepted){operationBusy.current=false;setBusy(false);}}}
      };
      const act=(action,extra={})=>perform({action,taskId:detail.task.taskId,revision:detail.task.revision,idempotencyKey:crypto.randomUUID(),...extra});
      const refresh=async()=>{if(busy||stopping)return;const epoch=scope.current.epoch;setBusy(true);setError("");try{await load(selected);}catch(e){if(live.current&&scope.current.epoch===epoch&&e.name!=="AbortError")setError(e.message);}finally{if(live.current&&scope.current.epoch===epoch)setBusy(false);}};
      const chooseTask=async id=>{
        if(busy||stopping||operationBusy.current)return;const epoch=scope.current.epoch;setPending(null);setConfirmation(null);setError("");setNotice("");setEffectCheck({});setDetail(null);setSelected(id);setBusy(true);
        try{await load(id);}catch(e){if(live.current&&scope.current.epoch===epoch)setError(e.message||String(e));}
        finally{if(live.current&&scope.current.epoch===epoch&&!operationBusy.current)setBusy(false);}
      };
      const task=detail?.task,terminal=task&&["completed","cancelled"].includes(task.state);
      const latestBusy=React.useRef(false);latestBusy.current=busy||stopping||operationBusy.current;
      React.useEffect(()=>{if(!editing)return;const timer=setInterval(()=>{if(document.visibilityState!=="hidden"&&!latestBusy.current)void load(selected).catch(()=>{});},5000);return()=>clearInterval(timer);},[editing,load,selected]);
      const edited=async(id,warning="")=>{setEditing(false);setHistoryOpen(false);setNotice("任务要求已保存，尚未启动执行。"+warning);setBusy(true);const epoch=scope.current.epoch;try{await load(id);}catch(e){if(live.current&&scope.current.epoch===epoch)setError("要求已保存，但读取最新任务失败："+e.message);}finally{if(live.current&&scope.current.epoch===epoch)setBusy(false);}};
      const contentChecks=task?.spec.acceptanceChecks.filter(check=>check.checker.path)||[];
      const currentResults=task?.acceptanceRefresh?.results??task?.acceptanceResults??[];
      const evidenceNeedsReview=task?.state==="completed"&&detail.blockers.length>0;
      const locked=busy||stopping;
      return h("section",{className:"dshTaskExecution","aria-label":"任务验收与恢复"},
        h("header",null,h("h2",null,"任务验收与恢复"),h("button",{disabled:locked,onClick:refresh},"刷新")),
        h("p",null,"验收记录与具体文件版本绑定；执行返回成功并不表示任务内容合格。效果未知的步骤须先核实，恢复操作不会重放命令。"),
        error&&h("p",{role:"alert"},error),notice&&h("p",{role:"status"},notice),
        pending&&error&&h("div",{className:"actions"},h("span",null,"上次操作尚未确认，可使用同一操作标识重试。"),h("button",{disabled:locked,onClick:()=>perform(pending)},"重试该操作")),
        (busy||stopping)&&h("div",{className:"actions"},h("p",{role:"status"},stopping?"正在停止验收…":"正在读取或更新任务记录…"),pending&&["validate","refresh_evidence","reconcile"].includes(pending.action)&&h("button",{disabled:stopping,onClick:stopChecking},"停止验收")),
        !tasks.length&&!busy&&h("p",null,"当前会话尚无验收契约；多步骤工作建立契约后会在这里显示。"),
        tasks.length>0&&h("label",null,"任务",h("select",{value:selected,disabled:locked,onChange:e=>chooseTask(e.target.value)},...tasks.map(row=>h("option",{key:row.taskId,value:row.taskId},`${labels[row.state]||row.state} · ${row.spec.objective}`)))),
        task&&h(React.Fragment,null,
          h("article",{"data-state":evidenceNeedsReview?"blocked":task.state},h("div",{className:"actions"},h("h3",null,task.spec.objective),h("span",{className:"badge"},evidenceNeedsReview?"历史已完成 · 当前证据需复核":labels[task.state]||task.state)),h("small",null,`任务 ${task.taskId} · 版本 ${task.revision}`),
            task.spec.constraints.length>0&&h("ul",null,...task.spec.constraints.map((constraint,index)=>h("li",{key:index},constraint))),
            h("small",null,`要求版本 ${task.requirementsRevision??1}；变更要求后须重新验收。`),
            detail.capabilities?.revise&&h("div",{className:"actions"},h("button",{disabled:locked,onClick:()=>setEditing(value=>!value)},editing?"收起编辑":task.state==="completed"?"创建后续任务":"编辑任务要求"),h("button",{disabled:locked,onClick:()=>setHistoryOpen(value=>!value)},historyOpen?"收起要求历史":"要求版本历史")),
            h("div",{className:"actions"},task.state==="completed"&&h("button",{disabled:locked,onClick:()=>act("refresh_evidence")},"刷新验收证据"),!terminal&&h("button",{disabled:locked,onClick:()=>act("validate")},"重新验收"),!terminal&&h("button",{disabled:locked,onClick:()=>setConfirmation({action:"migrate_environment"})},"切换到当前环境"),task.state==="validating"&&h("button",{disabled:locked||detail.blockers.length>0,onClick:()=>act("complete")},"核对版本并完成"),task.state==="cancelled"&&h("button",{disabled:locked,onClick:()=>act("resume")},"继续此任务"),!terminal&&h("button",{disabled:locked,onClick:()=>setConfirmation({action:"cancel"})},"取消任务")),
            confirmation?.action==="migrate_environment"&&h("div",{className:"actions"},h("span",null,"切换后保留任务要求和未知效果，旧验收证据全部失效，须重新验收；操作不会重放命令。"),h("button",{disabled:locked,onClick:()=>act("migrate_environment")},"确认切换并重新验收"),h("button",{disabled:locked,onClick:()=>setConfirmation(null)},"暂不切换")),
            confirmation?.action==="cancel"&&h("div",{className:"actions"},h("span",null,"取消后保留执行与副作用记录。已经发生的操作不会回滚。"),h("button",{disabled:locked,onClick:()=>act("cancel")},"确认取消"),h("button",{disabled:locked,onClick:()=>setConfirmation(null)},"保留任务"))),
          editing&&h(TaskRequirementsEditor,{key:sessionId+":"+task.taskId,sessionId,task,onSaved:edited,onClose:()=>setEditing(false),onReload:()=>load(task.taskId).catch(e=>setError(e.message))}),
          historyOpen&&h(RequirementsHistory,{key:sessionId+":"+task.taskId,sessionId,taskId:task.taskId}),
          detail.blockers.length>0&&h("article",null,h("h3",null,"尚未满足的完成条件"),h("ul",null,...detail.blockers.map((message,index)=>h("li",{key:index},message)))),
          task.acceptanceRefresh&&h("p",null,"以下展示当前复核结果；原完成记录和原人工确认保持不变。"),
          h("article",null,h("h3",null,"验收要求"),...task.spec.acceptanceChecks.map(check=>{
            const result=currentResults.find(result=>result.checkId===check.id);
            return h("section",{key:check.id},
              h("div",{className:"actions"},h("strong",null,check.description),h("span",{className:"badge"},result?({passed:"通过",failed:"未通过",awaiting_user:"待人工验收",unverified:"尚未验证"})[result.status]||result.status:"尚未验证")),
              check.checker.path&&h("code",null,check.checker.path),
              result?.coverage&&h("p",null,result.coverage),
              result?.failureReason&&h("p",{role:"alert"},result.failureReason),
              result?.inputIdentity&&h("small",null,`输入标识：${result.inputIdentity}`),
              check.checker.kind==="manual"&&result?.status==="awaiting_user"&&h("button",{disabled:locked||terminal,onClick:()=>setConfirmation({action:"confirm",checkId:check.id,inputIdentity:result.inputIdentity})},"核对并确认此项"),
              confirmation?.action==="confirm"&&confirmation.checkId===check.id&&h("div",{className:"actions"},
                h("span",null,"确认已检查上述输入版本，且此项要求已满足。"),
                h("button",{disabled:locked,onClick:()=>act("confirm",{checkId:check.id,inputIdentity:confirmation.inputIdentity})},"确认验收通过"),
                h("button",{disabled:locked,onClick:()=>setConfirmation(null)},"暂不确认")
              ),
              result?.evidenceRefs.length>0&&h("details",null,h("summary",null,"验收证据"),h("ul",null,...result.evidenceRefs.map(ref=>h("li",{key:ref},h("code",null,ref)))))
            );
          })),
          h("article",null,h("h3",null,"执行与恢复记录"),!task.steps.length&&h("p",null,"尚未分派执行步骤。"),...task.steps.map(step=>h("section",{key:step.id,"data-state":step.state},h("div",{className:"actions"},h("strong",null,step.tool),h("span",{className:"badge"},labels[step.state]||step.state)),h("small",null,step.executionId),step.failureReason&&h("p",null,step.failureReason),detail.recovery.find(item=>item.stepId===step.id)&&h("p",null,detail.recovery.find(item=>item.stepId===step.id).reason),["unknown","failed","running","effect_observed"].includes(step.state)&&!terminal&&h("fieldset",null,h("legend",null,"核实已有文件效果"),h("p",null,"选择能够证明该步骤效果的既定文件验收项；检查器会重新读取当前文件。外部请求或没有对应文件的效果需要单独核实。"),h("label",null,"对应验收项",h("select",{value:effectCheck[step.id]||"",disabled:locked,onChange:e=>setEffectCheck({...effectCheck,[step.id]:e.target.value})},h("option",{value:""},"选择验收项"),...contentChecks.map(check=>h("option",{key:check.id,value:check.id},check.description)))),h("button",{disabled:locked||!effectCheck[step.id],onClick:()=>act("reconcile",{stepId:step.id,checkId:effectCheck[step.id]})},"检查文件并确认步骤效果"))))),
          task.spec.expectedOutputs.length>0&&h("article",null,h("h3",null,"预期产物"),h("ul",null,...task.spec.expectedOutputs.map(path=>h("li",{key:path},h("code",null,path),h("small",null,task.outputIdentities[path]?` · ${task.outputIdentities[path]}`:" · 尚无验收版本")))))
        )
      );
    }
    function TaskExecutionAction({sessionId}) {
      const [open,setOpen]=React.useState(false),dialog=React.useRef(null),trigger=React.useRef(null);
      React.useEffect(()=>{setOpen(false);},[sessionId]);
      React.useEffect(()=>{
        if(!open)return;
        const node=dialog.current;node.showModal();
        return()=>{if(node.open)node.close();trigger.current?.focus();};
      },[open]);
      if(!sessionId)return null;
      return h(React.Fragment,null,
        h("button",{type:"button",ref:trigger,className:"dshTaskExecutionTrigger",title:"任务验收与恢复",onClick:()=>setOpen(true)},"任务验收"),
        open&&h("dialog",{ref:dialog,className:"dshTaskExecutionDialog","aria-label":"任务验收与恢复",onCancel:()=>setOpen(false),onClose:()=>setOpen(false)},
          h("button",{type:"button",className:"dshTaskExecutionClose",onClick:()=>setOpen(false),"aria-label":"关闭任务验收"},"关闭"),
          h(TaskExecutionView,{key:sessionId,sessionId})));
    }
    function apply(ctx) {
      const style=document.createElement("style");
      const editorCss=".dshTaskExecution .requirement-form,.dshTaskExecution .requirement-value{display:grid;gap:12px;min-width:0}.dshTaskExecution input:not([type=checkbox]),.dshTaskExecution textarea{box-sizing:border-box;width:100%;min-width:0;padding:8px 10px;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;background:var(--dsw-specific-input-major);color:inherit;font:inherit;line-height:1.6}.dshTaskExecution textarea{resize:vertical;min-height:76px}.dshTaskExecution input:disabled,.dshTaskExecution textarea:disabled{opacity:.75}.dshTaskExecution .requirement-row{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:10px;align-items:end}.dshTaskExecution .requirement-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:12px}.dshTaskExecution .requirement-checkbox{display:flex;align-items:center;gap:8px}.dshTaskExecution .requirement-checkbox input{width:16px;height:16px;accent-color:var(--dsw-alias-state-business-primary)}.dshTaskExecution .requirement-editor legend{padding:0 5px;font-weight:600}.dshTaskExecution .requirement-editor details>div{margin-top:10px}.dshTaskExecution .requirement-value{border-left:2px solid var(--dsw-alias-border-l2);padding-left:12px}";
      style.textContent=css+".dshTaskExecutionTrigger,.dshTaskExecutionClose{font:inherit;color:var(--dsw-alias-label-primary);background:var(--dsw-alias-bg-layer-1);border:1px solid var(--dsw-alias-border-l2);border-radius:8px;padding:6px 10px;cursor:pointer}.dshTaskExecutionDialog{box-sizing:border-box;width:min(1080px,calc(100vw - 32px));max-height:calc(100dvh - 32px);padding:12px 0;border:1px solid var(--dsw-alias-border-l2);border-radius:12px;background:var(--dsw-alias-bg-layer-1);color:var(--dsw-alias-label-primary);overflow:auto}.dshTaskExecutionDialog::backdrop{background:rgba(0,0,0,.4)}.dshTaskExecutionClose{display:block;margin:0 16px 0 auto}.dshTaskExecutionTrigger:focus-visible,.dshTaskExecutionClose:focus-visible{outline:2px solid var(--dsw-alias-state-business-primary);outline-offset:2px}";
      style.textContent+=editorCss;document.head.appendChild(style);ctx.effect(()=>()=>style.remove(),"task execution appearance");
      ctx.slots.inject("conversation.session.header.actions",()=>ctx.slots.register({name:"conversation.session.header.actions",id:"task-execution",order:12,inject:sessionId=>({sessionId})},TaskExecutionAction));
    }
    return {apply,inject:["slots"],test:{TaskExecutionView,TaskExecutionAction,TaskRequirementsEditor,RequirementsHistory,editorContract,savedContract,draftRecords,outcomeLabels:labels,request}};
  }
});
