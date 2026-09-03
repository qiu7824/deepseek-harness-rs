import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const bundle = fs.readFileSync(new URL("../../web/dist/plugins/ui-conversation.js", import.meta.url), "utf8");
const start = bundle.indexOf("\t\tfunction turnNavigatorWindow(");
const end = bundle.indexOf("\n\t\tfunction TurnNavigator", start);
assert.notEqual(start, -1, "turnNavigatorWindow must exist in the production bundle");
assert.notEqual(end, -1, "turnNavigatorWindow end marker");
const sandbox = {};
vm.runInNewContext(`${bundle.slice(start, end)}\nglobalThis.__window = turnNavigatorWindow;`, sandbox);
const windowFor = sandbox.__window;

const first = windowFor(10_000, 0, 420);
assert.deepEqual({ ...first }, { start: 0, end: 56 });
assert.ok(first.end - first.start <= 64);

const middle = windowFor(10_000, 60_000, 420);
assert.ok(middle.start <= 5_000 && middle.end > 5_000);
assert.ok(middle.end - middle.start <= 64);

const last = windowFor(10_000, 120_000, 420);
assert.equal(last.end, 10_000);
assert.ok(last.end - last.start <= 64);

const small = windowFor(20, 0, 420);
assert.deepEqual({ ...small }, { start: 0, end: 20 });
console.log(JSON.stringify({ first, middle, last, small }));
