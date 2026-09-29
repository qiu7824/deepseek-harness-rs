const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.argv[2]||process.env.DSH_REACT_TEST_MODULES;
const {JSDOM}=require(path.join(modules,'jsdom')),React=require(path.join(modules,'react'));
const dom=new JSDOM('<main></main>',{url:'http://localhost/'});
Object.assign(global,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true});
const root=require(path.join(modules,'react-dom/client')).createRoot(document.querySelector('main'));
let plugin,registered,Component,revision=4,active=false,withdrawn=false,enabled=true,reject=false;
const requests=[];
const candidate=()=>({id:'v1',name:'manual-workflow',description:'Project workflow',project:'D:/fixture',contentHash:'hash',activationMode:'manual',active,withdrawn});
const state=()=>({revision,enabled,candidates:[candidate()]});
const context={document,AbortController,crypto:require('node:crypto').webcrypto,Blob,URL,setTimeout,
  fetch:async(url,options)=>{
    assert.ok(!url.includes('task-execution'));
    const body=JSON.parse(options.body);requests.push({url,body});
    const {method,payload}=body;let value=state(),ok=true;
    if(method.endsWith('Read'))value={...candidate(),content:'Manually selected workflow content.'};
    else if(!method.endsWith('List')){
      assert.equal(payload.expectedRevision,revision);assert.ok(!('samples' in payload));assert.ok(!method.endsWith('Validate'));
      if(reject){ok=false;value=null;}
      else{
        revision++;
        if(method.endsWith('Create')){assert.equal(payload.project,'D:/fixture');assert.equal(payload.name,'manual-workflow');assert.ok(payload.content);value={candidate:{...candidate(),id:'v2'},state:state()};}
        else{if(method.endsWith('Activate'))active=true;if(method.endsWith('Withdraw')){active=false;withdrawn=true;}if(method.endsWith('Restore')){withdrawn=false;active=true;}if(method.endsWith('Toggle'))enabled=payload.enabled;value=state();}
      }
    }
    return{ok:true,json:async()=>({result:ok?{ok:true,value}:{ok:false,error:{message:'Revision changed; reload versions'}}})};
  },window:{__ModuleLoader__:{load:definition=>plugin=definition.factory(()=>React)}}};
vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../../web/src/runtime-plugins/ui-settings-skill-revisions.js'),'utf8'),context);
plugin.apply({get:name=>name==='sessions'?{list:{getSnapshot:()=>({current:'current',byId:{current:{cwd:'D:/current-project'}}})}}:undefined,slots:{inject:(_,fn)=>fn(),register:(config,component)=>{registered=config;Component=component;}}});
const settle=fn=>React.act(async()=>{fn?.();await new Promise(r=>setImmediate(r));});
const button=text=>[...document.querySelectorAll('button')].find(b=>b.textContent===text);
(async()=>{
  assert.equal(registered.id,'skill-revisions');await settle(()=>root.render(React.createElement(Component)));
  assert.equal(button('启用版本').disabled,false);
  await settle(()=>button('创建技能版本').click());assert.equal([...document.querySelectorAll('form input')][2].value,'D:/current-project');
  await settle(()=>button('取消编辑').click());await settle(()=>button('查看版本').click());
  assert.doesNotMatch(document.body.textContent,/验收|样本|已验证|核验/);
  await settle(()=>button('编辑为新版本').click());assert.equal(document.querySelector('form textarea').value,'Manually selected workflow content.');
  await settle(()=>document.querySelector('form').dispatchEvent(new dom.window.Event('submit',{bubbles:true,cancelable:true})));
  assert.equal(active,false,'saving a revision never activates it');assert.match(document.body.textContent,/版本已保存，尚未启用/);
  reject=true;await settle(()=>button('启用版本').click());assert.equal(active,false);assert.match(document.querySelector('[role=alert]').textContent,/Revision changed/);
  reject=false;await settle(()=>button('启用版本').click());assert.equal(active,true);
  await settle(()=>button('撤回版本').click());assert.equal(withdrawn,true);
  await settle(()=>button('恢复版本').click());assert.equal(active,true);assert.equal(withdrawn,false);
  await settle(()=>document.querySelector('[role=switch]').click());assert.equal(enabled,false);assert.equal(button('创建技能版本').disabled,true);
  assert.ok(requests.every(r=>!r.body.method?.startsWith('memory.')));
  await settle(()=>root.unmount());assert.equal(document.querySelector('style'),null);
  dom.window.close();console.log('PASS manual skill revisions: current project, immutable edits, explicit activation, revision conflicts, withdrawal/restore and independent toggle');
})().catch(e=>{console.error(e);process.exitCode=1});
