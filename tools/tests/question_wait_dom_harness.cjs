const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const { JSDOM } = require(path.join(modules, 'jsdom'));
const dom = new JSDOM('<main id="root"></main>', { url: 'http://fixture.invalid/' });
Object.assign(global, { window: dom.window, document: dom.window.document, IS_REACT_ACT_ENVIRONMENT: true });
const React = require(path.join(modules, 'react')), jsx = require(path.join(modules, 'react/jsx-runtime'));
const Client = require(path.join(modules, 'react-dom/client'));
const primitives = new Proxy({ Button: ({ children, variant, ...props }) => React.createElement('button', props, children) }, {
  get: (target, key) => target[key] || (props => React.createElement('svg', { 'data-icon': key, width: props.size || 16, height: props.size || 16, 'aria-hidden': true }))
});
let exported;
const source = fs.readFileSync(path.join(__dirname, '../../web/dist/plugins/ui-user-questions.js'), 'utf8');
vm.runInNewContext(source.replace('return module.exports;', 'exports.test={QuestionFlow,zh};return module.exports;'), {
  document, console, window: { __ModuleLoader__: { load: module => { exported = module.factory(id => id === 'react' ? React : id === 'react/jsx-runtime' ? jsx : primitives); } } }
});
const { QuestionFlow, zh } = exported.test;
const t = key => zh[key] || key;
const root = Client.createRoot(document.getElementById('root'));
const act = fn => React.act(async () => { await fn(); await new Promise(resolve => setImmediate(resolve)); });
let attempts = 0, rejectAnswer, cancelCount = 0;
const answers = [];
const pending = { key: 'first', questions: [{ id: 'format', question: '选择输出格式', options: [{ label: 'PDF' }, { label: 'Word' }] }],
  answer: answer => { attempts++; answers.push(answer); return new Promise((resolve, reject) => { rejectAnswer = reject; }); },
  cancel: () => { cancelCount++; return Promise.resolve(); }
};
const submit = () => [...document.querySelectorAll('button')].find(node => node.textContent === 'submit');
(async () => {
  await act(() => root.render(React.createElement(QuestionFlow, { key: pending.key, pending, t })));
  assert.equal(document.querySelector('[data-question-status]').textContent, '等待回答');
  assert.ok(document.querySelector('[data-question-status] svg[data-icon="IconQuestionOutline14"]'));
  assert.equal(submit().disabled, true); assert.equal(attempts, 0, 'waiting does not submit a suggested answer');
  await act(() => document.querySelector('[role=radio][aria-label=PDF]').click());
  assert.equal(attempts, 0, 'selecting an option waits for explicit submission');
  await act(() => submit().click());
  assert.equal(document.querySelector('[data-question-key]').getAttribute('aria-busy'), 'true');
  assert.equal(document.querySelector('[data-question-status]').textContent, '正在提交回答');
  assert.ok([...document.querySelectorAll('button')].every(node => node.disabled));
  assert.equal(answers[0].answers[0].selected[0], 'PDF');
  await act(() => rejectAnswer(new Error('fixture disconnected')));
  assert.equal(document.querySelector('[data-question-status]').textContent, '等待回答');
  assert.match(document.body.textContent, /fixture disconnected/);
  assert.equal(submit().disabled, false, 'failed submission preserves the answer for retry');
  await act(() => submit().click()); assert.equal(attempts, 2);
  const next = { ...pending, key: 'second' };
  await act(() => root.render(React.createElement(QuestionFlow, { key: next.key, pending: next, t })));
  assert.equal(submit().disabled, true, 'the next request has no stale selection or submitting latch');
  await act(() => document.querySelector('[aria-label="放弃整组问题"]').click());
  assert.equal(cancelCount, 1); assert.equal(document.querySelector('[data-question-status]').textContent, '正在取消');
  await act(() => root.unmount()); dom.window.close();
  console.log('PASS question wait: icon/text, explicit submission, busy lock, failed-answer retry, fresh request isolation and cancellation');
})().catch(error => { console.error(error); process.exitCode = 1; dom.window.close(); });
