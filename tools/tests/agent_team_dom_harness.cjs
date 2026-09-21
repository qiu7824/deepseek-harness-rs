const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.argv[2]||process.env.DSH_REACT_TEST_MODULES;
const {JSDOM}=require(path.join(modules,'jsdom'));
const dom=new JSDOM('<main id="root"></main>',{pretendToBeVisual:true,url:'http://localhost'});
Object.assign(global,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true});
const React=require(path.join(modules,'react'));
const source=fs.readFileSync(path.join(__dirname,'../../web/src/runtime-plugins/ui-subagent.js'),'utf8');
const requests=[],timers=new Map(),opened=[],writes=[];let timerId=0,settingsFail=false;
let settings={writable:true,mode:'host',revision:1,value:{enabled:true,maxMembers:8,showButton:true,defaultMode:'off',defaultProfile:'',profiles:[]}};
const listeners=new Set();const scope={getSnapshot(){return settings;},subscribe(fn){listeners.add(fn);return()=>listeners.delete(fn);},async load(){}};
const context={react:React,document,AbortController,structuredClone,crypto:require('node:crypto').webcrypto,
 fetch:(url,options)=>new Promise((resolve,reject)=>requests.push({url,options,resolve,reject,done:false})),
 setTimeout:fn=>{const id=++timerId;timers.set(id,fn);return id;},clearTimeout:id=>timers.delete(id),
 _deepseek_ai_dsh_client_ui_primitives:{Modal:({children,onClose,title})=>React.createElement('section',{role:'dialog','aria-label':title},React.createElement('button',{onClick:onClose},'close'),children)}
};
const start=source.indexOf('function TeamBoardAction('),end=source.indexOf('function apply(ctx)',start);
vm.runInNewContext(source.slice(start,end),context);
const root=require(path.join(modules,'react-dom/client')).createRoot(document.getElementById('root'));
const act=fn=>React.act(async()=>{await fn();}),t=key=>key;
const panel=context.createTeamPanel();
const board={teamId:'a',config:{revision:0,mode:'auto',profile:null},leadRunning:false,members:{worker:{id:'child-a',name:'worker',description:'Worker',phase:'active',status:'idle'}},tasks:{check:{id:'check',revision:1,subject:'Review module',description:'Verify behavior',ownerId:'child-a',status:'pending',blockedBy:[],writeScopes:['src/module'],acceptance:'Tests pass',result:''}},mailbox:[]};
const value=(id='a')=>({enabled:true,settings:settings.value,board:{...structuredClone(board),teamId:id}});
const controls=()=>requests.filter(r=>JSON.parse(r.options.body).action==='control');
const reply=(request,data,status=200)=>act(()=>{request.done=true;request.resolve({ok:status===200,status,json:async()=>data});});
const settleViews=async(data=value())=>{for(let i=0;i<4;i++){const waiting=requests.filter(r=>!r.done&&!r.options.signal?.aborted&&!JSON.parse(r.options.body).action);if(!waiting.length)return;for(const req of waiting)await reply(req,data);}};
const button=(text)=>{const node=[...document.querySelectorAll('button')].find(n=>n.textContent===text||n.getAttribute("aria-label")===text);assert.ok(node,'button '+text);return node;};
const click=text=>act(()=>button(text).click());
const field=(label,element='input')=>{const node=[...document.querySelectorAll('label')].find(n=>n.querySelector('span')?.textContent===label);assert.ok(node,label);return node.querySelector(element);};
const input=async(node,value)=>act(()=>{Object.getOwnPropertyDescriptor(node instanceof window.HTMLTextAreaElement?window.HTMLTextAreaElement.prototype:window.HTMLInputElement.prototype,'value').set.call(node,value);node.dispatchEvent(new window.Event('input',{bubbles:true}));});
const saveSettings=async(value,revision)=>{writes.push({value:structuredClone(value),revision});if(settingsFail)throw Error('revision conflict');assert.equal(revision,settings.revision);settings={...settings,revision:settings.revision+1,value:structuredClone(value)};for(const listener of listeners)listener();};
const render=id=>root.render(React.createElement(React.Fragment,null,
 React.createElement(context.TeamBoardAction,{parentSessionId:id,panel,panelOnly:true,teamSettings:scope,saveSettings,openChild:value=>opened.push(value),t}),
 React.createElement(context.TeamSidebarTrigger,{parentSessionId:id,panel,teamSettings:scope,t})));
