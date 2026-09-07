"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const modules = process.env.DSH_REACT_TEST_MODULES || process.argv[2];
if (!modules) throw new Error("Pass the React/jsdom node_modules directory");
const { JSDOM } = require(path.join(modules, "jsdom"));
const React = require(path.join(modules, "react"));
const ReactClient = require(path.join(modules, "react-dom/client"));
const dom = new JSDOM("<!doctype html><body><main id='root'></main></body>", {
  url: "http://127.0.0.1:59800",
  pretendToBeVisual: true,
});
Object.assign(global, {
  window: dom.window,
  document: dom.window.document,
  localStorage: dom.window.localStorage,
  CustomEvent: dom.window.CustomEvent,
  HTMLElement: dom.window.HTMLElement,
  IS_REACT_ACT_ENVIRONMENT: true,
});
Object.defineProperty(global, "navigator", { configurable: true, value: dom.window.navigator });
window.confirm = () => true;
window.HTMLElement.prototype.scrollIntoView = function scrollIntoView() {};
// The loader may mark another plugin's asynchronous style with this owner.
const unrelatedStyle=document.createElement('style');unrelatedStyle.dataset.plugin='dsh-better-sidebar/git-terminal';unrelatedStyle.textContent='.unrelated{display:block}';document.head.appendChild(unrelatedStyle);

const requests = [];
const xtermCalls = [];
const xtermInstances = [];
class FakeXterm {
  constructor(options) { this.options = options; this.cols = options.cols; this.rows = options.rows; this.data = null; xtermInstances.push(this); }
  open(host) { this.host = host; xtermCalls.push("open"); }
  onData(callback) { this.data = callback; return { dispose: () => xtermCalls.push("data-dispose") }; }
  focus() { xtermCalls.push("focus"); }
  write(value) { xtermCalls.push(["write", value]); }
  reset() { xtermCalls.push("reset"); }
  resize(cols, rows) { this.cols = cols; this.rows = rows; xtermCalls.push(["resize", cols, rows]); }
  dispose() { xtermCalls.push("dispose"); }
}
let terminals = [{ id: "pty-1", name: "构建终端", type: "shell", pid: 42, status: "running" }];
const response = (value, ok = true) => ({ ok, status: ok ? 200 : 400, json: async () => value });
global.fetch = window.fetch = async (raw, options = {}) => {
  const url = new URL(raw, window.location.href);
  const body = options.body ? JSON.parse(options.body) : null;
  requests.push({ path: url.pathname, query: Object.fromEntries(url.searchParams), body });
  if (url.pathname.endsWith("/git-status")) {
    const alternate = url.searchParams.get("sessionId") === "session-b";
    const repository = alternate ? "D:/repo-b" : "C:/repo/main";
    const worktree = alternate ? "D:/repo-b" : "C:/repo/agent";
    return response({
    repository, worktree, branch: alternate ? "main" : "agent", upstream: alternate ? null : "origin/agent", ahead: alternate ? 0 : 1, behind: 0,
    branches: ["main", "agent"],
    repositories: alternate ? [{ path: repository, label: "repo-b", relativePath: "." }] : [{ path: "C:/repo/main", label: "main", relativePath: "." }, { path: "C:/repo/sub", label: "sub", relativePath: "packages/sub" }],
    worktrees: alternate ? [{ path: worktree, branch: "main", current: true, changes: 1 }] : [{ path: "C:/repo/main", branch: "main", current: true, changes: 0 }, { path: "C:/repo/agent", branch: "agent", current: false, changes: 1 }],
    entries: [{ path: alternate ? "src/b.js" : "src/app.js", status: " M", indexStatus: " ", worktreeStatus: "M", group: "unstaged" }],
    });
  }
  if (url.pathname.endsWith("/git-log")) return response({ entries: [{ hash: "a".repeat(40), shortHash: "aaaaaaa", subject: "Add terminal view", author: "Tester", date: "2026-09-06T00:00:00+08:00", refs: "HEAD -> agent" }], hasMore: false });
  if (url.pathname.endsWith("/git-diff") || url.pathname.endsWith("/git-commit-diff")) return response({ path: url.searchParams.get("path") || url.searchParams.get("revision"), diff: "diff --git a/src/app.js b/src/app.js\n@@ -1 +1 @@\n-old\n+new\n", truncated: false });
  if (url.pathname.endsWith("/meta")) {
    const alternate = url.searchParams.get("sessionId") === "session-b";
    return response({ siteToken: "fixture", workspaceTitle: alternate ? "Fixture B" : "Fixture", workspaceKey: alternate ? "workspace-2" : "workspace-1" });
  }
  if (url.pathname.endsWith("/terminal-list")) {
    const alternate = url.searchParams.get("sessionId") === "session-b";
    return response({ entries: alternate ? [{ id: "pty-b", name: "B 终端", type: "shell", pid: 84, status: "running" }] : terminals });
  }
  if (url.pathname.endsWith("/terminal-read")) {
    const alternate = url.searchParams.get("sessionId") === "session-b";
    return response({ text: alternate ? "B ready\r\n> " : "\u001b[32mready\u001b[0m\r\n$ ", totalLines: 2, lineBegin: 0, lineEnd: 2, truncated: false });
  }
  if (url.pathname.endsWith("/terminal-action")) {
    if (body.action === "open") terminals = [...terminals, { id: "pty-2", name: body.name, type: "shell", pid: 43, status: "running" }];
    if (body.action === "close") terminals = terminals.filter(entry => entry.id !== body.terminalId);
    return response({ id: "pty-2", name: body.name, pid: 43, motd: "$ ", status: "running" });
  }
  if (url.pathname.endsWith("/git-action")) return response({ ok: true });
  return response({ message: `unexpected ${url.pathname}` }, false);
};

