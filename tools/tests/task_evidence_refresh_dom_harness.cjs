const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const { JSDOM } = require(path.join(modules, 'jsdom'));
const dom = new JSDOM('<main id="root"></main>', { pretendToBeVisual: true });
Object.assign(global, { window: dom.window, document: dom.window.document, IS_REACT_ACT_ENVIRONMENT: true });
const React = require(path.join(modules, 'react'));
const root = require(path.join(modules, 'react-dom/client')).createRoot(document.getElementById('root'));
const manual = { checkId: 'review', status: 'passed', evidenceRefs: ['user-confirmation'], inputIdentity: 'same-input' };
let task = { taskId: 'report', revision: 7, state: 'completed', spec: { objective: 'Report', constraints: [], expectedOutputs: ['report.docx'], acceptanceChecks: [
  { id: 'doc', description: 'Office content', checker: { kind: 'office_package', path: 'report.docx' } },
  { id: 'review', description: 'Layout reviewed', checker: { kind: 'manual', reason: 'Inspect pages' } }
] }, steps: [], outputIdentities: { 'report.docx': 'same-input' }, acceptanceResults: [
  { checkId: 'doc', status: 'passed', evidenceRefs: [], inputIdentity: 'same-input' }, manual
] };
let blockers = ['Checker version requires revalidation','Goal requirements changed or the linked goal is no longer current; old acceptance does not satisfy the current goal','Acceptance doc has not passed','Output report.docx has no verified identity','Step call-old is Unknown'], Component;
const requests = [], pending = [];
const response = value => ({ ok: true, status: 200, text: async () => JSON.stringify(value) });
vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../../web/src/runtime-plugins/ui-task-execution.js'), 'utf8'), {
  document, AbortController, crypto: require('node:crypto').webcrypto,
  fetch: async (_, options) => {
    const input = JSON.parse(options.body); requests.push(input);
    if (input.action === 'list') return response({ tasks: input.sessionId === 'session-a' ? [task] : [] });
    if (input.action === 'get') return response({ task, blockers, recovery: [] });
    if (input.action === 'refresh_evidence') return new Promise(resolve => pending.push({ input, resolve }));
    if (input.action === 'stop_validation') return response({ stopped: true });
    throw new Error('Unexpected action: ' + input.action);
  },
  window: { __ModuleLoader__: { load: value => { Component = value.factory(() => React).test.TaskExecutionView; } } }
});
const button = label => [...document.querySelectorAll('button')].find(button => button.textContent === label);
async function settle(fn) { await React.act(async () => { fn?.(); await new Promise(resolve => setImmediate(resolve)); }); }
(async () => {
  await settle(() => root.render(React.createElement(Component, { sessionId: 'session-a' })));
  assert.match(document.body.textContent, /历史已完成 · 当前证据需复核/);
  assert.match(document.body.textContent, /目标已变化，旧验收不再适用/);
  assert.match(document.body.textContent, /验收项“Office content”尚未通过/);
  assert.match(document.body.textContent, /产物“report.docx”尚无已核验版本/);
  assert.match(document.body.textContent, /步骤“call-old”：效果未知/);
  assert.match(document.body.textContent, /Checker version requires revalidation/, 'unrecognized backend details remain visible');
  await settle(() => { button('刷新验收证据').click(); button('刷新验收证据').click(); });
  assert.equal(pending.length, 1, 'same-frame double submit cannot start two validations');
  assert.equal(pending[0].input.revision, 7);
  assert.ok(pending[0].input.idempotencyKey);
  assert.equal(button('停止验收').disabled, false, 'Stop remains available while validation is pending');
  await settle(() => button('停止验收').click());
  assert.equal(requests.filter(request => request.action === 'stop_validation').length, 1);
  assert.match(document.body.textContent, /停止请求已处理/);
  await settle(() => button('刷新验收证据').click());
  assert.equal(pending.length, 2);
  await settle(() => pending[0].resolve(response({ task: { ...task, revision: 99 } })));
  assert.equal(button('刷新验收证据').disabled, true, 'late old completion must not unlock a newer request');

  task = { ...task, revision: 8, acceptanceRefresh: { results: [
    { checkId: 'doc', status: 'failed', evidenceRefs: [], failureReason: 'Current checker rejected input' }, manual
  ] } };
  blockers = ['Current checker rejected input'];
  await settle(() => pending[1].resolve(response({ task })));
  assert.match(document.body.textContent, /当前证据需复核/);
  assert.match(document.body.textContent, /未通过/);
  assert.match(document.body.textContent, /Current checker rejected input/);
  assert.equal(button('核对并确认此项'), undefined, 'existing manual consent is not requested again');

  await settle(() => button('刷新验收证据').click());
  task = { ...task, revision: 9, acceptanceRefresh: { results: task.acceptanceResults } };
  blockers = [];
  await settle(() => pending[2].resolve(response({ task })));
  assert.doesNotMatch(document.body.textContent, /当前证据需复核/);
  assert.match(document.body.textContent, /已验收完成/);
  assert.deepEqual(task.acceptanceResults[1], manual);

  await settle(() => button('刷新验收证据').click());
  await settle(() => root.render(React.createElement(Component, { sessionId: 'session-b' })));
  await settle(() => pending[3].resolve({ ok: false, status: 400, text: async () => JSON.stringify({ error: 'old session error' }) }));
  assert.doesNotMatch(document.body.textContent, /old session error/);
  assert.match(document.body.textContent, /当前会话尚无验收契约/);
  await settle(() => root.unmount()); dom.window.close();
  console.log('PASS task evidence: refresh identity, preserved manual consent, failed current evidence, cancellable checks and late-result isolation');
})().catch(error => { console.error(error); process.exitCode = 1; });
