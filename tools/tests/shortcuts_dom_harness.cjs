"use strict";
const assert = require("node:assert/strict"), fs = require("node:fs"), path = require("node:path"), vm = require("node:vm");
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const directory = process.argv[3] || path.resolve(__dirname, "../../web/src/runtime-plugins");
const { JSDOM } = require(path.join(modules, "jsdom"));
const dom = new JSDOM('<main></main><input id="outside">', { url: "http://localhost/", pretendToBeVisual: true });
Object.assign(globalThis, { window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
const React = require(path.join(modules, "react")), jsx = require(path.join(modules, "react/jsx-runtime")), ReactDOM = require(path.join(modules, "react-dom"));
const root = require(path.join(modules, "react-dom/client")).createRoot(document.querySelector("main"));
const h = React.createElement, reports = [];
function load(name) {
  let result;
  const previous = window.__ModuleLoader__;
  window.__ModuleLoader__ = { load: spec => result = spec.factory(id => id === "react" ? React : id === "react-dom" ? ReactDOM : id === "react/jsx-runtime" ? jsx : (() => { throw Error(id); })()) };
  vm.runInNewContext(fs.readFileSync(path.join(directory, name), "utf8"), { window, document, console, setTimeout, clearTimeout });
  window.__ModuleLoader__ = previous;
  return result;
}
const core = load("shortcuts.js"), ui = load("ui-shortcuts.js");
const act = async fn => React.act(async () => { await fn(); await new Promise(resolve => setImmediate(resolve)); });
const key = (target, code, options = {}) => { const event = new window.KeyboardEvent("keydown", { code, key: code.startsWith("Key") ? code.slice(3).toLowerCase() : code, bubbles: true, cancelable: true, ...options }); target.dispatchEvent(event); return event; };
const button = text => [...document.querySelectorAll("button")].find(node => node.textContent === text);
const plain = value => JSON.parse(JSON.stringify(value));
function register(service, id, run, code = "KeyB", extra = {}) { return service.register({ id, label: id, defaults: { [service.profile]: { code, modifiers: ["primary", "alt"] } }, run, ...extra }); }
async function serviceTests() {
  window.localStorage.clear();
  for (const platform of ["windows", "macos", "linux"]) {
    const service = core.createShortcuts(window, platform); let calls = 0;
    const off = register(service, "test.toggle", () => calls++);
    const accelerator = platform === "macos" ? { metaKey: true } : { ctrlKey: true };
    const input = document.getElementById("outside");
    key(input, "KeyB", { ...accelerator, altKey: true });
    assert.equal(calls, platform === "linux" ? 0 : 1);
    if (platform !== "linux") {
      assert.equal(key(input, "KeyB", { ...accelerator, altKey: true, repeat: true }).defaultPrevented, true); assert.equal(calls, 1);
      key(input, "KeyB", { ...accelerator, altKey: true, isComposing: true }); assert.equal(calls, 1);
      const altgr = new window.KeyboardEvent("keydown", { code: "KeyB", key: "b", ...accelerator, altKey: true, bubbles: true, cancelable: true }); Object.defineProperty(altgr, "getModifierState", { value: name => name === "AltGraph" }); input.dispatchEvent(altgr); assert.equal(calls, 1);
      window.dispatchEvent(new window.CompositionEvent("compositionstart")); key(input, "KeyB", { ...accelerator, altKey: true }); window.dispatchEvent(new window.CompositionEvent("compositionend")); assert.equal(calls, 1);
      const resume = service.recording(); key(input, "KeyB", { ...accelerator, altKey: true }); resume(); resume(); assert.equal(calls, 1);
      const modal = document.createElement("section"); modal.setAttribute("role", "dialog"); modal.setAttribute("aria-modal", "true"); document.body.append(modal); key(input, "KeyB", { ...accelerator, altKey: true }); modal.remove(); assert.equal(calls, 1);
      input.dataset.shortcutRegion = "terminal"; key(input, "KeyB", { ...accelerator, altKey: true }); delete input.dataset.shortcutRegion; assert.equal(calls, 1);
      const consumed = event => event.preventDefault(); input.addEventListener("keydown", consumed); key(input, "KeyB", { ...accelerator, altKey: true }); input.removeEventListener("keydown", consumed); assert.equal(calls, 1);
      key(input, "KeyB", { ...accelerator, altKey: true }); assert.equal(calls, 2);
      assert.equal(service.edit({ type: "set", id: "test.toggle", binding: { code: "KeyC", modifiers: ["primary"] } }, service.getSnapshot().revision).status, "invalid");
      assert.equal(service.edit({ type: "set", id: "test.toggle", binding: { code: "Enter", modifiers: ["primary"] } }, service.getSnapshot().revision).status, "invalid");
    }
    off(); key(input, "KeyB", { ...accelerator, altKey: true }); assert.equal(calls, platform === "linux" ? 0 : 2); service.dispose();
  }
  reports.push("platform defaults, owner disposal, editor/terminal/modal priority, repeats, consumed events, IME and AltGraph");
  const service = core.createShortcuts(window, "windows");
  const off = register(service, "test.first", () => {}), second = register(service, "test.second", () => {}, "KeyK");
  const fixed = service.registerFixed({ id: "fixed.save", label: "固定保存", bindings: [{ code: "KeyS", modifiers: ["control", "alt"] }] });
  for (const code of ["KeyK", "KeyS"]) assert.equal(service.edit({ type: "set", id: "test.first", binding: { code, modifiers: ["control", "alt"] } }, service.getSnapshot().revision).status, "conflict");
  const before = service.getSnapshot(), originalSet = window.Storage.prototype.setItem;
  window.Storage.prototype.setItem = () => { throw Error("quota"); };
  assert.equal(service.edit({ type: "set", id: "test.first", binding: null }, before.revision).status, "write-failed");
  assert.equal(service.getSnapshot(), before); window.Storage.prototype.setItem = originalSet;
  assert.equal(service.edit({ type: "set", id: "test.first", binding: null }, before.revision).status, "saved");
  assert.equal(service.getSnapshot().rows.find(row => row.id === "test.first").binding, null);
  const revision = service.getSnapshot().revision;
  const external = { schemaVersion: 1, profiles: { "web:windows": { "test.second": null, "dormant.plugin": null }, "web:macos": { "test.first": null } } };
  window.localStorage.setItem(core.STORAGE_KEY, JSON.stringify(external));
  window.dispatchEvent(new window.StorageEvent("storage", { key: core.STORAGE_KEY, storageArea: window.localStorage }));
  assert.equal(service.edit({ type: "reset-all" }, revision).status, "stale");
  assert.equal(service.edit({ type: "reset-all" }, service.getSnapshot().revision).status, "saved");
  assert.deepEqual(JSON.parse(window.localStorage.getItem(core.STORAGE_KEY)).profiles["web:macos"], { "test.first": null });
  assert.equal(service.getSnapshot().customCount, 0);
  const accepted = service.getSnapshot().rows;
  window.localStorage.setItem(core.STORAGE_KEY, '{"schemaVersion":99,"profiles":{}}');
  service.reload(); assert.equal(service.getSnapshot().writable, false); assert.deepEqual(plain(service.getSnapshot().rows), plain(accepted));
  assert.equal(service.edit({ type: "reset-all" }, service.getSnapshot().revision).status, "unreadable");
  assert.match(window.localStorage.getItem(core.STORAGE_KEY), /99/);
  off(); second(); fixed(); service.dispose(); window.localStorage.clear();
  reports.push("fixed and editable conflicts, write failure rollback, cross-tab CAS, profile-only reset, unreadable preservation");
}
async function uiTests() {
  const service = core.createShortcuts(window, "windows"), visibility = ui.createVisibility(); let calls = 0;
  const owner = register(service, "session.new", () => calls++, "KeyN", { label: "新建会话", aliases: ["new session"] });
  const fixed = service.registerFixed({ id: "fixed.send", label: "发送消息", keys: "Enter", bindings: [{ code: "Enter", modifiers: [] }] });
  document.getElementById("outside").focus();
  await act(() => root.render(h(React.Fragment, null, h(ui.ShortcutEntry, { shortcuts: service, visibility }), h(ui.ShortcutReference, { shortcuts: service, visibility }))));
  await act(() => key(document.getElementById("outside"), "Slash", { ctrlKey: true, key: "/" }));
  assert.ok(document.querySelector('[aria-label="快捷键"]')); assert.equal(document.activeElement.getAttribute("aria-label"), "搜索快捷键");
  assert.equal(document.querySelector('[aria-label="修改发送消息"]'), null);
  await act(() => document.querySelector('[aria-label="修改新建会话"]').click());
  await act(() => key(document.activeElement, "KeyP", { ctrlKey: true, altKey: true })); assert.match(document.querySelector(".dshShortcutEditor").textContent, /Ctrl\+Alt\+P/); assert.equal(calls, 0);
  const originalSet = window.Storage.prototype.setItem; window.Storage.prototype.setItem = () => { throw Error("disk"); };
  await act(() => button("保存").click()); assert.ok(document.querySelector(".dshShortcutEditor")); assert.match(document.querySelector('[role="status"]').textContent, /保存失败/); assert.equal(service.getSnapshot().rows.find(row => row.id === "session.new").binding.code, "KeyN");
  window.Storage.prototype.setItem = originalSet;
  await act(() => button("保存").click()); assert.equal(document.querySelector(".dshShortcutEditor"), null);
  await act(() => document.querySelector('[aria-label="修改新建会话"]').click());
  await act(() => { const value = JSON.parse(window.localStorage.getItem(core.STORAGE_KEY)); value.profiles["web:windows"]["dormant.other"] = null; window.localStorage.setItem(core.STORAGE_KEY, JSON.stringify(value)); window.dispatchEvent(new window.StorageEvent("storage", { key: core.STORAGE_KEY })); });
  assert.equal(button("保存").disabled, true); assert.match(document.querySelector(".dshShortcutEditor").textContent, /已变化/);
  await act(() => button("核对后重试").click()); await act(() => button("清除绑定").click()); assert.equal(service.getSnapshot().rows.find(row => row.id === "session.new").binding, null);
  await act(() => button("恢复全部默认").click()); assert.ok(button("取消恢复")); await act(() => button("确认恢复").click()); assert.equal(service.getSnapshot().customCount, 0);
  await act(() => key(document.activeElement, "Slash", { ctrlKey: true, key: "/" })); assert.equal(document.querySelector('[aria-label="快捷键"]'), null); assert.equal(document.activeElement.id, "outside");
  await act(() => key(document.getElementById("outside"), "KeyN", { ctrlKey: true, altKey: true })); assert.equal(calls, 1);
  await act(() => root.render(null)); assert.equal(service.getSnapshot().rows.some(row => row.id === "shortcuts.open"), false);
  owner(); fixed(); service.dispose(); reports.push("actual React settings entry/dialog, recording, fixed read-only rows, failed-save draft, stale rebase, clear/reset and focus return");
}
async function ownerTests() {
  window.localStorage.clear();
  const service = core.createShortcuts(window, "windows"), calls = [];
  const source = fs.readFileSync(path.join(directory, "ui-workspace.js"), "utf8");
  const primitives = new Proxy({ Tooltip: ({ children }) => children, Button: ({ children, variant, ...props }) => h("button", props, children), Modal: ({ open, title, children, footer }) => open ? h("section", { role: "dialog", "aria-modal": true }, h("h2", null, title), children, footer) : null }, { get: (target, name) => target[name] || (() => null) });
  const context = { react: React, react_jsx_runtime: jsx, window, document, console, Node: window.Node, AbortController, setTimeout, clearTimeout, normalizedArchiveFilter: value => value || "active", sanitizeSearchQuery: text => text, FLAT_SESSION_ORDER_KEY: "flat", EXPAND_SLIDE_MS: 1, SEARCH_DEBOUNCE_MS: 1, clsx: (...values) => values.filter(Boolean).join(" "), WorkspaceBrowser_module_css_default: {}, _deepseek_ai_dsh_client_ui_primitives: primitives, ViewOptionsMenu: () => null, SessionTree: () => null, FlatList: () => null, SearchResults: () => null, ArchiveReminderDialog: () => null, WorkspacePickFlow: ({ open }) => open ? h("section", { "data-picker": true }, "picker") : null, createArchiveController: options => ({ request(id, archived) { calls.push(["archive", id, archived]); }, getSnapshot: () => ({ target: null }), dispose() {} }) };
  context.SEARCH_QUERY_MAX_CODE_UNITS = 256;
  vm.runInNewContext(source.slice(source.indexOf("function WorkspaceBrowser("), source.indexOf("//#endregion", source.indexOf("function WorkspaceBrowser("))), context);
  const list = { current: "a", byId: { a: { id: "a", displayTitle: "Existing", blank: false } } }, workspace = { items: [], phase: "ready", archivedSessionIds: [] }, state = { groupBy: "flat", groupExpansion: {}, sessionOrderByAccount: {}, sessionUpdatedAtByAccount: {} };
  const props = { shortcuts: service, wide: true, expandSidebar: () => calls.push(["expand"]), useSessions: select => select(list), useWorkspaces: select => select(workspace), useStore: select => select(state), actions: new Proxy({}, { get: () => () => {} }), startSession: () => calls.push(["new"]), open() {}, readSessionTitle: id => ({ value: `base-${id}`, revision: 2 }), renameSession: async (...args) => calls.push(["rename", ...args]), forkSession: id => calls.push(["fork", id]), archiveSession: async () => {}, unarchiveSession: async () => {}, createWorkspace: async () => {}, searchSessions: async () => ({ items: [], hasMore: false }), searchResultLimit: 20, useDirectoryFlow: select => select(true), renderSlot: () => null, t: key => key };
  await act(() => root.render(h(context.WorkspaceBrowser, props)));
  await act(() => key(document.getElementById("outside"), "KeyN", { ctrlKey: true, altKey: true })); assert.deepEqual(calls.at(-1), ["new"]);
  await act(() => key(document.getElementById("outside"), "KeyK", { ctrlKey: true, altKey: true })); assert.equal(document.activeElement.getAttribute("placeholder"), "search.placeholder");
  await act(() => key(document.activeElement, "KeyR", { ctrlKey: true, shiftKey: true })); assert.ok(document.querySelector('[role="dialog"]')); assert.match(document.querySelector('[role="dialog"]').textContent, /rename.session.title/);
  await act(() => button("cancel").click());
  await act(() => key(document.getElementById("outside"), "KeyF", { ctrlKey: true, shiftKey: true })); assert.deepEqual(calls.at(-1), ["fork", "a"]);
  list.current = "b"; list.byId.b = { id: "b", displayTitle: "Changed", blank: false }; await act(() => root.render(h(context.WorkspaceBrowser, props)));
  await act(() => key(document.getElementById("outside"), "KeyA", { ctrlKey: true, altKey: true })); assert.deepEqual(calls.at(-1), ["archive", "b", false]);
  await act(() => key(document.getElementById("outside"), "KeyO", { ctrlKey: true, altKey: true })); assert.ok(document.querySelector("[data-picker]"));
  await act(() => root.render(null)); assert.equal(service.getSnapshot().rows.length, 0);
  const settings = fs.readFileSync(path.join(directory, "ui-settings-general.js"), "utf8"), settingsContext = { react: React, react_jsx_runtime: jsx, react_dom: ReactDOM, window, document, clsx: (...values) => values.filter(Boolean).join(" "), SettingsRoot_module_css_default: {}, navIcon: () => null, _deepseek_ai_dsh_client_ui_primitives: primitives };
  vm.runInNewContext(settings.slice(settings.indexOf("function SettingsPanel("), settings.indexOf("//#endregion", settings.indexOf("function SettingsRoot("))), settingsContext);
  const settingsProps = { shortcuts: service, wide: true, reconnect() {}, useConnectionState: select => select("connected"), useSections: select => select([{ id: "general", label: "General" }]), useOnboardingSteps: select => select([]), useSessions: select => select({ phase: "ready", current: "a", byId: { a: { blank: false } } }), renderSlot: name => name, t: key => key };
  const visibility = ui.createVisibility();
  await act(() => root.render(h(React.Fragment, null, h(settingsContext.SettingsRoot, settingsProps), h(ui.ShortcutReference, { shortcuts: service, visibility }))));
  await act(() => key(document.getElementById("outside"), "Comma", { ctrlKey: true, key: "," })); assert.ok(document.querySelector('[data-shortcut-modal="settings"]'));
  await act(() => key(document.activeElement, "Slash", { ctrlKey: true, key: "/" })); assert.ok(document.querySelector('[data-shortcut-modal="shortcuts"]'));
  await act(() => key(document.activeElement, "Comma", { ctrlKey: true, key: "," })); assert.ok(document.querySelector('[data-shortcut-modal="settings"]')); assert.ok(document.querySelector('[data-shortcut-modal="shortcuts"]'));
  await act(() => key(document.activeElement, "Escape", { key: "Escape" })); assert.equal(document.querySelector('[data-shortcut-modal="shortcuts"]'), null); assert.ok(document.querySelector('[data-shortcut-modal="settings"]'));
  window.__DSH_SETTINGS_DIRTY__ = true;
  await act(() => key(document.activeElement, "Comma", { ctrlKey: true, key: "," })); assert.ok(document.querySelector('[data-shortcut-modal="settings"]')); assert.match(document.body.textContent, /unsaved.discard/);
  window.__DSH_SETTINGS_DIRTY__ = false;
  await act(() => root.render(null)); assert.equal(service.getSnapshot().rows.length, 0); service.dispose();
  reports.push("actual workspace and settings owners: current target, new/search/rename/fork/archive/picker, dirty-settings confirmation, owner unmount");
}
(async () => { await serviceTests(); await uiTests(); await ownerTests(); console.log(JSON.stringify({ status: "passed", groups: reports }, null, 2)); })().catch(error => { console.error(error); process.exitCode = 1; }).finally(async () => { await act(() => root.unmount()); dom.window.close(); });
