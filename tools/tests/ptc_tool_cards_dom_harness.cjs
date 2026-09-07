const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const { JSDOM } = require(path.join(modules, 'jsdom'));
const React = require(path.join(modules, 'react')), jsx = require(path.join(modules, 'react/jsx-runtime'));
const dom = new JSDOM('<main id="root"></main>', { pretendToBeVisual: true });
Object.assign(global, { window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
const Client = require(path.join(modules, 'react-dom/client'));
const source = fs.readFileSync(path.join(__dirname, '../../web/dist/plugins/ui-tool.js'), 'utf8');
const assets = path.join(__dirname, '../../web/dist/assets');
const shell = fs.readFileSync(path.join(assets, fs.readdirSync(assets).find(name => /^index-.*\.js$/.test(name))), 'utf8');
const name = /DisclosureRow:([\w$]+)/.exec(shell)[1], start = shell.indexOf(`function ${name}(`), end = shell.indexOf('}const ', start) + 1;
const disclosure = { f: jsx, R: React, Fn: {}, ye: (...args) => args.filter(Boolean).join(' '), Bl: () => null };
vm.runInNewContext(shell.slice(start, end) + `;this.Component=${name};`, disclosure);
const primitives = new Proxy({ DisclosureRow: disclosure.Component,
  TerminalBlock: props => React.createElement('pre', { 'data-terminal': '', 'data-max-lines': props.maxLines }, props.command + '\n' + props.output + '\n' + (props.exitCode === undefined ? props.labels.done : props.labels.exitCode(props.exitCode))),
  CodeBlock: ({ code }) => React.createElement('pre', { 'data-code': '' }, code) }, { get: (target, key) => target[key] ?? (() => null) });
const context = { react: React, react_jsx_runtime: jsx, _deepseek_ai_dsh_client_ui_primitives: primitives, clsx: (...args) => args.filter(Boolean).join(' '),
  _deepseek_ai_dsh_client_runtime_client: { resolveWorkspacePath: (cwd, value) => value.startsWith('/') ? value : cwd + '/' + value },
  ToolRow_module_css_default: {}, ToolCallTree_module_css_default: {} };
for (const [from, to] of [['const VARIANT_TITLES =', '//#region lib/types/client/tool/models/read-card-model.js'], ['function terminalBlockLabels(', '//#region lib/types/client/tool/models/web-card-model.js'], ['function leadingFor$1(', '//#region lib/types/client/tool/toolviews/GenericToolCard.js'], ['function callName(', '//#region \0dsh-css:']]) {
  const a = source.indexOf(from); let b = source.indexOf(to, a);
  if (from === 'function callName(') b = source.indexOf('\n\t\t//#endregion', source.indexOf('function ToolCallTree(', a));
  assert.ok(a >= 0 && b > a); vm.runInNewContext(source.slice(a, b), context);
}
const t = (key, args) => key + (args?.code !== undefined ? ':' + args.code : '');
const child = { kind: 'tool-result', callId: 'shell-child', call: { name: 'pwsh', argsRaw: '{"command":"Write-Output fixture"}' }, callView: { card: 'terminal', title: 'Write-Output fixture', cwd: '/workspace' }, resultView: { card: 'terminal', output: 'fixture output\nlast line', exitCode: 7 }, content: [{ type: 'text', text: 'fixture output' }], subCalls: [] };
const parent = { kind: 'tool-result', callId: 'ptc-root', call: { name: 'run_code', argsRaw: JSON.stringify({ code: 'await tools.pwsh({command: "Write-Output fixture"})' }) }, content: [{ type: 'text', text: 'parent result' }], subCalls: [child] };
context.GenericToolCard = ({ toolName, block }) => React.createElement(context.ToolRow, { t, toolName, ...context.toolRowModel(toolName, block, '/workspace'), terminal: context.terminalCardModel(block, '/workspace') });
const root = Client.createRoot(document.getElementById('root'));
(async () => {
  await React.act(async () => root.render(React.createElement(context.ToolCallTree, { renderSlot: (_name, _owner, options) => options.fallback, node: { data: { root: parent } }, t, inspectCall() {} })));
  assert.equal(document.querySelectorAll('[data-chat-call-id]').length, 2, 'nested PTC call remains linked under its parent');
  assert.equal(document.querySelector('[data-terminal]'), null, 'collapsed output does not render a hidden terminal subtree');
  await React.act(async () => document.querySelector('[data-chat-call-id="ptc-root"] [data-disclosure-row]').click());
  assert.ok(document.querySelector('[data-code]').textContent.includes('tools.pwsh'), 'PTC source expands');
  assert.ok(document.body.textContent.includes('parent result'), 'PTC result expands separately from child output');
  await React.act(async () => document.querySelector('[data-chat-call-id="shell-child"] [data-disclosure-row]').click());
  const terminal = document.querySelector('[data-terminal]');
  assert.ok(terminal.textContent.includes('Write-Output fixture'));
  assert.ok(terminal.textContent.includes('last line'));
  assert.ok(terminal.textContent.includes('terminal.exitCode:7'));
  assert.equal(terminal.dataset.maxLines, '80', 'large output uses the native remainder expander');
  const unknown = context.terminalCardModel({ ...child, resultView: { card: 'terminal', output: 'no exit status reported' } }, '/workspace');
  assert.equal(context.terminalBlockLabels(t, unknown.card).done, 'terminal.unknown');
  assert.equal(context.terminalCardModel({ ...child, resultView: { card: 'generic', text: 'background job started' } }), null, 'background start confirmation is never rendered as a completed terminal');
  await React.act(async () => root.unmount()); dom.window.close();
  console.log('PASS PTC tool cards: nested structure, native disclosure, command/output expansion and truthful exit state');
})().catch(error => { console.error(error); process.exitCode = 1; dom.window.close(); });
