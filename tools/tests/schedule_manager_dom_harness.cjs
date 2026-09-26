"use strict";
const assert=require("node:assert/strict"),fs=require("node:fs"),path=require("node:path"),vm=require("node:vm");
const modules=process.env.DSH_REACT_TEST_MODULES||process.argv[2];
if(!modules)throw Error("Provide the external React/JSDOM node_modules directory.");
const {JSDOM}=require(path.join(modules,"jsdom"));
const dom=new JSDOM('<!doctype html><html lang="zh"><head></head><body><main></main></body></html>',{url:"http://localhost/"});
Object.assign(global,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
Object.defineProperty(global,"navigator",{value:dom.window.navigator,configurable:true});
const React=require(path.join(modules,"react")),{createRoot}=require(path.join(modules,"react-dom/client"));
const rootDir=path.resolve(__dirname,"../.."),bundle=process.env.DSH_SCHEDULE_BUNDLE||path.join(rootDir,"release/plugins/dsh-schedule/lib/client.js");
const source=fs.readFileSync(bundle,"utf8");
let plugin;
window.__ModuleLoader__={load:definition=>{plugin=definition.factory(name=>{assert.equal(name,"react");return React;});}};
vm.runInNewContext(source,{window,document,navigator,AbortController,Intl,Date,console},{filename:bundle});
const plain=value=>JSON.parse(JSON.stringify(value));
const tick=()=>new Promise(resolve=>setImmediate(resolve));
const settle=async action=>{await React.act(async()=>{action?.();await tick();await tick();});};
function store(initial){let snapshot=initial;const listeners=new Set();return {getSnapshot:()=>snapshot,subscribe:callback=>{listeners.add(callback);return()=>listeners.delete(callback);},set(value){snapshot=value;for(const callback of [...listeners])callback();},listeners};}
const initialRecord={kind:"after",id:"old",title:"检查构建",prompt:"核查上次构建",afterSeconds:60,scheduledAt:"2099-09-27T04:30:00.123Z",sessionId:"original",status:"active",lastDelivery:{scheduledAt:"2026-01-01T00:00:00Z",deliveredAt:"2026-01-01T00:00:01Z",messageId:"receipt-1"}};
function fixture(enabled=false){
  const generation=store({host:"one"}),sessionStore=store({current:"current",byId:{original:{id:"original",title:"原会话"},current:{id:"current",title:"当前会话"},archived:{id:"archived",title:"已归档会话"},child:{id:"child",title:"子会话",parentSessionId:"current"}}}),workspaceStore=store({archivedSessionIds:["archived"]});
  const listeners=new Set(),requests=[],toggles=[];
  const fixture={records:[plain(initialRecord),{...plain(initialRecord),id:"ended",title:"已结束提醒",status:"inactive"}],enabled,config:{deliveryHistoryDays:30,deliveryHistoryRecords:200},revision:"one",requests,toggles,generation,sessionStore,workspaceStore,override:null};
  const result=value=>({ok:true,value:plain(value)});
  const connection={generation,rpc:{call:async(channel,method,payload,signal)=>{
    assert.equal(channel,"/api");requests.push({method,payload:plain(payload),signal});
    if(fixture.override){const value=fixture.override(method,payload,signal);if(value!==undefined)return value;}
    if(method==="schedule.catalog")return result(fixture.records);
    if(method==="pluginInventory.list")return result({entries:[{moduleName:"dsh-schedule",entryId:"schedule-custom",enabled:fixture.enabled}]});
    if(method==="pluginInventory.getConfig")return result({moduleName:"dsh-schedule",entryId:"schedule-custom",revision:fixture.revision,config:fixture.config});
    if(method==="pluginInventory.setConfig"){
      if(payload.expectedRevision!==fixture.revision)return {ok:false,error:{code:"plugin-config-conflict",message:"配置版本冲突",details:{}}};
      assert.ok(!Object.hasOwn(payload,"enabled"));fixture.config=plain(payload.config);fixture.revision+="x";
      return result({moduleName:"dsh-schedule",entryId:"schedule-custom",revision:fixture.revision,config:fixture.config});
    }
    if(method==="schedule.history")return result(payload.before?{id:payload.id,records:[{scheduledAt:"2025-12-01T00:00:00Z",deliveredAt:"2025-12-01T00:00:01Z",messageId:"older",prompt:"旧内容"}],earlierRecordsUnavailable:true,earlierRecordsPruned:true,retention:{days:30,records:200}}:{id:payload.id,records:[initialRecord.lastDelivery],nextBefore:"receipt-1",earlierRecordsUnavailable:false,earlierRecordsPruned:false,retention:{days:30,records:200}});
    if(method==="schedule.create"){
      const kinds=["after_seconds","at","every_seconds","daily","weekly","cron"].filter(key=>Object.hasOwn(payload,key));assert.equal(kinds.length,1);
      const kind={after_seconds:"after",every_seconds:"every"}[kinds[0]]||kinds[0],input=payload[kinds[0]];
      if(["at","daily","weekly"].includes(kind))assert.match(input.time,/^\d{2}:\d{2}:\d{2}(?:\.\d{1,3})?$/,"Host requires seconds even when a browser time input reports HH:mm");
      const record={kind,id:`created-${fixture.records.length}`,title:payload.title,prompt:payload.prompt,scheduledAt:"2099-10-01T00:00:00.000Z"};
      if(kind==="after")record.afterSeconds=input;
      if(kind==="every")record.everySeconds=input;
      if(["daily","weekly"].includes(kind))record.time=input.time;
      if(["daily","weekly","cron"].includes(kind))record.timeZone=input.time_zone;
      if(kind==="weekly")record.weekdays=input.weekdays;
      if(kind==="cron")record.expression=input.expression;
      fixture.records.push({...record,sessionId:payload.sessionId,status:"active"});return result(record);
    }
    if(method==="schedule.update"){
      const row=fixture.records.find(row=>row.id===payload.id&&row.sessionId===payload.sessionId);
      if(JSON.stringify(plain(plugin.recordOf(row)))!==JSON.stringify(payload.expected))return result({id:payload.id,updated:false,code:"schedule_conflict"});
      const record={...plain(plugin.recordOf(row)),...Object.fromEntries(["title","prompt"].filter(key=>Object.hasOwn(payload,key)).map(key=>[key,payload[key]]))};
      assert.ok(!payload.expected.sessionId&&!payload.expected.status&&!payload.expected.lastDelivery);
      if(payload.change){assert.notEqual(payload.change.kind,"after");record.kind=payload.change.kind;delete record.afterSeconds;}
      Object.assign(row,record);return result({id:row.id,updated:true,record});
    }
    if(method==="schedule.delete"){fixture.records=fixture.records.filter(row=>row.id!==payload.id);return result({id:payload.id,deleted:true});}
    if(method==="schedule.retry")return result({requested:true,enabled:fixture.enabled});
    throw Error(`Unexpected RPC ${method}`);
  }}};
  fixture.services={connection,sessionStore,workspaceStore,subscribeChanged:callback=>{listeners.add(callback);return()=>listeners.delete(callback);},pluginList:async signal=>(await connection.rpc.call("/api","pluginInventory.list",{},signal)).value,setPluginEnabled:async(entry,value)=>{toggles.push({entry,value});fixture.enabled=value;},openSession:id=>{fixture.opened=id;}};
  fixture.changed=event=>{for(const callback of [...listeners])callback(event||{});};
  fixture.listeners=listeners;
  return fixture;
}
function input(label){return document.querySelector(`input[aria-label="${label}"],textarea[aria-label="${label}"],select[aria-label="${label}"]`);}
function button(label){return [...document.querySelectorAll("button")].find(element=>element.textContent===label);}
function edit(label,value){const element=input(label);assert.ok(element,`Missing field ${label}`);const prototype=element.tagName==="TEXTAREA"?window.HTMLTextAreaElement.prototype:element.tagName==="SELECT"?window.HTMLSelectElement.prototype:window.HTMLInputElement.prototype;Object.getOwnPropertyDescriptor(prototype,"value").set.call(element,value);element.dispatchEvent(new window.Event(element.tagName==="SELECT"?"change":"input",{bubbles:true}));}
const reports=[];
async function controllerTests(){
  const f=fixture(true);let state;const c=plugin.createScheduleController(f.services,value=>state=value);await c.load();assert.equal(state.records.length,2);
  c.select(state.records[0]);await tick();c.edit();c.change("title","仅改标题");await c.save();
  const titleWrite=f.requests.find(row=>row.method==="schedule.update").payload;
  assert.equal(titleWrite.expected.kind,"after");assert.equal(titleWrite.expected.scheduledAt,initialRecord.scheduledAt);assert.ok(!Object.hasOwn(titleWrite,"change"));
  assert.deepEqual(Object.keys(titleWrite.expected).sort(),["kind","id","title","prompt","afterSeconds","scheduledAt"].sort());
  c.edit();c.change("date","2099-10-12");await c.save();const moved=f.requests.filter(row=>row.method==="schedule.update").at(-1).payload;
  assert.equal(moved.change.kind,"at");assert.equal(moved.change.at.time_zone,"UTC");assert.equal(moved.change.at.time,"04:30:00.123");
  reports.push("after anchors and exact update records");
  c.edit();c.change("prompt","保留我的草稿");f.records[0].title="其他客户端标题";await c.save();assert.equal(state.draft.prompt,"保留我的草稿");assert.ok(state.conflict);
  const writes=f.requests.length;await c.save();assert.equal(f.requests.length,writes,"conflict must not auto-retry");
  await c.rebase();assert.equal(state.draft.prompt,"保留我的草稿");assert.equal(state.expected.title,"其他客户端标题");await c.save();assert.equal(state.draft,null);
  reports.push("conflict draft and explicit rebase");
  for(const kind of ["after","at","every","daily","weekly","cron"]){
    c.add();c.change("title",kind);c.change("prompt","测试提醒");c.change("kind",kind);c.change("date","2099-11-12");c.change("timeZone","Asia/Shanghai");c.change("weekdays",[7,1]);await c.save();assert.equal(state.draft,null,`${kind}: ${state.error}`);
    assert.equal(f.requests.filter(row=>row.method==="schedule.create").at(-1).payload.sessionId,"current");
  }
  const weekly=f.requests.find(row=>row.method==="schedule.create"&&row.payload.weekly).payload.weekly;assert.deepEqual(weekly.weekdays,[1,7]);
  assert.throws(()=>plugin.timing({kind:"every",seconds:"59"},key=>key),/invalidSeconds/);
  assert.throws(()=>plugin.timing({kind:"daily",time:"12:00",timeZone:"Mars/Olympus"},key=>key),/invalidZone/);
  assert.throws(()=>plugin.timing({kind:"daily",time:"12:00",timeZone:"GMT"},key=>key),/invalidZone/);
  assert.equal(plugin.timing({kind:"daily",time:"12:00",timeZone:"UTC"},key=>key).daily.time,"12:00:00");
  reports.push("six rules, original binding, interval and IANA validation");
  c.select(state.records[0]);await tick();await c.history(true);assert.equal(state.history.records.length,2);assert.equal(state.history.earlierRecordsPruned,true);assert.equal(f.requests.filter(row=>row.method==="schedule.history").at(-1).payload.before,"receipt-1");
  let release;f.override=(method)=>method==="schedule.history"?new Promise(resolve=>release=resolve):undefined;
  const pending=c.history();const request=f.requests.at(-1);f.override=null;c.select(state.records[1]);assert.ok(request.signal.aborted);release({ok:true,value:{id:"old",records:[],earlierRecordsUnavailable:false}});await pending;assert.equal(state.selected,JSON.stringify(["original","ended"]));
  f.override=null;c.dispose();assert.equal(f.listeners.size,0);reports.push("history cursor, retention, task switch cancellation");
  const g=fixture(true);let latest;const owner=plugin.createScheduleController(g.services,value=>latest=value);await owner.load();owner.add();owner.change("title","旧会话");owner.change("prompt","不跨会话");let writeRelease;
  g.override=method=>method==="schedule.create"?new Promise(resolve=>writeRelease=resolve):undefined;
  const saving=owner.save(),writeRequest=g.requests.at(-1);g.sessionStore.set({...g.sessionStore.getSnapshot(),current:"original"});assert.ok(writeRequest.signal.aborted);writeRelease({ok:true,value:plain(initialRecord)});await saving;assert.equal(latest.draft,null);assert.equal(latest.notice,"");
  let readRelease;g.override=method=>method==="schedule.catalog"?new Promise(resolve=>readRelease=resolve):undefined;const loading=owner.load();const readRequest=g.requests.findLast(row=>row.method==="schedule.catalog");g.generation.set({host:"two"});assert.ok(readRequest.signal.aborted);readRelease({ok:true,value:[]});await loading;assert.notEqual(latest.records.length,0);owner.dispose();
  reports.push("session and Host generation cancellation");
}
async function domTests(){
  const f=fixture(false);const root=createRoot(document.querySelector("main"));
  await settle(()=>root.render(React.createElement(React.StrictMode,null,React.createElement(plugin.ScheduleManager,{services:f.services,lang:"zh"}))));
  assert.ok(button("新建提醒").disabled);assert.equal(f.toggles.length,0);assert.ok(button("启用提醒"));
  await settle(()=>document.querySelector(".dshScheduleList button").click());assert.match(document.body.textContent,/不代表模型执行成功/);
  await settle(()=>button("更早记录").click());assert.match(document.body.textContent,/更早的记录已按保留策略清理/);
  await settle(()=>edit("状态","inactive"));assert.equal(document.querySelectorAll(".dshScheduleList li").length,1);
  await settle(()=>edit("搜索标题、内容或原会话","找不到"));assert.equal(document.querySelectorAll(".dshScheduleList li").length,0);
  await settle(()=>edit("搜索标题、内容或原会话",""));await settle(()=>edit("状态","all"));
  assert.equal(input("保留天数").value,"30");await settle(()=>edit("保留天数","3651"));await settle(()=>button("保存保留设置").click());assert.match(document.body.textContent,/3650/);assert.equal(f.requests.filter(row=>row.method==="pluginInventory.setConfig").length,0);
  await settle(()=>edit("保留天数","31"));f.revision="other";await settle(()=>button("保存保留设置").click());assert.equal(input("保留天数").value,"31");assert.match(document.body.textContent,/配置版本冲突/);
  await settle(()=>button("重新读取配置（保留草稿）").click());assert.equal(input("保留天数").value,"31");await settle(()=>button("保存保留设置").click());assert.equal(f.config.deliveryHistoryDays,31);assert.equal(f.config.deliveryHistoryRecords,200);assert.equal(f.enabled,false);assert.equal(f.toggles.length,0);
  reports.push("disabled read surface, filters, receipt wording, independent config CAS");
  await settle(()=>button("删除提醒").click());assert.ok(document.querySelector('[role="alertdialog"]'));assert.equal(f.requests.filter(row=>row.method==="schedule.delete").length,0);
  await settle(()=>button("取消").click());assert.equal(f.requests.filter(row=>row.method==="schedule.delete").length,0);
  await settle(()=>button("删除提醒").click());await settle(()=>button("确认删除").click());assert.equal(f.requests.filter(row=>row.method==="schedule.delete").length,1);assert.equal(f.toggles.length,0);
  await settle(()=>button("启用提醒").click());assert.equal(f.toggles.length,1);await settle(()=>button("新建提醒").click());
  assert.equal(input("原会话").value,"current");assert.ok(![...input("原会话").options].some(option=>["archived","child"].includes(option.value)));
  await settle(()=>edit("标题","界面提醒"));await settle(()=>edit("发送给会话的内容","核查完成情况"));await settle(()=>edit("时间规则","weekly"));assert.ok(input("时区（IANA）"));assert.equal(document.querySelectorAll('.dshScheduleWeek input').length,7);
  await settle(()=>button("保存").click());assert.equal(f.requests.filter(row=>row.method==="schedule.create").at(-1).payload.weekly.time_zone,Intl.DateTimeFormat().resolvedOptions().timeZone);
  await settle(()=>button("打开原会话").click());assert.equal(f.opened,"current");
  await settle(()=>{f.records.push({...plain(initialRecord),id:"external",title:"另一客户端提醒"});f.changed({enabled:true});});assert.match(document.body.textContent,/另一客户端提醒/);
  reports.push("explicit enable, confirmed disabled deletion, calendar form and changed refresh");
  let release;f.override=method=>method==="pluginInventory.getConfig"?new Promise(resolve=>release=resolve):undefined;await settle(()=>button("重新读取配置（保留草稿）").click());const request=f.requests.at(-1);
  await settle(()=>root.unmount());assert.ok(request.signal.aborted);release({ok:true,value:{entryId:"schedule-custom",revision:"late",config:{}}});await settle();assert.equal(f.listeners.size,0);assert.equal(f.generation.listeners.size,0);
  reports.push("StrictMode and unmount cleanup");
}
function schemaTests(){
  for(const file of ["connection.js","client-runtime.js"]){
    const raw=fs.readFileSync(path.join(rootDir,"web/src/runtime-plugins",file),"utf8"),begin=raw.indexOf("//#region ../../../node_modules/.pnpm/zod"),end=raw.indexOf("/**\n\t\t* Business success/failure",begin);
    const normalized=raw.replace(/\r\n/g,"\n"),schemaEnd=normalized.indexOf("/**\n\t\t* Business success/failure",normalized.indexOf("const rpcErrorSchema"));
    const scope={};vm.runInNewContext(normalized.slice(normalized.indexOf("//#region ../../../node_modules/.pnpm/zod"),schemaEnd)+"this.schema=rpcErrorSchema;this.object=object;this.string=string;this.literal=literal;if(typeof boolean!==\"undefined\")this.boolean=boolean;",scope);
    assert.ok(scope.schema.safeParse({code:"schedule-rejected",message:"Disabled",details:{reason:"schedule_disabled"}}).success);
    for(const details of [{}, {reason:2}, {reason:"disabled",extra:true}])assert.equal(scope.schema.safeParse({code:"schedule-rejected",message:"x",details}).success,false);
    if(file==="connection.js"){
      const start=normalized.indexOf('object({\n\t\t\t\ttype: literal("host/schedule-changed")'),stop=normalized.indexOf("}).strict()",start)+"}).strict()".length;
      assert.ok(start>=0);vm.runInNewContext("this.frame="+normalized.slice(start,stop),scope);
      for(const value of [{type:"host/schedule-changed"},{type:"host/schedule-changed",enabled:false}])assert.ok(scope.frame.safeParse(value).success);
      for(const value of [{type:"host/schedule-changed",enabled:"yes"},{type:"host/schedule-changed",enabled:null},{type:"host/schedule-changed",extra:1}])assert.equal(scope.frame.safeParse(value).success,false);
    }
  }
  reports.push("actual transport and runtime strict error/frame schemas");
}
async function archiveAndLoaderTests(){
  const workspace=fs.readFileSync(path.join(rootDir,"web/src/runtime-plugins/ui-workspace.js"),"utf8"),scope={AbortController,react:React,_deepseek_ai_dsh_client_ui_primitives:{Modal:({title,footer,children})=>React.createElement("div",{role:"alertdialog"},title,children,footer)}};
  vm.runInNewContext(workspace.slice(workspace.indexOf("function createArchiveController("),workspace.indexOf("function WorkspaceBrowser("))+"this.create=createArchiveController;this.Dialog=ArchiveReminderDialog;",scope);
  const generation=store("a"),calls=[];let state;
  const c=scope.create({generation,publish:value=>state=value,archiveSession:async(id,stop,signal)=>{calls.push({id,stop,signal});if(!stop)throw {code:"agent-busy",details:{reason:"active-schedules"}};},unarchiveSession:async()=>{}});
  c.request("old",false);await tick();assert.equal(calls.length,1);assert.equal(calls[0].stop,false);assert.equal(state.target.sessionId,"old");
  const root=createRoot(document.querySelector("main")),t=key=>({"archive.stopSchedulesTitle":"停止提醒并归档？","archive.stopSchedulesHint":"停止此会话待发送提醒","archive.stopSchedulesConfirm":"停止提醒并归档",cancel:"取消"}[key]||key);
  await settle(()=>root.render(React.createElement(scope.Dialog,{state,controller:c,t})));assert.match(document.querySelector('[role="alertdialog"]').textContent,/停止此会话/);
  await settle(()=>button("取消").click());assert.equal(calls.length,1);assert.equal(state.target,null);
  c.request("old",false);await tick();await settle(()=>root.render(React.createElement(scope.Dialog,{state,controller:c,t})));await settle(()=>button("停止提醒并归档").click());assert.equal(calls.at(-1).stop,true);assert.equal(calls.at(-1).id,"old");await settle(()=>root.unmount());
  c.request("old",false);await tick();generation.set("b");await c.confirm();assert.equal(calls.at(-1).stop,false);assert.equal(state.target,null);c.dispose();assert.equal(generation.listeners.size,0);
  const inventory=fs.readFileSync(path.join(rootDir,"web/src/runtime-plugins/ui-settings-plugin-inventory.js"),"utf8");
  const loaderScope={window,URL};vm.runInNewContext(inventory.slice(inventory.indexOf("function createScheduleManagerLoader("),inventory.indexOf("/** Services required",inventory.indexOf("function createScheduleManagerLoader(")))+"this.create=createScheduleManagerLoader;",loaderScope);
  window.__DSH_BOOT__={entries:[],availableEntries:[{id:"dsh-schedule",url:"/plugins/external/dsh-schedule/client.js"}]};let arrivals=0,imports=0,serviceCreations=0;
  const loader=loaderScope.create({modules:{arrive:async()=>{arrivals++;},import:async()=>{imports++;return {ScheduleManager:plugin.ScheduleManager,servicesFor:()=>{serviceCreations++;return {};}};}}},{});
  const abort=new AbortController();const loaded=await loader(abort.signal);assert.equal(loaded.Component,plugin.ScheduleManager);assert.equal(arrivals,1);assert.equal(imports,1);assert.equal(serviceCreations,1);
  abort.abort();await assert.rejects(loader(abort.signal),error=>error.name==="AbortError");assert.equal(imports,1);
  reports.push("explicit archive-stop confirmation and display-only disabled-module loading");
}
async function actualPluginControllerTests(){
  const inventory=fs.readFileSync(path.join(rootDir,"web/src/runtime-plugins/ui-settings-plugin-inventory.js"),"utf8");
  const scope={window,AbortController,setTimeout,clearTimeout};
  vm.runInNewContext(inventory.slice(inventory.indexOf("function createPluginController("),inventory.indexOf("function LazyScheduleManager("))+"this.create=createPluginController;",scope);
  for(const canonical of ["dsh-schedule","dsh-time-context","dsh-auto-review"]){
    const alias=`@deepseek-ai/${canonical}`,customId=`custom-${canonical}`,dependent="dependent-client";
    const catalog=[{id:canonical,url:`/plugins/external/${canonical}.js`,manageable:true},{id:dependent,url:"/plugins/external/dependent.js",manageable:true,inject:[alias]}];
    window.__DSH_BOOT__={entries:[],availableEntries:catalog};
    let host=[{entryId:customId,moduleName:alias,enabled:false,fiberPhase:null},{entryId:dependent,moduleName:dependent,enabled:true,fiberPhase:null}],operation;
    const requests=[],clients=new Map(),clientChanges=[];
    const ctx={modules:{loadCache:new Map(catalog.map(row=>[row.id,{}]))},remote:{pluginInventory:{list:async()=>({ok:true,value:{entries:plain(host),agentPresets:[]}})}},loader:{
      entries:()=>clients.values(),resolve:id=>clients.get(id),create:async({name})=>{
        assert.ok(catalog.some(row=>row.id===name),"browser Loader must receive a canonical bundle id");
        const entry={options:{name,disabled:false},fiber:{state:2},_await:async()=>{},update:async patch=>{clientChanges.push([name,patch.disabled]);entry.options.disabled=patch.disabled;entry.fiber.state=patch.disabled?4:2;}};
        clientChanges.push([name,false]);clients.set(name,entry);return name;
      }
    }};
    const request=async input=>{
      requests.push(plain(input));
      if(input.action==="enable"||input.action==="disable"){
        assert.equal(input.spec,customId,"Host operation must retain the user's entry id");
        host=host.map(row=>row.entryId===customId?{...row,enabled:input.action==="enable"}:row);
        operation={operationId:`op-${requests.length}`,phase:"awaiting-client"};
      }else if(input.action==="client-result"){assert.equal(input.ok,true);operation={...operation,phase:"succeeded",result:{}};}
      else assert.equal(input.action,"status");
      return {operation:plain(operation)};
    };
    const controller=scope.create(ctx,request),original=plain(host[0]);
    assert.equal(controller.canToggle(original),true);assert.equal(controller.canToggle({moduleName:"@other/schedule"}),false);
    let snapshot=await controller.list();await tick();assert.equal(clients.size,0,"listing a disabled alias must not create either browser plugin");assert.equal(snapshot.entries[0].moduleName,alias);
    await controller.setEnabled(original,true);snapshot=await controller.list();await tick();
    assert.equal(snapshot.entries[0].moduleName,alias);assert.equal(snapshot.entries[0].entryId,customId);assert.equal(snapshot.entries[0].fiberPhase,"active");
    assert.deepEqual([...clients.keys()],[canonical,dependent]);assert.equal(host[0].moduleName,alias);assert.deepEqual(original,{entryId:customId,moduleName:alias,enabled:false,fiberPhase:null});
    // A retained browser entry using the supported scoped name must be found as the same module.
    clients.get(canonical).options.name=alias;
    await controller.setEnabled(snapshot.entries[0],false);snapshot=await controller.list();await tick();
    assert.equal(snapshot.entries[0].fiberPhase,null);assert.equal(clients.get(canonical).options.disabled,true);assert.equal(clients.get(dependent).options.disabled,true);
    assert.ok(clientChanges.some(([name,disabled])=>name===dependent&&disabled));
    assert.equal(requests.filter(row=>row.action==="enable"||row.action==="disable").length,2);
    const before=requests.length;await assert.rejects(controller.setEnabled({entryId:"unknown",moduleName:"@other/schedule"},true));assert.equal(requests.length,before);
  }
  reports.push("actual plugin controller: custom entry ids, three scoped aliases, dependency enable/disable and preserved Host identities");
}
const watchdog=setTimeout(()=>{console.error("FAIL schedule harness did not finish: unresolved request");process.exitCode=1;},10000);
(async()=>{schemaTests();await controllerTests();await domTests();await archiveAndLoaderTests();await actualPluginControllerTests();for(const report of reports)console.log(`PASS ${report}`);})().catch(error=>{console.error(error);process.exitCode=1;}).finally(()=>clearTimeout(watchdog));
