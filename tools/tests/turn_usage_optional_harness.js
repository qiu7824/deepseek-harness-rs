const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const bundlePath = process.argv[2] ? path.resolve(process.argv[2]) : path.resolve(__dirname, "../../web/dist/plugins/ui-conversation.js");
const source = fs.readFileSync(bundlePath, "utf8");
function functionSource(name, text = source) {
const start = text.indexOf(`\t\tfunction ${name}(`);
assert.notEqual(start, -1, `${name} must exist in the production bundle`);

const open = text.indexOf("{", start);
let depth = 0;
let quote = null;
let escaped = false;
let end = -1;
for (let index = open; index < text.length; index += 1) {
	const char = text[index];
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
assert.notEqual(end, -1, `${name} function boundary must be readable`);
return text.slice(start, end);
}

const turnUsageBuckets = vm.runInNewContext(`(${functionSource("turnUsageBuckets")})`, { Number });
const unknown = turnUsageBuckets({ uncachedInputTokens: 4, outputTokens: 6, totalTokens: 10 });
assert.equal(unknown.totalTokens, 10);
assert.equal(Object.hasOwn(unknown, "cacheReadTokens"), false);
assert.equal(Object.hasOwn(unknown, "cacheWriteTokens"), false);
assert.equal(Object.hasOwn(unknown, "reasoningTokens"), false);
const known = turnUsageBuckets({ uncachedInputTokens: 4, outputTokens: 6, totalTokens: 14, cacheReadTokens: 3, cacheWriteTokens: 1, reasoningTokens: 2 });
assert.equal(known.cacheReadTokens, 3);
assert.equal(known.cacheWriteTokens, 1);
assert.equal(known.reasoningTokens, 2);
const cacheHitPercent = vm.runInNewContext(`(${functionSource("cacheHitPercent")})`, { Number });
const cacheUsageLabel = vm.runInNewContext(`(${functionSource("cacheUsageLabel")})`, { cacheHitPercent });
const t = (key, values) => `${key}${values ? `:${values.percent}` : ""}`;
const usage = { uncachedInputTokens: 200, outputTokens: 10, cacheReadTokens: 800, cacheWriteTokens: 0 };
assert.equal(cacheHitPercent(usage), null, "legacy totals cannot identify unreported requests");
assert.equal(cacheUsageLabel(usage, t), "stats.cacheUnavailable");
const unavailable = { ...usage, cacheReadTokens: 0, cacheStatistics: { reportedSamples: 0, unreportedSamples: 1, reportedInputTokens: 0 } };
assert.equal(cacheUsageLabel(unavailable, t), "stats.cacheUnavailable");
const miss = { ...unavailable, cacheStatistics: { reportedSamples: 1, unreportedSamples: 0, reportedInputTokens: 200 } };
assert.equal(cacheUsageLabel(miss, t), "stats.cacheHit:0", "explicit zero is a reported cache miss");
const hit = { ...usage, cacheStatistics: { reportedSamples: 1, unreportedSamples: 0, reportedInputTokens: 1000 } };
assert.equal(cacheUsageLabel(hit, t), "stats.cacheHit:80");
const mixed = { ...hit, uncachedInputTokens: 9200, cacheStatistics: { ...hit.cacheStatistics, unreportedSamples: 1 } };
assert.equal(cacheUsageLabel(mixed, t), "stats.cacheHitPartial:80", "unreported requests do not dilute the disclosed cache hit ratio");
const connection = fs.readFileSync(path.join(path.dirname(bundlePath), "connection.js"), "utf8");
const usageSampleOf = vm.runInNewContext(`(${functionSource("usageSampleOf", connection)})`);
const tokenUsageOf = vm.runInNewContext(`(${functionSource("tokenUsageOf", connection)})`, { usageSampleOf, Number, Object });
const fixture = tokenUsageOf([
	{ type: "assistant/chunk", data: { turn: 1, step: 1, chunk: { type: "usage", usage: { inputTokens: 1000, outputTokens: 1 } } } },
	{ type: "assistant/message", data: { turn: 1, step: 1, usage: { inputTokens: 200, outputTokens: 10, cacheReadTokens: 800 } } },
	{ type: "assistant/message", data: { turn: 1, step: 2, usage: { inputTokens: 9000, outputTokens: 20 } } }
]);
assert.equal(fixture.cacheStatistics.reportedSamples, 1);
assert.equal(fixture.cacheStatistics.unreportedSamples, 1);
assert.equal(fixture.cacheStatistics.reportedInputTokens, 1000);
assert.equal(cacheUsageLabel(fixture, t), "stats.cacheHitPartial:80");
console.log("turn usage optional buckets: ok");
