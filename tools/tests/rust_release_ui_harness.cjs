const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const plugins = path.resolve(__dirname, '../../web/dist/plugins');

const skills = fs.readFileSync(path.join(plugins, 'ui-skill.js'), 'utf8');
const skillStart = skills.indexOf('function rankSkillNames(');
const skillEnd = skills.indexOf('\n\t\tconst ', skillStart);
const skillContext = {};
vm.runInNewContext(skills.slice(skillStart, skillEnd), skillContext);
const catalog = ['build-site', 'site-builder', 'browser-tools', 'BUILD'].map(name => ({ name }));
assert.deepEqual(Array.from(skillContext.rankSkillNames(catalog, 'build'), row => row.name), ['build-site', 'BUILD', 'site-builder']);
assert.deepEqual(Array.from(skillContext.rankSkillNames(catalog, 'bst'), row => row.name), ['build-site', 'browser-tools']);
assert.equal(skillContext.rankSkillNames(catalog, 'xyz').length, 0);
assert.equal(skillContext.rankSkillNames(catalog, '').length, catalog.length);

const jsx = (type, props, key) => ({ type, props, key });
const effects = [];
const react = { memo: fn => fn, useState: () => [true, () => {}], useMemo: fn => fn(), useEffect: fn => effects.push(fn), useRef: value => ({ current: value }), useCallback: fn => fn };
const primitives = new Proxy({}, { get: (_, name) => name });
let exported;
let source = fs.readFileSync(path.join(plugins, 'ui-tool.js'), 'utf8');
source = source.replace('exports.apply = apply;', 'exports.__test = { imageCardModel, GenericToolCard, ToolCallTree, ToolCallBranch, ToolCall, ToolRow, ToolImage }; exports.apply = apply;');
vm.runInNewContext(source, {
    window: { __ModuleLoader__: { load: definition => { exported = definition.factory(name => {
        if (name === 'react') return react;
        if (name === 'react/jsx-runtime') return { jsx, jsxs: jsx, Fragment: 'fragment' };
        return primitives;
    }); } } },
});
const api = exported.__test;
const attachment = { attachmentId: 'custom-provider:image-1', name: 'result.png' };
const result = { kind: 'tool-result', callId: 'image-call', call: { name: 'read_image', argsRaw: '{"file_path":"/work/result.png"}' }, content: [{ type: 'text', text: 'image result' }, { type: 'image', attachment }], isError: false, subCalls: [] };
assert.equal(api.imageCardModel('read_image', result).images[0], attachment);
assert.equal(api.imageCardModel('read', result), null);
assert.equal(api.imageCardModel('read_image', { ...result, isError: true }), null);
assert.equal(api.imageCardModel('read_image', { ...result, content: [{type: 'image', attachment: {attachmentId: ''}}] }), null);
const loadImage = async () => 'blob:fixture-image';
const common = { cwd: '/work', openFile() {}, inspectCall() {}, t: key => key, renderSlot() {}, loadImage };
const card = api.GenericToolCard({ ...common, toolName: 'read_image', block: result });
assert.equal(card.props.variant, 'read');
assert.equal(card.props.output, null);
assert.equal(card.props.image.props.loadImage, loadImage);
assert.equal(card.props.image.props.model.text, 'image result');
const nested = { ...result, callId: 'nested-image' };
const tree = api.ToolCallTree({ ...common, node: {data: {root: {...result, subCalls: [nested]}}} });
assert.equal(tree.props.loadImage, loadImage);
const branch = api.ToolCallBranch(tree.props);
assert.equal(branch.props.loadImage, loadImage);
assert.equal(branch.props.children.props.children[0].props.loadImage, loadImage);
let owner;
api.ToolCall({ ...branch.props, renderSlot: (_, value) => { owner = value; return null; } });
assert.equal(owner.loadImage, loadImage);
assert.equal(owner.block.callId, 'image-call');

// Disclosure must not start image fetches while its row is collapsed.
react.useState = () => [false, () => {}];
const closedRow = api.ToolRow(card.props);
function contains(value, target) {
    if (value === target) return true;
    return value !== null && typeof value === 'object' && Object.values(value).some(child => contains(child, target));
}
assert.equal(contains(closedRow, card.props.image), false);
react.useState = () => [true, () => {}];
assert.equal(contains(api.ToolRow(card.props), card.props.image), true);
console.log('Rust release UI: fuzzy discovery, image result guards and nested image loading passed');

let workspaceApi;
const values = [];
let hook = 0;
react.useState = initial => {
    const index = hook++;
    if (!(index in values)) values[index] = typeof initial === 'function' ? initial() : initial;
    return [values[index], value => { values[index] = typeof value === 'function' ? value(values[index]) : value; }];
};
const workspaceSource = fs.readFileSync(path.join(plugins, 'ui-workspace.js'), 'utf8').replace('exports.apply = apply;', 'exports.__test = { WorkspaceBrowser }; exports.apply = apply;');
vm.runInNewContext(workspaceSource, { window: { __ModuleLoader__: { load: definition => {
    workspaceApi = definition.factory(name => name === 'react' ? react : name === 'react/jsx-runtime' ? {jsx, jsxs: jsx, Fragment: 'fragment'} : primitives).__test;
} } } });
const opened = [];
const workspaceState = {items: [], phase: 'ready', archivedSessionIds: []};
const view = {groupBy: 'workspace', orderBy: 'updated', groupExpansion: {}, sessionOrderByAccount: {}, sessionUpdatedAtByAccount: {}};
const workspaceProps = {
    wide: true, useWorkspaces: select => select(workspaceState), useStore: select => select(view), useDirectoryFlow: select => select(true),
    actions: {}, open: id => opened.push(id), t: key => key,
};
function findElement(value, name) {
    if (!value || typeof value !== 'object') return undefined;
    if (typeof value.type === 'function' && value.type.name === name) return value;
    for (const child of Object.values(value)) { const found = findElement(child, name); if (found) return found; }
}
values[0] = 'Image Search Target';
values[1] = true;
const searchPage = workspaceApi.WorkspaceBrowser(workspaceProps);
const search = findElement(searchPage, 'SearchResults');
assert.ok(search, 'search mode renders results');
search.props.open('target-session');
assert.deepEqual(opened, ['target-session']);
assert.equal(values[0], '');
assert.equal(values[1], false);
hook = 0;
const revealedPage = workspaceApi.WorkspaceBrowser(workspaceProps);
assert.equal(findElement(revealedPage, 'SearchResults'), undefined);
assert.equal(findElement(revealedPage, 'SessionTree').props.revealSessionId, 'target-session');
console.log('Rust workspace search: selection exits search and requests a targeted reveal');
