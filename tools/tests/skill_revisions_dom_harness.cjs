const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.argv[2]||process.env.DSH_REACT_TEST_MODULES;
const {JSDOM}=require(path.join(modules,'jsdom')),React=require(path.join(modules,'react'));
const dom=new JSDOM('<main></main>',{url:'http://localhost/'});
Object.assign(global,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true});
const root=require(path.join(modules,'react-dom/client')).createRoot(document.querySelector('main'));
let plugin,registered,Component,revision=4,active=false,withdrawn=false,enabled=true,validated=false,reject=false;
const requests=[];
const candidate=()=>({id:'v1',name:'validated-workflow',description:'Checked workflow',project:'D:/fixture',contentHash:'hash',validation:validated?{}:null,active,withdrawn});
const state=()=>({revision,enabled,candidates:[candidate()]});
const sample=(id,outcome,hash='hash')=>({taskId:id,revision:id==='positive'?31:32,state:'completed',spec:{objective:id,validationSubject:{kind:'skill',identity:hash,expectedOutcome:outcome}}});
const context={document,AbortController,crypto:require('node:crypto').webcrypto,Blob,URL,setTimeout,
  fetch:async(url,options)=>{
    const body=JSON.parse(options.body);requests.push({url,body});
    if(url==='/__dsh-task-execution'){assert.equal(body.summaryOnly,true);return{ok:true,json:async()=>({tasks:[sample('positive','success'),sample('negative','failure'),sample('foreign','success','other')]})};}
    const {method,payload}=body;let value=state(),ok=true;
    if(method.endsWith('Read'))value={...candidate(),ownerSessionId:'owner',content:'A workflow with independently verified samples.',samples:[]};
    else if(!method.endsWith('List')){
      assert.equal(payload.expectedRevision,revision);
      if(reject){ok=false;value=null;}
      else{
        revision++;
        if(method.endsWith('Validate')){assert.deepEqual(payload.samples.map(s=>[s.taskId,s.revision,s.expectedSuccess]),[['positive',31,true],['negative',32,false]]);validated=true;value={state:state(),evidence:{allMatched:true}};}
        else{if(method.endsWith('Activate'))active=true;if(method.endsWith('Withdraw')){active=false;withdrawn=true;}if(method.endsWith('Restore')){withdrawn=false;active=true;}if(method.endsWith('Toggle'))enabled=payload.enabled;value=state();}
      }
    }
    return{ok:true,json:async()=>({result:ok?{ok:true,value}:{ok:false,error:{message:'Evidence changed; verification required'}}})};
  },window:{__ModuleLoader__:{load:definition=>plugin=definition.factory(()=>React)}}};
vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../../web/src/runtime-plugins/ui-settings-skill-revisions.js'),'utf8'),context);
plugin.apply({slots:{inject:(_,fn)=>fn(),register:(config,component)=>{registered=config;Component=component;}}});
const settle=fn=>React.act(async()=>{fn?.();await new Promise(r=>setImmediate(r));});
const button=text=>[...document.querySelectorAll('button')].find(b=>b.textContent===text);
(async()=>{
  assert.equal(registered.id,'skill-revisions');
  await settle(()=>root.render(React.createElement(Component)));
  assert.equal(button('验证并启用').disabled,true);
  await settle(()=>button('查看与验证').click());
  assert.doesNotMatch(document.body.textContent,/foreign/);
  assert.equal(button('核验所选样本').disabled,true);
  const checks=[...document.querySelectorAll('input[type=checkbox]')].filter(e=>e.getAttribute('role')!=='switch');
  await settle(()=>checks[0].click());assert.equal(button('核验所选样本').disabled,true);
  await settle(()=>checks[1].click());await settle(()=>button('核验所选样本').click());
  assert.equal(button('验证并启用').disabled,false);
  reject=true;await settle(()=>button('验证并启用').click());assert.equal(active,false);assert.match(document.querySelector('[role=alert]').textContent,/Evidence changed/);
  reject=false;await settle(()=>button('验证并启用').click());assert.equal(active,true);
  await settle(()=>button('撤回版本').click());assert.equal(withdrawn,true);
  await settle(()=>button('复核并恢复此版本').click());assert.equal(active,true);assert.equal(withdrawn,false);
  await settle(()=>document.querySelector('[role=switch]').click());assert.equal(enabled,false);
  assert.ok(requests.every(r=>!r.body.method?.startsWith('memory.')),'skill controls never enable automatic memories');
  await settle(()=>root.unmount());assert.equal(document.querySelector('style'),null);
  dom.window.close();console.log('PASS skill revisions: sample provenance, positive/negative gate, fresh revisions, failed activation, withdrawal/restore, independent toggle and cleanup');
})().catch(e=>{console.error(e);process.exitCode=1});
