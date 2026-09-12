const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const React = require(path.join(modules, 'react')), {JSDOM} = require(path.join(modules, 'jsdom'));
const dom = new JSDOM('<main id="root"></main>', {pretendToBeVisual:true,url:'http://localhost'});
Object.assign(global,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true});
const source = fs.readFileSync(path.join(__dirname,'../../web/src/runtime-plugins/ui-subagent.js'),'utf8');
const requests=[],timers=new Map(),opened=[];let timerId=0;
const context = {react:React,document,AbortController,
 fetch:(_url,options)=>new Promise((resolve,reject)=>requests.push({options,resolve,reject})),
 setTimeout:fn=>{const id=++timerId;timers.set(id,fn);return id;},clearTimeout:id=>timers.delete(id),
 _deepseek_ai_dsh_client_ui_primitives:{Modal:({children,onClose,title})=>React.createElement('section',{role:'dialog','aria-label':title},React.createElement('button',{onClick:onClose},'close'),children)}
};
const begin=source.indexOf('function TeamBoardAction('),end=source.indexOf('function apply(ctx)',begin);
vm.runInNewContext(source.slice(begin,end),context);
const root=require(path.join(modules,'react-dom/client')).createRoot(document.getElementById('root'));
const act=fn=>React.act(async()=>{await fn();});
const t=key=>key;
const board={teamId:'a',members:{worker:{id:'child-a',name:'worker',phase:'active',status:'running'}},tasks:{check:{id:'check',subject:'Review module',description:'Verify behavior',ownerId:'child-a',status:'in_progress',blockedBy:[],writeScopes:['src/module']}}};
const render=id=>root.render(React.createElement(context.TeamBoardAction,{parentSessionId:id,openChild:value=>opened.push(value),t}));
const answer=(request,value)=>act(()=>request.resolve({ok:true,json:async()=>value}));
(async()=>{
 await act(()=>render('a'));assert.equal(JSON.parse(requests[0].options.body).sessionId,'a');
 await answer(requests[0],{enabled:false});assert.equal(document.querySelector('button'),null,'disabled teams add no header action');
 await act(()=>render('b'));await answer(requests.at(-1),{enabled:true,board:{...board,teamId:'b'}});
 await act(()=>document.querySelector('button').click());await answer(requests.at(-1),{enabled:true,board:{...board,teamId:'b'}});
 assert.match(document.querySelector('[role=dialog]').textContent,/Review module/);assert.match(document.body.textContent,/team.status.running/);
 assert.match(document.body.textContent,/src\/module/);
 const worker=[...document.querySelectorAll('button')].find(button=>button.textContent==='worker');await act(()=>worker.click());
 assert.deepEqual(JSON.parse(JSON.stringify(opened)),[{parentSessionId:'b',childSessionId:'child-a',mode:'continuable'}]);
 assert.equal(document.querySelector('[role=dialog]'),null);
 const stale=requests.at(-1);await act(()=>render('c'));const current=requests.at(-1);
 assert.equal(stale.options.signal.aborted,true);await answer(stale,{enabled:true,board});await answer(current,{enabled:false});
 assert.equal(document.querySelector('[data-agent-team-board]'),null);assert.equal(document.querySelector('button'),null,'late responses do not reopen another team');
 await act(()=>root.unmount());assert.equal(timers.size,0);dom.window.close();
 console.log('PASS team board: opt-in visibility, tasks, actual activity, child addressing and stale response isolation');
})().catch(error=>{console.error(error);process.exitCode=1;dom.window.close();});
