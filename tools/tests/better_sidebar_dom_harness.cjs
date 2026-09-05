const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.env.DSH_REACT_TEST_MODULES||process.argv[2];
if(!modules)throw Error('Pass react/react-dom/jsdom node_modules');
const {JSDOM}=require(path.join(modules,'jsdom'));
const dom=new JSDOM('<!doctype html><body><div data-conversation-scroll><a id="link" target="_blank" href="https://example.invalid/page">link</a></div><main id="root"></main></body>',{url:'http://127.0.0.1:59800',pretendToBeVisual:true});
Object.assign(global,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
const React=require(path.join(modules,'react')),ReactDOM=require(path.join(modules,'react-dom')),ReactClient=require(path.join(modules,'react-dom/client'));
const rootPath=path.resolve(__dirname,'../..'),clientSource=fs.readFileSync(path.join(rootPath,'release/plugins/dsh-better-sidebar/lib/client.js'),'utf8');
const writes=[],requests=[];let values={width:760,rememberWidth:true,fullscreenOnOpen:false,showFiles:true,showGit:true,showBrowser:true,showTerminal:true,httpLinks:'sidebar',httpsLinks:'sidebar'},revision=0,failNext=false;
Object.defineProperty(window,'innerWidth',{configurable:true,writable:true,value:1280});
const captures=new Map();
window.HTMLElement.prototype.setPointerCapture=function(id){captures.set(this,id)};
window.HTMLElement.prototype.hasPointerCapture=function(id){return captures.get(this)===id};
window.HTMLElement.prototype.releasePointerCapture=function(id){if(captures.get(this)===id)captures.delete(this)};
window.HTMLElement.prototype.getBoundingClientRect=function(){const width=this.classList.contains('dbs-panel')?(this.dataset.fullscreen==='true'?window.innerWidth:Math.min(parseFloat(this.style.getPropertyValue('--dbs-width'))||680,window.innerWidth-20)):0;return{width,height:700,left:window.innerWidth-width,right:window.innerWidth,top:0,bottom:700,x:window.innerWidth-width,y:0,toJSON(){return{}}}};
function snapshotStore(initial){let snapshot=initial;const listeners=new Set();return{getSnapshot:()=>snapshot,subscribe:listener=>{listeners.add(listener);return()=>listeners.delete(listener)},update:mutate=>{const next={...snapshot};mutate(next);snapshot=next;listeners.forEach(listener=>listener())}}}
let settingsExports;
vm.runInNewContext(fs.readFileSync(path.join(rootPath,'web/dist/plugins/ui-settings.js'),'utf8'),{window:{__ModuleLoader__:{load:definition=>settingsExports=definition.factory(id=>id==='@deepseek-ai/cordis'?{Service:class{}}:id==='@deepseek-ai/dsh-client-runtime/client'?{createSnapshotStore:snapshotStore}:{})}}});
const api={settings:{
 describe:async()=>({result:{ok:true,value:{writable:true,namespaces:[{ns:'dsh-better-sidebar',value:{...values},revision,schema:{},base:{},user:{...values}}]}}}),
 mutate:async payload=>{writes.push(structuredClone(payload));if(failNext){failNext=false;return{result:{ok:false,error:{message:'fixture save rejected'}}}}if(payload.expectedRevision!==undefined&&payload.expectedRevision!==revision)return{result:{ok:false,error:{message:'revision conflict'}}};for(const op of payload.ops){assert.equal(op.op,'set');values={...values,[op.path[0]]:op.value}}revision++;return{result:{ok:true,value:{ns:'dsh-better-sidebar',value:{...values},revision,schema:{},base:{},user:{...values}}}}}
}};
async function fixtureFetch(url,options={}){requests.push({url,options});const operation=new URL(url,locationOrigin()).pathname.split('/').pop();const data=operation==='meta'?{siteToken:'fixture'}:operation==='list'?{entries:[{name:'sample.rs',path:'sample.rs',kind:'file'}]}:operation==='git-status'?{branch:'main',entries:[],branches:['main']}:operation==='terminal-list'?{entries:[]}:{text:'',entries:[]};return{ok:true,json:async()=>data,text:async()=>'const value = 1;',headers:{get:()=> 'fixture-etag'}}}
function locationOrigin(){return window.location.origin}
const settle=()=>new Promise(resolve=>setTimeout(resolve,25));
const act=async action=>{await React.act(async()=>{await action();await settle()})};
function pressPointer(target,type,x,id=1){const event=new window.MouseEvent(type,{bubbles:true,cancelable:true,button:0,clientX:x});Object.defineProperty(event,'pointerId',{value:id});target.dispatchEvent(event);return event}
function button(label){const result=[...document.querySelectorAll('button')].find(node=>node.getAttribute('aria-label')===label||node.textContent===label);assert.ok(result,`button ${label}`);return result}
function panel(){return document.querySelector('.dbs-panel')}
function width(){return Number.parseFloat(panel().style.getPropertyValue('--dbs-width'))}

async function mount(){
 let exported,active='session-a';const slots=new Map(),effects=[];
 const scope=new settingsExports.SettingsScopeController(api,{namespace:'dsh-better-sidebar',decode:value=>value});
 const sandbox={window,document,location:window.location,URL,URLSearchParams,console,fetch:fixtureFetch,setTimeout,clearTimeout,setInterval,clearInterval};
 window.__ModuleLoader__={load:definition=>{exported=definition.factory(id=>id==='react'?React:id==='react-dom'?ReactDOM:{})}};
 vm.runInNewContext(clientSource,sandbox);
 const ctx={settingsScope:{bind:()=>{void scope.load();return scope}},get:()=>({api}),effect:fn=>{const dispose=fn();effects.push(dispose);return dispose},slots:{inject:(_name,fn)=>fn(),register:(options,component)=>{slots.set(options.name,{options,component});return()=>slots.delete(options.name)}}};
 exported.apply(ctx);
 assert.ok(slots.has('settings.plugin.item'),'settings uses the existing configurable-plugin card');
 assert.ok(!slots.has('settings.section'),'sidebar must not create a duplicate settings page');
 const Launch=slots.get('conversation.session.header.utilities').component,Overlay=slots.get('shell.overlay').component,Card=slots.get('settings.plugin.item').component;
 const root=ReactClient.createRoot(document.getElementById('root'));
 const render=()=>root.render(React.createElement(React.Fragment,null,React.createElement(Launch,{sessionId:active}),React.createElement(Overlay,{useSessions:selector=>selector({current:active})}),React.createElement(Card)));
 await act(render);
 return{scope,descriptor:exported.settingsDescriptor,select:async id=>{active=id;await act(render)},close:async()=>{await act(()=>root.unmount());for(const dispose of effects.reverse())if(typeof dispose==='function')await dispose();await scope.dispose()}};
}

(async()=>{
 let app=await mount();
 assert.equal(app.descriptor.fields.length,9);
 for(const field of app.descriptor.fields)assert.ok(document.getElementById(`dbs-setting-${field.key}`),`declarative control ${field.key}`);
 await act(()=>button('显示工作台').click());assert.equal(width(),760);assert.equal(writes.length,0);
 let separator=panel().querySelector('[role=separator]');
 await act(()=>{pressPointer(separator,'pointerdown',1000);pressPointer(window,'pointermove',930)});
 assert.equal(width(),830);assert.equal(writes.length,0,'pointermove is preview-only');
 await act(()=>pressPointer(window,'pointerup',920));assert.equal(values.width,840);assert.equal(width(),840);assert.equal(writes.length,1);assert.equal(writes[0].expectedRevision,0);
 await act(()=>button('全屏显示工作台').click());assert.equal(panel().dataset.fullscreen,'true');assert.equal(width(),840);
 await act(()=>{window.innerWidth=768;window.dispatchEvent(new window.Event('resize'))});assert.equal(writes.length,1);assert.equal(panel().querySelector('[role=separator]').tabIndex,-1);
 await act(()=>{window.innerWidth=1280;window.dispatchEvent(new window.Event('resize'));window.dispatchEvent(new window.KeyboardEvent('keydown',{key:'Escape',bubbles:true,cancelable:true}))});assert.equal(panel().dataset.fullscreen,undefined);assert.equal(width(),840);assert.equal(writes.length,1,'viewport/fullscreen changes never persist a clamped width');
 await app.select('session-b');await act(()=>button('显示工作台').click());assert.equal(width(),840,'new sessions inherit the Host width');
 separator=panel().querySelector('[role=separator]');const beforeCancel=writes.length;
 await act(()=>{pressPointer(separator,'pointerdown',1000);pressPointer(window,'pointermove',900)});assert.equal(width(),940);
 await act(()=>pressPointer(window,'pointercancel',900));assert.equal(width(),840);assert.equal(writes.length,beforeCancel);
 await act(()=>{pressPointer(separator,'pointerdown',1000);pressPointer(window,'pointermove',950)});
 await act(()=>pressPointer(separator,'lostpointercapture',950));assert.equal(width(),840);assert.equal(writes.length,beforeCancel);
 await act(()=>{pressPointer(separator,'pointerdown',1000);pressPointer(window,'pointermove',980)});
 await act(()=>button('关闭工作台').click());await act(()=>pressPointer(window,'pointerup',800));assert.equal(panel(),null);assert.equal(writes.length,beforeCancel,'closing during drag removes listeners without a commit');
 await act(()=>button('显示工作台').click());separator=panel().querySelector('[role=separator]');failNext=true;
 await act(()=>{pressPointer(separator,'pointerdown',1000);pressPointer(window,'pointerup',940)});
 assert.equal(width(),900);assert.equal(values.width,840);assert.match(document.querySelector('.dbs-persistence').textContent,/未保存|尚未持久化/);
 await act(()=>button('重试保存宽度').click());assert.equal(values.width,900);assert.equal(document.querySelector('.dbs-persistence'),null);
 await app.select('session-a');assert.equal(width(),900,'existing cached sessions use the same latest width');
 const navigationCount=writes.length;
 await act(()=>document.querySelector('.dbs-row[title="sample.rs"]').click());await act(()=>button('编辑').click());
 const textarea=document.querySelector('.dbs-code-input');await act(()=>{Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype,'value').set.call(textarea,'unsaved text');textarea.dispatchEvent(new window.Event('input',{bubbles:true}))});
 await act(()=>button('全屏显示工作台').click());await act(()=>button('退出全屏').click());assert.equal(document.querySelector('.dbs-code-input').value,'unsaved text','fullscreen preserves the active editor buffer');assert.equal(writes.length,navigationCount);
 await act(()=>document.getElementById('dbs-setting-showBrowser').click());assert.equal(values.showBrowser,false);assert.ok(![...panel().querySelectorAll('.dbs-tab')].some(tab=>tab.textContent==='网页'));
 const link=document.getElementById('link'),nativeLink=new window.MouseEvent('click',{bubbles:true,cancelable:true,button:0});await act(()=>link.dispatchEvent(nativeLink));assert.equal(nativeLink.defaultPrevented,false,'hidden browser entry falls back to normal link opening');
 await act(()=>document.getElementById('dbs-setting-showBrowser').click());
 const sidebarLink=new window.MouseEvent('click',{bubbles:true,cancelable:true,button:0});await act(()=>link.dispatchEvent(sidebarLink));assert.equal(sidebarLink.defaultPrevented,true);assert.ok(panel().querySelector('iframe'));
 await act(()=>document.getElementById('dbs-setting-showBrowser').click());assert.ok([...panel().querySelectorAll('.dbs-tab')].some(tab=>tab.textContent==='网页'),'an already opened page stays rendered when its future entry is hidden');
 const nextLink=new window.MouseEvent('click',{bubbles:true,cancelable:true,button:0});await act(()=>link.dispatchEvent(nextLink));assert.equal(nextLink.defaultPrevented,false);
 await act(()=>button('文件').click());assert.ok(![...panel().querySelectorAll('.dbs-tab')].some(tab=>tab.textContent==='网页'));
 await act(()=>{window.innerWidth=768;window.dispatchEvent(new window.Event('resize'))});await act(()=>button('文件树').click());assert.equal(panel().dataset.mobileTree,'true');await act(()=>document.querySelector('.dbs-row[title="sample.rs"]').click());assert.equal(panel().dataset.mobileTree,undefined);
 const beforeResize=writes.length;await act(()=>{window.innerWidth=769;window.dispatchEvent(new window.Event('resize'))});assert.equal(panel().querySelector('[role=separator]').tabIndex,0);assert.equal(width(),900);assert.equal(writes.length,beforeResize);
 await act(()=>{window.innerWidth=1280;window.dispatchEvent(new window.Event('resize'))});await app.close();
 app=await mount();await act(()=>button('显示工作台').click());assert.equal(width(),900,'fresh module/page reads durable width from Host, not a session map');
 await act(()=>document.getElementById('dbs-setting-rememberWidth').click());assert.equal(values.rememberWidth,false);const writesBeforeTemporary=writes.length;
 separator=panel().querySelector('[role=separator]');await act(()=>{pressPointer(separator,'pointerdown',1000);pressPointer(window,'pointerup',960)});assert.equal(width(),940);assert.equal(values.width,900);assert.equal(writes.length,writesBeforeTemporary);
 await app.select('session-c');await act(()=>button('显示工作台').click());assert.equal(width(),940,'temporary widths remain consistent inside this page');
 await app.close();app=await mount();await act(()=>button('显示工作台').click());assert.equal(width(),900,'unremembered width does not claim reload persistence');
 await act(()=>document.getElementById('dbs-setting-fullscreenOnOpen').click());await act(()=>button('关闭工作台').click());await act(()=>button('显示工作台').click());assert.equal(panel().dataset.fullscreen,'true');await act(()=>button('退出全屏').click());assert.equal(width(),900);
 await app.close();assert.equal(captures.size,0);
 console.log('PASS sidebar React DOM + real SettingsScopeController: declarative Host settings/CAS, cross-session and page width, fullscreen restore/editor preservation, pointer end/cancel/lostcapture/close, visible save failure/retry, hidden-browser native links, mobile tree and 768/769 boundary');
})().catch(error=>{console.error(error);process.exitCode=1}).finally(()=>dom.window.close());
