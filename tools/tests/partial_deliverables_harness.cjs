const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const root=path.join(__dirname,'../..'),runtime=fs.readFileSync(path.join(root,'web/src/runtime-plugins/client-runtime.js'),'utf8');
const scope={};vm.runInNewContext(runtime.slice(runtime.indexOf('function conversationContextKey('),runtime.indexOf('//#region lib/types/client/sessions/pending.js'))+';this.Engine=ConversationNodeAssembler;',scope);
let plugin,definition,tail;
vm.runInNewContext(fs.readFileSync(path.join(root,'web/src/runtime-plugins/ui-deliverables.js'),'utf8'),{window:{__ModuleLoader__:{load:entry=>plugin=entry.factory(()=>({isAppendSurfaceEvent:event=>event.surfaceOp==='append'}))}}});
plugin.apply({get:()=>({isLoopback:true,generation:{}}),conversationEvents:{register:value=>definition=value},effect:fn=>fn(),locale:{register(){},bind:()=>key=>key},slots:{inject:(_,fn)=>fn(),register:value=>tail=value},provide(){}});
const entry=(seq,type,data,view)=>({event:{seq,time:seq,type,data,...type==='tool/result'?{surfaceOp:'append'}:{}},view});
const start=entry(0,'turn/start',{turn:1});
const write=entry(10,'tool/call',{turn:1,step:2,callId:'write',name:'write'},{for:'call',view:{card:'diff',locations:[{path:'report.md'}]}});
const result=(seq,id,failed=false,turn=1)=>entry(seq,'tool/result',{turn,step:2,message:{source:{callId:id},content:[{type:'tool-result',toolCallId:id,isError:failed}]}});
const engine=new scope.Engine({entries:()=>[definition],fallbackEntry:()=>undefined},{entries:()=>[]});
const paths=()=>Array.from(plugin.producedForClosing(engine.locationIndex.snapshot().turns.get(1)?.data.get('deliverables')));
for (const failed of [false, true]) {
 const native=entry(11,'tool/result',{turn:1,step:2,message:{role:'tool',toolCallId:'write',source:{kind:'tool',callId:'write'},isError:failed,content:[]}});
 engine.replaceWindow([write,native],true);engine.flush();
 assert.deepEqual(paths(),failed?[]:['report.md'],'flat native results preserve empty success and error filtering');
}
engine.replaceWindow([write,result(11,'write')],true);engine.flush();
assert.deepEqual(paths(),['report.md'],'partial history must retain the successful visible write without requiring turn/start');
engine.append(entry(12,'tool/call',{turn:1,step:2,callId:'read',name:'read'},{for:'call',view:{card:'generic',kind:'read',locations:[{path:'not-produced.md'}]}}));
engine.append(result(13,'read'));engine.append(entry(14,'tool/call',{turn:1,step:2,callId:'failed',name:'write'},{for:'call',view:{card:'diff',locations:[{path:'failed.md'}]}}));engine.append(result(15,'failed',true));engine.flush();
assert.deepEqual(paths(),['report.md'],'reads and failed writes remain excluded');
engine.prepend([start],false);engine.flush();assert.deepEqual(paths(),['report.md'],'loading the start replaces partial derivation without duplication');
const declaration=entry(20,'deliverables/presented',{turn:1,callId:'present',files:[{path:'final.pdf'}]});
engine.replaceWindow([declaration],true);engine.flush();assert.deepEqual(paths(),['final.pdf'],'explicit delivery is recoverable without a loaded start');
const data=engine.locationIndex.snapshot().turns.get(1).data.get('deliverables');
assert.equal(tail.select({turn:{data:new Map([['deliverables',data]])},seq:21}).declared,true);
engine.replaceWindow([write,result(11,'write')],true);engine.flush();
assert.equal(tail.select({turn:{data:engine.locationIndex.snapshot().turns.get(1).data},seq:12}).declared,false,'observed output does not masquerade as accepted delivery');
engine.replaceWindow([entry(30,'turn/start',{turn:2}),result(31,'write',false,2)],false);engine.flush();
assert.equal(engine.locationIndex.snapshot().turns.get(1),undefined,'evicted turns do not leak artifacts');
if(process.argv[2]){
 const history=JSON.parse(fs.readFileSync(process.argv[2],'utf8'));
 engine.replaceWindow(history.events,history.hasMore);engine.flush();assert.ok(paths().some(value=>value.endsWith('dsh-v0.1.7-alpha.1-gap-assessment.md')),'original partial window recovers the original successful output');
}
console.log('PASS partial deliverables: missing turn start, live updates, prepend, explicit delivery, failure filtering and turn isolation');
