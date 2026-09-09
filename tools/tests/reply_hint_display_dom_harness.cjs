const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), os = require('node:os'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const { JSDOM } = require(path.join(modules, 'jsdom'));
const dom = new JSDOM('<!doctype html><main id="root"></main>', { url: 'http://fixture.invalid', pretendToBeVisual: true });
Object.assign(global, { window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
const React = require(path.join(modules, 'react')), jsx = require(path.join(modules, 'react/jsx-runtime')), Client = require(path.join(modules, 'react-dom/client'));
const plugins = path.resolve(__dirname, '../../web/dist/plugins'), assets = path.resolve(__dirname, '../../web/dist/assets');
const temporary = fs.mkdtempSync(path.join(process.env.DSH_TEST_TEMP_DIR || os.tmpdir(), 'dsh-hint-display-'));
const file = path.join(temporary, 'settings.json');
fs.writeFileSync(file, JSON.stringify({ revision: 7, value: { busyEnter: 'steer', hintDisplay: 'text' } }));
const shell = fs.readFileSync(path.join(assets, fs.readdirSync(assets).find(name => /^index-.*\.js$/.test(name))), 'utf8');
const name = /DisclosureRow:([\w$]+)/.exec(shell)[1], start = shell.indexOf(`function ${name}(`), end = shell.indexOf('}const ', start) + 1;
const native = { f: jsx, R: React, Fn: {}, ye: (...values) => values.filter(Boolean).join(' '), Bl: () => React.createElement('svg', { 'data-native-chevron': true }) };
vm.runInNewContext(shell.slice(start, end) + `;this.DisclosureRow=${name};`, native);
function store(initial) {
  let value = initial; const listeners = new Set();
  const set = next => { value = next; for (const listener of listeners) listener(); };
  return { getSnapshot: () => value, subscribe: listener => { listeners.add(listener); return () => listeners.delete(listener); }, set, update: change => { const next = { ...value }; change(next); set(next); } };
}
const primitives = new Proxy({
  DisclosureRow: native.DisclosureRow,
  StateDot: ({ state }) => React.createElement('span', { 'data-state-dot': state }),
  Tooltip: ({ children }) => children,
  MarkdownText: ({ text }) => React.createElement('p', null, text),
  Menu: ({ anchor, open, items, onSelect, selectedId }) => React.createElement(React.Fragment, null, anchor, open && React.createElement('div', { role: 'menu' }, items.map(item => React.createElement('button', { key: item.id, role: 'menuitemradio', 'aria-checked': item.id === selectedId, onClick: () => onSelect(item.id) }, item.label))))
}, { get: (target, key) => target[key] ?? (props => React.createElement('svg', { 'data-native-icon': key, 'aria-hidden': true, width: props.size ?? 14, height: props.size ?? 14 })) });
function load(name, names) {
  let exported;
  vm.runInNewContext(fs.readFileSync(path.join(plugins, name), 'utf8').replace('return module.exports;', `exports.test={${names}};return module.exports;`), {
    document, console, setTimeout, clearTimeout, setInterval, clearInterval,
    requestAnimationFrame: callback => setTimeout(callback, 0), cancelAnimationFrame: clearTimeout,
    window: { __ModuleLoader__: { load: definition => { exported = definition.factory(id => id === 'react' ? React : id === 'react/jsx-runtime' ? jsx : id.endsWith('ui-primitives') ? primitives : id === '@deepseek-ai/cordis' ? { Service: class {} } : { createSnapshotStore: store }); } } }
  });
  return exported.test;
}
const ui = load('ui-conversation.js', 'ReplyHintPreference,ReplyHintDisplayRow,ReasoningRow,TurnStatus,TodoPanel,GenericCommandCard,zh,en');
const tools = load('ui-tool.js', 'GenericToolCard');
const subagents = load('ui-subagent.js', 'SubagentToolRow,zh,en');
const cordis = load('ui-cordis.js', 'CordisDefineRow,CordisRunRow,CordisActionRow,zh,en');
const settings = load('ui-settings.js', 'SettingsScopeController');
const translate = dictionary => (key, values = {}) => Object.entries(values).reduce((text, [name, value]) => text.replaceAll('{' + name + '}', String(value)), dictionary[key] ?? key);
const writes = []; let fail = false;
const read = () => JSON.parse(fs.readFileSync(file, 'utf8'));
const api = { settings: {
  describe: async () => ({ result: { ok: true, value: { writable: true, namespaces: [{ ns: 'ui-conversation', ...read() }] } } }),
  mutate: async request => {
    writes.push(JSON.parse(JSON.stringify(request)));
    if (fail) return { result: { ok: false, error: { code: 'write-failed', message: 'fixture disk unavailable' } } };
    const durable = read(); assert.equal(request.expectedRevision, durable.revision);
    assert.equal(request.ns, 'ui-conversation'); assert.deepEqual(Array.from(request.ops[0].path), ['hintDisplay']);
    assert.ok(['text', 'icons'].includes(request.ops[0].value));
    durable.value.hintDisplay = request.ops[0].value; durable.revision++;
    fs.writeFileSync(file, JSON.stringify(durable));
    return { result: { ok: true, value: { ns: 'ui-conversation', ...durable } } };
  }
} };
const createScope = () => new settings.SettingsScopeController(api, { namespace: 'ui-conversation', decode: value => value });
let scope = createScope(), preference = new ui.ReplyHintPreference(scope);
const root = Client.createRoot(document.getElementById('root')), flush = () => new Promise(resolve => setImmediate(resolve));
const act = fn => React.act(async () => { await fn(); await flush(); });
const snapshot = { byId: {}, subagentsByParent: {} }, sessions = { getSnapshot: () => snapshot, subscribe: () => () => {} };
const noop = () => {}, inspections = [];
const t = translate(ui.zh);
function Scene() {
  const useHintDisplay = selector => React.useSyncExternalStore(preference.store.subscribe, () => selector(preference.store.getSnapshot()), () => selector(preference.store.getSnapshot()));
  return React.createElement(React.Fragment, null,
    React.createElement(ui.ReplyHintDisplayRow, { useHintDisplay, setHintDisplay: mode => preference.set(mode), t }),
    React.createElement(tools.GenericToolCard, { toolName: 'fixture_tool', block: { kind: 'tool-result', callId: 'tool', call: { argsRaw: '{"value":"PARAMETER_UNCHANGED"}' }, content: [{ type: 'text', text: 'MODEL_OUTPUT_UNCHANGED' }], subCalls: [] }, t }),
    React.createElement(ui.GenericCommandCard, { node: { name: 'fixture_command', outcome: { kind: 'ok', text: 'COMMAND_RESULT\nSECOND_LINE' } }, t }),
    React.createElement(cordis.CordisDefineRow, { callId: 'cordis', block: { kind: 'tool-result', call: { argsRaw: '{"name":"fixture","purpose":"PURPOSE_UNCHANGED"}' }, content: [{ type: 'text', text: 'CORDIS_RESULT' }] }, useInventory: selector => selector({ rows: [], removed: new Set() }), useLoaded: selector => selector(new Set()), t: translate(cordis.zh) }),
    React.createElement(cordis.CordisRunRow, { callId: 'cordis-run', block: { kind: 'tool-result', call: { argsRaw: '{}' }, content: [{ type: 'text', text: 'CORDIS_RUN_RESULT' }] }, inspect: () => inspections.push('run'), renderSlot: () => null, useInventory: selector => selector({ rows: [], removed: new Set() }), useLoaded: selector => selector(new Set()), useRunCards: selector => selector(new Map()), useActiveRuns: selector => selector(new Map()), onObserveRunCard: noop, t: translate(cordis.zh) }),
    ...['cordis_stop', 'cordis_undefine'].map(toolName => React.createElement(cordis.CordisActionRow, { key: toolName, callId: toolName, toolName, block: { kind: 'tool-result', call: { argsRaw: '{}' }, content: [{ type: 'text', text: 'CORDIS_ACTION_RESULT' }] }, inspect: () => inspections.push(toolName), t: translate(cordis.zh) })),
    React.createElement(ui.ReasoningRow, { text: 'REASONING_UNCHANGED', running: false, t }),
    React.createElement(ui.TodoPanel, { todos: [{ content: 'TASK_DESCRIPTION_UNCHANGED', status: 'in_progress' }], useSession: selector => selector({ running: false, subagent: null, openState: 'open' }), updateTodos: async () => {}, cancelTurn: noop, notify: noop, t }),
    React.createElement(subagents.SubagentToolRow, { parentSessionId: 'parent', block: { kind: 'tool-result', callId: 'child', call: { argsRaw: '{"description":"SUBTASK_UNCHANGED"}' }, content: [{ type: 'text', text: 'SUBTASK_RESULT' }] }, sessionsStore: sessions, refresh: noop, loadProgress: async () => ({ events: [] }), openChild: noop, t: translate(subagents.zh) }),
    React.createElement(ui.TurnStatus, { startTime: Date.now(), toolActive: false, phase: 'text', t })
  );
}
const style = node => dom.window.getComputedStyle(node);
function assertMode(mode) {
  assert.equal(document.documentElement.dataset.replyHintDisplay, mode);
  const task = document.querySelector('button[title="任务"]');
  assert.ok(task.querySelector('svg'), 'task header retains its original icon in either preference');
  assert.equal(task.querySelector('.dshReplyHintIcon,.dshReplyHintLabel'), null, 'task decoration is independent of reply hints');
  assert.notEqual(style(task.querySelector('svg')).display, 'none');
  const labels = [...document.querySelectorAll('.dshReplyHintLabel')], icons = [...document.querySelectorAll('.dshReplyHintIcon')];
  assert.ok(labels.length >= 5 && icons.length >= 3, `tools, thinking, task/subtask and activity hints participate: labels=${labels.length} icons=${icons.length}`);
  for (const icon of icons) assert.equal(style(icon).display, mode === 'icons' ? 'inline-flex' : 'none');
  for (const label of labels) assert.equal(style(label).position === 'absolute', mode === 'icons', 'icon mode does not leave the text label visually alongside its icon');
  for (const leading of document.querySelectorAll('.dshReplyHintLeading')) {
    assert.notEqual(style(leading).display, 'none', 'the native leading still owns its interactive disclosure arrow');
    assert.ok(leading.querySelector('[data-native-chevron]'), 'native expanded/hover chevron remains mounted');
  }
  assert.ok(document.querySelector('.dshReplyHintIcon svg'), 'symbols use native SVG components');
  assert.ok(document.querySelector('.dshReplyHintIcon[title]') || document.querySelector('.dsh-subagent-tool-trigger[title]'), 'icon-only labels remain discoverable by tooltip');
  assert.ok(document.querySelector('[title="fixture_command"] .dshReplyHintLabel'), 'generic command rows use the display preference');
  assert.ok(document.querySelector('[data-tool=cordis_define] .dshReplyHintLabel'), 'the keyed Cordis row cannot bypass the display preference');
  for (const name of ['cordis_run', 'cordis_stop', 'cordis_undefine']) {
    assert.ok(document.querySelector(`[data-tool=${name}] .dshReplyHintLabel`), `${name} title participates in the display mode`);
    assert.equal(document.querySelector(`[data-tool=${name}] button`).getAttribute('aria-label'), '调用详情', `${name} inspection label is localized`);
  }
  assert.match(document.body.textContent, /PARAMETER_UNCHANGED/);
  assert.match(document.body.textContent, /TASK_DESCRIPTION_UNCHANGED/);
  assert.match(document.body.textContent, /SUBTASK_UNCHANGED/);
}
async function main() {
  await scope.load(); await act(() => root.render(React.createElement(Scene)));
  await act(() => document.querySelector('button[title="任务"]').click());
  assertMode('text'); assert.equal(writes.length, 0, 'default rendering does not rewrite persisted settings');
  for (const name of ['cordis_run', 'cordis_stop', 'cordis_undefine']) await act(() => document.querySelector(`[data-tool=${name}] button`).click());
  assert.deepEqual(inspections, ['run', 'cordis_stop', 'cordis_undefine'], 'localized inspection controls preserve their original action');
  await act(() => document.querySelector('[data-disclosure-row]').click());
  assert.equal(document.querySelector('[data-disclosure-row]').getAttribute('aria-expanded'), 'true');
  assert.match(document.body.textContent, /MODEL_OUTPUT_UNCHANGED/);
  await act(() => document.querySelector('[data-reply-hint-setting] button').click());
  assert.deepEqual([...document.querySelectorAll('[role=menuitemradio]')].map(node => node.textContent), ['图标显示', '文字显示']);
  await act(() => document.querySelector('[role=menuitemradio]').click());
  await scope.tail; await act(flush); assertMode('icons');
  assert.equal(document.querySelector('[data-disclosure-row]').getAttribute('aria-expanded'), 'true', 'changing display preference preserves disclosure state');
  assert.equal(read().value.hintDisplay, 'icons'); assert.equal(read().value.busyEnter, 'steer', 'the other conversation setting is preserved');
  await assert.rejects(preference.set('both'), /Invalid/); assert.equal(writes.length, 1, 'no third display mode is persisted');
  fail = true;
  await act(() => preference.set('text')); assertMode('icons');
  assert.match(document.querySelector('[role=alert]').textContent, /fixture disk unavailable/);
  fail = false;
  await act(() => root.render(null)); preference.dispose(); await scope.dispose();
  scope = createScope(); preference = new ui.ReplyHintPreference(scope); await scope.load();
  await act(() => root.render(React.createElement(Scene))); await act(() => document.querySelector('button[title="任务"]').click()); assertMode('icons');
  await act(() => preference.set('text')); assertMode('text');
  assert.equal(read().value.hintDisplay, 'text');
  await act(() => root.unmount()); preference.dispose(); await scope.dispose();
  assert.equal(document.documentElement.hasAttribute('data-reply-hint-display'), false, 'plugin teardown removes its visual preference owner');
  console.log('PASS reply hints: two native-menu modes; text default; immediate SVG/text switch; accessible tooltip/name; native disclosure leading retained; real SettingsScope persistence/reload and write rollback; untouched content');
}
main().catch(async error => { console.error(error); process.exitCode = 1; await act(() => root.unmount()); preference.dispose(); await scope.dispose(); }).finally(() => { dom.window.close(); fs.rmSync(temporary, { recursive: true, force: true }); });
