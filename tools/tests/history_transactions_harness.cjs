const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const text = fs.readFileSync(path.join(__dirname, '../../web/dist/plugins/client-runtime.js'), 'utf8');
const start = text.indexOf('const HISTORY_PAGE_MESSAGES'), end = text.indexOf('\n\t\tvar SessionManager = class', start);
const ctx = { console, conversationInput: x => x, transportError: error => ({ ok: false, error }) };
vm.runInNewContext(text.slice(start, end) + ';this.TestSession = Session;', ctx);
const flush = () => new Promise(resolve => setImmediate(resolve));
const entry = (seq, type = 'user/message', size = 10) => ({ event: { seq, type, time: seq, data: { text: 'x'.repeat(size) } }, view: undefined });
const page = (first, last) => Array.from({ length: last - first + 1 }, (_, index) => entry(first + index));
const result = (entries, after = false, before = true) => ({ result: { ok: true, value: { events: entries, hasMore: before, hasMoreBefore: before, hasMoreAfter: after } } });
function fixture(first = 100, last = 111) {
  const session = Object.create(ctx.TestSession.prototype), requests = [];
  Object.assign(session, { events: [], views: [], historyPages: [], historyTargetSeq: null, historyNavigationRevision: 0,
    historyNavigationReason: null, navigationRequest: null, gapRequest: null, historyFetch: null,
    hasMoreBefore: true, hasMoreAfter: false, openState: 'open', openGeneration: 0, openPromise: null,
    loadingOlder: false, loadingNewer: false, olderRequest: null, newerRequest: null, jumpPromise: null,
    liveBuffer: [], liveBufferBytes: 0, liveBufferDroppedThrough: -1, tailRepairNeeded: false, stitching: false,
    readingAwayFromTail: false, pending: new Map(), pendingRev: 0, subscribedLastSeq: null,
    notifier: { markDirty() {}, markFrameDirty() {} }, queueMirror: { acceptDurable: () => false },
    projections: { seed() {} }, conversation: { replaceWindow() {}, prepend() {}, append() { return 'immediate'; } },
    history(payload) { return new Promise(resolve => requests.push({ payload, resolve })); }
  });
  session.installWindow(page(first, last), true);
  return { session, requests };
}
(async () => {
  let checks = 0;
  {
    const { session: s, requests } = fixture(); s.historyTargetSeq = 100; s.hasMoreAfter = true;
    const olderWindow = s.events, pending = s.loadNewer(); await flush();
    const jump = s.loadAround(20, true); await flush();
    assert.equal(requests.length, 1, 'navigation waits for the actual page promise');
    requests[0].resolve(result(page(112, 123), false)); await pending; await flush();
    assert.equal(s.events, olderWindow, 'superseded forward page cannot replace the reader window');
    assert.equal(s.hasMoreAfter, true, 'stale response cannot report historical window as the tail');
    requests[1].resolve(result(page(20, 31), true)); assert.equal(await jump, true);
    assert.equal(s.baseSeq, 20); assert.equal(s.historyNavigationRevision, 1); checks++;
  }
  {
    const { session: s, requests } = fixture(); const pending = s.loadOlder(); await flush();
    let settled = false; pending.then(() => { settled = true; }); s.cancelHistoryPaging();
    const newer = s.loadAround(50, true); const newest = s.loadAround(10, true); await flush();
    assert.equal(settled, false, 'cancellation must not pretend the network request settled');
    assert.equal(requests.length, 1);
    requests[0].resolve(result(page(88, 99))); await pending; await flush();
    assert.equal(requests.length, 2, 'coalesced navigation sends only the last queued target');
    assert.equal(requests[1].payload.afterSeq, 10);
    requests[1].resolve(result(page(10, 21), true));
    assert.equal(await newer, false); assert.equal(await newest, true); assert.equal(s.baseSeq, 10); checks++;
  }
  {
    const { session: s, requests } = fixture(); const first = s.loadOlder(); await flush();
    s.cancelHistoryPaging(); const second = s.loadOlder();
    requests[0].resolve(result(page(88, 99))); await first; await flush();
    assert.equal(s.loadingOlder, true, 'old finally cannot clear a replacement request loading flag');
    requests[1].resolve(result(page(88, 99))); await second; assert.equal(s.baseSeq, 88); checks++;
  }
  {
    const { session: s, requests } = fixture(); const pending = s.loadThrough(10); await flush();
    s.cancelHistoryPaging(); requests[0].resolve(result(page(0, 99))); await pending;
    assert.equal(s.baseSeq, 100); assert.equal(requests.length, 1); checks++;
  }
  {
    const { session: s, requests } = fixture(); s.readingAwayFromTail = true;
    const original = s.events; s.acceptLiveEvent(entry(112).event);
    assert.equal(s.events, original); assert.equal(s.events.length, 12); assert.equal(s.hasMoreAfter, true);
    const latest = s.returnLatest(); await flush(); requests[0].resolve(result(page(110, 121))); await latest;
    assert.equal(s.historyTargetSeq, null); assert.equal(s.historyNavigationReason, 'latest'); assert.equal(s.liveBuffer.length, 0); checks++;
  }
  {
    const { session: s } = fixture(); s.historyTargetSeq = 100;
    for (let seq = 112; seq < 18112; seq++) s.acceptLiveEvent(entry(seq, 'assistant/chunk', 400).event);
    assert.ok(s.liveBuffer.length <= 4096); assert.ok(s.liveBufferBytes <= 8 * 1024 * 1024);
    assert.ok(s.liveBufferDroppedThrough > 111); const tail = s.windowTailSeq();
    s.historyTargetSeq = null; s.stitchLiveBuffer();
    assert.equal(s.windowTailSeq(), tail, 'overflow cannot silently stitch a non-contiguous window');
    assert.equal(s.tailRepairNeeded, true);
    s.installWindow(page(18100, 18111), true);
    assert.equal(s.liveBuffer.length, 0); assert.equal(s.liveBufferBytes, 0); assert.equal(s.tailRepairNeeded, false); checks++;
  }
  {
    const { session: s } = fixture();
    for (let seq = 112; seq < 160; seq++) s.bufferLive(entry(seq, 'tool/result', 300000).event);
    assert.ok(s.liveBufferBytes <= 8 * 1024 * 1024); assert.ok(s.liveBuffer.length < 48);
    s.bufferLive(entry(160, 'tool/result', 5 * 1024 * 1024).event);
    assert.equal(s.liveBuffer.length, 0, 'one oversized live result is recovered from history without retained duplication'); checks++;
  }
  {
    const { session: s } = fixture(0, 0); let seq = 1;
    for (let step = 0; step < 30; step++) {
      s.appendLive(entry(seq++, 'step/start').event);
      for (let item = 0; item < 3; item++) s.appendLive(entry(seq++, 'tool/result', 350000).event);
      s.appendLive(entry(seq++, 'step/end').event);
    }
    assert.ok(s.baseSeq > 0, 'long multi-step turns release completed pages before turn/end');
    assert.ok(s.historyPages.length <= 5); assert.ok(s.historyPages.reduce((n, p) => n + p.bytes, 0) <= 8 * 1024 * 1024);
    assert.equal(s.events.length, s.historyPages.reduce((n, p) => n + p.eventCount, 0)); checks++;
  }
  {
    const { session: s, requests } = fixture(); s.historyTargetSeq = 100; s.readingAwayFromTail = true; s.hasMoreAfter = true;
    const old = s.loadNewer(); await flush(); const resync = s.resync(); await flush();
    assert.equal(requests.length, 2, 'a new connection must not wait for an unreachable old connection');
    requests[1].resolve(result(page(100, 111), false)); await resync;
    requests[0].resolve(result(page(112, 123), true)); await old;
    assert.equal(s.historyTargetSeq, 100, 'reconnect retains explicit reading intent even when window touches tail');
    assert.equal(s.historyNavigationReason, 'resync'); assert.equal(s.openState, 'open'); assert.equal(s.windowTailSeq(), 111); checks++;
  }
  {
    const { session: s, requests } = fixture(); s.bufferLive(entry(114).event);
    const gap = s.repairGap(); await flush(); const latest = s.returnLatest(); await flush();
    assert.equal(requests.length, 1); requests[0].resolve(result(page(112, 114))); await gap; await flush();
    assert.equal(requests.length, 2, 'superseding a gap repair still fetches an authoritative tail');
    requests[1].resolve(result(page(112, 115))); await latest;
    assert.equal(s.windowTailSeq(), 115); assert.equal(s.liveBuffer.length, 0); checks++;
  }
  console.log(JSON.stringify({ checks, maximumHistoryRequestsPerConnection: 1, liveBufferBytesCap: 8 * 1024 * 1024 }));
})().catch(error => { console.error(error); process.exitCode = 1; });
