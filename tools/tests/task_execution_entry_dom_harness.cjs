const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const { JSDOM } = require(path.join(modules, 'jsdom'));
const dom = new JSDOM('<main id="root"></main>', { pretendToBeVisual: true });
Object.assign(global, { window: dom.window, document: dom.window.document, IS_REACT_ACT_ENVIRONMENT: true });
const React = require(path.join(modules, 'react'));
const root = require(path.join(modules, 'react-dom/client')).createRoot(document.getElementById('root'));
dom.window.HTMLDialogElement.prototype.showModal = function () { this.setAttribute('open', ''); };
dom.window.HTMLDialogElement.prototype.close = function () { this.removeAttribute('open'); this.dispatchEvent(new dom.window.Event('close')); };
let plugin, registration, Component; const effects = [], requests = [];
const context = {
  document, AbortController, crypto: require('node:crypto').webcrypto,
  fetch: async (url, init) => { const input = JSON.parse(init.body); requests.push(input); return { ok: true, status: 200, text: async () => JSON.stringify({ tasks: [] }) }; },
  window: { __ModuleLoader__: { load: value => { plugin = value.factory(() => React); } } }
};
vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../../web/src/runtime-plugins/ui-task-execution.js'), 'utf8'), context);
plugin.apply({ effect: fn => effects.push(fn()), slots: {
  inject: (_, fn) => fn(), register: (config, component) => { registration = config; Component = component; }
} });
async function settle(fn) { await React.act(async () => { fn?.(); await new Promise(resolve => setImmediate(resolve)); }); }
(async () => {
  assert.equal(registration.name, 'conversation.session.header.actions');
  assert.equal(registration.id, 'task-execution');
  await settle(() => root.render(React.createElement(Component, registration.inject('session-a'))));
  const trigger = document.querySelector('button');
  assert.equal(trigger.textContent, '任务验收');
  await settle(() => trigger.click());
  assert.ok(document.querySelector('dialog[open]'), 'registered header action opens the real panel');
  assert.equal(requests.at(-1).sessionId, 'session-a');
  assert.ok(document.querySelector('section[aria-label="任务验收与恢复"]'));
  await settle(() => root.render(React.createElement(Component, registration.inject('session-b'))));
  assert.equal(document.querySelector('dialog'), null, 'session switch closes old task panel');
  await settle(() => document.querySelector('button').click());
  assert.equal(requests.at(-1).sessionId, 'session-b');
  await settle(() => document.querySelector('[aria-label="关闭任务验收"]').click());
  assert.equal(document.querySelector('dialog'), null);
  await settle(() => root.unmount());
  effects.forEach(dispose => dispose?.());
  assert.equal(document.querySelector('style'), null, 'plugin disposal removes its appearance');
  dom.window.close();
  console.log('PASS task execution entry: registration, real panel loading, session switch, close and disposal');
})().catch(error => { console.error(error); process.exitCode = 1; });
