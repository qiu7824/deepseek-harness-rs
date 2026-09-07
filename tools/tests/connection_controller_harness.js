const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const bundlePath = path.resolve(__dirname, "../../web/dist/plugins/connection.js");
const source = fs.readFileSync(bundlePath, "utf8");
const start = source.indexOf("const CONNECTION_DEFAULTS =");
const end = source.indexOf("\n\t\t//#endregion", start);
assert.notEqual(start, -1, "connection bundle has production recovery section");
assert.notEqual(end, -1, "connection bundle has recovery section end");
const context = { AbortController, console: { error() {}, warn() {} }, Math, Promise, clearTimeout, setTimeout };
vm.runInNewContext(source.slice(start, end), context);
const ConnectionController = context.ConnectionController;

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const waitFor = async (predicate, message) => {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    if (predicate()) return;
    await tick();
  }
  assert.fail(message);
};

function createFixture() {
  const state = {
    generations: 0,
    controllers: [],
    naturalEnds: [],
    connectionStates: [],
  };
  const source = (signal, ready) => {
    state.generations += 1;
    state.controllers.push(signal);
    ready({ home: "C:/Users/Test" });
    let endNaturally;
    const naturalEnd = new Promise((resolve) => {
      endNaturally = resolve;
    });
    state.naturalEnds.push(endNaturally);
    return Promise.race([
      naturalEnd,
      new Promise((resolve) => signal.addEventListener("abort", resolve, { once: true })),
    ]);
  };
  const controller = new ConnectionController(source, {
    onStateChange(value) {
      state.connectionStates.push(value);
    },
  }, {
    backoffBaseMs: 1,
    backoffFactor: 1,
    backoffMaxMs: 1,
    generationReadyTimeoutMs: 25,
  });
  return { controller, state };
}

(async () => {
  {
    const { controller, state } = createFixture();
    controller.start();
    await waitFor(() => state.generations === 1, "initial generation starts");
    controller.reconnect();
    await waitFor(() => state.generations === 2, "manual reconnect creates one replacement generation");
    await tick();
    await tick();
    assert.equal(state.generations, 2, "manual reconnect creates exactly one replacement generation");
    controller.stop();
  }

  {
    const { controller, state } = createFixture();
    controller.start();
    await waitFor(() => state.generations === 1, "initial generation starts");
    state.naturalEnds[0]();
    await waitFor(() => state.generations === 2, "natural end creates a replacement generation");
    await tick();
    assert.equal(state.generations, 2, "natural end creates exactly one replacement generation");
    controller.stop();
  }

  {
    const { controller, state } = createFixture();
    controller.start();
    await waitFor(() => state.generations === 1, "initial generation starts");
    controller.setNetworkAvailable(false);
    await tick();
    await tick();
    assert.equal(state.generations, 1, "offline pauses retry");
    assert.equal(state.connectionStates.at(-1), "disconnected", "offline publishes disconnected");
    controller.setNetworkAvailable(true);
    await waitFor(() => state.generations === 2, "online resumes exactly one generation");
    await tick();
    assert.equal(state.generations, 2, "online resumes exactly one generation");
    controller.stop();
  }

  {
    let attempts = 0, active = 0, maxActive = 0;
    const readyCallbacks = [], states = [];
    const controller = new ConnectionController((signal, ready) => {
      attempts++; active++; maxActive = Math.max(maxActive, active); readyCallbacks.push(ready);
      if (attempts > 6) ready({ recovered: true });
      return new Promise(resolve => signal.addEventListener("abort", () => setTimeout(() => { active--; resolve(); }, 3), { once: true }));
    }, { onConnected: host => states.push(host) }, { backoffBaseMs: 1, backoffFactor: 2, backoffMaxMs: 2, generationReadyWarningMs: 1, generationReadyTimeoutMs: 4 });
    controller.start();
    await waitFor(() => attempts >= 7, "hard readiness deadline continues beyond the final backoff tier");
    await waitFor(() => states.length === 1, "service recovery connects without page refresh");
    readyCallbacks[0]({ stale: true });
    await tick();
    assert.equal(states.length, 1, "stale ready cannot publish a replaced generation");
    assert.equal(maxActive, 1, "aborted generation cleanup completes before replacement starts");
    controller.stop();
    await waitFor(() => active === 0, "stop releases the live generation");
    const stoppedAttempts = attempts; await new Promise(resolve => setTimeout(resolve, 15));
    assert.equal(attempts, stoppedAttempts, "stop cancels retries");
  }
  {
    const controller = new ConnectionController(() => {}, {}, { generationReadyTimeoutMs: Infinity, backoffMaxMs: -1, backoffFactor: NaN });
    assert.equal(controller.config.generationReadyTimeoutMs, 15000);
    assert.equal(controller.config.backoffMaxMs, 10000);
    assert.equal(controller.config.backoffFactor, 2);
    const abort = new AbortController(); abort.abort();
    await assert.rejects(context.waitForReady(new Promise(() => {}), 1000, abort.signal, 10), /aborted/);
  }
  {
    let active = 0, maximum = 0, attempts = 0;
    const controller = new ConnectionController((signal, ready) => {
      attempts++; active++; maximum = Math.max(maximum, active); ready({});
      return new Promise(resolve => signal.addEventListener('abort', () => setTimeout(() => { active--; resolve(); }, 10), { once: true }));
    });
    controller.start(); await waitFor(() => attempts === 1, 'first lifecycle starts');
    controller.stop(); controller.start();
    await waitFor(() => attempts === 2, 'stop followed by start starts a new lifecycle');
    assert.equal(maximum, 1, 'quick stop/start also waits for the previous source to release');
    controller.stop(); await waitFor(() => active === 0, 'restarted lifecycle stops cleanly');
  }
  {
    const sockets = [], delivered = [];
    class Socket extends EventTarget {
      static CONNECTING = 0; static OPEN = 1;
      readyState = 0;
      constructor() { super(); sockets.push(this); }
      close() { this.readyState = 3; }
      message(value) { const event = new Event('message'); event.data = JSON.stringify(value); this.dispatchEvent(event); }
    }
    const start = source.indexOf('var WebApiClient = class'), end = source.indexOf('\n\t\t//#endregion', start);
    const transport = { AbstractApiClient: class { resolveBase() { return 'http://localhost'; } onEnvelope(value) { delivered.push(value); } }, URL, WebSocket: Socket,
      serverRequestSchema: { parse: value => value } };
    vm.runInNewContext(source.slice(start, end), transport);
    const api = new transport.WebApiClient(), abort = new AbortController();
    const stream = api.readWebSocket('/events', abort.signal, { parse: value => value });
    const pending = stream.next();
    abort.abort();
    assert.equal((await pending).done, true, 'abort releases a waiting transport even without a socket close event');
    sockets[0].message({ rpcId: 'stale', payload: {} });
    assert.equal(delivered.length, 0, 'cancelled WebSocket generation cannot publish late frames');
  }
  process.stdout.write("connection controller harness: ok (production readiness, persistent retries, stale ready and cleanup)\n");
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
