const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.argv[2]||process.env.DSH_REACT_TEST_MODULES;
const {JSDOM}=require(path.join(modules,'jsdom'));const dom=new JSDOM('<main></main>',{url:'http://localhost/'});
Object.assign(globalThis,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
const React=require(path.join(modules,'react')),Client=require(path.join(modules,'react-dom/client'));
const source=fs.readFileSync(path.join(__dirname,'../../web/dist/plugins/ui-conversation.js'),'utf8');
const start=source.indexOf('function isImageTool('),end=source.indexOf('/** Subscribe and dispatch',start);assert.ok(start>=0&&end>start);
const observers=[];class Observer{constructor(fn){this.fn=fn;this.dead=false;observers.push(this)}observe(){}disconnect(){this.dead=true}}
const submitted=[];let reject=true,followed=0,live=0;
const context={react:React,IntersectionObserver:Observer,lightboxLabels:()=>({}),fetch:async(_url,options)=>{const body=JSON.parse(options.body);submitted.push(body);return{ok:true,json:async()=>({rpcId:body.rpcId,result:reject?{ok:false,error:{message:'fixture save failure'}}:{ok:true,value:{accepted:true}}})}},
_deepseek_ai_dsh_client_ui_attachment:{ImageLightbox:({src,onClose})=>React.createElement('dialog',{open:true},React.createElement('img',{src}),React.createElement('button',{onClick:onClose},'close-preview'))}};
vm.runInNewContext(source.slice(start,end)+';this.Card=GeneratedImageCard;this.Tool=GeneratedImageTool;this.isImageTool=isImageTool;',context);
const root=Client.createRoot(document.querySelector('main'));const t=key=>key;
const loadImage=()=>{throw Error('lease expected')};loadImage.acquire=()=>{live++;let released=false;const release=()=>{if(!released){released=true;live--}};const pending=Promise.resolve({url:'blob:fixture-image',release});pending.release=release;return pending};
const button=label=>[...document.querySelectorAll('button')].find(node=>node.textContent===label||node.getAttribute('aria-label')===label);
async function act(fn){await React.act(async()=>{await fn();await new Promise(resolve=>setImmediate(resolve))})}
(async()=>{
 assert.equal(context.isImageTool({kind:'tool-call',data:{root:{name:'generate_image'}}}),true);
 await act(()=>root.render(React.createElement(context.Tool,{root:{callId:'live-image',name:'generate_image',argsRaw:JSON.stringify({prompt:'city scene'})},t,loadImage})));
 assert.match(document.querySelector('[role=status]').textContent,/image.running/);assert.ok(document.querySelector('svg'));assert.equal(document.querySelectorAll('img').length,0,'running state never invents a generated image');
 await act(()=>root.render(React.createElement(context.Tool,{root:{callId:'failed-image',kind:'tool-result',isError:true,call:{name:'generate_image'},content:[{type:'text',text:'Provider unavailable'}]},t,loadImage})));
 assert.match(document.querySelector('[role=alert]').textContent,/Provider unavailable/);
 await act(()=>root.render(React.createElement(context.Card,{attachment:{attachmentId:'sha256:fixture',name:'generated-fixture.png',width:64,height:64},loadImage,sessionId:'session-a',onEditSubmitted:()=>{followed++},t})));
 assert.equal(live,0,'offscreen image does not decode');
 await act(()=>observers[0].fn([{isIntersecting:true}]));assert.equal(live,1);
 assert.equal(document.querySelector('a').getAttribute('download'),'generated-fixture.png');
 await act(()=>button('image.previewGenerated').click());assert.ok(document.querySelector('dialog'));
 await act(()=>observers[0].fn([{isIntersecting:false}]));assert.equal(live,1,'open preview keeps image alive');
 await act(()=>button('close-preview').click());assert.equal(live,0,'offscreen closed preview releases bytes');
 await act(()=>observers[0].fn([{isIntersecting:true}]));assert.equal(live,1);
 await act(()=>button('image.editGenerated').click());let area=document.querySelector('textarea');
 await act(()=>{Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype,'value').set.call(area,'make it blue');area.dispatchEvent(new window.Event('input',{bubbles:true}))});
 await act(()=>button('image.submitEdit').click());assert.match(document.querySelector('[role=alert]').textContent,/fixture save failure/);assert.equal(area.value,'make it blue');assert.equal(followed,0);
 reject=false;await act(()=>button('image.submitEdit').click());assert.equal(submitted.length,2);assert.equal(submitted[0].payload.requestId,submitted[1].payload.requestId,'uncertain retry keeps request identity');
 assert.equal(submitted[1].payload.sessionId,'session-a');assert.match(submitted[1].payload.content[0].text,/generated-fixture.png/);assert.equal(followed,1,'an edit returns the conversation to live output');assert.equal(document.querySelector('textarea'),null);
 await act(()=>root.unmount());assert.equal(live,0);assert.ok(observers.every(observer=>observer.dead));dom.window.close();console.log('PASS generated images: lazy leases, preview, download, same-session edits, retry deduplication, live return and cleanup');
})().catch(error=>{console.error(error);process.exitCode=1;dom.window.close()});
