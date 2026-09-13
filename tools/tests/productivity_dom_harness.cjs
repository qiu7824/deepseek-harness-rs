const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.argv[2]||process.env.DSH_REACT_TEST_MODULES;const {JSDOM}=require(path.join(modules,'jsdom'));const dom=new JSDOM('<main></main>',{url:'http://localhost/'});
Object.assign(globalThis,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
const React=require(path.join(modules,'react')),Client=require(path.join(modules,'react-dom/client'));let plugin,fail=false,board={path:'/workspace/PROJECT_TASKS.md',revision:'one',tasks:[]},writes=[];
const fetch=async(url,options)=>{const body=JSON.parse(options.body);if(url.endsWith('/tasks/save')){writes.push(body);if(fail)return{ok:false,json:async()=>({error:'external file changed'})};board={...board,tasks:body.tasks,revision:'two'}}return{ok:true,json:async()=>board}};
const sandbox={window:{__ModuleLoader__:{load:spec=>{plugin=spec.factory(name=>{if(name==='react')return React;throw Error(name)})}}},document:dom.window.document,fetch,crypto:require('node:crypto').webcrypto};
vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../../web/dist/plugins/ui-productivity.js'),'utf8'),sandbox);
const root=Client.createRoot(document.querySelector('main'));const t=key=>plugin.test.en[key]||key;
async function act(fn){await React.act(async()=>{await fn();await new Promise(r=>setImmediate(r))})}
const button=text=>[...document.querySelectorAll('button')].find(e=>e.textContent===text);
const fill=(node,text)=>{const proto=node.tagName==='TEXTAREA'?window.HTMLTextAreaElement.prototype:window.HTMLInputElement.prototype;Object.getOwnPropertyDescriptor(proto,'value').set.call(node,text);node.dispatchEvent(new window.Event('input',{bubbles:true}))};
(async()=>{
 await act(()=>root.render(React.createElement(plugin.test.ProjectTasks,{sessionId:'session-one',t})));
 await act(()=>button('Add task').click());let title=document.querySelector('form input');await act(()=>fill(title,'Fix shared task fixture'));
 fail=true;await act(()=>document.querySelector('form').dispatchEvent(new window.Event('submit',{bubbles:true,cancelable:true})));assert.match(document.querySelector('[role=alert]').textContent,/external file changed/);assert.equal(title.value,'Fix shared task fixture','failed save preserves the edit form');
 fail=false;await act(()=>document.querySelector('form').dispatchEvent(new window.Event('submit',{bubbles:true,cancelable:true})));assert.equal(writes.at(-1).sessionId,'session-one');assert.equal(writes.at(-1).revision,'one');assert.equal(document.querySelector('form'),null);assert.match(document.querySelector('li').textContent,/Fix shared task fixture/);
 await act(()=>document.querySelector('li input[type=checkbox]').click());assert.equal(writes.at(-1).tasks[0].status,'done');assert.equal(writes.at(-1).revision,'two');
 let snapshot={value:{artifacts:true},revision:1},listeners=new Set();const changes=[];const scope={getSnapshot(){assert.equal(this,scope);return snapshot},subscribe(fn){assert.equal(this,scope);listeners.add(fn);return()=>listeners.delete(fn)},set:async(key,value)=>{changes.push([key,value]);snapshot={value:{...snapshot.value,[key]:value},revision:snapshot.revision+1};for(const listener of listeners)listener()}};
 await act(()=>root.render(React.createElement(plugin.test.Menus,{scope,t})));assert.equal(document.querySelectorAll('input[type=checkbox]').length,4);
 const artifact=[...document.querySelectorAll('label')].find(e=>e.textContent==='Artifacts');await act(()=>artifact.querySelector('input').click());assert.deepEqual(changes,[['artifacts',false]]);
 await act(()=>root.unmount());assert.equal(listeners.size,0);dom.window.close();console.log('PASS productivity UI: task creation/completion, conflict-preserved drafts, shared revision writes, four menu toggles and cleanup');
})().catch(error=>{console.error(error);process.exitCode=1;dom.window.close()});