(async()=>{
 await act(()=>render('a'));await settleViews();await click('team.title');await settleViews();
 assert.equal(document.querySelectorAll('[role=dialog]').length,1,'top-right collaboration opens the session team panel');
 assert.match(document.body.textContent,/Review module/);assert.match(document.body.textContent,/src\/module/);
 await click('team.members');await click('team.openConversation');assert.equal(document.querySelector('[role=dialog]'),null);
 assert.deepEqual(JSON.parse(JSON.stringify(opened)),[{parentSessionId:'a',childSessionId:'child-a',mode:'continuable'}]);
 await click('team.title');await settleViews();await click('team.members');
 await act(()=>document.querySelector('.dshTeamCreate>summary').click());assert.ok(document.querySelector('.dshTeamCreate').open);
 await input(field('team.memberName'),'中文成员');await input(field('team.initialTask','textarea'),'Inspect only the assigned module');
 const form=field('team.memberName').closest('form');await act(()=>form.dispatchEvent(new window.Event('submit',{bubbles:true,cancelable:true})));
 const creating=controls().at(-1),args=JSON.parse(creating.options.body).arguments;
 assert.equal(args.action,'create');assert.equal(args.description,'中文成员');assert.ok(args.requestId);assert.equal(args.name,args.requestId);
 assert.equal(form.querySelector('fieldset').disabled,true,'duplicate submissions remain locked while admission is pending');
 await reply(creating,{error:'admission failed'},400);await settleViews();assert.equal(field('team.initialTask','textarea').value,'Inspect only the assigned module','failed creation preserves input');
 await act(()=>form.dispatchEvent(new window.Event('submit',{bubbles:true,cancelable:true})));const retry=controls().at(-1);assert.equal(JSON.parse(retry.options.body).arguments.requestId,args.requestId,'same retry keeps its request identity');
 const failedReceipt=value();failedReceipt.board.receipt={error:'provider admission refused'};
 await reply(retry,failedReceipt);await settleViews();assert.equal(field('team.memberName').value,'中文成员','a rejected operation receipt must preserve the form');assert.match(document.querySelector('[role=alert]').textContent,/provider admission refused/);
 await act(()=>form.dispatchEvent(new window.Event('submit',{bubbles:true,cancelable:true})));await reply(controls().at(-1),value());await settleViews();assert.equal(field('team.memberName').value,'');
 await click('team.tasks');await act(()=>document.querySelector('.dshTeamCreate>summary').click());await input(field('team.subject'),'First task');
 const taskForm=field('team.subject').closest('form');await act(()=>taskForm.dispatchEvent(new window.Event('submit',{bubbles:true,cancelable:true})));
 const firstTask=controls().at(-1),firstId=JSON.parse(firstTask.options.body).arguments.taskId;assert.equal(JSON.parse(firstTask.options.body).arguments.expectedRevision,0);
 await reply(firstTask,value());await settleViews();await input(field('team.subject'),'Second task');await act(()=>taskForm.dispatchEvent(new window.Event('submit',{bubbles:true,cancelable:true})));
 assert.notEqual(JSON.parse(controls().at(-1).options.body).arguments.taskId,firstId,'new tasks receive a new identity after successful creation');
 await reply(controls().at(-1),value());await settleViews();await click('team.dispatch');
 const dispatch=controls().at(-1);assert.deepEqual(JSON.parse(dispatch.options.body).arguments,{action:'dispatch',taskId:'check',expectedRevision:1});
 await reply(dispatch,{error:'task revision conflict'},400);await settleViews();assert.match(document.querySelector('[role=alert]').textContent,/revision conflict/);
 await click('team.settings');await act(()=>document.querySelector('[data-team-settings] input[role=switch]').click());assert.equal(settings.value.enabled,true,'settings edits remain draft until saved');
 await click('team.save');await settleViews({...value(),enabled:false});assert.equal(settings.value.enabled,false);assert.match(document.body.textContent,/team.saved/);
 await click('team.members');assert.ok(document.querySelector('[data-standalone-subagents]'),'standalone subagents stay discoverable when team collaboration is disabled');assert.match(document.querySelector('[data-standalone-subagents]').textContent,/team.standaloneHint/);assert.equal([...document.querySelectorAll('label')].some(n=>n.querySelector('span')?.textContent==='team.memberName'),false,'disabled collaboration does not offer team member creation');await click('team.settings');
 await click('team.addProfile');await input(field('team.profileName'),'编码方案');await click('team.save');await settleViews({...value(),enabled:false});assert.equal(settings.value.profiles[0].name,'编码方案');assert.equal(settings.value.profiles[0].roles.length,1);
 settingsFail=true;await act(()=>document.querySelector('[data-team-settings] input[role=switch]').click());await click('team.save');assert.match(document.querySelector('[role=alert]').textContent,/revision conflict/);assert.equal(settings.value.enabled,false,'failed settings writes do not report success');
 await click('close');await act(()=>render('b'));const stale=requests.at(-1);await act(()=>render('c'));assert.equal(stale.options.signal.aborted,true);await reply(stale,value('b'));await settleViews(value('c'));assert.equal(document.querySelector('[role=dialog]'),null,'late responses do not reopen a different conversation');
 settings={...settings,value:{...settings.value,enabled:true}};await act(()=>listeners.forEach(fn=>fn()));
 await act(()=>render('a'));await settleViews(value('a'));await act(()=>panel.open('a'));await settleViews(value('a'));await click('team.tasks');await click('team.dispatch');const oldMutation=controls().at(-1);
 await act(()=>render('b'));await act(()=>render('a'));await settleViews(value('a'));await act(()=>panel.open('a'));await settleViews(value('a'));await click('team.tasks');await click('team.dispatch');const newMutation=controls().at(-1);assert.notEqual(newMutation,oldMutation);
 const staleMutation=value('a');staleMutation.board.tasks.check.subject='STALE MUTATION';await reply(oldMutation,staleMutation);
 assert.doesNotMatch(document.body.textContent,/STALE MUTATION/,'A→B→A navigation cannot accept a previous generation mutation result');assert.equal(button('team.dispatch').disabled,true,'the old completion cannot unlock the current mutation');
 const freshMutation=value('a');freshMutation.board.tasks.check.subject='FRESH MUTATION';await reply(newMutation,freshMutation);await settleViews(freshMutation);assert.match(document.body.textContent,/FRESH MUTATION/);
 await act(()=>root.unmount());assert.equal(timers.size,0);
 const directory=context.teamModelDirectory(),modelRequest=requests.at(-1);assert.equal(modelRequest.url,'/task-models/describe','global settings must not call a session-only model endpoint without a session');assert.deepEqual(JSON.parse(modelRequest.options.body),{});await reply(modelRequest,{providers:[{id:'fixture',name:'Fixture',models:[{id:'worker',name:'Worker'}]}]});assert.equal((await directory).groups[0].models[0].id,'worker');dom.window.close();
 console.log('PASS unified collaboration: shared entry, real action payloads, task CAS, stable retries, independent member addressing, live settings and stale-response isolation');
})().catch(error=>{console.error(error);process.exitCode=1;dom.window.close();});
