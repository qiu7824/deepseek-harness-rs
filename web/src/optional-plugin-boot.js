/** External client plugins do not own the core workspace's readiness gate. */
export function optionalClientPluginIds(manifest) {
    const entries=Array.isArray(manifest?.entries)?manifest.entries:[];
    const required=new Set(entries.filter(entry=>entry.manageable!==true).map(entry=>entry.id));
    return new Set(entries.filter(entry=>entry.manageable===true&&!required.has(entry.id)).map(entry=>entry.id));
}

function recordFailure(id,error) {
    const message=(error instanceof Error?error.message:String(error)).slice(0,2048);
    const failures=Array.isArray(globalThis.__DSH_PLUGIN_FAILURES__)?globalThis.__DSH_PLUGIN_FAILURES__:[];
    globalThis.__DSH_PLUGIN_FAILURES__=[...failures.filter(failure=>failure.id!==id),{id,message}].slice(-128);
    console.error(`Optional client plugin ${id} failed:`,error);
}

export async function bootPluginEntries(loader,ids,status,optional,assertRequired) {
    globalThis.__DSH_PLUGIN_FAILURES__=[];
    const load=async id=>{
        status.set(id,"loading");
        const key=await loader.create({name:id});
        const entry=loader.resolve(key);
        if(!entry?.fiber)throw new Error(`${id}: plugin import failed`);
        if(typeof entry._await==="function")await entry._await();
    };
    // Required plugins retain their strict startup contract. Optional factories
    // start afterwards, so even one that never settles cannot hide the workspace.
    await Promise.all(ids.filter(id=>!optional.has(id)).map(load));
    await loader.await();
    assertRequired();
    for(const id of ids.filter(id=>optional.has(id))) {
        void load(id).catch(error=>{status.set(id,"failed");recordFailure(id,error);});
    }
}
