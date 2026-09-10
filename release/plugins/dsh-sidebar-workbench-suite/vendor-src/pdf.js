import { getDocument, PDFWorker } from "pdfjs-dist";
import { assets, workerSource } from "dsh-pdf-assets";

class BinaryDataFactory {
  async fetch({kind,filename}) {
    const encoded=Object.hasOwn(assets[kind]||{},filename)?assets[kind][filename]:undefined;
    if(encoded===undefined)throw new Error(`PDF resource is not bundled: ${kind}/${filename}`);
    return Uint8Array.from(atob(encoded),character=>character.charCodeAt(0));
  }
}

function open(data,signal) {
  let worker,bridge,loading,url,timer,disposed=false,disposePromise;
  let rejectFailure;
  const failed=new Promise((_,reject)=>{rejectFailure=reject});
  const abortError=()=>Object.assign(new Error("PDF loading cancelled"),{name:"AbortError"});
  const onAbort=()=>{rejectFailure(abortError());void dispose()};
  const dispose=()=>disposePromise||=(async()=>{
    disposed=true;clearTimeout(timer);signal?.removeEventListener("abort",onAbort);
    try{await loading?.destroy()}catch{}finally{try{bridge?.destroy()}catch{}worker?.terminate();if(url)URL.revokeObjectURL(url);}
  })();
  const initialize=async()=>{
    if(signal?.aborted)throw abortError();
    signal?.addEventListener("abort",onAbort,{once:true});
    url=URL.createObjectURL(new Blob([workerSource,'\nself.postMessage({type:"dsh-pdf-ready"});'],{type:"text/javascript"}));
    worker=new Worker(url,{type:"module",name:"dsh-document-pdf"});
    worker.addEventListener("error",()=>rejectFailure(new Error("PDF worker failed to start")));
    worker.addEventListener("messageerror",()=>rejectFailure(new Error("PDF worker response could not be decoded")));
    timer=setTimeout(()=>rejectFailure(new Error("PDF worker startup timed out")),15000);
    await Promise.race([new Promise(resolve=>{const ready=event=>{if(event.data?.type==="dsh-pdf-ready"){worker.removeEventListener("message",ready);resolve()}};worker.addEventListener("message",ready)}),failed]);
    clearTimeout(timer);if(disposed||signal?.aborted)throw abortError();
    bridge=PDFWorker.create({port:worker});
    loading=getDocument({data:data.slice(),worker:bridge,BinaryDataFactory,cMapPacked:true,useWorkerFetch:false,enableXfa:false,stopAtErrors:true,isEvalSupported:false});
    return loading.promise;
  };
  const document=Promise.race([initialize(),failed]).catch(async error=>{await dispose();throw error});
  return {document,dispose};
}
globalThis.__DSH_SIDEBAR_PDF__=Object.freeze({open});
