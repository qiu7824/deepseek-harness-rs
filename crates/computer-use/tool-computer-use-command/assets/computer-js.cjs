'use strict';
// Dedicated Computer Use realm. Host actions cross a generation-checked pipe;
// the script has no Node module loader, filesystem, network or process API.
const {Worker,isMainThread,parentPort,workerData}=require('node:worker_threads');
if(isMainThread){
  if(Number(process.versions.node.split('.')[0])<25){process.stderr.write('Computer Use JS requires Node 25+ with network permissions\n');process.exit(69);}
  const readline=require('node:readline');
  let worker=null,active=null,timer=null;
  const emit=value=>process.stdout.write(JSON.stringify(value)+'\n');
  const stop=async()=>{clearTimeout(timer);timer=null;const old=worker;worker=null;if(old)await old.terminate();};
  function start(){
    worker=new Worker(__filename,{workerData:{kernel:true},resourceLimits:{maxOldGenerationSizeMb:128,maxYoungGenerationSizeMb:16,stackSizeMb:4}});
    const selected=worker;
    worker.on('message',message=>{
      if(worker!==selected||!active||message.evalId!==active.id)return;
      if(message.type==='action'){
        if(++active.actions>64){const id=active.id;active=null;void stop();emit({type:'result',evalId:id,ok:false,error:'COMPUTER_USE_ACTION_LIMIT'});return;}
        emit(message);
      }else if(message.type==='result'){
        clearTimeout(timer);timer=null;active=null;emit(message);
      }
    });
    worker.on('error',()=>{if(worker!==selected)return;const id=active?.id;active=null;worker=null;clearTimeout(timer);if(id)emit({type:'result',evalId:id,ok:false,error:'COMPUTER_USE_KERNEL_FAILED'});});
    worker.on('exit',()=>{if(worker!==selected)return;const id=active?.id;active=null;worker=null;clearTimeout(timer);if(id)emit({type:'result',evalId:id,ok:false,error:'COMPUTER_USE_KERNEL_EXITED'});});
  }
  readline.createInterface({input:process.stdin,crlfDelay:Infinity}).on('line',line=>{
    if(Buffer.byteLength(line)>2*1024*1024){process.exit(65);}
    let message;try{message=JSON.parse(line)}catch{process.exit(65);}
    if(message.type==='reset'){
      const id=active?.id;active=null;void stop().then(()=>emit({type:'reset',evalId:message.evalId,ok:true}));
      if(id)emit({type:'result',evalId:id,ok:false,error:'COMPUTER_USE_RESET'});
    }else if(message.type==='eval'){
      if(active){emit({type:'result',evalId:message.evalId,ok:false,error:'COMPUTER_USE_BUSY'});return;}
      if(typeof message.code!=='string'||Buffer.byteLength(message.code)>65536){emit({type:'result',evalId:message.evalId,ok:false,error:'COMPUTER_USE_CODE_LIMIT'});return;}
      if(!worker)start();active={id:message.evalId,actions:0};worker.postMessage(message);
      timer=setTimeout(()=>{if(!active)return;const id=active.id;active=null;void stop();emit({type:'result',evalId:id,ok:false,error:'COMPUTER_USE_JS_TIMEOUT',reset:true});},Math.max(1,Math.min(message.timeoutMs||60000,120000)));
    }else if(message.type==='action_result'&&active?.id===message.evalId){worker?.postMessage(message);}
  }).on('close',()=>{active=null;void stop().then(()=>process.exit(0));});
}else{
  const vm=require('node:vm'),acorn=require('./acorn.cjs');
  const {AsyncLocalStorage}=require('node:async_hooks');const evaluationContext=new AsyncLocalStorage();
  let active=null,sequence=0;const pending=new Map();
  const call=payload=>{
    const lease=evaluationContext.getStore();
    if(!active||!lease?.active||lease.id!==active.id)return Promise.reject(new Error('COMPUTER_USE_INACTIVE_EVALUATION'));
    let arguments_;try{arguments_=JSON.parse(payload)}catch{return Promise.reject(new Error('Invalid action JSON'));}
    const id=++sequence,evalId=active.id;
    let completed;const done=new Promise(resolve=>{completed=resolve;});
    return new Promise((resolve,reject)=>{pending.set(id,{resolve,reject,evalId,done,completed});parentPort.postMessage({type:'action',id,evalId,arguments:arguments_});});
  };
  const output=payload=>{const lease=evaluationContext.getStore();if(!active||!lease?.active||lease.id!==active.id)throw Error('COMPUTER_USE_INACTIVE_EVALUATION');active.bytes+=Buffer.byteLength(payload);if(active.bytes>1024*1024)throw Error('COMPUTER_USE_OUTPUT_LIMIT');active.logs.push(JSON.parse(payload));};
  const sandbox=Object.assign(Object.create(null),{__call:call,__output:output});
  const context=vm.createContext(sandbox,{codeGeneration:{strings:false,wasm:false}});
  vm.runInContext(`((hostCall,hostOutput)=>{'use strict';
    const parse=JSON.parse,stringify=JSON.stringify,SafeError=Error;
    const safeMessage=error=>typeof error?.message==='string'?error.message:'Computer Use bridge failed';
    const perform=async arguments_=>{try{return parse(await hostCall(stringify(arguments_)));}catch(error){throw new SafeError(safeMessage(error));}};
    const emit=payload=>{try{hostOutput(payload);}catch(error){throw new SafeError(safeMessage(error));}};
    const write=value=>emit(stringify({type:'text',value:value===undefined?null:value}));
    const keys=value=>String(value).split('+').map(key=>({ctrl:'Control',Control_L:'Control',Control_R:'Control',alt:'Alt',shift:'Shift',Return:'Enter',Esc:'Escape'})[key]||key);
    let bound=null;
    async function bind(app){
      if(bound===app.windowRef)return;
      if(bound)await perform({action:'close',target:app.target});
      await perform({action:'start',target:app.target,windowId:app.windowId,windowRef:app.windowRef,includeScreenshot:false});bound=app.windowRef;
    }
    function appHandle(info,target){
      let observed=null;
      const act=async args=>{await bind({...info,target});const result=await perform({...args,target});observed=result;return result;};
      const element=async(action,id,extra)=>{if(!observed?.accessibility?.snapshotId)throw Error('Observe the current accessibility state before using an element');const result=await act({action,elementId:id,snapshotId:observed.accessibility.snapshotId,...extra});observed=null;return result;};
      return Object.freeze({appRef:info.appRef,windowRef:info.windowRef,title:info.title,
        getAXState:()=>act({action:'ax_state'}),getAXStateAndScreenshot:()=>act({action:'ax_state'}),getScreenshot:()=>act({action:'capture'}),
        click:target=>Array.isArray(target)?act({action:'click',x:target[0],y:target[1]}):element('invoke',target,{}),
        setValue:(id,text)=>element('set_value',id,{text}),select:(id)=>element('select',id,{}),
        typeText:text=>act({action:'type',text}),pressKey:key=>act({action:'key',keys:keys(key)}),
        scroll:(target,direction,pages=1)=>typeof target==='number'?element('scroll_element',target,{direction}):act({action:'scroll',x:target[0],y:target[1],deltaY:(direction==='up'?-1:1)*Math.min(Math.max(pages,1),10)*480}),
        close:async()=>{const result=await perform({action:'close',target});bound=null;observed=null;return result;}
      });
    }
    globalThis.cua=Object.freeze({
      perform,
      getState:()=>perform({action:'list_windows',target:'local'}),
      listApps:()=>perform({action:'list_apps',target:'local'}),
      launchApp:executable=>perform({action:'launch_app',target:'local',executable}),
      getApp:async name=>{const result=await perform({action:'list_windows',target:'local'});const query=String(name).toLowerCase();const windows=(result.windows||[]).filter(window=>window.windowRef===name||window.appRef===name||String(window.title||'').toLowerCase().includes(query)||String(window.executable||'').toLowerCase().split(/[\\\\/]/).at(-1).replace(/\\.exe$/,'')===query);if(windows.length!==1)throw Error('Select one windowRef from cua.getState(); matching windows: '+windows.length);return appHandle(windows[0],'local');},
      remote:()=>Object.freeze({perform:args=>perform({...args,target:'remote'})}),
      browser:()=>Object.freeze({perform:args=>perform({...args,target:'browser'})})
    });
    globalThis.nodeRepl=Object.freeze({write,emitImage:image=>emit(stringify({type:'image',image}))});
    globalThis.console=Object.freeze({log:(...values)=>write(values)});
  })(__call,__output);delete globalThis.__call;delete globalThis.__output;`,context);
  const boundNames=pattern=>pattern.type==='Identifier'?[pattern.name]:pattern.type==='RestElement'?boundNames(pattern.argument):pattern.type==='AssignmentPattern'?boundNames(pattern.left):pattern.type==='ArrayPattern'?pattern.elements.filter(Boolean).flatMap(boundNames):pattern.type==='ObjectPattern'?pattern.properties.flatMap(property=>boundNames(property.type==='RestElement'?property.argument:property.value)):[];
  function compile(code){
    const tree=acorn.parse(code,{ecmaVersion:'latest',allowAwaitOutsideFunction:true,allowReturnOutsideFunction:true});
    const visit=node=>{if(!node||typeof node!=='object')return;if(node.type==='ImportExpression'||node.type==='ImportDeclaration'||String(node.type).startsWith('Export'))throw Error('Module loading is unavailable in Computer Use JS');for(const value of Object.values(node)){if(Array.isArray(value))value.forEach(visit);else if(value&&typeof value==='object')visit(value);}};visit(tree);
    const edits=[];
    for(const statement of tree.body){
      if(statement.type==='VariableDeclaration'){
        const pieces=[];
        for(const declaration of statement.declarations){
          const names=boundNames(declaration.id);for(const name of names){if(Object.getOwnPropertyDescriptor(context,name)?.writable===false)throw Error('Identifier already declared: '+name);if(!Object.hasOwn(context,name))Object.defineProperty(context,name,{value:undefined,writable:true,configurable:true,enumerable:true});}
          if(declaration.init)pieces.push('('+code.slice(declaration.id.start,declaration.id.end)+' = '+code.slice(declaration.init.start,declaration.init.end)+');');
          if(statement.kind==='const')for(const name of names)pieces.push('Object.defineProperty(globalThis,'+JSON.stringify(name)+',{writable:false});');
        }
        edits.push({start:statement.start,end:statement.end,value:pieces.join('\n')});
      }else if((statement.type==='FunctionDeclaration'||statement.type==='ClassDeclaration')&&statement.id){edits.push({start:statement.start,end:statement.end,value:'globalThis['+JSON.stringify(statement.id.name)+'] = ('+code.slice(statement.start,statement.end)+');'});}
    }
    for(const edit of edits.reverse())code=code.slice(0,edit.start)+edit.value+code.slice(edit.end);
    return '(async()=>{"use strict";\n'+code+'\n})()';
  }
  parentPort.on('message',async message=>{
    if(message.type==='action_result'){
      const entry=pending.get(message.id);if(!entry||entry.evalId!==message.evalId)return;pending.delete(message.id);
      if(message.ok)entry.resolve(JSON.stringify(message.value));else entry.reject(new Error(String(message.error||'Computer Use action failed')));entry.completed();return;
    }
    if(message.type!=='eval'||active)return;
    active={id:message.evalId,logs:[],bytes:0};
    const lease={id:message.evalId,active:true};
    try{
      const code=compile(message.code),result=await evaluationContext.run(lease,()=>new vm.Script(code).runInContext(context));
      // Actions initiated during evaluation settle before its lease closes.
      // Continuations from an older lease remain rejected during later evals.
      while([...pending.values()].some(entry=>entry.evalId===lease.id)){await Promise.all([...pending.values()].filter(entry=>entry.evalId===lease.id).map(entry=>entry.done));await new Promise(setImmediate);}
      const value=JSON.parse(JSON.stringify(result===undefined?null:result));
      if(Buffer.byteLength(JSON.stringify({value,logs:active.logs}))>1024*1024)throw Error('COMPUTER_USE_OUTPUT_LIMIT');
      parentPort.postMessage({type:'result',evalId:active.id,ok:true,value,logs:active.logs});
    }catch(error){parentPort.postMessage({type:'result',evalId:active.id,ok:false,error:String(error?.message||error).slice(0,8192),logs:active.logs});}
    finally{const id=active.id;lease.active=false;active=null;for(const [key,entry]of pending){if(entry.evalId===id){pending.delete(key);entry.completed();entry.reject(new Error('COMPUTER_USE_INACTIVE_EVALUATION'));}}}
  });
}
