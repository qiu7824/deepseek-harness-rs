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
  emit('step/start', 6);
  const message = (id, text, flags = {}) => emit('assistant/message', 6, { message: { id, source: { provider: 'fixture', model: 'fixture' }, content: [{ type: 'text', text }] }, ...flags });
  message('same-step-a', 'First message.'); message('same-step-b', 'Second message.');
  const rendered = () => context.finalNode(states.get('1:6').state, states.get('1:6'));
  assert.deepEqual(JSON.parse(JSON.stringify(rendered().blocks.map(block => block.text))), ['First message.', 'Second message.'], `${file}: distinct durable message identities survive one step`);
  message('same-step-a', 'First message corrected.');
  assert.deepEqual(JSON.parse(JSON.stringify(rendered().blocks.map(block => block.text))), ['First message corrected.', 'Second message.'], `${file}: same identity replaces its own content without appending a duplicate`);
  emit('assistant/chunk', 6, { chunk: { type: 'text-delta', index: 0, text: 'third partial' } });
  assert.deepEqual(JSON.parse(JSON.stringify(context.visibleAssistantBlocks(states.get('1:6').state).map(block => block.text))), ['First message corrected.', 'Second message.', 'third partial'], `${file}: later message deltas use their own block indexes`);
  emit('llm/retry', 6, { failure: { message: 'temporary' }, retry: 1, mode: 'normal', maxRetries: 2, delayMs: 1 });
  emit('assistant/chunk', 6, { chunk: { type: 'text-delta', index: 0, text: 'third recovered' } });
  message('same-step-c', 'Third recovered.', { interrupted: true });
  assert.equal(rendered().interrupted, true, `${file}: a durable interrupted prefix cannot be reported as completed`);
  assert.deepEqual(JSON.parse(JSON.stringify(rendered().blocks.map(block => block.text))), ['First message corrected.', 'Second message.', 'Third recovered.']);
  message('same-step-c', 'Third completed.', { truncated: true });
  assert.equal(rendered().truncated, true);
  assert.equal(rendered().interrupted, undefined);
  emit('step/start', 7);
  emit('assistant/chunk', 7, { chunk: { type: 'text-delta', index: 0, text: 'failed prefix' } });
  emit('llm/retry', 7, { failure: { message: 'retry' }, retry: 1, mode: 'normal', maxRetries: 2, delayMs: 1 });
  emit('assistant/chunk', 7, { chunk: { type: 'text-delta', index: 0, text: 'replayed prefix' } });
  const replay = file === 'ui-conversation.js' ? context.fallbackState$4 : context.fallbackState$1;
  assert.deepEqual(JSON.parse(JSON.stringify(replay({ matches: states.get('1:7').matches }).blocks.map(block => block.text))), ['replayed prefix'], `${file}: cold-page projection applies retry resets too`);
}
console.log('PASS assistant steps: distinct messages within/between steps; same-ID replacement; later partial stream; retry preservation; durable interruption/truncation metadata');
