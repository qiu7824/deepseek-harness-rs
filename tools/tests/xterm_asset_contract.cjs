"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const root = path.resolve(__dirname, "..", "..");
const source = fs.readFileSync(path.join(root, "release", "plugins", "dsh-better-sidebar", "lib", "xterm.js"), "utf8");
const sandbox = {
  console,
  setTimeout,
  clearTimeout,
  setInterval,
  clearInterval,
  navigator: { platform: "Win32", userAgent: "DSH xterm contract" },
};
sandbox.window = sandbox;
sandbox.self = sandbox;
sandbox.globalThis = sandbox;
vm.runInNewContext(source, sandbox, { filename: "xterm.js" });
assert.equal(typeof sandbox.Terminal, "function");

const terminal = new sandbox.Terminal({ cols: 20, rows: 5, scrollback: 20 });
terminal.write("first\r\n\u001b[31mred\u001b[0m", () => {
  assert.equal(terminal.buffer.active.getLine(0).translateToString(true), "first");
  assert.equal(terminal.buffer.active.getLine(1).translateToString(true), "red");
  terminal.resize(30, 8);
  assert.equal(terminal.cols, 30);
  assert.equal(terminal.rows, 8);
  terminal.write("\u001b[?1049halt\u001b[?1049l", () => {
    assert.equal(terminal.buffer.active.getLine(1).translateToString(true), "red");
    terminal.dispose();
    console.log("xterm.js 5.5.0 asset parser, alternate screen and resize verified");
  });
});
