const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const { JSDOM } = require(path.join(modules, 'jsdom'));
const React = require(path.join(modules, 'react')), jsx = require(path.join(modules, 'react/jsx-runtime'));
const dom = new JSDOM('<main id="root"></main>', { pretendToBeVisual: true, url: 'http://localhost/' });
Object.assign(global, { window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
const Client = require(path.join(modules, 'react-dom/client'));
const source = fs.readFileSync(path.join(__dirname, '../../web/dist/plugins/ui-tool.js'), 'utf8');
const assets = path.join(__dirname, '../../web/dist/assets');
const shell = fs.readFileSync(path.join(assets, fs.readdirSync(assets).find(name => /^index-.*\.js$/.test(name))), 'utf8');
const name = /DisclosureRow:([\w$]+)/.exec(shell)[1], start = shell.indexOf(`function ${name}(`), end = shell.indexOf('}const ', start) + 1;
const disclosure = { f: jsx, R: React, Fn: {}, ye: (...args) => args.filter(Boolean).join(' '), Bl: () => null };
vm.runInNewContext(shell.slice(start, end) + `;this.Component=${name};`, disclosure);
const primitives = new Proxy({ DisclosureRow: disclosure.Component,
  WebBlock: props => React.createElement('div', { 'data-web': '' }, props.url ?? '') }, { get: (target, key) => target[key] ?? (() => null) });
const context = { react: React, react_jsx_runtime: jsx, _deepseek_ai_dsh_client_ui_primitives: primitives, clsx: (...args) => args.filter(Boolean).join(' '), URL,
  _deepseek_ai_dsh_client_runtime_client: { resolveWorkspacePath: (cwd, value) => value }, ToolRow_module_css_default: { fileLink: 'file-link' } };
const region = marker => source.indexOf(`//#region lib/types/client/tool/${marker}`);
for (const [from, to] of [['const VARIANT_TITLES =', 'models/read-card-model.js'], ['function terminalBlockLabels(', 'models/web-card-model.js'], ['function webCardModel(', '../../../node_modules/'], ['function leadingFor$1(', 'toolviews/GenericToolCard.js'], ['const WEB_TITLES =', null]]) {
  const a = source.indexOf(from);
  const b = to === '../../../node_modules/' ? source.indexOf('//#region ../../../node_modules/', a) : to === null ? source.indexOf('//#endregion', a) : region(to);
  assert.ok(a >= 0 && b > a, from); vm.runInNewContext(source.slice(a, b), context);
}
const t = key => key;
const fetchBlock = (url, extra = {}) => ({ kind: 'tool-result', callId: 'fetch-1', call: { name: 'web_fetch', argsRaw: JSON.stringify({ url }) }, resultView: { card: 'web', kind: 'fetch', url, statusCode: 200, truncated: false }, content: [{ type: 'text', text: 'page' }], ...extra });

assert.equal(context.webFetchHref(fetchBlock('https://example.com/page')), 'https://example.com/page');
assert.equal(context.webFetchHref(fetchBlock('http://a.test/')), 'http://a.test/');
assert.equal(context.webFetchHref(fetchBlock('javascript:alert(1)')), undefined, 'script URLs stay plain text');
assert.equal(context.webFetchHref(fetchBlock('not a url')), undefined);
assert.equal(context.webFetchHref({ callId: 'running', argsRaw: '{"url":1}' }), undefined);
assert.equal(context.webFetchHref({ callId: 'running', argsRaw: '{"url":"https://live.test/"' }), undefined, 'partial streamed arguments stay plain text');

const root = Client.createRoot(document.getElementById('root'));
(async () => {
  await React.act(async () => root.render(React.createElement(context.WebRow, { toolName: 'web_fetch', block: fetchBlock('https://example.com/page'), inspect() {}, t })));
  const link = document.querySelector('a[href]');
  assert.ok(link, 'collapsed web_fetch summary is a link');
  assert.equal(link.getAttribute('href'), 'https://example.com/page');
  assert.equal(link.getAttribute('target'), '_blank');
  assert.equal(link.getAttribute('rel'), 'noopener noreferrer');
  link.addEventListener('click', event => event.preventDefault());
  await React.act(async () => link.dispatchEvent(new window.MouseEvent('click', { bubbles: true, cancelable: true })));
  await React.act(async () => link.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Enter', bubbles: true })));
  assert.equal(document.querySelector('[data-web]'), null, 'opening the link does not expand the row');

  await React.act(async () => root.render(React.createElement(context.WebRow, { toolName: 'web_fetch', block: fetchBlock('https://example.com/page', { isError: true, resultView: undefined }), inspect() {}, t })));
  assert.equal(document.querySelector('a[href]'), null, 'a failed fetch keeps its plain error summary');

  await React.act(async () => root.render(React.createElement(context.WebRow, { toolName: 'web_search', block: { ...fetchBlock('https://example.com/'), call: { name: 'web_search', argsRaw: '{"query":"q","url":"https://example.com/"}' } }, inspect() {}, t })));
  assert.equal(document.querySelector('a[href]'), null, 'only web_fetch summaries link');
  await React.act(async () => root.unmount());
  dom.window.close();
  console.log('PASS collapsed web_fetch summary links its http(s) URL without toggling the row');
})().catch(error => { console.error(error); process.exitCode = 1; dom.window.close(); });
