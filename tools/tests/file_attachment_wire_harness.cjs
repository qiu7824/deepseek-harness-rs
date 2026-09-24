const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
let apiModule;
const scope={window:{__ModuleLoader__:{load:entry=>apiModule=entry.factory(()=>({}))}},console,crypto:require('node:crypto').webcrypto,URL,URLSearchParams,AbortController,AbortSignal,Response,TextEncoder,TextDecoder,queueMicrotask,setTimeout,clearTimeout,structuredClone};
vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../../web/src/runtime-plugins/connection.js'),'utf8'),scope);
const reference={attachmentId:'sha256:'+'a'.repeat(64),name:'报告.docx',bytes:18};
class Api extends apiModule.AbstractApiClient {
  async doFetch(url,options){const request=JSON.parse(options.body);assert.equal(url.pathname,'/api/session.fileAttachment');assert.deepEqual(request.payload.attachment,reference);return new Response(JSON.stringify({type:'server-response',rpcId:request.rpcId,result:{ok:true,value:{attachment:reference,path:'D:\\inputs\\报告.docx'}}}),{headers:{'content-type':'application/json'}})}
}
(async()=>{const response=await new Api().sessions.fileAttachment({sessionId:'owner',attachment:reference});assert.equal(response.result.ok,true);assert.equal(response.result.value.attachment.name,'报告.docx');assert.equal(response.result.value.path,'D:\\inputs\\报告.docx');console.log('PASS file attachment wire: method routing, response schema and path/reference preservation')})().catch(error=>{console.error(error);process.exitCode=1});
