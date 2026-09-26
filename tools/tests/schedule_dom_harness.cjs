const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const { JSDOM } = require(path.join(modules, 'jsdom'));
const dom = new JSDOM('<main></main>', { url: 'http://localhost/', pretendToBeVisual: true });
Object.assign(globalThis, { window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
const React = require(path.join(modules, 'react')), Client = require(path.join(modules, 'react-dom/client'));

const future = hours => new Date(Date.now() + hours * 3600e3).toISOString();
let tasks = [
  { id: 'task-daily', sessionId: 'session-one', title: 'Morning brief', prompt: 'Summarize the news', rule: { kind: 'daily', time: '09:00', timeZone: 'UTC' }, status: 'active', origin: 'user', createdAt: '2026-09-01T00:00:00.000Z', updatedAt: '2026-09-01T00:00:00.000Z', nextRunAt: future(20), historyCount: 1, lastDelivery: { occurrenceAt: '2026-09-25T09:00:00.000Z', deliveredAt: '2026-09-25T09:00:01.000Z', outcome: 'delivered', messageId: 'msg-1', prompt: 'Summarize the news' } },
  { id: 'task-weekly', sessionId: 'session-two', title: 'Weekly report', prompt: 'Write the report', rule: { kind: 'weekly', time: '18:30', weekdays: [1, 5], timeZone: 'UTC' }, status: 'active', origin: 'agent', createdAt: '2026-09-02T00:00:00.000Z', updatedAt: '2026-09-02T00:00:00.000Z', nextRunAt: future(50), historyCount: 0 },
  { id: 'task-cron', sessionId: 'session-one', title: 'Paused cron', prompt: 'Check CI', rule: { kind: 'cron', expression: '0 9 * * 1-5', timeZone: 'UTC' }, status: 'inactive', origin: 'user', createdAt: '2026-09-03T00:00:00.000Z', updatedAt: '2026-09-03T00:00:00.000Z', historyCount: 0 },
];
let revision = 1;
const calls = [];
const waiters = [];
let nextUpdateConflict = false;
function respond(value, status = 200) { return { ok: status < 400, status, json: async () => value }; }
async function fetch(url, options) {
  const body = options?.body ? JSON.parse(options.body) : {};
  if (url.startsWith('/api/')) {
    if (url === '/api/workspace.list') return respond({ type: 'server-response', result: { ok: true, value: { items: [{ workspaceId: 'ws-1', title: 'Project', path: '\\\\?\\C:\\work\\project' }], archivedSessionIds: [] } } });
    throw Error('unexpected rpc ' + url);
  }
  const operation = url.replace('/__dsh-schedule/', '');
  calls.push([operation, body]);
  if (operation === 'wait') return new Promise((resolve, reject) => { waiters.push(resolve); options.signal?.addEventListener('abort', () => reject(Object.assign(Error('aborted'), { name: 'AbortError' }))); });
  if (operation === 'catalog') return respond({ tasks: tasks.filter(task => !body.sessionId || task.sessionId === body.sessionId), revision, error: null, hostTimeZone: 'UTC' });
  if (operation === 'update') {
    if (nextUpdateConflict) { nextUpdateConflict = false; return respond({ error: 'changed', code: 'conflict' }, 409); }
    const task = tasks.find(row => row.id === body.id);
    Object.assign(task, { title: body.title, prompt: body.prompt, updatedAt: new Date().toISOString() }, body.rule ? { rule: body.rule } : {});
    revision++;
    return respond({ task });
  }
  if (operation === 'setActive') { const task = tasks.find(row => row.id === body.id); task.status = body.active ? 'active' : 'inactive'; revision++; return respond({ task }); }
  if (operation === 'delete') { tasks = tasks.filter(row => row.id !== body.id); revision++; return respond({ id: body.id, deleted: true }); }
  if (operation === 'history') return respond({ records: [{ occurrenceAt: '2026-09-25T09:00:00.000Z', deliveredAt: '2026-09-25T09:00:01.000Z', outcome: 'delivered', messageId: 'msg-1', prompt: 'Summarize the news' }, { occurrenceAt: '2026-09-24T09:00:00.000Z', deliveredAt: '2026-09-24T09:00:01.000Z', outcome: 'failed', error: 'session unavailable', prompt: 'Summarize the news' }], total: 2, hasMore: false, earlierRecordsPruned: false, retentionDays: 30, retentionRecords: 200 });
  if (operation === 'runNow') return respond({ delivery: {} });
  if (operation === 'create') { const task = { ...tasks[0], id: 'task-new', sessionId: body.sessionId, title: body.title || body.prompt, prompt: body.prompt, rule: body.rule, historyCount: 0, lastDelivery: undefined, origin: 'user' }; tasks = [...tasks, task]; revision++; return respond({ task }); }
  throw Error('unexpected operation ' + operation);
}

let listeners = new Set();
let snapshot = { ids: ['session-one', 'session-two'], byId: { 'session-one': { id: 'session-one', displayTitle: 'News desk' }, 'session-two': { id: 'session-two', displayTitle: 'Reports' } }, current: 'session-one' };
const created = [];
const opened = [];
const sessions = {
  list: { subscribe: fn => { listeners.add(fn); return () => listeners.delete(fn); }, getSnapshot: () => snapshot },
  open: id => opened.push(id),
  create: async opts => { created.push(opts); return 'session-new'; },
  binding: () => ({ session: { rename: async () => ({ ok: true }) } }),
};
const panels = [];
const layout = { selectPanel: id => panels.push(id) };

let plugin;
const sandbox = { window: { __ModuleLoader__: { load: spec => { plugin = spec.factory(name => { if (name === 'react') return React; throw Error(name); }); } } }, document, fetch, navigator: { clipboard: { writeText: async () => {} } }, confirm: () => true, setTimeout, clearTimeout, Intl, Date, console, AbortController, Error, JSON, Math, Number, String, Object, Array, Set, Promise };
sandbox.window.confirm = () => true;
sandbox.window.addEventListener = dom.window.addEventListener.bind(dom.window);
sandbox.window.removeEventListener = dom.window.removeEventListener.bind(dom.window);
vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../../web/dist/plugins/ui-schedule.js'), 'utf8'), sandbox);
const t = key => plugin.test.en[key] || key;
const root = Client.createRoot(document.querySelector('main'));
async function act(fn) { await React.act(async () => { await fn(); await new Promise(resolve => setImmediate(resolve)); }); }
const button = text => [...document.querySelectorAll('button')].find(node => node.textContent.trim() === text || node.textContent.trim().startsWith(text));
const fill = (node, text) => { const proto = node.tagName === 'TEXTAREA' ? window.HTMLTextAreaElement.prototype : node.tagName === 'SELECT' ? window.HTMLSelectElement.prototype : window.HTMLInputElement.prototype; Object.getOwnPropertyDescriptor(proto, 'value').set.call(node, text); node.dispatchEvent(new window.Event(node.tagName === 'SELECT' ? 'change' : 'input', { bubbles: true })); };

(async () => {
  // Rule labels cover every kind without a browser-specific zone suffix for UTC rules.
  const label = plugin.test.ruleLabel;
  assert.equal(label({ kind: 'every', everySeconds: 7200 }, t), 'Every 2 hours');
  assert.equal(label({ kind: 'weekly', time: '18:30', weekdays: [1, 5], timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone }, t), 'Mon, Fri 18:30');
  // Values built inside the plugin sandbox come from another realm; compare their JSON.
  assert.equal(JSON.stringify(plugin.test.ruleFromDraft({ ...plugin.test.draftFromRule(null), kind: 'every', interval: '15', unit: '60' })), JSON.stringify({ kind: 'every', everySeconds: 900 }));

  await act(() => root.render(React.createElement(plugin.test.TaskManager, { t, sessions, layout })));
  const cards = () => [...document.querySelectorAll('.dshSchedCard')];
  assert.equal(cards().length, 3);
  assert.equal(cards()[2].dataset.status, 'inactive', 'inactive tasks sort after active ones');
  assert.match(cards()[0].textContent, /Morning brief/);
  assert.match(cards()[0].textContent, /News desk/, 'cards name their conversation');
  await act(() => button('Inactive').click());
  assert.equal(cards().length, 1);
  await act(() => button('All').click());

  // Detail: edit and save with the observed updatedAt; a conflict reloads and keeps the page usable.
  await act(() => cards()[0].click());
  const detail = () => document.querySelector('.dshSchedDetail');
  assert.ok(detail(), 'detail opens');
  assert.match(detail().textContent, /Created by you/);
  const title = detail().querySelector('.dshSchedForm input');
  await act(() => fill(title, 'Morning brief v2'));
  assert.match(detail().textContent, /Unsaved changes/);
  await act(() => button('Save').click());
  const update = calls.filter(([name]) => name === 'update').at(-1)[1];
  assert.equal(update.expectedUpdatedAt, '2026-09-01T00:00:00.000Z');
  assert.equal(update.title, 'Morning brief v2');
  assert.equal(update.rule, undefined, 'an unchanged rule is not resent');
  nextUpdateConflict = true;
  await act(() => fill(detail().querySelector('.dshSchedForm input'), 'stale edit'));
  await act(() => button('Save').click());
  assert.match(detail().textContent, /changed elsewhere/);

  // Toggle, records, open conversation, run now.
  await act(() => detail().querySelector('input[role=switch]').click());
  assert.deepEqual(calls.filter(([name]) => name === 'setActive').at(-1)[1], { id: 'task-daily', sessionId: 'session-one', active: false });
  await act(() => button('Runs').click());
  assert.match(detail().textContent, /session unavailable/);
  assert.equal(detail().querySelectorAll('.dshSchedRecords li').length, 2);
  await act(() => button('Open conversation').click());
  assert.deepEqual(opened, ['session-one']);
  assert.deepEqual(panels, [null], 'opening the conversation leaves the schedule panel');
  await act(() => button('Rule').click());
  await act(() => button('Run now').click());
  assert.equal(calls.filter(([name]) => name === 'runNow').length, 1);

  assert.equal(detail().querySelector('[role=tab][aria-selected=true]').textContent.trim().startsWith('Runs'), true, 'run now shows the records');

  // Delete after confirmation.
  await act(() => button('Rule').click());
  await act(() => button('Delete').click());
  assert.deepEqual(calls.filter(([name]) => name === 'delete').at(-1)[1], { id: 'task-daily', sessionId: 'session-one' });
  assert.equal(detail(), null);

  // Create into a new conversation of a workspace; the verbatim path prefix is dropped.
  await act(() => button('+ New task').click());
  const dialog = document.querySelector('.dshSchedDialog');
  assert.ok(dialog);
  await act(() => fill(dialog.querySelector('textarea'), 'Check the build every morning'));
  await act(() => [...dialog.querySelectorAll('[role=radio]')].find(node => node.textContent === 'Weekly').click());
  const select = dialog.querySelector('select');
  const newOption = [...select.options].find(option => option.value.startsWith('new:'));
  assert.equal(newOption.value, 'new:C:\\work\\project');
  await act(() => fill(select, newOption.value));
  await act(() => dialog.dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })));
  assert.equal(JSON.stringify(created), JSON.stringify([{ cwd: 'C:\\work\\project' }]));
  const create = calls.filter(([name]) => name === 'create').at(-1)[1];
  assert.equal(create.sessionId, 'session-new');
  assert.equal(create.rule.kind, 'weekly');
  assert.deepEqual(create.rule.weekdays, [1, 2, 3, 4, 5]);
  assert.equal(document.querySelector('.dshSchedDialog'), null);
  assert.equal(document.querySelector('.dshSchedDetail h2').textContent, 'Check the build every morning');

  // A change notification reloads the catalog.
  const before = calls.filter(([name]) => name === 'catalog').length;
  revision++;
  await act(() => waiters.shift()?.({ ok: true, status: 200, json: async () => ({ revision }) }));
  await act(() => new Promise(resolve => setTimeout(resolve, 10)));
  assert.ok(calls.filter(([name]) => name === 'catalog').length > before, 'wait wakes a catalog reload');

  await act(() => root.unmount());
  assert.equal(listeners.size, 0);

  // Header clock: only active tasks of this conversation, opening the page on the task.
  tasks = tasks.filter(task => task.sessionId !== 'session-new');
  const headerRoot = Client.createRoot(document.querySelector('main'));
  await act(() => headerRoot.render(React.createElement(plugin.test.ScheduleHeaderAction, { sessionId: 'session-two', t, layout })));
  let header = document.querySelector('.dshSchedHeaderButton');
  assert.ok(header, 'header shows active tasks');
  assert.equal(header.getAttribute('aria-label'), 'Scheduled tasks 1');
  await act(() => header.click());
  assert.equal(panels.at(-1), 'schedule');
  await act(() => headerRoot.render(React.createElement(plugin.test.ScheduleHeaderAction, { sessionId: 'session-empty', t, layout })));
  await act(() => new Promise(resolve => setTimeout(resolve, 10)));
  assert.equal(document.querySelector('.dshSchedHeaderButton'), null, 'no clock without active tasks');
  await act(() => headerRoot.unmount());
  dom.window.close();
  console.log('PASS schedule UI: filters, labels, conflict-safe edit, toggle, records, run now, delete, create into a new conversation and live reload');
})().catch(error => { console.error(error); process.exitCode = 1; dom.window.close(); });
