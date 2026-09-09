"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const path = require("node:path");

const root = path.resolve(__dirname, "..", "..");
const source = fs.readFileSync(
  path.join(root, "release", "plugins", "dsh-better-sidebar", "lib", "git-terminal.js"),
  "utf8",
);

let exported;
global.window = {
  __ModuleLoader__: {
    load(definition) {
      assert.equal(definition.id, "dsh-better-sidebar/git-terminal");
      exported = definition.factory((name) => {
        assert.equal(name, "react");
        return {};
      });
    },
  },
};
vm.runInThisContext(source, { filename: "git-terminal.js" });
assert.ok(exported);

const parsed = exported.parseDiff([
  "diff --git a/src/a.js b/src/a.js",
  "--- a/src/a.js",
  "+++ b/src/a.js",
  "@@ -1,2 +1,2 @@",
  "-old",
  "+new",
  " same",
].join("\n"));
assert.equal(parsed.length, 1);
assert.equal(parsed[0].path, "src/a.js");
assert.deepEqual(parsed[0].hunks[0].lines.map((line) => line.kind), ["del", "add", "context"]);
assert.deepEqual(parsed[0].hunks[0].lines.map((line) => [line.left, line.right]), [[1, null], [null, 1], [2, 2]]);

const terminal = new exported.AnsiTerminalModel(40, 10).feed(
  "hello\rY\n\u001b[31mred\u001b[0m\u001b[2D!",
);
assert.equal(terminal.lines[0].map((cell) => cell.ch).join(""), "Yello");
assert.equal(terminal.lines[1][1].attr.fg, "#bf616a");
assert.equal(terminal.lines[1][3].attr.fg, "#bf616a");
assert.equal(terminal.lines[1][2].ch, "!");
assert.equal(terminal.lines[1][2].attr.fg, null);

const beforeAlt = terminal.lines.map((line) => line.map((cell) => cell.ch).join(""));
terminal.feed("\u001b[?1049hfull screen\u001b[?1049l");
assert.deepEqual(terminal.lines.map((line) => line.map((cell) => cell.ch).join("")), beforeAlt);

const sanitized = exported.safePins([
  { terminalId: "pty-1", homeSessionId: "session-a", workspaceKey: "workspace-a", name: "构建", scope: "workspace", pid: 12 },
  { terminalId: "pty-1", homeSessionId: "session-a", workspaceKey: "workspace-a", name: "重复", scope: "global" },
  { terminalId: "pty-2", homeSessionId: "session-b", workspaceKey: "", name: "非法", scope: "workspace" },
  { terminalId: "pty-3", homeSessionId: "session-c", workspaceKey: "", name: "全局", scope: "global" },
]);
assert.equal(sanitized.length, 2);
assert.equal(sanitized[0].name, "构建");
assert.equal(sanitized[1].scope, "global");

for (const required of [
  "git-log",
  "git-commit-diff",
  "dsh-better-sidebar:v2:pinned-terminals",
  "ResizeObserver",
  '"/xterm.js"',
  'data-terminal-engine": "xterm.js-5.5.0"',
  'action: "resize"',
  "固定到工作区",
  "固定到全局",
]) {
  assert.ok(source.includes(required), `missing ${required}`);
}

console.log("git and interactive terminal contract verified");

(async () => {
  const sent = [], errors = [];
  const queue = exported.createTerminalInputQueue(async (entry, text) => {
    sent.push({ ...entry, text }); await new Promise(resolve => setImmediate(resolve));
  }, error => errors.push(error.message));
  const a = {homeSessionId:'session-a',terminalId:'first'}, b = {homeSessionId:'session-b',terminalId:'second'};
  queue.enqueue(a, 'tail-a'); queue.enqueue(b, 'tail-b');
  a.terminalId = 'changed-after-input';
  await queue.flush();
  assert.deepEqual(sent, [{homeSessionId:'session-a',terminalId:'first',text:'tail-a'}, {...b,text:'tail-b'}]);
  sent.length = 0;
  queue.enqueue(b, 'z'.repeat(9000)); await queue.flush();
  assert.equal(sent.map(row => row.text).join(''), 'z'.repeat(9000));
  assert.ok(sent.every(row => row.text.length <= 4096));
  queue.enqueue(b, 'x'.repeat(1024 * 1024 + 1)); await queue.flush();
  assert.equal(errors.length, 1, 'oversized input is rejected visibly');
  const failed = exported.createTerminalInputQueue(async () => { throw Error('disconnected'); }, error => errors.push(error.message));
  failed.enqueue(b, 'a'.repeat(9000)); await failed.flush();
  assert.equal(errors.at(-1), 'disconnected');
  console.log('PASS terminal queue: captured owner, switch/unmount flush, serial chunking, bounded backlog and failed-stream stop');
})().catch(error => { console.error(error); process.exitCode = 1; });
