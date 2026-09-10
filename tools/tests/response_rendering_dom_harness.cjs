const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), os = require('node:os'), vm = require('node:vm');
const { pathToFileURL } = require('node:url');
const modules = process.argv[2] || process.env.DSH_REACT_TEST_MODULES;
const { JSDOM } = require(path.join(modules, 'jsdom'));
const dom = new JSDOM('<!doctype html><main id="root"></main>', { url: 'http://fixture.invalid', pretendToBeVisual: true });
Object.assign(global, { window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, MutationObserver: dom.window.MutationObserver, IS_REACT_ACT_ENVIRONMENT: true });
const React = require(path.join(modules, 'react')), jsx = require(path.join(modules, 'react/jsx-runtime'));
const Client = require(path.join(modules, 'react-dom/client'));
const plugins = path.resolve(__dirname, '../../web/dist/plugins'), assets = path.resolve(__dirname, '../../web/dist/assets');
const temporary = fs.mkdtempSync(path.join(process.env.DSH_TEST_TEMP_DIR || os.tmpdir(), 'dsh-response-dom-'));
const shellPath = path.join(assets, fs.readdirSync(assets).find(name => /^index-.*\.js$/.test(name)));
const shell = fs.readFileSync(shellPath, 'utf8');
const created = [], revoked = [], errors = [], sessions = new Map();
const imageURL = { createObjectURL: () => { const url = 'blob:fixture-' + created.length; created.push(url); return url; }, revokeObjectURL: url => revoked.push(url) };
const runtime = { resolveWorkspacePath: (cwd, value) => value.startsWith('/') ? value : cwd + '/' + value };
const root = Client.createRoot(document.getElementById('root'));
const flush = () => new Promise(resolve => setImmediate(resolve));
const act = fn => React.act(async () => { await fn(); await flush(); });
const translate = dictionary => (key, values = {}) => Object.entries(values).reduce((text, [name, value]) => text.replaceAll('{' + name + '}', String(value)), dictionary[key] ?? key);
let primitives;
function load(file, names) {
  let module;
  const context = { document, URL: imageURL, Blob, console, queueMicrotask, setTimeout, clearTimeout, setInterval, clearInterval, requestAnimationFrame: callback => setTimeout(callback, 0), cancelAnimationFrame: clearTimeout,
    window: { getSelection: () => dom.window.getSelection(), __ModuleLoader__: { load: definition => { module = definition.factory(id => id === 'react' ? React : id === 'react/jsx-runtime' ? jsx : id === 'react-dom' ? require(path.join(modules, 'react-dom')) : id.endsWith('ui-primitives') ? primitives : id === '@deepseek-ai/cordis' ? { Service: class { constructor(ctx) { this.ctx = ctx; } } } : runtime); } } } };
  if (file === 'ui-subagent.js') context.URL = URL;
  vm.runInNewContext(fs.readFileSync(path.join(plugins, file), 'utf8').replace('return module.exports;', `exports.test={${names}};return module.exports;`), context);
  return module.test;
}
async function nativeMarkdown() {
  // Execute the shipped parser/URL sanitizer without booting the application.
  const bootstrap = shell.lastIndexOf('const Vc=document.getElementById("root")');
  assert.ok(bootstrap > 0);
  let source = shell.slice(0, bootstrap).replace(/from(["'])(\.\/[^"']+)\1/g, (_all, _quote, relative) => 'from' + JSON.stringify(pathToFileURL(path.join(assets, relative)).href));
  source += '\nexport {bp as renderMarkdown};';
  const filename = path.join(temporary, 'native-markdown.mjs'); fs.writeFileSync(filename, source);
  const native = await import(pathToFileURL(filename));
  // The shipped shell uses React 18; rewrap its plain HTML element tree for the
  // React 19 test renderer without replacing the parser or security decisions.
  const adapt = node => Array.isArray(node) ? node.map(adapt) : node && typeof node === 'object' && node.props ? React.createElement(typeof node.type === 'symbol' ? React.Fragment : node.type, { ...node.props, key: node.key }, adapt(node.props.children)) : node;
  return ({ text }) => React.createElement('div', { 'data-native-markdown': '' }, adapt(native.renderMarkdown(text)));
}
async function main() {
  const name = /DisclosureRow:([\w$]+)/.exec(shell)[1], start = shell.indexOf(`function ${name}(`), end = shell.indexOf('}const ', start) + 1;
  const disclosure = { f: jsx, R: React, Fn: {}, ye: (...args) => args.filter(Boolean).join(' '), Bl: () => null };
  vm.runInNewContext(shell.slice(start, end) + `;this.Component=${name};`, disclosure);
  primitives = new Proxy({ DisclosureRow: disclosure.Component, MarkdownText: await nativeMarkdown(), Menu: ({ anchor, open, items, onSelect }) => React.createElement(React.Fragment, null, anchor, open && React.createElement('div', { role: 'menu' }, items.map(item => React.createElement('button', { key: item.id, title: item.detail, role: 'menuitem', onClick: () => onSelect?.(item.id) }, item.label)))) }, { get: (target, key) => target[key] ?? (() => null) });
  const tool = load('ui-tool.js', 'GenericToolCard,ToolImage,toolDisplayTitle,toolDisplaySummary');
  const conversation = load('ui-conversation.js', 'ConversationController,zh,en,ReasoningRow,messageDefinition,PermissionSelect,registerChatNodeRenderers');
  Object.assign(runtime, load('client-runtime.js', 'contextProvenance,contextForm'));
  const subagent = load('ui-subagent.js', 'SubagentMarkdownOutput,SubagentToolRow,subagentFileLinks,zh,en');
  const trajectory = load('ui-trajectory.js', 'TrajectoryLocale,LaneLabels,RecordTiming,AssistantTimingPanel,StartedAtValue,zh,en');
  const modelUi = load('ui-model-selection.js', 'ModelSelect,zh,en');
  const permissionUi = load('ui-permission.js', 'optionsOf,accessZh,accessEn');
  const zh = translate(conversation.zh), en = translate(conversation.en);
  assert.deepEqual(Object.keys(conversation.zh).sort(), Object.keys(conversation.en).sort());
  assert.deepEqual(Object.keys(trajectory.zh).sort(), Object.keys(trajectory.en).sort());
  let dispose;
  const service = new conversation.ConversationController({ effect: effect => { dispose = effect(); }, get: name => name === 'sessions' ? { binding: id => ({ session: sessions.get(id) }) } : null }, { input: {}, blocks: {} });
  let reads = 0;
  const data = { ok: true, value: { data: [1, 2, 3], attachment: { mediaType: 'image/png' } } };
  sessions.set('session', { readAttachment: async () => { reads++; return data; } });
  const attachment = { attachmentId: 'sha256:shared', mediaType: 'image/png', name: 'frame.png' };
  const loader = service.imageLoader('session');
  const one = await loader.acquire(attachment), two = await loader.acquire(attachment);
  assert.equal(reads, 1); assert.equal(one.url, two.url); assert.equal(service.leasedImages.size, 1);
  one.release(); assert.equal(revoked.length, 0, 'a visible second consumer retains its URL');
  two.release(); await flush(); assert.deepEqual(revoked, [one.url]); assert.equal(service.leasedImages.size, 0);
  let currentLocale = zh;
  const content = [{ type: 'text', text: 'MODEL_ORIGINAL_TEXT' }, { type: 'image', attachment }, { type: 'image', attachment }];
  const block = { kind: 'tool-result', callId: 'capture', call: { name: 'computer_use', argsRaw: '{"action":"start"}' }, content, subCalls: [] };
  const renderTool = () => act(() => root.render(React.createElement(tool.GenericToolCard, { toolName: 'computer_use', block, loadImage: loader, t: currentLocale })));
  await renderTool(); assert.equal(reads, 1, 'collapsed image calls do not read attachments');
  assert.match(document.body.textContent, /工具调用.*computer_use · 启动/);
  await act(() => document.querySelector('[data-disclosure-row]').click());
  assert.equal(document.querySelectorAll('img').length, 1, 'duplicate attachment references show one image');
  assert.equal(service.leasedImages.size, 1); assert.match(document.body.textContent, /MODEL_ORIGINAL_TEXT/);
  currentLocale = en; await renderTool(); assert.match(document.body.textContent, /Tool call.*computer_use · Start/); assert.equal(reads, 2, 'locale updates do not reload visible images');
  await act(() => document.querySelector('[data-disclosure-row]').click()); assert.equal(service.leasedImages.size, 0, 'collapse releases its image lease');
  for (let index = 0; index < 20; index++) {
    const ref = { ...attachment, attachmentId: 'sha256:page-' + index };
    await act(() => root.render(React.createElement(tool.ToolImage, { key: index, attachment: ref, loadImage: loader, t: zh })));
    assert.equal(service.leasedImages.size, 1, 'paging retains only the visible image lease');
  }
  await act(() => root.render(null)); assert.equal(service.leasedImages.size, 0);
  let finish; sessions.set('session', { readAttachment: () => new Promise(resolve => { finish = resolve; }) });
  await act(() => root.render(React.createElement(tool.ToolImage, { attachment: { ...attachment, attachmentId: 'sha256:late' }, loadImage: loader, t: zh })));
  await act(() => root.render(null)); assert.equal(service.leasedImages.size, 0, 'unmount releases pending leases before the attachment RPC settles'); finish(data); await act(flush);
  assert.equal(service.leasedImages.size, 0); assert.equal(created.length, revoked.length, 'an image completing after unmount is released');
  sessions.set('session', { readAttachment: async () => data });
  const ordinary = await service.resolveImage('session', attachment), leased = await loader.acquire(attachment);
  leased.release(); assert.equal(revoked.includes(ordinary), false, 'tool cleanup does not invalidate assistant or user image URLs');
  service.releaseSessionImages('session'); await flush(); assert.equal(revoked.includes(ordinary), true);
  const opened = [], output = '[报告](E:/workspace/report.html) [web](https://example.test/report) [unsafe](javascript:alert) <script>do_not_execute()</script>';
  const snapshot = { byId: {}, subagentsByParent: {} }, store = { getSnapshot: () => snapshot, subscribe: () => () => {} };
  await act(() => root.render(React.createElement(subagent.SubagentToolRow, { block: { kind: 'tool-result', callId: 'subtask', call: { argsRaw: '{"description":"Build report"}' }, content: [{ type: 'text', text: output }] }, parentSessionId: 'parent', sessionsStore: store, refresh() {}, loadProgress: async () => ({ events: [] }), openChild() {}, openFile: target => opened.push(target), t: translate(subagent.zh) })));
  await act(() => document.querySelector('.dsh-subagent-tool-trigger').click());
  const file = [...document.querySelectorAll('a')].find(node => node.textContent === '报告'); assert.ok(file);
  const browserLinks = [];
  const sidebarSource = fs.readFileSync(path.resolve(__dirname, '../../release/plugins/dsh-better-sidebar/lib/client.js'), 'utf8');
  const sidebarContext = { URL, location: dom.window.location, preferences: { values: { httpLinks: 'sidebar', httpsLinks: 'sidebar', showBrowser: true } }, window: { __DSH_BETTER_SIDEBAR_SESSION__: 'fixture' }, tabRegistry: new Map(), tabEnabled: () => true, openPluginTab() {}, openWebTab: (sessionId, url) => browserLinks.push({ sessionId, url }) };
  vm.runInNewContext(sidebarSource.split('\n').find(line => line.includes('const onLink=event=>')) + ';this.onLink=onLink;', sidebarContext);
  document.getElementById('root').setAttribute('data-conversation-scroll', '');
  document.addEventListener('click', sidebarContext.onLink, true);
  await act(() => file.click()); assert.deepEqual(opened, ['E:/workspace/report.html']);
  assert.equal(browserLinks.length, 0, 'the actual sidebar capture listener lets owned local file links reach the file handler');
  await act(() => [...document.querySelectorAll('a')].find(node => node.textContent === 'web').click());
  assert.equal(browserLinks[0].url, 'https://example.test/report', 'ordinary web links still obey the sidebar preference');
  const outside = document.createElement('a'); outside.href = 'http://fixture.invalid/__dsh-file-link/outside'; outside.textContent = 'outside'; document.getElementById('root').appendChild(outside);
  await act(() => outside.click()); assert.equal(browserLinks.at(-1).url, outside.href, 'the bypass is limited to the owned Markdown container'); outside.remove();
  const foreign = document.createElement('a'); foreign.href = 'https://example.test/__dsh-file-link/foreign'; document.querySelector('.dsh-subagent-markdown').appendChild(foreign);
  await act(() => foreign.click()); assert.equal(browserLinks.at(-1).url, foreign.href, 'the bypass is limited to same-origin links'); foreign.remove();
  document.removeEventListener('click', sidebarContext.onLink, true);
  assert.ok([...document.querySelectorAll('a')].some(node => node.href === 'https://example.test/report'));
  assert.equal(document.querySelector('a[href^="javascript:"]'), null); assert.equal(document.querySelector('script'), null);
  for (const literal of ['`[code](E:/workspace/code.txt)`', '```text\n[code](E:/workspace/code.txt)\n```', '    [code](E:/workspace/code.txt)', '![image](E:/workspace/code.png)', '\\[escaped](E:/workspace/code.txt)']) {
    const transformed = subagent.subagentFileLinks(literal, '/fixture/'); assert.equal(transformed.text, literal); assert.equal(transformed.files.size, 0);
  }
  await act(() => root.render(React.createElement(conversation.ReasoningRow, { text: 'ORIGINAL_REASONING', running: false, t: zh })));
  assert.match(document.body.textContent, /思考/);
  assert.equal(document.querySelector('[data-variant="think"]')?.getAttribute('data-expanded'), null, 'completed reasoning is collapsed by default');
  assert.equal(document.querySelector('.yiubIW_thinkBody'), null, 'collapsed reasoning body is not mounted');
  await act(() => document.querySelector('[data-disclosure-row]').click());
  assert.equal(document.querySelector('.yiubIW_thinkBody')?.textContent, 'ORIGINAL_REASONING', 'reasoning expands on demand');
  await act(() => root.render(React.createElement(conversation.ReasoningRow, { text: 'ORIGINAL_REASONING', running: false, t: en })));
  assert.match(document.body.textContent, /Think/);
  for (const [dictionary, expected] of [[trajectory.zh, '输入模型工具'], [trajectory.en, 'InputModelTools']]) {
    await act(() => root.render(React.createElement(trajectory.TrajectoryLocale.Provider, { value: { t: translate(dictionary) } }, React.createElement(trajectory.LaneLabels))));
    assert.equal(document.body.textContent, expected);
  }
  const runningMetrics = { timingRecorded: true, usageProvided: true, stepStartTime: 1000, firstTokenTime: 2000, completedTime: null, outputTokens: 12 };
  for (const [dictionary, pending] of [[trajectory.zh, '待完成'], [trajectory.en, 'Pending'], [trajectory.zh, '待完成']]) {
    await act(() => root.render(React.createElement(trajectory.TrajectoryLocale.Provider, { value: { t: translate(dictionary) } }, React.createElement(trajectory.AssistantTimingPanel, { metrics: runningMetrics }))));
    const values = [...document.querySelectorAll('dd')].map(node => node.textContent);
    assert.deepEqual([values[1], values[3], values[4]], [pending, pending, pending], 'in-progress duration, generation and throughput follow locale changes without completing the request');
    assert.equal(document.body.textContent.includes(pending === 'Pending' ? '待完成' : 'Pending'), false);
  }
  await act(() => root.render(React.createElement(trajectory.TrajectoryLocale.Provider, { value: { t: translate(trajectory.zh) } }, React.createElement(trajectory.AssistantTimingPanel, { metrics: { ...runningMetrics, firstTokenTime: null } }))));
  assert.equal([...document.querySelectorAll('dd')].filter(node => node.textContent === '未提供首 Token 时间').length, 3, 'missing first-token evidence is localized as unavailable, not reported as zero');
  await act(() => root.render(React.createElement(trajectory.TrajectoryLocale.Provider, { value: { t: translate(trajectory.zh) } }, React.createElement('dl', null, React.createElement(trajectory.StartedAtValue, { timestamp: 1000 })))));
  assert.equal(document.querySelector('button').title, '显示 Unix 时间戳');
  await act(() => document.querySelector('button').click()); assert.equal(document.querySelector('button').title, '显示本地时间');
  await act(() => root.render(React.createElement(trajectory.TrajectoryLocale.Provider, { value: { t: translate(trajectory.en) } }, React.createElement('dl', null, React.createElement(trajectory.StartedAtValue, { timestamp: 1000 })))));
  assert.equal(document.querySelector('button').title, 'Show local time');
  const effortState = { status: 'ready', error: null, routable: true, failures: [], current: { provider: 'fixture', model: 'model', reasoningEffort: 'high' }, groups: [{ id: 'fixture', name: 'Fixture', models: [{ id: 'model', name: 'MODEL_NAME_UNCHANGED', reasoning: { defaultEffort: 'high', efforts: [{ id: 'medium', name: 'Medium' }, { id: 'high', name: 'High' }, { id: 'custom', name: 'CUSTOM_LEVEL_UNCHANGED' }] } }] }] };
  const directory = { getSnapshot: () => effortState, subscribe: () => () => {} }, selections = [];
  const renderModel = dictionary => act(() => root.render(React.createElement(modelUi.ModelSelect, { locked: false, available: true, directory, load() {}, select: async value => { selections.push(value); return true; }, t: translate(dictionary) })));
  await renderModel(modelUi.zh); assert.match(document.querySelector('button[aria-haspopup=menu]').textContent, /MODEL_NAME_UNCHANGED.*高/);
  await act(() => document.querySelector('button[aria-haspopup=menu]').click());
  await act(() => [...document.querySelectorAll('[role=menuitem]')].find(node => node.textContent.startsWith('推理等级')).click());
  assert.deepEqual([...document.querySelectorAll('[role=menuitemradio]')].map(node => node.textContent), ['中', '高', 'CUSTOM_LEVEL_UNCHANGED']);
  await act(() => document.querySelector('[role=menuitemradio]').click()); assert.equal(selections[0].reasoningEffort, 'medium'); assert.equal(selections[0].model, 'model');
  await renderModel(modelUi.en); assert.match(document.querySelector('button[aria-haspopup=menu]').textContent, /MODEL_NAME_UNCHANGED.*High/);
  const permission = { currentValue: 'workspace-write', options: [{ value: 'workspace-write', name: 'workspace-write', description: 'Write inside the workspace and permitted temporary directories; wider retries require approval.' }, { value: 'custom-preset', name: 'CUSTOM_PRESET', description: 'CUSTOM_DESCRIPTION_UNCHANGED' }] };
  const commands = [];
  for (const [dictionary, expected] of [[conversation.zh, '可在工作区'], [conversation.en, 'Write inside the workspace']]) {
    await act(() => root.render(React.createElement(conversation.PermissionSelect, { value: permission, locked: false, command: async value => { commands.push(value); }, t: translate(dictionary) })));
    assert.ok(document.querySelector('button').title.startsWith(expected), 'the actual composer permission tooltip follows the current locale');
  }
  await act(() => document.querySelector('button').click()); await act(() => [...document.querySelectorAll('[role=menuitem]')].find(node => node.textContent === 'CUSTOM_PRESET').click());
  assert.equal(commands[0], '/permission custom-preset');
  for (const dictionary of [permissionUi.accessZh, permissionUi.accessEn]) {
    const options = permissionUi.optionsOf(permission, translate(dictionary));
    await act(() => root.render(React.createElement(primitives.Menu, { open: true, items: options })));
    assert.equal(document.querySelectorAll('button')[0].title, dictionary['description.workspaceWrite']);
    assert.equal(document.querySelectorAll('button')[1].title, 'CUSTOM_DESCRIPTION_UNCHANGED');
  }
  const recovery = { type: 'user/message', surfaceOp: 'append', data: { id: 'internal', source: { kind: 'plugin', plugin: 'agent-loop:response-recovery' } } };
  runtime.isAppendSurfaceEvent = event => event.surfaceOp === 'append'; runtime.isReplacementSurfaceEvent = event => event.surfaceOp?.kind === 'replace';
  assert.equal(conversation.messageDefinition.match(recovery), null, 'internal continuation is not a user chat message');
  assert.ok(conversation.messageDefinition.match({ ...recovery, data: { ...recovery.data, source: { kind: 'user' } } }), 'ordinary user messages remain visible');
  const nodeRenderers = new Map();
  conversation.registerChatNodeRenderers({ slots: { inject: (_name, register) => register(), register: (entry, renderer) => nodeRenderers.set(entry.key, renderer) } });
  for (const [suffix, prefix, surfaceOp] of [['initial', true, 'append'], ['dynamic', true, { kind: 'replace', start: 0, end: 0 }], ['legacy', undefined, 'append']]) {
    const event = { type: 'system/message', seq: 2, time: 1000, surfaceOp, data: { prefix, message: { id: suffix, role: 'system', content: [{ type: 'text', text: 'SYSTEM_PROMPT_' + suffix }], source: { kind: 'plugin', plugin: '@deepseek-ai/dsh-system-prompt' } } } };
    const matched = conversation.messageDefinition.match(event);
    assert.ok(matched, `${suffix} system messages are claimed before the unknown-surface fallback`);
    const context = { key: matched.id, id: matched.id, matches: [{ event, location: { kind: 'unresolved' } }] };
    context.state = conversation.messageDefinition.start(context, context.matches[0], { previous: () => undefined });
    const node = conversation.messageDefinition.buildViewNode(context);
    assert.equal(node.kind, 'context', 'system messages dispatch through the registered context renderer');
    assert.equal(node.data.content[0].text, 'SYSTEM_PROMPT_' + suffix, 'nested system message content survives classification');
    await act(() => root.render(React.createElement(nodeRenderers.get(node.kind), { key: suffix, node, t: zh })));
    assert.match(document.body.textContent, /系统提示词.*@deepseek-ai\/dsh-system-prompt/);
    assert.equal(document.querySelector('[data-context-injection-body]'), null, 'system prompt uses the collapsed context disclosure by default');
    assert.equal(document.body.textContent.includes('未知 surface'), false);
    await act(() => document.querySelector('[data-disclosure-row]').click());
    assert.match(document.querySelector('[data-context-injection-body]').textContent, new RegExp('SYSTEM_PROMPT_' + suffix), 'expanded system context exposes the actual model-facing prompt');
  }
  const dynamicContext = { type: 'user/message', seq: 3, time: 1001, surfaceOp: 'append', data: { id: 'runtime-context', content: [{ type: 'text', text: 'DYNAMIC_RUNTIME_CONTEXT' }], source: { kind: 'plugin', plugin: '@deepseek-ai/dsh-system-prompt' } } };
  const dynamicMatch = conversation.messageDefinition.match(dynamicContext);
  const dynamicState = conversation.messageDefinition.start({}, { event: dynamicContext }, { previous: () => undefined });
  assert.equal(dynamicState.provenance.role, 'inject', 'the same producer can publish runtime context without presenting it as another system prompt');
  await act(() => root.render(React.createElement(nodeRenderers.get('context'), { key: dynamicMatch.id, node: { data: dynamicState }, t: zh })));
  assert.match(document.body.textContent, /上下文注入.*@deepseek-ai\/dsh-system-prompt/);
  assert.equal(conversation.messageDefinition.match({ type: 'system/message', seq: 9, surfaceOp: 'append', data: { message: null } }), null, 'malformed system records retain the diagnostic fallback');
  await act(() => root.unmount()); dispose();
  console.log('PASS response rendering DOM: same attachment leases; collapse/paging/late load cleanup; image dedup; live locales; native safe Markdown and actual local-file click; internal recovery filtering; initial, dynamic and legacy system context disclosures');
}
main().catch(error => { errors.push(error); console.error(error); process.exitCode = 1; }).finally(() => { dom.window.close(); fs.rmSync(temporary, { recursive: true, force: true }); });
