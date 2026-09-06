const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const { JSDOM } = require(path.join(modules, 'jsdom'));
const React = require(path.join(modules, 'react'));
const jsx = require(path.join(modules, 'react/jsx-runtime'));
const dom = new JSDOM('<div data-conversation-scroll><div id="root"></div></div>', { pretendToBeVisual: true });
Object.assign(global, { window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
const Client = require(path.join(modules, 'react-dom/client'));
const source = fs.readFileSync(path.join(__dirname, '../../web/dist/plugins/ui-conversation.js'), 'utf8');
const numberStart = source.indexOf('function continueNumberedDraft(');
const inputStart = source.indexOf('function InputBar(', numberStart);
const numberContext = {};
vm.runInNewContext(source.slice(numberStart, inputStart), numberContext);
const next = numberContext.continueNumberedDraft;
assert.equal(next('1. 第一项', 6).text, '1. 第一项\n2. ');
assert.equal(next('9. item', 7).text, '9. item\n10. ');
assert.equal(next('  2、内容', 6).text, '  2、内容\n  3、 ');
assert.equal(next('plain text', 10), null);
assert.equal(next('version 1.2', 11), null);
assert.equal(next('1. abc\nnormal', 13), null);
assert.equal(next('1. abcXX', 6, 8).text, '1. abc\n2. ');
const keyStart = source.indexOf('const onKeyDown = (e) => {', inputStart);
const keyEnd = source.indexOf('\n\t\t\tconst onChange =', keyStart);
const calls = [], input = document.createElement('textarea');
const keyContext = { workspaceTrigger: false, inputActions: {}, composingRef: { current: false }, machineBusy: false, locked: false, running: true, subagent: null, canSteerQueue: false,
  draft: '1. 内容', continueNumberedDraft: next, restoreCaret: (_el, value) => calls.push(['caret', value]),
  keyboard: { arbitrate: () => 'pass', setDraft: value => calls.push(['draft', value]), track: () => {}, submit: mode => calls.push(['submit', mode]) },
  resolveSubmitMode: () => 'steer' };
vm.runInNewContext(source.slice(keyStart, keyEnd) + ';globalThis.key = onKeyDown;', keyContext);
input.value = keyContext.draft; input.setSelectionRange(input.value.length, input.value.length);
keyContext.key({ key: 'Enter', ctrlKey: true, currentTarget: input, nativeEvent: {}, preventDefault() {} });
assert.ok(calls.some(([kind, text]) => kind === 'draft' && text === '1. 内容\n2. '));
assert.ok(!calls.some(([kind]) => kind === 'submit'));
keyContext.draft = 'ordinary steering'; input.value = keyContext.draft;
keyContext.key({ key: 'Enter', ctrlKey: true, currentTarget: input, nativeEvent: {}, preventDefault() {} });
assert.ok(calls.some(([kind, mode]) => kind === 'submit' && mode === 'steer'));

const observers = [];
class ResizeObserver { constructor(fn) { this.fn = fn; observers.push(this); } observe() {} disconnect() { this.dead = true; } }
const context = { react: React, react_jsx_runtime: jsx, window, document, HTMLElement, ResizeObserver,
  setInterval, clearInterval, setTimeout, clearTimeout,
  ChatNodeSeat: ({ nodeKey }) => React.createElement('div', { 'data-chat-anchor-key': nodeKey }, nodeKey),
  ChatView_module_css_default: new Proxy({}, { get: (_t, k) => String(k) }),
  PendingSteeringBubble: () => null, formatRunDuration: () => '',
  _deepseek_ai_dsh_client_ui_primitives: { IconChevronDownOutline14: () => null } };
const chatStart = source.indexOf('const SCROLL_SAMPLE_INTERVAL_MS');
const chatEnd = source.indexOf('//#region lib/types/client/contract/slots.js', chatStart);
vm.runInNewContext(source.slice(chatStart, chatEnd), context);
const scroll = document.querySelector('[data-conversation-scroll]');
let height = 1000, top = 0;
Object.defineProperties(scroll, { scrollHeight: { get: () => height }, clientHeight: { get: () => 200 }, scrollTop: { get: () => top, set: value => { top = Math.max(0, Math.min(value, height - 200)); } } });
const state = { chat: { order: [], nodes: new Map(), timeline: { turns: new Map() } }, queue: [], running: false, runningCalls: [], openState: 'open', openError: null, hasMoreBefore: false, hasMoreAfter: false, historyBrowsing: false, loadingOlder: false, loadingNewer: false, baseSeq: 0 };
const props = { useSession: select => select(state), useSessions: select => select({ byId: { a: { cwd: 'fixture' } } }), useStore: select => select({}), useProjection: () => {}, renderSlot: () => null, sessionId: 'a', chatScroll: { read: () => null, save() {} }, t: key => key };
const root = Client.createRoot(document.getElementById('root'));
const render = () => React.act(async () => root.render(React.createElement(context.ChatView, props)));
(async () => {
  await render(); assert.equal(top, 800);
  await React.act(async () => scroll.dispatchEvent(new window.WheelEvent('wheel', { deltaY: -80, bubbles: true })));
  height = 1200;
  await React.act(async () => observers.filter(o => !o.dead).forEach(o => o.fn()));
  assert.equal(top, 800, 'stream resize must honor upward intent before the first scroll sample');
  await React.act(async () => { scroll.scrollTop = 400; scroll.dispatchEvent(new window.Event('scroll')); });
  state.chat.order = ['new-user']; state.chat.nodes.set('new-user', { kind: 'user', anchorSeq: 1 });
  await render(); assert.equal(top, 400, 'incoming user/steering rows must preserve the reader position');
  await React.act(async () => document.querySelector('button[aria-label="chat.toBottom"]').click());
  assert.equal(top, 1000);
  await React.act(async () => { scroll.dispatchEvent(new window.Event('scrollend')); });
  height = 1400;
  await React.act(async () => observers.filter(o => !o.dead).forEach(o => o.fn()));
  assert.equal(top, 1200, 'explicit return-to-bottom restores follow mode');
  await React.act(async () => root.unmount()); dom.window.close();
  console.log('PASS conversation: numbered Ctrl+Enter, plain steering shortcut, upward intent, stream resize and explicit follow recovery');
})().catch(error => { console.error(error); process.exitCode = 1; dom.window.close(); });
