window.__ModuleLoader__.load({
  id: "dsh-web-preview-rs",
  factory: (require) => {
    const module = { exports: {} };
    const exports = module.exports;
    const React = require("react");
    const ReactDOM = require("react-dom");
    const inject = ["slots"];

    const ID = "dsh-web-preview-rs";
    const stateBySession = new Map();
    const listeners = new Set();
    const inputBySession = new Map();
    const TEXT_EXTENSIONS = new Set([
      "md", "mdx", "markdown", "txt", "log", "json", "jsonl", "yaml", "yml", "toml",
      "xml", "csv", "tsv", "js", "mjs", "cjs", "jsx", "ts", "tsx", "py", "rs", "go",
      "java", "kt", "kts", "c", "h", "cpp", "hpp", "cs", "php", "rb", "swift", "sh",
      "bash", "zsh", "ps1", "sql", "css", "scss", "sass", "less", "vue", "svelte", "ini",
      "conf", "cfg", "properties", "gitignore", "dockerfile"
    ]);
    const CSS = `
.dwp-launch{width:28px;height:28px;border:0;border-radius:8px;background:transparent;color:var(--dsw-alias-label-secondary,#667085);cursor:pointer;display:grid;place-items:center;font-size:15px}.dwp-launch:hover,.dwp-launch[data-active=true]{background:var(--dsw-alias-interactive-bg-hover,rgba(127,127,137,.12));color:var(--dsw-alias-state-business-primary,#175cd3)}
.dwp-backdrop{position:fixed;inset:0;z-index:1008;background:rgba(16,24,40,.18);pointer-events:auto}.dwp-shell{--dwp-width:560px;position:fixed;z-index:1009;top:8px;right:8px;bottom:8px;width:min(var(--dwp-width),calc(100vw - 32px));min-width:340px;pointer-events:auto;background:color-mix(in srgb,var(--dsw-alias-bg-base,#fff) 94%,transparent);backdrop-filter:blur(20px);border:1px solid var(--dsw-alias-border-l2,rgba(127,127,137,.2));border-radius:14px;box-shadow:-12px 10px 40px rgba(16,24,40,.18);display:flex;flex-direction:column;overflow:hidden;color:var(--dsw-alias-label-primary,#101828)}
.dwp-resize{position:absolute;left:-5px;top:0;bottom:0;width:10px;cursor:col-resize;touch-action:none;z-index:3}.dwp-resize:hover:after{content:"";position:absolute;left:4px;top:44%;height:42px;width:2px;border-radius:2px;background:var(--dsw-alias-state-business-primary,#175cd3)}
.dwp-head{height:50px;flex:none;display:flex;align-items:center;gap:7px;padding:0 11px;border-bottom:1px solid var(--dsw-alias-border-l2,rgba(127,127,137,.18))}.dwp-title{min-width:0;flex:1;font-size:14px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.dwp-icon{border:0;background:transparent;color:var(--dsw-alias-label-secondary,#667085);height:30px;min-width:30px;border-radius:8px;cursor:pointer;font:inherit}.dwp-icon:hover,.dwp-icon[data-active=true]{background:var(--dsw-alias-interactive-bg-hover,rgba(127,127,137,.12));color:var(--dsw-alias-label-primary,#101828)}.dwp-icon:disabled{opacity:.35;cursor:default}
.dwp-project{flex:none;padding:7px 10px;border-bottom:1px solid var(--dsw-alias-border-l2,rgba(127,127,137,.14));background:var(--dsw-alias-bg-layer-1,rgba(127,127,137,.04));font-size:11px}.dwp-project-row{display:flex;align-items:center;gap:7px;min-height:25px}.dwp-project-name{font-weight:600}.dwp-project-state{color:var(--dsw-alias-label-tertiary,#667085)}.dwp-project-state[data-running=true]{color:var(--dsw-alias-state-success-primary,#079455)}.dwp-project-spacer{flex:1}.dwp-small{height:25px;padding:0 9px;border:1px solid var(--dsw-alias-border-l2,rgba(127,127,137,.2));border-radius:7px;background:transparent;color:var(--dsw-alias-label-secondary,#667085);cursor:pointer;font:inherit;font-size:11px}.dwp-small:hover{border-color:var(--dsw-alias-state-business-primary,#175cd3);color:var(--dsw-alias-state-business-primary,#175cd3)}.dwp-small[data-danger=true]{color:var(--dsw-alias-state-error-primary,#d92d20)}.dwp-log{max-height:110px;overflow:auto;margin-top:5px;padding:7px 9px;border-radius:7px;background:var(--dsw-alias-markdown-code-block,rgba(127,127,137,.1));white-space:pre-wrap;overflow-wrap:anywhere;font:10px/1.5 var(--ds-font-family-code,ui-monospace,monospace)}
.dwp-confirm{padding:10px 12px;border-bottom:1px solid color-mix(in srgb,var(--dsw-alias-state-warn-label,#b54708) 35%,transparent);background:color-mix(in srgb,var(--dsw-alias-state-warn-label,#b54708) 8%,transparent);font-size:11px}.dwp-confirm-title{font-weight:600;color:var(--dsw-alias-state-warn-label,#b54708)}.dwp-command{margin:6px 0;padding:7px 9px;border-radius:7px;background:var(--dsw-alias-markdown-code-block,rgba(127,127,137,.1));font:11px/1.5 var(--ds-font-family-code,ui-monospace,monospace);overflow-wrap:anywhere}.dwp-confirm-actions{display:flex;justify-content:flex-end;gap:6px;margin-top:7px}.dwp-primary{background:var(--dsw-alias-state-business-primary,#175cd3)!important;border-color:transparent!important;color:#fff!important}
.dwp-crumbs{height:37px;flex:none;display:flex;align-items:center;gap:3px;padding:0 11px;border-bottom:1px solid var(--dsw-alias-border-l2,rgba(127,127,137,.14));overflow:auto;scrollbar-width:none}.dwp-crumb{border:0;background:transparent;color:var(--dsw-alias-label-tertiary,#667085);cursor:pointer;padding:3px 5px;border-radius:5px;white-space:nowrap;font-size:11px}.dwp-crumb:hover{background:var(--dsw-alias-interactive-bg-hover,rgba(127,127,137,.12));color:var(--dsw-alias-label-primary,#101828)}
.dwp-body{min-height:0;flex:1;display:grid;grid-template-columns:180px minmax(0,1fr)}.dwp-tree{overflow:auto;border-right:1px solid var(--dsw-alias-border-l2,rgba(127,127,137,.18));padding:8px}.dwp-row{width:100%;min-height:29px;display:flex;align-items:center;gap:6px;border:0;border-radius:6px;background:transparent;color:var(--dsw-alias-label-secondary,#475467);cursor:pointer;text-align:left;padding:4px 7px;font-size:11px}.dwp-row:hover,.dwp-row[data-active=true]{background:var(--dsw-alias-interactive-bg-hover,rgba(127,127,137,.12));color:var(--dsw-alias-label-primary,#101828)}.dwp-row-name{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.dwp-row-size{margin-left:auto;color:var(--dsw-alias-label-caption,#98a2b3);font-size:9px}.dwp-view{min-width:0;min-height:0;display:flex;flex-direction:column}.dwp-tabs{min-height:34px;flex:none;display:flex;align-items:center;gap:3px;padding:3px 8px;border-bottom:1px solid var(--dsw-alias-border-l2,rgba(127,127,137,.14));overflow:auto}.dwp-tab{border:0;background:transparent;color:var(--dsw-alias-label-tertiary,#667085);border-radius:6px;padding:4px 8px;cursor:pointer;font-size:10px;white-space:nowrap}.dwp-tab[data-active=true]{background:var(--dsw-alias-interactive-bg-hover,rgba(127,127,137,.12));color:var(--dsw-alias-label-primary,#101828)}
.dwp-content{min-height:0;flex:1;overflow:auto;padding:18px;position:relative}.dwp-content[data-frame=true]{padding:0}.dwp-code{margin:0;font:12px/1.65 var(--ds-font-family-code,ui-monospace,monospace);white-space:pre;tab-size:2;color:var(--dsw-alias-label-primary,#101828)}.dwp-code-line{display:block}.dwp-line-no{display:inline-block;width:44px;margin-right:12px;text-align:right;user-select:none;color:var(--dsw-alias-label-caption,#98a2b3)}.dwp-image{display:block;max-width:100%;max-height:100%;margin:auto;object-fit:contain}.dwp-media{width:100%;max-height:100%}.dwp-frame{width:100%;height:100%;border:0;background:#fff}.dwp-markdown{max-width:820px;margin:0 auto;font-size:14px;line-height:1.75;overflow-wrap:anywhere}.dwp-markdown h1,.dwp-markdown h2,.dwp-markdown h3{line-height:1.3;margin:1.2em 0 .6em}.dwp-markdown h1{font-size:25px;border-bottom:1px solid var(--dsw-alias-border-l2,#ddd);padding-bottom:7px}.dwp-markdown h2{font-size:20px}.dwp-markdown pre{overflow:auto;background:var(--dsw-alias-markdown-code-block,rgba(127,127,137,.1));padding:12px;border-radius:8px}.dwp-markdown code{font-family:var(--ds-font-family-code,ui-monospace,monospace)}.dwp-markdown blockquote{margin:10px 0;padding-left:12px;border-left:3px solid var(--dsw-alias-border-l3,#bbb);color:var(--dsw-alias-label-secondary,#475467)}.dwp-markdown a{color:var(--dsw-alias-state-business-primary,#175cd3)}
.dwp-state{padding:24px;color:var(--dsw-alias-label-tertiary,#667085);font-size:12px}.dwp-error{color:var(--dsw-alias-state-error-primary,#d92d20)}.dwp-drop{position:absolute;inset:12px;z-index:4;border:2px dashed var(--dsw-alias-state-business-primary,#175cd3);border-radius:12px;background:color-mix(in srgb,var(--dsw-alias-bg-base,#fff) 90%,transparent);display:grid;place-items:center;color:var(--dsw-alias-state-business-primary,#175cd3);font-size:14px;font-weight:600}.dwp-status{height:25px;flex:none;display:flex;align-items:center;gap:8px;padding:0 10px;border-top:1px solid var(--dsw-alias-border-l2,rgba(127,127,137,.14));color:var(--dsw-alias-label-caption,#98a2b3);font-size:9px}.dwp-status-spacer{flex:1}
.dwp-annotation{flex:none;padding:8px 10px;border-top:1px solid color-mix(in srgb,var(--dsw-alias-state-business-primary,#175cd3) 30%,transparent);background:color-mix(in srgb,var(--dsw-alias-state-business-primary,#175cd3) 6%,transparent);font-size:11px}.dwp-ann-head{display:flex;gap:6px;align-items:center}.dwp-ann-kind{font-weight:600;color:var(--dsw-alias-state-business-primary,#175cd3)}.dwp-ann-target{min-width:0;flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-family:var(--ds-font-family-code,ui-monospace,monospace)}.dwp-note{box-sizing:border-box;width:100%;min-height:52px;resize:vertical;margin-top:6px;padding:7px 9px;border:1px solid var(--dsw-alias-border-l2,#ddd);border-radius:7px;background:var(--dsw-alias-bg-base,#fff);color:var(--dsw-alias-label-primary,#101828);font:inherit;outline:none}.dwp-note:focus{border-color:var(--dsw-alias-state-business-primary,#175cd3)}.dwp-ann-actions{display:flex;justify-content:flex-end;gap:6px;margin-top:6px}
@media(max-width:760px){.dwp-shell{top:0;right:0;bottom:0;width:100vw;min-width:0;border-radius:0}.dwp-resize{display:none}.dwp-body{grid-template-columns:135px minmax(0,1fr)}}
@media(max-width:520px){.dwp-body{display:flex}.dwp-tree{width:112px;flex:none}.dwp-content{padding:12px}.dwp-project-name{display:none}}
@media(prefers-reduced-motion:reduce){*{scroll-behavior:auto!important}}
`;

    function installStyle() {
      let style = document.querySelector('style[data-plugin-css="' + ID + '/client.css"]');
      if (style) return () => {};
      style = document.createElement("style");
      style.dataset.pluginCss = ID + "/client.css";
      style.textContent = CSS;
      document.head.appendChild(style);
      return () => style.remove();
    }

    function defaultState() {
      return { open:false,dir:"",file:"",recent:[],width:560,revision:0,projectView:false };
    }
    function hydrate(id) {
      if (stateBySession.has(id)) return;
      let saved = {};
      try { saved = JSON.parse(localStorage.getItem(ID + ":" + id) || "{}"); } catch {}
      stateBySession.set(id, {
        ...defaultState(),
        dir:typeof saved.dir === "string" ? saved.dir : "",
        file:typeof saved.file === "string" ? saved.file : "",
        recent:Array.isArray(saved.recent) ? saved.recent.filter((x)=>typeof x === "string").slice(0,12) : [],
        width:Number.isFinite(saved.width) ? Math.max(340,Math.min(900,saved.width)) : 560
      });
    }
    function readState(id) { hydrate(id); return stateBySession.get(id); }
    function writeState(id, patch) {
      const next = { ...readState(id), ...patch };
      stateBySession.set(id,next);
      try { localStorage.setItem(ID + ":" + id,JSON.stringify({dir:next.dir,file:next.file,recent:next.recent.slice(0,12),width:next.width})); } catch {}
      listeners.forEach((fn)=>fn());
    }
    function usePreviewState(id) {
      hydrate(id);
      const [,redraw] = React.useState(0);
      React.useEffect(()=>{ const fn=()=>redraw((n)=>n+1); listeners.add(fn); return()=>listeners.delete(fn); },[]);
      return readState(id);
    }

    function api(id,op,path) {
      const query = new URLSearchParams({sessionId:id});
      if(path) query.set("path",path);
      return "/__dsh-preview/" + op + "?" + query;
    }
    function siteUrl(id,path) {
      const encoded = String(path||"").split("/").filter(Boolean).map(encodeURIComponent).join("/");
      const isolatedHost = location.hostname === "127.0.0.1" ? "localhost" : "127.0.0.1";
      const token = readState(id).siteToken || "";
      return location.protocol + "//" + isolatedHost + ":" + location.port + "/__dsh-preview/site/" + encodeURIComponent(token) + "/" + encodeURIComponent(id) + "/" + encoded;
    }
    async function requestJson(url,options) {
      const response = await fetch(url,options);
      let value = {};
      try { value = await response.json(); } catch {}
      if(!response.ok) throw new Error(value.message || value.error || ("HTTP " + response.status));
      return value;
    }
    function postControl(op,sessionId,extra) {
      return requestJson("/__dsh-preview/"+op,{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({sessionId,...(extra||{})})});
    }
    function parentOf(path) { const parts=String(path||"").split("/").filter(Boolean);parts.pop();return parts.join("/"); }
    function extension(path) { const name=String(path||"").split("/").pop()||"";const at=name.lastIndexOf(".");return at<0?name.toLowerCase():name.slice(at+1).toLowerCase(); }
    function isMarkdown(path){return ["md","mdx","markdown"].includes(extension(path));}
    function isHtml(path){return ["html","htm","xhtml"].includes(extension(path));}
    function isPdf(path){return extension(path)==="pdf";}
    function isImage(path){return ["png","jpg","jpeg","gif","webp","bmp","ico","svg","avif"].includes(extension(path));}
    function isAudio(path){return ["mp3","wav","ogg","m4a","flac"].includes(extension(path));}
    function isVideo(path){return ["mp4","webm","mov","m4v"].includes(extension(path));}
    function isText(path){return TEXT_EXTENSIONS.has(extension(path));}
    function formatBytes(size){if(!Number.isFinite(size))return"";if(size<1024)return size+" B";if(size<1048576)return(size/1024).toFixed(1)+" KiB";return(size/1048576).toFixed(1)+" MiB";}
    function escapeHtml(value){return String(value).replace(/[&<>"']/g,(c)=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));}
    function renderMarkdown(source) {
      const code=[];
      let text=String(source||"").replace(/```[^\n]*\n([\s\S]*?)```/g,(_,body)=>{const token="\u0000CODE"+code.length+"\u0000";code.push("<pre><code>"+escapeHtml(body)+"</code></pre>");return token;});
      text=escapeHtml(text)
        .replace(/^######\s+(.+)$/gm,"<h6>$1</h6>").replace(/^#####\s+(.+)$/gm,"<h5>$1</h5>")
        .replace(/^####\s+(.+)$/gm,"<h4>$1</h4>").replace(/^###\s+(.+)$/gm,"<h3>$1</h3>")
        .replace(/^##\s+(.+)$/gm,"<h2>$1</h2>").replace(/^#\s+(.+)$/gm,"<h1>$1</h1>")
        .replace(/^&gt;\s?(.+)$/gm,"<blockquote>$1</blockquote>")
        .replace(/`([^`]+)`/g,"<code>$1</code>").replace(/\*\*([^*]+)\*\*/g,"<strong>$1</strong>")
        .replace(/\*([^*]+)\*/g,"<em>$1</em>")
        .replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g,'<a href="$2" target="_blank" rel="noreferrer">$1</a>');
      text=text.split(/\n{2,}/).map((part)=>/^<(?:h\d|pre|blockquote)/.test(part)||/^\u0000CODE/.test(part)?part:"<p>"+part.replace(/\n/g,"<br>")+"</p>").join("");
      return text.replace(/\u0000CODE(\d+)\u0000/g,(_,index)=>code[Number(index)]||"");
    }
    function appendDraft(sessionId,text) {
      const input=inputBySession.get(sessionId);
      if(!input || !input.actions) return false;
      const current=input.draft||"";
      input.actions.setDraft(current + (current.trim()?"\n\n":"") + text);
      return true;
    }

    function LaunchButton({sessionId,useInput,inputActions}) {
      const state=usePreviewState(sessionId);
      const draft=useInput((snapshot)=>snapshot.draft);
      React.useEffect(()=>{
        const value={draft,actions:inputActions};inputBySession.set(sessionId,value);
        return()=>{if(inputBySession.get(sessionId)===value)inputBySession.delete(sessionId);};
      },[sessionId,draft,inputActions]);
      return React.createElement("button",{type:"button",className:"dwp-launch","data-active":state.open||undefined,title:"工作区预览","aria-label":"打开工作区预览",onClick:()=>writeState(sessionId,{open:!state.open})},"▣");
    }

    function CodeView({text}) {
      return React.createElement("pre",{className:"dwp-code"},String(text||"").split("\n").map((line,index)=>React.createElement("span",{className:"dwp-code-line",key:index},React.createElement("span",{className:"dwp-line-no"},index+1),line||" ")));
    }

    function ProjectBar({meta,status,confirm,setConfirm,sessionId,setError,onProjectView}) {
      const project=meta&&meta.project;
      if(!project)return null;
      const running=status&&["running","stopping"].includes(status.status);
      const prepare=async()=>{try{setError("");setConfirm(await postControl("project-prepare",sessionId));}catch(error){setError(String(error.message||error));}};
      const start=async()=>{try{setError("");await postControl("project-start",sessionId,{challenge:confirm.challenge});setConfirm(null);}catch(error){setError(String(error.message||error));}};
      const stop=async()=>{try{setError("");await postControl("project-stop",sessionId);}catch(error){setError(String(error.message||error));}};
      return React.createElement(React.Fragment,null,
        React.createElement("section",{className:"dwp-project"},
          React.createElement("div",{className:"dwp-project-row"},
            React.createElement("span",{className:"dwp-project-name"},project.name),
            React.createElement("span",{className:"dwp-project-state","data-running":running||undefined},status&&status.status!=="idle"?status.status:(project.runnable?"可运行":"直接预览")),
            React.createElement("span",{className:"dwp-project-spacer"}),
            status&&status.url&&React.createElement("button",{className:"dwp-small",onClick:onProjectView},"打开运行页"),
            running?React.createElement("button",{className:"dwp-small","data-danger":true,onClick:stop},"停止"):project.runnable&&React.createElement("button",{className:"dwp-small",onClick:prepare},"运行")),
          status&&status.logs&&status.logs.length>0&&React.createElement("pre",{className:"dwp-log"},status.logs.slice(-80).join("\n"))),
        confirm&&React.createElement("section",{className:"dwp-confirm"},
          React.createElement("div",{className:"dwp-confirm-title"},"确认运行工作区代码"),
          React.createElement("div",null,confirm.warning),React.createElement("div",{className:"dwp-command"},confirm.command),
          React.createElement("div",{className:"dwp-confirm-actions"},React.createElement("button",{className:"dwp-small",onClick:()=>setConfirm(null)},"取消"),React.createElement("button",{className:"dwp-small dwp-primary",onClick:start},"确认运行")))
      );
    }

    function PreviewPanel({sessionId}) {
      const state=usePreviewState(sessionId);
      const [meta,setMeta]=React.useState(null);
      const [listing,setListing]=React.useState(null);
      const [content,setContent]=React.useState("");
      const [error,setError]=React.useState("");
      const [mode,setMode]=React.useState("preview");
      const [project,setProject]=React.useState({status:"idle",logs:[]});
      const [confirm,setConfirm]=React.useState(null);
      const [dragging,setDragging]=React.useState(false);
      const [uploading,setUploading]=React.useState(false);
      const [marking,setMarking]=React.useState(false);
      const [annotation,setAnnotation]=React.useState(null);
      const [note,setNote]=React.useState("");
      const iframeRef=React.useRef(null);
      const markdownRef=React.useRef(null);

      React.useEffect(()=>{if(!state.open)return;const controller=new AbortController();requestJson(api(sessionId,"meta"),{signal:controller.signal}).then((value)=>{setMeta(value);writeState(sessionId,{siteToken:value.siteToken||""});}).catch((e)=>{if(e.name!=="AbortError")setError(String(e.message||e));});return()=>controller.abort();},[sessionId,state.open,state.revision]);
      React.useEffect(()=>{if(!state.open)return;const controller=new AbortController();setError("");requestJson(api(sessionId,"list",state.dir),{signal:controller.signal}).then(setListing).catch((e)=>{if(e.name!=="AbortError")setError(String(e.message||e));});return()=>controller.abort();},[sessionId,state.open,state.dir,state.revision]);
      React.useEffect(()=>{if(!state.open||!state.file||!isText(state.file)||isHtml(state.file)){setContent("");return;}const controller=new AbortController();setError("");fetch(api(sessionId,"file",state.file),{signal:controller.signal}).then(async(response)=>{if(!response.ok){let value={};try{value=await response.json();}catch{}throw new Error(value.message||("HTTP "+response.status));}return response.text();}).then(setContent).catch((e)=>{if(e.name!=="AbortError")setError(String(e.message||e));});return()=>controller.abort();},[sessionId,state.open,state.file,state.revision]);
      React.useEffect(()=>{if(!state.open)return;let stopped=false;const refresh=()=>requestJson(api(sessionId,"project-status")).then((value)=>{if(!stopped)setProject(value);}).catch(()=>{});refresh();const timer=setInterval(refresh,1000);return()=>{stopped=true;clearInterval(timer);};},[sessionId,state.open]);
      React.useEffect(()=>{if(!state.open)return;const onKey=(event)=>{if(event.key==="Escape"){if(confirm)setConfirm(null);else writeState(sessionId,{open:false});}};window.addEventListener("keydown",onKey);return()=>window.removeEventListener("keydown",onKey);},[sessionId,state.open,confirm]);
      React.useEffect(()=>{const onMessage=(event)=>{if(event.source!==iframeRef.current?.contentWindow||!event.data||event.data.source!==ID)return;if(event.data.type==="bridge-ready")event.source.postMessage({source:ID,type:"mark-mode",enabled:marking},"*");if(event.data.type==="element-selected"){setAnnotation({kind:"元素",target:event.data.selector||state.file,text:event.data.text||"",html:event.data.html||""});setMarking(false);}};window.addEventListener("message",onMessage);return()=>window.removeEventListener("message",onMessage);},[marking,state.file]);
      React.useEffect(()=>{iframeRef.current?.contentWindow?.postMessage({source:ID,type:"mark-mode",enabled:marking},"*");},[marking]);
      if(!state.open)return null;

      const openEntry=(entry)=>{if(entry.kind==="directory")writeState(sessionId,{dir:entry.path});else{setMode("preview");setAnnotation(null);setMarking(false);writeState(sessionId,{file:entry.path,projectView:false,recent:[entry.path,...state.recent.filter((x)=>x!==entry.path)].slice(0,12)});}};
      const startResize=(event)=>{const start=event.clientX,width=state.width;event.currentTarget.setPointerCapture(event.pointerId);const move=(next)=>writeState(sessionId,{width:Math.max(340,Math.min(900,width+start-next.clientX))});const up=()=>{window.removeEventListener("pointermove",move);window.removeEventListener("pointerup",up);};window.addEventListener("pointermove",move);window.addEventListener("pointerup",up);};
      const uploadFiles=async(files)=>{if(!files.length)return;setUploading(true);setError("");const paths=[];try{for(const file of files){const query=new URLSearchParams({sessionId,name:file.name});const value=await requestJson("/__dsh-preview/upload?"+query,{method:"POST",headers:{"Content-Type":file.type||"application/octet-stream"},body:file});paths.push(value.path);}if(paths.length){appendDraft(sessionId,"请查看这些工作区文件：\n"+paths.map((path)=>"- `"+path+"`").join("\n"));writeState(sessionId,{dir:".dsh-drops",revision:state.revision+1});}}catch(uploadError){setError(String(uploadError.message||uploadError));}finally{setUploading(false);setDragging(false);}};
      const selectMarkdown=()=>{const selection=window.getSelection();const text=selection&&selection.toString().trim();if(!text||!markdownRef.current||!markdownRef.current.contains(selection.anchorNode))return;const index=content.indexOf(text);const line=index<0?null:content.slice(0,index).split("\n").length;setAnnotation({kind:"文档选区",target:state.file+(line?":"+line:""),text,html:""});};
      const sendAnnotation=()=>{if(!annotation)return;const body=["请按以下预览批注修改：","- 文件：`"+state.file+"`","- 目标：`"+annotation.target+"`",annotation.text?"- 当前内容：\n> "+annotation.text.replace(/\n/g,"\n> "):"",note.trim()?"- 批注："+note.trim():""].filter(Boolean).join("\n");if(appendDraft(sessionId,body)){setAnnotation(null);setNote("");}else setError("当前会话输入框不可用");};
      const crumbs=state.dir.split("/").filter(Boolean);
      const activeProjectUrl=state.projectView&&project.url?project.url:null;
      const frameMode=!!activeProjectUrl||isHtml(state.file)||isPdf(state.file);
      let viewer=React.createElement("div",{className:"dwp-state"},"选择一个文件进行预览");
      if(activeProjectUrl)viewer=React.createElement("iframe",{ref:iframeRef,className:"dwp-frame",src:activeProjectUrl,sandbox:"allow-scripts allow-forms allow-modals allow-popups",title:"运行项目预览"});
      else if(state.file&&isHtml(state.file))viewer=React.createElement("iframe",{ref:iframeRef,className:"dwp-frame",src:siteUrl(sessionId,state.file),sandbox:"allow-scripts",title:state.file,onLoad:()=>iframeRef.current?.contentWindow?.postMessage({source:ID,type:"mark-mode",enabled:marking},"*")});
      else if(state.file&&isPdf(state.file))viewer=React.createElement("iframe",{className:"dwp-frame",src:api(sessionId,"file",state.file),sandbox:"allow-same-origin",title:state.file});
      else if(state.file&&isImage(state.file))viewer=React.createElement("img",{className:"dwp-image",src:api(sessionId,"file",state.file),alt:state.file});
      else if(state.file&&isAudio(state.file))viewer=React.createElement("audio",{className:"dwp-media",controls:true,src:api(sessionId,"file",state.file)});
      else if(state.file&&isVideo(state.file))viewer=React.createElement("video",{className:"dwp-media",controls:true,src:api(sessionId,"file",state.file)});
      else if(state.file&&isMarkdown(state.file)&&mode==="preview")viewer=React.createElement("article",{ref:markdownRef,className:"dwp-markdown",onMouseUp:selectMarkdown,dangerouslySetInnerHTML:{__html:renderMarkdown(content)}});
      else if(state.file&&isText(state.file))viewer=React.createElement(CodeView,{text:content});
      else if(state.file)viewer=React.createElement("div",{className:"dwp-state"},"该文件类型没有内置渲染器。可复制路径交给 Agent 读取。");

      return React.createElement(React.Fragment,null,
        React.createElement("div",{className:"dwp-backdrop",onClick:()=>writeState(sessionId,{open:false})}),
        React.createElement("aside",{className:"dwp-shell",style:{"--dwp-width":state.width+"px"},role:"dialog","aria-label":"工作区预览",onDragEnter:(event)=>{if(event.dataTransfer?.types?.includes("Files")){event.preventDefault();setDragging(true);}},onDragOver:(event)=>{if(event.dataTransfer?.types?.includes("Files"))event.preventDefault();},onDragLeave:(event)=>{if(!event.currentTarget.contains(event.relatedTarget))setDragging(false);},onDrop:(event)=>{event.preventDefault();uploadFiles([...event.dataTransfer.files]);}},
          React.createElement("div",{className:"dwp-resize",onPointerDown:startResize}),
          React.createElement("header",{className:"dwp-head"},
            React.createElement("div",{className:"dwp-title",title:activeProjectUrl||state.file||meta?.workspaceTitle},activeProjectUrl||state.file||meta?.workspaceTitle||"工作区预览"),
            isHtml(state.file)&&!activeProjectUrl&&React.createElement("button",{className:"dwp-icon","data-active":marking||undefined,title:"元素标注",onClick:()=>setMarking(!marking)},"⌖"),
            React.createElement("button",{className:"dwp-icon",title:"刷新",onClick:()=>writeState(sessionId,{revision:state.revision+1})},"↻"),
            React.createElement("button",{className:"dwp-icon",title:"复制路径",disabled:!state.file,onClick:()=>navigator.clipboard?.writeText(state.file)},"⧉"),
            React.createElement("button",{className:"dwp-icon",title:"关闭",onClick:()=>writeState(sessionId,{open:false})},"✕")),
          React.createElement(ProjectBar,{meta,status:project,confirm,setConfirm,sessionId,setError,onProjectView:()=>writeState(sessionId,{projectView:true})}),
          React.createElement("nav",{className:"dwp-crumbs"},React.createElement("button",{className:"dwp-crumb",onClick:()=>writeState(sessionId,{dir:""})},"工作区"),crumbs.map((part,index)=>React.createElement(React.Fragment,{key:index},"/",React.createElement("button",{className:"dwp-crumb",onClick:()=>writeState(sessionId,{dir:crumbs.slice(0,index+1).join("/")})},part)))),
          React.createElement("div",{className:"dwp-body"},
            React.createElement("aside",{className:"dwp-tree"},state.dir&&React.createElement("button",{className:"dwp-row",onClick:()=>writeState(sessionId,{dir:parentOf(state.dir)})},"↰ 上一级"),listing?.entries?.map((entry)=>React.createElement("button",{key:entry.path,className:"dwp-row","data-active":entry.path===state.file&&!activeProjectUrl||undefined,onClick:()=>openEntry(entry),title:entry.path},React.createElement("span",null,entry.kind==="directory"?"▸":"·"),React.createElement("span",{className:"dwp-row-name"},entry.name),entry.size!==null&&React.createElement("span",{className:"dwp-row-size"},formatBytes(entry.size))))),
            React.createElement("section",{className:"dwp-view"},
              React.createElement("div",{className:"dwp-tabs"},activeProjectUrl&&React.createElement("button",{className:"dwp-tab","data-active":true,onClick:()=>{}},"运行项目"),isMarkdown(state.file)&&!activeProjectUrl&&React.createElement(React.Fragment,null,React.createElement("button",{className:"dwp-tab","data-active":mode==="preview"||undefined,onClick:()=>setMode("preview")},"预览"),React.createElement("button",{className:"dwp-tab","data-active":mode==="source"||undefined,onClick:()=>setMode("source")},"源码")),state.recent.slice(0,4).map((path)=>React.createElement("button",{key:path,className:"dwp-tab",title:path,"data-active":path===state.file&&!activeProjectUrl||undefined,onClick:()=>writeState(sessionId,{file:path,dir:parentOf(path),projectView:false})},path.split("/").pop()))),
              React.createElement("div",{className:"dwp-content","data-frame":frameMode||undefined},error?React.createElement("div",{className:"dwp-state dwp-error"},error):viewer,dragging&&React.createElement("div",{className:"dwp-drop"},uploading?"正在保存到工作区…":"释放以保存到 .dsh-drops")))),
          annotation&&React.createElement("section",{className:"dwp-annotation"},React.createElement("div",{className:"dwp-ann-head"},React.createElement("span",{className:"dwp-ann-kind"},annotation.kind),React.createElement("span",{className:"dwp-ann-target",title:annotation.target},annotation.target)),React.createElement("textarea",{className:"dwp-note",value:note,placeholder:"描述希望如何修改…",onChange:(event)=>setNote(event.target.value)}),React.createElement("div",{className:"dwp-ann-actions"},React.createElement("button",{className:"dwp-small",onClick:()=>{setAnnotation(null);setNote("");}},"取消"),React.createElement("button",{className:"dwp-small dwp-primary",onClick:sendAnnotation},"发送到对话"))),
          React.createElement("footer",{className:"dwp-status"},uploading?"上传中":marking?"标注模式：点击页面元素":state.file||state.dir||"工作区根目录",React.createElement("span",{className:"dwp-status-spacer"}),meta&&("文本 "+formatBytes(meta.maxTextBytes)+" · 媒体 "+formatBytes(meta.maxMediaBytes)))));
    }

    function OverlayEntry({useSessions}) {
      const sessionId=useSessions((state)=>state.current===undefined?null:state.current);
      return sessionId?ReactDOM.createPortal(React.createElement(PreviewPanel,{sessionId}),document.body):null;
    }

    function apply(ctx) {
      const removeStyle=installStyle();
      if(typeof ctx.effect==="function")ctx.effect(()=>removeStyle);
      const onClick=(event)=>{
        if(event.defaultPrevented||event.button!==0||event.ctrlKey||event.metaKey||event.shiftKey||event.altKey)return;
        const anchor=event.target?.closest?.('[data-conversation-scroll] a[href]');
        if(!anchor)return;
        const raw=(anchor.getAttribute("href")||"").trim();
        if(!raw||/^(?:https?:|mailto:|tel:|javascript:|data:|#)/i.test(raw)||raw.startsWith("/"))return;
        const path=raw.split(/[?#]/)[0].replace(/^\.\//,"");
        if(!path||path.includes("..")||!/[.][A-Za-z0-9]{1,12}$/.test(path))return;
        const current=document.querySelector('[data-slot="conversation.session"]');
        let sid=null;
        for(const [candidate,state] of stateBySession){if(state.open){sid=candidate;break;}}
        if(!sid){const bootCurrent=window.__DSH_PREVIEW_CURRENT_SESSION__;sid=typeof bootCurrent==="string"?bootCurrent:null;}
        if(!sid)return;
        event.preventDefault();event.stopPropagation();writeState(sid,{open:true,file:path,dir:parentOf(path),projectView:false,recent:[path,...readState(sid).recent.filter((x)=>x!==path)].slice(0,12)});
      };
      document.addEventListener("click",onClick,true);
      if(typeof ctx.effect==="function")ctx.effect(()=>()=>document.removeEventListener("click",onClick,true));
      ctx.slots.inject("conversation.session.header.utilities",()=>ctx.slots.register({name:"conversation.session.header.utilities",id:"dsh-web-preview",order:700,label:"Workspace preview"},(props)=>{window.__DSH_PREVIEW_CURRENT_SESSION__=props.sessionId;return React.createElement(LaunchButton,props);}));
      ctx.slots.inject("shell.overlay",()=>ctx.slots.register({name:"shell.overlay",id:"dsh-web-preview-panel",order:100,label:"Workspace preview panel"},OverlayEntry));
    }

    exports.apply=apply;
    exports.inject=inject;
    return module.exports;
  }
});
