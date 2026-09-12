const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.argv[2]||process.env.DSH_REACT_TEST_MODULES;const {JSDOM}=require(path.join(modules,'jsdom'));
const dom=new JSDOM('<main id="root"></main>',{pretendToBeVisual:true,url:'http://127.0.0.1/'});Object.assign(global,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true});
const React=require(path.join(modules,'react')),jsx=require(path.join(modules,'react/jsx-runtime')),Client=require(path.join(modules,'react-dom/client')),h=React.createElement;
const source=fs.readFileSync(path.join(__dirname,'../../web/src/runtime-plugins/ui-conversation.js'),'utf8');
const emptyDecorations={token:null,chips:[],textRefs:[],hint:null};
const context={ContextMeter:()=>null,react:React,react_jsx_runtime:jsx,window,document,Text:window.Text,getComputedStyle:window.getComputedStyle.bind(window),setTimeout,clearTimeout,setInterval,clearInterval,requestAnimationFrame:fn=>setTimeout(fn,0),cancelAnimationFrame:clearTimeout,ResizeObserver:class{observe(){}disconnect(){}},clsx:(...parts)=>parts.filter(Boolean).join(' '),InputBar_module_css_default:new Proxy({},{get:(_,key)=>key}),INERT_DECORATIONS:emptyDecorations,deriveDecorations:()=>emptyDecorations,attachmentRailLabels:()=>({}),imageSizeText:String,
 _deepseek_ai_dsh_client_ui_primitives:new Proxy({Tooltip:({children})=>children,Toast:({text})=>h("div",{role:"alert"},text)},{get:(object,key)=>object[key]||(()=>null)}),
 _deepseek_ai_dsh_client_ui_attachment:{AttachmentRail:({items,onRemove})=>h('div',null,items.map(item=>h('button',{key:item.id,'aria-label':item.removeLabel,onClick:()=>onRemove(item)},item.alt)))}};
vm.runInNewContext(source.slice(source.indexOf('function isRasterFile('),source.indexOf('function imageMediaType(')),context);
const begin=source.indexOf('function InputBar('),end=source.indexOf('\n\t\t//#endregion',begin);vm.runInNewContext(source.slice(begin,end),context);

let active='a', counter=0; const states=new Map(['a','b'].map(id=>[id,{draft:'',imageIds:[],phase:'editing',queue:[],claim:null}]));
const drafts=new Map(), submitted=[], failures=[];
const root=Client.createRoot(document.getElementById('root')), act=fn=>React.act(async()=>{await fn();});
const render=()=>root.render(h(context.InputBar,{key:active,sessionId:active,
 useSession:select=>select({running:false,removed:false}),useInput:select=>select(states.get(active)),
 inputActions:{pruneImages(){},submit(){submitted.push({...states.get(active),sessionId:active});}},
 keyboard:{snapshot:{},track(){},setDraft(){}},
 addImages:files=>{const state=states.get(active);for(const file of files){const id=String(++counter);drafts.set(id,{id,kind:context.isRasterFile(file)?'image':'file',file,previewUrl:'blob:test'});state.imageIds.push(id);}render();return null;},
 draftImages:ids=>ids.map(id=>drafts.get(id)),removeImage:id=>{states.get(active).imageIds=states.get(active).imageIds.filter(value=>value!==id);drafts.delete(id);render();},
 useNotices:select=>select(null),useLexicon:select=>select({}),useMenuLauncher:select=>select(null),
 useProjection:(name,select)=>select?select(undefined):undefined,t:(key,args)=>args?.name?`${key}:${args.name}`:key,renderSlot:()=>null}));
async function choose(files){const input=document.querySelector('[data-file-picker]');Object.defineProperty(input,'files',{configurable:true,value:files});await act(()=>input.dispatchEvent(new window.Event('change',{bubbles:true})));}
(async()=>{
 await act(render);assert.ok(document.querySelector('button[aria-label="file.upload"]'));
 await choose([new window.File(['hello'],'设计.txt',{type:'text/plain'}),new window.File(['%PDF'],'report.pdf',{type:'application/pdf'})]);
 assert.equal(document.querySelectorAll('[data-file-draft]').length,2);assert.equal(document.querySelectorAll('[data-file-draft] img, [data-file-draft] iframe').length,0);
 assert.equal(document.querySelector('button[aria-label="input.send"]').disabled,false,'file-only prompts are sendable');
 await act(()=>document.querySelector('button[aria-label="input.send"]').click());assert.equal(submitted[0].imageIds.length,2);
 await act(()=>document.querySelector('button[aria-label="file.remove:设计.txt"]').click());assert.equal(states.get('a').imageIds.length,1);
 const oversized=new window.File(['x'],'large.bin');Object.defineProperty(oversized,'size',{value:17*1024*1024});await choose([oversized]);
 assert.equal(states.get('a').imageIds.length,1,'size rejection preserves existing drafts');assert.match(document.body.textContent,/file.limits/);
 active='b';await act(render);assert.equal(document.querySelectorAll('[data-file-draft]').length,0);active='a';await act(render);assert.equal(document.querySelectorAll('[data-file-draft]').length,1);
 states.get('a').phase='submitting';await act(render);assert.equal(document.querySelector('button[aria-label="file.upload"]').disabled,true);
 await choose([new window.File(['x'],'late.txt')]);assert.equal(states.get('a').imageIds.length,1,'late chooser completion cannot modify a submitting draft');
 await act(()=>root.unmount());dom.window.close();console.log('PASS actual file composer: picker, binary labels, file-only send, remove, limits, session isolation and submission lock');
})().catch(error=>{console.error(error);process.exitCode=1;dom.window.close();});
