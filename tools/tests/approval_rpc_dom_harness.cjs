"use strict";
const assert = require("node:assert/strict"), fs = require("node:fs"), path = require("node:path"), vm = require("node:vm"), crypto = require("node:crypto");
const work = path.resolve(process.argv[2]), owned = JSON.parse(fs.readFileSync(path.join(work, "ui-owned-process.json"))), fixture = JSON.parse(fs.readFileSync(path.join(work, "approval-fixtures.json")));
assert.equal(path.resolve(owned.home), path.join(work, "ui-home"));
const base = `http://127.0.0.1:${owned.port}`, modules = owned.nodeModules;
const { JSDOM } = require(path.join(modules, "jsdom"));
const dom = new JSDOM('<!doctype html><html><body><div id="root"></div></body></html>', { url: base });
Object.assign(global, { window: dom.window, document: dom.window.document, IS_REACT_ACT_ENVIRONMENT: true });
const React = require(path.join(modules, "react")), jsx = require(path.join(modules, "react/jsx-runtime")), ReactDOM = require(path.join(modules, "react-dom/client"));
const request = (target, options = {}) => fetch(new URL(target, base), { ...options, headers: { Origin: base, ...options.headers } });
async function rpc(method, payload = {}) {
  const response = await request('/api/' + method, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ type: 'client-request', rpcId: crypto.randomUUID(), method, payload }) });
  const reply = await response.json(); assert.equal(reply.result.ok, true, method + ': ' + JSON.stringify(reply)); return reply.result.value;
}
async function respond(frame, outcome, sessionId = frame.payload.sessionId) {
  const response = await request('/api/respond', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ type: 'client-response', rpcId: frame.rpcId, result: { ok: true, value: { sessionId, approvalId: frame.payload.approvalId, outcome } } }) });
  return response.json();
}
const frames = [], streams = [];
async function connect() {
  const controller = new AbortController(); streams.push(controller);
  const response = await request('/api/events.mux', { signal: controller.signal }); assert.equal(response.ok, true);
  const reader = response.body.getReader(); let buffer = ''; const decoder = new TextDecoder();
  const pump = (async () => { try { for (;;) { const { value, done } = await reader.read(); if (done) break; buffer += decoder.decode(value, { stream: true }); let end; while ((end = buffer.indexOf('\n\n')) >= 0) { const chunk = buffer.slice(0, end); buffer = buffer.slice(end + 2); const data = chunk.split('\n').filter(line => line.startsWith('data:')).map(line => line.slice(5).trimStart()).join('\n'); if (data) frames.push(JSON.parse(data)); } } } catch (error) { if (!controller.signal.aborted) throw error; } })();
  return { controller, pump };
}
async function until(predicate, label, timeout = 15000) { const end = Date.now() + timeout; while (Date.now() < end) { const value = await predicate(); if (value) return value; await new Promise(resolve => setTimeout(resolve, 25)); } throw Error(label); }
const history = async sessionId => (await rpc('session.history', { sessionId })).events.map(row => row.event);
const session = fixture.sessionIds[0], otherSession = fixture.sessionIds[1];
let root = ReactDOM.createRoot(document.getElementById('root')), Component, current = null, turns = 0;
const evidence = { origin: base, binarySha256: fixture.binarySha256, checks: [], detailsVisible: [] };
const words = { 'approval.waiting': '等待审批', 'approval.submitting': '正在提交', 'approval.reject': '拒绝', 'approval.allowOnce': '允许一次', 'approval.allowAlways': '始终允许', 'approval.rememberScope': '仅记住匹配范围', 'approval.onceOnly': '仅限一次', 'approval.detail.aria': '审批详情', 'approval.failed': '提交失败：{message}', 'approval.escalation': '{toolName} 需要审批', 'approval.targetPath': '路径：{path}' };
const t = (key, data = {}) => (words[key] || key).replace(/\{(\w+)\}/g, (_, name) => data[name] ?? name);
async function loadComponent() {
  const source = await (await request('/plugins/ui-conversation.js')).text();
  function section(start) { const at = source.indexOf(start); assert.ok(at >= 0, start); const end = source.indexOf('//#endregion', at); return source.slice(at, end); }
  const code = section('var PendingApproval = class') + section('function toolNode(node)') + section('var ApprovalPanel_module_css_default =') + section('function commandOf(call)') + '\nresult.Component = ApprovalPanel; result.commandOf = commandOf;';
  const result = {};
  vm.runInNewContext(code, { react: React, react_jsx_runtime: jsx, result, _deepseek_ai_dsh_client_runtime_client: { conversationContextKey: (_, id) => id }, _deepseek_ai_dsh_client_ui_primitives: { Button: ({ children, variant, ...props }) => React.createElement('button', props, children) } });
  Component = result.Component; evidence.componentSha256 = crypto.createHash('sha256').update(code).digest('hex');
  assert.equal(result.commandOf({argsRaw: JSON.stringify({file_path: String.raw`\\?\D:\folder\file.txt`})}), String.raw`D:\folder\file.txt`);
  assert.equal(result.commandOf({argsRaw: JSON.stringify({file_path: String.raw`\\?\UNC\server\share\file.txt`})}), String.raw`\\server\share\file.txt`);
}
async function render(frame, options = {}) {
  const events = await history(frame.payload.sessionId), calls = events.filter(event => event.type === 'tool/call');
  const nodes = new Map(calls.map(event => [event.data.callId, { kind: 'tool-call', data: { root: { callId: event.data.callId, argsRaw: event.data.arguments, name: event.data.name, subCalls: [] } } }]));
  let first = true;
  current = frame;
  const matched = { key: frame.rpcId, payload: frame.payload, sessionId: frame.payload.sessionId, respond: async result => {
    if (options.rejectFirst && first) { first = false; return respond(frame, result.value.outcome, otherSession); }
    return respond(frame, result.value.outcome);
  } };
  await React.act(async () => root.render(React.createElement(Component, { key: frame.rpcId, matched, t, useSession: selector => selector({ chat: { nodes } }) })));
  const paired = calls.find(event => event.data.callId === frame.payload.callId); assert.ok(paired, 'approval correlates with a real tool call');
  const target = JSON.parse(paired.data.arguments).file_path;
  evidence.detailsVisible.push({ tool: frame.payload.toolName, target, visible: document.body.textContent.includes(target) || document.body.textContent.includes(target.replace(/^\\\\\?\\/, '')) });
  assert.match(document.body.textContent, /拒绝/); assert.match(document.body.textContent, /允许一次/);
  assert.equal([...document.querySelectorAll('button')].find(button => button.textContent === '始终允许').disabled, !frame.payload.rememberable);
}
async function click(label) { const button = [...document.querySelectorAll('button')].find(button => button.textContent === label); assert.ok(button); await React.act(async () => { button.click(); await new Promise(resolve => setTimeout(resolve, 30)); }); }
async function resolved(frame, outcome) { await until(() => frames.find(item => item.payload?.type === 'approval/resolved' && item.payload.approvalId === frame.payload.approvalId && (!outcome || item.payload.outcome === outcome)), 'approval resolution broadcast missing'); await React.act(async () => root.render(null)); current = null; }
async function completed(expected = 'completed') { const events = await until(async () => { const value = await history(session); return value.filter(event => event.type === 'turn/end').length >= turns ? value : null; }, 'turn did not settle'); assert.equal(events.filter(event => event.type === 'turn/end').at(-1).data.reason.kind, expected); return events; }
async function start(tool, relative, marker) {
  const target = path.resolve(work, relative); assert.ok(target.startsWith(work + path.sep)); fs.mkdirSync(path.dirname(target), { recursive: true }); if (tool === 'write') fs.writeFileSync(target, 'BEFORE:' + marker);
  const index = frames.length; turns++;
  await rpc('session.prompt', { sessionId: session, content: [{ type: 'text', text: 'approval-e2e:' + JSON.stringify({ tool, path: relative, marker }) }], mode: 'queue' });
  return { index, target, before: tool === 'write' ? 'BEFORE:' + marker : null };
}
const pending = async index => until(() => frames.slice(index).find(frame => frame.payload?.type === 'approval/requested' && frame.payload.sessionId === session), 'approval request missing');

