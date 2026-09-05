"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
let plugin;
vm.runInNewContext(fs.readFileSync(path.join(__dirname, "../../web/dist/plugins/ui-workbench.js"), "utf8"), {
  window: { __ModuleLoader__: { load(definition) { plugin = definition.factory(() => ({})); } } }
});
assert.equal(plugin.nodeStatusText(undefined), "尚未检测");
assert.equal(plugin.nodeStatusText({ status: "missing" }), "未安装");
assert.equal(plugin.nodeStatusText({ status: "timeout" }), "检测超时");
assert.equal(plugin.nodeStatusText({ status: "error" }), "检测失败");
assert.equal(plugin.nodeStatusText({ status: "ready", path: "node.exe" }), "可用");
assert.equal(plugin.nodeStatusText({ status: "incompatible" }), "版本或能力不兼容");
process.stdout.write("runtime environment presentation checks passed\n");
