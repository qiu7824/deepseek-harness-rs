import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const bundle = fs.readFileSync(new URL("../../web/dist/plugins/client-runtime.js", import.meta.url), "utf8");
const start = bundle.indexOf("\t\tfunction eventEndSeq(");
const end = bundle.indexOf("\t\tvar Session = class", start);
assert.notEqual(start, -1, "event range helpers are missing");
assert.notEqual(end, -1, "Session class boundary is missing");
const source = bundle.slice(start, end)
  + "\nthis.__compactSingleHistoryPage = compactSingleHistoryPage;"
  + "\nthis.__eventStartSeq = eventStartSeq;"
  + "\nthis.__eventEndSeq = eventEndSeq;";
const sandbox = {};
vm.runInNewContext(source, sandbox);

const events = Array.from({ length: 5000 }, (_, seq) => ({
  type: "assistant/chunk",
  seq,
  time: seq,
  data: {
    turn: 1,
    step: 1,
    chunk: { type: "text-delta", index: 0, text: "x" },
  },
}));
const original = events[0].data.chunk.text;
const views = Array.from({ length: events.length }, () => undefined);
const compacted = sandbox.__compactSingleHistoryPage(events, views);
assert.equal(compacted.events.length, 1);
assert.equal(compacted.views.length, 1);
assert.equal(compacted.events[0].data.chunk.text.length, 5000);
assert.equal(sandbox.__eventStartSeq(compacted.events[0]), 0);
assert.equal(sandbox.__eventEndSeq(compacted.events[0]), 4999);
assert.equal(events[0].data.chunk.text, original, "compaction must not mutate retained wire events");
const completed = { type: "assistant/message", seq: 5000, time: 5000, sourceEventSeqs: events.map(event => event.seq), data: { text: "x".repeat(5000) } };
const settled = sandbox.__compactSingleHistoryPage([...events, completed], [...views, undefined]);
assert.equal(settled.events.length, 1, "completed text must not retain its duplicated stream deltas");
assert.equal(sandbox.__eventStartSeq(settled.events[0]), 0);
assert.equal(sandbox.__eventEndSeq(settled.events[0]), 5000);
assert.equal(completed.data.__historyStartSeq, undefined, "range hints must not mutate the full wire message");
console.log(JSON.stringify({ rawEvents: events.length, compactedEvents: compacted.events.length, completedEvents: settled.events.length }));
