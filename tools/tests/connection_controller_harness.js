const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const bundlePath = path.resolve(__dirname, "../../web/dist/plugins/connection.js");
const source = fs.readFileSync(bundlePath, "utf8");
const start = source.indexOf("var ConnectionController = class");
const end = source.indexOf("\n\t\t//#endregion", start);
assert.notEqual(start, -1, "connection bundle has ConnectionController");
assert.notEqual(end, -1, "connection bundle has controller section end");

const controllerSource = source
  .slice(start, end)
  .replace("var ConnectionController = class", "globalThis.ConnectionController = class");
const context = {
  AbortController,
  console: { error() {}, warn() {} },
  Math,
  Promise,
  clearTimeout,
  setTimeout,
};
vm.runInNewContext(
  `const CONNECTION_DEFAULTS = { backoffBaseMs: 500, backoffFactor: 2, backoffMaxMs: 10000, generationReadyTimeoutMs: 3000 };\n` +
    `const MANUAL_RECONNECT = new Error("manual");\n` +
    `const NETWORK_STATE_CHANGED = new Error("network");\n` +
    `function sleep(ms, signal) { return new Promise((resolve) => { const timer = setTimeout(resolve, ms); signal?.addEventListener("abort", () => { clearTimeout(timer); resolve(); }, { once: true }); }); }\n` +
    `function waitForAbort(signal) { return signal.aborted ? Promise.resolve() : new Promise((resolve) => signal.addEventListener("abort", resolve, { once: true })); }\n` +
    `function waitForReady(ready, timeoutMs, signal) { return new Promise((resolve, reject) => { let settled=false; const timer=setTimeout(()=>finish(new Error("timeout")), timeoutMs); const aborted=()=>finish(new Error("aborted")); const finish=(error,value)=>{ if(settled)return; settled=true; clearTimeout(timer); signal.removeEventListener("abort",aborted); error?reject(error):resolve(value); }; signal.addEventListener("abort",aborted,{once:true}); ready.then(value=>finish(null,value),error=>finish(error)); }); }\n` +
    controllerSource,
  context,
);
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

  process.stdout.write("connection controller harness: ok\n");
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