global.ResizeObserver = window.ResizeObserver = class ResizeObserver {
  constructor(callback) { this.callback = callback; }
  observe() { this.callback([{ contentRect: { width: 800, height: 500 } }]); }
  disconnect() {}
};

let exported;
window.__ModuleLoader__ = {
  load(definition) {
    exported = definition.factory(name => {
      assert.equal(name, "react");
      return React;
    });
  },
};
const rootPath = path.resolve(__dirname, "..", "..");
vm.runInThisContext(fs.readFileSync(path.join(rootPath, "release", "plugins", "dsh-better-sidebar", "lib", "git-terminal.js"), "utf8"));
const settle = () => new Promise(resolve => setTimeout(resolve, 30));
const act = async callback => React.act(async () => { await callback(); await settle(); });
const button = text => [...document.querySelectorAll("button")].find(node => node.textContent.trim() === text);

(async () => {
  const root = ReactClient.createRoot(document.getElementById("root"));
  await act(() => root.render(React.createElement(exported.GitWorkbench, { sessionId: "session-a" })));
  assert.ok(document.querySelector('style[data-dsh-git-terminal-style]'),'independent style marker is required even with a loader-owned tag');
  assert.equal(window.getComputedStyle(document.querySelector('.dgt-shell')).display,'flex');
  assert.equal(window.getComputedStyle(document.querySelector('.dgt-toolbar')).display,'flex');
  assert.equal(document.querySelectorAll('select[aria-label^="Git "]').length, 3);
  assert.match(document.body.textContent, /packages\/sub/);
  assert.match(document.body.textContent, /src\/app\.js/);
  await act(() => root.render(React.createElement(exported.GitWorkbench, { sessionId: "session-b" })));
  const switched = requests.filter(item => item.path.endsWith("/git-status") && item.query.sessionId === "session-b").at(-1);
  assert.equal(switched.query.repository, undefined, "switching sessions must not send the previous repository target");
  assert.equal(switched.query.worktree, undefined, "switching sessions must not send the previous worktree target");
  assert.match(document.body.textContent, /repo-b/);
  assert.match(document.body.textContent, /src\/b\.js/);
  await act(() => root.render(React.createElement(exported.GitWorkbench, { sessionId: "session-a" })));
  await act(() => document.querySelector(".dgt-change").click());
  assert.ok(document.querySelector(".dgt-diff"));
  await act(() => button("并排").click());
  assert.ok(document.querySelector(".dgt-split"));
  await act(() => button("历史").click());
  assert.match(document.body.textContent, /Add terminal view/);
  await act(() => document.querySelector(".dgt-commit").click());
  assert.ok(button("拣选"));
  await act(() => button("还原提交").click());
  assert.ok(requests.some(item => item.path.endsWith("/git-action") && item.body.action === "revert" && item.body.revision === "a".repeat(40)));

  await act(() => root.render(React.createElement(exported.WorkbenchTerminal, { sessionId: "session-a" })));
  assert.match(document.body.textContent, /构建终端/);
  assert.match(document.body.textContent, /ready/);
  await act(() => root.render(React.createElement(exported.WorkbenchTerminal, { sessionId: "session-b" })));
  assert.match(document.body.textContent, /B 终端/);
  assert.doesNotMatch(document.body.textContent, /构建终端/);
  assert.ok(requests.some(item => item.path.endsWith("/terminal-read") && item.query.sessionId === "session-b" && item.query.terminalId === "pty-b"), "the reused terminal component must read only the new owner terminal");
  await act(() => root.render(React.createElement(exported.WorkbenchTerminal, { sessionId: "session-a" })));
  assert.match(document.body.textContent, /构建终端/);
  assert.ok(document.querySelector(".dgt-screen"), "an unavailable xterm asset falls back to the built-in ANSI renderer");
  const pin = document.querySelector('select[aria-label="固定终端范围"]');
  await act(() => {
    Object.getOwnPropertyDescriptor(window.HTMLSelectElement.prototype, "value").set.call(pin, "workspace");
    pin.dispatchEvent(new window.Event("change", { bubbles: true }));
  });
  const stored = JSON.parse(localStorage.getItem(exported.PIN_KEY));
  assert.equal(stored[0].scope, "workspace");
  assert.equal(stored[0].workspaceKey, "workspace-1");
  await act(() => document.querySelector(".dgt-screen").dispatchEvent(new window.KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true, cancelable: true })));
  assert.ok(requests.some(item => item.path.endsWith("/terminal-action") && item.body.action === "input" && item.body.text === "\u001b[A"));
  await act(() => new Promise(resolve => setTimeout(resolve, 150)));
  assert.ok(requests.some(item => item.path.endsWith("/terminal-action") && item.body.action === "resize" && item.body.rows > 1 && item.body.cols > 9));
  await act(() => button("+ 新建").click());
  assert.match(document.body.textContent, /终端 2/);
  await act(() => root.render(React.createElement("div", null, "切换渲染器")));
  window.Terminal = FakeXterm;
  await act(() => root.render(React.createElement(exported.WorkbenchTerminal, { sessionId: "session-a", key: "xterm-success" })));
  assert.ok(document.querySelector('.dgt-xterm-host[data-terminal-engine="xterm.js-5.5.0"]'));
  assert.ok(xtermCalls.includes("open"));
  assert.ok(xtermCalls.some(call => Array.isArray(call) && call[0] === "write" && call[1].includes("ready")));
  assert.ok(xtermCalls.some(call => Array.isArray(call) && call[0] === "resize" && call[1] > 9 && call[2] > 1));
  await act(() => xtermInstances.at(-1).data("\u001b[B"));
  assert.ok(requests.some(item => item.path.endsWith("/terminal-action") && item.body.action === "input" && item.body.text === "\u001b[B"));
  await act(() => root.render(React.createElement("div", null, "终端已卸载")));
  assert.ok(xtermCalls.includes("data-dispose"));
  assert.ok(xtermCalls.includes("dispose"));
  await act(() => root.unmount());
  console.log("PASS Git repositories/worktrees/history/diff and interactive pinned terminal DOM flows");
})().catch(error => {
  console.error(error);
  process.exitCode = 1;
}).finally(() => {
  dom.window.close();
  process.exit(process.exitCode || 0);
});
