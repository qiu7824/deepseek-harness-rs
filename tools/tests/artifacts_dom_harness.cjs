const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.argv[2]||process.env.DSH_REACT_TEST_MODULES;
const {JSDOM}=require(path.join(modules,'jsdom'));const React=require(path.join(modules,'react'));
const dom=new JSDOM('<div id="root"></div>',{url:'http://127.0.0.1:1/',pretendToBeVisual:true});
Object.assign(global,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
const Client=require(path.join(modules,'react-dom/client'));
const requests=[],opened=[],slots=new Map(),effects=[];let plugin,fail=false;
let rows=[{path:'输出/report.html',change:'created',size:50,etag:'50:1'}];
const scope={snapshot:{value:{enabled:true,autoClean:true,reduceContext:true,keepDays:7},writable:true},listeners:new Set(),getSnapshot(){assert.equal(this,scope);return this.snapshot},subscribe(fn){assert.equal(this,scope);this.listeners.add(fn);return()=>this.listeners.delete(fn)},async set(key,value){if(fail)throw Error('保存失败');this.snapshot={...this.snapshot,value:{...this.snapshot.value,[key]:value}};this.listeners.forEach(fn=>fn())}};
const ctx={
 get:name=>name==='betterSidebar'?{openFile:(scope,path)=>opened.push({scope,path})}:undefined,
 settingsScope:{bind:()=>scope},effect:fn=>effects.push(fn()),
 slots:{inject:(_name,fn)=>fn(),register:(options,component)=>{slots.set(options.name,{options,component});return()=>slots.delete(options.name)}}
};
const fixture=async(url,options)=>{const args=JSON.parse(options.body);const operation=url.split('/').at(-1);requests.push({operation,args});let body={};if(operation==='list')body={entries:rows};else if(operation==='resources')body={entries:[{id:'resource',owner:'session-a',label:'执行输出',kind:'run',state:'active',busy:true,bytes:900,path:'D:/managed/run'}]};else if(operation==='file-action'&&args.action==='trash')rows=[];return{ok:true,json:async()=>body}};
window.__ModuleLoader__={load:definition=>plugin=definition.factory(id=>id==='react'?React:{})};
vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../../release/plugins/dsh-artifacts/lib/client.js'),'utf8'),{window,document,fetch:fixture,URLSearchParams,setTimeout,clearTimeout,AbortController,console});
const root=Client.createRoot(document.getElementById('root'));
const act=async fn=>React.act(async()=>{await fn();await new Promise(resolve=>setTimeout(resolve,15))});
const button=text=>{const result=[...document.querySelectorAll('button')].find(node=>node.textContent===text);assert.ok(result,`button ${text}`);return result};
(async()=>{
 plugin.apply(ctx);assert.equal(slots.get('conversation.view').options.id,'artifacts');assert.equal(slots.get('conversation.view').options.order,15);
 const View=slots.get('conversation.view').component;await act(()=>root.render(React.createElement(View,{sessionId:'session-a',ctx})));
 assert.match(document.body.textContent,/输出\/report.html/);await act(()=>document.querySelector('.dsa-file').click());assert.equal(opened[0].scope.sessionId,'session-a');
 await act(()=>document.querySelector('.dsa-files li').dispatchEvent(new window.MouseEvent('contextmenu',{bubbles:true,cancelable:true,clientX:30,clientY:30})));
 assert.deepEqual([...document.querySelectorAll('[role=menuitem]')].map(node=>node.textContent),['预览','复制路径','在资源管理器中显示','使用本地工具打开','在编辑器中打开','重命名','移入垃圾槽']);
 await act(()=>button('在编辑器中打开').click());assert.ok(requests.some(request=>request.operation==='file-action'&&request.args.sessionId==='session-a'&&request.args.path==='输出/report.html'&&request.args.intent==='editor'));
 await act(()=>document.querySelector('.dsa-files li').dispatchEvent(new window.MouseEvent('contextmenu',{bubbles:true,cancelable:true,clientX:30,clientY:30})));await act(()=>button('移入垃圾槽').click());
 assert.ok(requests.some(request=>request.operation==='file-action'&&request.args.etag==='50:1'&&request.args.action==='trash'));
 await act(()=>button('查看产生的垃圾列表').click());assert.match(document.body.textContent,/正在使用/);assert.equal(button('移入恢复队列').disabled,true);
 await act(()=>root.render(React.createElement(View,{sessionId:'session-b',ctx})));
 assert.ok(requests.some(request=>request.operation==='list'&&request.args.sessionId==='session-b'));assert.equal(document.querySelector('.dsa-resources'),null,'new task resets resource panel');
 const Settings=slots.get('settings.section').component;await act(()=>root.render(React.createElement(Settings)));
 await act(()=>document.querySelector('input[type=checkbox]').click());assert.equal(scope.snapshot.value.enabled,false);
 fail=true;await act(()=>document.querySelector('input[type=checkbox]').click());assert.equal(scope.snapshot.value.enabled,false);assert.match(document.body.textContent,/保存失败/);
 await act(()=>root.unmount());effects.reverse().forEach(dispose=>dispose?.());assert.equal(document.querySelector('style[data-plugin=dsh-artifacts]'),null);
 console.log('PASS artifacts DOM: tab placement, preview, context menu, versioned trash, busy resource protection, session isolation and settings rollback');
})().catch(error=>{console.error(error);process.exitCode=1}).finally(()=>{dom.window.close();process.exit(process.exitCode||0)});
