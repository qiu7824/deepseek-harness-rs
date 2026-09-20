"use strict";
const assert = require("node:assert/strict"), fs = require("node:fs"), path = require("node:path"), vm = require("node:vm");
const modules = path.resolve(process.argv[2]);
const sourceRoot = process.argv[3] ? path.resolve(process.argv[3]) : path.resolve(__dirname, "../../web/dist/plugins");
const { JSDOM } = require(path.join(modules, "jsdom"));
const dom = new JSDOM("<!doctype html><html><head></head><body><main></main></body></html>", { url: "http://localhost/" });
Object.assign(global, { window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
const React = require(path.join(modules, "react")), jsx = require(path.join(modules, "react/jsx-runtime"));
const root = require(path.join(modules, "react-dom/client")).createRoot(document.querySelector("main"));
const h = React.createElement;
const flush = async () => React.act(async () => { await new Promise(resolve => setImmediate(resolve)); });
const render = async element => { await React.act(async () => root.render(element)); await flush(); };
const button = text => [...document.querySelectorAll("button")].find(element => element.textContent === text);
const click = async element => { assert.ok(element); await React.act(async () => element.click()); await flush(); };
function load(name, fetch, exports = []) {
    let result;
    const primitives = new Proxy({ Button: ({ children, ...props }) => h("button", props, children) }, { get: (object, key) => object[key] || (() => h("svg")) });
    const runtime = { defineStore: spec => spec, createSnapshotStore: initial => ({ getSnapshot: () => initial, subscribe: () => () => {} }) };
    const source = fs.readFileSync(path.join(sourceRoot, name), "utf8").replace("return module.exports;", exports.map(name => `exports.${name}=${name};`).join("") + "return module.exports;");
    vm.runInNewContext(source, { document, console, fetch, AbortController, setTimeout, clearTimeout, setInterval, clearInterval,
        window: { __ModuleLoader__: { load: definition => result = definition.factory(id => id === "react" ? React : id === "react/jsx-runtime" ? jsx : id.endsWith("/client") ? runtime : id.endsWith("ui-primitives") ? primitives : { Service: class {} }) } }
    });
    return result;
}
const response = value => ({ ok: true, json: async () => value });
(async () => {
    const generalSource = fs.readFileSync(path.join(sourceRoot, "ui-settings-general.js"), "utf8");
    for (const retired of ["function EnvironmentSection", "function RemoteExecutionSection"])
        assert.ok(!generalSource.includes(retired), "retired settings sections must not return");
    const general = load("ui-settings-general.js", async () => response({}));
    assert.equal(typeof general.apply, "function", "remaining settings plugin still loads");
    const discovery = load("ui-settings-tool-discovery.js", async () => response({ revision: 1, configuration: { enabled: true, eagerLimit: 12, listingChars: 2048, maxLoaded: 20, maxSchemaBytes: 65536 }, runtime: { enabled: true } }));
    await render(h(discovery.ToolDiscoverySection));
    assert.equal(window.getComputedStyle(button("保存")).minHeight, "38px", "discovery styles do not require workbench previews");
    assert.ok(document.querySelector(".dshDiscoveryBudgets"));
    await render(null);
    assert.equal(document.querySelector("style[data-tool-discovery-controls]"), null, "discovery releases its owned stylesheet");
    const skills = load("ui-settings-skill-revisions.js", async () => response({}));
    let registered = false;
    skills.apply({ slots: { inject: () => { registered = true; }, register: () => { registered = true; } } });
    assert.equal(registered, false, "retired skill revisions page stays absent");
    const plugins = load("ui-settings-plugins.js", async () => response({}), ["PluginInstallControls"]);
    await render(h(plugins.PluginInstallControls));
    assert.equal(window.getComputedStyle(button("检查操作")).minHeight, "36px", "plugin controls do not require workbench previews");
    await render(null);
    assert.equal(document.querySelector("style[data-plugin-center-controls]"), null);
    console.log("PASS settings controls: retired sections absent, settings plugin loads, independent styles and switch alignment");
})().catch(error => { console.error(error); process.exitCode = 1; }).finally(async () => { await React.act(async () => root.unmount()); dom.window.close(); });
