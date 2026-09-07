const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const { JSDOM } = require(path.join(modules, 'jsdom'));
const React = require(path.join(modules, 'react')), ReactDOM = require(path.join(modules, 'react-dom'));
const dom = new JSDOM('<main id="root"></main>', { pretendToBeVisual: true, url: 'http://localhost/' });
Object.assign(global, { window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
const Client = require(path.join(modules, 'react-dom/client'));
window.matchMedia = () => ({ matches: true });
const observers = [];
class ResizeObserver { constructor(fn) { this.fn = fn; observers.push(this); } observe() {} disconnect() { this.dead = true; } }
const runtimeSource=fs.readFileSync(path.join(__dirname,'../../web/dist/plugins/client-runtime.js'),'utf8');
const referenceRuntime={TextEncoder,TextDecoder,Uint8Array,atob,btoa};
const referenceStart=runtimeSource.indexOf('function sessionReferenceParts('),referenceEnd=runtimeSource.indexOf('//#endregion',referenceStart);
vm.runInNewContext(runtimeSource.slice(referenceStart,referenceEnd),referenceRuntime);
let exported, Component, inject;
const context = { window, document, console, setTimeout, clearTimeout, MutationObserver: window.MutationObserver, ResizeObserver,
  requestAnimationFrame: window.requestAnimationFrame.bind(window), cancelAnimationFrame: window.cancelAnimationFrame.bind(window) };
window.__ModuleLoader__ = { load: definition => { exported = definition.factory(id => id === 'react' ? React : id === 'react-dom' ? ReactDOM : id === 'react/jsx-runtime' ? require(path.join(modules, 'react/jsx-runtime')) : id === '@deepseek-ai/dsh-client-runtime/client' ? referenceRuntime : {}); } };
vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../../release/plugins/dsh-context-jump/lib/client.js'), 'utf8'), context);
const listeners = new Set(), snapshotListeners = new Set();
let index = Array.from({ length: 120 }, (_, seq) => ({ key: `user:${seq}`, seq, text: `Message ${seq}`, images: 0 }));
const state = { chat: { order: [], nodes: new Map() } }, loaded = [];
const session = { projections: { faceOf: () => ({ getSnapshot: () => index, subscribe: fn => { listeners.add(fn); return () => listeners.delete(fn); } }) },
  subscribe: fn => { snapshotListeners.add(fn); return () => snapshotListeners.delete(fn); }, getSnapshot: () => state,
  loadAround: async seq => { loaded.push(seq); state.chat.order = [`user:${seq}`]; state.chat.nodes = new Map([[`user:${seq}`, { kind: 'user', anchorSeq: seq }]]); const row = document.createElement('div'); row.dataset.chatAnchorKey = `user:${seq}`; document.querySelector('[data-chat-flow]').append(row); return true; } };
exported.apply({ effect: fn => fn(), locale: { register: () => () => {} }, sessions: { binding: () => ({ session }) },
  slots: { inject: (_name, fn) => fn(), register: (definition, component) => { Component = component; inject = definition.inject; } } });
const root = Client.createRoot(document.getElementById('root'));
const props = { ...inject('session'), useSession: select => select(state), t: (key, args) => key + JSON.stringify(args ?? {}) };
const act = async fn => React.act(async () => { await fn(); await new Promise(resolve => setTimeout(resolve, 35)); });
let bodyTop=80,bodyBottom=680;
function mountConversation() { const port=document.createElement('div');port.dataset.conversationScroll='';port.innerHTML='<div data-chat-flow></div>';port.getBoundingClientRect=()=>({left:100,top:bodyTop,right:700,bottom:bodyBottom,width:600,height:bodyBottom-bodyTop});Object.defineProperty(port,'clientHeight',{get:()=>bodyBottom-bodyTop});document.body.append(port); }
function assertBodyBounds() { const rail=document.querySelector('._6bmela_layer');assert.equal(parseFloat(rail.style.top),bodyTop);assert.equal(parseFloat(rail.style.height),bodyBottom-bodyTop);assert.equal(rail.style.bottom,'auto');for(const tick of rail.querySelectorAll('._6bmela_tick')){const y=bodyTop+parseFloat(tick.style.top);assert.ok(y>=bodyTop&&y<=bodyBottom,'navigation ticks stay inside the body, below title and tabs')} }
(async () => {
  await act(() => root.render(React.createElement(Component, props)));
  assert.equal(document.querySelector('._6bmela_layer'), null, 'unmounted conversation has no orphan rail');
  await act(mountConversation);
  assert.ok(document.querySelector('._6bmela_layer'), 'late conversation mount restores navigation without refresh');
  assertBodyBounds();
  await act(()=>{bodyTop=120;bodyBottom=620;observers.filter(observer=>!observer.dead).forEach(observer=>observer.fn())});
  assertBodyBounds();
  const first = document.querySelector('._6bmela_tick');
  await act(() => first.click());
  assert.equal(loaded.length, 1, 'an evicted target loads only its containing history page');
  await act(() => { index = [{ key: 'user:0', seq: 0, text: 'retained', images: 0 }]; listeners.forEach(fn => fn()); });
  assert.equal(document.querySelectorAll('._6bmela_tick').length, 1, 'shrinking projection never leaves the rail window empty');
  await act(() => document.querySelector('[data-conversation-scroll]').remove());
  assert.equal(document.querySelector('._6bmela_layer'), null, 'detached conversation releases rail');
  await act(mountConversation);
  assert.ok(document.querySelector('._6bmela_layer'), 'replacement conversation mounts rail again');
  assertBodyBounds();
  const uri='dsh-session:'+Buffer.from(JSON.stringify('source-中文')).toString('base64url');
  const canonical='@[验收 · 引用源]('+uri+')';
  await act(()=>{index=[{key:'user:0',seq:0,text:canonical+' 请核对事实',images:0}];listeners.forEach(fn=>fn())});
  await act(()=>document.querySelector('._6bmela_tick').focus());
  assert.ok(document.querySelector('[role=tooltip]').textContent.includes('@验收 · 引用源'));
  assert.ok(!document.querySelector('[role=tooltip]').textContent.includes(uri),'rail preview hides canonical opaque URI');
  const conversation=fs.readFileSync(path.join(__dirname,'../../web/dist/plugins/ui-conversation.js'),'utf8');
  const messageContext={react:React,react_jsx_runtime:require(path.join(modules,'react/jsx-runtime')),_deepseek_ai_dsh_client_runtime_client:referenceRuntime,
    _deepseek_ai_dsh_client_ui_primitives:{MessageText:({text})=>React.createElement('span',null,text)},
    _deepseek_ai_dsh_client_ui_attachment:{ImageGallery:()=>null},MessageItem_module_css_default:{refChip:'native-reference-chip'},MessageIconActions:()=>null,messageImageLabels:()=>({})};
  let begin=conversation.indexOf('function contentParts('),end=conversation.indexOf('function retrySeconds(',begin);vm.runInNewContext(conversation.slice(begin,end),messageContext);
  begin=conversation.indexOf('function projectPlainUserText(');end=conversation.indexOf('/** Injected-context keyed Chat renderer.',begin);vm.runInNewContext(conversation.slice(begin,end)+';this.UserMessageNodeView=UserMessageNodeView;',messageContext);
  const content=Object.freeze([{type:'text',text:canonical+' 请核对事实'}]);
  await act(()=>root.render(React.createElement(messageContext.UserMessageNodeView,{node:{data:{content,time:1}},loadImage:async()=>'',t:props.t})));
  assert.equal(document.querySelector('[data-ref-chip=session]').textContent,'@验收 · 引用源');
  assert.ok(!document.body.textContent.includes(uri),'the real user-message renderer displays a friendly reference');
  assert.equal(content[0].text,canonical+' 请核对事实','durable content remains unchanged');
  for(const literal of ['@[普通](https://example.invalid/path?q=123)','@[假的](dsh-session:not-base64)','dsh-session:'+Buffer.from(JSON.stringify('bare-id')).toString('base64url')]){
    await act(()=>root.render(React.createElement(messageContext.UserMessageNodeView,{node:{data:{content:[{type:'text',text:literal}],time:1}},loadImage:async()=>'',t:props.t})));
    assert.equal(document.querySelector('[data-ref-chip=session]'),null);assert.ok(document.body.textContent.includes(literal),'lookalikes and ordinary URLs stay literal');
  }
  await act(() => root.unmount());
  assert.equal(listeners.size, 0); assert.equal(snapshotListeners.size, 0);
  assert.ok(observers.every(observer => observer.dead), 'unmount releases layout observers');
  dom.window.close();
  console.log('PASS context rail/user renderer: canonical label projection, literal lookalikes, body vertical bounds/resize, delayed mount, target page lookup, shrinking index, remount and observer cleanup');
})().catch(error => { console.error(error); process.exitCode = 1; dom.window.close(); });
