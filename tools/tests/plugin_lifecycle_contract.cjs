const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
let pluginExports;
const rows = [{id:'provider',url:'/plugins/external/provider.js',manageable:true},{id:'dependent',url:'/plugins/external/dependent.js',manageable:true,inject:['provider']}];
const window = {location:{href:'http://localhost:1/',origin:'http://localhost:1'},__DSH_BOOT__:{availableEntries:rows},__ModuleLoader__:{load:definition=>{pluginExports=definition.factory(()=>({}));}}};
vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../../web/dist/plugins/ui-settings-plugin-inventory.js'),'utf8'),{window,URL,AbortController,setTimeout,clearTimeout});
let failPersist = false, failStart = false;
const changes=[], entries = new Map();
const create = name => {
  const entry = {options:{name,disabled:false},fiber:{state:2},_await:async()=>{},update:async patch=>{changes.push([name,patch.disabled]);entry.options.disabled=patch.disabled;entry.fiber.state=patch.disabled?4:failStart&&name==='provider'?3:2;}};
  entries.set(name,entry); return entry;
};
rows.forEach(row=>create(row.id));
let host = rows.map(row=>({entryId:row.id,moduleName:row.id,enabled:true,fiberPhase:'active'}));
const ctx = {
 modules:{loadCache:new Map(rows.map(row=>[row.id,{}]))},
 loader:{entries:()=>entries.values(),create:async({name})=>{create(name);return name},resolve:id=>entries.get(id)},
 remote:{pluginInventory:{
  list:async()=>({ok:true,value:{entries:host.map(row=>({...row})),agentPresets:[]}}),
  setEnabled:async({entryId,enabled})=>{
   if(failPersist)return{ok:false,error:{message:'disk full'}};
   host=host.map(row=>row.entryId===entryId?{...row,enabled}:row);
   return{ok:true,value:{entry:host.find(row=>row.entryId===entryId)}};
  }
 }}
};
let operation,previousHost;
const operationRequest=async input=>{
 if(input.action==='enable'||input.action==='disable'){
  if(failPersist)throw Error('disk full');previousHost=host.map(row=>({...row}));
  host=host.map(row=>row.entryId===input.spec?{...row,enabled:input.action==='enable'}:row);
  operation={operationId:'operation',phase:'awaiting-client'};
 }else if(input.action==='client-result'){
  if(operation.phase!=='awaiting-client')return {operation:{...operation}};
  if(input.ok)operation={...operation,phase:'succeeded'};
  else {host=previousHost;operation={...operation,phase:'failed',error:input.error};}
 }else if(input.action==='cancel'&&!['succeeded','failed','cancelled'].includes(operation.phase)){
  host=previousHost;operation={...operation,phase:'cancelled'};
 }
 return {operation:{...operation}};
};
(async()=>{
 assert.ok(pluginExports.inject.includes('modules'),'cold-start bundle loading declares its module service dependency');
 let deliverables,tail;
 const generation={getSnapshot:()=>({host:{canOpenPath:true}}),subscribe:()=>()=>{}};
 vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../../web/dist/plugins/ui-deliverables.js'),'utf8'),{window:{__ModuleLoader__:{load:def=>{deliverables=def.factory(()=>({}))}}}});
 deliverables.apply({get:()=>({isLoopback:true,generation}),conversationEvents:{register:()=>{}},effect:fn=>fn(),locale:{register:()=>{},bind:()=>()=>''},slots:{inject:(_,fn)=>fn(),register:entry=>{tail=entry}},provide:()=>{}});
 const tailHooks=tail.inject().hooks;
 for(const hook of Object.values(tailHooks))new WeakMap().set(hook,true);
 assert.equal(tailHooks.hostGeneration,generation,'produced files subscribe to the current connection generation');
 const controller=pluginExports.createPluginController(ctx,operationRequest);
 await controller.list();
 await controller.setEnabled(host[0],false);
 assert.equal(host[1].enabled,true,'dependency preference is retained');
 assert.deepEqual(changes,[['dependent',true],['provider',true]],'dependents stop before provider');
 let snapshot=await controller.list();assert.equal(snapshot.entries[1].fiberPhase,null);
 await controller.setEnabled(host[0],true);
 assert.deepEqual(changes.slice(-2),[['provider',false],['dependent',false]],'provider starts before dependent');
 failPersist=true;await assert.rejects(controller.setEnabled(host[0],false),/disk full/);assert.equal(entries.get('provider').fiber.state,2);failPersist=false;
 await controller.setEnabled(host[0],false);failStart=true;await assert.rejects(controller.setEnabled(host[0],true),/插件未启动/);assert.equal(host[0].enabled,false,'failed start rolls back durable preference');assert.equal(entries.get('provider').options.disabled,true);
 failStart=false;entries.clear();await controller.setEnabled(host[0],true);assert.equal(entries.get('dependent').fiber.state,2,'disabled-at-boot plugins can start without page reload');
 await controller.setEnabled(host[0],false);
 let release,entered;const enteredPromise=new Promise(resolve=>entered=resolve),late=new Promise(resolve=>release=resolve);
 const provider=entries.get('provider'),previousUpdate=provider.update;
 provider.update=async patch=>{if(!patch.disabled){provider.options.disabled=false;provider.fiber.state=1;entered();await late;provider.fiber.state=2;}else await previousUpdate(patch);};
 const opening=controller.setEnabled(host[0],true);const rejected=assert.rejects(opening,error=>error.name==='AbortError');await enteredPromise;await controller.cancel();await rejected;
 assert.equal(host[0].enabled,false);release();await new Promise(resolve=>setTimeout(resolve,100));assert.equal(provider.fiber.state,4,'late activation is disposed after cancellation');
 provider.update=previousUpdate;
 const bad={id:'broken',url:'/plugins/external/broken.js',manageable:true};rows.push(bad);window.__DSH_BOOT__.availableEntries=rows;
 const broken=create('broken');broken.fiber.state=3;broken._await=async()=>{throw Error('unrelated broken plugin');};
 host.push({entryId:'broken',moduleName:'broken',enabled:true,fiberPhase:'active'});
 const isolated=pluginExports.createPluginController(ctx,operationRequest);
 const listed=await isolated.list();assert.equal(listed.entries.find(row=>row.moduleName==='broken').fiberPhase,'failed');
 await isolated.setEnabled(host[0],true);assert.equal(host[0].enabled,true,'an unrelated failed plugin cannot prevent enablement');
 console.log('PASS plugin lifecycle: dependency order, persistence failure, start rollback, cold enablement, cancellation, late completion and unrelated failure isolation');
})().catch(error=>{console.error(error);process.exitCode=1});
