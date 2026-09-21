const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const { JSDOM } = require(path.join(modules, 'jsdom'));
const dom = new JSDOM('<main id="root"></main>', { pretendToBeVisual: true, url: 'http://localhost' });
Object.assign(global, { window: dom.window, document: dom.window.document, IS_REACT_ACT_ENVIRONMENT: true });
const React = require(path.join(modules, 'react')), jsx = require(path.join(modules, 'react/jsx-runtime'));
const root = require(path.join(modules, 'react-dom/client')).createRoot(document.getElementById('root'));
const plugins = {}, timers = new Map(), readers = []; let timerId = 0;
const runtimeSource=fs.readFileSync(path.resolve(__dirname,'../../web/src/runtime-plugins/client-runtime.js'),'utf8'),sharedRequests={};
vm.runInNewContext(runtimeSource.slice(runtimeSource.indexOf('const activePromptRequests ='),runtimeSource.indexOf('function resolvedClientTimeZone(')),sharedRequests);
const primitives = new Proxy({
  StateDot: ({ state }) => React.createElement('span', { 'data-dot': state }),
  MarkdownText: ({ text }) => React.createElement('div', null, text),
}, { get: (target, key) => target[key] || (() => null) });
window.__ModuleLoader__ = { load: definition => { plugins[definition.id] = definition.factory(id => id === 'react' ? React : id === 'react/jsx-runtime' ? jsx : id.endsWith('ui-primitives') ? primitives : id.endsWith('runtime/client') ? { ...sharedRequests, indexSubagentDescendants: () => new Map([['parent', { count: 1, runningCount: 0 }]]) } : {}); } };
const context = { window, document, console, Node: dom.window.Node, AbortController, crypto: require('node:crypto').webcrypto,
  setTimeout: fn => { const id = ++timerId; timers.set(id, fn); return id; }, clearTimeout: id => timers.delete(id),
  setInterval: () => 0, clearInterval() {}, queueMicrotask, structuredClone,
  FileReader: class { readAsDataURL() { this.result = 'data:image/png;base64,YWJj'; readers.push(this); } },
};
const directory = path.resolve(__dirname, '../../web/src/runtime-plugins');
const subagentSource = fs.readFileSync(path.join(directory, 'ui-subagent.js'), 'utf8');
vm.runInNewContext(subagentSource.replace('return module.exports;', 'exports.test={SubagentToolRow,TeamTaskCard,SubagentCatalogAction,createTeamPanel,zh,en};return module.exports;'), context);
vm.runInNewContext(fs.readFileSync(path.join(directory, 'ui-workbench-previews.js'), 'utf8'), context);
const { SubagentToolRow, TeamTaskCard, SubagentCatalogAction, createTeamPanel, zh, en } = plugins['@deepseek-ai/dsh-client-ui-subagent'].test;
const { SideConversation } = plugins['@deepseek-ai/dsh-client-ui-workbench-previews'].test;
const act = fn => React.act(async () => { await fn(); });
const render = element => act(() => root.render(element));
const button = text => { const value = [...document.querySelectorAll('button')].find(node => node.textContent === text || node.getAttribute('aria-label') === text); assert.ok(value, `button ${text}`); return value; };
const input = (element, value) => act(() => { const prototype = element instanceof window.HTMLTextAreaElement ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype; Object.getOwnPropertyDescriptor(prototype, 'value').set.call(element, value); element.dispatchEvent(new window.Event('input', { bubbles: true })); });
const submit = form => act(() => form.dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })));
const field = label => [...document.querySelectorAll('label')].find(node => node.querySelector('span')?.textContent === label)?.querySelector('input,textarea');
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };

