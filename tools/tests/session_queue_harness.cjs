const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const source = fs.readFileSync(path.join(__dirname, '../../web/dist/plugins/client-runtime.js'), 'utf8');
const start = source.indexOf('function sessionReferenceParts('), end = source.indexOf('\n\t\tvar SessionManager = class', start);
const context = { console, resolvedClientTimeZone: () => 'Asia/Shanghai', transportError: error => ({ ok: false, error: { code: 'transport-error', message: error.message } }) };
vm.runInNewContext(source.slice(start, end), context);
function fixture(address) {
  const session = Object.create(context.Session.prototype), calls = [], pending = [];
  const prompt = payload => { calls.push(payload); return new Promise(resolve => pending.push(resolve)); };
  Object.assign(session, { sessionId: address?.childSessionId ?? 'main', address, promptInFlight: [], promptRetry: null,
    queueMirror: new context.SessionQueueMirror(), notifier: { markDirty() {} }, returnLatest: async () => {},
    api: { sessions: { prompt, updateQueue: async payload => { calls.push(payload); return { result: { ok: true } }; } }, subagents: { prompt } },
    handleRunning(value) { this.running = value; }, runningRevision: 0, blankBit: false });
  return { session, calls, pending };
}
(async () => {
  const content = [{ type: 'text', text: 'queue fixture' }];
  for (const address of [undefined, { parentSessionId: 'parent', childSessionId: 'child', mode: 'continuable' }]) {
    const { session, calls, pending } = fixture(address);
    const first = session.prompt(content, 'steer');
    const duplicate = session.prompt(structuredClone(content), 'steer');
    assert.equal(first, duplicate, 'duplicate clicks share one in-flight request');
    assert.equal(session.queueMirror.snapshot()[0].placement, 'sending');
    session.queueMirror.reset();
    assert.equal(session.queueMirror.snapshot()[0].placement, 'sending', 'reconnect preserves unacknowledged sends');
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(calls.length, 1);
    const requestId = calls[0].requestId;
    assert.ok(requestId);
    assert.equal(address ? calls[0].delivery : calls[0].mode, 'steer');
    session.queueMirror.replace([{ id: 'queue-1', placement: 'queued', message: { id: 'message-1', content, source: { kind: 'user', rpcId: requestId } } }]);
    assert.equal(session.queueMirror.snapshot().length, 1, 'Host acknowledgement merges the local sending row');
    pending.shift()({ result: { ok: true, value: { accepted: true, messageId: 'message-1' } } });
    await first;
    assert.equal(session.queueMirror.snapshot()[0].placement, 'queued');
    await session.updateQueue('queue-1', { kind: 'steer' });
    assert.equal(calls[1].sessionId, address ? 'child' : 'main');
    assert.equal(calls[1].parentSessionId, address?.parentSessionId);
    assert.equal(calls[1].mode, address?.mode);
    session.queueMirror.acceptDurable({ type: 'user/message', data: { id: 'message-1', source: { rpcId: requestId } } });
    assert.equal(session.queueMirror.snapshot().length, 0, 'durable message retires the transient queue row');
    const failed = session.prompt(content, 'queue'); await new Promise(resolve => setImmediate(resolve));
    const failureRequestId = calls.at(-1).requestId;
    pending.shift()({ result: { ok: false, error: { code: 'transport-error', message: 'connection lost' } } }); await failed;
    assert.equal(session.queueMirror.snapshot().length, 0);
    const retry = session.prompt(content, 'queue'); await new Promise(resolve => setImmediate(resolve));
    assert.equal(calls.at(-1).requestId, failureRequestId, 'retry after an uncertain reply reuses admission identity');
    pending.shift()({ result: { ok: true, value: { accepted: true, messageId: 'retry-message' } } }); await retry;
  }
  console.log('PASS session queues: main/child delivery, sending, reconnect, acknowledgement merge, retry and durable handoff');
})().catch(error => { console.error(error); process.exitCode = 1; });
