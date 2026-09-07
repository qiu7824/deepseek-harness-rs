const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const modules = path.resolve(process.argv[2]);
const { JSDOM } = require(path.join(modules, "jsdom"));
const React = require(path.join(modules, "react"));
const ReactDOM = require(path.join(modules, "react-dom/client"));
const dom = new JSDOM("<!doctype html><html><head></head><body><main id=\"root\"></main></body></html>", { pretendToBeVisual: true, url: "http://127.0.0.1:58080/" });
Object.assign(global, { window: dom.window, document: dom.window.document, Node: dom.window.Node, HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
const definitions = {};
window.__ModuleLoader__ = { load: definition => { definitions[definition.id] = definition; } };
vm.runInNewContext(fs.readFileSync(path.resolve(__dirname, "../../release/plugins/dsh-sidebar-workbench-suite/lib/client.js"), "utf8"), { window, document, URLSearchParams, AbortController: dom.window.AbortController, console, setInterval, clearInterval, setTimeout, clearTimeout, fetch: (...args) => global.fetch(...args) });
const registrations = { tabs: [], viewers: [] }, disposed = [];
let updatedTab = null;
const updatedTabs = [];
const sidebar = {
  registerTab(descriptor) { registrations.tabs.push(descriptor); return () => disposed.push("tab:" + descriptor.id); },
  registerFileViewer(descriptor) { registrations.viewers.push(descriptor); return () => disposed.push("viewer:" + descriptor.id); },
  updateTab(id, patch, scope) { updatedTab = { id, patch, scope }; updatedTabs.push(updatedTab); },
  getSnapshot() { return { prefs: { pluginSettings: {} } }; },
  subscribeState() { return () => {}; }
};
const sessions={},connection={};
let cleanup = null;
const settingWrites = [], slotEntries = [];
const computerSnapshot = { status: "ready", writable: true, value: { enabled: false, adapter: "auto", browserHeadless: true, maxBrowserSessions: 4, timeoutSeconds: 60, browserExecutable: "", command: "" } };
const computerScope = { getSnapshot: () => computerSnapshot, subscribe: () => () => {}, set: async (key, value) => { settingWrites.push([key, value]); } };
const settingsScope = { controls:{Switch:({checked,onChange,label,id,disabled})=>React.createElement('button',{id,disabled,role:'switch','aria-label':label,'aria-checked':checked,onClick:()=>onChange(!checked)})}, bind: ({ namespace }) => { assert.equal(namespace, "computer-use"); return computerScope; } };
const slots = { inject: (_name, factory) => factory(), register: (descriptor, component) => { slotEntries.push({ descriptor, component }); return () => {}; } };
const context = {
  betterSidebar: sidebar,
  settingsScope,
  slots,
  get(id) { return { betterSidebar: sidebar, connection, sessions, settingsScope, slots }[id]; },
  effect(factory) { cleanup = factory(); return cleanup; }
};
(async () => {
const plugin = definitions["dsh-sidebar-workbench-suite"].factory(id => id === "react" ? React : id === "@deepseek-ai/dsh-client-ui-primitives" ? {Button:({variant,size,children,...props})=>React.createElement('button',props,children)} : {});
plugin.apply(context);
assert.deepEqual(registrations.tabs.map(row => row.id), ["suite:jobs", "suite:controlled-browser"]);
assert.deepEqual(registrations.viewers.map(row => row.id), ["suite:markdown", "suite:structured", "suite:office", "suite:code"]);
assert.equal(registrations.viewers[0].priority > registrations.viewers[1].priority, true);
assert.equal(registrations.viewers[0].settings.pluginToggles.length, 2);
assert.equal(slotEntries[0].descriptor.id, "computer-use");
const root = ReactDOM.createRoot(document.getElementById("root"));
const h = React.createElement;
const render = async element => { await React.act(async () => { root.render(element); await new Promise(resolve => setTimeout(resolve, 20)); }); };
const click = async element => { assert.ok(element); await React.act(async () => { element.dispatchEvent(new dom.window.MouseEvent("click", { bubbles: true })); await new Promise(resolve => setTimeout(resolve, 20)); }); };
const input = async (element, value) => { assert.ok(element); await React.act(async () => { const propsKey = Object.keys(element).find(key => key.startsWith("__reactProps$")); if (propsKey && typeof element[propsKey].onChange === "function") element[propsKey].onChange({ target: { value } }); else { const prototype = element instanceof dom.window.HTMLTextAreaElement ? dom.window.HTMLTextAreaElement.prototype : dom.window.HTMLInputElement.prototype; Object.getOwnPropertyDescriptor(prototype, "value").set.call(element, value); element.dispatchEvent(new dom.window.Event("input", { bubbles: true })); } await new Promise(resolve => setTimeout(resolve, 20)); }); };
const sourceText = "# Overview\n\n- first\n\nText content.\n";
const fetchRecords = [];
const jobEntries = { parent: [{ id: "job-1", kind: "bash", label: "compile", status: "running", startedAt: 1 }], other: [{ id: "job-1", kind: "bash", label: "other compile", status: "running", startedAt: 2 }] };
const fileBodies = new Map([["README.md", sourceText], ["data.json", '[{"name":"alpha","count":2}]'], ["preview.html", "<button>Preview</button>"], ["main.rs", "fn main() {}"]]);
let fileRevision = 1;
global.fetch = async (url, options = {}) => {
  let requestBody = null;
  if (options.body) { try { requestBody = JSON.parse(options.body); } catch { requestBody = String(options.body); } }
  fetchRecords.push([String(url), options.method || "GET", requestBody]);
  if (String(url).includes("/__dsh-devices/")) return new Response(JSON.stringify({installed:false,devices:[],website:"https://uuyc.163.com/"}),{status:200,headers:{"Content-Type":"application/json"}});
  if (String(url).includes("__dsh-computer-use/meta")) return new Response(JSON.stringify({ enabled: true, available: true, adapter: "native", defaultBrowserSessionId: "default" }), { status: 200, headers: { "Content-Type": "application/json" } });
  if (String(url).includes("__dsh-computer-use/action")) { const owner = requestBody.ownerSessionId; return new Response(JSON.stringify({ state: { url: "https://" + owner + ".test/", title: owner + " browser", viewport: { width: 1280, height: 720 } }, screenshot: { base64: "iVBORw0KGgo=", mediaType: "image/png" } }), { status: 200, headers: { "Content-Type": "application/json" } }); }
  if (String(url).includes("job-list")) { const parsed = new URL(String(url), dom.window.location.href); const owner = parsed.searchParams.get("sessionId"); return new Response(JSON.stringify({ entries: jobEntries[owner] || [] }), { status: 200, headers: { "Content-Type": "application/json" } }); }
  if (String(url).includes("job-read")) { const parsed = new URL(String(url), dom.window.location.href); const owner = parsed.searchParams.get("sessionId"); return new Response(JSON.stringify({ text: owner + " job output", cursor: 10, truncated: false, snapshot: { status: "completed" } }), { status: 200, headers: { "Content-Type": "application/json" } }); }
  if (String(url).includes("job-action")) return new Response(JSON.stringify({ accepted: true }), { status: 200, headers: { "Content-Type": "application/json" } });
  const parsed = new URL(String(url), dom.window.location.href), filePath = parsed.searchParams.get("path") || "README.md";
  if (options.method === "POST") { fileBodies.set(filePath, String(options.body)); fileRevision += 1; return new Response(JSON.stringify({ etag: '"file-' + fileRevision + '"', size: options.body.length }), { status: 200, headers: { "Content-Type": "application/json" } }); }
  return new Response(fileBodies.get(filePath) || "", { status: 200, headers: { etag: '"file-' + fileRevision + '"' } });
};
dom.window.fetch = global.fetch;
const viewerProps = { ctx: context, scope: { sessionId: "parent" }, path: "README.md", title: "README.md", content: sourceText };
await render(h(slotEntries[0].component));
assert.equal(document.querySelectorAll(".dswSuiteSetting").length, 7);
await click(document.querySelector('.dswSuiteSetting [role="switch"]'));
assert.deepEqual(settingWrites[0], ["enabled", true]);
await render(h(registrations.viewers[0].component, viewerProps));
assert.ok(document.querySelector('[aria-label="Markdown 大纲"]'));
assert.equal(document.querySelectorAll(".dswSuiteEditor textarea").length, 1);
await click([...document.querySelectorAll("button")].find(button => button.textContent === "预览"));
assert.equal(document.querySelectorAll(".dswSuiteEditor textarea").length, 0);
await click([...document.querySelectorAll("button")].find(button => button.textContent === "源码"));
const unsavedMarkdown = sourceText + "\nUNSAVED MARKDOWN";
await input(document.querySelector('textarea[aria-label="编辑 README.md"]'), unsavedMarkdown);
await render(h(plugin.test.MermaidDiagram, { source: "flowchart LR\nA[Start] --> B[Done]" }));
await render(h(registrations.viewers[0].component, viewerProps));
assert.equal(document.querySelector('textarea[aria-label="编辑 README.md"]').value, unsavedMarkdown, "Markdown draft must survive viewer remount/move");
await click([...document.querySelectorAll("button")].find(button => button.textContent === "保存"));
fileBodies.set("README.md", "# External after save\n"); fileRevision += 1;
await render(h(plugin.test.MermaidDiagram, { source: "flowchart LR\nA[Start] --> B[Done]" }));
assert.ok(document.querySelector('svg[aria-label="Mermaid flowchart"]'));
await render(h(registrations.viewers[0].component, viewerProps));
assert.equal(document.querySelector('textarea[aria-label="编辑 README.md"]').value, "# External after save\n", "saved drafts must leave the in-memory cache");
await render(h(registrations.viewers[1].component, { ...viewerProps, path: "data.json", title: "data.json", content: '[{"name":"alpha","count":2}]' }));
assert.deepEqual([...document.querySelectorAll("th")].map(node => node.textContent), ["name", "count"]);
assert.match(document.body.textContent, /alpha/);
await render(h(registrations.viewers[3].component, { ...viewerProps, path: "preview.html", title: "preview.html", content: "<button>Preview</button>" }));
await click([...document.querySelectorAll("button")].find(button => button.textContent === "预览"));
assert.equal(document.querySelector("iframe").getAttribute("sandbox"), "allow-scripts");
await render(h(registrations.viewers[3].component, { ...viewerProps, path: "main.rs", title: "main.rs", content: "fn main() {}" }));
assert.ok(document.querySelector('.dswSuiteEditor textarea[aria-label="编辑 main.rs"]'), "switching from previewable HTML to code must restore the editor");
await input(document.querySelector('textarea[aria-label="编辑 main.rs"]'), "fn main() { /* unsaved */ }");
await render(h(registrations.viewers[1].component, { ...viewerProps, path: "data.json", title: "data.json", content: '[{"name":"alpha","count":2}]' }));
await render(h(registrations.viewers[3].component, { ...viewerProps, path: "main.rs", title: "main.rs", content: "fn main() {}" }));
assert.equal(document.querySelector('textarea[aria-label="编辑 main.rs"]').value, "fn main() { /* unsaved */ }", "code draft must survive viewer remount/move");
await render(h(registrations.tabs[0].component, { ctx: context, scope: { sessionId: "parent" }, tab: { id: "jobs" }, visible: true }));
assert.ok(fetchRecords.some(row => row[0].includes("job-list") && row[0].includes("sessionId=parent")), "jobs must come from the owner-scoped Host projection");
await click(document.querySelector(".dswSuiteRow"));
await new Promise(resolve => setTimeout(resolve, 30));
assert.match(document.body.textContent, /parent job output/);
await click([...document.querySelectorAll("button")].find(button => button.textContent === "终止"));
assert.ok(fetchRecords.some(row => row[0].includes("job-action") && row[1] === "POST"));
await render(h(registrations.tabs[0].component, { ctx: context, scope: { sessionId: "other" }, tab: { id: "jobs" }, visible: true }));
assert.doesNotMatch(document.body.textContent, /parent job output/);
await click(document.querySelector(".dswSuiteRow"));
await new Promise(resolve => setTimeout(resolve, 30));
assert.match(document.body.textContent, /other job output/);
await render(h(registrations.tabs[1].component, { ctx: context, scope: { sessionId: "parent" }, tab: { id: "browser", meta: {} }, visible: true, pluginSettings: { autoRefresh: false } }));
assert.ok(fetchRecords.some(row => row[0].includes("__dsh-computer-use/action") && row[1] === "POST"));
assert.equal(document.querySelector('[data-tab="controlled-browser"]').dataset.browserSession, "default");
assert.equal(fetchRecords.find(row => row[0].includes("__dsh-computer-use/action"))[2].browserSessionId, "default");
assert.equal(document.querySelector(".dswSuiteBrowser img").alt, "parent browser");
await input(document.querySelector('input[aria-label="发送到受控浏览器"]'), "private browser input");
await render(h(registrations.tabs[1].component, { ctx: context, scope: { sessionId: "other" }, tab: { id: "browser", meta: {} }, visible: true, pluginSettings: { autoRefresh: false } }));
assert.equal(document.querySelector('input[aria-label="发送到受控浏览器"]').value, "");
assert.equal(document.querySelector(".dswSuiteBrowser img").alt, "other browser");
assert.doesNotMatch(document.body.textContent, /parent browser|private browser input/);
registrations.tabs[1].onClose({ meta: { browserSessionId: "default" } }, { sessionId: "other" });
await new Promise(resolve => setTimeout(resolve, 20));
assert.ok(fetchRecords.filter(row => row[0].includes("__dsh-computer-use/action")).length >= 2);
plugin.test.clearFileDrafts();
const threeMiBInMemory = "x".repeat(1536 * 1024);
assert.equal(plugin.test.rememberFileDraft("oldest", threeMiBInMemory, "", "etag"), true);
assert.equal(plugin.test.rememberFileDraft("middle", threeMiBInMemory, "", "etag"), true);
assert.equal(plugin.test.rememberFileDraft("newest", threeMiBInMemory, "", "etag"), false);
assert.equal(JSON.stringify(plugin.test.fileDraftCacheSnapshot().keys), JSON.stringify(["oldest", "middle"]), "capacity rejection must preserve older unsaved drafts");
assert.equal(plugin.test.rememberFileDraft("oversized", "x".repeat(2 * 1024 * 1024 + 1), "", "etag"), false);
assert.equal(plugin.test.fileDraftCacheSnapshot().keys.includes("oversized"), false, "single drafts over 4 MiB are not cached");
plugin.test.clearFileDrafts();
let pageHidden=false;
Object.defineProperty(document,"hidden",{configurable:true,get:()=>pageHidden});
let polls=0,concurrent=0,peakConcurrent=0;
const stopPolling=plugin.test.visiblePoll(async signal=>{polls++;concurrent++;peakConcurrent=Math.max(peakConcurrent,concurrent);await new Promise(resolve=>setTimeout(resolve,20));concurrent--;return 20},20);
await new Promise(resolve=>setTimeout(resolve,25));
pageHidden=true;document.dispatchEvent(new dom.window.Event("visibilitychange"));
const hiddenPolls=polls;await new Promise(resolve=>setTimeout(resolve,75));assert.equal(polls,hiddenPolls,"hidden pages stop polling");
pageHidden=false;document.dispatchEvent(new dom.window.Event("visibilitychange"));
await new Promise(resolve=>setTimeout(resolve,25));assert.ok(polls>hiddenPolls,"visible pages resume promptly");
stopPolling();const stoppedPolls=polls;await new Promise(resolve=>setTimeout(resolve,75));assert.equal(polls,stoppedPolls);assert.equal(peakConcurrent,1,"poll requests never overlap");
cleanup();
assert.equal(disposed.length, 6);
await React.act(async () => root.unmount());
dom.window.close();
console.log("PASS companion suite: external registration, Markdown outline/Mermaid, structured data, devices, jobs and disposal");
})().catch(error => { console.error(error); process.exitCode = 1; dom.window.close(); });
