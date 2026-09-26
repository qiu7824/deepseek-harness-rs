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
  const workspacePath = process.env.DSH_WORKSPACE_PLUGIN_SOURCE || path.join(__dirname, '../../web/src/runtime-plugins/ui-workspace.js');
  const workspace = fs.readFileSync(workspacePath, 'utf8');
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

  let definition;
  const archivePrimitives = new Proxy({
    Tooltip: ({ children }) => children,
    HoverCard: ({ anchor }) => anchor,
    Modal: ({ open, children }) => open ? React.createElement('section', null, children) : null,
    Menu: ({ open, anchor, items, selectedIds = [], onSelect }) => React.createElement(React.Fragment, null, anchor,
      open && React.createElement('div', { role: 'menu' }, items.filter(item => !item.type).map(item => React.createElement('button', {
        key: item.id, role: 'menuitemradio', 'aria-checked': selectedIds.includes(item.id), onClick: () => onSelect(item.id)
      }, item.label))))
  }, { get: (target, key) => target[key] ?? (() => null) });
  vm.runInNewContext(workspace.replace('exports.apply = apply;', 'exports.test = { createWorkspaceViewStore, normalizedArchiveFilter, deriveGroups, deriveFlat, deriveSearchResults, WorkspaceBrowser, zh }; exports.apply = apply;'), {
    window: { __ModuleLoader__: { load: value => { definition = value; } } }, document, console,
    setTimeout, clearTimeout, setInterval, clearInterval, AbortController, URL, Date,
    requestAnimationFrame: window.requestAnimationFrame.bind(window), cancelAnimationFrame: window.cancelAnimationFrame.bind(window)
  });
  const api = definition.factory(name => ({ react: React, 'react/jsx-runtime': jsx,
    '@deepseek-ai/dsh-client-ui-primitives': archivePrimitives,
    '@deepseek-ai/dsh-client-runtime/client': { defineStore: value => value, indexSubagentDescendants: () => new Map() }
  })[name]).test;
  const summary = (id, updatedAt, extra = {}) => ({ id, displayTitle: 'record ' + id, running: false, blank: false, updatedAt, ...extra });
  const summaries = [summary('active', 10), summary('archived', 8), summary('blank-archived', 7, { blank: true }),
    summary('child', 12, { origin: 'subagent' }), summary('blank', 11, { blank: true }), summary('loose', 6)];
  const list = { phase: 'ready', ids: summaries.map(row => row.id), byId: Object.fromEntries(summaries.map(row => [row.id, row])), current: 'active' };
  const workspaceRows = [
    { workspaceId: 'mixed', title: 'Mixed workspace', sessionIds: ['active', 'archived', 'blank-archived', 'child', 'blank'], createdAt: '2026-01-01' },
    { workspaceId: 'empty', title: 'Empty workspace', sessionIds: [], createdAt: '2026-01-01' }
  ];
  let archiveIds = ['archived', 'blank-archived', 'child', 'loose'];
  const untouched = JSON.stringify(list);
  const ids = rows => Array.from(rows, row => row.id);
  assert.deepEqual(ids(api.deriveFlat(list, archiveIds)), ['active'], 'old preference omission retains hidden mode');
  assert.deepEqual(ids(api.deriveFlat(list, archiveIds, 'invalid')), ['active'], 'unknown persisted mode retains the old default');
  assert.deepEqual(ids(api.deriveFlat(list, archiveIds, 'all')), ['active', 'archived', 'blank-archived', 'loose']);
  assert.deepEqual(ids(api.deriveFlat(list, archiveIds, 'archived')), ['archived', 'blank-archived', 'loose']);
  const groups = mode => api.deriveGroups(list, workspaceRows, archiveIds, { expandedGroups: ['mixed', 'empty', ''], archiveFilter: mode });
  assert.deepEqual(Array.from(groups('hidden'), row => row.key), ['mixed', 'empty']);
  assert.deepEqual(Array.from(groups('archived'), row => row.key), ['mixed', ''], 'archive-only hides empty workspaces and includes loose archived sessions');
  assert.equal(groups('archived')[0].sessionCount, 2);
  assert.equal(groups('archived')[0].sessions.every(row => row.archived), true);
  const remote = { query: 'record', status: 'ready', items: [{ sessionId: 'archived', snippet: 'matched body' }, { sessionId: 'active', snippet: 'active body' }], hasMore: false };
  assert.deepEqual(ids(api.deriveSearchResults(list, workspaceRows, 'record', archiveIds, remote, 100, 'archived').items), ['archived', 'loose']);
  assert.deepEqual(ids(api.deriveSearchResults(list, workspaceRows, 'body-only', archiveIds, remote, 100, 'archived').items), ['archived']);
  assert.equal(JSON.stringify(list), untouched, 'derivations preserve the complete authoritative session list');

  const runtimeSource = fs.readFileSync(path.join(path.dirname(workspacePath), 'client-runtime.js'), 'utf8');
  const runtimeStart = runtimeSource.indexOf('var WorkspaceRuntime = class');
  const runtimeContext = { recentWorkspace: () => undefined };
  assert.notEqual(runtimeStart, -1);
  vm.runInNewContext(runtimeSource.slice(runtimeStart, runtimeSource.indexOf('\n\t\t/** Stable tie-breaking', runtimeStart)), runtimeContext);
  const workspacesRuntime = Object.create(runtimeContext.WorkspaceRuntime.prototype);
  let selectedId = 'archived', archiveSnapshot = [], clearCalls = 0;
  Object.assign(workspacesRuntime, {
    manager: { getSnapshot: () => ({ items: workspaceRows, archivedSessionIds: archiveIds, phase: 'ready' }) },
    sessions: { list: { getSnapshot: () => ({ ...list, current: selectedId }) }, clear: () => { selectedId = undefined; clearCalls++; } },
    list: { getSnapshot: () => ({ archivedSessionIds: archiveSnapshot }), set: value => { archiveSnapshot = value.archivedSessionIds; } }
  });
  workspacesRuntime.project();
  assert.equal(clearCalls, 1, 'a newly archived current session still closes as before');
  selectedId = 'archived';
  workspacesRuntime.project();
  assert.equal(clearCalls, 1, 'explicitly opening an already archived session remains selected');
  assert.equal(selectedId, 'archived');

  const spec = api.createWorkspaceViewStore();
  assert.equal(spec.persist, 'dsh.workspace.view.v5', 'existing viewing preferences are retained');
  assert.equal(spec.init().archiveFilter, 'hidden');
  let view = { ...spec.init(), groupBy: 'flat', orderBy: 'manual', sessionOrderByAccount: { __flat_session_order__: ['loose', 'active', 'archived', 'blank-archived'] } };
  const mutations = [];
  const t = (key, args) => String(api.zh[key] ?? key).replace(/\{([^}]+)\}/g, (_, name) => args?.[name] ?? '');
  let rerender;
  const actions = Object.fromEntries(Object.entries(spec.actions).map(([name, action]) => [name, (...args) => {
    action(view, ...args); rerender();
  }]));
  const browserProps = {
    wide: true, expandSidebar() {}, useSessions: select => select(list),
    useWorkspaces: select => select({ items: workspaceRows, phase: 'ready', archivedSessionIds: archiveIds }),
    useStore: select => select(view), actions, useDirectoryFlow: select => select(false), renderSlot: () => null,
    startSession() {}, open() {}, forkSession() {}, insertSessionBefore: async () => {}, insertWorkspaceBefore: async () => {},
    renameWorkspace: async () => {}, deleteWorkspace: async () => {}, createWorkspace: async () => {}, searchSessions: async () => remote,
    searchResultLimit: 100, t,
    archiveSession: async id => { mutations.push(['archive', id]); archiveIds = [...archiveIds, id]; rerender(); },
    unarchiveSession: async id => { mutations.push(['unarchive', id]); archiveIds = archiveIds.filter(value => value !== id); rerender(); }
  };
  rerender = () => root.render(React.createElement(api.WorkspaceBrowser, browserProps));
  await act(rerender);
  const visibleIds = () => [...document.querySelectorAll('[data-workspace-session-id]')].map(row => row.dataset.workspaceSessionId);
  const chooseFilter = async label => {
    await act(() => document.querySelector('button[aria-label="视图选项"]').click());
    await act(() => [...document.querySelectorAll('[role=menuitemradio]')].find(button => button.textContent === label).click());
  };
  assert.deepEqual(visibleIds(), ['active']);
  await chooseFilter('全部对话');
  assert.equal(view.archiveFilter, 'all');
  assert.deepEqual(visibleIds(), ['loose', 'active', 'archived', 'blank-archived'], 'switching filters preserves the full manual order');
  await chooseFilter('仅显示已归档');
  assert.deepEqual(visibleIds(), ['loose', 'archived', 'blank-archived']);
  window.localStorage.setItem(spec.persist, JSON.stringify(view));
  await act(() => root.render(null));
  view = JSON.parse(window.localStorage.getItem(spec.persist));
  await act(rerender);
  assert.deepEqual(visibleIds(), ['loose', 'archived', 'blank-archived'], 'archive preference survives a persisted-view remount');
  await act(() => document.querySelector('[data-workspace-session-id="archived"] button').click());
  assert.ok([...document.querySelectorAll('[role=menuitemradio]')].some(button => button.textContent === '恢复会话'));
  await act(() => [...document.querySelectorAll('[role=menuitemradio]')].find(button => button.textContent === '恢复会话').click());
  assert.deepEqual(mutations, [['unarchive', 'archived']], 'archived-row action invokes restore rather than archive');
  assert.deepEqual(visibleIds(), ['loose', 'blank-archived'], 'restored session leaves archive-only view');
  await chooseFilter('隐藏已归档');
  assert.deepEqual(visibleIds(), ['active', 'archived']);
  assert.equal(JSON.stringify(list), untouched, 'filtering and restoring never discard list entries');
  await act(() => root.unmount()); dom.window.close();
  console.log('PASS browser/workspace: HTTP local URLs, safe embedding, reload/errors, lazy app menu, archive modes/search/workspace filtering, persisted defaults, manual order, and restore routing');
})().catch(error => { console.error(error); process.exitCode = 1; root.unmount(); dom.window.close(); });
