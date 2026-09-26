"use strict";
const assert=require("node:assert/strict"),fs=require("node:fs"),path=require("node:path"),vm=require("node:vm");
const modules=process.env.DSH_REACT_TEST_MODULES||process.argv[2];
const {JSDOM}=require(path.join(modules,"jsdom"));
const dom=new JSDOM('<!doctype html><body><main id="root"></main></body>',{url:"http://localhost/"});
Object.assign(global,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
Object.defineProperty(global,"navigator",{value:dom.window.navigator,configurable:true});
const react=require(path.join(modules,"react")),{createRoot}=require(path.join(modules,"react-dom/client"));
const source=fs.readFileSync(process.env.DSH_TIME_CONTEXT_BUNDLE||path.join(__dirname,"../../web/src/runtime-plugins/ui-settings-plugin-inventory.js"),"utf8");
const begin=source.indexOf("function timeContextDraft("),end=source.indexOf("function PluginInventorySettingsTab(",begin);
assert.ok(begin>=0&&end>begin);
const scope={react,AbortController};
vm.runInNewContext(source.slice(begin,end)+";this.Card=TimeContextConfigCard;this.Controller=createTimeContextConfigController;",scope);
let generation={},config={timeZone:"UTC",refreshIntervalMs:0},revision="one",failure="",pending;
const listeners=new Set(),requests=[];
const connection={generation:{getSnapshot:()=>generation,subscribe:listener=>{listeners.add(listener);return()=>listeners.delete(listener);}},rpc:{call:async(channel,method,payload,signal)=>{
  assert.equal(channel,"/api");requests.push({method,payload,signal});
  if(method.endsWith("setConfig")){
    if(pending)return pending;
    if(failure)return {ok:false,error:{message:failure}};
    assert.equal(payload.expectedRevision,revision);assert.ok(!Object.hasOwn(payload,"enabled"));
    config=JSON.parse(JSON.stringify(payload.config));revision+="x";
  }
  return {ok:true,value:{entryId:payload.entryId,moduleName:"dsh-time-context",revision,config}};
}}};
const button=label=>[...document.querySelectorAll("button")].find(button=>button.textContent===label);
const input=label=>document.querySelector(`input[aria-label="${label}"]`);
async function settle(fn){await react.act(async()=>{fn?.();await new Promise(resolve=>setImmediate(resolve));});}
function edit(label,value){const element=input(label);Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,"value").set.call(element,value);element.dispatchEvent(new window.Event("input",{bubbles:true}));}
(async()=>{
  let root=createRoot(document.querySelector("main"));
  await settle(()=>root.render(react.createElement(react.StrictMode,null,react.createElement(scope.Card,{connection,entryId:"dsh-time-context"}))));
  assert.equal(input("刷新间隔（毫秒）").value,"0");assert.ok(button("保存配置").disabled);
  await settle(()=>edit("刷新间隔（毫秒）","1234567"));assert.ok(!button("保存配置").disabled);
  failure="PLUGIN_CONFIG_CONFLICT: stale revision";
  await settle(()=>button("保存配置").click());assert.equal(input("刷新间隔（毫秒）").value,"1234567");assert.match(document.querySelector('[role="alert"]').textContent,/CONFLICT/);
  revision="fresh";await settle(()=>button("重新读取（保留草稿）").click());assert.equal(input("刷新间隔（毫秒）").value,"1234567");
  failure="";await settle(()=>button("保存配置").click());assert.equal(config.refreshIntervalMs,1234567);assert.ok(button("保存配置").disabled);
  await settle(()=>edit("刷新间隔（毫秒）","-1"));assert.ok(button("保存配置").disabled);
  await settle(()=>button("取消修改").click());assert.equal(input("刷新间隔（毫秒）").value,"1234567");
  let resolve;pending=new Promise(done=>resolve=done);
  await settle(()=>edit("刷新间隔（毫秒）","0"));await settle(()=>button("保存配置").click());
  const request=requests.at(-1);await settle(()=>root.unmount());assert.ok(request.signal.aborted);
  resolve({ok:true,value:{entryId:"dsh-time-context",moduleName:"dsh-time-context",revision:"late",config:{refreshIntervalMs:0}}});
  await settle();assert.equal(listeners.size,0);pending=null;
  let state;const controller=scope.Controller(connection,"dsh-time-context",value=>state=value);await controller.load();
  const before=requests.length;await controller.save();assert.equal(requests.length,before,"same-value save must not issue a mutation");
  controller.edit("refreshIntervalMs","0");let release;pending=new Promise(done=>release=done);const saving=controller.save();
  generation={};release({ok:true,value:{entryId:"dsh-time-context",revision:"wrong-host",config:{refreshIntervalMs:0}}});await saving;
  assert.notEqual(state.snapshot.revision,"wrong-host");const count=requests.length;await controller.save();assert.equal(requests.length,count,"a stale Host generation must not dispatch a write");controller.dispose();
  assert.ok(requests.filter(row=>row.method.endsWith("setConfig")).every(row=>!Object.hasOwn(row.payload,"enabled")));
  console.log("PASS time-context config: explicit zero, custom interval, disabled-state-neutral save, CAS draft retention, reload, no-op, validation, late generation and unmount cancellation");
})().catch(error=>{console.error(error);process.exitCode=1;});
