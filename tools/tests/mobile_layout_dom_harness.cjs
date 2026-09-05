"use strict";
const assert = require("node:assert/strict"), fs = require("node:fs"), path = require("node:path"), vm = require("node:vm");

async function run(modules, output) {
  const { JSDOM } = require(path.join(modules, "jsdom"));
  const dom = new JSDOM('<!doctype html><html><head></head><body><div id="root"></div></body></html>', { url: "http://fixture.invalid" });
  const React = require(path.join(modules, "react")), jsx = require(path.join(modules, "react/jsx-runtime")), ReactDOM = require(path.join(modules, "react-dom/client"));
  Object.assign(global, { window: dom.window, document: dom.window.document, IS_REACT_ACT_ENVIRONMENT: true });
  const rootDirectory = path.resolve(__dirname, "../.."), h = React.createElement;
  const primitives = new Proxy({ Button: ({ variant, children, ...props }) => h("button", props, children), Modal: ({ open, children }) => open ? h("div", { role: "dialog" }, children) : null }, { get: (object, key) => object[key] || (() => h("svg", { width: 16, height: 16 })) });
  function load(file, names) {
    let result;
    const source = fs.readFileSync(path.join(rootDirectory, "web/dist/plugins", file), "utf8");
    const runtime = { defineStore: spec => spec, createSnapshotStore: initial => ({ getSnapshot: () => initial, subscribe: () => () => {} }) };
    const browser = { innerWidth: 390, __ModuleLoader__: { load: definition => result = definition.factory(id => id === "react" ? React : id === "react/jsx-runtime" ? jsx : id === "react-dom" ? require(path.join(modules, "react-dom")) : id.endsWith("/client") ? runtime : id.endsWith("ui-primitives") ? primitives : { Service: class {} }) } };
    const context = { window: browser, document, console, URL, setTimeout, clearTimeout, setInterval, clearInterval, requestAnimationFrame: callback => setTimeout(callback, 0), cancelAnimationFrame: clearTimeout, ResizeObserver: class { observe() {} disconnect() {} }, ...names.context };
    vm.runInNewContext(source.replace("return module.exports;", names.values.map(name => `exports.${name} = ${name};`).join("\n") + "\nreturn module.exports;"), context);
    return { ...result, browser };
  }
  const general = load("ui-settings-general.js", { values: ["SettingsPanel", "MemorySection"] });
  const theme = load("ui-theme.js", { values: ["AppearanceRow", "ThemeRuntime"] });
  const layout = load("ui-layout.js", { values: ["AppFrame", "computeColumns", "createLayoutStore"] });
  const model = load("ui-settings-models.js", { values: ["ModelListEditor", "ModelsSection_module_css_default", "zh"] });
  load("ui-settings-capabilities.js", { values: [] });
  const conversation = fs.readFileSync(path.join(rootDirectory, "web/dist/plugins/ui-conversation.js"), "utf8");
  for (const match of conversation.matchAll(/const css(?:\$\d+)? = ("(?:\\.|[^"\\])*");/g)) { const style = document.createElement("style"); style.textContent = JSON.parse(match[1]); document.head.append(style); }
  const ok = value => Promise.resolve({ result: { ok: true, value } });
  const api = { settings: { describe: () => ok({ namespaces: [{ ns: "memory", revision: 1, value: { enabled: true, maxEntries: 30, maxTokens: 2000 } }] }) }, memory: { categories: () => ok({ categories: [{ id: "custom", label: "项目知识" }] }), list: () => ok({ entries: [] }), learningList: () => ok({ items: [], enabled: true, effectiveEnabled: true }) }, agentPresets: { list: () => ok({ presets: [] }) } };
  let root = ReactDOM.createRoot(document.getElementById("root"));
  const flush = async () => React.act(async () => { await new Promise(resolve => setImmediate(resolve)); });
  const render = async element => { await React.act(async () => root.render(element)); await flush(); };
  const click = async button => React.act(async () => button.click());
  const rows = ["通用", "模型", "免费模型", "技能与 MCP", "记忆与上下文", "子智能体", "插件", "安全盾", "目录与运行环境"].map((label, index) => ({ id: "section-" + index, label }));
  let selected = rows[0].id, closed = 0;
  const panel = children => h(general.SettingsPanel, { rows, activeId: selected, onSelect: id => { selected = id; }, onClose: () => closed++, renderSlot: name => name === "settings.header" ? "设置" : name === "settings.close" ? "关闭设置" : name === "settings.section" ? children : null });
  const snapshots = [];
  const capture = (name, width) => snapshots.push({ name, width, body: document.getElementById("root").innerHTML + [...document.body.children].filter(node => node.id !== "root").map(node => node.outerHTML).join("") });
  await render(panel(h(theme.AppearanceRow, { t: key => ({ "appearance.title": "外观", "appearance.light": "浅色", "appearance.dark": "深色" })[key], setTheme: () => {}, useStore: select => select({ preference: "light" }) })));
  assert.equal(document.body.textContent.includes("Bing"), false); assert.equal(document.body.textContent.includes("内容字号"), false);
  assert.equal(document.querySelector("._7h7_Oq_overlay").parentElement, document.body, "Settings must be outside the sidebar animation's stacking context");
  assert.equal(document.getElementById("root").contains(document.querySelector("._7h7_Oq_panel")), false);
  await click([...document.querySelectorAll("button")].find(button => button.textContent === "技能与 MCP")); assert.equal(selected, rows[3].id);
  await click(document.querySelector("._7h7_Oq_close")); assert.equal(closed, 1);
  capture("appearance", 390);
  await render(panel(h(general.MemorySection, { api }))); await flush();
  await click([...document.querySelectorAll("button")].find(button => button.textContent === "新增记忆"));
  assert.ok(document.querySelector(".dshMemoryItem textarea")); capture("memory-editor", 390);
  const t = key => model.zh[key] || key;
  await render(panel(h(model.ModelListEditor, { models: [{ id: "provider-model-with-a-long-identifier-".repeat(3), name: "可用模型" }], probe: { provider: "fixture" }, api, onChange: () => {}, t, disabled: false })));
  capture("model-editor", 390);
  await render(panel(h("section", { className: "dshCaps" }, h("h2", null, "技能与 MCP"), h("div", { className: "dshCapsEditor" }, h("label", null, "名称", h("input", { defaultValue: "项目工具" })), h("label", null, "说明", h("textarea", { defaultValue: "支持项目任务的工具配置" })), h("div", { className: "dshCapsBar" }, h("button", null, "保存"), h("button", null, "取消"))))));
  capture("skill-editor", 390);
  const shellSpec = layout.createLayoutStore(); let state = shellSpec.init();
  const actions = Object.fromEntries(Object.entries(shellSpec.actions).map(([name, action]) => [name, (...args) => action(state, ...args)]));
  const sessions = { current: "test-session", byId: { "test-session": { blank: false } } };
  const composer = h("div", { className: "c9AePG_root", "data-phase": "active" }, h("div", { className: "c9AePG_header" }, h("div", { className: "c9AePG_titleRow" }, h("div", { className: "c9AePG_titleCluster" }, "项目任务")), h("div", { className: "c9AePG_tabs" }, h("button", { className: "c9AePG_tab" }, "对话"), h("button", { className: "c9AePG_tab" }, "产物"))), h("div", { className: "c9AePG_viewArea" }, h("p", { style: { padding: "12px", overflowWrap: "anywhere" } }, "查看任务执行结果与相关说明。")), h("div", { className: "_l4_0G_root" }, h("div", { className: "_l4_0G_card" }, h("div", { className: "_l4_0G_strip" }, "等待审批"), h("div", { className: "_l4_0G_body" }, h("div", { className: "_l4_0G_headline" }, "写入文件需要审批"), h("div", { className: "_l4_0G_command" }, "D:\\工作目录\\" + "project-file".repeat(20) + ".ts")), h("div", { className: "_l4_0G_actionRow" }, ...["拒绝", "允许一次", "始终允许"].map(label => h("button", { key: label }, label))))), h("div", { className: "Uzx--a_root" }, h("div", { className: "Uzx--a_card" }, h("div", { style: { padding: "12px" } }, "输入消息"), h("div", { className: "Uzx--a_row" }, h("div", { className: "Uzx--a_tools" }, h("button", { className: "Uzx--a_add" }, "+"), h("select", { className: "Uzx--a_select" }, h("option", null, "GPT-6 Astra")), h("select", { className: "Uzx--a_select" }, h("option", null, "最高思考能力"))), h("div", { className: "Uzx--a_trailing" }, h("button", { className: "Uzx--a_primary" }, "↑"))))));
  const shell = () => h(layout.AppFrame, { useStore: selector => selector(state), useSessions: selector => selector(sessions), actions, renderSlot: (name, props) => name === "conversation" ? composer : name === "sidebar" ? h("nav", { "data-test-sidebar-width": props.width }, h("button", { onClick: () => actions.toggleSidebar() }, "菜单")) : name === "details" ? h("section", null, h("h2", null, "上下文"), h("button", { onClick: () => actions.closeDetails() }, "关闭")) : null });
  for (const width of [320, 375, 390, 430, 1280]) {
    await React.act(async () => root.unmount()); root = ReactDOM.createRoot(document.getElementById("root")); layout.browser.innerWidth = width; state = shellSpec.init();
    await render(shell()); capture("conversation", width);
    actions.toggleSidebar(); await render(shell());
    if (width <= 768) { assert.ok(document.querySelector(".dshMobileSidebarBackdrop")); assert.ok(Number(document.querySelector("[data-test-sidebar-width]").dataset.testSidebarWidth) <= width - 44); capture("sidebar-open", width); await click(document.querySelector(".dshMobileSidebarBackdrop")); await render(shell()); assert.equal(document.querySelector(".dshMobileSidebarBackdrop"), null); }
    actions.openDetails(); await render(shell());
    if (width <= 768) { assert.ok(document.querySelector("[data-mobile-details-open]")); capture("details-open", width); }
  }
  assert.deepEqual(JSON.parse(JSON.stringify(layout.computeColumns(1440, 280, 360))), { sidebar: 280, center: 800, details: 360 });
  if (output) {
    fs.mkdirSync(output, { recursive: true });
    const baseCss = fs.readFileSync(path.join(rootDirectory, "web/dist/assets/index-CSGf6Qzd.css"), "utf8"), styles = [...document.head.querySelectorAll("style")].map(style => style.textContent).join("\n");
    for (const fixture of snapshots) fs.writeFileSync(path.join(output, fixture.name + "-" + fixture.width + ".html"), '<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>' + baseCss + styles + '</style></head><body><div id="root">' + fixture.body + '</div></body></html>');
    fs.writeFileSync(path.join(output, "fixtures.json"), JSON.stringify(snapshots.map(({ name, width }) => ({ name, width })), null, 2));
  }
  await React.act(async () => root.unmount());
  console.log("PASS appearance restoration, mobile settings navigation, memory editor, model editor, sidebar overlay dismissal, mobile details availability and desktop column geometry");
}
if (require.main === module) run(process.env.DSH_REACT_TEST_MODULES || process.argv[2], process.argv[3]).catch(error => { console.error(error); process.exitCode = 1; });
module.exports = { run };
