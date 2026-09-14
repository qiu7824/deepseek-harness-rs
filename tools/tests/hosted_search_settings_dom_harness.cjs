const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.argv[2]||process.env.DSH_REACT_TEST_MODULES;
const {JSDOM}=require(path.join(modules,'jsdom')),React=require(path.join(modules,'react')),jsx=require(path.join(modules,'react/jsx-runtime'));
const dom=new JSDOM('<main></main>',{url:'http://fixture.invalid/'});Object.assign(global,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true});
const Client=require(path.join(modules,'react-dom/client'));
const store=value=>{const listeners=new Set();return {getSnapshot:()=>value,subscribe:fn=>{listeners.add(fn);return()=>listeners.delete(fn)},set:next=>{value=next;for(const fn of listeners)fn()}}};
let exported;
const primitives=new Proxy({},{get:()=>()=>React.createElement('svg',{'aria-hidden':true})});
vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../../web/dist/plugins/ui-settings-plugins.js'),'utf8').replace('return module.exports;','exports.test={WebSearchCard,WebSearchCardController};return module.exports;'),{
 document,console,window:{__ModuleLoader__:{load:module=>{exported=module.factory(id=>id==='react'?React:id==='react/jsx-runtime'?jsx:id.endsWith('runtime/client')?{createSnapshotStore:store}:primitives)}}}
});
const {WebSearchCard,WebSearchCardController}=exported.test;
const base={baseURL:'https://chosen.example.test/anthropic/v1',maxUses:5,mode:'deepseek',access:'live',apiKeyEnv:'FIXTURE_REF'};
let user={},listeners=new Set(),writes=[],fail=false;
const snapshot=()=>({status:'ready',writable:true,value:{...base,...user},base,user});
const scope={getSnapshot:snapshot,subscribe:fn=>{listeners.add(fn);return()=>listeners.delete(fn)},set:async(field,value)=>{writes.push({field,value});if(!fail){user={...user,[field]:value};for(const fn of listeners)fn()}},unset:async field=>{delete user[field];for(const fn of listeners)fn()}};
const api={credentials:{describe:async()=>({result:{ok:true,value:{credentials:{FIXTURE_REF:{configured:true,writable:true}}}}})}};
let controller=new WebSearchCardController(scope,api);
const root=Client.createRoot(document.querySelector('main')),act=fn=>React.act(async()=>{await fn();await new Promise(resolve=>setImmediate(resolve))});
function Scene(){const useWebSearchCard=select=>React.useSyncExternalStore(controller.store.subscribe,()=>select(controller.store.getSnapshot()));return React.createElement(WebSearchCard,{t:key=>key,useWebSearchCard,...controller.form.actions()})}
const button=text=>[...document.querySelectorAll('button')].find(b=>b.textContent===text);
const change=async(id,value)=>act(()=>{const input=document.getElementById(id);input.value=value;input.dispatchEvent(new window.Event('change',{bubbles:true}))});
(async()=>{
 await act(()=>root.render(React.createElement(Scene)));await act(()=>document.querySelector('[aria-expanded=false]').click());
 assert.equal(document.getElementById('web-search-mode').value,'deepseek');assert.ok(document.getElementById('plugin-config-web-search-endpoint'));
 await change('web-search-mode','hosted');assert.equal(document.getElementById('plugin-config-web-search-endpoint'),null);assert.ok(document.getElementById('web-search-access').classList.contains('KMY8pG_input'));
 await change('web-search-access','cached');assert.equal(writes.length,0,'changing the selectors stages choices without switching the live endpoint');
 await act(()=>button('save').click());assert.deepEqual(user,{mode:'hosted',access:'cached'});assert.equal(snapshot().value.baseURL,base.baseURL);assert.equal(snapshot().value.apiKeyEnv,'FIXTURE_REF');
 await change('web-search-mode','deepseek');fail=true;await act(()=>button('save').click());assert.equal(user.mode,'hosted');assert.match(document.body.textContent,/saveFailed/);
 fail=false;await act(()=>button('discard').click());assert.equal(document.getElementById('web-search-mode').value,'hosted');
 await act(()=>root.render(null));controller=new WebSearchCardController(scope,api);await act(()=>root.render(React.createElement(Scene)));await act(()=>document.querySelector('[aria-expanded=false]').click());assert.equal(document.getElementById('web-search-access').value,'cached');
 await act(()=>root.unmount());dom.window.close();console.log('PASS hosted search settings: shared field styles, staged mode/access selection, save/reload, failed write/discard and unchanged DeepSeek endpoint/credential');
})().catch(error=>{console.error(error);process.exitCode=1;dom.window.close()});
