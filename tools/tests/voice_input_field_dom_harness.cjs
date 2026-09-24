const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm'),assert=require('node:assert/strict');
const modules=process.argv[2],{JSDOM}=require(path.join(modules,'jsdom'));
const dom=new JSDOM('<main></main>',{url:'http://localhost/',pretendToBeVisual:true});
Object.assign(globalThis,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
const React=require(path.join(modules,'react')),Client=require(path.join(modules,'react-dom/client'));
let now=0,timerId=0,engines=[],plugin;const timers=new Map(),drafts=new Map();
window.setTimeout=(fn,delay=0)=>{const id=++timerId;timers.set(id,{fn,at:now+delay});return id};window.clearTimeout=id=>timers.delete(id);
function advance(ms){const end=now+ms;for(;;){const next=[...timers].filter(([,task])=>task.at<=end).sort((a,b)=>a[1].at-b[1].at)[0];if(!next)break;now=next[1].at;timers.delete(next[0]);next[1].fn();}now=end;}
class Recognition{constructor(){engines.push(this)}start(){this.started=true}stop(){this.stopped=true}abort(){this.aborted=true}}
window.SpeechRecognition=Recognition;window.__ModuleLoader__={load:def=>plugin=def.factory(()=>React)};
vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../../release/plugins/dsh-voice-input/lib/client.js'),'utf8'),{window,console});
const root=Client.createRoot(document.querySelector('main'));let leftId='任务:一/子',locked=false,fieldVersion=0;
function Composer({id,side}){
 const [draft,setDraft]=React.useState(()=>({owner:id,text:drafts.get(id)??'开头'}));
 const text=draft.owner===id?draft.text:(drafts.get(id)??'开头');
 const setText=text=>{drafts.set(id,text);setDraft({owner:id,text});};
 drafts.set(id,text);
 return React.createElement('div',{'data-composer-card':true,'data-side':side},
  React.createElement('textarea',{key:fieldVersion,'data-composer-input':true,'data-session-id':id,disabled:locked,value:text,onInput:event=>setText(event.target.value)}),
  React.createElement(plugin.VoiceInputButton,{sessionId:id,locked,input:{draft:text},inputActions:{setDraft:setText}}));
}
function App(){return React.createElement(React.Fragment,null,React.createElement(Composer,{id:leftId,side:'left'}),React.createElement(Composer,{id:'two',side:'right'}));}
const act=fn=>React.act(async()=>{await fn();await new Promise(resolve=>setImmediate(resolve))});
const field=(side='left')=>document.querySelector(`[data-side=${side}] textarea`);
const phase=(side='left')=>document.querySelector(`[data-side=${side}] [data-voice-phase]`).dataset.voicePhase;
function pointer(target,type,{x=100,y=200,id=1,button=0}={}){const event=new window.MouseEvent(type,{bubbles:true,cancelable:true,button,clientX:x,clientY:y});Object.defineProperty(event,'pointerId',{value:id});target.dispatchEvent(event);}
function result(engine,text,final=false){engine.onresult?.({results:[Object.assign([{transcript:text}],{isFinal:final})]});}
async function hold(side='left'){const f=field(side);f.setSelectionRange(f.value.length,f.value.length);await act(()=>pointer(f,'pointerdown'));await act(()=>advance(350));return engines.at(-1);}
(async()=>{
 await act(()=>root.render(React.createElement(App)));
 await act(()=>pointer(field(),'pointerdown'));await act(()=>advance(349));assert.equal(engines.length,0);
 await act(()=>pointer(document.body,'pointerup'));await act(()=>advance(1));assert.equal(engines.length,0,'ordinary clicks do not start dictation');
 await act(()=>pointer(field(),'pointerdown'));await act(()=>field().closest('[data-composer-card]').dispatchEvent(new window.MouseEvent('pointerleave')));await act(()=>advance(350));assert.equal(engines.length,0,'leaving the composer before the hold threshold cancels pending recognition');
 await act(()=>pointer(field(),'pointerdown'));await act(()=>pointer(document.body,'pointermove',{x:130}));await act(()=>advance(350));assert.equal(engines.length,0,'selection drags cancel the pending hold');
 field().setSelectionRange(0,2);await act(()=>pointer(field(),'pointerdown'));await act(()=>advance(350));assert.equal(engines.length,0,'selected text keeps normal editing semantics');
 let engine=await hold();assert.equal(engines.length,1);assert.equal(engine.interimResults,true);assert.equal(engine.continuous,true);assert.equal(phase(),'listening');assert.equal(phase('right'),'idle');assert.equal(field().style.touchAction,'none');
 await act(()=>{result(engine,'你好');result(engine,'你好世界');});assert.equal(drafts.get(leftId),'开头 你好世界','back-to-back partials do not look like a manual edit');
 await act(()=>pointer(document.body,'pointerup'));assert.equal(engine.stopped,true);assert.equal(phase(),'finishing');assert.equal(field().style.touchAction,'');
 await act(()=>result(engine,'你好世界。',true));await act(()=>engine.onend());assert.equal(phase(),'idle');assert.equal(drafts.get(leftId),'开头 你好世界。');assert.equal(timers.size,0);
 const beforeCancel=drafts.get(leftId);engine=await hold();const staleResult=engine.onresult,staleEnd=engine.onend;
 await act(()=>result(engine,'应被取消'));await act(()=>pointer(document.body,'pointermove',{y:120}));assert.match(document.querySelector('[data-side=left] [role=status]').textContent,/取消/);
 await act(()=>pointer(document.body,'pointerup',{y:120}));assert.equal(engine.aborted,true);assert.equal(drafts.get(leftId),beforeCancel);
 await act(()=>{staleResult({results:[[{transcript:'迟到文字'}]]});staleEnd();});assert.equal(drafts.get(leftId),beforeCancel);assert.equal(phase(),'idle');
 engine=await hold();await act(()=>result(engine,'Esc 取消'));await act(()=>document.dispatchEvent(new window.KeyboardEvent('keydown',{key:'Escape',bubbles:true,cancelable:true})));assert.equal(engine.aborted,true);assert.equal(drafts.get(leftId),beforeCancel);
 engine=await hold();await act(()=>pointer(field(),'lostpointercapture'));assert.equal(engine.aborted,true);assert.equal(phase(),'idle');
 engine=await hold();await act(()=>result(engine,'原识别'));const lateEdit=engine.onresult;
 await act(()=>{field().value='手动文本';field().dispatchEvent(new window.Event('input',{bubbles:true}));});assert.equal(engine.aborted,true);await act(()=>lateEdit({results:[[{transcript:'覆盖'}]]}));assert.equal(drafts.get(leftId),'手动文本');
 engine=await hold();await act(()=>result(engine,'第一句',true));await act(()=>engine.onend());const continuation=engines.at(-1);assert.notEqual(continuation,engine);assert.equal(continuation.started,true);
 await act(()=>result(continuation,'第二句',true));assert.equal(drafts.get(leftId),'手动文本 第一句 第二句');await act(()=>pointer(document.body,'pointerup'));await act(()=>continuation.onend());
 engine=await hold();await act(()=>pointer(document.body,'pointerup'));await act(()=>advance(3000));assert.equal(engine.aborted,true);assert.equal(phase(),'idle');assert.match(document.querySelector('[data-side=left] [role=alert]').textContent,/结束超时/);
 engine=await hold();await act(()=>engine.onerror({error:'not-allowed'}));assert.equal(engine.aborted,true);assert.equal(phase(),'idle');assert.match(document.querySelector('[data-side=left] [role=alert]').textContent,/权限被拒绝/);
 engine=await hold();const oldResult=engine.onresult;leftId='another';await act(()=>root.render(React.createElement(App)));assert.equal(engine.aborted,true);await act(()=>oldResult({results:[[{transcript:'不能串入'}]]}));assert.equal(drafts.get('another'),'开头');
 await act(()=>pointer(field(),'pointerdown'));locked=true;await act(()=>root.render(React.createElement(App)));const count=engines.length;await act(()=>advance(500));assert.equal(engines.length,count,'locking a composer cancels pending gestures');
 locked=false;fieldVersion++;await act(()=>root.render(React.createElement(App)));engine=await hold();assert.equal(engine.started,true,'replacement textarea is handled by its composer boundary');
 const other=await hold('right');assert.equal(engine.aborted,true,'only one composer owns microphone recognition');assert.notEqual(other,engine);assert.equal(phase('right'),'listening');
 await act(()=>window.dispatchEvent(new window.Event('blur')));assert.equal(other.aborted,true);assert.equal(phase('right'),'idle');
 engine=await hold();await act(()=>root.unmount());assert.equal(engine.aborted,true);assert.equal(timers.size,0,'hold and finalization timers are released');
 dom.window.close();console.log('PASS voice input field: deliberate hold, selection/drag protection, streaming partials, outside release, slide/Escape/capture-loss cancel, final timeout, edits, restart, session/card isolation and cleanup');
})().catch(error=>{console.error(error);process.exitCode=1;dom.window.close()});
