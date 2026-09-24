const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const root = path.join(__dirname, '../..');
const read = name => fs.readFileSync(path.join(root, 'web/src/runtime-plugins', name), 'utf8');
const native = {id:'result-1', role:'tool', toolCallId:'call-1', isError:true, source:{kind:'tool',callId:'call-1'}, content:[{type:'text',text:'Denied'}]};
const legacy = {id:'result-1', role:'user', source:{kind:'tool',callId:'call-1'}, content:[{type:'tool-result',toolCallId:'call-1',isError:true,content:native.content}]};
for (const file of ['ui-conversation.js', 'ui-trajectory.js']) {
  const source = read(file), begin = source.indexOf('function rootResult('), end = source.indexOf('\n\t\tfunction ', begin + 10), scope = {};
  assert.ok(begin >= 0 && end > begin);
  vm.runInNewContext(source.slice(begin, end) + ';this.result=rootResult;', scope);
  for (const message of [native, legacy]) {
    const projected = scope.result({event:{type:'tool/result',seq:5,time:10,data:{message}}});
    assert.equal(projected.callId, 'call-1');
    assert.equal(projected.isError, true, file);
    assert.equal(projected.content[0].text, 'Denied', file);
  }
}
const connection = read('connection.js'), scope = {};
const begin = connection.indexOf('function isDeepEqualJson('), end = connection.indexOf('/** Validate one event at its replay boundary', begin);
assert.ok(begin >= 0 && end > begin);
vm.runInNewContext(connection.slice(begin, end) + ';this.rewrite=assertToolResultRewrite;', scope);
for (const message of [native, legacy]) {
  const original = {type:'tool/result',seq:0,data:{message}}, pruned = structuredClone(original);
  if (pruned.data.message.role === 'tool') pruned.data.message.content = [];
  else pruned.data.message.content[0].content = [];
  scope.rewrite(pruned, [0], [original], 0);
  for (const mutation of [m => m.source.callId = 'other', m => m.id = 'other', m => m.role === 'tool' ? m.isError = false : m.content[0].isError = false]) {
    const invalid = structuredClone(pruned); mutation(invalid.data.message);
    assert.throws(() => scope.rewrite(invalid, [0], [original], 0), /only content/);
  }
}
const surfaceScope = {};
const surfaceBegin = connection.indexOf('const SURFACE_EVENT_TYPES = new Set(');
const surfaceEnd = connection.indexOf('function assertProvenance(', surfaceBegin);
vm.runInNewContext(connection.slice(surfaceBegin, surfaceEnd) + ';this.operation=surfaceOpOf;this.message=deriveEventMessage;', surfaceScope);
for (const op of [{op:'replace',start:2,end:4},{op:'replace',startSeq:2,endSeq:4}]) {
  const event = {type:'developer/message',surfaceOp:op,data:{message:{id:'d',role:'developer',source:{kind:'tools'},content:[]}}};
  assert.equal(surfaceScope.operation(event).start, 2);
  assert.equal(surfaceScope.operation(event).end, 4);
  assert.equal(surfaceScope.message(event).role, 'developer');
}
assert.throws(() => surfaceScope.operation({type:'user/message',surfaceOp:{op:'replace',startSeq:2,end:4}}), /invalid/);
console.log('PASS native and legacy tool results, immutable correlation, developer messages and both replacement wire formats');
