const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const source=fs.readFileSync(path.join(__dirname,'../../web/src/runtime-plugins/client-runtime.js'),'utf8');
const begin=source.indexOf('const HISTORY_PAGE_MESSAGES'),end=source.indexOf('\n\t\tvar SessionManager = class',begin);
const context={console,AbortController,transportError:error=>({ok:false,error:{message:String(error)}})};
vm.runInNewContext(source.slice(begin,end)+';this.TestSession=Session;',context);
(async()=>{
 const s=Object.create(context.TestSession.prototype),requests=[];
 s.sessionId='title-owner';s.projections={rows:new Map([['title',{value:'Original',seq:10}]]),apply(key,value,seq){if(seq>(this.rows.get(key)?.seq??-2))this.rows.set(key,{value,seq})}};
 s.api={sessions:{rename:async payload=>{requests.push(payload);return {result:{ok:false,error:{code:'title-conflict',details:{title:'Other window',seq:20}}}}}}};
 const frozen=s.titleEditBase();s.projections.apply('title','Other window',20);
 await s.rename('My draft',frozen);assert.equal(requests[0].expectedTitle.value,'Original');assert.equal(requests[0].expectedTitle.throughSeq,10,'saving uses the opening baseline, not a newer pushed projection');
 assert.equal(s.titleEditBase().value,'Other window');assert.equal(s.titleEditBase().throughSeq,20);
 s.projections.apply('title','Latest third window',25);await s.rename('My draft',frozen);assert.equal(s.titleEditBase().value,'Latest third window','a delayed conflict cannot regress a newer title');
 s.api.sessions.rename=async payload=>({result:{ok:true,value:{title:payload.title.trim(),seq:30}}});
 const adopted=s.titleEditBase();await s.rename(' My draft ',adopted);assert.equal(s.titleEditBase().value,'My draft');
 s.projections.rows.clear();assert.equal(s.titleEditBase(),null,'an unloaded title is distinct from an observed empty title');
 console.log('PASS title edit baseline: frozen draft, conflict state, stale response ordering and explicit adoption');
})().catch(error=>{console.error(error);process.exitCode=1});
