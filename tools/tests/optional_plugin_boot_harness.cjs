const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path');
const source=fs.readFileSync(path.join(__dirname,'../../web/src/optional-plugin-boot.js'),'utf8');
(async()=>{
    const {bootPluginEntries,optionalClientPluginIds}=await import('data:text/javascript;base64,'+Buffer.from(source).toString('base64'));
    const optional=optionalClientPluginIds({entries:[{id:'core'},{id:'failed',manageable:true},{id:'never',manageable:true},{id:'healthy',manageable:true},{id:'core',manageable:true}]});
    assert.ok(!optional.has('core'));
    const states=new Map(),created=[],entries=new Map();let validated=false;
    const loader={create:async({name})=>{created.push(name);if(name==='failed')throw Error('broken optional bundle');if(name==='never')await new Promise(()=>{});entries.set(name,{fiber:{state:'active'},_await:async()=>{}});return name;},resolve:id=>entries.get(id),await:async()=>{assert.deepEqual(created,['core']);}};
    const previous=console.error;console.error=()=>{};
    try {await bootPluginEntries(loader,['core','failed','never','healthy'],states,optional,()=>{validated=true;assert.deepEqual(created,['core']);});await new Promise(resolve=>setImmediate(resolve));}
    finally {console.error=previous;}
    assert.ok(validated);assert.ok(entries.has('healthy'));assert.equal(states.get('failed'),'failed');
    assert.equal(globalThis.__DSH_PLUGIN_FAILURES__[0].id,'failed');
    await assert.rejects(bootPluginEntries({create:async()=>{throw Error('core unavailable');}},['core'],states,new Set(),()=>{}),/core unavailable/);
    console.log('PASS optional plugin boot: failed and stalled plugins do not block core/healthy plugins; core failures stay fatal');
})().catch(error=>{console.error(error);process.exitCode=1});
