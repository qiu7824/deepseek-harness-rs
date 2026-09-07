const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const text=fs.readFileSync(require('node:path').join(__dirname,'../../web/dist/plugins/client-runtime.js'),'utf8');
const start=text.indexOf('const HISTORY_PAGE_MESSAGES'),end=text.indexOf('\n\t\tvar SessionManager = class',start);
const ctx={console,conversationInput:x=>x};vm.runInNewContext(text.slice(start,end)+';this.TestSession=Session;',ctx);
const session=Object.create(ctx.TestSession.prototype);
let rebuilds=0,prepends=0;
Object.assign(session,{events:[],views:[],historyPages:[],hasMoreBefore:true,hasMoreAfter:false,baseSeq:0,openState:'open',openGeneration:0,loadingOlder:false,loadingNewer:false,historyTargetSeq:null,notifier:{markDirty(){}},conversation:{replaceWindow(){rebuilds++},prepend(){prepends++}},liveBuffer:[]});
const entry=seq=>({event:{seq,type:'user/message',time:seq,data:{text:'x'.repeat(180000)}},view:undefined});
const appendPage=first=>{const page=Array.from({length:12},(_,i)=>entry(first+i));session.events.push(...page.map(x=>x.event));session.views.push(...page.map(x=>x.view));session.historyPages.push(session.pageMeta(page));return page;};
appendPage(1200);assert.equal(session.trimHistoryWindow('head'),false);assert.equal(rebuilds,0,'unchanged windows must not rebuild');
session.baseSeq=1200;
session.history=async payload=>({result:{ok:true,value:{events:Array.from({length:12},(_,i)=>entry(payload.beforeSeq-12+i)),hasMoreBefore:payload.beforeSeq>12}}});
(async()=>{
 for(let i=0;i<85;i++){
  const before=rebuilds;await session.loadOlder();assert.ok(rebuilds-before<=1,'paging must rebuild at most once');
  assert.ok(session.historyPages.length<=5);assert.ok(session.historyPages.length===1||session.historyPages.reduce((n,p)=>n+p.bytes,0)<=8*1024*1024);
  assert.equal(session.events.length,session.historyPages.reduce((n,p)=>n+p.eventCount,0));
  assert.equal(session.hasMoreAfter,true);assert.notEqual(session.historyTargetSeq,null,'evicting the live tail must enter historical browsing');
 }
 const before=session.events;
 let release;session.history=()=>new Promise(resolve=>{release=resolve});const pending=session.loadOlder();session.openGeneration++;
 release({result:{ok:true,value:{events:Array.from({length:12},(_,i)=>entry(session.baseSeq-12+i)),hasMoreBefore:true}}});await pending;
 assert.equal(session.events,before,'stale pages cannot rewrite a new window');
 console.log(JSON.stringify({pages:85,retainedEvents:session.events.length,retainedBytes:session.historyPages.reduce((n,p)=>n+p.bytes,0),rebuilds,prepends}));
})().catch(error=>{console.error(error);process.exitCode=1});
