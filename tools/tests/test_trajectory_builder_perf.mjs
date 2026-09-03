import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const bundle = fs.readFileSync(new URL("../../web/dist/plugins/ui-trajectory.js", import.meta.url), "utf8");
const start = bundle.indexOf("\t\tconst EMPTY_LIST = [];");
const end = bundle.indexOf("\t\t/** Trajectory target factory", start);
assert.notEqual(start, -1, "trajectory builder start marker");
assert.notEqual(end, -1, "trajectory builder end marker");
const source = `${bundle.slice(start, end)}\nglobalThis.__TrajectorySnapshotBuilder = TrajectorySnapshotBuilder;`;
const sandbox = { Map, Set };
vm.runInNewContext(source, sandbox, { filename: "trajectory-builder.js" });
const Builder = sandbox.__TrajectorySnapshotBuilder;
assert.equal(typeof Builder, "function");

const nodes = [];
for (let index = 0; index < 2000; index += 1) {
  nodes.push({
    key: `node-${index}`,
    anchorSeq: index,
    location: { kind: "turn", turn: { turn: index } },
    data: {
      kind: "node",
      node: { kind: "event", seq: index },
    },
  });
}
const assistant = {
  key: "assistant-live",
  anchorSeq: 3000,
  location: { kind: "step", turn: { turn: 3000 }, step: { step: 1 } },
  data: {
    kind: "assistant",
    partial: { turn: 3000, step: 1, blocks: [{ kind: "text", text: "a" }] },
    request: {
      purpose: "assistant",
      startSeq: 3000,
      turn: 3000,
      step: 1,
      startedAt: 1,
      completedAt: null,
      status: "running",
    },
  },
};
nodes.push(assistant);

const builder = new Builder();
const initial = builder.replace({ nodes, timeline: null });
assert.equal(initial.partial.blocks[0].text, "a");

let fullScans = 0;
const contributions = builder.contributions;
builder.contributions = new Proxy(contributions, {
  get(target, property, receiver) {
    if (property === Symbol.iterator) {
      return function* countedIterator() {
        fullScans += 1;
        yield* target;
      };
    }
    return Reflect.get(target, property, receiver);
  },
});

let current = assistant;
let next;
const started = performance.now();
for (let index = 0; index < 500; index += 1) {
  const updated = {
    ...current,
    data: {
      ...current.data,
      partial: { turn: 3000, step: 1, blocks: [{ kind: "text", text: `chunk-${index}` }] },
      request: { ...current.data.request },
    },
  };
  next = builder.apply({ upserts: [updated], timeline: null });
  current = updated;
}
assert.equal(next.partial.blocks[0].text, "chunk-499");
assert.equal(fullScans, 0, "partial-only assistant updates must not rescan settled trajectory history");
const partialElapsedMs = performance.now() - started;
fullScans = 0;
const settled = {
  ...current,
  data: {
    kind: "assistant",
    node: { kind: "assistant", seq: 4000, turn: 3000, step: 1, blocks: [] },
    partial: null,
    request: {
      ...current.data.request,
      completedAt: 2,
      status: "complete",
      resultSeq: 4000,
    },
  },
};
const finalSnapshot = builder.apply({ upserts: [settled], timeline: null });
assert.equal(finalSnapshot.partial, null);
assert.ok(finalSnapshot.eventNodes.some((node) => node.seq === 4000));
assert.ok(fullScans > 0, "settlement must rebuild the finalized trajectory snapshot");
console.log(JSON.stringify({ settledNodes: nodes.length - 1, partialUpdates: 500, partialFullScans: 0, settlementFullScans: fullScans, partialElapsedMs }));
