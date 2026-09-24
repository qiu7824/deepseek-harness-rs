const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const root=path.resolve(__dirname,'../..');
const source=fs.readFileSync(path.join(root,'web/src/runtime-plugins/connection.js'),'utf8');
const start=source.indexOf('function sessionStatsOf(log)'),end=source.indexOf('\n\t\t/** Fixed token-meter',start);
assert.ok(start>=0&&end>start);
const context={isTokenDelta:chunk=>['text-delta','reasoning-delta','tool-call-delta'].includes(chunk?.type)};
vm.runInNewContext(source.slice(start,end),context);
const cases=JSON.parse(fs.readFileSync(path.join(root,'crates/session/session-stats/tests/fixtures/request-timing.json'),'utf8'));
for(const test of cases){
  const value=context.sessionStatsOf(test.events);
  for(const [field,expected] of Object.entries(test.expected))assert.equal(value[field],expected,test.name+': '+field);
}
console.log('PASS request timing: '+cases.length+' shared Host/client cases, cancellation, failure, retry, legacy history and clock correction');
