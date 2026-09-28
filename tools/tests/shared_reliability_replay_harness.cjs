const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const root=path.resolve(__dirname,'../..');
const fixture=JSON.parse(fs.readFileSync(path.join(root,'test-fixtures/reliability/session-replay.json'),'utf8'));
const runtime=fs.readFileSync(path.join(root,'web/dist/plugins/client-runtime.js'),'utf8');
const conversation=fs.readFileSync(path.join(root,'web/dist/plugins/ui-conversation.js'),'utf8');
const end=runtime.indexOf('\n\t\t//#endregion',runtime.indexOf('\n\t\tvar SessionManager = class'));
const ctx={console,setTimeout,clearTimeout,queueMicrotask,transportError:error=>({ok:false,error})};
vm.createContext(ctx);
vm.runInContext(runtime.slice(runtime.indexOf('var Notifier = class'),end)+';this.SessionClass=Session;this.coverage=events=>events.reduce((n,e)=>n+eventEndSeq(e)-eventStartSeq(e)+1,0);',ctx);
vm.runInContext('this.helpers={toAssistantBlocks,isTokenDelta:chunk=>chunk.type.endsWith("-delta")};',ctx);
ctx._deepseek_ai_dsh_client_runtime_client=ctx.helpers;
ctx.CHAT_SYNTHETIC_SEQ_OFFSETS={interruptedAssistant:0.1};
const start=conversation.indexOf('//#region lib/types/client/conversation-nodes/assistant.js');
vm.runInContext(conversation.slice(conversation.indexOf('\n',start)+1,conversation.indexOf('//#endregion',start))+';this.project=projectAssistant;',ctx);
const flush=()=>new Promise(resolve=>setImmediate(resolve));
const entries=events=>events.map(event=>({event}));
const response=events=>({result:{ok:true,value:{events:entries(events),hasMore:false,hasMoreBefore:false,hasMoreAfter:false}}});
const sessions=[];
function session(){const s=new ctx.SessionClass('shared-replay',{},{});s.openState='open';sessions.push(s);return s;}
function verify(s){
  assert.equal(s.windowTailSeq(),fixture.events.at(-1).seq);
  assert.deepEqual(Array.from(s.events.filter(e=>e.type==='user/message'),e=>e.data.content[0].text),[fixture.expected.prompt,'开始下一步']);
  const answers=[];
  for(const turn of [1,2]){
    const matches=s.events.filter(e=>e.data.turn===turn&&['assistant/message','assistant/chunk'].includes(e.type)).map(event=>({event,location:{kind:'step',step:{status:'closed',end:s.events.find(e=>e.type==='step/end'&&e.data.turn===turn)},turn:{status:'closed',end:s.events.find(e=>e.type==='turn/end'&&e.data.turn===turn)}}}));
    const projected=ctx.project({matches});
    answers.push(projected.data.blocks.filter(b=>b.kind==='text').map(b=>b.text).join(''));
    if(turn===2)assert.equal(projected.data.status,'interrupted');
  }
  assert.deepEqual(answers,[fixture.expected.assistant,fixture.expected.cancelledPartial]);
  assert.equal(s.events.find(e=>e.data.acceptance).data.acceptance.status,fixture.expected.acceptance);
}
(async()=>{
  const live=session();
  for(const event of fixture.events){live.acceptLiveEvent(event);if(event.seq%fixture.faults.duplicateEvery===0)live.acceptLiveEvent(event);}
  verify(live);assert.equal(ctx.coverage(live.events),fixture.events.length,'coalesced ranges preserve every event exactly once');
  const cold=session();cold.installWindow(entries(fixture.events),false);verify(cold);
  const repair=session();repair.installWindow(entries(fixture.events.slice(0,fixture.faults.gapStart)),false);
  let release;repair.history=()=>new Promise(resolve=>{release=resolve});
  repair.acceptLiveEvent(fixture.events[fixture.faults.gapEnd]);await flush();
  assert.equal(repair.windowTailSeq(),fixture.faults.gapStart-1,'a gap cannot be appended as complete history');
  assert.equal(repair.stitching,true);release(response(fixture.events));await flush();await flush();verify(repair);
  const late=session();late.installWindow(entries(fixture.events),false);let releaseLate;
  late.history=()=>new Promise(resolve=>{releaseLate=resolve});const pending=late.resync();await flush();
  late.openGeneration++;late.openState='open';late.installWindow(entries(fixture.events),false);
  releaseLate(response(fixture.events.slice(0,fixture.faults.lateSnapshotThrough)));await pending;verify(late);
  for(const s of sessions)s.releaseHistory();
  console.log(JSON.stringify({passed:true,rawEvents:fixture.events.length,scenarios:['long-unicode','duplicates','gap-repair','cold-restore','late-snapshot','cancel','acceptance']}));
})().catch(error=>{console.error(error);process.exitCode=1;for(const s of sessions)s.releaseHistory();});
