const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const React = require(path.join(modules, 'react')), jsx = require(path.join(modules, 'react/jsx-runtime'));
const { JSDOM } = require(path.join(modules, 'jsdom'));
const dom = new JSDOM('<main id="root"></main>', { url: 'http://localhost', pretendToBeVisual: true });
Object.assign(global, { window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
let exposed;
const primitives = { Modal: ({ open, children, onClose }) => open && React.createElement('section', { role: 'dialog' }, React.createElement('button', { onClick: onClose }, 'close'), children) };
const source = fs.readFileSync(path.join(__dirname, '../../web/src/runtime-plugins/ui-deliverables.js'), 'utf8');
vm.runInNewContext(source.replace('exports.apply = apply;', 'exports.test = {ProducedFiles, producedFileType, fitProducedFiles}; exports.apply = apply;'), {
  window: { __ModuleLoader__: { load: entry => { exposed = entry.factory(name => name === 'react' ? React : name === 'react/jsx-runtime' ? jsx : name.endsWith('ui-primitives') ? primitives : {}).test; } } },
  document, getComputedStyle: () => ({ columnGap: '6' }),
});
const root = require(path.join(modules, 'react-dom/client')).createRoot(document.getElementById('root'));
const calls = [], t = (key, args) => key + (args?.count ? ':' + args.count : '');
const act = fn => React.act(async () => { await fn(); });
const paths = ['D:/a/重复文件.rs', 'D:/b/重复文件.rs', 'D:/长文件名'.repeat(12) + '.pdf', 'D:/a/file.json', 'D:/a/image.png', 'D:/a/unknown.strange', 'D:/a/seventh.txt'];
(async () => {
  assert.equal(exposed.fitProducedFiles(120, 6, [600], [50, undefined]), 1, 'a long single filename remains visible and truncatable');
  assert.equal(exposed.fitProducedFiles(20, 6, [600, 100], [40, 40, undefined]), 0, 'extremely narrow rows keep the remainder instead of overflowing');
  for (const theme of ['light', 'dark']) {
    document.documentElement.dataset.theme = theme;
    await act(() => root.render(React.createElement(exposed.ProducedFiles, { matched: { declared: true, paths }, openFile: (...args) => calls.push(args), isLoopback: true, useHostGeneration: () => ({ status: 'ready' }), t })));
    const row = document.querySelector('[data-produced-files-row]');
    const buttons = row.querySelectorAll('button.D13QPq_file'); assert.equal(buttons.length, 6);
    assert.equal(buttons[0].textContent, 'RS重复文件.rs'); assert.equal(buttons[1].textContent, 'RS重复文件.rs');
    assert.notEqual(buttons[0].title, buttons[1].title);
    assert.equal(buttons[2].querySelector('svg').dataset.fileType, 'PDF');
    assert.equal(buttons[5].querySelector('svg').dataset.fileType, 'FILE');
    assert.equal(document.querySelectorAll('svg [id],svg [href],svg [xlink\\:href]').length, 0, 'built-in icons contain no shared SVG ids or external references');
    await act(() => buttons[1].click()); assert.equal(calls.at(-1)[0], paths[1]);
    await act(() => buttons[0].dispatchEvent(new window.MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 20, clientY: 30 })));
    assert.equal(calls.at(-1)[0], paths[0]); assert.equal(calls.at(-1)[1].intent, 'menu');
    assert.match(row.textContent, /produced.moreOne/);
    assert.ok([...buttons].every(button => button.getAttribute('aria-label')), 'file open actions keep accessible names');
    await act(() => row.querySelector('button.D13QPq_more').click());
    const all = document.querySelectorAll('[data-all-produced-files] button'); assert.equal(all.length, 7);
    await act(() => all[6].click()); assert.equal(calls.at(-1)[0], paths[6]);
    assert.equal(document.querySelector('[role=dialog]'), null);
  }
  await act(() => root.unmount()); dom.window.close();
  console.log('PASS file presentation: compact typed icons, duplicate names, unknown types, narrow row sizing, themes and exact-path actions');
})().catch(error => { console.error(error); process.exitCode = 1; root.unmount(); dom.window.close(); });
