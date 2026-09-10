const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.argv[2]||process.env.DSH_REACT_TEST_MODULES;
const {JSDOM}=require(path.join(modules,'jsdom'));const dom=new JSDOM('<main id="root"></main>',{pretendToBeVisual:true,url:'http://127.0.0.1/'});
Object.assign(global,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true});
const React=require(path.join(modules,'react')),jsx=require(path.join(modules,'react/jsx-runtime')),Client=require(path.join(modules,'react-dom/client')),h=React.createElement;
const primitives=new Proxy({}, {get:()=>()=>h('svg')});
const load=(name,extra)=>{let result;window.__ModuleLoader__={load:definition=>{result=definition.factory(id=>id==='react'?React:id==='react/jsx-runtime'?jsx:id.endsWith('ui-primitives')?primitives:{defineStore:spec=>spec})}};
 const source=fs.readFileSync(path.join(__dirname,'../../web/src/runtime-plugins',name),'utf8').replace('return module.exports;',extra+';return module.exports;');
 vm.runInNewContext(source,{window,document,console,AbortController:window.AbortController,setTimeout,clearTimeout,ResizeObserver:class{observe(){}disconnect(){}},requestAnimationFrame:fn=>setTimeout(fn,0),cancelAnimationFrame:clearTimeout});return result;};
const layout=load('ui-layout.js','exports.AppFrame=AppFrame;exports.createLayoutStore=createLayoutStore'),sidebar=load('ui-sidebar.js','exports.SidebarRoot=SidebarRoot');
const controller=new layout.LayoutController(),allowed=new Set(['statistics']);controller.hasMainPanel=id=>allowed.has(id);
let notifications=0;const unsubscribe=controller.panelInfo.subscribe(()=>notifications++);
const before=controller.panelInfo.getSnapshot();assert.throws(()=>controller.selectPanel('missing'),/not registered/);assert.equal(controller.panelInfo.getSnapshot(),before);
const first=controller.beginNavigation(),second=controller.beginNavigation();assert.equal(first.aborted,true);assert.equal(second.aborted,false);
const usePanelInfo=select=>select(React.useSyncExternalStore(controller.panelInfo.subscribe,controller.panelInfo.getSnapshot));
const items=[{id:'statistics',label:'统计',order:1}],sessions={current:'original',byId:{original:{blank:false}}};let draft='保留的输入草稿';
const spec=layout.createLayoutStore(),state=spec.init(),actions=Object.fromEntries(Object.entries(spec.actions).map(([name,fn])=>[name,(...args)=>fn(state,...args)]));controller.attachPanels(actions);
const calls=[];
function renderSlot(name,props,options){calls.push({name,options});if(name==='main')return options.entryKey==='statistics'?h('article',{'data-global-panel':true},'账号统计'):h('textarea',{'aria-label':'消息草稿',value:draft,onChange:event=>draft=event.target.value});if(name==='sidebar')return h(sidebar.SidebarRoot,{...props,startSession:()=>controller.selectPanel(null),toggleSidebar:()=>{},selectPanel:id=>controller.selectPanel(id),usePanelInfo,usePanels:select=>select(items),t:key=>key,renderSlot:(name,props,options)=>name==='sidebar.panellist'?h('span',{'data-icon-for':options.only},'◉'):null});return null;}
const root=Client.createRoot(document.getElementById('root')),act=fn=>React.act(async()=>{await fn();await new Promise(resolve=>setTimeout(resolve,10));});
const render=()=>root.render(h(layout.AppFrame,{useStore:select=>select(state),useSessions:select=>select(sessions),usePanelInfo,actions,renderSlot}));
(async()=>{
 await act(render);assert.equal(document.querySelector('textarea').value,draft);
 await act(()=>document.querySelector('button[aria-label="统计"]').click());assert.ok(document.querySelector('[data-global-panel]'));assert.equal(second.aborted,true);assert.equal(sessions.current,'original');assert.equal(document.querySelector('button[aria-label="统计"]').getAttribute('aria-current'),'page');
 await act(()=>[...document.querySelectorAll('button')].find(button=>button.textContent==='panels.conversation').click());assert.equal(document.querySelector('textarea').value,draft);assert.equal(notifications,2);
 assert.ok(calls.some(call=>call.name==='main'&&call.options.entryKey==='statistics'));assert.ok(calls.some(call=>call.name==='main'&&call.options.entryKey==='conversation'));
 const pending=controller.beginNavigation();controller.dispose();assert.equal(pending.aborted,true);unsubscribe();await act(()=>root.unmount());dom.window.close();console.log('PASS global panels: validated keyed navigation, sidebar icons, unchanged session/draft, cancellation and cleanup');
})().catch(error=>{console.error(error);process.exitCode=1;dom.window.close();});
