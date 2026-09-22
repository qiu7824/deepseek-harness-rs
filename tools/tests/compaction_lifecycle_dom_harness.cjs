const assert = require('node:assert/strict'), fs = require('node:fs'), vm = require('node:vm'), path = require('node:path');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const React = require(path.join(modules, 'react')), jsx = require(path.join(modules, 'react/jsx-runtime'));
const { JSDOM } = require(path.join(modules, 'jsdom'));
const dom = new JSDOM('<main></main>', { url: 'http://compaction.test', pretendToBeVisual: true });
Object.assign(global, { window: dom.window, document: dom.window.document, IS_REACT_ACT_ENVIRONMENT: true });
const source = fs.readFileSync(path.join(__dirname, '../../web/src/runtime-plugins/ui-conversation.js'), 'utf8');
const context = { react: React, react_jsx_runtime: jsx, MessageItem_module_css_default: {},
  _deepseek_ai_dsh_client_ui_primitives: new Proxy({ MarkdownText: ({ text }) => React.createElement('p', null, text) }, { get: (o, k) => o[k] || (() => null) }),
  _deepseek_ai_dsh_client_runtime_client: { isReplacementSurfaceEvent: e => e.surfaceOp?.op === 'replace' },
  chatNode: (owner, kind, seq, data) => ({ key: owner.key, kind, anchorSeq: seq, data }) };
const begin = source.indexOf('const COMPACT_PLUGIN ='), end = source.indexOf('//#region lib/types/client/conversation-nodes/fallback.js', begin);
const item = source.indexOf('const CompactionItem ='), itemEnd = source.indexOf('//#endregion', item);
vm.runInNewContext(source.slice(begin, end) + ';this.def=compactionDefinition;this.commands=commandDefinition;' + source.slice(item, itemEnd) + ';this.Item=CompactionItem;', context);
const root = require(path.join(modules, 'react-dom/client')).createRoot(document.querySelector('main'));
const act = fn => React.act(async () => { fn?.(); await new Promise(resolve => setImmediate(resolve)); });
let seq = 0;
const event = (type, id, extra = {}) => ({ type, seq: ++seq, time: seq * 100, data: { compactionId: id, sourceCommandId: null, ...extra } });
function session() {
  let state;
  return { emit(e) {
    const match = context.def.match(e); assert.ok(match, 'automatic compaction with null sourceCommandId must be recognized');
    const item = { event: e, location: { kind: 'unresolved' } };
    assert.equal(context.commands.match(e), null, 'automatic events must not leak into a command named null');
    if (match.role === 'start') state = { key: 'compact:' + match.id, id: match.id, start: item, matches: [item], state: context.def.start({}, item) };
    else { state.matches.push(item); state.state = context.def.update(state, item); }
    return context.def.buildViewNode(state);
  } };
}
(async () => {
  const failed = session();
  let node = failed.emit(event('compaction/start', 'failed'));
  assert.equal(node.data.pending, true);
  await act(() => root.render(React.createElement(context.Item, { node: node.data, t: k => k })));
  assert.match(document.body.textContent, /message.compaction.running/);
  node = failed.emit(event('compaction/end', 'failed', { error: 'compaction cancelled' }));
  assert.equal(node.data.pending, false, 'end without a replacement must settle the indicator');
  await act(() => root.render(React.createElement(context.Item, { node: node.data, t: k => k })));
  assert.doesNotMatch(document.body.textContent, /message.compaction.running/);
  assert.match(document.body.textContent, /compaction cancelled/);
  assert.equal(document.querySelector('button').disabled, true);
  const completed = session(); const first = completed.emit(event('compaction/start', 'complete'));
  completed.emit(event('compaction/summary', 'complete', { summary: [{ type: 'text', text: 'Keep unfinished constraints.' }], shadowedSeqs: [1, 2], shadowedTokenCount: 4000 }));
  node = completed.emit({ type: 'user/message', seq: ++seq, time: seq * 100, surfaceOp: { op: 'replace', start: 1, end: 2 }, data: { source: { kind: 'plugin', plugin: 'compact', compactionId: 'complete', sourceCommandId: null } } });
  assert.equal(node.key, first.key); assert.equal(node.data.pending, undefined); assert.equal(node.data.shadowedTokenCount, 4000);
  const checkpointKey = node.key;
  node = completed.emit(event('compaction/end', 'complete'));
  assert.equal(node.key, checkpointKey); assert.equal(node.data.summary, 'Keep unfinished constraints.');
  await act(() => root.render(React.createElement(context.Item, { node: node.data, t: k => k })));
  assert.doesNotMatch(document.body.textContent, /message.compaction.running/);
  assert.equal(context.def.match(event('compaction/start', 'manual', { sourceCommandId: 'cmd-1' })), null);
  assert.equal(context.commands.match(event('compaction/start', 'manual', { sourceCommandId: 'cmd-1' })).id, 'cmd-1');
  await act(() => root.unmount()); dom.window.close();
  console.log('PASS automatic compaction: null wire fields, pending display, cancelled end, landed checkpoint and manual separation');
})().catch(error => { console.error(error); process.exitCode = 1; dom.window.close(); });