(async () => {
  assert.deepEqual(Object.keys(zh).sort(), Object.keys(en).sort());
  const opened = [], address = { parentSessionId: 'parent', childSessionId: 'child-a', mode: 'continuable' };
  let snapshot = { byId: {}, subagentsByParent: {} }; const rowListeners = new Set();
  await render(React.createElement(SubagentToolRow, {
    parentSessionId: 'parent', sessionsStore: { getSnapshot: () => snapshot, subscribe: callback => { rowListeners.add(callback); return () => rowListeners.delete(callback); } },
    refresh() {}, loadProgress: async () => ({ events: [], hasMore: false }), openChild: value => opened.push(value), t: key => zh[key] || key,
    block: { kind: 'tool-result', call: { argsRaw: JSON.stringify({ description: '检查模块', prompt: 'Check the module' }) }, content: [{ type: 'text', text: 'started continuable subagent child-a' }] },
  }));
  assert.equal(document.querySelector('.dsh-subagent-tool-trigger').disabled, true, 'navigation waits for the actual catalog capability, not only a text result ID');
  snapshot = { ...snapshot, subagentsByParent: { parent: { state: 'ready', entries: [{ kind: 'child', id: 'child-a', mode: 'continuable', activity: 'inactive' }] } } };
  await act(() => rowListeners.forEach(callback => callback()));
  assert.equal(document.querySelector('.dsh-subagent-tool-trigger').disabled, false);
  assert.equal(document.querySelector('.dsh-subagent-tool-body'), null);
  await act(() => document.querySelector('.dsh-subagent-tool-trigger').click());
  assert.deepEqual(JSON.parse(JSON.stringify(opened)), [address], 'the primary row opens the exact child in one click');
  assert.equal(document.querySelector('.dsh-subagent-tool-body'), null, 'navigation does not require a details expansion');
  await act(() => document.querySelector('.dsh-subagent-tool-expand').click());
  assert.match(document.querySelector('.dsh-subagent-tool-body').textContent, /Check the module/);
  assert.equal(opened.length, 1, 'the separate details control never navigates');
  const catalogSnapshot = { byId: { 'child-a': { id: 'child-a', parentId: 'parent', origin: 'subagent', displayTitle: 'Worker' } }, subagentsByParent: { parent: { state: 'ready', parentAvailable: true, entries: [{ kind: 'child', id: 'child-a', mode: 'continuable', label: 'Worker', activity: 'inactive', hasChildren: false }] } } };
  await render(React.createElement(SubagentCatalogAction, { sessionId: 'parent', parentSessionId: 'parent', panel: createTeamPanel(), useSessions: select => select(catalogSnapshot), setCatalogOpen() {}, refresh() {}, loadProgress: async () => ({ events: [], hasMore: false }), openChild: value => opened.push(value), t: key => zh[key] || key }));
  const catalogButton = document.querySelector('button[aria-haspopup=tree]'); assert.ok(catalogButton, 'child navigation is directly available in the conversation header');
  await act(() => catalogButton.click()); assert.ok(document.querySelector('[role=tree]')); assert.equal(document.querySelector('[role=dialog]'), null, 'the child directory does not require opening team management');
  await act(() => document.querySelector('[role=treeitem]').click()); assert.equal(opened.at(-1).childSessionId, 'child-a'); assert.equal(document.querySelector('[role=tree]'), null);

  const ancestryContext = {};
  const conversationSource = fs.readFileSync(path.join(directory, 'ui-conversation.js'), 'utf8');
  const ancestryStart = conversationSource.indexOf('function deriveAncestry('), ancestryEnd = conversationSource.indexOf('function equalBreadcrumbs(', ancestryStart);
  vm.runInNewContext(conversationSource.slice(ancestryStart, ancestryEnd), ancestryContext);
  assert.deepEqual(JSON.parse(JSON.stringify(ancestryContext.deriveAncestry({ byId: {
    root: { id: 'root', displayTitle: 'Root' }, parent: { id: 'parent', parentId: 'root', origin: 'subagent', displayTitle: 'Parent' },
    'child-a': { id: 'child-a', parentId: 'parent', origin: 'subagent', displayTitle: 'Child' },
  } }, 'child-a'))).map(row => row.id), ['root', 'parent', 'child-a'], 'nested conversations retain the direct parent breadcrumb');

  let task = { id: 't', revision: 1, subject: 'Original', description: 'Description', acceptance: 'Pass', ownerId: null, blockedBy: [], writeScopes: [], status: 'pending', result: '' };
  const saves = []; let accepted = false;
  const taskView = () => React.createElement(TeamTaskCard, { task, tasks: [task], members: [], label: value => value || 'Nobody', busy: false, enabled: true, t: key => key, onSave: async value => { saves.push(value); return accepted; } });
  await render(taskView()); await act(() => button('team.edit').click());
  await input(field('team.subject'), 'Unsaved work');
  task = { ...task, revision: 2, subject: 'Remote work' }; await render(taskView());
  assert.equal(field('team.subject').value, 'Unsaved work', 'polling a newer revision preserves the editor draft');
  assert.match(document.querySelector('[role=alert]').textContent, /team.taskChanged/);
  assert.equal(button('team.save').disabled, true);
  await submit(document.querySelector('form')); assert.equal(saves.length, 0, 'stale drafts cannot bypass the revision fence through form submit');
  await act(() => button('team.loadLatest').click()); assert.equal(field('team.subject').value, 'Remote work');
  await input(field('team.subject'), 'Reviewed work'); await submit(document.querySelector('form'));
  assert.equal(saves[0].expectedRevision, 2); assert.equal(field('team.subject').value, 'Reviewed work', 'failed writes preserve draft and editor');
  accepted = true; await submit(document.querySelector('form')); assert.equal(document.querySelector('form'), null);

  const rpcCalls = [], pendingPrompts = [], histories = new Map();
  const rpc = (method, payload, signal) => {
    rpcCalls.push({ method, payload, signal });
    if (method === 'subagent.history') return histories.get(payload.childSessionId)?.promise || Promise.resolve({ events: [], hasMore: false });
    if (method === 'subagent.prompt') { const pending = deferred(); pendingPrompts.push(pending); return pending.promise; }
    if (method === 'subagent.interrupt') return Promise.resolve({ stopped: true });
    throw Error(method);
  };
  const preview = child => React.createElement(SideConversation, { tab: { title: child, meta: { address: { ...address, childSessionId: child } } }, rpc, onMain: value => opened.push(value) });
  await render(preview('child-a'));
  await input(document.querySelector('textarea'), 'first request'); await submit(document.querySelector('form'));
  assert.equal(button('停止').disabled, false, 'Stop remains available while prompt admission is pending');
  await input(document.querySelector('textarea'), 'next draft');
  const stoppedRequestId = rpcCalls.filter(call => call.method === 'subagent.prompt').at(-1).payload.requestId;
  await act(() => button('停止').click());
  assert.equal(rpcCalls.filter(call => call.method === 'subagent.interrupt').length, 1, 'Stop interrupts promptly before slow admission finishes');
  assert.deepEqual(JSON.parse(JSON.stringify(rpcCalls.filter(call => call.method === 'subagent.interrupt')[0].payload.requestIds)), [stoppedRequestId], 'Stop identifies the exact in-flight logical request even if transport reorders it');
  assert.equal(button('发送').disabled, false, 'a durable Stop receipt releases the composer before the old transport settles');
  await act(() => pendingPrompts[0].resolve({ status: 'late admission' }));
  assert.equal(rpcCalls.filter(call => call.method === 'subagent.interrupt').length, 1, 'the durable request cancellation replaces the racy second interrupt');
  assert.equal(document.querySelector('textarea').value, 'next draft', 'a send receipt cannot erase text typed after submission');
  assert.doesNotMatch(document.body.textContent, /late admission/);
  assert.ok(rpcCalls.filter(call => call.method === 'subagent.interrupt').every(call => call.payload.childSessionId === 'child-a'));
  await act(() => button('在主区打开').click()); assert.equal(opened.at(-1).childSessionId, 'child-a');

  const staleHistory = deferred(); histories.set('child-old', staleHistory);
  await render(preview('child-old')); await input(document.querySelector('textarea'), 'old draft'); await submit(document.querySelector('form'));
  const stalePrompt = pendingPrompts.at(-1);
  await render(preview('child-new')); await input(document.querySelector('textarea'), 'new draft');
  await act(() => { staleHistory.resolve({ events: [{ event: { seq: 1, type: 'assistant/message', data: { message: { content: [{ type: 'text', text: 'OLD HISTORY' }] } } } }] }); stalePrompt.resolve({ status: 'OLD RECEIPT' }); });
  assert.equal(document.querySelector('textarea').value, 'new draft'); assert.doesNotMatch(document.body.textContent, /OLD HISTORY|OLD RECEIPT/);
  await render(preview('child-old')); assert.equal(document.querySelector('textarea').value, 'old draft', 'switching children retains the originating draft without sharing it');
  const originalId = rpcCalls.filter(call => call.method === 'subagent.prompt' && call.payload.childSessionId === 'child-old').at(-1).payload.requestId;
  await submit(document.querySelector('form')); assert.equal(rpcCalls.at(-1).payload.requestId, originalId, 'reopening before a late acknowledgement retains the retry identity');
  await act(() => pendingPrompts.at(-1).resolve({ status: 'confirmed' }));
  await input(document.querySelector('textarea'), 'cancelled text'); await submit(document.querySelector('form'));
  const cancelledId = rpcCalls.at(-1).payload.requestId;
  await act(() => pendingPrompts.at(-1).reject(Object.assign(new Error('request cancelled'), { code: 'cancelled' })));
  assert.equal(document.querySelector('textarea').value, 'cancelled text');
  await submit(document.querySelector('form')); assert.notEqual(rpcCalls.at(-1).payload.requestId, cancelledId, 'explicit resending after a definitive cancellation gets a fresh identity');
  await act(() => pendingPrompts.at(-1).resolve({ status: 'resent' }));
  await render(preview('child-upload'));
  const picker = document.querySelector('input[type=file]');
  await act(() => { Object.defineProperty(picker, 'files', { value: [new window.File(['abc'], 'frame.png', { type: 'image/png' })], configurable: true }); picker.dispatchEvent(new window.Event('change', { bubbles: true })); });
  const promptCount = rpcCalls.filter(call => call.method === 'subagent.prompt').length;
  await submit(document.querySelector('form')); assert.equal(readers.length, 1);
  await act(() => button('停止').click()); await act(() => readers[0].onload());
  assert.equal(rpcCalls.filter(call => call.method === 'subagent.prompt').length, promptCount, 'Stop while reading attachments prevents the later prompt dispatch entirely');
  await render(preview('child-cross-stop'));
  const crossAddress={...address,childSessionId:'child-cross-stop'},stopReplies=[],stopBatches=[];
  const initialTicket=sharedRequests.trackPromptRequest(crossAddress,'main-view-pending');
  const externalStop=sharedRequests.stopPromptRequests(crossAddress,[initialTicket.requestId],ids=>{stopBatches.push([...ids]);const pending=deferred();stopReplies.push(pending);return pending.promise;});
  await input(document.querySelector('textarea'),'preview waits for main Stop');
  const crossPromptCount=rpcCalls.filter(call=>call.method==='subagent.prompt').length;
  await submit(document.querySelector('form'));
  assert.equal(rpcCalls.filter(call=>call.method==='subagent.prompt').length,crossPromptCount,'preview prompt waits for a Stop started in the main conversation');
  const waitingTicketId=sharedRequests.pendingPromptRequestIds(crossAddress).find(id=>id!==initialTicket.requestId);
  await act(()=>button('停止').click());
  const laterTicket=sharedRequests.trackPromptRequest(crossAddress,'new-id-from-other-view');
  await act(()=>button('正在停止…').click());
  await act(()=>stopReplies.shift().resolve({ok:true}));
  assert.deepEqual(stopBatches,[['main-view-pending'],[waitingTicketId,laterTicket.requestId]],'clicking Stop again while waiting also includes identities from another view');
  await act(()=>stopReplies.shift().resolve({ok:true}));await externalStop;
  assert.equal(rpcCalls.filter(call=>call.method==='subagent.prompt').length,crossPromptCount,'a second Stop cancels the waiting preview before dispatch');
  assert.equal(document.querySelector('textarea').value,'preview waits for main Stop','cancelled preparation retains the draft');
  assert.equal(button('发送').disabled,false);
  initialTicket.release();laterTicket.release();
  await submit(document.querySelector('form'));
  assert.notEqual(rpcCalls.at(-1).payload.requestId,waitingTicketId,'an explicit resend after Stop gets a fresh identity');
  const externalIds=sharedRequests.pendingPromptRequestIds(crossAddress);
  await act(()=>sharedRequests.stopPromptRequests(crossAddress,externalIds,async()=>({ok:true})));
  await act(()=>pendingPrompts.at(-1).resolve({status:'cancelled cross-view late receipt'}));
  assert.equal(document.querySelector('textarea').value,'preview waits for main Stop','a Stop from another view fences late receipt clearing');
  assert.doesNotMatch(document.body.textContent,/cancelled cross-view late receipt/);
  await submit(document.querySelector('form'));
  assert.ok(!externalIds.includes(rpcCalls.at(-1).payload.requestId),'cross-view cancellation releases the stopped retry identity');
  await act(()=>pendingPrompts.at(-1).resolve({status:'fresh explicit send'}));
  await render(React.createElement(SideConversation, { tab: { title: 'Read only', meta: { address: { ...address, mode: 'one-shot' } } }, rpc, onMain() {} }));
  assert.equal(button('停止').disabled, true); assert.equal(document.querySelector('form'), null, 'one-shot previews expose no mutation controls');
  await act(() => root.unmount()); assert.equal(timers.size, 0, 'unmounted previews leave no polling timers');
  dom.window.close();
  console.log('PASS collaboration reliability: direct child navigation, ancestry, task draft conflicts, stop during admission, late receipt draft preservation, child switch isolation and polling cleanup');
})().catch(error => { console.error(error); process.exitCode = 1; dom.window.close(); });
