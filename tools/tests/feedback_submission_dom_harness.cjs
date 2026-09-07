const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const { JSDOM } = require(path.join(modules, 'jsdom'));
const React = require(path.join(modules, 'react')), jsx = require(path.join(modules, 'react/jsx-runtime'));
const dom = new JSDOM('<main id="root"></main>', { pretendToBeVisual: true, url: 'http://127.0.0.1/' });
Object.assign(global, { window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
const Client = require(path.join(modules, 'react-dom/client'));
const source = fs.readFileSync(path.join(__dirname, '../../web/dist/plugins/ui-message-feedback.js'), 'utf8');
const calls = []; let configured = false, failed = true, status = 'local', id;
const context = { react: React, react_jsx_runtime: jsx, localStorage: window.localStorage, document, URL, Blob, setTimeout,
  _deepseek_ai_dsh_client_ui_primitives: { Modal: ({ open, children, footer }) => open && React.createElement('section', { role: 'dialog' }, children, footer), Button: ({ variant, size, children, ...props }) => React.createElement('button', props, children) },
  fetch: async (url, options) => {
    const action = url.split('/').at(-1), body = JSON.parse(options.body); calls.push({ action, body });
    if (action === 'prepare') id = body.requestId;
    if (action === 'send') { if (failed) { status = 'failed'; return { ok: false, json: async () => ({ error: 'mock acknowledgement failed' }) }; } status = 'delivered'; }
    return { ok: true, json: async () => ({ submissionId: id, status, canSend: configured && status !== 'delivered', destination: configured ? 'https://recipient.invalid' : null, destinationKey: configured ? 'configured-key' : null,
      payload: { submissionId: id, capturedThroughSeq: 8, messages: [{ role: 'assistant', text: 'fixed fixture answer' }], omittedMessages: 0 } }) };
  } };
vm.runInNewContext(source.slice(source.indexOf('async function feedbackSubmissionRequest('), source.indexOf('function MessageFeedbackActions(')), context);
const root = Client.createRoot(document.getElementById('root'));
const props = { sessionId: 'session-fixture', messageId: 'message-fixture', open: false, onClose() {}, t: key => key };
const act = async fn => React.act(async () => { await fn(); await new Promise(resolve => setTimeout(resolve, 10)); });
const render = () => root.render(React.createElement(context.FeedbackSubmissionDialog, props));
const button = label => [...document.querySelectorAll('button')].find(button => button.textContent === label);
(async () => {
  await act(render); assert.equal(calls.length, 0, 'ordinary rendering does not prepare or transmit feedback');
  props.open = true; await act(render); assert.equal(calls[0].action, 'prepare');
  const preparedId = calls[0].body.requestId;
  assert.ok(/^[0-9a-f-]{36}$/.test(preparedId));
  assert.equal(button('submission.send'), undefined, 'unconfigured recipient has no send action');
  assert.ok(document.body.textContent.includes('submission.localOnly'));
  assert.ok(document.querySelector('pre').textContent.includes('fixed fixture answer'), 'the exact captured payload is reviewable');
  configured = true; props.open = false; await act(render); props.open = true; await act(render);
  assert.equal(calls.at(-1).body.requestId, preparedId, 'reopening reviews the existing immutable package');
  assert.equal(calls.filter(call => call.action === 'send').length, 0, 'configuring a recipient never automatically sends');
  await act(() => button('submission.send').click());
  assert.ok(document.querySelector('[role=alert]').textContent.includes('mock acknowledgement failed'));
  failed = false; await act(() => button('submission.retry').click());
  const sends = calls.filter(call => call.action === 'send');
  assert.equal(sends.length, 2); assert.equal(sends[0].body.submissionId, sends[1].body.submissionId);
  assert.equal(sends[0].body.destinationKey, 'configured-key');
  assert.equal(button('submission.send'), undefined); assert.equal(button('submission.retry'), undefined);
  assert.ok(document.body.textContent.includes('submission.status.delivered'));
  let settingsModule;
  const store = initial => { let value=initial; const listeners=new Set(); return { getSnapshot:()=>value, set:next=>{value=next;listeners.forEach(fn=>fn())}, update:fn=>{const next={...value};fn(next);value=next;listeners.forEach(fn=>fn())}, subscribe:fn=>{listeners.add(fn);return()=>listeners.delete(fn)} }; };
  const settingsContext={window,document};
  window.__ModuleLoader__={load:definition=>{settingsModule=definition.factory(id=>id==='react'?React:id==='@deepseek-ai/cordis'?{Service:class{}}:id==='@deepseek-ai/dsh-client-runtime/client'?{createSnapshotStore:store}:{})}};
  vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../../web/dist/plugins/ui-settings.js'),'utf8'),settingsContext);
  let enabled=false, endpointSet=true, rejectWrite=true; const writes=[];
  const snapshot=()=>({ns:'feedback-delivery',revision:1,value:{enabled},secrets:[{path:['endpoint'],set:endpointSet}]});
  const scope=new settingsModule.SettingsScopeController({settings:{describe:async()=>({result:{ok:true,value:{writable:true,namespaces:[snapshot()]}}}),mutate:async request=>{writes.push(request);if(rejectWrite)return{result:{ok:false,error:{message:'mock settings rejected'}}};for(const op of request.ops){if(op.path[0]==='enabled')enabled=op.value;else endpointSet=op.value!==''}return{result:{ok:true,value:snapshot()}}}}},{namespace:'feedback-delivery',decode:value=>value});
  await scope.load();
  vm.runInNewContext(source.slice(source.indexOf('function FeedbackDeliverySettings('),source.indexOf('async function feedbackSubmissionRequest(')),context);
  const controls=new settingsModule.SettingsScopeBinder({}).controls;
  await act(()=>root.render(React.createElement(context.FeedbackDeliverySettings,{scope,controls,t:key=>key})));
  assert.equal(document.querySelector('input').type,'password');assert.equal(document.querySelector('input').value,'','configured secret is never echoed into the input');
  assert.ok(document.body.textContent.includes('delivery.configured'));
  await act(()=>document.querySelector('[role=switch]').click());
  assert.ok(document.body.textContent.includes('mock settings rejected'),'secret settings use checked admission rather than silently clearing a failed draft');
  rejectWrite=false;await act(()=>document.querySelector('[role=switch]').click());
  assert.equal(enabled,true);assert.equal(writes.at(-1).ops[0].path[0],'enabled');
  await act(()=>button('delivery.clear').click());assert.equal(endpointSet,false);assert.equal(writes.at(-1).ops[0].value,'');
  await act(() => root.unmount());await scope.dispose();dom.window.close();
  console.log('PASS feedback UI: local-only review, stable retry identity, native secret controls and checked settings writes');
})().catch(error => { console.error(error); process.exitCode = 1; dom.window.close(); });
