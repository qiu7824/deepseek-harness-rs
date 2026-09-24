const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.argv[2]||process.env.DSH_REACT_TEST_MODULES;
const {JSDOM}=require(path.join(modules,'jsdom'));
const dom=new JSDOM('<main id="root"></main>',{pretendToBeVisual:true,url:'http://task-editor.test'});
Object.assign(global,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true});
const React=require(path.join(modules,'react')),root=require(path.join(modules,'react-dom/client')).createRoot(document.getElementById('root'));
const source=fs.readFileSync(path.join(__dirname,'../../web/src/runtime-plugins/ui-task-execution.js'),'utf8');
const copy=value=>JSON.parse(JSON.stringify(value)),requests=[],pending=[],saved=[],reloads=[];
const spec={objective:'Complete report',goalId:'goal-1',constraints:['Keep original files'],expectedOutputs:['report.docx'],environmentFingerprint:'host-owned',validationSubject:{kind:'document',identity:'report-v1',expectedOutcome:'Reviewed pages'},acceptanceChecks:[
 {id:'manual',description:'Visual review',checker:{kind:'manual',reason:'Inspect every page'}},
 {id:'text',description:'Required content',checker:{kind:'text',path:'report.txt',required:['Overview'],forbidden:['PLACEHOLDER']}},
 {id:'json',description:'Data',checker:{kind:'json',path:'report.json',assertions:{'/summary':{complete:true,items:[1,'two',null]}}}},
 {id:'image',description:'Rendered page',checker:{kind:'image',path:'page.png',min_width:1200,min_height:1600,channels:4}},
 {id:'office',description:'Office structure',checker:{kind:'office_package',path:'report.docx',format:'docx'}},
 {id:'result',description:'Tool assertion',checker:{kind:'tool_result',step_id:'tool:build',assertions:{'/ok':true}}}
]};
let task={taskId:'task-a',revision:7,requirementsRevision:1,state:'planned',spec:copy(spec),goalBinding:{goalId:'goal-1',objectiveRevision:1},goalBindingStatus:'current',steps:[],acceptanceResults:[],outputIdentities:{}};
let currentGoalRequirements={goalId:'goal-1',objectiveRevision:1,objective:'Complete report'};
const response=(value,code)=>({ok:!code,status:code?409:200,text:async()=>JSON.stringify(code?{error:value,code}:value)});
function plugin(){let exported;window.__ModuleLoader__={load:value=>{exported=value.factory(()=>React);}};vm.runInNewContext(source,{window,document,AbortController,TextEncoder,crypto:require('node:crypto').webcrypto,setInterval,clearInterval,
 fetch:async(_,options)=>{const input=JSON.parse(options.body);requests.push(input);if(input.action==='revise')return new Promise((resolve,reject)=>pending.push({input,resolve,reject}));if(input.action==='requirements_history')return response({history:[{idempotencyKey:'old-key',sourceRevision:4,targetRevision:5,targetTaskId:'task-a'}]});if(input.action==='requirements_snapshot')return response({snapshot:{...task,revision:4,spec:{...spec,objective:'Historical goal'}}});throw Error('Unexpected request '+input.action);}});return exported.test;}
