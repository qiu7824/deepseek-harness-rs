const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.argv[2],{JSDOM}=require(path.join(modules,'jsdom')),React=require(path.join(modules,'react')),jsx=require(path.join(modules,'react/jsx-runtime'));
const dom=new JSDOM('<main></main>',{url:'http://localhost/'});Object.assign(globalThis,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true});
const root=require(path.join(modules,'react-dom/client')).createRoot(document.querySelector('main'));
const source=fs.readFileSync(path.join(__dirname,'../../web/src/runtime-plugins/ui-settings-general.js'),'utf8');
const acorn=require(path.join(__dirname,'../../crates/computer-use/tool-computer-use-command/assets/acorn.cjs'));
const definition=acorn.parseExpressionAt(source,source.indexOf('function SecuritySection('),{ecmaVersion:'latest'});
const host=fs.readFileSync(path.join(__dirname,'../../crates/host/dsh-host/src/lib.rs'),'utf8');
function acceptedValues(field){const start=host.indexOf(`"${field}".to_string(),`),end=host.indexOf('.default(',start);assert.ok(start>=0&&end>start);return new Set([...host.slice(start,end).matchAll(/Data::String\(\s*"([^"]+)"/g)].map(match=>match[1]));}
const accepted={riskToolPolicy:acceptedValues('riskToolPolicy'),outsideWritePolicy:acceptedValues('outsideWritePolicy')};
let namespace={ns:'security',revision:1,value:{riskToolPolicy:'follow-access',outsideWritePolicy:'follow-access',sensitiveReadPolicy:'ask',credentialShellPolicy:'strict'}},reject=false,loseReply=false,readFails=false,gate=null;const writes=[];
const clone=value=>JSON.parse(JSON.stringify(value));
const api={settings:{
 describe:async()=>{if(readFails)throw Error('读取配置失败');return {result:{ok:true,value:{namespaces:[clone(namespace)]}}};},
 mutate:async request=>{
  writes.push(request);if(gate)await gate;assert.equal(request.expectedRevision,namespace.revision);const op=request.ops[0];
  if(accepted[op.path[0]])assert.ok(accepted[op.path[0]].has(op.value),'UI value must be accepted by the actual Host schema');
  if(reject)return {result:{ok:false,error:{message:'配置版本冲突'}}};
  namespace={...namespace,revision:namespace.revision+1,value:{...namespace.value,[op.path[0]]:op.value}};
  if(loseReply){loseReply=false;throw Error('保存回执丢失');}
  return {result:{ok:true,value:clone(namespace)}};
 }
}};
const context={react:React,react_jsx_runtime:jsx,_deepseek_ai_dsh_client_ui_primitives:{Button:({children,variant,size,...props})=>React.createElement('button',props,children)}};
vm.runInNewContext(source.slice(definition.start,definition.end)+';this.Component=SecuritySection;',context);
const act=fn=>React.act(async()=>{await fn();await new Promise(resolve=>setImmediate(resolve))});
function select(id,value){const node=document.getElementById(id);const props=node[Object.keys(node).find(key=>key.startsWith('__reactProps$'))];props.onChange({target:{value}});}
(async()=>{
 await act(()=>root.render(React.createElement(context.Component,{api})));
 assert.equal(writes.length,0);assert.equal(document.getElementById('security-risk').value,'follow-access');assert.equal(document.getElementById('security-outside-write').value,'follow-access');
 for(const [id,field] of [['security-risk','riskToolPolicy'],['security-outside-write','outsideWritePolicy']])for(const option of document.getElementById(id).options)assert.ok(accepted[field].has(option.value),`${field}:${option.value}`);
 await act(()=>select('security-outside-write','allow'));assert.equal(namespace.value.outsideWritePolicy,'allow');
 reject=true;await act(()=>select('security-risk','deny'));assert.match(document.querySelector('[role=alert]').textContent,/版本冲突/,'reloading must not erase the save error');assert.equal(document.getElementById('security-risk').value,'follow-access');reject=false;
 loseReply=true;await act(()=>select('security-risk','ask'));assert.equal(document.getElementById('security-risk').value,'ask');assert.match(document.querySelector('[role=alert]').textContent,/回执丢失/);assert.match(document.querySelector('[role=alert]').textContent,/重新读取/);
 let release;gate=new Promise(resolve=>release=resolve);const prior=writes.length;await act(()=>{select('security-risk','follow-access');select('security-outside-write','deny');});assert.equal(writes.length,prior+1,'parallel changes cannot share a stale revision');assert.equal(document.getElementById('security-risk').disabled,true);gate=null;await act(()=>release());
 readFails=true;reject=true;await act(()=>select('security-risk','deny'));assert.equal(document.querySelectorAll('select').length,0,'unknown current security state is not shown as stale editable controls');assert.match(document.querySelector('[role=alert]').textContent,/读取配置失败/);
 readFails=false;reject=false;await act(()=>document.querySelector('button').click());assert.equal(document.getElementById('security-risk').value,'follow-access');
 await act(()=>root.unmount());dom.window.close();console.log('PASS security settings: backend/UI value parity, explicit overrides, visible CAS errors, lost-response reconciliation and serialized saves');
})().catch(error=>{console.error(error);process.exitCode=1;dom.window.close()});
