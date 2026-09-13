const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.argv[2]||process.env.DSH_REACT_TEST_MODULES;const {JSDOM}=require(path.join(modules,'jsdom'));const dom=new JSDOM('<main></main>',{url:'http://localhost/'});
Object.assign(globalThis,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
const React=require(path.join(modules,'react')),Client=require(path.join(modules,'react-dom/client'));
const source=fs.readFileSync(path.join(__dirname,'../../web/dist/plugins/ui-settings-models.js'),'utf8');const start=source.indexOf('async function taskModelRequest('),end=source.indexOf('function AccountAuthorizationBrowser(',start);assert.ok(start>=0&&end>start);
let state={routes:{image:{provider:'existing',model:'gpt-image-2.5-sunburst'}},main:{provider:'existing',model:'main-model'},revision:7,mainRevision:4,providers:[{id:'existing',name:'Existing connection',models:[{id:'main-model',name:'Main'},{id:'gpt-image-2.5-sunburst',name:'Sunburst'}]}]},writes=[],fail=false;
const context={react:React,fetch:async(url,options)=>{const body=JSON.parse(options.body);if(url.endsWith('/save')){writes.push(body);if(fail)return{ok:false,json:async()=>({error:'revision conflict'})};state={...state,routes:body.routes||state.routes,revision:state.revision+1}}return{ok:true,json:async()=>state}}};vm.runInNewContext(source.slice(start,end)+';this.Panel=TaskModelsPanel;',context);
const root=Client.createRoot(document.querySelector('main')),t=key=>key;const button=label=>[...document.querySelectorAll('button')].find(node=>node.textContent===label);
(async()=>{
 await React.act(()=>root.render(React.createElement(context.Panel,{t})));
 assert.equal(document.querySelectorAll('[data-task-role]').length,5);assert.equal(document.querySelectorAll('input[type=password]').length,0,'task roles reuse existing credentials');
 const image=document.querySelector('[data-task-role=image] input');assert.equal(image.value,'gpt-image-2.5-sunburst');
 await React.act(()=>{Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value').set.call(image,'gpt-image-2.5-flare');image.dispatchEvent(new window.Event('input',{bubbles:true}))});
 fail=true;await React.act(()=>button('taskSave').click());assert.match(document.querySelector('[role=alert]').textContent,/revision conflict/);assert.equal(image.value,'gpt-image-2.5-flare');
 fail=false;await React.act(()=>button('taskSave').click());assert.equal(writes.at(-1).revision,7);assert.equal(writes.at(-1).routes.image.provider,'existing');assert.equal(writes.at(-1).routes.image.model,'gpt-image-2.5-flare');assert.ok(!('main' in writes.at(-1)),'auxiliary assignment does not change main route');
 await React.act(()=>root.unmount());dom.window.close();console.log('PASS task model assignments: existing connections, five roles, no duplicate credentials, preserved drafts and revision writes');
})().catch(error=>{console.error(error);process.exitCode=1;dom.window.close()});