let api=plugin();
const act=fn=>React.act(async()=>{fn?.();await new Promise(resolve=>setImmediate(resolve));});
const render=props=>act(()=>root.render(React.createElement(api.TaskRequirementsEditor,{sessionId:'session-a',task,currentGoalRequirements,onSaved:id=>saved.push(id),onClose:()=>{},onReload:()=>reloads.push(true),...props})));
const button=text=>{const item=[...document.querySelectorAll('button')].find(node=>node.textContent===text);assert.ok(item,'button '+text);return item;};
const input=label=>{const item=[...document.querySelectorAll('label')].find(node=>node.querySelector('span')?.textContent===label);assert.ok(item,'field '+label);return item.querySelector('input,textarea');};
const edit=(label,value)=>act(()=>{const node=input(label),prototype=node.tagName==='TEXTAREA'?window.HTMLTextAreaElement.prototype:window.HTMLInputElement.prototype;Object.getOwnPropertyDescriptor(prototype,'value').set.call(node,value);node.dispatchEvent(new window.Event('input',{bubbles:true}));});
const click=text=>act(()=>button(text).click());
(async()=>{
 const expected=copy(spec);delete expected.environmentFingerprint;
 assert.deepEqual(copy(api.savedContract(api.editorContract(spec))),expected,'all six checker kinds and nested JSON survive structured editing');
 await render();await edit('任务目标','Revised report');
 assert.equal(api.draftRecords('session-a','task-a').length,1,'draft persisted before any save');
 await act(()=>root.render(null));api=plugin();await render();
 assert.equal(input('任务目标').value,'Revised report','fresh plugin restores unsaved draft');
 await act(()=>{button('保存任务要求').click();button('保存任务要求').click();});
 assert.equal(pending.length,1,'double click starts one mutation');
 assert.equal(pending[0].input.expectedRevision,7);assert.equal(pending[0].input.contract.objective,'Revised report');
 assert.equal('environmentFingerprint' in pending[0].input.contract,false,'Host owns environment identity');
 const retryKey=pending[0].input.idempotencyKey;
 await act(()=>pending[0].reject(Error('connection lost after dispatch')));
 assert.equal(input('任务目标').disabled,true,'unknown effects keep exact pending request immutable');
 await act(()=>root.render(null));api=plugin();await render();await click('重试保存并核实');
 assert.equal(pending[1].input.idempotencyKey,retryKey,'reload preserves idempotency key');
 assert.deepEqual(pending[1].input,pending[0].input,'retry keeps exact payload');
 await act(()=>pending[1].resolve(response('Task revision conflict','TASK_REVISION_CONFLICT')));
 assert.equal(input('任务目标').value,'Revised report');assert.equal(input('任务目标').disabled,false);assert.equal(reloads.length,1);
 task={...task,revision:9,spec:{...spec,objective:'Concurrent saved goal'}};await render();
 assert.match(document.body.textContent,/版本已变化/);assert.equal(button('保存任务要求').disabled,true);
 await click('保留我的内容并使用当前版本');await click('保存任务要求');
 assert.equal(pending[2].input.expectedRevision,9);assert.notEqual(pending[2].input.idempotencyKey,retryKey);
 assert.equal(pending[2].input.contract.objective,'Revised report');
 await act(()=>pending[2].resolve(response({task:{...task,revision:10}})));
 assert.deepEqual(saved,['task-a']);
 assert.equal(api.draftRecords('session-a','task-a').some(row=>row.value.pendingSave?.idempotencyKey===retryKey),false,'all replicas retain content but stop restoring a definitively rejected submission');

 // A response belonging to a closed editor cannot update the new session.
 await act(()=>root.render(null));window.localStorage.clear();api=plugin();task={...task,state:'planned',revision:10};await render();
 await edit('任务目标','Pending old session');await click('保存任务要求');
 const old=pending[3];await act(()=>root.render(null));await render({sessionId:'session-b',task:{...task,taskId:'task-b'}});
 await edit('任务目标','New session draft');await act(()=>old.resolve(response({task:{...task,revision:11}})));
 assert.equal(input('任务目标').value,'New session draft');assert.deepEqual(saved,['task-a'],'late old response cannot select or close new editor');

 await act(()=>root.render(null));window.localStorage.clear();api=plugin();task={...task,state:'completed',revision:12};await render();await edit('任务目标','Follow-up goal');await click('保存为后续任务');
 assert.equal(pending[4].input.mode,'successor');await act(()=>pending[4].resolve(response({task:{...task,taskId:'task-successor',revision:1,state:'planned'}})));assert.equal(saved.at(-1),'task-successor');

 // Simulate a second page writer. Draft copies must not overwrite one another.
 await act(()=>root.render(null));window.localStorage.clear();api=plugin();task={...task,state:'cancelled',revision:15};await render();await edit('任务目标','Window A draft');await act(()=>root.render(null));
 const firstWriter=api;api=plugin();await render();await edit('任务目标','Window B draft');
 const records=api.draftRecords('session-a','task-a');assert.equal(records.length,2);assert.ok(records.some(row=>row.value.contract.objective==='Window A draft'));assert.ok(records.some(row=>row.value.contract.objective==='Window B draft'));
 await click('保存任务要求');assert.equal(pending[5].input.mode,'in_place','editing cancelled task does not implicitly create or resume execution');await act(()=>pending[5].resolve(response({task:{...task,revision:16}})));
 assert.equal(requests.some(row=>row.action==='resume'),false);
 const remaining=api.draftRecords('session-a','task-a');assert.ok(remaining.some(row=>row.value.contract.objective==='Window A draft'),'saving B retains different unsaved content from window A');assert.equal(remaining.some(row=>row.value.contract.objective==='Window B draft'),false,'committed B is no longer offered as an unsaved draft');

 // Repeated page reloads of one unresolved submission must not revive an old copy.
 await act(()=>root.render(null));window.localStorage.clear();api=plugin();task={...task,state:'planned',revision:20};await render();await edit('任务目标','Stable submission');await act(()=>root.render(null));api=plugin();await render();await click('保存任务要求');
 const unresolved=pending.at(-1);await act(()=>unresolved.reject(Error('unknown receipt')));await act(()=>root.render(null));api=plugin();await render();await click('重试保存并核实');assert.equal(pending.at(-1).input.idempotencyKey,unresolved.input.idempotencyKey);await act(()=>pending.at(-1).resolve(response({task:{...task,revision:21}})));assert.equal(api.draftRecords('session-a','task-a').length,0,'successful replay retires all identical A→B→C copies');

 // Storage failure preserves the latest volatile edit and cannot dispatch a non-durable operation.
 await act(()=>root.render(null));window.localStorage.clear();api=plugin();task={...task,revision:25};await render();await edit('任务目标','Persisted older text');
 const storagePrototype=Object.getPrototypeOf(window.localStorage),setItem=storagePrototype.setItem;storagePrototype.setItem=function(){throw new window.DOMException('full','QuotaExceededError');};
 await edit('任务目标','Latest volatile text');await act(()=>root.render(null));await render();assert.equal(input('任务目标').value,'Latest volatile text','volatile newest draft wins over an older durable copy');
 const countBefore=pending.length;await click('保存任务要求');assert.equal(pending.length,countBefore,'failed durable intent never reaches RPC');assert.match(document.body.textContent,/尚未发送/);
 storagePrototype.setItem=setItem;await click('重试保存并核实');await act(()=>pending.at(-1).resolve(response({task:{...task,revision:26}})));assert.equal(api.draftRecords('session-a','task-a').length,0,'superseded older version cannot reappear after volatile content is committed');
 const unsafe=api.editorContract(spec);unsafe.acceptanceChecks.find(row=>row.id==='json').checker.assertionRows[0].value={type:'number',value:'9007199254740993'};assert.throws(()=>api.savedContract(unsafe),/精确表示范围/,'integers must not silently lose precision');

 // A failed completion receipt must never destroy its durable retry identity.
 await act(()=>root.render(null));window.localStorage.clear();api=plugin();task={...task,revision:30};await render();await edit('任务目标','Receipt must persist');await click('保存任务要求');const receiptRequest=pending.at(-1);
 storagePrototype.setItem=function(){throw new window.DOMException('full','QuotaExceededError');};await act(()=>receiptRequest.resolve(response({task:{...task,revision:31}})));
 const durable=[];for(let i=0;i<window.localStorage.length;i++)durable.push(JSON.parse(window.localStorage.getItem(window.localStorage.key(i))));assert.ok(durable.some(value=>value.pendingSave?.idempotencyKey===receiptRequest.input.idempotencyKey),'durable pending intent survives receipt failure');
 await act(()=>root.render(null));storagePrototype.setItem=setItem;api=plugin();await render();await click('重试保存并核实');assert.equal(pending.at(-1).input.idempotencyKey,receiptRequest.input.idempotencyKey);await act(()=>pending.at(-1).resolve(response({task:{...task,revision:31}})));

 // One identical submission's success does not resolve another independent request.
 await act(()=>root.render(null));window.localStorage.clear();api=plugin();task={...task,revision:35};await render();await edit('任务目标','Identical content');await click('保存任务要求');const firstPending=pending.at(-1),firstRecord=api.draftRecords('session-a','task-a')[0];
 const independent={...copy(firstRecord.value),writer:'independent-writer',token:'independent-token',writerSequence:1,pendingSave:{...copy(firstPending.input),idempotencyKey:'independent-submit'}};const independentKey=firstRecord.key.slice(0,firstRecord.key.indexOf('draft:'))+'draft:independent-writer:independent-token';window.localStorage.setItem(independentKey,JSON.stringify(independent));
 await act(()=>firstPending.resolve(response({task:{...task,revision:36}})));assert.ok(api.draftRecords('session-a','task-a').some(row=>row.value.pendingSave?.idempotencyKey==='independent-submit'),'independent in-flight request remains unresolved even when content matches');

 // Adopting a changed goal must unlock a fresh revision, never the old acceptance.
 await act(()=>root.render(null));window.localStorage.clear();api=plugin();
 task={...task,revision:40,state:'completed',goalBindingStatus:'stale'};
 currentGoalRequirements={goalId:'goal-1',objectiveRevision:2,objective:'Changed report objective'};
 await render();assert.equal(button('保存为后续任务').disabled,true);
 await click('核对并采用当前目标');assert.equal(button('保存为后续任务').disabled,false,'explicit adoption must unlock save despite the old task remaining stale');
 await edit('任务目标','Changed report objective');await click('保存为后续任务');
 assert.deepEqual(pending.at(-1).input.expectedGoalBinding,{goalId:'goal-1',objectiveRevision:2});
 const beforeReload=reloads.length;
 await act(()=>pending.at(-1).resolve(response('Goal changed again','TASK_GOAL_REQUIREMENTS_CHANGED')));
 assert.equal(reloads.length,beforeReload+1,'a rejected goal binding reloads the latest goal while retaining the draft');
 currentGoalRequirements={...currentGoalRequirements,objectiveRevision:3};await render();
 assert.equal(input('任务目标').value,'Changed report objective');assert.equal(button('保存为后续任务').disabled,true);
 await click('核对并采用当前目标');await click('保存为后续任务');
 assert.equal(pending.at(-1).input.expectedGoalBinding.objectiveRevision,3);
 await act(()=>pending.at(-1).resolve(response({task:{...task,taskId:'new-goal-successor',revision:1}})));

 // Legacy linked contracts have no trusted binding and require explicit adoption.
 await act(()=>root.render(null));window.localStorage.clear();api=plugin();
 task={...task,revision:45,state:'planned',goalBinding:null,goalBindingStatus:'missing'};
 await render();assert.equal(button('保存任务要求').disabled,true);
 await click('核对并采用当前目标');assert.equal(button('保存任务要求').disabled,false);
 // Replacing the entire goal must update the requested goal as well as its binding.
 currentGoalRequirements={goalId:'goal-2',objectiveRevision:1,objective:'Replacement goal'};await render();
 assert.equal(button('保存任务要求').disabled,true);await click('核对并采用当前目标');await click('保存任务要求');
 assert.equal(pending.at(-1).input.contract.goalId,'goal-2');
 assert.deepEqual(pending.at(-1).input.expectedGoalBinding,{goalId:'goal-2',objectiveRevision:1});
 await act(()=>pending.at(-1).resolve(response({task:{...task,revision:46}})));

 await act(()=>root.render(null));window.localStorage.clear();api=plugin();
 task={...task,goalBindingStatus:'unavailable'};currentGoalRequirements=null;await render();
 assert.equal(button('保存任务要求').disabled,true);assert.equal(button('核对并采用当前目标').disabled,true);

 await act(()=>root.render(React.createElement(api.RequirementsHistory,{sessionId:'session-a',taskId:'task-a'})));await click('查看原版本 4');
 assert.equal(input('任务目标').value,'Historical goal');assert.equal(input('任务目标').disabled,true,'historical requirements are readonly');
 await act(()=>root.unmount());dom.window.close();
 console.log('PASS task requirements: six checker kinds, nested JSON, persisted drafts, cross-window copies, CAS, stable retry after reload, stale-response isolation, successor and readonly history');
})().catch(error=>{console.error(error);process.exitCode=1;dom.window.close();});
