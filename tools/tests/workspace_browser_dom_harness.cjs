const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const { JSDOM } = require(path.join(modules, 'jsdom'));
const React = require(path.join(modules, 'react')), jsx = require(path.join(modules, 'react/jsx-runtime'));
const dom = new JSDOM('<main id="root"></main>', { pretendToBeVisual: true, url: 'http://127.0.0.1:58080/' });
Object.assign(global, { window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
const Client = require(path.join(modules, 'react-dom/client'));
const source = fs.readFileSync(path.join(__dirname, '../../release/plugins/dsh-better-sidebar/lib/client.js'), 'utf8');
const opened = [], context = { React, URL, location: window.location, window, setTimeout, clearTimeout,
  SettingsButton: ({ variant, size, children, ...props }) => React.createElement('button', props, children),
  DocTabs: () => null, EmptyState: ({ text }) => React.createElement('p', null, text), set() {}, closeWebTab() {}, openWebTab: (sessionId, url) => opened.push({ sessionId, url }) };
vm.runInNewContext(source.slice(source.indexOf('function browserUrl('), source.indexOf('function WorkbenchTerminal(')), context);
for (const [raw, expected] of [['localhost:5173', 'http://localhost:5173/'], ['127.0.0.1:3000/a', 'http://127.0.0.1:3000/a'], ['192.168.1.2:8080', 'http://192.168.1.2:8080/'], ['example.com/a', 'https://example.com/a']]) assert.equal(context.browserUrl(raw), expected);
for (const invalid of ['javascript:alert(1)', 'file:///C:/Users/Test', 'https://user:secret@example.com']) assert.throws(() => context.browserUrl(invalid));
const root = Client.createRoot(document.getElementById('root'));
const act = async fn => React.act(async () => { await fn(); await new Promise(resolve => setTimeout(resolve, 10)); });
const state = url => ({ url, history: [url], historyIndex: 0, webTabs: [url] });
(async () => {
  await act(() => root.render(React.createElement(context.WorkbenchBrowser, { sessionId: 'a', state: state('http://localhost:5173') })));
  let frame = document.querySelector('iframe');
  assert.ok(frame.getAttribute('sandbox').includes('allow-same-origin'), 'cross-origin browser preview retains the site origin');
  await act(() => frame.dispatchEvent(new window.Event('load')));
  const before = frame;
  await act(() => [...document.querySelectorAll('button')].find(button => button.textContent === '访问').click());
  assert.notEqual(document.querySelector('iframe'), before, 'revisiting an unchanged URL actually reloads it');
  await act(() => root.render(React.createElement(context.WorkbenchBrowser, { sessionId: 'a', state: state('http://127.0.0.1:58080/same-origin') })));
  frame = document.querySelector('iframe');
  assert.ok(!frame.getAttribute('sandbox').includes('allow-same-origin'), 'Host-origin previews keep their script isolation');
  assert.ok(document.body.textContent.includes('页面空白或拒绝连接？'), 'cross-origin embedding restrictions have an actionable fallback');
  await act(() => root.render(React.createElement(context.WorkbenchBrowser, { sessionId: 'a', state: state('file:///C:/private') })));
  await act(() => [...document.querySelectorAll('button')].find(button => button.textContent === '访问').click());
  assert.ok(document.querySelector('[role=alert]').textContent.includes('HTTP / HTTPS'));
  const workspace = fs.readFileSync(path.join(__dirname, '../../web/dist/plugins/ui-workspace.js'), 'utf8');
  const calls = []; let failOpen = true;
  const primitives = new Proxy({ IconCodeOutline16: () => React.createElement('svg', { 'data-native-icon': 'editor' }), IconApiOutline14: () => React.createElement('svg', { 'data-native-icon': 'terminal' }), IconFolderOpen16: () => React.createElement('svg', { 'data-native-icon': 'folder' }), Menu: ({ open, anchor, items, onSelect }) => React.createElement(React.Fragment, null, anchor, open && React.createElement('div', { role: 'menu' }, items.flatMap(item => item.submenu ?? [item]).map(item => React.createElement('button', { key: item.id, role: 'menuitem', disabled: item.disabled, onClick: () => onSelect(item.id) }, item.icon, item.label)))) }, { get: (target, key) => target[key] ?? (() => null) });
  const workspaceContext = { react: React, react_jsx_runtime: jsx, _deepseek_ai_dsh_client_ui_primitives: primitives, Rows_module_css_default: {}, clsx: (...args) => args.filter(Boolean).join(' '),
    AbortController, setTimeout, clearTimeout, location: { hostname: 'remote.example.com' }, fetch: async (url, options) => { calls.push({ url, payload: JSON.parse(options.body) }); return { ok: !url.endsWith('/open') || !failOpen, json: async () => url.endsWith('/meta') ? { apps: [{ id: 'vscode', name: 'VS Code' }, { id: 'terminal', name: 'Terminal' }, { id: 'files', name: 'Files' }] } : failOpen ? { error: 'editor unavailable' } : { opened: true } }; } };
  const start = workspace.indexOf('function ProjectRowItem('), end = workspace.indexOf('function assertNever(', start);
  vm.runInNewContext(workspace.slice(start, end), workspaceContext);
  const props = { group: { workspaceId: '工作区 id', label: 'Workspace', key: 'workspace', expanded: true }, actions: { rename() {}, delete() {} }, onToggle() {}, onCreate() {}, t: (key, args) => key + (args?.name ? ':' + args.name : '') };
  await act(() => root.render(React.createElement(workspaceContext.ProjectRowItem, props)));
  assert.equal(calls.length, 0, 'application discovery is lazy');
  await act(() => document.querySelector('button[aria-label="actions.workspace.aria:Workspace"]').click());
  assert.equal(calls[0].payload.workspaceId, '工作区 id');
  const app = () => [...document.querySelectorAll('[role=menuitem]')].find(button => button.textContent.includes('remoteName:VS Code'));
  assert.ok(app(), 'remote browser labels the Host computer explicitly');
  for (const [name,icon] of [['VS Code','editor'],['Terminal','terminal'],['Files','folder']]) { const button=[...document.querySelectorAll('[role=menuitem]')].find(node=>node.textContent.includes('remoteName:'+name)); assert.equal(button.querySelector('[data-native-icon]').getAttribute('data-native-icon'),icon); }
  await act(() => app().click());
  assert.equal(calls.at(-1).payload.appId, 'vscode');
  assert.ok(document.querySelector('[role=menu]').textContent.includes('editor unavailable'), 'failed launch keeps the real error visible');
  failOpen = false; await act(() => app().click());
  assert.equal(document.querySelector('[role=menu]'), null, 'only successful launch closes the menu');
  await act(() => root.unmount()); dom.window.close();
  console.log('PASS browser/workspace: HTTP local URLs, safe embedding, reload/errors, lazy app menu and real launch status');
})().catch(error => { console.error(error); process.exitCode = 1; root.unmount(); dom.window.close(); });
