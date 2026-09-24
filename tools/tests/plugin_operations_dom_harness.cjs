"use strict";
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.env.DSH_REACT_TEST_MODULES||process.argv[2];
const {JSDOM}=require(path.join(modules,'jsdom'));
const dom=new JSDOM('<!doctype html><body><main id="root"></main></body>',{url:'http://localhost:58080/'});
Object.assign(global,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
Object.defineProperty(global,'navigator',{value:dom.window.navigator,configurable:true});
const react=require(path.join(modules,'react')), {createRoot}=require(path.join(modules,'react-dom/client'));
const source=fs.readFileSync(path.join(__dirname,'../../web/src/runtime-plugins/ui-settings-plugins.js'),'utf8');
const begin=source.indexOf('async function pluginOperationRequest('),end=source.indexOf('function PluginsSettingsSection(',begin);
assert.ok(begin>=0&&end>begin);
const timers=new Map(),requests=[];let next=0;
let operation={operationId:'op-1',phase:'running',log:'package output',restartRequired:false},configurationError='';
const scope={react,document,AbortController,pluginCenterControlsCss:'',setTimeout:(fn)=>{timers.set(++next,fn);return next;},clearTimeout:id=>timers.delete(id),
fetch:async(_,options)=>{const input=JSON.parse(options.body);requests.push(input);if(input.action==='cancel'){assert.equal(input.operationId,'op-1');operation={...operation,phase:'cancelled'};}if(input.action==='recover'){operation={operationId:'recovery',phase:'succeeded',restartRequired:true,log:'configuration restored'};configurationError='';}return {ok:true,json:async()=>({operation,configurationError})};}};
vm.runInNewContext(source.slice(begin,end)+';this.Controls=PluginInstallControls;',scope);
const text=()=>document.querySelector('main').textContent;
const button=label=>[...document.querySelectorAll('button')].find(button=>button.textContent===label);
async function settle(fn){await react.act(async()=>{fn?.();await new Promise(resolve=>setImmediate(resolve));});}
(async()=>{
    let root=createRoot(document.querySelector('main'));
    await settle(()=>root.render(react.createElement(react.StrictMode,null,react.createElement(scope.Controls))));
    assert.ok(text().includes('正在处理'));assert.ok(button('取消操作'));assert.equal(timers.size,1,'StrictMode does not leak an old polling loop');
    await settle(()=>button('取消操作').click());assert.ok(text().includes('已取消'));assert.ok(!text().includes('更新已提交'));
    await settle(()=>root.unmount());assert.equal(timers.size,0);
    operation=null;configurationError='原文件保留，配置解析失败';root=createRoot(document.querySelector('main'));
    await settle(()=>root.render(react.createElement(scope.Controls)));
    assert.ok(button('恢复上次有效配置'));await settle(()=>button('恢复上次有效配置').click());
    assert.ok(text().includes('已完成'));assert.ok(text().includes('更新已提交'));assert.ok(requests.some(request=>request.action==='recover'));
    await settle(()=>root.unmount());assert.equal(timers.size,0);
    console.log('PASS plugin operations: recovered status, cancellation, configuration restore, commit notice and polling disposal');
})().catch(error=>{console.error(error);process.exitCode=1;});
