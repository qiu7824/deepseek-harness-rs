const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict'),path=require('node:path');
const modules=path.resolve(process.argv[2]);
const {JSDOM}=require(path.join(modules,'jsdom'));const dom=new JSDOM('<main id="root"></main>',{url:'http://127.0.0.1',pretendToBeVisual:true});
Object.assign(global,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
const React=require(path.join(modules,'react')),Client=require(path.join(modules,'react-dom/client'));let plugin,engines=[];
class Recognition{constructor(){engines.push(this)}start(){this.started=true}stop(){this.stopped=true}abort(){this.aborted=true}}
dom.window.SpeechRecognition=Recognition;dom.window.__ModuleLoader__={load:v=>plugin=v.factory(()=>React)};
vm.runInNewContext(fs.readFileSync(path.resolve(__dirname,'../../release/plugins/dsh-voice-input/lib/client.js'),'utf8'),{window:dom.window,navigator:{language:'zh-CN'},console});
const root=Client.createRoot(document.getElementById('root'));const {act}=React;let props,draft='开头';
function App({sessionId='one'}){const [text,setText]=React.useState(draft);draft=text;props={sessionId,input:{draft:text},inputActions:{setDraft:setText}};return React.createElement(plugin.VoiceInputButton,props)}
const pointer=type=>{const e=new dom.window.MouseEvent(type,{bubbles:true,button:0});document.querySelector('button').dispatchEvent(e)};
const result=(engine,parts)=>engine.onresult({resultIndex:0,results:parts.map(([text,final])=>Object.assign([{transcript:text}],{isFinal:final}))});
(async()=>{
 await act(async()=>root.render(React.createElement(App,{})));
 await act(async()=>pointer('pointerdown'));let r=engines.at(-1);assert.equal(r.started,true);assert.equal(r.interimResults,true);assert.equal(r.continuous,true);
 await act(async()=>result(r,[['你好',false]]));assert.equal(draft,'开头 你好','interim must appear before stop');
 await act(async()=>result(r,[['你好世界',true],['继续',false]]));assert.equal(draft,'开头 你好世界继续');
 await act(async()=>result(r,[['你好世界',true],['继续说',true]]));assert.equal(draft,'开头 你好世界继续说','no duplicated final segments');
 await act(async()=>pointer('pointerup'));assert.equal(r.stopped,true);await act(async()=>r.onend());assert.equal(engines.length,1,'release must not restart');
 await act(async()=>pointer('pointerdown'));r=engines.at(-1);await act(async()=>r.onerror({error:'not-allowed'}));assert.match(document.querySelector('[role=alert]').textContent,/权限被拒绝/);await act(async()=>r.onend());
 await act(async()=>pointer('pointerdown'));r=engines.at(-1);await act(async()=>root.render(React.createElement(App,{sessionId:'two'})));assert.equal(r.aborted,true);assert.equal(r.onresult,null);assert.equal(document.querySelector('button').getAttribute('aria-pressed'),'false');
 await act(async()=>root.unmount());console.log('PASS voice hold/release, immediate partials, final replacement, failure display and session isolation');
})().catch(e=>{console.error(e);process.exitCode=1});
