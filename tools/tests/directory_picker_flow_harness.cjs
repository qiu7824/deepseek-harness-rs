const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const React = require(path.join(modules, 'react')), jsx = require(path.join(modules, 'react/jsx-runtime'));
const { JSDOM } = require(path.join(modules, 'jsdom'));
const dom = new JSDOM('<main id="root"></main>', { url: 'http://localhost:58080', pretendToBeVisual: true });
Object.assign(global, { window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, Node: dom.window.Node, IS_REACT_ACT_ENVIRONMENT: true });
const root = require(path.join(modules, 'react-dom/client')).createRoot(document.getElementById('root'));
const primitives = new Proxy({
  Modal: ({ open, title, children, footer }) => open ? React.createElement('section', { role: 'dialog' }, title, children, footer) : null,
  Button: ({ children, onClick, disabled }) => React.createElement('button', { onClick, disabled }, children),
  Menu: () => null,
}, { get: (target, name) => target[name] || (() => null) });
const registrations = [], selected = [], created = [];
let behavior = async () => 'E:\\工程\\项目', picks = 0, plugin;
const runtimeSource = fs.readFileSync(path.join(__dirname, '../../web/dist/plugins/client-runtime.js'), 'utf8');
const methodStart = runtimeSource.indexOf('async pickDirectory() {');
const methodEnd = runtimeSource.indexOf('\n\t\t\t}', methodStart) + '\n\t\t\t}'.length;
const service = vm.runInNewContext('({' + runtimeSource.slice(methodStart, methodEnd) + '})');
service.api = { host: { pickDirectory: async () => {
  picks++;
  const value = await behavior();
  return { result: { ok: true, value: { path: value } } };
} } };
service.listDirectory = async () => ({ path: 'E:\\', home: 'E:\\', crumbs: [{ path: 'E:\\', name: 'E:' }], entries: [], truncated: false });
service.createDirectory = async () => {};
const context = { window: { setTimeout, clearTimeout, __ModuleLoader__: { load: definition => {
  plugin = definition.factory(name => name === 'react' ? React : name === 'react/jsx-runtime' ? jsx : name.includes('primitives') ? primitives : { DirectoryBrowseError: class extends Error {} });
} } }, document, localStorage: window.localStorage, AbortController, setTimeout, clearTimeout, Node: window.Node, console };
vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../../web/dist/plugins/directory-picker-browse.js'), 'utf8'), context);
const ctx = {
  effect: callback => callback(),
  locale: { register: () => () => {}, bind: () => key => key },
  workspaces: service,
  connection: { api: service.api },
  slots: {
    inject: (_name, callback) => { const result = callback(); if (result && result[Symbol.iterator]) for (const item of result) {} },
    register: (slot, Component) => { registrations.push({ slot, Component }); return () => {}; },
  },
};
plugin.apply(ctx);
assert.equal(registrations.length, 3, 'workspace, sidebar and settings share the same picker adapter');
const registration = registrations[0], injected = registration.slot.inject();
const workspaceSource = fs.readFileSync(path.join(__dirname, '../../web/dist/plugins/ui-workspace.js'), 'utf8');
const start = workspaceSource.indexOf('const ADD_WORKSPACE ='), end = workspaceSource.indexOf('function WorkspacePicker(', start);
const ownerContext = { react: React, react_jsx_runtime: jsx, _deepseek_ai_dsh_client_ui_primitives: primitives, WorkspacePicker_module_css_default: {},
  fetch: async (_url, options) => { assert.equal(typeof JSON.parse(options.body).path, 'string'); return { ok: true, json: async () => ({}) }; } };
vm.runInNewContext(workspaceSource.slice(start, end), ownerContext);
const act = callback => React.act(async () => { await callback(); await new Promise(resolve => setTimeout(resolve, 15)); });
const t = key => key;
function Owner() {
  const [open, setOpen] = React.useState(true);
  const close = React.useCallback(() => setOpen(false), []);
  return React.createElement(ownerContext.WorkspacePickFlow, {
    t, open, onClose: close, addOnly: true,
    useWorkspaces: () => ({ phase: 'ready', items: [] }), useDirectoryFlow: () => true,
    createWorkspace: async input => { created.push(input); return { workspaceId: 'selected-workspace' }; },
    onPick: id => selected.push(id),
    renderDirectoryFlow: props => React.createElement(registration.Component, { ...injected, ...props }),
  });
}
(async () => {
  window.localStorage.setItem('dsh.directory-picker-mode', 'native');
  await act(() => root.render(React.createElement(Owner)));
  assert.ok(document.body.textContent.includes('E:\\工程\\项目'), 'native choice reaches the workspace form as a path');
  const add = [...document.querySelectorAll('button')].find(button => button.textContent === '添加');
  assert.ok(add); await act(() => add.click());
  assert.equal(created.length, 1); assert.equal(created[0].path, 'E:\\工程\\项目');
  assert.deepEqual(selected, ['selected-workspace'], 'confirmed workspace id reaches the owner selection');
  await act(() => root.render(null));
  let cancelled = 0;
  behavior = async () => null;
  await act(() => root.render(React.createElement(registration.Component, { ...injected, open: true, busy: false, onPicked: () => assert.fail('cancel must not pick'), onCancel: () => cancelled++ })));
  assert.equal(cancelled, 1);
  await act(() => root.render(null));
  behavior = async () => { throw new Error('picker unavailable'); };
  await act(() => root.render(React.createElement(registration.Component, { ...injected, open: true, busy: false, onPicked: () => assert.fail(), onCancel: () => assert.fail('errors must remain visible') })));
  assert.ok(document.querySelector('[role=alert]').textContent.includes('picker unavailable'));
  const before = picks;
  await act(() => {}); assert.equal(picks, before, 'a failed native request is not retried in a render loop');
  await act(() => root.unmount()); dom.window.close();
  console.log('PASS native directory flow: unwrapped path, workspace selection, cancellation and visible failure');
})().catch(error => { console.error(error); process.exitCode = 1; root.unmount(); dom.window.close(); });
