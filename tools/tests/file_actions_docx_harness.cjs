const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const {JSDOM}=require(path.join(process.argv[2]||process.env.DSH_REACT_TEST_MODULES,'jsdom'));
const dom=new JSDOM('<button id="origin">Open</button>',{url:'http://localhost/',pretendToBeVisual:true});
let plugin,opens=0,disposals=0,prints=0,oversized=false,hold=false,release,revoked=[];
const frames=[];
const scope={window:dom.window,document:dom.window.document,navigator:dom.window.navigator,innerWidth:900,innerHeight:700,
  AbortController,DOMException,URL:class extends URL {static createObjectURL(){return 'blob:test-pdf'}static revokeObjectURL(url){revoked.push(url)}},URLSearchParams,Uint8Array,
  setTimeout,clearTimeout,queueMicrotask,console,
  __DSH_SIDEBAR_DOCX__:{open(bytes,frame,signal,options){opens++;assert.equal(frame.getAttribute('sandbox'),'allow-same-origin');assert.equal(bytes.length,3);const view={numPages:3,setZoom(value){view.zoom=value},goToPage(page){view.page=page;options.onPageChange(page)}};frames.push(view);return{ready:Promise.resolve(view),dispose(){disposals++}}}},
  async fetch(url,options={}){
    const parsed=new URL(String(url),'http://localhost');
    if(parsed.pathname.endsWith('file-resolve'))return new Response(JSON.stringify({path:'input.docx',absolutePath:'D:\\inputs\\input.docx',kind:'file',size:3,readOnly:true}),{headers:{'content-type':'application/json'}});
    if(parsed.pathname.endsWith('/office')){prints++;return new Response(new Uint8Array([37,80,68,70]),{headers:{'content-type':'application/pdf'}})}
    if(hold)await new Promise((resolve,reject)=>{release=resolve;options.signal.addEventListener('abort',()=>reject(new DOMException('Aborted','AbortError')),{once:true})});
    return new Response(new Uint8Array([1,2,3]),{headers:{'content-length':oversized?String(33*1024*1024):'3'}});
  },
};
dom.window.__ModuleLoader__={load:entry=>plugin=entry.factory()};
vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../../web/src/runtime-plugins/ui-file-actions.js'),'utf8'),scope);
plugin.apply();const api=scope.__DSH_FILE_ACTIONS__,options={sessionId:'owner',path:'input.docx'};
const tick=()=>new Promise(resolve=>setImmediate(resolve));
const button=label=>[...dom.window.document.querySelectorAll('button')].find(button=>button.textContent===label);
(async()=>{
  dom.window.document.getElementById('origin').focus();await api.open(options);
  assert.equal(opens,1);assert.equal(prints,0,'DOCX opens in docx-preview without WPS export');assert.equal(frames[0].zoom,'fit');
  button('下一页').click();assert.equal(frames[0].page,2);assert.match(dom.window.document.body.textContent,/2 \/ 3 页/);
  button('WPS 打印版式').click();await tick();await tick();assert.equal(prints,1);assert.equal(disposals,1);assert.equal(dom.window.document.querySelector('iframe').getAttribute('src'),'blob:test-pdf');
  api.close();assert.deepEqual(revoked,['blob:test-pdf']);assert.equal(dom.window.document.activeElement.id,'origin');
  hold=true;const pending=api.open(options);await tick();api.close();release?.();await pending;assert.equal(opens,1,'closing a pending file read cannot create a late renderer');
  hold=false;oversized=true;await api.open(options);assert.match(dom.window.document.body.textContent,/32 MiB/);assert.equal(opens,1);api.close();
  oversized=false;await api.open(options);assert.equal(opens,2);api.close();assert.equal(disposals,2);
  console.log('PASS DOCX attachment dialog: default renderer, pages, print switch, cancellation, size limit and disposal');dom.window.close();
})().catch(error=>{console.error(error);api.close();dom.window.close();process.exitCode=1});
