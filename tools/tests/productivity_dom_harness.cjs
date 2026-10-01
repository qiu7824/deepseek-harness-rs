const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.argv[2]||process.env.DSH_REACT_TEST_MODULES;const {JSDOM}=require(path.join(modules,'jsdom'));const dom=new JSDOM('<main></main>',{url:'http://localhost/'});
Object.assign(globalThis,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
const React=require(path.join(modules,'react')),Client=require(path.join(modules,'react-dom/client'));let plugin;
const requests=[];const fetch=async(url)=>{requests.push(url);return{ok:true,json:async()=>({sources:[],configured:{},workspaces:[]})}};
const sandbox={window:{__ModuleLoader__:{load:spec=>{plugin=spec.factory(name=>{if(name==='react')return React;throw Error(name)})}}},document:dom.window.document,fetch};
vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../../web/dist/plugins/ui-productivity.js'),'utf8'),sandbox);
const root=Client.createRoot(document.querySelector('main'));const t=key=>plugin.test.en[key]||key;
async function act(fn){await React.act(async()=>{await fn();await new Promise(r=>setImmediate(r))})}
const button=text=>[...document.querySelectorAll('button')].find(e=>e.textContent===text);
(async()=>{
 let snapshot={value:{artifacts:true,tasks:true},revision:1},listeners=new Set();const changes=[];const scope={getSnapshot(){assert.equal(this,scope);return snapshot},subscribe(fn){assert.equal(this,scope);listeners.add(fn);return()=>listeners.delete(fn)},set:async(key,value)=>{changes.push([key,value]);snapshot={value:{...snapshot.value,[key]:value},revision:snapshot.revision+1};for(const listener of listeners)listener()}};
 const entries=[],cleanups=[];
 plugin.apply({effect(fn){const cleanup=fn();if(typeof cleanup==='function')cleanups.push(cleanup)},locale:{register(){},bind:()=>t},settingsScope:{bind:()=>scope},slots:{inject(name,fn){fn()},register(options,render){entries.push({options,render})}}});
 assert.equal(plugin.test.ProjectTasks,undefined);
 assert.equal(entries.some(({options})=>options.name==='conversation.view'||options.id==='project-tasks'),false);
 assert.deepEqual(entries.map(({options})=>options.name),['settings.section','settings.memory.import','settings.models.network']);
 await act(()=>root.render(React.createElement(plugin.test.Menus,{scope,t})));
 assert.deepEqual([...document.querySelectorAll('label')].map(e=>e.textContent),['Trace','Artifacts','Code graph','Context']);
 const artifact=[...document.querySelectorAll('label')].find(e=>e.textContent==='Artifacts');await act(()=>artifact.querySelector('input').click());assert.deepEqual(changes,[['artifacts',false]]);
 assert.equal(snapshot.value.tasks,true,'legacy preference is ignored without mutating user settings');
 await act(()=>root.render(React.createElement(plugin.test.MemoryImport,{t})));
 await act(()=>button('Detect local agents').click());
 assert.match(document.querySelector('main').textContent,/No importable memory files found/);
 assert.deepEqual(requests,['/__dsh-productivity/memory/discover']);
 await act(()=>root.unmount());assert.equal(listeners.size,0);for(const cleanup of cleanups.reverse())cleanup();assert.equal(document.querySelector('style[data-productivity]'),null);dom.window.close();console.log('PASS productivity UI: retired project entry absent, four saved menu toggles, memory import and cleanup');
})().catch(error=>{console.error(error);process.exitCode=1;dom.window.close()});
