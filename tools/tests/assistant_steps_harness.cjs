const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const runtime = { isAppendSurfaceEvent: event => event.surfaceOp === "append", toAssistantBlocks: content => content.map(block => ({ ...block, kind: block.type })), toAssistantBlock: block => ({ ...block, kind: block.type }), isTokenDelta: chunk => chunk.type === 'text-delta' || chunk.type === 'reasoning-delta', displayFailureMessage: failure => failure.message };
for (const [file, marker, definition] of [
  ['ui-conversation.js', '//#region lib/types/client/conversation-nodes/assistant.js', 'assistantDefinition'],
  ['ui-trajectory.js', '//#region lib/types/client/trajectory-assistant-definition.js', 'trajectoryAssistantDefinition'],
]) {
  const source = fs.readFileSync(path.join(__dirname, '../../web/dist/plugins', file), 'utf8'), start = source.indexOf(marker), end = source.indexOf('//#endregion', start);
  const context = { _deepseek_ai_dsh_client_runtime_client: runtime };
  vm.runInNewContext(source.slice(start + marker.length, end) + `;this.definition=${definition};`, context);
  const def = context.definition, states = new Map(); let seq = 0;
  const emit = (type, step, data = {}) => {
    const event = { type, surfaceOp: type === "assistant/message" ? "append" : undefined, seq: seq++, time: Date.now(), data: { turn: 1, step, ...data } }, matched = def.match(event);
    if (!matched) return;
    const match = { event, location: { kind: 'unresolved' } };
    if (matched.role === 'start') states.set(matched.id, { start: match, matches: [match], state: def.start({}, match) });
    else { const current = states.get(matched.id); current.matches.push(match); current.state = def.update(current, match); }
  };
  for (const [step, content] of [[1, [{ type: 'text', text: 'I will inspect the files.' }]], [2, [{ type: 'reasoning', text: 'Compare the two implementations.' }]], [3, [{ type: 'tool-call', id: 'cmd-1', name: 'pwsh', arguments: '{}' }]], [4, [{ type: 'text', text: 'The change is complete.' }]]]) {
    emit('step/start', step); emit('assistant/message', step, { message: { id: `item-${step}`, content, source: { provider: 'codex-cli', model: 'fixture' } } }); emit('step/end', step);
  }
  assert.equal(states.size, 4, `${file}: independent Codex items have distinct step identities`);
  const text = [...states.values()].flatMap(current => context.finalNode(current.state, current).blocks).map(block => block.text || block.name).join('\n');
  for (const expected of ['I will inspect the files.', 'Compare the two implementations.', 'pwsh', 'The change is complete.']) assert.ok(text.includes(expected), `${file}: preserved ${expected}`);
  emit('step/start', 5); emit('assistant/chunk', 5, { chunk: { type: 'text-delta', index: 0, text: 'discarded attempt' } });
  emit('llm/retry', 5, { failure: { message: 'temporary transport failure' }, retry: 1, mode: 'normal', maxRetries: 2, delayMs: 10 });
  emit('assistant/chunk', 5, { chunk: { type: 'text-delta', index: 0, text: 'fresh attempt' } });
  assert.equal(states.get('1:5').state.blocks[0].text, 'fresh attempt', `${file}: retry removes stale chunk content`);
}
console.log('PASS assistant steps: commentary, reasoning, tool call and final remain distinct; retries reset partial text in chat and trajectory');
