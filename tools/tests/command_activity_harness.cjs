const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const source=fs.readFileSync(path.join(__dirname,'../../web/src/runtime-plugins/client-runtime.js'),'utf8');
const begin=source.indexOf('const HISTORY_PAGE_MESSAGES'),end=source.indexOf('\n\t\tvar SessionManager = class',begin);
const ctx={console,AbortController};vm.runInNewContext(source.slice(begin,end)+';this.TestSession=Session;',ctx);
const flush=()=>new Promise(resolve=>setImmediate(resolve));
function fixture(id='s'){
 const requests=[],session=Object.create(ctx.TestSession.prototype);
 Object.assign(session,{sessionId:id,api:{commandActivity:(sessionId,signal)=>new Promise(resolve=>requests.push({sessionId,signal,resolve}))},disposed:false,removed:false,openGeneration:0,openState:'open',events:[],readingAwayFromTail:false,historyTargetSeq:null,commandRunning:false,liveCommands:new Set(),commandActivityRevision:0,commandActivityAbort:null,notifier:{markDirty(){}},acceptLiveEvent(){},queueMirror:{reset:()=>false},pending:new Map(),pendingRev:0,beginHistoryNavigation(){},releaseHistory(){},open(){return session.refreshCommandActivity()}});
 return {session,requests};
}
const answer=(request,active)=>request.resolve({result:{ok:true,value:{active}}});
const event=(session,type,id,name='compact')=>session.handleMuxEnvelope('rpc',{type:'session/event',event:{type,data:{commandId:id,name}}});
(async()=>{
 const {session:s,requests}=fixture();
 event(s,'command/run','first');assert.equal(s.commandRunning,true,'live command exposes Stop before status reply');
 event(s,'command/done','first');assert.equal(requests[0].signal.aborted,true);
 answer(requests[1],false);await flush();assert.equal(s.commandRunning,false);
 answer(requests[0],true);await flush();assert.equal(s.commandRunning,false,'stale start response cannot resurrect completed work');
 s.handleMuxEnvelope('rpc',{type:'session/subscribed',lastSeq:100});answer(requests[2],true);await flush();
 assert.equal(s.commandRunning,true,'a fresh page recovers live work without replaying a command/run');
 const stale=s.refreshCommandActivity();const staleRequest=requests.at(-1);
 const reconnect=s.resync();const fresh=requests.at(-1);assert.notEqual(staleRequest,fresh);
 answer(fresh,false);await reconnect;answer(staleRequest,true);await stale;
 assert.equal(s.commandRunning,false,'reconnect status replaces the old Host generation');
 event(s,'command/run','second');const live=requests.at(-1);s.dispose();answer(live,true);await flush();
 assert.equal(s.commandRunning,false);assert.equal(live.signal.aborted,true,'disposed session drops pending status reads');
 const other=fixture('other');other.session.handleMuxEnvelope('rpc',{type:'session/subscribed',lastSeq:3});answer(other.requests[0],true);await flush();assert.equal(other.session.commandRunning,true);assert.equal(s.commandRunning,false);
 other.session.dispose();
 console.log('PASS command activity: live lifecycle, stale replies, reload bootstrap, reconnect generation, independent scopes and disposal');
})().catch(error=>{console.error(error);process.exitCode=1});
