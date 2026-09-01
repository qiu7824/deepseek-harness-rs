const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const bundlePath = path.resolve(__dirname, "../../web/dist/plugins/ui-conversation.js");
const source = fs.readFileSync(bundlePath, "utf8");
const start = source.indexOf("\t\tfunction turnUsageBuckets(value) {");
assert.notEqual(start, -1, "turnUsageBuckets must exist in the production bundle");

const open = source.indexOf("{", start);
let depth = 0;
let quote = null;
let escaped = false;
let end = -1;
for (let index = open; index < source.length; index += 1) {
	const char = source[index];
	if (quote !== null) {
		if (escaped) escaped = false;
		else if (char === "\\") escaped = true;
		else if (char === quote) quote = null;
		continue;
	}
	if (char === '"' || char === "'" || char === "`") {
		quote = char;
		continue;
	}
	if (char === "{") depth += 1;
	else if (char === "}" && --depth === 0) {
		end = index + 1;
		break;
	}
}
assert.notEqual(end, -1, "turnUsageBuckets function boundary must be readable");

const turnUsageBuckets = vm.runInNewContext(`(${source.slice(start, end)})`, { Number });
const unknown = turnUsageBuckets({ uncachedInputTokens: 4, outputTokens: 6, totalTokens: 10 });
assert.equal(unknown.totalTokens, 10);
assert.equal(Object.hasOwn(unknown, "cacheReadTokens"), false);
assert.equal(Object.hasOwn(unknown, "cacheWriteTokens"), false);
assert.equal(Object.hasOwn(unknown, "reasoningTokens"), false);
const known = turnUsageBuckets({ uncachedInputTokens: 4, outputTokens: 6, totalTokens: 14, cacheReadTokens: 3, cacheWriteTokens: 1, reasoningTokens: 2 });
assert.equal(known.cacheReadTokens, 3);
assert.equal(known.cacheWriteTokens, 1);
assert.equal(known.reasoningTokens, 2);
console.log("turn usage optional buckets: ok");
