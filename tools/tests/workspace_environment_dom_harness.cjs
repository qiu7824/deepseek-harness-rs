const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.argv[2],React=require(path.join(modules,'react')),jsx=require(path.join(modules,'react/jsx-runtime')),{JSDOM}=require(path.join(modules,'jsdom'));
const dom=new JSDOM('<main></main>',{url:'http://localhost/'});Object.assign(globalThis,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true});
const root=require(path.join(modules,'react-dom/client')).createRoot(document.querySelector('main')),h=React.createElement;
const source=fs.readFileSync(path.join(__dirname,'../../web/src/runtime-plugins/ui-workspace.js'),'utf8');
const requests=[],created=[],picked=[],projectSources=new Set();let flow,describeGate=null,failDescribe=false,failSave=false,loseSaveReply=false,revision=7;
let preferences={shellKind:'bash',shellPath:'E:/bin/bash.exe',pythonPath:null,useProjectPython:true,wpsPath:'E:/Office/wps.exe',toolchainPaths:{node:'E:/bin/node.exe',git:'E:/bin/git.exe',ffmpeg:'E:/bin/ffmpeg.exe',rustc:'E:/Rust/rustc.exe',cargo:'E:/Rust/cargo.exe'}};
const copy=value=>JSON.parse(JSON.stringify(value));
const primitive={Menu:({open,items,onSelect})=>open?h('div',null,items.map(item=>h('button',{key:item.id,disabled:item.disabled,onClick:()=>onSelect(item.id)},item.label))):null,Modal:({open,title,children,footer})=>open?h('section',{role:'dialog'},h('h1',null,title),children,footer):null,Button:({children,variant,size,...props})=>h('button',props,children)};
const context={react:React,react_jsx_runtime:jsx,AbortController,crypto:require('node:crypto').webcrypto,setInterval,clearInterval,WorkspacePicker_module_css_default:{},_deepseek_ai_dsh_client_ui_primitives:new Proxy(primitive,{get:(o,k)=>o[k]||(()=>null)}),fetch:async(url,options)=>{
 const args=JSON.parse(options.body);requests.push({url,args});
 if(url==='/__dsh-environment'){
  const described=args.action==='describe'?{version:1,revision,source:projectSources.has(args.cwd)?'project:fixture':'global',workspace:args.cwd,os:'windows',preferences:copy(preferences),effective:{status:'located'}}:null;
  if(args.action==='describe'&&describeGate&&(!describeGate.cwd||describeGate.cwd===args.cwd))await describeGate.promise;
  if(args.action==='describe'&&failDescribe)return {ok:false,status:503,json:async()=>({error:'读取失败'})};
  if(args.action==='save'){
   assert.equal(args.scope,'project');
   if(failSave){failSave=false;return {ok:false,status:400,json:async()=>({error:'配置已被其他窗口修改，请刷新后重试'})};}
   assert.equal(args.expectedRevision,revision);preferences=copy(args.preferences);revision++;projectSources.add(args.cwd);
   if(loseSaveReply){loseSaveReply=false;throw Error('保存回执连接中断');}
  }
  return {ok:true,status:200,json:async()=>described||({version:1,revision,source:projectSources.has(args.cwd)?'project:fixture':'global',workspace:args.cwd,os:'windows',preferences:copy(preferences),effective:{status:'located'}})};
 }
 return {ok:true,status:200,json:async()=>({location:'E:/scratch'})};
}};
vm.runInNewContext(source.slice(source.indexOf('const ADD_WORKSPACE ='),source.indexOf('function WorkspacePicker(')),context);
const props={t:key=>key,open:true,addOnly:true,useWorkspaces:fn=>fn({items:[],phase:'ready'}),useDirectoryFlow:fn=>fn(true),renderDirectoryFlow:owner=>{flow=owner;return null;},createWorkspace:async value=>{created.push(value);return {workspaceId:'workspace-'+created.length}},onPick:id=>picked.push(id),onClose(){}};
const act=fn=>React.act(async()=>{await fn();await new Promise(resolve=>setImmediate(resolve))});
const button=text=>[...document.querySelectorAll('button')].find(node=>node.textContent===text);
async function choose(cwd){await act(()=>button('menu.addWorkspace').click());await act(()=>flow.onPicked(cwd));}
async function change(label,value){const node=document.querySelector(`[aria-label="${label}"]`);assert.ok(node,label);assert.equal(node.disabled,false,label);await act(()=>node[Object.keys(node).find(key=>key.startsWith('__reactProps$'))].onChange({target:{value}}));}
const saves=()=>requests.filter(row=>row.args.action==='save');
(async()=>{
 await act(()=>root.render(h(context.WorkspacePickFlow,props)));
 let release;describeGate={promise:new Promise(resolve=>release=resolve)};
 await choose('E:/work/basic');await act(()=>button('添加').click());assert.equal(saves().length,0,'adding a directory without environment edits must not create a profile override');assert.equal(created.length,1);assert.equal(requests.filter(row=>row.url.includes('workspace-settings')).length,0,'unmodified trash location must be retained');
 describeGate=null;await act(()=>release());
 const original=copy(preferences);await choose('E:/work/configured');await act(()=>button('高级设置').click());
 assert.equal(document.querySelector('[aria-label="Python 环境"]').value,'project');
 await change('Cargo 可执行文件','E:/NewRust/cargo.exe');await act(()=>button('添加').click());
 assert.equal(saves().length,1);assert.equal(preferences.shellKind,original.shellKind);assert.equal(preferences.wpsPath,original.wpsPath);assert.equal(preferences.useProjectPython,true);assert.equal(preferences.toolchainPaths.node,original.toolchainPaths.node);assert.equal(preferences.toolchainPaths.git,original.toolchainPaths.git);assert.equal(preferences.toolchainPaths.ffmpeg,original.toolchainPaths.ffmpeg);assert.equal(preferences.toolchainPaths.cargo,'E:/NewRust/cargo.exe');
 failDescribe=true;await choose('E:/work/read-error');await act(()=>button('高级设置').click());assert.match(document.querySelector('[role=alert]').textContent,/读取失败/);assert.equal(document.querySelector('[aria-label="Cargo 可执行文件"]').disabled,true);
 const before=saves().length;await act(()=>button('添加').click());assert.equal(saves().length,before,'failed reads never become an empty save');failDescribe=false;
 await choose('E:/work/conflict');await act(()=>button('高级设置').click());await change('Cargo 可执行文件','E:/MyCargo/cargo.exe');
 preferences.toolchainPaths.node='E:/Updated/node.exe';preferences.toolchainPaths.cargo='E:/OtherCargo/cargo.exe';revision++;failSave=true;const beforeCreate=created.length;
 await act(()=>button('添加').click());assert.equal(created.length,beforeCreate);assert.match(document.querySelector('[role=alert]').textContent,/其他窗口/);
 if(button('cancel'))await act(()=>button('cancel').click());
 assert.equal(document.querySelector('[aria-label="Cargo 可执行文件"]').value,'E:/MyCargo/cargo.exe');
 await act(()=>button('重新读取并保留输入').click());assert.equal(document.querySelector('[aria-label="Cargo 可执行文件"]').value,'E:/MyCargo/cargo.exe');assert.match(document.body.textContent,/E:\/OtherCargo\/cargo.exe/,'concurrent saved value remains visible for review');
 await act(()=>button('添加').click());assert.equal(preferences.toolchainPaths.node,'E:/Updated/node.exe');assert.equal(preferences.toolchainPaths.cargo,'E:/MyCargo/cargo.exe');assert.equal(created.length,beforeCreate+1);
 await choose('E:/work/python');await act(()=>button('高级设置').click());await change('Python 环境','custom');await change('Python 解释器文件','E:/venv/Scripts/python.exe');await act(()=>button('添加').click());assert.equal(preferences.pythonPath,'E:/venv/Scripts/python.exe');assert.equal(preferences.useProjectPython,false);
 await choose('E:/work/project-python');await act(()=>button('高级设置').click());await change('Python 环境','project');await act(()=>button('添加').click());assert.equal(preferences.pythonPath,null);assert.equal(preferences.useProjectPython,true);
 await choose('E:/work/uncertain');await act(()=>button('高级设置').click());await change('Cargo 可执行文件','E:/Acknowledged/cargo.exe');loseSaveReply=true;const priorCreated=created.length;
 await act(()=>button('添加').click());assert.equal(created.length,priorCreated);assert.match(document.querySelector('[role=alert]').textContent,/回执/);await act(()=>button('cancel').click());
 const saveCount=saves().length;await act(()=>button('重新读取并保留输入').click());await act(()=>button('添加').click());assert.equal(saves().length,saveCount,'a committed save with a lost response is recognized after re-reading instead of written again');assert.equal(created.length,priorCreated+1);
 await choose('E:/work/global-match');await act(()=>button('高级设置').click());await change('Cargo 可执行文件','E:/Pinned/cargo.exe');preferences.toolchainPaths.cargo='E:/Pinned/cargo.exe';revision++;failSave=true;
 await act(()=>button('添加').click());await act(()=>button('cancel').click());const beforePin=saves().length;await act(()=>button('重新读取并保留输入').click());await act(()=>button('添加').click());assert.equal(saves().length,beforePin+1,'matching global values cannot acknowledge a requested project save');assert.equal(projectSources.has('E:/work/global-match'),true);
 const originalCargo=preferences.toolchainPaths.cargo;await choose('E:/work/undo-uncertain');await act(()=>button('高级设置').click());await change('Cargo 可执行文件','E:/Uncertain/cargo.exe');loseSaveReply=true;
 await act(()=>button('添加').click());await act(()=>button('cancel').click());await change('Cargo 可执行文件',originalCargo);assert.equal(button('添加').disabled,true,'undoing input cannot bypass reconciliation after an uncertain save');
 await act(()=>button('重新读取并保留输入').click());assert.equal(document.querySelector('[aria-label="Cargo 可执行文件"]').value,originalCargo,'explicitly edited values survive even when they matched the old baseline');await act(()=>button('添加').click());assert.equal(preferences.toolchainPaths.cargo,originalCargo);
 let releaseStale;describeGate={cwd:'E:/work/stale-a',promise:new Promise(resolve=>releaseStale=resolve)};
 await choose('E:/work/stale-a');await act(()=>button('cancel').click());preferences.toolchainPaths.cargo='E:/NewWorkspace/cargo.exe';revision++;
 await choose('E:/work/stale-b');await act(()=>button('高级设置').click());assert.equal(document.querySelector('[aria-label="Cargo 可执行文件"]').value,'E:/NewWorkspace/cargo.exe');
 describeGate=null;await act(()=>releaseStale());assert.equal(document.querySelector('[aria-label="Cargo 可执行文件"]').value,'E:/NewWorkspace/cargo.exe','late configuration from an abandoned directory cannot replace the new workspace draft');
 await act(()=>root.unmount());dom.window.close();console.log('PASS workspace environment: no implicit writes, load failure protection, complete preference preservation, CAS/rebase review and explicit Python modes');
})().catch(error=>{console.error(error);process.exitCode=1;dom.window.close()});
