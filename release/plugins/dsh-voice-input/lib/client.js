window.__ModuleLoader__.load({id:"dsh-voice-input",factory:require=>{
 const React=require("react"),h=React.createElement;
 let recordingOwner=null;
 const HOLD_MS=350,MOVE_TOLERANCE=10,CANCEL_DISTANCE=60,FINISH_TIMEOUT_MS=3000;
 function VoiceInputButton(props){
  const current=React.useRef(props);current.current=props;
  const root=React.useRef(null),active=React.useRef(null),gesture=React.useRef(null),mounted=React.useRef(true);
  const [phase,setPhase]=React.useState("idle"),[error,setError]=React.useState(""),[cancelling,setCancelling]=React.useState(false);
  const Recognition=window.SpeechRecognition??window.webkitSpeechRecognition;
  const disabled=!Recognition||typeof props.inputActions?.setDraft!=="function"||props.locked||["adjudicating","submitting"].includes(props.input?.phase);
  const clearGesture=()=>{
   const hold=gesture.current;gesture.current=null;if(!hold)return;
   window.clearTimeout(hold.timer);
   if(hold.field.style.touchAction==="none")hold.field.style.touchAction=hold.touchAction;
   try{hold.field.releasePointerCapture?.(hold.pointerId)}catch{}
   if(mounted.current)setCancelling(false);
  };
  const settle=record=>{
   window.clearTimeout(record.deadline);
   if(record.engine)record.engine.onresult=record.engine.onerror=record.engine.onend=null;
   if(active.current===record)active.current=null;
   if(recordingOwner===record)recordingOwner=null;
   if(mounted.current)setPhase("idle");
  };
  const abort=(rollback=false)=>{
   clearGesture();const record=active.current;if(!record)return;
   const draft=current.current.input?.draft;
   if(rollback&&current.current.sessionId===record.owner&&(draft===record.written||draft===record.observedDraft))current.current.inputActions?.setDraft(record.startDraft);
   settle(record);try{record.engine?.abort()}catch{}
  };
  const stop=()=>{
   const record=active.current;if(!record||record.stopping)return;
   record.held=false;record.stopping=true;setPhase("finishing");
   record.deadline=window.setTimeout(()=>{if(active.current!==record)return;abort(false);if(mounted.current)setError("语音识别结束超时，已保留识别文字。");},FINISH_TIMEOUT_MS);
   try{record.engine.stop()}catch{abort(false)}
  };
  const begin=()=>{
   if(disabled||active.current)return false;
   recordingOwner?.cancel();setError("");
   const base=current.current.input?.draft??"";
   const record={owner:current.current.sessionId,startDraft:base,base,written:base,observedDraft:base,held:true,stopping:false,engine:null,deadline:null,cancel:()=>abort(false)};
   active.current=record;recordingOwner=record;
   const launch=()=>{
    if(active.current!==record||!record.held)return;
    let engine;try{engine=new Recognition()}catch(reason){abort(false);setError(`无法启动语音识别：${reason.message}`);return;}
    record.engine=engine;engine.lang=window.navigator?.language||"zh-CN";engine.interimResults=true;engine.continuous=true;
    const currentEngine=()=>active.current===record&&record.engine===engine&&current.current.sessionId===record.owner;
    engine.onresult=event=>{
     if(!currentEngine())return;
     const latest=current.current.input?.draft??"";
     if(latest!==record.written&&latest!==record.observedDraft){abort(false);return;}
     record.observedDraft=latest;
     let spoken="";for(let i=0;i<event.results.length;i++){const text=event.results[i][0]?.transcript;if(typeof text==="string")spoken+=text;}
     const next=record.base+(record.base&&!/\s$/.test(record.base)&&spoken?" ":"")+spoken;
     record.written=next;current.current.inputActions.setDraft(next);
    };
    engine.onerror=event=>{
     if(!currentEngine())return;
     const messages={"not-allowed":"麦克风权限被拒绝，请在浏览器中允许麦克风。","audio-capture":"没有可用的麦克风。","network":"语音识别服务连接失败，请检查网络。","no-speech":"未检测到语音，请重试。","service-not-allowed":"浏览器的语音识别服务不可用。"};
     abort(false);if(event.error!=="aborted")setError(messages[event.error]||`语音识别失败：${event.error}`);
    };
    engine.onend=()=>{
     if(!currentEngine())return;
     engine.onresult=engine.onerror=engine.onend=null;
     if(record.held&&!record.stopping){record.base=record.written;launch();}else settle(record);
    };
    try{setPhase("listening");engine.start()}catch(reason){abort(false);setError(`无法启动语音识别：${reason.message}`);}
   };
   launch();return active.current===record;
  };
  const commands=React.useRef(null);commands.current={begin,stop,abort};
  React.useEffect(()=>{
   mounted.current=true;setPhase("idle");setError("");
   const card=root.current?.closest("[data-composer-card]"),doc=root.current?.ownerDocument||window.document;
   const pointerId=event=>event.pointerId??0;
   const down=event=>{
    const field=event.target?.closest?.("textarea[data-composer-input]");
    if(!field||field.closest("[data-composer-card]")!==card||field.dataset.sessionId!==current.current.sessionId||disabled||field.disabled||field.readOnly||event.button!==0||event.isPrimary===false||event.detail>1||event.shiftKey||event.ctrlKey||event.altKey||event.metaKey||active.current||field.selectionStart!==field.selectionEnd)return;
    clearGesture();const hold={field,pointerId:pointerId(event),x:event.clientX,y:event.clientY,touchAction:field.style.touchAction||"",started:false,cancel:false,timer:null};gesture.current=hold;
    hold.timer=window.setTimeout(()=>{
     if(gesture.current!==hold||!field.isConnected||field.selectionStart!==field.selectionEnd){clearGesture();return;}
     if(!commands.current.begin()){clearGesture();return;}
     hold.started=true;field.style.touchAction="none";try{field.setPointerCapture?.(hold.pointerId)}catch{}
    },HOLD_MS);
   };
   const move=event=>{
    const hold=gesture.current;if(!hold||pointerId(event)!==hold.pointerId)return;
    if(!hold.started){if(Math.hypot(event.clientX-hold.x,event.clientY-hold.y)>MOVE_TOLERANCE)clearGesture();return;}
    hold.cancel=hold.y-event.clientY>=CANCEL_DISTANCE;setCancelling(hold.cancel);event.preventDefault();
   };
   const up=event=>{
    const hold=gesture.current;if(!hold||pointerId(event)!==hold.pointerId)return;
    const cancel=hold.cancel,started=hold.started;clearGesture();if(started)cancel?commands.current.abort(true):commands.current.stop();
   };
   const cancelPointer=event=>{if(gesture.current&&pointerId(event)===gesture.current.pointerId)commands.current.abort(true);};
   const contextMenu=event=>{if(gesture.current?.started)event.preventDefault();else clearGesture();};
   const leave=()=>{if(gesture.current&&!gesture.current.started)clearGesture();};
   const escape=event=>{if(event.key==="Escape"&&(active.current||gesture.current)){event.preventDefault();event.stopPropagation();commands.current.abort(true);}else if(gesture.current&&!gesture.current.started)clearGesture();};
   const userInput=event=>{if((active.current||gesture.current)&&event.target?.matches?.("textarea[data-composer-input]")&&event.target.closest("[data-composer-card]")===card)commands.current.abort(false);};
   const blur=()=>commands.current.abort(false),visibility=()=>{if(doc.hidden)commands.current.abort(false);};
   card?.addEventListener("pointerdown",down);card?.addEventListener("pointerleave",leave);card?.addEventListener("contextmenu",contextMenu);
   doc?.addEventListener("pointermove",move,{passive:false});doc?.addEventListener("pointerup",up);doc?.addEventListener("pointercancel",cancelPointer);doc?.addEventListener("lostpointercapture",cancelPointer);doc?.addEventListener("keydown",escape,true);doc?.addEventListener("input",userInput,true);doc?.addEventListener("visibilitychange",visibility);window.addEventListener("blur",blur);
   return()=>{
    mounted.current=false;card?.removeEventListener("pointerdown",down);card?.removeEventListener("pointerleave",leave);card?.removeEventListener("contextmenu",contextMenu);
    doc?.removeEventListener("pointermove",move);doc?.removeEventListener("pointerup",up);doc?.removeEventListener("pointercancel",cancelPointer);doc?.removeEventListener("lostpointercapture",cancelPointer);doc?.removeEventListener("keydown",escape,true);doc?.removeEventListener("input",userInput,true);doc?.removeEventListener("visibilitychange",visibility);window.removeEventListener("blur",blur);commands.current.abort(false);
   };
  },[props.sessionId,disabled]);
  const label=!Recognition?"当前浏览器不支持语音识别":phase==="finishing"?"正在完成语音识别…":phase==="listening"?(cancelling?"松开取消语音输入":"松开结束，上移或 Esc 取消；文字实时写入"):"按住输入框或语音按钮说话；按钮聚焦时也可按住空格输入";
  const messageStyle={position:"absolute",right:0,bottom:"calc(100% + 8px)",width:"max-content",maxWidth:"min(260px,calc(100vw - 40px))",padding:"6px 10px",fontSize:12,lineHeight:1.5,border:"1px solid var(--dsw-alias-border-l2)",borderRadius:8,background:"var(--dsw-alias-bg-base)",color:"var(--dsw-alias-label-primary)",boxShadow:"var(--dsw-shadow-lv2)",zIndex:5};
  return h("span",{ref:root,"data-voice-phase":phase,style:{position:"relative",display:"inline-flex",alignItems:"center",gap:4}},h("button",{
   type:"button",className:"dsh-voice-input-button",title:error||label,"aria-label":label,"aria-pressed":phase==="listening",disabled:disabled||phase==="finishing",
   onPointerDown:event=>{if(event.button!==0)return;event.preventDefault();try{event.currentTarget.setPointerCapture?.(event.pointerId)}catch{}begin();},
   onPointerUp:stop,onPointerCancel:()=>abort(true),onLostPointerCapture:stop,
   onKeyDown:event=>{if([" ","Enter"].includes(event.key)&&!event.repeat){event.preventDefault();begin();}if(event.key==="Escape")abort(true);},
   onKeyUp:event=>{if([" ","Enter"].includes(event.key)){event.preventDefault();stop();}},onBlur:stop,
   onClick:event=>{if(event.detail===0){active.current?stop():begin();}},
   style:{width:28,height:28,display:"inline-flex",alignItems:"center",justifyContent:"center",border:0,borderRadius:8,touchAction:"none",background:phase==="listening"?"rgba(217,45,32,.12)":"var(--dsw-alias-interactive-bg-hover,rgba(127,127,137,.10))",color:phase==="listening"?"var(--dsw-alias-state-error-primary,#d92d20)":"inherit",cursor:disabled?"not-allowed":"pointer",opacity:disabled?.45:1}
  },h("svg",{width:16,height:16,viewBox:"0 0 24 24",fill:"none",stroke:"currentColor",strokeWidth:1.8,"aria-hidden":true},h("rect",{x:8,y:3,width:8,height:12,rx:4}),h("path",{d:"M5 11a7 7 0 0 0 14 0M12 18v3M9 21h6"}))),phase!=="idle"&&h("span",{role:"status",style:messageStyle},phase==="finishing"?"正在完成语音识别…":cancelling?"松开取消":"松开结束，上移取消"),error&&h("span",{role:"alert",style:{...messageStyle,color:"var(--dsw-alias-state-error-primary,#d92d20)"}},error));
 }
 function apply(ctx){ctx.slots.inject("conversation.input.right",()=>ctx.slots.register({name:"conversation.input.right",id:"voice-input",order:100,label:"语音输入"},props=>h(VoiceInputButton,props)));}
 return {apply,inject:["slots"],VoiceInputButton};
}});
