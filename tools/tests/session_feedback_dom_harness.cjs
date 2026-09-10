const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const { JSDOM } = require(path.join(modules, 'jsdom'));
const dom = new JSDOM('<main id="root"></main>', {pretendToBeVisual:true,url:'http://127.0.0.1/'});
Object.assign(global,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
const React=require(path.join(modules,'react')), Client=require(path.join(modules,'react-dom/client'));
const source=fs.readFileSync(path.join(__dirname,'../../web/src/runtime-plugins/ui-message-feedback.js'),'utf8');
let mode='failure';const calls=[];
const context={react:React,MessageFeedbackActions_module_css_default:{noteInput:'note'},
 _deepseek_ai_dsh_client_ui_primitives:{Modal:({open,children,footer})=>open&&React.createElement('section',{role:'dialog'},children,footer),Button:({variant,size,children,...props})=>React.createElement('button',props,children)},
 fetch:async(url,options)=>{const body=JSON.parse(options.body);calls.push({url,body});return {ok:true,json:async()=>({rpcId:mode==='wrong-id'?'wrong':body.rpcId,result:{ok:true,value:mode==='failure'?{ok:false,error:{message:'Temporary storage failure'}}:{ok:true,value:mode==='missing-ack'?{}:{recorded:true}}}})};}};
vm.runInNewContext(source.slice(source.indexOf('function feedbackSubmissionId('),source.indexOf('function FeedbackSubmissionDialog(')),context);
const root=Client.createRoot(document.getElementById('root'));
const act=fn=>React.act(async()=>{await fn();await new Promise(resolve=>setTimeout(resolve,10));});
const button=label=>[...document.querySelectorAll('button')].find(el=>el.textContent===label);
const input=(el,value)=>el[Object.keys(el).find(key=>key.startsWith('__reactProps$'))].onChange({target:{value}});
(async()=>{
 await act(()=>root.render(React.createElement(context.SessionFeedbackEntry,{key:'a',sessionId:'a',t:key=>key})));
 assert.equal(calls.length,0,'ordinary dialogue does not submit feedback');
 await act(()=>button('session.title').click());assert.equal(button('note.save').disabled,true);
 await act(()=>input(document.querySelector('select'),'service-stability'));
 await act(()=>input(document.querySelector('textarea'),'network retry details'));
 await act(()=>{button('note.save').click();button('note.save').click();});
 assert.equal(calls.length,1,'double clicks are deduplicated');assert.match(document.querySelector('[role=alert]').textContent,/Temporary/);
 assert.equal(document.querySelector('textarea').value,'network retry details');
 const first=calls[0].body.payload;assert.equal(first.category,'service-stability');assert.equal(first.sessionId,'a');
 mode='wrong-id';await act(()=>button('note.save').click());assert.match(document.querySelector('[role=alert]').textContent,/id mismatch/);
 mode='missing-ack';await act(()=>button('note.save').click());assert.match(document.querySelector('[role=alert]').textContent,/not acknowledged/);
 mode='success';await act(()=>button('note.save').click());assert.equal(document.querySelector('[role=dialog]'),null);
 assert.ok(document.querySelector('[role=status]').textContent.includes('session.recorded'));
 assert.ok(calls.every(call=>call.body.payload.requestId===first.requestId));assert.ok(calls.every(call=>call.url==='/api/sessionFeedback.record'));
 await act(()=>root.render(React.createElement(context.SessionFeedbackEntry,{key:'b',sessionId:'b',t:key=>key})));
 await act(()=>button('session.title').click());assert.equal(document.querySelector('textarea').value,'');
 await act(()=>input(document.querySelector('select'),'task-result'));await act(()=>button('note.save').click());
 assert.equal(calls.at(-1).body.payload.sessionId,'b');assert.notEqual(calls.at(-1).body.payload.requestId,first.requestId);
 await act(()=>root.unmount());dom.window.close();console.log('PASS session feedback: local persistence, checked acknowledgement, retry identity, double-submit and session isolation');
})().catch(error=>{console.error(error);process.exitCode=1;dom.window.close();});
