const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const React = require(path.join(modules, 'react')), jsx = require(path.join(modules, 'react/jsx-runtime'));
const { JSDOM } = require(path.join(modules, 'jsdom'));
const dom = new JSDOM('<main id="root"></main>', { url: 'http://localhost/', pretendToBeVisual: true });
Object.assign(global, { window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
const Client = require(path.join(modules, 'react-dom/client'));
const source = fs.readFileSync(path.join(__dirname, '../../web/src/runtime-plugins/ui-message-feedback.js'), 'utf8');
let tested;
const primitives = new Proxy({
  Tooltip: ({ children }) => children,
  Button: ({ children, variant, size, ...props }) => React.createElement('button', props, children),
  Modal: ({ open, title, children, footer, onClose }) => open && React.createElement('section', { role: 'dialog', 'aria-label': title }, React.createElement('button', { onClick: onClose, 'aria-label': 'close-dialog' }, 'close'), children, footer),
}, { get: (target, name) => target[name] || (() => null) });
const noDelivery = [];
vm.runInNewContext(source.replace('exports.apply = apply;', 'exports.test = {MessageFeedbackController,MessageFeedbackActions,zh,en}; exports.apply = apply;'), {
  window: { __ModuleLoader__: { load: entry => { tested = entry.factory(name => name === 'react' ? React : name === 'react/jsx-runtime' ? jsx : primitives).test; } } },
  document, console, setTimeout, clearTimeout, fetch: (...args) => { noDelivery.push(args); throw new Error('unexpected remote delivery'); },
});
const deferred = () => { let resolve; const promise = new Promise(r => { resolve = r; }); return { promise, resolve }; };
const ok = value => ({ ok: true, value: { ok: true, value } });
function fixture(items = []) {
  const data = new Map(items.map(item => [item.messageId, { ...item }])), calls = [];
  let serial = 0;
  const state = { gate: null, listGate: null, failure: null, data, calls };
  const remote = {
    list: async payload => { calls.push({ method: 'list', payload }); if (state.listGate) await state.listGate.promise; return ok({ items: [...data.values()] }); },
    put: async payload => {
      calls.push({ method: 'put', payload }); if (state.gate) await state.gate.promise;
      if (state.failure) return { ok: true, value: { ok: false, error: { code: state.failure } } };
      const current = data.get(payload.messageId);
      if (payload.ifVersion !== (current?.version ?? null)) return { ok: true, value: { ok: false, error: { code: 'version-conflict', current: current ?? null } } };
      const item = { ...payload, version: `v${++serial}` }; delete item.ifVersion;
      data.set(payload.messageId, item); return ok(item);
    },
    delete: async payload => { calls.push({ method: 'delete', payload }); const current = data.get(payload.messageId); assert.equal(payload.ifVersion, current.version); data.delete(payload.messageId); return ok({ deleted: true }); },
  };
  state.controller = new tested.MessageFeedbackController(remote, 'session-a');
  return state;
}
const root = Client.createRoot(document.getElementById('root'));
const act = fn => React.act(async () => { await fn(); await new Promise(resolve => setTimeout(resolve, 10)); });
const button = label => [...document.querySelectorAll('button')].find(el => el.textContent === label || el.getAttribute('aria-label') === label);
const input = (selector, value) => { const element = document.querySelector(selector); element[Object.keys(element).find(k => k.startsWith('__reactProps$'))].onChange({ target: { value } }); };
const t = key => key;
const props = (fixture, messageId = 'm', sessionId = 'session-a') => {
  const c = fixture.controller;
  return { key: `${sessionId}:${messageId}`, sessionId, messageId, t,
    ensure: () => c.ensure(), readItem: id => c.getSnapshot().items.get(id),
    confirmRating: (...args) => c.confirmRating(...args), retract: (...args) => c.retract(...args),
    useFeedback: selector => selector(React.useSyncExternalStore(c.subscribe, c.getSnapshot)),
  };
};
const render = (fixture, id, session) => act(() => root.render(React.createElement(tested.MessageFeedbackActions, props(fixture, id, session))));
const mutations = f => f.calls.filter(call => call.method !== 'list');
(async () => {
  let f = fixture(); await render(f);
  await act(() => button('action.like').click());
  assert.equal(mutations(f).length, 0); assert.equal(document.querySelector('[role=dialog]').getAttribute('aria-label'), 'confirm.positive');
  await act(() => button('note.cancel').click()); assert.equal(mutations(f).length, 0);
  await act(() => button('action.dislike').click());
  await act(() => { input('textarea', 'original negative note'); input('select', 'instruction-following'); });
  f.failure = 'note-too-large';
  await act(() => { button('confirm.save').click(); button('confirm.save').click(); });
  assert.equal(mutations(f).length, 1); assert.equal(document.querySelector('textarea').value, 'original negative note');
  assert.equal(document.querySelector('select').value, 'instruction-following');
  assert.match(document.querySelector('[role=alert]').textContent, /confirm.tooLong/);
  await act(() => button('confirm.dismissError').click()); assert.equal(document.querySelector('textarea').value, 'original negative note');
  f.failure = null; await act(() => button('confirm.save').click());
  assert.equal(document.querySelector('[role=dialog]'), null); assert.equal(f.data.get('m').category, 'instruction-following');
  assert.equal(button('action.dislikeActive').getAttribute('aria-pressed'), 'true');
  await act(() => button('action.like').click());
  assert.equal(document.querySelector('textarea').value, ''); assert.equal(document.querySelector('select').value, '');
  await act(() => button('note.cancel').click()); assert.equal(f.data.get('m').rating, 'negative');
  await act(() => button('action.like').click()); await act(() => button('confirm.save').click());
  assert.equal(f.data.get('m').rating, 'positive'); assert.equal(f.data.get('m').note, undefined); assert.equal(f.data.get('m').category, undefined);
  await act(() => button('action.likeActive').click()); assert.equal(f.data.has('m'), false); assert.equal(document.querySelector('[role=dialog]'), null);

  await act(() => root.render(null));
  f = fixture([{ messageId: 'm', rating: 'positive', version: 'seed' }]); f.listGate = deferred();
  await render(f); await act(() => button('action.like').click()); assert.equal(mutations(f).length, 0);
  await act(() => f.listGate.resolve()); assert.equal(mutations(f)[0].method, 'delete'); assert.equal(document.querySelector('[role=dialog]'), null);

  await act(() => root.render(null));
  f = fixture([{ messageId: 'm', rating: 'positive', version: 'seed' }]); await f.controller.ensure();
  f.gate = deferred(); const changed = f.controller.confirmRating('m', 'negative', 'new note', 'other');
  await new Promise(resolve => setImmediate(resolve)); const staleRetraction = f.controller.retract('m', 'positive');
  f.gate.resolve(); await changed; await staleRetraction;
  assert.equal(mutations(f).length, 1); assert.equal(f.data.get('m').rating, 'negative');

  f = fixture(); await render(f); await act(() => button('action.like').click());
  await act(() => input('textarea', 'retry draft'));
  f.data.set('m', { messageId: 'm', rating: 'negative', version: 'remote' });
  await act(() => button('confirm.save').click()); assert.equal(document.querySelector('textarea').value, 'retry draft');
  assert.match(document.querySelector('[role=alert]').textContent, /error.conflict/);
  await act(() => button('confirm.save').click()); assert.equal(mutations(f).at(-1).payload.ifVersion, 'remote');

  await act(() => button('action.dislike').click());
  f.gate = deferred(); await act(() => button('confirm.save').click());
  await act(() => button('close-dialog').click());
  await act(() => button('action.dislike').click()); await act(() => input('textarea', 'new dialog'));
  await act(() => f.gate.resolve());
  assert.equal(document.querySelector('textarea').value, 'new dialog', 'late completion cannot close a newer dialog');
  await act(() => root.render(null));

  f = fixture(); await render(f); await act(() => button('action.like').click());
  f.gate = deferred(); await act(() => button('confirm.save').click());
  const next = fixture(); next.controller.sessionId = 'session-b'; await render(next, 'other-message', 'session-b');
  await act(() => button('action.dislike').click()); await act(() => input('textarea', 'session-b draft'));
  await act(() => f.gate.resolve()); assert.equal(document.querySelector('textarea').value, 'session-b draft');
  assert.equal(mutations(next).length, 0); assert.equal(mutations(f)[0].payload.sessionId, 'session-a');
  assert.equal(noDelivery.length, 0, 'local feedback never sends a remote submission');
  assert.deepEqual(Object.keys(tested.zh).sort(), Object.keys(tested.en).sort());
  await act(() => root.unmount()); dom.window.close();
  console.log('PASS message feedback: confirmation, cancellation, category/note drafts, cold retraction, CAS retry, serialized stale retraction, double submit and late completion isolation');
})().catch(error => { console.error(error); process.exitCode = 1; root.unmount(); dom.window.close(); });