(async () => {
  let stream = await connect(); await loadComponent();
  let run = await start('read', 'project/.env', 'reject-sensitive'), frame = await pending(run.index); assert.equal(frame.payload.rememberable, false);
  await render(frame); await click('拒绝'); await resolved(frame, 'rejected'); let events = await completed(); assert.ok(events.filter(event => event.type === 'tool/result').some(event => JSON.stringify(event).includes('USER_APPROVAL_DENIED'))); evidence.checks.push('reject-sensitive-read');
  run = await start('read', 'project/.env', 'allow-sensitive'); frame = await pending(run.index); await render(frame, { rejectFirst: true }); await click('允许一次'); await until(() => document.querySelector('[role=alert]'), 'rejected receipt must show retryable error'); assert.equal([...document.querySelectorAll('button')].find(button => button.textContent === '允许一次').disabled, false); await click('允许一次'); await resolved(frame, 'allowed-once'); await completed(); evidence.checks.push('wrong-session-rejected-and-ui-retry');
  run = await start('read', 'project/.env', 'repeat-sensitive'); frame = await pending(run.index); assert.equal(frame.payload.rememberable, false); await rpc('session.cancel', { sessionId: session }); await resolved(frame, 'cancelled'); await completed('aborted'); assert.equal((await respond(frame, 'allowed-once')).accepted, false); evidence.checks.push('single-use-reprompts-cancel-retires-request');
  run = await start('write', 'outside-one/denied.txt', 'deny-write'); frame = await pending(run.index); assert.equal(frame.payload.rememberable, true); assert.equal(fs.readFileSync(run.target, 'utf8'), run.before); await render(frame); await click('拒绝'); await resolved(frame, 'rejected'); await completed(); assert.equal(fs.readFileSync(run.target, 'utf8'), run.before); evidence.checks.push('rejected-write-never-executes');
  run = await start('write', 'outside-one/once.txt', 'once-write'); frame = await pending(run.index); await render(frame); await click('允许一次'); await resolved(frame, 'allowed-once'); await completed(); assert.equal(fs.readFileSync(run.target, 'utf8'), 'APPROVAL_FIXTURE_WRITTEN:once-write'); evidence.checks.push('allow-once-executes-only-after-response');
  run = await start('write', 'outside-one/once.txt', 'remember-write'); frame = await pending(run.index); const beforeReconnect = frames.length; stream.controller.abort(); await stream.pump; stream = await connect(); const replayed = await until(() => frames.slice(beforeReconnect).find(item => item.payload?.approvalId === frame.payload.approvalId && item.payload.type === 'approval/requested'), 'reconnect did not replay pending approval'); assert.equal(replayed.rpcId, frame.rpcId); await render(replayed); await click('始终允许'); await resolved(frame, 'allowed-always'); await completed(); assert.equal(fs.readFileSync(run.target, 'utf8'), 'APPROVAL_FIXTURE_WRITTEN:remember-write'); evidence.checks.push('reconnect-replays-original-request-and-scope-grant');
  run = await start('write', 'outside-one/sibling.txt', 'same-directory'); await completed(); assert.equal(frames.slice(run.index).some(frame => frame.payload?.type === 'approval/requested'), false); assert.equal(fs.readFileSync(run.target, 'utf8'), 'APPROVAL_FIXTURE_WRITTEN:same-directory'); evidence.checks.push('remembered-directory-reuses-exact-scope');
  for (const relative of ['outside-two/other.txt', 'outside-one/child/nested.txt']) { run = await start('write', relative, 'different-directory'); frame = await pending(run.index); await render(frame); await click('拒绝'); await resolved(frame, 'rejected'); await completed(); assert.equal(fs.readFileSync(run.target, 'utf8'), run.before); }
  evidence.checks.push('other-and-child-directories-still-require-approval');
  const security = (await rpc('settings.describe')).namespaces.find(row => row.ns === 'security');
  await rpc('settings.mutate', {ns:'security', ops:[{op:'set',path:['approvalTimeoutSeconds'],value:5}], expectedRevision:security.revision});
  run = await start('write', 'outside-two/timeout.txt', 'timeout-denied'); frame = await pending(run.index); const started = Date.now(); await render(frame); await resolved(frame); await completed(); assert.ok(Date.now()-started >= 4000, 'configured wait must not be skipped'); assert.equal(fs.readFileSync(run.target,'utf8'),run.before); assert.equal((await respond(frame,'allowed-once')).accepted,false); evidence.checks.push('configured-timeout-denies-and-retires-stale-card');
  fs.writeFileSync(path.join(work, 'approval-interaction-evidence.json'), JSON.stringify(evidence, null, 2));
  assert.ok(evidence.detailsVisible.every(item => item.visible), 'file approval cards must show the exact target path: ' + JSON.stringify(evidence.detailsVisible));
  console.log('PASS actual Host + shipped ApprovalPanel: reject/allow once/remember scope, no execution before approval, forged-session rejection and UI retry, cancellation/stale receipt, SSE reconnect, exact directory boundary and visible target paths');
})().catch(error => { console.error(error); process.exitCode = 1; }).finally(async () => { for (const controller of streams) controller.abort(); await React.act(async () => root.unmount()); });
