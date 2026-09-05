const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.env.DSH_REACT_TEST_MODULES||process.argv[2];if(!modules)throw Error('Pass react/react-dom/jsdom node_modules');
const {JSDOM}=require(path.join(modules,'jsdom')),dom=new JSDOM('<!doctype html><body><button id="origin">文件</button><main id="root"></main></body>',{url:'http://fixture.invalid',pretendToBeVisual:true});
Object.assign(global,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
const React=require(path.join(modules,'react')),jsx=require(path.join(modules,'react/jsx-runtime')),ReactDOM=require(path.join(modules,'react-dom/client'));
const plugins=path.resolve(__dirname,'../../web/dist/plugins'),settle=()=>new Promise(resolve=>setImmediate(resolve));
const requests=[],scrollTargets=[];let copied='',sourceFailure=false,pendingRequest=null;
dom.window.HTMLElement.prototype.scrollIntoView=function(){scrollTargets.push(this.dataset.line)};
const resolved={absolutePath:'\\\\?\\D:\\中文工作区\\示例.rs',path:'示例.rs',kind:'file'};
let fileExport;const context={document,window:{__ModuleLoader__:{load:def=>{fileExport=def.factory()}}},URLSearchParams,AbortController,Error,queueMicrotask,innerWidth:1000,innerHeight:720,navigator:{clipboard:{writeText:async text=>{copied=text}}},fetch:async(url,options={})=>{
 requests.push({url,options});const pathname=new URL(url,'http://fixture.invalid').pathname;
 if(pendingRequest&&pathname.endsWith('file-resolve'))return new Promise((resolve,reject)=>{pendingRequest.signal=options.signal;options.signal.addEventListener('abort',()=>reject(new DOMException('aborted','AbortError')),{once:true})});
 if(pathname.endsWith('source'))return{ok:!sourceFailure,json:async()=>sourceFailure?{message:'文件不存在：示例.rs'}:{text:Array.from({length:700},(_,index)=>`第 ${index+1} 行`).join('\n')}};
 return{ok:true,json:async()=>pathname.endsWith('file-resolve')?resolved:{ok:true}};
}};
vm.runInNewContext(fs.readFileSync(path.join(plugins,'ui-file-actions.js'),'utf8'),context);fileExport.apply();const file=context.__DSH_FILE_ACTIONS__;
async function click(label){const node=[...document.querySelectorAll('button')].find(node=>node.textContent===label);assert.ok(node,label);node.click();await settle();await settle()}
async function fileTests(){
const origin=document.getElementById('origin');origin.focus();await file.open({sessionId:'s',path:'示例.rs',line:200});await settle();assert.equal(document.querySelector('.dshFileTitle').textContent,'D:\\中文工作区\\示例.rs');assert.equal(document.querySelector('[data-focus=true]').dataset.line,'200');assert.equal(scrollTargets.at(-1),'200');assert.ok(document.querySelector('[data-line="200"]').textContent.includes('第 200 行'));await click('复制路径');assert.equal(copied,resolved.absolutePath);document.dispatchEvent(new dom.window.KeyboardEvent('keydown',{key:'Escape',bubbles:true}));assert.equal(document.querySelector('[role=dialog]'),null);assert.equal(document.activeElement,origin);
await file.open({sessionId:'s',path:'示例.rs',intent:'menu',x:999,y:999});await settle();assert.equal(document.querySelector('[role=menu]').style.left,'795px');await click('在文件夹中显示');const action=requests.find(request=>request.url==='/__dsh-preview/file-action');assert.deepEqual(JSON.parse(action.options.body),{sessionId:'s',path:'示例.rs',intent:'reveal'});assert.equal(document.querySelector('[role=menu]'),null);
sourceFailure=true;await file.open({sessionId:'s',path:'示例.rs'});assert.match(document.querySelector('.dshFileError').textContent,/文件不存在/);sourceFailure=false;file.close();pendingRequest={};const waiting=file.open({sessionId:'s',path:'hang.rs'});await settle();file.close();assert.equal(pendingRequest.signal.aborted,true);await waiting;pendingRequest=null;assert.equal(document.querySelector('.dshFileShade'),null);
console.log('PASS independent DOM file actions: normalized Chinese path, source line positioning, copy, exact reveal POST, Escape/focus restore, missing-file error, close aborts request');
}
async function graphTests(){
let exported,paused=false;const graphRequests=[],opened=[],intervals=new Map();let timerSeq=0;
const graph={symbols:[{id:'caller',name:'caller',path:'src/调用.rs',line:10,endLine:15},{id:'callee',name:'callee',path:'src/被调.rs',line:20,endLine:25}],calls:[{source:'caller',target:'callee',name:'callee',path:'src/调用.rs',line:12,resolution:'lexical'}],references:[],deps:[],stats:{indexedFiles:2},files:2,totalSymbols:2,totalCalls:1,status:'ready'};
const graphContext={React,document,AbortController,URLSearchParams,Date,console,setTimeout,clearTimeout,setInterval:fn=>{const id=++timerSeq;intervals.set(id,fn);return id},clearInterval:id=>intervals.delete(id),window:{__ModuleLoader__:{load:def=>{exported=def.factory(id=>id==='react'?React:jsx)}}},__DSH_FILE_ACTIONS__:{open:options=>opened.push(options)},fetch:async(url,options={})=>{graphRequests.push({url,options});if(url.endsWith('code-graph-cancel')){paused=true;return{ok:true,json:async()=>({})}}if(new URL(url,'http://fixture.invalid').searchParams.get('resume'))paused=false;return{ok:true,json:async()=>({...graph,status:paused?'cancelled':'ready'})}}};
let source=fs.readFileSync(path.join(plugins,'ui-code-graph.js'),'utf8').replace('return {apply,inject:["slots","sessions"]}','return {apply,inject:["slots","sessions"],CodeGraphView,CodeGraphWatch}');vm.runInNewContext(source,graphContext);
const root=ReactDOM.createRoot(document.getElementById('root')),wait=async()=>React.act(async()=>{await new Promise(resolve=>setTimeout(resolve,25))});
await React.act(async()=>root.render(React.createElement(exported.CodeGraphView,{sessionId:'graph-session'})));await wait();assert.ok(graphRequests.length>0,'opening graph requests index automatically');assert.equal(document.querySelectorAll('.dshGraphCard').length,2);
await React.act(async()=>document.querySelector('.dshGraphCard').click());await wait();assert.ok(graphRequests.some(row=>new URL(row.url,'http://fixture.invalid').searchParams.get('selected')==='caller'));assert.equal(document.querySelectorAll('.dshGraphNode').length,2);await React.act(async()=>document.querySelector('.dshGraphNode[data-focus=true]').dispatchEvent(new dom.window.MouseEvent('dblclick',{bubbles:true})));assert.equal(opened[0].path,'src/调用.rs');assert.equal(opened[0].line,10);
const press=async label=>{await React.act(async()=>{const node=[...document.querySelectorAll('button')].find(node=>node.textContent===label);assert.ok(node,label);node.click()});await wait()};await press('暂停自动更新');assert.equal(JSON.parse(graphRequests.find(row=>row.url.endsWith('code-graph-cancel')).options.body).sessionId,'graph-session');assert.match(document.body.textContent,/自动索引已暂停/);await press('继续索引');assert.ok(graphRequests.some(row=>new URL(row.url,'http://fixture.invalid').searchParams.get('resume')==='1'));
await press('读取符号');await press('读取 src/调用.rs:10–15');assert.equal(opened.at(-1).path,'src/调用.rs');
await React.act(async()=>root.unmount());assert.equal(intervals.size,0,'graph polling cleanup');
console.log('PASS independent React DOM graph: automatic request, select symbol, node double-click exact source, pause/resume, read-symbol source action, polling cleanup');
}
(async()=>{await fileTests();await graphTests()})().catch(error=>{console.error(error);process.exitCode=1});
