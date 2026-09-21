window.__ModuleLoader__.load({id:"dsh-voice-input",factory:require=>{
 const React=require("react"),h=React.createElement;
 function VoiceInputButton(props){
  const current=React.useRef(props);current.current=props;
  const active=React.useRef(null),held=React.useRef(false),generation=React.useRef(0);
  const [listening,setListening]=React.useState(false),[error,setError]=React.useState("");
  const Recognition=window.SpeechRecognition??window.webkitSpeechRecognition;
  const disabled=!Recognition||!props.inputActions||props.locked||["adjudicating","submitting"].includes(props.input?.phase);
  const stop=()=>{held.current=false;const state=active.current;if(state&&!state.stopping){state.stopping=true;try{state.engine.stop()}catch{state.engine.abort()}}};
  React.useEffect(()=>{setListening(false);setError("");return()=>{generation.current++;held.current=false;const state=active.current;active.current=null;if(state){state.engine.onresult=state.engine.onerror=state.engine.onend=null;state.engine.abort()}}},[props.sessionId]);
  React.useEffect(()=>{if(disabled)stop()},[disabled]);
  const begin=()=>{
   if(disabled||active.current)return;
   setError("");held.current=true;const token=++generation.current;
   const launch=()=>{
    if(!held.current||generation.current!==token)return;
    const engine=new Recognition(),base=current.current.input?.draft??"";
    const state={engine,base,written:base,failed:false};active.current=state;
    engine.lang=navigator.language||"zh-CN";engine.interimResults=true;engine.continuous=true;
    engine.onresult=event=>{
     if(active.current!==state||generation.current!==token)return;
     const latest=current.current.input?.draft??"";
     if(latest!==state.written&&latest!==state.base){stop();return}
     let spoken="";for(let i=0;i<event.results.length;i++)spoken+=event.results[i][0]?.transcript??"";
     const next=state.base+(state.base&&!/\s$/.test(state.base)&&spoken?" ":"")+spoken;
     state.written=next;current.current.inputActions.setDraft(next);
    };
    engine.onerror=event=>{
     if(active.current!==state)return;state.failed=true;held.current=false;
     const messages={"not-allowed":"麦克风权限被拒绝，请在浏览器中允许麦克风。","audio-capture":"没有可用的麦克风。","network":"语音识别服务连接失败，请检查网络。","no-speech":"未检测到语音，请重试。","service-not-allowed":"浏览器的语音识别服务不可用。"};
     if(event.error!=="aborted")setError(messages[event.error]||`语音识别失败：${event.error}`);
    };
    engine.onend=()=>{
     if(active.current!==state||generation.current!==token)return;active.current=null;setListening(false);
     if(held.current&&!state.failed)launch();
    };
    try{setListening(true);engine.start()}catch(e){active.current=null;held.current=false;setListening(false);setError(`无法启动语音识别：${e.message}`)}
   };launch();
  };
  const label=!Recognition?"当前浏览器不支持语音识别":listening?"松开结束，识别文字实时写入":"按住说话；空格键也可按住输入";
  return h("span",{style:{display:"inline-flex",alignItems:"center",gap:4}},h("button",{
   type:"button",className:"dsh-voice-input-button",title:error||label,"aria-label":label,"aria-pressed":listening,disabled,
   onPointerDown:e=>{if(e.button!==0)return;e.preventDefault();e.currentTarget.setPointerCapture?.(e.pointerId);begin()},
   onPointerUp:stop,onPointerCancel:stop,onLostPointerCapture:stop,
   onKeyDown:e=>{if([" ","Enter"].includes(e.key)&&!e.repeat){e.preventDefault();begin()}if(e.key==="Escape")stop()},
   onKeyUp:e=>{if([" ","Enter"].includes(e.key)){e.preventDefault();stop()}},onBlur:stop,
   onClick:e=>{if(e.detail===0){active.current?stop():begin()}},
   style:{width:28,height:28,display:"inline-flex",alignItems:"center",justifyContent:"center",border:0,borderRadius:8,touchAction:"none",background: listening?"rgba(217,45,32,.12)":"var(--dsw-alias-interactive-bg-hover,rgba(127,127,137,.10))",color:listening?"#d92d20":"inherit",cursor:disabled?"not-allowed":"pointer",opacity:disabled?.45:1}
  },h("svg",{width:16,height:16,viewBox:"0 0 24 24",fill:"none",stroke:"currentColor",strokeWidth:1.8,"aria-hidden":true},h("rect",{x:8,y:3,width:8,height:12,rx:4}),h("path",{d:"M5 11a7 7 0 0 0 14 0M12 18v3M9 21h6"}))),error&&h("span",{role:"alert",style:{maxWidth:260,fontSize:12,color:"var(--dsw-alias-state-error-primary,#d92d20)"}},error));
 }
 function apply(ctx){ctx.slots.inject("conversation.input.right",()=>ctx.slots.register({name:"conversation.input.right",id:"voice-input",order:100,label:"语音输入"},props=>h(VoiceInputButton,props)))}
 return {apply,inject:["slots"],VoiceInputButton};
}});
