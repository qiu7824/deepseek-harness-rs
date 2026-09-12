const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
for(const [file,marker] of [
  ['ui-conversation.js','//#region lib/types/client/conversation-nodes/assistant.js'],
  ['ui-trajectory.js','//#region lib/types/client/trajectory-assistant-definition.js']
]){
const source=fs.readFileSync(path.join(__dirname,'../../web/dist/plugins',file),'utf8');
const start=source.indexOf(marker),end=source.indexOf('//#endregion',start);
assert.ok(start>=0&&end>start);
const context=vm.createContext({_deepseek_ai_dsh_client_runtime_client:{isTokenDelta:chunk=>chunk.type.endsWith('-delta')}});
vm.runInContext(`var scanned=0;const originalTrim=String.prototype.trim;String.prototype.trim=function(){scanned+=this.length;return originalTrim.call(this)};`,context);
vm.runInContext(source.slice(start+marker.length,end),context);
for(const blankPrefix of [false,true]){
  context.blankPrefix=blankPrefix;
  const result=vm.runInContext(`(() => {
    scanned=0;let state=initialState(1,1);const delta=blankPrefix?' '.repeat(64):'性能采样：完整保存连续生成的内容。'.repeat(4);
    const count=4000;
    for(let i=0;i<count;i++){
      state=updateChunk(state,{event:{type:'assistant/chunk',seq:i,time:i,data:{turn:1,step:1,chunk:{type:'reasoning-delta',index:0,text:delta}}}});
      if(i%16===0)hasVisibleContent(state.blocks);
    }
    const visibleBefore=hasVisibleContent(state.blocks);
    state=updateChunk(state,{event:{type:'assistant/chunk',seq:count,time:count,data:{turn:1,step:1,chunk:{type:'reasoning-delta',index:0,text:'完成'}}}});
    return {scanned,characters:delta.length*count+2,exact:state.blocks[0].text===delta.repeat(count)+'完成',visibleBefore,visibleAfter:hasVisibleContent(state.blocks)};
  })()`,context);
  assert.equal(result.exact,true);
  assert.equal(result.visibleBefore,!blankPrefix);
  assert.equal(result.visibleAfter,true);
  assert.ok(result.scanned<=result.characters*8,`${file}: visibility checks must be linear in received text, scanned ${result.scanned} for ${result.characters} characters`);
}
}
console.log('PASS chat and trajectory streamed reasoning visibility: exact text and linear inspection work for visible and whitespace prefixes');
