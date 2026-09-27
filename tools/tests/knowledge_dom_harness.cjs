const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const { JSDOM } = require(path.join(modules, 'jsdom'));
const dom = new JSDOM('<main></main>', { url: 'http://localhost/', pretendToBeVisual: true });
Object.assign(globalThis, { window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
const React = require(path.join(modules, 'react')), Client = require(path.join(modules, 'react-dom/client'));

let bases = [];
const documents = {};
const calls = [];
function respond(value, status = 200) { return { ok: status < 400, status, json: async () => value }; }
const view = base => ({ ...base, documentCount: (documents[base.id] || []).length, chunkCount: (documents[base.id] || []).reduce((sum, doc) => sum + doc.chunkCount, 0) });
const doc = (id, name, extra = {}) => ({ id, baseId: 'kb-1', name, bytes: 2048, chars: 1200, chunkCount: 2, createdAt: '2026-09-20T08:00:00.000Z', ...extra });
async function fetch(url, options) {
  const body = options?.body ? JSON.parse(options.body) : {};
  const operation = url.replace('/__dsh-knowledge/', '');
  calls.push([operation, body]);
  const base = bases.find(row => row.id === body.id);
  switch (operation) {
    case 'catalog': return respond({ bases: bases.map(view), extensions: ['pdf', 'docx', 'md', 'txt'], maxDocumentBytes: 20 });
    case 'create': {
      if (bases.some(row => row.name === body.name.trim())) return respond({ error: '已有同名知识库', code: 'duplicate_name' }, 409);
      const created = { id: 'kb-1', name: body.name.trim(), description: body.description, enabled: true, createdAt: '2026-09-20T08:00:00.000Z', updatedAt: '2026-09-20T08:00:00.000Z' };
      bases.push(created); documents[created.id] = [doc('doc-1', 'Manual.pdf', { source: 'D:\\docs\\Manual.pdf' })];
      return respond({ base: view(created) });
    }
    case 'update': Object.assign(base, Object.fromEntries(Object.entries(body).filter(([key]) => key !== 'id'))); return respond({ base: view(base) });
    case 'delete': bases = bases.filter(row => row.id !== body.id); return respond({ id: body.id, deleted: true });
    case 'documents': return base ? respond({ documents: documents[base.id] }) : respond({ error: 'gone', code: 'not_found' }, 404);
    case 'upload':
      if (body.name.endsWith('.png')) return respond({ error: '不支持的文件类型', code: 'unsupported' }, 400);
      documents[base.id].unshift(doc('doc-' + body.name, body.name, { bytes: Buffer.from(body.data, 'base64').length }));
      return respond({ document: documents[base.id][0] });
    case 'importPath':
      documents[base.id].push(doc('doc-a', 'a.txt'), doc('doc-b', 'b.md'));
      return respond({ report: { added: [doc('doc-a', 'a.txt'), doc('doc-b', 'b.md')], skipped: [{ name: 'D:\\docs\\empty.txt', reason: '文档中没有可索引的文字' }], truncated: false } });
    case 'deleteDocument': for (const id of Object.keys(documents)) documents[id] = documents[id].filter(row => row.id !== body.id); return respond({ id: body.id, deleted: true });
    case 'search': return respond({ results: [{ baseId: 'kb-1', baseName: 'Ops', documentId: 'doc-1', documentName: 'Manual.pdf', chunk: 3, text: 'full text', snippet: '…stop the service, then restore…', score: 3.2 }] });
    default: throw Error('unexpected operation ' + operation);
  }
}

let plugin;
const sandbox = { window: { __ModuleLoader__: { load: spec => { plugin = spec.factory(name => { if (name === 'react') return React; throw Error(name); }); } } }, document, fetch, FileReader: dom.window.FileReader, setTimeout, clearTimeout, Intl, Date, console, Error, JSON, Math, Number, String, Object, Array, Set, Promise };
sandbox.window.confirm = () => true;
sandbox.window.addEventListener = dom.window.addEventListener.bind(dom.window);
sandbox.window.removeEventListener = dom.window.removeEventListener.bind(dom.window);
vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../../web/dist/plugins/ui-knowledge.js'), 'utf8'), sandbox);
const t = key => plugin.test.en[key] || key;
const root = Client.createRoot(document.querySelector('main'));
async function act(fn) { await React.act(async () => { await fn(); for (let i = 0; i < 5; i++) await new Promise(resolve => setTimeout(resolve, 5)); }); }
const button = text => [...document.querySelectorAll('button')].find(node => node.textContent.trim() === text || node.textContent.trim().startsWith(text) || node.textContent.trim() === '+ ' + text);
const fill = (node, text) => { const proto = node.tagName === 'TEXTAREA' ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype; Object.getOwnPropertyDescriptor(proto, 'value').set.call(node, text); node.dispatchEvent(new window.Event('input', { bubbles: true })); };
const text = () => document.querySelector('main').textContent;
const last = name => calls.filter(([operation]) => operation === name).at(-1)?.[1];

(async () => {
  await act(() => root.render(React.createElement(plugin.test.KnowledgePage, { t })));
  assert.ok(document.querySelector('.dshKbEmpty'), 'empty state');
  assert.match(text(), /No knowledge bases yet/);

  // Create from the empty state.
  await act(() => document.querySelector('.dshKbEmpty button').click());
  const dialog = document.querySelector('.dshKbDialog');
  assert.ok(dialog, 'create dialog opens');
  assert.equal(dialog.querySelector('button[type=submit]').disabled, true, 'name is required');
  await act(() => fill(dialog.querySelector('input'), ' Ops '));
  await act(() => fill(dialog.querySelector('textarea'), 'Deploy and rollback'));
  await act(() => dialog.querySelector('button[type=submit]').click());
  assert.deepEqual(last('create'), { name: ' Ops ', description: 'Deploy and rollback' });
  assert.equal(document.querySelector('.dshKbDialog'), null, 'dialog closes');
  assert.equal(document.querySelectorAll('.dshKbCard').length, 1);
  assert.ok(document.querySelector('.dshKbDetail'), 'the new base opens');
  assert.equal(document.querySelector('.dshKbDocList li strong').textContent, 'Manual.pdf');
  assert.equal(document.querySelector('.dshKbDocList li strong').title, 'D:\\docs\\Manual.pdf', 'source path on hover');
  assert.match(document.querySelector('input[type=file]').accept, /\.pdf,\.docx,\.md,\.txt/);

  // Upload: a small file is sent as base64, oversized and rejected files are listed.
  const input = document.querySelector('input[type=file]');
  const files = [new dom.window.File(['hello'], 'hello.md'), new dom.window.File(['x'.repeat(30)], 'big.txt'), new dom.window.File(['png'], 'logo.png')];
  Object.defineProperty(input, 'files', { value: files, configurable: true });
  await act(() => input.dispatchEvent(new window.Event('change', { bubbles: true })));
  await act(() => new Promise(resolve => setTimeout(resolve, 30)));
  const uploads = calls.filter(([operation]) => operation === 'upload').map(([, body]) => body);
  assert.deepEqual(uploads.map(body => [body.name, body.data]), [['hello.md', 'aGVsbG8='], ['logo.png', 'cG5n']], 'oversized files are not sent');
  const failures = [...document.querySelectorAll('.dshKbFailures li')].map(node => node.textContent);
  assert.equal(failures.length, 2);
  assert.match(failures[0], /big\.txt: Files over 20 B cannot be imported/);
  assert.match(failures[1], /logo\.png: 不支持的文件类型/);
  assert.match(document.querySelector('[role=status]').textContent, /Imported 1 documents/);
  assert.equal(document.querySelectorAll('.dshKbDocList li').length, 2);

  // Drag and drop uses the same path.
  const drop = new window.Event('drop', { bubbles: true, cancelable: true });
  Object.defineProperty(drop, 'dataTransfer', { value: { files: [new dom.window.File(['hi'], 'drop.txt')] } });
  await act(() => document.querySelector('.dshKbDrop').dispatchEvent(drop));
  await act(() => new Promise(resolve => setTimeout(resolve, 30)));
  assert.equal(last('upload').name, 'drop.txt');

  // Import a folder by path with Enter; skipped files are reported.
  const pathInput = document.querySelector('.dshKbPathRow input');
  await act(() => fill(pathInput, 'D:\\docs'));
  await act(() => pathInput.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Enter', bubbles: true })));
  assert.deepEqual(last('importPath'), { id: 'kb-1', path: 'D:\\docs' });
  assert.match(document.querySelector('[role=status]').textContent, /Imported 2 documents · 1 skipped/);
  assert.match(document.querySelector('.dshKbFailures').textContent, /empty\.txt/);
  assert.equal(pathInput.value, '', 'path clears after a successful import');
  assert.equal(document.querySelectorAll('.dshKbDocList li').length, 5);

  // Remove one document.
  await act(() => [...document.querySelectorAll('.dshKbDocList li')].find(node => node.textContent.includes('a.txt')).querySelector('button').click());
  assert.deepEqual(last('deleteDocument'), { id: 'doc-a' });
  assert.equal(document.querySelectorAll('.dshKbDocList li').length, 4);
  assert.match(document.querySelector('.dshKbTabs').textContent, /Documents 4/);

  // Enable switch and rename.
  await act(() => document.querySelector('.dshKbSwitch input').click());
  assert.deepEqual(last('update'), { id: 'kb-1', enabled: false });
  assert.match(document.querySelector('.dshKbCard').textContent, /Disabled/);
  const nameInput = document.querySelector('.dshKbGrid input');
  await act(() => fill(nameInput, 'Operations'));
  assert.match(text(), /Unsaved changes/);
  await act(() => button('Save').click());
  assert.deepEqual(last('update'), { id: 'kb-1', name: 'Operations', description: 'Deploy and rollback' });
  assert.equal(document.querySelector('.dshKbDetail h2').textContent, 'Operations');
  assert.doesNotMatch(text(), /Unsaved changes/);

  // Try search within this base.
  await act(() => button('Try search').click());
  const query = document.querySelector('.dshKbSearch input');
  await act(() => fill(query, 'how to roll back'));
  await act(() => document.querySelector('.dshKbSearch form button').click());
  assert.deepEqual(last('search'), { query: 'how to roll back', baseIds: ['kb-1'], limit: 10 });
  assert.match(document.querySelector('.dshKbResults').textContent, /Manual\.pdf#4…stop the service, then restore…/);

  // Delete the base.
  await act(() => button('Delete knowledge base').click());
  assert.deepEqual(last('delete'), { id: 'kb-1' });
  assert.ok(document.querySelector('.dshKbEmpty'), 'back to the empty state');

  await act(() => root.unmount());
  dom.window.close();
  console.log('PASS knowledge UI: create, upload with size and type errors, drop, path import, remove, enable, rename, search and delete');
})().catch(error => { console.error(error); process.exitCode = 1; dom.window.close(); });
