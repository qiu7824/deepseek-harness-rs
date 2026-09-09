const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const { JSDOM } = require(path.join(modules, 'jsdom'));
const dom = new JSDOM('<main></main>', { pretendToBeVisual: true, url: 'http://localhost/' });
Object.assign(global, { window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
const React = require(path.join(modules, 'react')), Client = require(path.join(modules, 'react-dom/client'));
const observers = [];
class ResizeObserver { constructor(callback) { this.callback = callback; this.dead = false; observers.push(this); } observe() {} disconnect() { this.dead = true; } }
const source = fs.readFileSync(path.join(__dirname, '../../web/dist/plugins/ui-conversation.js'), 'utf8');
const localeContext = { PLAN_NEXT_ACTION_ZH: '' };
vm.runInNewContext(source.slice(source.indexOf('\t\tconst zh = {'), source.indexOf('\t\tconst en = {')) + ';this.dictionary=zh;', localeContext);
const translate = key => localeContext.dictionary[key] ?? key;
const context = { react: React, react_jsx_runtime: require(path.join(modules, 'react/jsx-runtime')), window, document, HTMLElement, ResizeObserver, setInterval, clearInterval,
  ChatNodeSeat: ({ nodeKey }) => React.createElement('div', { 'data-chat-anchor-key': nodeKey }, nodeKey),
  ChatView_module_css_default: new Proxy({}, { get: (_target, key) => String(key) }), PendingSteeringBubble: () => null, formatRunDuration: () => '',
  _deepseek_ai_dsh_client_ui_primitives: { IconChevronDownOutline14: () => null, IconApiOutline14: () => null, IconThinkOutline14: () => null } };
const start = source.indexOf('const SCROLL_SAMPLE_INTERVAL_MS'), end = source.indexOf('//#region lib/types/client/contract/slots.js', start);
vm.runInNewContext(source.slice(start, end), context);
const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
const act = async fn => React.act(async () => { await fn(); });
let active = null;

async function fixture({ rows, viewport = 200, heights = {}, before = false, after = true, history = true, saved = null }) {
  const port = document.createElement('section'); port.dataset.conversationScroll = ''; port.innerHTML = '<div></div>'; document.querySelector('main').append(port);
  let top = 0, writes = 0;
  const heightOf = key => heights[key] ?? 100;
  const state = { chat: { order: rows.slice(), nodes: new Map(), timeline: { turns: new Map() } }, queue: [], running: false, partial: null, runningCalls: [], openState: 'open', openError: null,
    baseSeq: Number(rows[0] || 0), hasMoreBefore: before, hasMoreAfter: after, historyBrowsing: history, historyNavigationRevision: 0, loadingOlder: false, loadingNewer: false };
  const install = next => { state.chat.order = next.slice(); state.chat.nodes = new Map(next.map(key => [key, { kind: 'user', anchorSeq: Number(key) }])); state.baseSeq = Number(next[0] || 0); };
  install(rows);
  const height = () => state.chat.order.reduce((sum, key) => sum + heightOf(key), 0);
  Object.defineProperties(port, { scrollHeight: { get: height }, clientHeight: { get: () => viewport }, scrollTop: { get: () => top, set: value => { writes++; top = Math.max(0, Math.min(value, Math.max(0, height() - viewport))); } } });
  port.getBoundingClientRect = () => ({ top: 0, bottom: viewport, left: 0, right: 600, width: 600, height: viewport });
  const nativeRect = window.HTMLElement.prototype.getBoundingClientRect;
  window.HTMLElement.prototype.getBoundingClientRect = function () {
    const key = this.dataset?.chatAnchorKey;
    if (key !== undefined && port.contains(this)) {
      const index = state.chat.order.indexOf(key), offset = state.chat.order.slice(0, index).reduce((sum, item) => sum + heightOf(item), 0);
      return { top: offset - top, bottom: offset - top + heightOf(key), left: 0, right: 600, width: 600, height: heightOf(key) };
    }
    return nativeRect.call(this);
  };
  const pending = [], calls = { older: 0, newer: 0, latest: 0, cancelled: 0 }, saves = [];
  const root = Client.createRoot(port.firstChild);
  const render = () => root.render(React.createElement(context.ChatView, props));
  const load = direction => { calls[direction]++; state[direction === 'older' ? 'loadingOlder' : 'loadingNewer'] = true; render(); return new Promise(resolve => pending.push({ direction, resolve, cancelled: false })); };
  const props = { sessionId: 'fixture', useSession: select => select(state), useSessions: select => select({ byId: { fixture: { cwd: 'fixture' } } }), useStore: select => select({}),
    chatScroll: { read: () => saved, save: value => saves.push(value) }, renderSlot: () => null, t: translate,
    loadOlder: () => load('older'), loadNewer: () => load('newer'), returnLatest: () => load('latest'),
    cancelHistoryPaging: () => { calls.cancelled++; for (const request of pending) if (request.direction !== 'latest') request.cancelled = true; state.loadingOlder = state.loadingNewer = false; render(); } };
  const result = { state, port, calls, pending, saves, heights, heightOf,
    top: () => top, writes: () => writes,
    render: () => act(render),
    wheel: async deltaY => act(async () => { port.dispatchEvent(new window.WheelEvent('wheel', { deltaY, bubbles: true })); await wait(85); }),
    move: async (value, direction) => act(async () => { if (direction) port.dispatchEvent(new window.WheelEvent('wheel', { deltaY: direction, bubbles: true })); port.scrollTop = value; port.dispatchEvent(new window.Event('scroll')); port.dispatchEvent(new window.Event('scrollend')); await wait(1); }),
    resolve: async (request, next, flags = {}) => act(async () => { if (!request.cancelled) { install(next); Object.assign(state, flags); state.loadingOlder = state.loadingNewer = false; render(); } request.resolve(); await wait(25); }),
    jump: async next => act(async () => { state.historyNavigationRevision++; state.historyBrowsing = true; state.hasMoreAfter = true; install(next); render(); await wait(1); port.scrollTop = 0; port.dispatchEvent(new window.Event('scroll')); port.dispatchEvent(new window.Event('scrollend')); }),
    resize: async () => act(async () => { observers.filter(observer => !observer.dead).forEach(observer => observer.callback()); await wait(25); }),
    close: async () => { await act(() => root.unmount()); port.remove(); window.HTMLElement.prototype.getBoundingClientRect = nativeRect; active = null; }
  };
  active = result; await result.render(); return result;
}

(async () => {
  // Real regression geometry from the configured Host: appending a page must
  // preserve the reader at 485px rather than snapping to the new 1639px floor.
  let f = await fixture({ rows: ['0'], viewport: 617, heights: { 0: 1102, 1: 1154 } });
  await f.move(485, 200); assert.equal(f.calls.newer, 1);
  await f.resolve(f.pending[0], ['0', '1']);
  assert.equal(f.top(), 485, 'forward append preserves the current reading anchor');
  assert.ok(document.querySelector(`button[aria-label="${translate('chat.toBottom')}"]`), 'the local page bottom is not the live tail');
  await f.close();

  f = await fixture({ rows: Array.from({length:10},(_value,index)=>String(index)), viewport: 400 });
  await f.move(320, 100);
  assert.equal(f.calls.newer,1,'viewport-relative prefetch starts before the reader reaches the bottom');
  assert.equal(f.top(),320);
  await f.resolve(f.pending[0],Array.from({length:11},(_value,index)=>String(index)));
  assert.equal(f.top(),320,'prefetch cannot move the reader');
  await f.close();

  f = await fixture({ rows: ['3', '4'], heights: { 1: 300, 2: 300, 3: 300, 4: 300 }, before: true });
  await f.wheel(-100); assert.equal(f.calls.older, 1);
  await f.resolve(f.pending[0], ['1', '2', '3']);
  assert.equal(f.top(), 600, 'older prepend and tail eviction keep the same visible semantic row');
  await f.close();

  f = await fixture({ rows: ['1', '2', '3'], viewport: 100 });
  await f.move(200, 100); assert.equal(f.calls.newer, 1);
  await f.resolve(f.pending[0], ['2', '3', '4'], { hasMoreAfter: false, historyBrowsing: false });
  assert.equal(f.top(), 100, 'head eviction compensates the retained row before joining the live tail');
  assert.ok(document.querySelector(`button[aria-label="${translate('chat.toBottom')}"]`), 'a resolved final page cannot implicitly enable following');
  await f.move(200, 100); f.heights['4'] = 200; await f.resize();
  assert.equal(f.top(), 300, 'an explicit downward gesture at the real tail restores following');
  await f.close();

  for (const gesture of ['wheel', 'keyboard', 'touch']) {
    f = await fixture({ rows: ['0'], heights: { 0: 80 }, before: true });
    await f.jump(['0']);
    await act(async () => {
      if (gesture === 'wheel') f.port.dispatchEvent(new window.WheelEvent('wheel', { deltaY: 120, bubbles: true }));
      if (gesture === 'keyboard') f.port.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'End', bubbles: true }));
      if (gesture === 'touch') for (const [type, y] of [['touchstart', 160], ['touchmove', 50]]) { const event = new window.Event(type, { bubbles: true }); Object.defineProperty(event, 'touches', { value: [{ clientY: y }] }); f.port.dispatchEvent(event); }
      await wait(170);
    });
    assert.equal(f.top(), 0); assert.equal(f.calls.newer, 1, `${gesture} loads a short page without requiring a native scroll event`);
    await f.wheel(120); assert.equal(f.calls.newer, 1, 'only one request may be outstanding');
    await f.wheel(-120); assert.equal(f.calls.cancelled, 1, 'opposite intent cancels stale directional pagination');
    await act(async()=>{for(const deltaY of [120,-120,120,-120,120,-120])f.port.dispatchEvent(new window.WheelEvent('wheel',{deltaY,bubbles:true}));await wait(85);});
    assert.equal(f.calls.newer,1);assert.equal(f.calls.cancelled,1,'rapid reversals retain one pending transport and one latest direction');
    assert.equal(f.calls.older, 0, 'reverse intent waits for the one outstanding transport request');
    await f.resolve(f.pending[0], ['0', '99']); assert.deepEqual(f.state.chat.order, ['0']);
    await act(()=>wait(85));assert.equal(f.calls.older,1);
    const reverse = f.pending.find(request => request.direction === 'older');
    await f.resolve(reverse, ['-1', '0'], { hasMoreBefore: false });
    await f.close();
  }

  f = await fixture({ rows: ['1', '2', '3'], after: true });
  await act(async () => { f.port.dispatchEvent(new window.MouseEvent('pointerdown', { bubbles: true })); f.port.scrollTop = 100; f.port.dispatchEvent(new window.Event('scroll')); window.dispatchEvent(new window.Event('pointerup')); await wait(85); });
  assert.equal(f.calls.newer, 1, 'scrollbar dragging can request the next page');
  await f.close();

  f = await fixture({rows:['0','1','2','3','4','5'],history:false,after:false});
  await f.move(250,-100);f.heights['0']=200;await f.resize();
  assert.equal(f.top(),350,'late media or width reflow above the reader preserves the visible row offset');
  f.state.historyNavigationRevision++;f.state.historyNavigationReason='resync';f.state.openState='loading';await f.render();
  f.heights['0']=300;f.state.openState='open';await f.render();
  assert.equal(f.top(),450,'reconnect retains the reader anchor while restoring the bounded window');
  await f.close();

  f = await fixture({rows:['0','1','2'],history:false,after:false,viewport:290});
  await f.move(0);f.heights['2']=120;await f.resize();
  assert.equal(f.top(),0,'a programmatic near-tail jump also releases automatic following');
  await f.close();

  f = await fixture({ rows: ['8', '9'], heights: { 8: 300, 9: 300 }, history: false, after: false });
  assert.equal(f.top(), 400);
  await f.jump(['0', '1']); assert.equal(f.calls.newer, 0, 'programmatic navigation does not manufacture downward paging intent');
  await act(async () => { document.querySelector(`button[aria-label="${translate('chat.toBottom')}"]`).click(); await wait(1); });
  assert.equal(f.calls.latest, 1);
  assert.equal(f.calls.newer, 0, 'returning to latest requests the latest window directly');
  const latest = f.pending.find(request => request.direction === 'latest');
  await f.resolve(latest, ['8', '9'], { hasMoreAfter: false, historyBrowsing: false, historyNavigationRevision: f.state.historyNavigationRevision + 1 });
  assert.equal(f.top(), 400);
  f.state.running = true; await f.render(); assert.match(f.port.textContent, /等待模型响应/);
  f.state.partial = { blocks: [{ type: 'reasoning', text: 'reasoning' }] }; await f.render(); assert.match(f.port.textContent, /模型推理中/);
  f.state.partial = { blocks: [{ type: 'text', text: 'first token' }] }; await f.render(); assert.match(f.port.textContent, /生成回复中/);
  f.state.running = false; await f.render(); assert.doesNotMatch(f.port.textContent, /等待模型响应|模型推理中|生成回复中/);
  const count = f.calls.newer; const port = f.port; await f.close();
  port.dispatchEvent(new window.WheelEvent('wheel', { deltaY: 100, bubbles: true })); await wait(85); assert.equal(f.calls.newer, count);
  assert.ok(observers.every(observer => observer.dead)); dom.window.close();
  console.log('PASS chat scroll: anchored append/prepend/eviction, short-page wheel/keyboard/touch, cancellation, scrollbar, jump/latest exclusivity, token phases and cleanup');
})().catch(async error => { console.error(error); process.exitCode = 1; if (active) await active.close(); dom.window.close(); });
