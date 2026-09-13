const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.argv[2]||process.env.DSH_REACT_TEST_MODULES;
if(!modules)throw new Error('React test modules are required');
const {JSDOM}=require(path.join(modules,'jsdom'));
const dom=new JSDOM('<main></main>',{url:'http://localhost/'});
Object.assign(globalThis,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
const React=require(path.join(modules,'react')),Client=require(path.join(modules,'react-dom/client'));
const source=fs.readFileSync(path.join(__dirname,'../../web/dist/plugins/ui-settings-models.js'),'utf8');
const start=source.indexOf('function AccountAuthorizationBrowser('),end=source.indexOf('function SidebarAccount(',start);
assert.ok(start>=0&&end>start);
const calls=[],timers=new Map();let timerId=0,releaseCapture=null;
const imageUrls=new Set();let imageId=0;window.URL.createObjectURL=()=>{const url='blob:test-'+(++imageId);imageUrls.add(url);return url};window.URL.revokeObjectURL=url=>imageUrls.delete(url);
const picture={state:{url:'https://example.com/login',viewport:{width:1100,height:760}},screenshot:{mediaType:'image/png',base64:'aW1hZ2U='}};
const context={react:React,ModelsSection_module_css_default:{},document,window,messageOf$1:e=>e.message,
 setInterval:fn=>{const id=++timerId;timers.set(id,fn);return id},clearInterval:id=>timers.delete(id),
 accountRequest:async(route,body)=>{calls.push({route,...body});if(body.action==='capture')await new Promise(resolve=>{releaseCapture=resolve});return JSON.parse(JSON.stringify(picture))}};
vm.runInNewContext(source.slice(start,end)+';this.Prompt=AccountLoginPrompt;',context);
const root=Client.createRoot(document.querySelector('main'));
const t=key=>({accountVerify:'Enter the device code',accountOpen:'Open authorization',cancel:'Cancel'}[key]||key);
(async()=>{
 let cancelled=0;
 await React.act(()=>root.render(React.createElement(context.Prompt,{attempt:{flow:'browser',verificationUri:'https://app.devin.ai/auth/cli/continue?state=fixture'},t,busy:false,onCancel:()=>cancelled++})));
 assert.equal(document.querySelector('code'),null);
 assert.ok(!document.body.textContent.includes('Enter the device code'));
 assert.equal(new URL(document.querySelector('a').href).hostname,'app.devin.ai');
 await React.act(()=>document.querySelector('button').click());assert.equal(cancelled,1);
 await React.act(()=>root.render(React.createElement(context.Prompt,{attempt:{userCode:'ABCD',verificationUri:'https://example.com/device'},t,busy:false,onCancel:()=>{}})));
 assert.equal(document.querySelector('code').textContent,'ABCD');assert.ok(document.body.textContent.includes('Enter the device code'));
 await React.act(()=>root.render(React.createElement(context.Prompt,{attempt:{attempt:'auth-1',flow:'browser',embeddedBrowser:true,verificationUri:'https://example.com/login'},t,busy:false,onCancel:()=>{}})));
 assert.equal(calls[0].action,'start');assert.equal(calls[0].attempt,'auth-1');
 const img=document.querySelector('img');assert.ok(img);img.getBoundingClientRect=()=>({left:0,top:0,width:550,height:380});
 await React.act(()=>img.dispatchEvent(new window.MouseEvent('click',{bubbles:true,clientX:125,clientY:90})));
 assert.equal(calls.at(-1).action,'click');assert.equal(calls.at(-1).x,250);assert.equal(calls.at(-1).y,180);
 await React.act(()=>{timers.values().next().value()});assert.equal(calls.at(-1).action,'capture');
 const enter=[...document.querySelectorAll('button')].find(button=>button.textContent==='Enter');
 await React.act(()=>enter.click());assert.equal(calls.at(-1).action,'capture','foreground input waits for a pending capture');
 await React.act(async()=>{releaseCapture();await Promise.resolve()});assert.equal(calls.at(-1).action,'key');assert.equal(calls.at(-1).key,'Enter');
 assert.equal(document.querySelector('input').type,'password');
 const textInput=document.querySelector('input');
 await React.act(()=>{Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value').set.call(textInput,'fixture-only-input');textInput.dispatchEvent(new window.Event('input',{bubbles:true}))});
 await React.act(()=>[...document.querySelectorAll('button')].find(button=>button.textContent==='accountBrowserType').click());
 assert.equal(calls.at(-1).action,'type');assert.equal(calls.at(-1).text,'fixture-only-input');assert.equal(textInput.value,'','submitted input is cleared');
 for(let n=0;n<20;n++){
  await React.act(()=>{timers.values().next().value()});
  await React.act(async()=>{releaseCapture();await Promise.resolve()});
  assert.equal(imageUrls.size,1,'capture frames replace and revoke their predecessor');
 }
 assert.equal(calls.filter(call=>call.action==='start').length,1,'polling and rendering do not create more browsers');assert.equal(timers.size,1);
 await React.act(()=>root.unmount());dom.window.close();
 assert.equal(timers.size,0,'authorization capture stops on unmount');
 assert.equal(imageUrls.size,0,'authorization images are revoked on unmount');
 console.log('PASS browser OAuth prompt: authorization link, cancellation, no empty device-code instruction, existing device flow preserved');
})().catch(error=>{console.error(error);process.exitCode=1});
