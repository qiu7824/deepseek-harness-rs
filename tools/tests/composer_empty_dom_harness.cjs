const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const modules=process.argv[2]||process.env.DSH_REACT_TEST_MODULES;const {JSDOM}=require(path.join(modules,'jsdom'));
const dom=new JSDOM('<main id="root"></main>',{pretendToBeVisual:true,url:'http://127.0.0.1/'});Object.assign(global,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true});
const React=require(path.join(modules,'react')),jsx=require(path.join(modules,'react/jsx-runtime')),Client=require(path.join(modules,'react-dom/client')),h=React.createElement;
const source=fs.readFileSync(path.join(__dirname,'../../web/src/runtime-plugins/ui-conversation.js'),'utf8');
const emptyDecorations={token:null,chips:[],textRefs:[],hint:null};
const context={ContextMeter:()=>null,react:React,react_jsx_runtime:jsx,window,document,Text:window.Text,getComputedStyle:window.getComputedStyle.bind(window),setTimeout,clearTimeout,setInterval,clearInterval,requestAnimationFrame:fn=>setTimeout(fn,0),cancelAnimationFrame:clearTimeout,ResizeObserver:class{observe(){}disconnect(){}},clsx:(...parts)=>parts.filter(Boolean).join(' '),InputBar_module_css_default:new Proxy({},{get:(_,key)=>key}),INERT_DECORATIONS:emptyDecorations,deriveDecorations:()=>emptyDecorations,attachmentRailLabels:()=>({}),imageSizeText:String,
 _deepseek_ai_dsh_client_ui_primitives:new Proxy({Tooltip:({children})=>children},{get:(object,key)=>object[key]||(()=>null)}),
 _deepseek_ai_dsh_client_ui_attachment:{AttachmentRail:({items,onRemove})=>h('div',null,items.map(item=>h('button',{key:item.id,'aria-label':item.removeLabel,onClick:()=>onRemove(item)},item.alt)))}};
const begin=source.indexOf('function InputBar('),end=source.indexOf('\n\t\t//#endregion',begin);vm.runInNewContext(source.slice(begin,end),context);
let active='a';const state=new Map(['a','b'].map(id=>[id,{draft:'',imageIds:[],phase:'editing',queue:[],claim:null}])),submissions=[];
const image={id:'image',file:{name:'frame.png'},previewUrl:'data:image/png;base64,fixture'};
const root=Client.createRoot(document.getElementById('root')),act=fn=>React.act(async()=>{await fn();await new Promise(resolve=>setTimeout(resolve,10));});
const render=()=>root.render(h(context.InputBar,{key:active,sessionId:active,useSession:select=>select({running:false,removed:false}),useInput:select=>select(state.get(active)),inputActions:{pruneImages(){},submit(){submissions.push({sessionId:active,...state.get(active)})}},keyboard:{snapshot:{},setDraft(value){state.set(active,{...state.get(active),draft:value});render()},track(){}},draftImages:ids=>ids.map(()=>image),removeImage:()=>{state.set(active,{...state.get(active),imageIds:[]});render()},useNotices:select=>select(null),useLexicon:select=>select({}),useMenuLauncher:select=>select(null),useProjection:(name,select)=>select?select(undefined):undefined,t:key=>key,renderSlot:()=>null}));
const send=()=>document.querySelector('button[aria-label="input.send"]');
(async()=>{
 await act(render);assert.equal(send().disabled,true);assert.equal(document.querySelector('textarea').placeholder,'placeholder.default');
 state.set('a',{...state.get('a'),draft:' \n\t'});await act(render);assert.equal(send().disabled,true);assert.equal(document.querySelector('textarea').value,' \n\t','meaningful whitespace is preserved for editing');
 state.set('a',{...state.get('a'),draft:'text'});await act(render);assert.equal(send().disabled,false);
 state.set('a',{...state.get('a'),draft:''});await act(render);assert.equal(send().disabled,true);assert.equal(document.querySelector('textarea').placeholder,'placeholder.default');
 state.set('a',{...state.get('a'),imageIds:['image']});await act(render);assert.equal(send().disabled,false);assert.equal(document.querySelector('textarea').placeholder,'','attachments hide the empty-composer hint');assert.equal(document.querySelector('textarea').getAttribute('aria-label'),'placeholder.default');
 await act(()=>send().click());assert.equal(submissions.length,1);assert.deepEqual(submissions[0].imageIds,['image']);assert.equal(submissions[0].draft,'');
 active='b';await act(render);assert.equal(send().disabled,true);assert.equal(document.querySelector('textarea').placeholder,'placeholder.default');assert.ok(!document.body.textContent.includes('frame.png'));
 active='a';await act(render);assert.equal(send().disabled,false);await act(()=>document.querySelector('[aria-label="image.remove"]').click());assert.equal(send().disabled,true);assert.equal(document.querySelector('textarea').placeholder,'placeholder.default');
 await act(()=>root.unmount());dom.window.close();console.log('PASS actual InputBar DOM: whitespace, deletion, attachment-only submission, session switching and placeholder restoration');
})().catch(error=>{console.error(error);process.exitCode=1;dom.window.close();});
