window.__ModuleLoader__.load({
  id: "@deepseek-ai/dsh-client-ui-task-execution",
  factory: require => {
    const React = require("react"), h = React.createElement;
    const labels = {
      planned:"待执行",running:"执行中",validating:"验收中",validation_failed:"验收未通过",awaiting_user:"待人工验收",completed:"已验收完成",cancelled:"已取消",blocked:"待恢复核实",
      prepared:"已保存执行意图",dispatched:"已分派",effect_observed:"已观察到效果",verified:"执行结果已核实",committed:"已交付",failed:"执行失败",unknown:"效果未知",
      passed:"通过",unverified:"尚未验证",not_dispatched:"请求操作未启动"
    };
    const css = ".dshTaskExecution{max-width:1080px;margin:auto;padding:20px 24px;display:grid;gap:14px;color:var(--dsw-alias-label-primary);font-size:14px;line-height:1.6}.dshTaskExecution h2,.dshTaskExecution h3,.dshTaskExecution p{margin:0}.dshTaskExecution h2{font-size:18px}.dshTaskExecution h3{font-size:15px}.dshTaskExecution header,.dshTaskExecution .actions{display:flex;gap:10px;align-items:center;flex-wrap:wrap}.dshTaskExecution header{justify-content:space-between}.dshTaskExecution article,.dshTaskExecution fieldset{border:1px solid var(--dsw-alias-border-l2);border-radius:10px;padding:14px;display:grid;gap:10px;min-width:0}.dshTaskExecution button,.dshTaskExecution select{font:inherit;background:var(--dsw-alias-bg-layer-1);color:inherit;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;padding:7px 12px;max-width:100%}.dshTaskExecution button{cursor:pointer}.dshTaskExecution button:disabled{opacity:.45;cursor:default}.dshTaskExecution :focus-visible{outline:2px solid var(--dsw-alias-state-business-primary);outline-offset:2px}.dshTaskExecution small{color:var(--dsw-alias-label-tertiary)}.dshTaskExecution code,.dshTaskExecution small,.dshTaskExecution p{overflow-wrap:anywhere}.dshTaskExecution [role=alert]{color:var(--dsw-alias-state-error-primary)}.dshTaskExecution [data-state=unknown],.dshTaskExecution [data-state=blocked],.dshTaskExecution [data-state=awaiting_user]{border-color:var(--dsw-alias-state-warning-primary,#b88929)}.dshTaskExecution ul{padding-left:22px;margin:0}.dshTaskExecution pre{white-space:pre-wrap;overflow-wrap:anywhere;max-height:220px;overflow:auto}.dshTaskExecution label{display:grid;gap:5px}.dshTaskExecution details summary{cursor:pointer}.dshTaskExecution .badge{font-size:12px;padding:2px 7px;border-radius:5px;background:var(--dsw-alias-bg-layer-2)}@media(max-width:640px){.dshTaskExecution{padding:14px 12px}}";
    async function request(sessionId, payload, signal) {
      const response = await fetch("/__dsh-task-execution", {method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({sessionId,...payload}),signal});
      const value = await response.json();
      if (!response.ok) throw new Error(value.error || `HTTP ${response.status}`);
      return value;
    }
    function TaskExecutionView({sessionId}) {
      const [tasks,setTasks]=React.useState([]),[selected,setSelected]=React.useState(""),[detail,setDetail]=React.useState(null),[error,setError]=React.useState(""),[busy,setBusy]=React.useState(false),[pending,setPending]=React.useState(null),[confirmation,setConfirmation]=React.useState(null),[effectCheck,setEffectCheck]=React.useState({});
      const live=React.useRef(false),generation=React.useRef(0),controller=React.useRef(null);
      const load=React.useCallback(async(id="")=>{
        const current=++generation.current;
        controller.current?.abort();const abort=new AbortController();controller.current=abort;
        const value=await request(sessionId,{action:"list"},abort.signal);
        const choice=id||value.tasks[0]?.taskId||"";
        const view=choice?await request(sessionId,{action:"get",taskId:choice},abort.signal):null;
        if(live.current&&current===generation.current){setTasks(value.tasks);setSelected(choice);setDetail(view);}
      },[sessionId]);
      React.useEffect(()=>{live.current=true;setTasks([]);setDetail(null);setSelected("");setPending(null);setConfirmation(null);setError("");setBusy(false);setEffectCheck({});load().catch(e=>{if(live.current&&e.name!=="AbortError")setError(e.message);});return()=>{live.current=false;generation.current++;controller.current?.abort();};},[load]);
      const perform=async payload=>{
        if(busy)return;const current=generation.current;setBusy(true);setError("");setPending(payload);
        try{await request(sessionId,payload);if(live.current&&current===generation.current){setPending(null);setConfirmation(null);await load(payload.taskId);}}
        catch(e){if(live.current&&current===generation.current)setError(e.message||String(e));}
        finally{if(live.current)setBusy(false);}
      };
      const act=(action,extra={})=>perform({action,taskId:detail.task.taskId,revision:detail.task.revision,idempotencyKey:crypto.randomUUID(),...extra});
      const refresh=async()=>{if(busy)return;setBusy(true);setError("");try{await load(selected);}catch(e){if(live.current&&e.name!=="AbortError")setError(e.message);}finally{if(live.current)setBusy(false);}};
      const task=detail?.task,terminal=task&&["completed","cancelled"].includes(task.state);
      const contentChecks=task?.spec.acceptanceChecks.filter(check=>check.checker.path)||[];
      return h("section",{className:"dshTaskExecution","aria-label":"任务验收与恢复"},
        h("header",null,h("h2",null,"任务验收与恢复"),h("button",{disabled:busy,onClick:refresh},"刷新")),
        h("p",null,"验收记录与具体文件版本绑定；执行返回成功并不表示任务内容合格。效果未知的步骤须先核实，恢复操作不会重放命令。"),
        error&&h("p",{role:"alert"},error),
        pending&&error&&h("div",{className:"actions"},h("span",null,"上次操作尚未确认，可使用同一操作标识重试。"),h("button",{disabled:busy,onClick:()=>perform(pending)},"重试该操作")),
        busy&&h("p",{role:"status"},"正在读取或更新任务记录…"),
        !tasks.length&&!busy&&h("p",null,"当前会话尚无验收契约；多步骤工作建立契约后会在这里显示。"),
        tasks.length>0&&h("label",null,"任务",h("select",{value:selected,disabled:busy,onChange:e=>{setPending(null);setConfirmation(null);load(e.target.value).catch(e=>setError(e.message));}},...tasks.map(row=>h("option",{key:row.taskId,value:row.taskId},`${labels[row.state]||row.state} · ${row.spec.objective}`)))),
        task&&h(React.Fragment,null,
          h("article",{"data-state":task.state},h("div",{className:"actions"},h("h3",null,task.spec.objective),h("span",{className:"badge"},labels[task.state]||task.state)),h("small",null,`任务 ${task.taskId} · 版本 ${task.revision}`),
            task.spec.constraints.length>0&&h("ul",null,...task.spec.constraints.map((constraint,index)=>h("li",{key:index},constraint))),
            h("small",null,"目标、约束及验收要求在此任务内保持固定。"),
            h("div",{className:"actions"},!terminal&&h("button",{disabled:busy,onClick:()=>act("validate")},"重新验收"),!terminal&&h("button",{disabled:busy,onClick:()=>setConfirmation({action:"migrate_environment"})},"切换到当前环境"),task.state==="validating"&&h("button",{disabled:busy||detail.blockers.length>0,onClick:()=>act("complete")},"核对版本并完成"),task.state==="cancelled"&&h("button",{disabled:busy,onClick:()=>act("resume")},"继续此任务"),!terminal&&h("button",{disabled:busy,onClick:()=>setConfirmation({action:"cancel"})},"取消任务")),
            confirmation?.action==="migrate_environment"&&h("div",{className:"actions"},h("span",null,"切换后保留任务要求和未知效果，旧验收证据全部失效，须重新验收；操作不会重放命令。"),h("button",{disabled:busy,onClick:()=>act("migrate_environment")},"确认切换并重新验收"),h("button",{disabled:busy,onClick:()=>setConfirmation(null)},"暂不切换")),
            confirmation?.action==="cancel"&&h("div",{className:"actions"},h("span",null,"取消后保留执行与副作用记录。已经发生的操作不会回滚。"),h("button",{disabled:busy,onClick:()=>act("cancel")},"确认取消"),h("button",{disabled:busy,onClick:()=>setConfirmation(null)},"保留任务"))),
          detail.blockers.length>0&&h("article",null,h("h3",null,"尚未满足的完成条件"),h("ul",null,...detail.blockers.map((message,index)=>h("li",{key:index},message)))),
          h("article",null,h("h3",null,"验收要求"),...task.spec.acceptanceChecks.map(check=>{
            const result=task.acceptanceResults.find(result=>result.checkId===check.id);
            return h("section",{key:check.id},
              h("div",{className:"actions"},h("strong",null,check.description),h("span",{className:"badge"},result?labels[result.status]||result.status:"尚未验证")),
              check.checker.path&&h("code",null,check.checker.path),
              result?.coverage&&h("p",null,result.coverage),
              result?.failureReason&&h("p",{role:"alert"},result.failureReason),
              result?.inputIdentity&&h("small",null,`输入标识：${result.inputIdentity}`),
              check.checker.kind==="manual"&&result?.status==="awaiting_user"&&h("button",{disabled:busy||terminal,onClick:()=>setConfirmation({action:"confirm",checkId:check.id,inputIdentity:result.inputIdentity})},"核对并确认此项"),
              confirmation?.action==="confirm"&&confirmation.checkId===check.id&&h("div",{className:"actions"},
                h("span",null,"确认已检查上述输入版本，且此项要求已满足。"),
                h("button",{disabled:busy,onClick:()=>act("confirm",{checkId:check.id,inputIdentity:confirmation.inputIdentity})},"确认验收通过"),
                h("button",{disabled:busy,onClick:()=>setConfirmation(null)},"暂不确认")
              ),
              result?.evidenceRefs.length>0&&h("details",null,h("summary",null,"验收证据"),h("ul",null,...result.evidenceRefs.map(ref=>h("li",{key:ref},h("code",null,ref)))))
            );
          })),
          h("article",null,h("h3",null,"执行与恢复记录"),!task.steps.length&&h("p",null,"尚未分派执行步骤。"),...task.steps.map(step=>h("section",{key:step.id,"data-state":step.state},h("div",{className:"actions"},h("strong",null,step.tool),h("span",{className:"badge"},labels[step.state]||step.state)),h("small",null,step.executionId),step.failureReason&&h("p",null,step.failureReason),detail.recovery.find(item=>item.stepId===step.id)&&h("p",null,detail.recovery.find(item=>item.stepId===step.id).reason),["unknown","failed","running","effect_observed"].includes(step.state)&&!terminal&&h("fieldset",null,h("legend",null,"核实已有文件效果"),h("p",null,"选择能够证明该步骤效果的既定文件验收项；检查器会重新读取当前文件。外部请求或没有对应文件的效果需要单独核实。"),h("label",null,"对应验收项",h("select",{value:effectCheck[step.id]||"",disabled:busy,onChange:e=>setEffectCheck({...effectCheck,[step.id]:e.target.value})},h("option",{value:""},"选择验收项"),...contentChecks.map(check=>h("option",{key:check.id,value:check.id},check.description)))),h("button",{disabled:busy||!effectCheck[step.id],onClick:()=>act("reconcile",{stepId:step.id,checkId:effectCheck[step.id]})},"检查文件并确认步骤效果"))))),
          task.spec.expectedOutputs.length>0&&h("article",null,h("h3",null,"预期产物"),h("ul",null,...task.spec.expectedOutputs.map(path=>h("li",{key:path},h("code",null,path),h("small",null,task.outputIdentities[path]?` · ${task.outputIdentities[path]}`:" · 尚无验收版本")))))
        )
      );
    }
    function apply(ctx) {
      const style=document.createElement("style");style.dataset.taskExecution="";style.textContent=css;document.head.appendChild(style);ctx.effect(()=>()=>style.remove(),"task execution styles");
      ctx.slots.inject("conversation.view",()=>ctx.slots.register({name:"conversation.view",id:"task-execution",order:17,label:()=>"任务验收",inject:sessionId=>({sessionId})},props=>h(TaskExecutionView,{...props,key:props.sessionId})));
    }
    return {apply,inject:["slots"],test:{TaskExecutionView,outcomeLabels:labels,request}};
  }
});
