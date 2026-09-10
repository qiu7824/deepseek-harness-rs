(() => {
const suiteAssetBase = document.currentScript?.src.replace(/\.js(?:\?.*)?$/, "/") || "";
globalThis.__DSH_SIDEBAR_SUITE_ASSET_BASE__ = suiteAssetBase;
window.__ModuleLoader__.load({
  id: "dsh-sidebar-workbench-suite",
  factory: (require) => {
    const module = { exports: {} }, exports = module.exports;
    const React = require("react"), h = React.createElement;
    const {Button,Menu}=require("@deepseek-ai/dsh-client-ui-primitives");
    let SettingsSwitch;
    const inject = ["betterSidebar", "connection", "sessions", "settingsScope", "slots"];
    const assetBase = suiteAssetBase;
    const assetVersions = globalThis.__DSH_SIDEBAR_SUITE_ASSET_VERSIONS__ ||= Object.create(null);
    const assetLoads = new Map();
    const desktopStarts = new Map(), desktopClosures = new Map();
    const desktopStartKey = (owner, session) => owner + "\u0000" + session;
    function abortDesktopStart(owner, session) { const key=desktopStartKey(owner,session),entry=desktopStarts.get(key);if(entry){desktopStarts.delete(key);entry.controller.abort()} }
    function desktopClose(owner, session, options) {
      abortDesktopStart(owner,session);
      const key=desktopStartKey(owner,session),existing=desktopClosures.get(key);
      if(existing?.pending)return existing.promise;
      const controller=new AbortController(),entry={pending:true,promise:null};
      const timer=setTimeout(()=>controller.abort(),75_000);
      entry.promise=json("/__dsh-computer-use/action",{...options,signal:controller.signal}).then(value=>{
        entry.pending=false;if(desktopClosures.get(key)===entry)desktopClosures.delete(key);return value;
      },error=>{entry.pending=false;throw error}).finally(()=>clearTimeout(timer));
      // Failed closes remain a barrier until an explicit close retries them.
      desktopClosures.set(key,entry);
      return entry.promise;
    }
    function desktopStart(owner, session, options, keepAlive=true) {
      const key=desktopStartKey(owner,session),existing=desktopStarts.get(key);
      if(existing)return existing.promise;
      const controller=new AbortController(),entry={controller,promise:null};
      const timer=setTimeout(()=>controller.abort(),75_000);
      const viewerSignal=keepAlive?null:options.signal,onViewerAbort=()=>controller.abort();
      if(viewerSignal?.aborted)controller.abort();else viewerSignal?.addEventListener("abort",onViewerAbort,{once:true});
      desktopStarts.set(key,entry);
      entry.promise=(async()=>{
        const closing=desktopClosures.get(key);let abort;
        const cancelled=()=>Object.assign(new Error("Control startup cancelled"),{name:"AbortError"});
        if(controller.signal.aborted)throw cancelled();
        if(closing)try{
          await Promise.race([closing.promise,new Promise((_,reject)=>{abort=()=>reject(cancelled());controller.signal.addEventListener("abort",abort,{once:true})})]);
        }finally{if(abort)controller.signal.removeEventListener("abort",abort)}
        if(controller.signal.aborted)throw cancelled();
        return json("/__dsh-computer-use/action",{...options,signal:controller.signal});
      })().finally(()=>{clearTimeout(timer);viewerSignal?.removeEventListener("abort",onViewerAbort);if(desktopStarts.get(key)===entry)desktopStarts.delete(key)});
      return entry.promise;
    }
    const fileDrafts = new Map(), MAX_FILE_DRAFTS = 32, MAX_FILE_DRAFT_BYTES = 4 * 1024 * 1024, MAX_FILE_DRAFT_TOTAL_BYTES = 8 * 1024 * 1024;
    const FILE_DRAFT_WARNING = "草稿与文件基线合计超过 4 MiB；当前仍可编辑，但移动或关闭此查看器会丢失未保存内容。";
    let fileDraftBytes = 0;
    const fileDraftKey = (kind, sessionId, path) => kind + "\u0000" + sessionId + "\u0000" + path;
    function boundedTextBytes(value, limit) {
      if (value.length * 2 > limit) return limit + 1;
      let bytes = 0;
      for (let index = 0; index < value.length; index++) {
        const code = value.charCodeAt(index);
        if (code < 0x80) bytes += 1;
        else if (code < 0x800) bytes += 2;
        else if (code >= 0xd800 && code <= 0xdbff && index + 1 < value.length && value.charCodeAt(index + 1) >= 0xdc00 && value.charCodeAt(index + 1) <= 0xdfff) { bytes += 4; index += 1; }
        else bytes += 3;
        if (bytes > limit) return bytes;
      }
      return Math.max(bytes, value.length * 2);
    }
    function dropFileDraft(key) {
      const current = fileDrafts.get(key);
      if (!current) return;
      fileDraftBytes = Math.max(0, fileDraftBytes - current.bytes);
      fileDrafts.delete(key);
    }
    function clearFileDrafts() {
      fileDrafts.clear();
      fileDraftBytes = 0;
    }
    function rememberFileDraft(key, source, saved, etag) {
      if (source === saved) { dropFileDraft(key); return false; }
      const sourceBytes = boundedTextBytes(source, MAX_FILE_DRAFT_BYTES);
      const bytes = sourceBytes > MAX_FILE_DRAFT_BYTES ? sourceBytes : sourceBytes + boundedTextBytes(saved, MAX_FILE_DRAFT_BYTES - sourceBytes);
      dropFileDraft(key);
      if (bytes > MAX_FILE_DRAFT_BYTES || fileDrafts.size >= MAX_FILE_DRAFTS || fileDraftBytes + bytes > MAX_FILE_DRAFT_TOTAL_BYTES) return false;
      fileDrafts.set(key, { source, saved, etag, bytes }); fileDraftBytes += bytes;
      return true;
    }
    function applyFetchedFile(key, text, etag, setters) {
      const draft = fileDrafts.get(key);
      if (!draft) {
        setters.source(text); setters.saved(text); setters.etag(etag); setters.error("");
        return;
      }
      const sameVersion = draft.saved === text || !!draft.etag && !!etag && draft.etag === etag;
      const effectiveEtag = sameVersion ? etag || draft.etag : draft.etag;
      setters.source(draft.source); setters.saved(draft.saved); setters.etag(effectiveEtag);
      rememberFileDraft(key, draft.source, draft.saved, effectiveEtag);
      setters.error(sameVersion ? "" : "文件已在外部更改；当前未保存草稿已保留，保存时会进行版本检查。");
    }
    function loadAsset(name, globalName) {
      if (globalThis[globalName] && assetVersions[globalName] === assetBase) return Promise.resolve(globalThis[globalName]);
      if (assetLoads.has(name)) return assetLoads.get(name);
      if (!assetBase) return Promise.reject(new Error("插件资源地址不可用"));
      const promise = new Promise((resolve, reject) => {
        const script = document.createElement("script");
        script.src = assetBase + name;
        script.async = true;
        script.onload = () => { script.remove(); if (globalThis[globalName]) { assetVersions[globalName] = assetBase; resolve(globalThis[globalName]); } else reject(new Error(name + " 未注册运行时")); };
        script.onerror = () => { script.remove(); reject(new Error(name + " 加载失败")); };
        document.head.appendChild(script);
      });
      const tracked = promise.catch(error => { assetLoads.delete(name); throw error; });
      assetLoads.set(name, tracked);
      return tracked;
    }
    const endpoint = (op, sessionId, path) => {
      const query = new URLSearchParams({ sessionId });
      if (path !== undefined) query.set("path", path);
      return "/__dsh-preview/" + op + "?" + query;
    };
    async function json(url, options) {
      const response = await fetch(url, options);
      const value = await response.json().catch(() => ({}));
      if (!response.ok) throw Object.assign(new Error(value.message || value.error?.message || value.error || "HTTP " + response.status),{code:typeof value.error==="string"?value.error:value.error?.code});
      return value;
    }
    async function loadPdfBytes(path,scope,signal) {
      const response=await fetch(endpoint("file",scope.sessionId,path),{signal});
      if(!response.ok)throw new Error(`PDF HTTP ${response.status}`);
      const mime=(response.headers.get("content-type")||"").split(";")[0].toLowerCase();
      if(mime!=="application/pdf")throw new Error("文件不是 PDF 格式");
      if(Number(response.headers.get("content-length"))>64*1024*1024)throw new Error("PDF 超过 64 MiB 预览限制");
      const data=new Uint8Array(await response.arrayBuffer());
      if(data.byteLength>64*1024*1024)throw new Error("PDF 超过 64 MiB 预览限制");
      if(!String.fromCharCode(...data.slice(0,1024)).includes("%PDF-"))throw new Error("PDF 文件头无效");
      return data;
    }
    function PdfViewer(props) {
      const positionKey=fileDraftKey("pdf-position",props.scope.sessionId,props.path),stored=readingPositions.get(positionKey);
      const savedPage=Number.isInteger(stored?.page)&&stored.page>0?stored.page:1,savedZoom=[0.5,0.75,1,1.25,1.5,2].includes(stored?.zoom)?stored.zoom:1;
      const [document,setDocument]=React.useState(null),[page,setPage]=React.useState(savedPage),[zoom,setZoom]=React.useState(savedZoom),[error,setError]=React.useState(""),[rendering,setRendering]=React.useState(true);
      const canvas=React.useRef(null);
      React.useEffect(()=>{
        const controller=new AbortController();let opened;
        setDocument(null);setPage(savedPage);setZoom(savedZoom);setError("");setRendering(true);
        loadAsset("pdf.js","__DSH_SIDEBAR_PDF__").then(async runtime=>{
          if(controller.signal.aborted)return;
          opened=runtime.open(props.customData,controller.signal);
          const next=await opened.document;if(!controller.signal.aborted){setPage(value=>Math.min(next.numPages,value));setDocument(next);}
        }).catch(reason=>{if(!controller.signal.aborted){setError(reason.message||String(reason));setRendering(false)}});
        return()=>{controller.abort();void opened?.dispose()};
      },[props.customData,props.path,props.scope.sessionId]);
      React.useEffect(()=>{
        if(!document||!canvas.current)return;let active=true,task;
        setRendering(true);setError("");
        document.getPage(page).then(async current=>{
          if(!active)return;
          const base=current.getViewport({scale:1}),scale=Math.min(zoom*Math.min(window.devicePixelRatio||1,2),Math.sqrt(16000000/Math.max(1,base.width*base.height))),viewport=current.getViewport({scale});
          const target=canvas.current;target.width=Math.ceil(viewport.width);target.height=Math.ceil(viewport.height);target.style.width=Math.ceil(base.width*zoom)+"px";target.style.height=Math.ceil(base.height*zoom)+"px";
          task=current.render({canvasContext:target.getContext("2d"),viewport});await task.promise;
          if(active)setRendering(false);
        }).catch(reason=>{if(active&&reason?.name!=="RenderingCancelledException"){setError(reason.message||String(reason));setRendering(false)}});
        return()=>{active=false;task?.cancel()};
      },[document,page,zoom]);
      React.useEffect(()=>{if(document)rememberReading(positionKey,{page,zoom})},[document,page,zoom,positionKey]);
      return h("section",{className:"dswSuite","data-preview-kind":"pdf"},
        h("div",{className:"dswSuiteBar"},h("strong",{className:"dswSuiteTitle",title:props.path},props.title),
          h(Button,{variant:"outline",size:"sm",disabled:!document||page<=1,onClick:()=>setPage(value=>value-1)},"上一页"),
          h("input",{type:"number",min:1,max:document?.numPages||1,value:page,"aria-label":"PDF 页码",style:{width:65},disabled:!document,onChange:event=>{const value=Number(event.target.value);if(Number.isInteger(value)&&value>=1&&value<=document.numPages)setPage(value)}}),h("span",null,"/ "+(document?.numPages||"—")),
          h(Button,{variant:"outline",size:"sm",disabled:!document||page>=document.numPages,onClick:()=>setPage(value=>value+1)},"下一页"),
          h("select",{value:zoom,"aria-label":"PDF 缩放",onChange:event=>setZoom(Number(event.target.value))},...[0.5,0.75,1,1.25,1.5,2].map(value=>h("option",{key:value,value},Math.round(value*100)+"%")))),
        error&&h("div",{className:"dswSuiteStatus dswSuiteError",role:"alert"},error),rendering&&!error&&h("div",{className:"dswSuiteStatus",role:"status"},"正在渲染 PDF…"),
        h("div",{style:{minHeight:0,flex:1,overflow:"auto",padding:16,background:"var(--dsw-alias-bg-layer-1)"}},h("canvas",{ref:canvas,role:"img","aria-label":`${props.title} · 第 ${page} 页`,style:{display:error?"none":"block",margin:"auto",background:"white"}})));
    }
    function ImageViewer(props) {
      const [zoom,setZoom]=React.useState(1),[error,setError]=React.useState(false);
      React.useEffect(()=>{setZoom(1);setError(false)},[props.path,props.scope.sessionId]);
      return h("section",{className:"dswSuite","data-preview-kind":"image"},h("div",{className:"dswSuiteBar"},h("strong",{className:"dswSuiteTitle"},props.title),h(Button,{variant:"outline",size:"sm",disabled:zoom<=0.25,onClick:()=>setZoom(value=>value/2)},"缩小"),h("span",null,Math.round(zoom*100)+"%"),h(Button,{variant:"outline",size:"sm",disabled:zoom>=4,onClick:()=>setZoom(value=>value*2)},"放大")),error?h("div",{role:"alert",className:"dswSuiteError"},"图片无法解码或文件已失效"):h("div",{style:{minHeight:0,flex:1,overflow:"auto",padding:16}},h("img",{src:props.mediaUrl,alt:props.title,onError:()=>setError(true),style:{display:"block",maxWidth:zoom===1?"100%":"none",transform:`scale(${zoom})`,transformOrigin:"top left"}})));
    }
    function isolatedHtml(source,base) {
      const document=new window.DOMParser().parseFromString(source,"text/html");
      for(const node of document.querySelectorAll("base"))node.remove();
      const address=new URL(base),root=address.origin,directory=root+address.pathname.split("/").slice(0,5).join("/")+"/";
      const csp=document.createElement("meta");csp.httpEquiv="Content-Security-Policy";
      csp.content=`default-src 'none'; img-src data: blob: ${directory}; media-src data: blob: ${directory}; style-src 'unsafe-inline' ${directory}; script-src 'unsafe-inline' blob: ${directory}; connect-src ${directory}; font-src data: ${directory}; form-action 'none'; base-uri ${root}`;
      const anchor=document.createElement("base");anchor.href=base;document.head.prepend(csp,anchor);
      return "<!doctype html>"+document.documentElement.outerHTML;
    }
    function HtmlPreview({source,path,sessionId}) {
      const [base,setBase]=React.useState(""),[error,setError]=React.useState("");
      React.useEffect(()=>{
        const controller=new AbortController();setBase("");setError("");
        json(endpoint("meta",sessionId),{signal:controller.signal}).then(value=>{
          if(controller.signal.aborted)return;
          const token=value.siteToken||value.token;if(typeof token!=="string"||!token)throw new Error("HTML 预览授权不可用");
          const origin=new URL(window.location.href);if(origin.hostname==="127.0.0.1")origin.hostname="localhost";else if(origin.hostname==="localhost")origin.hostname="127.0.0.1";
          const relative=path.replaceAll("\\","/").split("/").map(encodeURIComponent).join("/");
          setBase(origin.origin+"/__dsh-preview/site/"+encodeURIComponent(token)+"/"+encodeURIComponent(sessionId)+"/"+relative);
        }).catch(reason=>{if(!controller.signal.aborted)setError(reason.message||String(reason))});
        return()=>controller.abort();
      },[path,sessionId]);
      const packed=React.useMemo(()=>base?isolatedHtml(source,base):"",[source,base]);
      return error?h("div",{className:"dswSuiteStatus dswSuiteError",role:"alert"},error):base?h("iframe",{className:"dswSuiteHtml",sandbox:"allow-scripts",srcDoc:packed,title:"预览 "+path,referrerPolicy:"no-referrer"}):h("div",{className:"dswSuiteStatus",role:"status"},"正在准备 HTML 预览…");
    }
    function installStyle() {
      if (document.querySelector('style[data-plugin-css="dsh-sidebar-workbench-suite"]')) return;
      const style = document.createElement("style");
      style.dataset.pluginCss = "dsh-sidebar-workbench-suite";
      style.textContent = ".dswSuite{box-sizing:border-box;min-width:0;min-height:0;height:100%;display:flex;flex-direction:column;color:var(--dsw-alias-label-primary);background:var(--dsw-alias-bg-base)}.dswSuite *{box-sizing:border-box}.dswSuiteBar{min-height:42px;display:flex;align-items:center;gap:6px;padding:6px 10px;border-bottom:1px solid var(--dsw-alias-border-l2);flex-wrap:wrap}.dswSuiteTitle{font-weight:600;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;margin-right:auto}.dswSuiteEditor{min-height:0;flex:1;display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr)}.dswSuiteSource{min-width:0;min-height:0;display:flex;border-right:1px solid var(--dsw-alias-border-l2)}.dswSuiteSource textarea{width:100%;min-height:0;resize:none;border:0;outline:0;background:var(--dsw-alias-bg-base);color:var(--dsw-alias-label-primary);padding:14px;font:12.5px/1.65 var(--ds-font-family-code);tab-size:2}.dswSuitePreview{min-width:0;min-height:0;overflow:auto;padding:16px 20px;line-height:1.7;overflow-wrap:anywhere}.dswSuitePreview pre{overflow:auto;padding:12px;border-radius:8px;background:var(--dsw-alias-markdown-code-block);font:12px/1.6 var(--ds-font-family-code)}.dswSuiteHtml{display:block;width:100%;height:100%;min-height:0;border:0;background:white}.dswSuiteOutline{max-width:240px;max-height:160px;overflow:auto;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;padding:4px}.dswSuiteOutline button{width:100%;display:block;text-align:left;border:0;background:none;color:var(--dsw-alias-label-secondary);padding:4px 6px;cursor:pointer;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.dswSuiteDiagram{width:100%;overflow:auto;border:1px solid var(--dsw-alias-border-l2);border-radius:10px;padding:10px;margin:10px 0;background:var(--dsw-alias-bg-layer-1)}.dswSuiteDiagram svg{display:block;min-width:420px;max-width:100%;height:auto}.dswSuiteStatus{padding:8px 12px;color:var(--dsw-alias-label-tertiary);font-size:12px}.dswSuiteError{color:var(--dsw-alias-state-error-primary)}.dswSuiteList{min-height:0;flex:1;overflow:auto;padding:8px}.dswSuiteRow{width:100%;display:grid;grid-template-columns:auto minmax(0,1fr) auto;gap:8px;align-items:start;text-align:left;border:1px solid transparent;border-radius:8px;background:none;color:inherit;padding:9px;cursor:pointer}.dswSuiteRow:hover,.dswSuiteRow[data-active=true]{background:var(--dsw-alias-interactive-bg-hover);border-color:var(--dsw-alias-border-l2)}.dswSuiteDot{width:8px;height:8px;margin-top:5px;border-radius:50%;background:var(--dsw-alias-label-caption)}.dswSuiteDot[data-live=true]{background:var(--dsw-alias-state-business-primary)}.dswSuiteDot[data-error=true]{background:var(--dsw-alias-state-error-primary)}.dswSuiteMeta{font-size:11px;color:var(--dsw-alias-label-tertiary)}.dswSuiteSplit{min-height:0;flex:1;display:grid;grid-template-columns:240px minmax(0,1fr)}.dswSuiteDetail{min-width:0;min-height:0;overflow:auto;padding:12px;border-left:1px solid var(--dsw-alias-border-l2);white-space:pre-wrap}.dswSuiteTableWrap{min-height:0;flex:1;overflow:auto}.dswSuiteTable{border-collapse:collapse;width:max-content;min-width:100%;font:12px/1.5 var(--ds-font-family-code)}.dswSuiteTable th,.dswSuiteTable td{padding:7px 9px;border:1px solid var(--dsw-alias-border-l2);text-align:left;max-width:420px;overflow-wrap:anywhere}.dswSuiteTable th{position:sticky;top:0;background:var(--dsw-alias-bg-layer-1)}.dswSuiteDownload{margin:auto;max-width:420px;text-align:center;padding:24px}.dswSuiteDownload a{display:inline-block;padding:8px 12px;border-radius:8px;background:var(--dsw-alias-interactive-bg-hover);color:var(--dsw-alias-label-link)}.dswSuiteBrowser{min-height:0;flex:1;display:grid;place-items:center;overflow:hidden;background:#111}.dswSuiteBrowser img{display:block;max-width:100%;max-height:100%;cursor:crosshair;user-select:none}.dswDesktopVideo{position:relative;min-height:0;flex:1;display:flex;flex-direction:column;background:var(--dsw-alias-bg-base)}.dswDesktopCanvas{min-height:0;flex:1;display:grid;place-items:center;overflow:hidden;position:relative;background:#111}.dswDesktopCanvas canvas{display:block;max-width:100%;max-height:100%;outline:none;touch-action:none;cursor:default}.dswDesktopCanvas[data-actual-size=true]{display:block;overflow:auto}.dswDesktopCanvas[data-actual-size=true] canvas{max-width:none;max-height:none;margin:auto}.dswDesktopCanvas:focus-within{outline:1px solid var(--dsw-alias-border-l2);outline-offset:-1px}.dswDesktopWaiting{position:absolute;color:#aaa;font-size:12px}.dswDesktopKeyboard{position:absolute;opacity:0;width:1px;height:1px;padding:0;border:0;resize:none;pointer-events:none;left:50%;top:50%}.dswDesktopStatus{min-height:34px;display:flex;align-items:center;justify-content:space-between;gap:8px;padding:4px 10px;border-top:1px solid var(--dsw-alias-border-l2);font-size:11px;color:var(--dsw-alias-label-tertiary)}.dswDesktopVideo:fullscreen{width:100vw;height:100vh}.dswDesktopVideo:fullscreen .dswDesktopCanvas{width:100%;height:100%}.dswSuiteBrowserEmpty{color:#ccc;text-align:center;padding:24px}@media(max-width:768px){.dswSuiteEditor,.dswSuiteSplit{grid-template-columns:minmax(0,1fr)}.dswSuiteSource{border-right:0;border-bottom:1px solid var(--dsw-alias-border-l2);min-height:240px}.dswSuitePreview{min-height:240px}.dswSuiteSplit>.dswSuiteList{max-height:180px}.dswSuiteDetail{border-left:0;border-top:1px solid var(--dsw-alias-border-l2)}.dswSuiteBar input{min-width:0;flex:1}.dswSuiteOutline{max-width:100%;width:100%}}";
      style.textContent += ".dswSuiteSettings{border:1px solid var(--dsw-alias-border-l2);border-radius:14px;padding:16px;color:var(--dsw-alias-label-primary);background:var(--dsw-alias-bg-layer-1)}.dswSuiteSettings h3{margin:0 0 6px;font-size:14px}.dswSuiteSettings>p{margin:0 0 10px;color:var(--dsw-alias-label-tertiary);font-size:12px}.dswSuiteSetting{display:grid;grid-template-columns:minmax(0,1fr) minmax(120px,220px);gap:12px;align-items:center;padding:10px 0;border-top:1px solid var(--dsw-alias-border-l2)}.dswSuiteSetting span{font-size:13px}.dswSuiteSetting input,.dswSuiteSetting select{font:inherit;min-height:32px;border:1px solid var(--dsw-alias-border-l2);border-radius:7px;color:inherit;background:var(--dsw-alias-bg-base);padding:4px 8px}.dswSuiteSettingControl{display:flex;gap:6px;justify-content:flex-end}.dswSuiteSettingControl input{min-width:0;width:100%}@media(max-width:520px){.dswSuiteSetting{grid-template-columns:minmax(0,1fr)}.dswSuiteSettingControl{justify-content:stretch}}";
      style.textContent += ".dswSuiteDiagnostics{margin:0 10px 8px;border:1px solid var(--dsw-alias-border-l2);border-radius:10px;background:var(--dsw-alias-bg-layer-1);overflow:hidden}.dswSuiteDiagnostics>summary{cursor:pointer;padding:8px 10px;font-size:12px;font-weight:600;list-style:none}.dswSuiteDiagnostics>summary::-webkit-details-marker{display:none}.dswSuiteDiagnostics>summary:before{content:'›';display:inline-block;margin-right:6px;transition:transform .15s}.dswSuiteDiagnostics[open]>summary:before{transform:rotate(90deg)}.dswSuiteDiagnosticBody{padding:0 10px 10px}.dswSuiteDiagnosticGrid{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:7px;margin:0 0 8px}.dswSuiteDiagnosticCard{min-width:0;padding:8px;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;background:var(--dsw-alias-bg-base)}.dswSuiteDiagnosticCard small{display:block;color:var(--dsw-alias-label-tertiary);font-size:10px;margin-bottom:3px}.dswSuiteDiagnosticCard strong{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:12px}.dswSuiteDiagnosticCard[data-state=ok]{border-color:var(--dsw-alias-state-business-primary)}.dswSuiteDiagnosticCard[data-state=error]{border-color:var(--dsw-alias-state-error-primary)}.dswSuiteDiagnosticDetails{display:grid;grid-template-columns:minmax(90px,150px) minmax(0,1fr);gap:4px 10px;margin:0;font-size:11px}.dswSuiteDiagnosticDetails dt{color:var(--dsw-alias-label-tertiary)}.dswSuiteDiagnosticDetails dd{margin:0;overflow-wrap:anywhere}.dswSuiteDiagnosticRaw{max-height:140px;overflow:auto;margin:8px 0 0;padding:7px;border-radius:6px;background:var(--dsw-alias-markdown-code-block);font:10px/1.45 var(--ds-font-family-code);white-space:pre-wrap}.dswSuiteAnnotationToolbar{position:absolute;top:8px;right:8px;z-index:4;display:flex;gap:4px}.dswSuiteAnnotationToolbar input{width:150px;min-width:0;padding:3px 6px;border:1px solid rgba(255,255,255,.35);border-radius:6px;background:rgba(20,20,20,.78);color:#fff;font-size:11px}.dswSuiteAnnotationToolbar input::placeholder{color:#ccc}.dswSuiteAnnotationToolbar button{font-size:11px;padding:3px 7px;border:1px solid rgba(255,255,255,.35);border-radius:6px;background:rgba(20,20,20,.78);color:#fff;cursor:pointer}.dswSuiteAnnotationToolbar button:disabled{opacity:.5;cursor:default}@media(max-width:520px){.dswSuiteDiagnosticDetails{grid-template-columns:1fr}.dswSuiteDiagnosticDetails dd{margin-bottom:3px}}";
      document.head.appendChild(style);
    }
    function inlineNodes(text, key) {
      const parts = [], pattern = /(\x60[^\x60]+\x60|\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)|\*\*([^*]+)\*\*)/g;
      let at = 0, match;
      while ((match = pattern.exec(text))) {
        if (match.index > at) parts.push(text.slice(at, match.index));
        if (match[0][0] === "\x60") parts.push(h("code", { key: key + "-" + at }, match[0].slice(1, -1)));
        else if (match[2]) parts.push(h("a", { key: key + "-" + at, href: match[3], target: "_blank", rel: "noopener noreferrer" }, match[2]));
        else parts.push(h("strong", { key: key + "-" + at }, match[4]));
        at = pattern.lastIndex;
      }
      if (at < text.length) parts.push(text.slice(at));
      return parts;
    }
    function FallbackDiagram({ source }) {
      const lines = source.split(/\r?\n/).map(line => line.trim()).filter(Boolean);
      const sequence = /^sequenceDiagram\b/i.test(lines[0] || "");
      if (sequence) {
        const participants = [], events = [];
        for (const line of lines.slice(1)) {
          let match = /^(?:participant|actor)\s+([^\s]+)(?:\s+as\s+(.+))?$/i.exec(line);
          if (match) { if (!participants.some(item => item.id === match[1])) participants.push({ id: match[1], label: match[2] || match[1] }); continue; }
          match = /^([^\s-]+)\s*(-{1,2}>>?|-->>?)\s*([^:]+):\s*(.+)$/.exec(line);
          if (match) { for (const id of [match[1], match[3].trim()]) if (!participants.some(item => item.id === id)) participants.push({ id, label: id }); events.push({ from: match[1], to: match[3].trim(), text: match[4] }); }
        }
        const width = Math.max(440, participants.length * 150), height = Math.max(150, 80 + events.length * 60);
        return h("svg", { viewBox: "0 0 " + width + " " + height, role: "img", "aria-label": "Mermaid sequence diagram" },
          h("defs", null, h("marker", { id: "suiteSequenceArrow", markerWidth: 8, markerHeight: 8, refX: 7, refY: 4, orient: "auto" }, h("path", { d: "M0,0 L8,4 L0,8 z", fill: "currentColor" }))),
          participants.map((part, index) => h("g", { key: part.id }, h("rect", { x: index * 150 + 20, y: 10, width: 110, height: 30, rx: 5, fill: "none", stroke: "currentColor" }), h("text", { x: index * 150 + 75, y: 30, textAnchor: "middle", fill: "currentColor", fontSize: 12 }, part.label), h("line", { x1: index * 150 + 75, y1: 40, x2: index * 150 + 75, y2: height - 10, stroke: "currentColor", strokeDasharray: "4 4", opacity: .45 }))),
          events.map((event, index) => { const a = participants.findIndex(item => item.id === event.from) * 150 + 75, b = participants.findIndex(item => item.id === event.to) * 150 + 75, y = 70 + index * 60; return h("g", { key: index }, h("line", { x1: a, y1: y, x2: b, y2: y, stroke: "currentColor", markerEnd: "url(#suiteSequenceArrow)" }), h("text", { x: (a + b) / 2, y: y - 7, textAnchor: "middle", fill: "currentColor", fontSize: 11 }, event.text)); }));
      }
      const edges = [], nodes = new Map();
      for (const line of lines.slice(/^\s*(?:flowchart|graph)\b/i.test(lines[0] || "") ? 1 : 0)) {
        const match = /^([\w.-]+)(?:\[([^\]]+)\]|\(([^)]+)\)|\{([^}]+)\})?\s*(-->|---|==>)\s*([\w.-]+)(?:\[([^\]]+)\]|\(([^)]+)\)|\{([^}]+)\})?/.exec(line);
        if (!match) continue;
        nodes.set(match[1], match[2] || match[3] || match[4] || match[1]);
        nodes.set(match[6], match[7] || match[8] || match[9] || match[6]);
        edges.push([match[1], match[6]]);
      }
      const list = [...nodes.entries()], height = Math.max(150, list.length * 70);
      return h("svg", { viewBox: "0 0 560 " + height, role: "img", "aria-label": "Mermaid flowchart" },
        h("defs", null, h("marker", { id: "suiteFlowArrow", markerWidth: 8, markerHeight: 8, refX: 7, refY: 4, orient: "auto" }, h("path", { d: "M0,0 L8,4 L0,8 z", fill: "currentColor" }))),
        edges.map((edge, index) => { const a = list.findIndex(row => row[0] === edge[0]), b = list.findIndex(row => row[0] === edge[1]); return h("line", { key: "e" + index, x1: a % 2 ? 450 : 110, y1: a * 70 + 35, x2: b % 2 ? 450 : 110, y2: b * 70 + 35, stroke: "currentColor", markerEnd: "url(#suiteFlowArrow)" }); }),
        list.map((node, index) => h("g", { key: node[0] }, h("rect", { x: index % 2 ? 380 : 40, y: index * 70 + 12, width: 140, height: 44, rx: 8, fill: "var(--dsw-alias-bg-layer-1)", stroke: "currentColor" }), h("text", { x: index % 2 ? 450 : 110, y: index * 70 + 39, textAnchor: "middle", fill: "currentColor", fontSize: 12 }, node[1]))));
    }
    function MermaidDiagram({ source }) {
      const [state, setState] = React.useState({ svg: "", error: "" });
      React.useEffect(() => {
        let active = true;
        loadAsset("mermaid.js", "__DSH_SIDEBAR_MERMAID__").then(runtime => runtime.render("suite-mermaid-" + Math.random().toString(36).slice(2), source, document.body.hasAttribute("data-ds-dark-theme"))).then(svg => { if (active) setState({ svg, error: "" }); }).catch(error => { if (active) setState({ svg: "", error: error.message || String(error) }); });
        return () => { active = false; };
      }, [source]);
      if (state.svg) return h("div", { className: "dswSuiteDiagram", "data-mermaid-runtime": "loaded", dangerouslySetInnerHTML: { __html: state.svg } });
      return h("div", { className: "dswSuiteDiagram", title: state.error || "正在加载 Mermaid" }, h(FallbackDiagram, { source }));
    }
    const READING_STORAGE="dsh.documentReading.v1",readingPositions=new Map();
    try{for(const [key,value] of JSON.parse(window.sessionStorage?.getItem(READING_STORAGE)||"[]").slice(-128)){if(typeof key==="string"&&value&&typeof value==="object")readingPositions.set(key,value)}}catch{}
    function rememberReading(key,value){readingPositions.delete(key);readingPositions.set(key,value);while(readingPositions.size>128)readingPositions.delete(readingPositions.keys().next().value);try{window.sessionStorage?.setItem(READING_STORAGE,JSON.stringify([...readingPositions]))}catch{}}
    function CodeEditor({ value, path, stateKey=path, ready=true, onChange, onSave }) {
      const host = React.useRef(null), controller = React.useRef(null);
      const latest = React.useRef(value);
      const changeHandler=React.useRef(onChange);changeHandler.current=onChange;
      const [line,setLine]=React.useState(""),[wrap,setWrap]=React.useState(readingPositions.get(stateKey)?.wrap!==false);
      latest.current = value;
      const [fallback, setFallback] = React.useState(true);
      React.useLayoutEffect(() => {
        if(!ready){setFallback(true);return;}
        let active = true;
        loadAsset("editor.js", "__DSH_SIDEBAR_EDITOR__").then(runtime => {
          if (!active || !host.current) return;
          controller.current = runtime.mount({ parent: host.current, value: latest.current, path, onChange:next=>changeHandler.current(next),position:readingPositions.get(stateKey),onViewportChange:position=>rememberReading(stateKey,position) });
          setFallback(false);
        }).catch(() => { if (active) setFallback(true); });
        const save=()=>{const position=controller.current?.getPosition?.();if(position)rememberReading(stateKey,position)};
        window.addEventListener("pagehide",save);
        return () => { active = false;save();window.removeEventListener("pagehide",save);controller.current?.destroy(); controller.current = null; };
      }, [path,stateKey,ready]);
      React.useEffect(() => { controller.current?.setValue(value); }, [value]);
      return h("div", { className: "dswSuiteSource",style:{flexDirection:"column"}, onKeyDown: event => { if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") { event.preventDefault(); onSave(); } } },h("div",{className:"dswSuiteBar"},h("input",{type:"number",min:1,value:line,placeholder:"行号","aria-label":"跳转行号",style:{width:82},onChange:event=>setLine(event.target.value),onKeyDown:event=>{if(event.key==="Enter"){event.preventDefault();controller.current?.revealLine?.(line)}}}),h(Button,{variant:"outline",size:"sm",disabled:fallback||!line,onClick:()=>controller.current?.revealLine?.(line)},"跳转"),h(Button,{variant:"outline",size:"sm",disabled:fallback,"aria-pressed":wrap,onClick:()=>{setWrap(!wrap);controller.current?.setWrap?.(!wrap)}},"自动换行")), h("div", { ref: host, style: { minWidth: 0, minHeight: 0, flex: 1 }, "aria-label": "CodeMirror 编辑器 " + path }), fallback && h("textarea", { value, spellCheck: false, "aria-label": "编辑 " + path, onChange: event => onChange(event.target.value) }));
    }
    function MarkdownPreview({ source, outline, mermaidEnabled, fallback, stateKey }) {
      const host = React.useRef(null);
      const [rendered, setRendered] = React.useState(null);
      const position=React.useRef(readingPositions.get(stateKey));
      React.useLayoutEffect(()=>{if(host.current){host.current.scrollTop=position.current?.top||0;host.current.scrollLeft=position.current?.left||0}},[rendered,stateKey]);
      const onScroll=event=>{position.current={top:event.currentTarget.scrollTop,left:event.currentTarget.scrollLeft};rememberReading(stateKey,position.current)};
      React.useEffect(() => {
        let active = true;
        loadAsset("markdown.js", "__DSH_SIDEBAR_MARKDOWN__").then(runtime => runtime.render(source)).then(value => { if (active) setRendered(value); }).catch(() => { if (active) setRendered(null); });
        return () => { active = false; };
      }, [source]);
      React.useEffect(() => {
        if (!rendered || !host.current) return;
        const headings = host.current.querySelectorAll("h1,h2,h3,h4");
        headings.forEach((heading, index) => { if (outline[index]) heading.id = outline[index].id; });
        host.current.querySelectorAll("a[href]").forEach(link => { link.target = "_blank"; link.rel = "noopener noreferrer"; });
        const placeholders = [...host.current.querySelectorAll("[data-dsh-mermaid-index]")];
        if (!mermaidEnabled) {
          for (const placeholder of placeholders) {
            const code = document.createElement("code"), pre = document.createElement("pre");
            code.textContent = rendered.diagrams[Number(placeholder.dataset.dshMermaidIndex)] || "";
            pre.appendChild(code); placeholder.replaceChildren(pre);
          }
          return;
        }
        let active = true;
        loadAsset("mermaid.js", "__DSH_SIDEBAR_MERMAID__").then(async runtime => {
          for (const placeholder of placeholders) {
            if (!active) return;
            const index = Number(placeholder.dataset.dshMermaidIndex);
            try {
              placeholder.className = "dswSuiteDiagram";
              placeholder.innerHTML = await runtime.render("suite-rich-mermaid-" + index + "-" + Math.random().toString(36).slice(2), rendered.diagrams[index] || "", document.body.hasAttribute("data-ds-dark-theme"));
            } catch (error) {
              placeholder.textContent = error.message || "Mermaid 图表渲染失败";
              placeholder.className = "dswSuiteDiagram dswSuiteError";
            }
          }
        }).catch(error => {
          for (const placeholder of placeholders) { placeholder.textContent = error.message || "Mermaid 资源加载失败"; placeholder.className = "dswSuiteDiagram dswSuiteError"; }
        });
        return () => { active = false; };
      }, [rendered, mermaidEnabled, outline]);
      if (!rendered) return h("article", { ref:host,onScroll,className: "dswSuitePreview" }, fallback);
      return h("article", { ref: host,onScroll, className: "dswSuitePreview", "data-markdown-runtime": "marked", dangerouslySetInnerHTML: { __html: rendered.html } });
    }
    function renderMarkdown(source, outlineEnabled, mermaidEnabled) {
      const lines = source.split(/\r?\n/), content = [], outline = [];
      let paragraph = [], code = null, language = "", key = 0;
      const flush = () => { if (paragraph.length) { const text = paragraph.join(" "); content.push(h("p", { key: key++ }, inlineNodes(text, key))); paragraph = []; } };
      for (const line of lines) {
        const fence = /^\x60\x60\x60\s*([^\s]*)/.exec(line);
        if (fence) {
          if (code === null) { flush(); code = []; language = fence[1].toLowerCase(); }
          else { const text = code.join("\n"); content.push(language === "mermaid" && mermaidEnabled ? h("div", { className: "dswSuiteDiagram", key: key++ }, h(MermaidDiagram, { source: text })) : h("pre", { key: key++ }, h("code", { "data-language": language }, text))); code = null; language = ""; }
          continue;
        }
        if (code !== null) { code.push(line); continue; }
        const heading = /^(#{1,4})\s+(.+)$/.exec(line);
        if (heading) { flush(); const level = heading[1].length, id = "suite-heading-" + key; outline.push({ level, text: heading[2], id }); content.push(h("h" + level, { id, key: key++ }, inlineNodes(heading[2], key))); continue; }
        const item = /^\s*[-*+]\s+(.+)$/.exec(line);
        if (item) { flush(); content.push(h("ul", { key: key++ }, h("li", null, inlineNodes(item[1], key)))); continue; }
        if (!line.trim()) flush(); else paragraph.push(line.trim());
      }
      flush();
      return { content, outline: outlineEnabled ? outline : [] };
    }
    function MarkdownWorkbenchSession(props) {
      const [fileLoaded,setFileLoaded]=React.useState(false);
      const pluginSettings = props.pluginSettings || {};
      const draftKey = fileDraftKey("markdown", props.scope.sessionId, props.path), initialDraft = fileDrafts.get(draftKey);
      const [source, setSource] = React.useState(initialDraft?.source ?? props.content ?? ""), [saved, setSaved] = React.useState(initialDraft?.saved ?? props.content ?? ""), [etag, setEtag] = React.useState(initialDraft?.etag || "");
      const [mode, setMode] = React.useState("split"), [query, setQuery] = React.useState(""), [replacement, setReplacement] = React.useState(""), [status, setStatus] = React.useState(""), [error, setError] = React.useState("");
      const editSource = next => { setSource(next); const cached = rememberFileDraft(draftKey, next, saved, etag); setError(current => next !== saved && !cached ? FILE_DRAFT_WARNING : current === FILE_DRAFT_WARNING ? "" : current); };
      React.useEffect(() => {
        let active = true;
        fetch(endpoint("file", props.scope.sessionId, props.path)).then(async response => {
          if (!response.ok) throw new Error("HTTP " + response.status);
          const text = await response.text();
          if (active) applyFetchedFile(draftKey, text, response.headers.get("etag") || "", { source: setSource, saved: setSaved, etag: setEtag, error: setError });
        }).catch(reason => { if (active) setError(reason.message || String(reason)); }).finally(()=>{if(active)setFileLoaded(true)});
        return () => { active = false; };
      }, [draftKey]);
      const rendered = React.useMemo(() => renderMarkdown(source, pluginSettings.outline !== false, pluginSettings.mermaid !== false), [source, pluginSettings.outline, pluginSettings.mermaid]);
      const matches = query ? source.toLocaleLowerCase().split(query.toLocaleLowerCase()).length - 1 : 0;
      const save = async () => {
        try {
          setStatus("保存中"); setError("");
          const response = await fetch(endpoint("file-save", props.scope.sessionId, props.path), { method: "POST", headers: { "Content-Type": "text/plain; charset=utf-8", "If-Match": etag }, body: source });
          const value = await response.json().catch(() => ({}));
          if (!response.ok) throw new Error(value.message || "HTTP " + response.status);
          setEtag(value.etag || etag); setSaved(source); dropFileDraft(draftKey); setStatus("已保存");
        } catch (reason) { setError(reason.message || String(reason)); setStatus("保存失败"); }
      };
      const showSource = mode !== "preview", showPreview = mode !== "source";
      return h("section", { className: "dswSuite", "data-viewer": "markdown-workbench" },
        h("div", { className: "dswSuiteBar" },
          h("strong", { className: "dswSuiteTitle", title: props.path }, props.title),
          [["source", "源码"], ["preview", "预览"], ["split", "分栏"]].map(row => h(Button, { variant: "outline", size: "sm", key: row[0], "data-active": mode === row[0] || undefined, onClick: () => setMode(row[0]) }, row[1])),
          h("input", { value: query, placeholder: "查找", "aria-label": "在文档中查找", onChange: event => setQuery(event.target.value) }),
          h("span", { className: "dswSuiteMeta" }, matches + " 处"),
          h("input", { value: replacement, placeholder: "替换为", "aria-label": "替换文本", onChange: event => setReplacement(event.target.value) }),
          h(Button, { variant: "outline", size: "sm", disabled: !query, onClick: () => editSource(source.split(query).join(replacement)) }, "全部替换"),
          h(Button, { variant: "outline", size: "sm", disabled: source === saved || !etag, onClick: save }, status || "保存")),
        error && h("div", { className: "dswSuiteStatus dswSuiteError", role: "alert" }, error),
        rendered.outline.length > 0 && h("nav", { className: "dswSuiteOutline", "aria-label": "Markdown 大纲" }, rendered.outline.map(row => h(Button, { variant: "outline", size: "sm", key: row.id, style: { paddingLeft: 6 + row.level * 8 }, onClick: () => document.getElementById(row.id)?.scrollIntoView({ behavior: "smooth", block: "start" }) }, row.text))),
        h("div", { className: "dswSuiteEditor", style: mode === "split" ? undefined : { gridTemplateColumns: "minmax(0,1fr)" } },
          showSource && h(CodeEditor, { ready:fileLoaded,value: source, path: props.path,stateKey:fileDraftKey("editor",props.scope.sessionId,props.path), onChange: editSource, onSave: save }),
          showPreview && h(MarkdownPreview, { source,stateKey:fileDraftKey("markdown-position",props.scope.sessionId,props.path), outline: rendered.outline, mermaidEnabled: pluginSettings.mermaid !== false, fallback: rendered.content })));
    }
    function MarkdownWorkbench(props) {
      return h(MarkdownWorkbenchSession, { ...props, key: props.scope.sessionId + "\u0000" + props.path });
    }
    function parseCsv(source, separator) {
      const rows = []; let row = [], cell = "", quoted = false;
      for (let index = 0; index <= source.length; index++) {
        const char = source[index] || "\n";
        if (quoted) {
          if (char === '"' && source[index + 1] === '"') { cell += '"'; index++; }
          else if (char === '"') quoted = false;
          else cell += char;
        } else if (char === '"') quoted = true;
        else if (char === separator) { row.push(cell); cell = ""; }
        else if (char === "\n") { row.push(cell.replace(/\r$/, "")); rows.push(row); row = []; cell = ""; }
        else cell += char;
      }
      return rows.filter(cells => cells.some(cell => cell !== ""));
    }
    function CodeWorkbenchSession(props) {
      const [fileLoaded,setFileLoaded]=React.useState(false);
      const draftKey = fileDraftKey("code", props.scope.sessionId, props.path), initialDraft = fileDrafts.get(draftKey);
      const [source, setSource] = React.useState(initialDraft?.source ?? props.content ?? ""), [saved, setSaved] = React.useState(initialDraft?.saved ?? props.content ?? ""), [etag, setEtag] = React.useState(initialDraft?.etag || "");
      const [query, setQuery] = React.useState(""), [replacement, setReplacement] = React.useState(""), [error, setError] = React.useState(""), [status, setStatus] = React.useState("");
      const previewable = /\.(?:html?|svg)$/i.test(props.path);
      const [mode, setMode] = React.useState(previewable ? "split" : "source");
      const editSource = next => { setSource(next); const cached = rememberFileDraft(draftKey, next, saved, etag); setError(current => next !== saved && !cached ? FILE_DRAFT_WARNING : current === FILE_DRAFT_WARNING ? "" : current); };
      React.useEffect(() => setMode(previewable ? "split" : "source"), [props.path, previewable]);
      React.useEffect(() => {
        let active = true;
        fetch(endpoint("file", props.scope.sessionId, props.path)).then(async response => {
          if (!response.ok) throw new Error("HTTP " + response.status);
          const text = await response.text();
          if (active) applyFetchedFile(draftKey, text, response.headers.get("etag") || "", { source: setSource, saved: setSaved, etag: setEtag, error: setError });
        }).catch(reason => { if (active) setError(reason.message || String(reason)); }).finally(()=>{if(active)setFileLoaded(true)});
        return () => { active = false; };
      }, [draftKey]);
      const save = async () => {
        try {
          setStatus("保存中"); setError("");
          const response = await fetch(endpoint("file-save", props.scope.sessionId, props.path), { method: "POST", headers: { "Content-Type": "text/plain; charset=utf-8", "If-Match": etag }, body: source });
          const value = await response.json().catch(() => ({}));
          if (!response.ok) throw new Error(value.message || "HTTP " + response.status);
          setSaved(source); setEtag(value.etag || etag); dropFileDraft(draftKey); setStatus("已保存");
        } catch (reason) { setStatus("保存失败"); setError(reason.message || String(reason)); }
      };
      const matches = query ? source.toLocaleLowerCase().split(query.toLocaleLowerCase()).length - 1 : 0;
      return h("section", { className: "dswSuite", "data-viewer": "code-workbench" },
        h("div", { className: "dswSuiteBar" }, h("strong", { className: "dswSuiteTitle", title: props.path }, props.title), previewable && [["source", "源码"], ["preview", "预览"], ["split", "分栏"]].map(row => h(Button, { variant: "outline", size: "sm", key: row[0], onClick: () => setMode(row[0]) }, row[1])), h("input", { value: query, placeholder: "查找", onChange: event => setQuery(event.target.value) }), h("span", { className: "dswSuiteMeta" }, matches + " 处"), h("input", { value: replacement, placeholder: "替换为", onChange: event => setReplacement(event.target.value) }), h(Button, { variant: "outline", size: "sm", disabled: !query, onClick: () => editSource(source.split(query).join(replacement)) }, "全部替换"), h(Button, { variant: "outline", size: "sm", disabled: source === saved || !etag, onClick: save }, status || "保存")),
        error && h("div", { className: "dswSuiteStatus dswSuiteError" }, error),
        h("div", { className: "dswSuiteEditor", style: mode === "split" ? undefined : { gridTemplateColumns: "minmax(0,1fr)" } }, mode !== "preview" && h(CodeEditor, { ready:fileLoaded,value: source, path: props.path,stateKey:fileDraftKey("editor",props.scope.sessionId,props.path), onChange: editSource, onSave: save }), previewable && mode !== "source" && h(HtmlPreview,{source,path:props.path,sessionId:props.scope.sessionId})));
    }
    function CodeWorkbench(props) {
      return h(CodeWorkbenchSession, { ...props, key: props.scope.sessionId + "\u0000" + props.path });
    }
    function StructuredViewer(props) {
      const [source, setSource] = React.useState(props.content || ""), [saved, setSaved] = React.useState(props.content || ""), [etag, setEtag] = React.useState(""), [mode, setMode] = React.useState("table"), [saveError, setSaveError] = React.useState("");
      React.useEffect(() => {
        let active = true;
        fetch(endpoint("file", props.scope.sessionId, props.path)).then(async response => {
          if (!response.ok) throw new Error("HTTP " + response.status);
          const text = await response.text();
          if (active) { setSource(text); setSaved(text); setEtag(response.headers.get("etag") || ""); }
        }).catch(reason => { if (active) setSaveError(reason.message || String(reason)); });
        return () => { active = false; };
      }, [props.scope.sessionId, props.path]);
      const save = async () => {
        try {
          setSaveError("");
          const response = await fetch(endpoint("file-save", props.scope.sessionId, props.path), { method: "POST", headers: { "Content-Type": "text/plain; charset=utf-8", "If-Match": etag }, body: source });
          const value = await response.json().catch(() => ({}));
          if (!response.ok) throw new Error(value.message || "HTTP " + response.status);
          setSaved(source); setEtag(value.etag || etag);
        } catch (reason) { setSaveError(reason.message || String(reason)); }
      };
      let rows = [], error = "";
      try {
        if (/\.json$/i.test(props.path)) {
          const value = JSON.parse(source), list = Array.isArray(value) ? value : [value];
          const keys = [...new Set(list.flatMap(item => item && typeof item === "object" && !Array.isArray(item) ? Object.keys(item) : ["value"]))];
          rows = [keys, ...list.map(item => keys.map(key => {
            const value = key === "value" ? item : item && item[key];
            return value && typeof value === "object" ? JSON.stringify(value) : String(value == null ? "" : value);
          }))];
        } else rows = parseCsv(source, /\.tsv$/i.test(props.path) ? "\t" : ",");
      } catch (reason) { error = reason.message || String(reason); }
      const headers = rows[0] || [], body = rows.slice(1);
      return h("section", { className: "dswSuite", "data-viewer": "structured-data" },
        h("div", { className: "dswSuiteBar" }, h("strong", { className: "dswSuiteTitle" }, props.title), h(Button, { variant: "outline", size: "sm", onClick: () => setMode("table") }, "表格"), h(Button, { variant: "outline", size: "sm", onClick: () => setMode("source") }, "源码"), h("span", { className: "dswSuiteMeta" }, body.length + " 行 · " + headers.length + " 列"), h(Button, { variant: "outline", size: "sm", disabled: source === saved || !etag, onClick: save }, "保存")),
        (error || saveError) && h("div", { className: "dswSuiteStatus dswSuiteError" }, error || saveError),
        mode === "source" ? h("div", { className: "dswSuiteEditor", style: { gridTemplateColumns: "minmax(0,1fr)" } }, h(CodeEditor, { value: source, path: props.path,stateKey:fileDraftKey("editor",props.scope.sessionId,props.path), onChange: setSource, onSave: save })) : !error && h("div", { className: "dswSuiteTableWrap" }, h("table", { className: "dswSuiteTable" },
          h("thead", null, h("tr", null, headers.map((cell, index) => h("th", { key: index }, cell)))),
          h("tbody", null, body.map((row, rowIndex) => h("tr", { key: rowIndex }, headers.map((_, index) => h("td", { key: index }, row[index] || ""))))))));
    }
    function DownloadViewer(props) {
      return h("section", { className: "dswSuite" }, h("div", { className: "dswSuiteDownload" }, h("h3", null, props.title), h("p", null, "可下载后使用系统关联的本地应用打开。"), h("a", { href: endpoint("file", props.scope.sessionId, props.path), download: props.title }, "下载文件")));
    }
    function visiblePoll(run, initialDelay=1500) {
      let live=true,timer=0,inflight=false,controller=null,delay=initialDelay;
      const tick=async()=>{
        clearTimeout(timer);if(!live||document.hidden||inflight||delay===null)return;
        inflight=true;controller=new AbortController();
        try { delay=await run(controller.signal,delay); } catch(error) { if(error?.name!=="AbortError")delay=Math.min(15000,Math.max(3000,delay*2)); }
        finally { inflight=false;if(live&&!document.hidden&&delay!==null)timer=setTimeout(tick,delay); }
      };
      const visibility=()=>{clearTimeout(timer);if(document.hidden)controller?.abort();else void tick()};
      document.addEventListener("visibilitychange",visibility);void tick();
      return ()=>{live=false;clearTimeout(timer);controller?.abort();document.removeEventListener("visibilitychange",visibility)};
    }
    function JobsSessionTab(props) {
      const [jobs, setJobs] = React.useState([]), [selected, setSelected] = React.useState(""), [output, setOutput] = React.useState(""), [error, setError] = React.useState(""), [refreshSerial, setRefreshSerial] = React.useState(0);
      const alive = React.useRef(true), mutations = React.useRef(new Set());
      React.useEffect(() => { alive.current = true; return () => { alive.current = false; for (const controller of mutations.current) controller.abort(); mutations.current.clear(); }; }, []);
      React.useEffect(() => {
        if (!props.visible) return;
        let active=true,last="";
        const stop=visiblePoll(async(signal,delay)=>{
          try {
            const value=await json(endpoint("job-list",props.scope.sessionId),{signal});if(!active||signal.aborted)return delay;
            const entries=Array.isArray(value.entries)?value.entries:[],signature=JSON.stringify(entries);
            if(signature!==last){last=signature;setJobs(entries)}setError("");
            return entries.some(job=>["running","stopping"].includes(job.status))?1500:15000;
          }catch(reason){if(active&&reason?.name!=="AbortError")setError(reason.message||String(reason));throw reason}
        });
        return ()=>{active=false;stop()};
      }, [props.visible, props.scope.sessionId, refreshSerial]);
      React.useEffect(() => {
        if (!props.visible || !selected) return;
        let active=true,cursor=0;
        setOutput("");
        const stop=visiblePoll(async(signal,delay)=>{
          try{
            const value=await json(endpoint("job-read",props.scope.sessionId)+"&jobId="+encodeURIComponent(selected)+"&cursor="+cursor,{signal});
            if(!active||signal.aborted)return delay;
            if(value.text)setOutput(current=>(value.truncated?value.text:current+value.text).slice(-1024*1024));
            if(Number.isSafeInteger(value.cursor)&&value.cursor>=cursor)cursor=value.cursor;
            setError("");
            return value.snapshot&&["completed","killed","failed"].includes(value.snapshot.status)?null:value.text?1000:Math.min(5000,delay*1.5);
          }catch(reason){if(active&&reason?.name!=="AbortError")setError(reason.message||String(reason));throw reason}
        },1000);
        return ()=>{active=false;stop()};
      }, [props.visible, selected, props.scope.sessionId]);
      const kill = async id => {
        const controller = new AbortController(); mutations.current.add(controller);
        try { await json("/__dsh-preview/job-action", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ sessionId: props.scope.sessionId, action: "kill", jobId: id }), signal: controller.signal }); if (alive.current) setRefreshSerial(value => value + 1); }
        catch (reason) { if (alive.current && reason?.name !== "AbortError") setError(reason.message || String(reason)); }
        finally { mutations.current.delete(controller); }
      };
      return h("section", { className: "dswSuite", "data-tab": "background-jobs" },
        h("div", { className: "dswSuiteBar" }, h("strong", { className: "dswSuiteTitle" }, "后台任务"), h("span", { className: "dswSuiteMeta" }, jobs.filter(job => ["running", "stopping"].includes(job.status)).length + " 个运行中 · " + jobs.length + " 个总计"),h(Button,{variant:"outline",size:"sm",onClick:()=>setRefreshSerial(value=>value+1)},"刷新")),
        error && h("div", { className: "dswSuiteStatus dswSuiteError" }, error),
        h("div", { className: "dswSuiteSplit" },
          h("div", { className: "dswSuiteList" }, jobs.length ? jobs.map(job => h("div", { className: "dswSuiteRow", "data-active": selected === job.id || undefined, key: job.id, role: "button", tabIndex: 0, onClick: () => setSelected(job.id) }, h("span", { className: "dswSuiteDot", "data-live": ["running", "stopping"].includes(job.status) || undefined, "data-error": job.status === "failed" || undefined }), h("span", null, h("strong", null, job.label), h("div", { className: "dswSuiteMeta" }, job.kind + " · " + job.status + (job.detail ? " · " + job.detail : ""))), ["running", "stopping"].includes(job.status) && h(Button, { variant: "outline", size: "sm", onClick: event => { event.stopPropagation(); kill(job.id); } }, "终止"))) : h("div", { className: "dswSuiteStatus" }, "当前会话没有后台任务。")),
          h("pre", { className: "dswSuiteDetail" }, selected ? output || "等待输出…" : "选择任务查看实时输出。")));
    }
    function JobsTab(props) {
      return h(JobsSessionTab, { ...props, key: props.scope.sessionId + "\u0000" + props.tab.id });
    }
    const annotationStorageKey = (ownerId, sessionId) => `dsh-screen-annotations:v2:${encodeURIComponent(ownerId)}:${encodeURIComponent(sessionId)}`;
    function annotationFrame(element) {
      const isCanvas = element?.tagName === "CANVAS";
      const width = isCanvas ? element.width : element?.naturalWidth, height = isCanvas ? element.height : element?.naturalHeight;
      if (!width || !height || (!isCanvas && !element.complete)) throw new Error("当前画面尚未加载，请等待画面后提交批注");
      let source = element;
      if (!isCanvas) { source = document.createElement("canvas"); source.width = width; source.height = height; const context = source.getContext("2d"); if (!context) throw new Error("无法读取当前画面"); context.drawImage(element, 0, 0); }
      const data = source.toDataURL("image/jpeg", .85), match = /^data:(image\/(?:png|jpeg));base64,(.+)$/.exec(data);
      if (!match) throw new Error("无法生成批注截图");
      return { screenshot: { mediaType: match[1], base64: match[2] }, viewport: { width, height } };
    }
    /** Drafts are isolated by conversation and control session. Submission
     * sends normalized annotations with the frame visible at that instant. */
    function ScreenAnnotation(props) { return h(ScreenAnnotationSession, { ...props, key: props.storageKey }); }
    function ScreenAnnotationSession({ enabled, onChange, onSubmit, storageKey, frameRef }) {
      const readStored = () => { try { const value = JSON.parse(window.localStorage.getItem(storageKey) || "{}"); const validPoint = p => p && Number.isFinite(p.x) && Number.isFinite(p.y) && p.x >= 0 && p.x <= 1 && p.y >= 0 && p.y <= 1; return { strokes: Array.isArray(value.strokes) ? value.strokes.filter(stroke => Array.isArray(stroke) && stroke.length <= 1024 && stroke.every(validPoint)).slice(-64) : [], notes: Array.isArray(value.notes) ? value.notes.filter(row => row && typeof row.text === "string" && row.text.length <= 500).slice(-64) : [], note: typeof value.note === "string" ? value.note.slice(0, 500) : "" }; } catch { return { strokes: [], notes: [], note: "" }; } };
      const stored = React.useMemo(readStored, []);
      const [strokes, setStrokes] = React.useState(stored.strokes), [notes, setNotes] = React.useState(stored.notes), [note,setNote] = React.useState(stored.note), [draft,setDraft] = React.useState(null), active = React.useRef(null);
      const [submitting,setSubmitting] = React.useState(false), [message,setMessage] = React.useState(""), [failed,setFailed] = React.useState(false), [bounds,setBounds] = React.useState(null);
      const submittingRef = React.useRef(false), layer = React.useRef(null), mounted = React.useRef(true);
      React.useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
      React.useEffect(() => { const value = { strokes, notes, note }; try { window.localStorage.setItem(storageKey, JSON.stringify(value)); } catch { setFailed(true); setMessage("批注无法保存到本机，请勿关闭此页面"); } onChange?.({ strokes, notes }); }, [strokes, notes, note, storageKey]);
      React.useLayoutEffect(() => {
        const frame = frameRef?.current, parent = layer.current?.parentElement;
        if (!frame || !parent) return;
        const measure = () => { const image = frame.getBoundingClientRect(), box = parent.getBoundingClientRect(); const next = { left: image.left - box.left + parent.scrollLeft - parent.clientLeft, top: image.top - box.top + parent.scrollTop - parent.clientTop, width: image.width, height: image.height }; setBounds(old => old && Object.keys(next).every(key => old[key] === next[key]) ? old : next); };
        measure(); const observer = typeof window.ResizeObserver === "function" ? new window.ResizeObserver(measure) : null; observer?.observe(frame); observer?.observe(parent);
        window.addEventListener("resize",measure); document.addEventListener("scroll",measure,true);
        return () => { observer?.disconnect(); window.removeEventListener("resize",measure); document.removeEventListener("scroll",measure,true); };
      }, [frameRef, enabled]);
      const point = event => { const rect = event.currentTarget.getBoundingClientRect(); return { x: rect.width ? Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width)) : 0, y: rect.height ? Math.max(0, Math.min(1, (event.clientY - rect.top) / rect.height)) : 0 }; };
      const start = event => { if (!enabled || event.button !== 0) return; const p = point(event); active.current = [p]; setDraft([p]); event.preventDefault(); event.stopPropagation(); event.currentTarget.setPointerCapture?.(event.pointerId); };
      const move = event => { if (!active.current) return; const p = point(event); if (active.current.length < 1024) active.current.push(p); else active.current[1023] = p; setDraft([...active.current]); event.preventDefault(); };
      const end = event => { if (active.current) { const completed=active.current; setStrokes(value => [...value.slice(-63), completed]); active.current = null; setDraft(null); event?.currentTarget?.releasePointerCapture?.(event.pointerId); } };
      const clear = () => { active.current = null; setStrokes([]); setNotes([]); setDraft(null); setNote(""); setMessage(""); };
      const undo = () => setStrokes(value => value.slice(0, -1));
      const addNote = () => { const text = note.trim(); if (!text) return; setNotes(value => [...value.slice(-63), { text, at: Date.now() }]); setNote(""); };
      const copy = async () => { try { await navigator.clipboard?.writeText(JSON.stringify({ strokes, notes }, null, 2)); } catch {} };
      const submit = async () => { if (submittingRef.current) return; submittingRef.current = true; setSubmitting(true); setMessage(""); setFailed(false); try { const annotations = { strokes, notes }; if (new TextEncoder().encode(JSON.stringify(annotations)).length > 48 * 1024) throw new Error("批注内容过多，请撤销部分线条后重试"); if (!onSubmit) throw new Error("当前会话无法提交批注"); await onSubmit(annotations); if (mounted.current) setMessage("批注已发送到当前对话"); } catch (error) { if (mounted.current) { setFailed(true); setMessage(error.message || String(error)); } } finally { submittingRef.current = false; if (mounted.current) setSubmitting(false); } };
      return h("div", { className: "dswAnnotationLayer", ref: layer, "data-enabled": enabled || undefined, "data-stroke-count": strokes.length, "data-note-count": notes.length, "aria-label": enabled ? "画面注释层，拖动画线或添加文字批注" : undefined, style: { position: "absolute", ...(bounds || { inset: 0 }), pointerEvents: enabled ? "auto" : "none", zIndex: 3 } }, h("svg", { viewBox: "0 0 1 1", preserveAspectRatio: "none", "aria-hidden": true, style: { width: "100%", height: "100%", pointerEvents: "none" } }, (draft?[...strokes,draft]:strokes).map((stroke, i) => h("polyline", { key: i, points: stroke.map(p => `${p.x},${p.y}`).join(" "), fill: "none", stroke: "#ff3b30", strokeWidth: .004, strokeLinecap: "round", strokeLinejoin: "round" }))), enabled && h("div", { className: "dswAnnotationCapture", style: { position: "absolute", inset: 0, cursor: "crosshair" }, onPointerDown: start, onPointerMove: move, onPointerUp: end, onPointerCancel: end }), (enabled || strokes.length > 0 || notes.length > 0) && h("div", { className: "dswSuiteAnnotationToolbar", style: { pointerEvents: "auto" } }, enabled && h("input", { value: note, onChange: event => setNote(event.target.value), onKeyDown: event => { if (event.key === "Enter") addNote(); }, placeholder: "批注内容", "aria-label": "批注内容", maxLength: 500 }), enabled && h("button", { type: "button", onClick: addNote, disabled: !note.trim() }, "添加批注"), strokes.length > 0 && h("button", { type: "button", onClick: undo, "aria-label": "撤销注释" }, "撤销"), (strokes.length > 0 || notes.length > 0) && h("button", { type: "button", onClick: clear, "aria-label": "清除注释" }, "清除"), (strokes.length > 0 || notes.length > 0) && h("button", { type: "button", onClick: copy }, "复制批注"), (strokes.length > 0 || notes.length > 0) && h("button", { type: "button", onClick: submit, disabled: submitting }, submitting ? "正在提交…" : "提交批注"), message && h("span", { role: failed ? "alert" : "status", className: "dswSuiteMeta" }, message), enabled && h("span", { className: "dswSuiteMeta", style: { color: "#fff", padding: "3px 4px" } }, `${strokes.length} 条线 · ${notes.length} 条文字`)));
    }

    function DesktopVideo({ ownerId, sessionId, state, control, visible, playing, send, updateState, updateControl, reportError, api, video = true, snapshot = "", canActivate = false, annotate = false, onSubmit }) {
      const canvas = React.useRef(null), keyboard = React.useRef(null), root = React.useRef(null);
      const current = React.useRef(null); current.current = { state, control, send, updateState, updateControl, reportError, video, canActivate };
      const ready = React.useRef(false), alive = React.useRef(true), composing = React.useRef(false);
      const activating = React.useRef(false);
      const keys = React.useRef(new Set()), buttons = React.useRef(new Set()), queue = React.useRef([]), pumping = React.useRef(false), drain = React.useRef([]);
      const lastMove = React.useRef(0), pendingMove = React.useRef(null), moveTimer = React.useRef(null);
      const [hasFrame, setHasFrame] = React.useState(false), [fps, setFps] = React.useState(0), [focused, setFocused] = React.useState(false), [actualSize, setActualSize] = React.useState(false);
      const [activation,setActivation]=React.useState("");
      const connected = state?.connected === true;
      const updateMode = value => current.current.updateControl(previous => !previous || (value.generation ?? 0) >= (previous.generation ?? 0) ? value : previous);
      const pump = async () => {
        if (pumping.current) return;
        pumping.current = true;
        try {
          while (queue.current.length) {
            const next = queue.current.shift();
            const value = await current.current.send(next.action, { ...next.args, controlId: next.controlId, includeScreenshot: false }, { quiet: true, input: true });
            if (!value && next.action !== "release_inputs") { queue.current = [{ action: "release_inputs", args: {}, controlId: next.controlId }]; keys.current.clear(); buttons.current.clear(); }
          }
        } finally { pumping.current = false; for (const resolve of drain.current.splice(0)) resolve(); }
      };
      const enqueue = (action, args = {}) => {
        const controlId = current.current.state?.controlId;
        if (!controlId || (!ready.current && !["release_inputs", "key_up", "mouse_up"].includes(action))) return;
        const next = { action, args, controlId };
        if (action === "mouse_move" && queue.current.at(-1)?.action === "mouse_move") queue.current[queue.current.length - 1] = next;
        else if (queue.current.length < 64) queue.current.push(next);
        else { queue.current = [{ action: "release_inputs", args: {}, controlId }]; keys.current.clear(); buttons.current.clear(); current.current.reportError("输入队列已满，请稍后继续"); }
        void pump();
      };
      const release = () => {
        if (moveTimer.current) clearTimeout(moveTimer.current);
        moveTimer.current = null; pendingMove.current = null;
        composing.current = false;
        if (keyboard.current) keyboard.current.value = "";
        if (keys.current.size || buttons.current.size) { keys.current.clear(); buttons.current.clear(); enqueue("release_inputs"); }
        if (alive.current) setFocused(false);
      };
      const releaseAndFlush = () => { release(); return pumping.current || queue.current.length ? new Promise(resolve => drain.current.push(resolve)) : Promise.resolve(); };
      const needsActivation = () => !current.current.video && Number.isSafeInteger(current.current.state?.windowId) && current.current.state.foreground === false;
      const activateWindow = async () => {
        if(activating.current||current.current.video||!current.current.state?.windowId)return;
        if(!current.current.canActivate){current.current.reportError("请在 Host 电脑将目标窗口置于前台后重试");return;}
        activating.current=true;ready.current=false;setActivation("pending");
        await releaseAndFlush();
        const value=await current.current.send("focus_window",{controlId:current.current.state?.controlId,includeScreenshot:true});
        activating.current=false;
        if(alive.current)setActivation(value?.state?.foreground===true?"ready":"");
      };
      React.useEffect(() => {
        alive.current = true; api.current = { releaseAndFlush, activateWindow };
        const blur = () => release(); window.addEventListener("blur", blur);
        const unload = () => {
          const controlId = current.current.state?.controlId;
          if (controlId && (keys.current.size || buttons.current.size || pumping.current || queue.current.length)) {
            void fetch("/__dsh-computer-use/action", { method: "POST", headers: { "Content-Type": "application/json" }, keepalive: true, body: JSON.stringify({ ownerSessionId: ownerId, browserSessionId: sessionId, action: "release_inputs", controlId, includeScreenshot: false }) }).catch(() => {});
          }
          queue.current = []; keys.current.clear(); buttons.current.clear(); release();
        };
        window.addEventListener("pagehide", unload);
        return () => { unload(); alive.current = false; window.removeEventListener("blur", blur); window.removeEventListener("pagehide", unload); window.navigator.keyboard?.unlock?.(); api.current = null; };
      }, []);
      React.useEffect(() => { if (!visible || !playing || !connected || state?.interactive === false) { ready.current = false; release(); } }, [visible, playing, connected, state?.interactive]);
      React.useEffect(() => {
        if (video) return;
        if (activation === "pending") { ready.current = false; return; }
        if (!visible || !connected || state?.interactive === false || !snapshot) { ready.current = false; setHasFrame(false); return; }
        let cancelled = false;
        const image = new window.Image();
        image.onload = () => {
          if (cancelled || !canvas.current) return;
          const context = canvas.current.getContext("2d", { alpha: false, desynchronized: true });
          if (!context) { ready.current = false; setHasFrame(false); reportError("无法创建桌面显示区域"); return; }
          canvas.current.width = image.naturalWidth; canvas.current.height = image.naturalHeight;
          context.drawImage(image, 0, 0); ready.current = playing; setHasFrame(true);
        };
        image.onerror = () => { if (!cancelled) { ready.current = false; setHasFrame(false); release(); reportError("桌面画面不可用，请重新连接"); } };
        image.src = snapshot;
        return () => { cancelled = true; image.onload = null; image.onerror = null; image.src = ""; };
      }, [video, snapshot, visible, connected, playing, state?.interactive, state?.foreground, activation]);
      React.useEffect(() => {
        if (!video) return;
        ready.current = false; setHasFrame(false); setFps(0);
        if (!visible || !connected || !playing) { release(); return; }
        let cancelled = false, socket = null, decoder = null, configuration = null, needKey = true, stopRequested = false;
        let count = 0, measuredAt = window.performance.now();
        const closeDecoder = () => { const old = decoder; decoder = null; if (old && old.state !== "closed") { try { old.close(); } catch {} } };
        const fail = message => {
          if (cancelled) return;
          ready.current = false; setHasFrame(false); release(); current.current.reportError(message);
          current.current.updateState(previous => previous ? { ...previous, interactive: false } : previous);
          stopRequested = true; socket?.close(); closeDecoder();
        };
        if (typeof window.VideoDecoder !== "function" || typeof window.EncodedVideoChunk !== "function") { fail("当前浏览器不支持 H.264 实时视频解码"); return; }
        const context = canvas.current?.getContext("2d", { alpha: false, desynchronized: true });
        if (!context) { fail("无法创建视频显示区域"); return; }
        const configure = config => {
          closeDecoder(); needKey = true; configuration = config;
          const nextDecoder = new window.VideoDecoder({
            output(frame) {
              try {
                if (cancelled || decoder !== nextDecoder) return;
                if (canvas.current.width !== frame.displayWidth || canvas.current.height !== frame.displayHeight) { canvas.current.width = frame.displayWidth; canvas.current.height = frame.displayHeight; }
                context.drawImage(frame, 0, 0, canvas.current.width, canvas.current.height);
                ready.current = true; setHasFrame(true); count++;
                const now = window.performance.now();
                if (now - measuredAt >= 1000) { setFps(Math.round(count * 1000 / (now - measuredAt))); count = 0; measuredAt = now; }
              } finally { frame.close(); }
            },
            error() { fail("视频解码中断，请重新连接"); }
          });
          decoder = nextDecoder;
          decoder.configure({ ...config, optimizeForLatency: true, hardwareAcceleration: "prefer-hardware" });
        };
        const open = () => {
          if (document.hidden || cancelled || stopRequested) return;
          const url = new URL("/__dsh-computer-use/stream", window.location.href); url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
          url.searchParams.set("ownerSessionId", ownerId); url.searchParams.set("browserSessionId", sessionId);
          const connection = new window.WebSocket(url.href); socket = connection; connection.binaryType = "arraybuffer";
          connection.onmessage = event => {
            if (cancelled || socket !== connection) return;
            try {
              if (typeof event.data === "string") {
                const message = JSON.parse(event.data);
                if (message.kind === "error") { fail(message.message || "视频连接失败"); return; }
                if (message.kind !== "state") return;
                if (message.state) current.current.updateState(message.state);
                if (message.control) updateMode(message.control);
                const config = { codec: message.codec, codedWidth: message.width, codedHeight: message.height };
                if (!configuration || JSON.stringify(configuration) !== JSON.stringify(config)) configure(config);
                return;
              }
              if (!decoder || event.data.byteLength < 10 || event.data.byteLength > 4 * 1024 * 1024) return;
              const bytes = new Uint8Array(event.data), key = bytes[0] === 1;
              if (decoder.decodeQueueSize > 2) { configure(configuration); connection.send("keyframe"); }
              if (needKey && !key) return;
              needKey = false;
              const view = new DataView(event.data); const timestamp = Number(view.getBigUint64(1, true));
              decoder.decode(new window.EncodedVideoChunk({ type: key ? "key" : "delta", timestamp, data: bytes.subarray(9) }));
            } catch { fail("视频数据不可用，请重新连接"); }
          };
          connection.onerror = () => { if (!cancelled && !document.hidden && socket === connection) fail("视频通道连接失败，请重新连接"); };
          connection.onclose = () => { if (!cancelled && !document.hidden && !stopRequested && socket === connection) fail("视频连接已断开，请重新连接"); };
        };
        const visibility = () => {
          if (document.hidden) { ready.current = false; release(); const old = socket; socket = null; old?.close(); closeDecoder(); configuration = null; }
          else open();
        };
        open(); document.addEventListener("visibilitychange", visibility);
        return () => { cancelled = true; ready.current = false; document.removeEventListener("visibilitychange", visibility); const old = socket; socket = null; old?.close(); closeDecoder(); };
      }, [ownerId, sessionId, visible, connected, playing, video]);
      const point = event => {
        const rect = canvas.current.getBoundingClientRect(), viewport = current.current.state?.viewport;
        if (!viewport || !rect.width || !rect.height) return null;
        return { x: Math.max(0, Math.min(viewport.width - 1, (event.clientX - rect.left) * viewport.width / rect.width)), y: Math.max(0, Math.min(viewport.height - 1, (event.clientY - rect.top) * viewport.height / rect.height)) };
      };
      const mouseName = button => ["left", "middle", "right"][button];
      const focusKeyboard = () => { if (ready.current && keyboard.current) { keyboard.current.focus({ preventScroll: true }); setFocused(true); } };
      React.useEffect(() => { if (hasFrame && document.activeElement === canvas.current) focusKeyboard(); }, [hasFrame]);
      const flushMove = () => { if (pendingMove.current) { enqueue("mouse_move", pendingMove.current); pendingMove.current = null; } lastMove.current = window.performance.now(); moveTimer.current = null; };
      const move = event => {
        if (!ready.current || needsActivation() || activating.current || (current.current.control?.mode !== "manual" && !buttons.current.size && !focused)) return;
        pendingMove.current = point(event);
        if (!moveTimer.current) { const delay = Math.max(0, 16 - (window.performance.now() - lastMove.current)); moveTimer.current = setTimeout(flushMove, delay); }
      };
      const down = event => {
        if(needsActivation()&&connected&&playing&&hasFrame){event.preventDefault();event.stopPropagation();void activateWindow();return;}
        if (!ready.current || activating.current || !mouseName(event.button)) return;
        setActivation("");
        event.preventDefault(); event.currentTarget.setPointerCapture?.(event.pointerId);
        focusKeyboard();
        flushMove(); const position = point(event); if (!position) return;
        const button = mouseName(event.button); buttons.current.add(button); enqueue("mouse_down", { ...position, button });
      };
      const up = event => {
        const button = mouseName(event.button); if (!buttons.current.delete(button)) return;
        pendingMove.current = point(event); flushMove(); enqueue("mouse_up", { button });
        event.currentTarget.releasePointerCapture?.(event.pointerId);
      };
      const keyName = event => event.code || event.key;
      const keyDown = event => {
        if (event.key === "Escape") { event.preventDefault();event.stopPropagation();release(); window.navigator.keyboard?.unlock?.(); keyboard.current.blur(); if (document.fullscreenElement) void document.exitFullscreen?.(); void releaseAndFlush().then(() => current.current.send("takeover", { includeScreenshot: false })); return; }
        if (activating.current || !ready.current) { event.preventDefault();event.stopPropagation();composing.current=false;if(keyboard.current)keyboard.current.value="";return; }
        if (composing.current || event.nativeEvent?.isComposing || event.key === "Process" || event.keyCode === 229) return;
        if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "v") return;
        event.preventDefault(); event.stopPropagation();
        if(needsActivation()){void activateWindow();return;}
        setActivation("");
        const key = keyName(event); keys.current.add(key); enqueue("key_down", { key });
      };
      const keyUp = event => { const key = keyName(event); if (keys.current.delete(key)) { event.preventDefault(); event.stopPropagation(); enqueue("key_up", { key }); } };
      const paste = event => { const text = event.clipboardData?.getData("text/plain"); if (!text) return; event.preventDefault();if(!ready.current||activating.current){composing.current=false;keyboard.current.value="";return;}if(needsActivation()){void activateWindow();return;}release(); enqueue("type", { text: text.slice(0, 8192) }); keyboard.current.value = ""; focusKeyboard(); };
      const compositionStart = event => {
        composing.current=ready.current&&!activating.current&&!needsActivation();
        if(!composing.current){event.currentTarget.value="";if(ready.current&&needsActivation())void activateWindow();}
      };
      const compositionEnd = event => {
        const active=composing.current;composing.current=false;event.currentTarget.value="";
        if(!active||!ready.current||activating.current)return;
        if(needsActivation()){void activateWindow();return;}
        if(event.data)enqueue("type",{text:event.data.slice(0,8192)});
      };
      React.useEffect(() => {
        const element = canvas.current;
        const wheel = event => {
          if (!ready.current || activating.current) return;
          event.preventDefault(); event.stopPropagation();
          if(needsActivation()){void activateWindow();return;}
          if(moveTimer.current)clearTimeout(moveTimer.current);moveTimer.current=null;pendingMove.current=null;
          const position = point(event), atomicPosition = !current.current.video && current.current.canActivate;
          if (position && !atomicPosition) enqueue("mouse_move", position);
          const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? element.getBoundingClientRect().height : 1;
          enqueue("scroll", { ...(atomicPosition&&position?position:{}), deltaX: Math.max(-10000, Math.min(10000, event.deltaX * unit)), deltaY: Math.max(-10000, Math.min(10000, event.deltaY * unit)) });
        };
        element.addEventListener("wheel", wheel, { passive: false });
        return () => element.removeEventListener("wheel", wheel);
      }, []);
      const fullscreen = async () => { try { if (document.fullscreenElement) { window.navigator.keyboard?.unlock?.(); await document.exitFullscreen(); return; } await root.current.requestFullscreen?.(); if (document.fullscreenElement && window.navigator.keyboard?.lock) await window.navigator.keyboard.lock(); keyboard.current.focus({ preventScroll: true }); setFocused(true); } catch { current.current.reportError("浏览器未开启全屏键盘捕获"); } };
      return h("div", { className: "dswDesktopVideo", ref: root, "data-video-ready": hasFrame || undefined },
        h("div", { className: "dswDesktopCanvas", "data-actual-size": actualSize || undefined }, h("canvas", { ref: canvas, width: 1920, height: 1080, tabIndex: 0, role: "application", "aria-label": (video ? "远程桌面" : "本机窗口") + "：点击后直接使用键盘和鼠标", style: { visibility: hasFrame ? "visible" : "hidden" }, onFocus: focusKeyboard, onPointerDown: down, onPointerUp: up, onPointerMove: move, onPointerCancel: release, onLostPointerCapture: () => { if (buttons.current.size) release(); }, onContextMenu: event => event.preventDefault() }), !hasFrame && h("div", { className: "dswDesktopWaiting", role: "status" }, connected ? video ? "正在连接实时视频…" : "正在获取桌面画面…" : "连接桌面后显示画面"), h(ScreenAnnotation, { enabled: annotate && hasFrame, frameRef: canvas, onSubmit: annotations => { if (!hasFrame || !connected || !visible) throw new Error("当前桌面画面不可用，请重新连接后提交批注"); return onSubmit(annotations, annotationFrame(canvas.current)); }, storageKey: annotationStorageKey(ownerId, sessionId) })),
        h("textarea", { ref: keyboard, tabIndex: -1, "aria-label": "桌面键盘输入", className: "dswDesktopKeyboard", autoComplete: "off", spellCheck: false, onKeyDown: keyDown, onKeyUp: keyUp, onBlur: release, onPaste: paste, onCompositionStart: compositionStart, onCompositionEnd: compositionEnd }),
        h("div", { className: "dswDesktopStatus" }, h("span", null, !playing?"画面已暂停，恢复后可操作":activation==="pending"?"正在激活目标窗口…":needsActivation()?"目标窗口在后台，点击画面激活":activation==="ready"?"目标窗口已激活，请继续操作":focused ? "正在操作桌面 · Esc 释放键鼠" : "点击画面后直接使用键盘和鼠标"), h("span", null, hasFrame ? `${state?.viewport?.width || 0}×${state?.viewport?.height || 0}` + (video ? ` · ${fps} 帧/秒` : "") : ""), h(Button, { variant: "ghost", size: "sm", "aria-pressed": actualSize, onClick: () => setActualSize(value => !value) }, actualSize ? "适应窗口" : "100%"), h(Button, { variant: "ghost", size: "sm", onClick: fullscreen }, "全屏")));
    }

    function controlPauseText(control, state) {
      const reason = control?.pauseReason || state?.controlDiagnostics?.pauseReason;
      return ({
        "local-physical-input": "检测到本机键鼠输入（包括其他窗口）",
        "external-injected-input": "检测到其他程序注入键鼠输入",
        "escape-hotkey": "已触发全局急停快捷键",
        "gui-input": "控制画面收到人工输入",
        "gui-takeover": "已在控制面板选择人工接管",
        "start-human": "连接以人工控制模式启动",
        "resume-pending": "控制权尚未交还",
        "release-failed": "键鼠释放未完成",
        "emergency-stop-unavailable": "全局急停快捷键不可用",
        "reader-shutdown": "控制进程已断开",
        "connection-closing": "控制连接正在关闭"
      })[reason] || "暂停来源尚未确认";
    }
    function RuntimeDiagnostics({ meta, state, control, error, image, desktop, sdkDesktop, annotate, liveDesktop }) {
      const [copied, setCopied] = React.useState(false);
      const connected = state?.connected === true;
      const phase = state?.phase || (state ? "unknown" : "idle");
      const status = error ? "error" : connected && state?.interactive !== false ? "ok" : state ? "warn" : "idle";
      const statusText = error ? "错误" : connected && state?.interactive !== false ? "正常" : state ? "等待" : "未连接";
      const diagnostic = { adapter: meta?.adapter || null, phase, connected, interactive: state?.interactive !== false, control: control || null, viewport: state?.viewport || null, windowId: state?.windowId ?? null, foreground: state?.foreground ?? null, frame: { available: !!image, live: !!(desktop && sdkDesktop && liveDesktop), annotated: !!annotate }, error: error || null, capabilities: Array.isArray(meta?.actions) ? meta.actions : [] };
      const copy = async () => {
        try { await window.navigator?.clipboard?.writeText(JSON.stringify(diagnostic, null, 2)); setCopied(true); setTimeout(() => setCopied(false), 1600); }
        catch { setCopied(false); }
      };
      return h("details", { className: "dswSuiteDiagnostics", "data-status": status },
        h("summary", { "aria-label": "运行诊断" }, "运行诊断", h("span", { className: "dswSuiteMeta", style: { marginLeft: 8 } }, statusText)),
        h("div", { className: "dswSuiteDiagnosticBody" },
          h("div", { className: "dswSuiteDiagnosticGrid" },
            h("div", { className: "dswSuiteDiagnosticCard", "data-state": status === "ok" ? "ok" : status === "error" ? "error" : undefined }, h("small", null, "连接"), h("strong", null, connected ? "已连接" : state ? "连接中" : "未连接")),
            h("div", { className: "dswSuiteDiagnosticCard" }, h("small", null, "适配器"), h("strong", null, meta?.adapter || "未选择")),
            h("div", { className: "dswSuiteDiagnosticCard" }, h("small", null, "控制权"), h("strong", null, control?.mode === "manual" ? "人工接管" : control?.mode === "agent" ? "智能体" : "未建立")),
            h("div", { className: "dswSuiteDiagnosticCard" }, h("small", null, "画面"), h("strong", null, state?.viewport ? `${state.viewport.width}×${state.viewport.height}` : "未收到"))
          ),
          h("dl", { className: "dswSuiteDiagnosticDetails" },
            h("dt", null, "连接阶段"), h("dd", null, phase),
            h("dt", null, "交互状态"), h("dd", null, state?.interactive === false ? "已暂停" : connected ? "可操作" : "不可操作"),
            h("dt", null, "窗口焦点"), h("dd", null, state?.foreground === true ? "前台" : state?.foreground === false ? "后台" : "不适用"),
            h("dt", null, "暂停原因"), h("dd", null, controlPauseText(control, state)),
            h("dt", null, "控制代次"), h("dd", null, Number.isSafeInteger(control?.generation) ? String(control.generation) : "未提供"),
            h("dt", null, "急停快捷键"), h("dd", null, state?.emergencyStopShortcut || "未提供"),
            h("dt", null, "能力"), h("dd", null, Array.isArray(meta?.actions) && meta.actions.length ? meta.actions.join("、") : "未声明"),
            h("dt", null, "注释层"), h("dd", null, annotate ? "已启用（本地）" : "关闭")
          ),
          error && h("div", { className: "dswSuiteStatus dswSuiteError", role: "alert" }, error),
          h("div", { className: "dswSuiteToolbar", style: { justifyContent: "flex-end", marginTop: 8 } }, h(Button, { variant: "outline", size: "sm", onClick: copy }, copied ? "已复制" : "复制诊断")),
          h("pre", { className: "dswSuiteDiagnosticRaw" }, JSON.stringify(diagnostic, null, 2))
        )
      );
    }
    function ControlledBrowserSession(props) {
      const browserSessionId = props.browserSessionId;
      const desktopInput=React.useRef(null), browserImage=React.useRef(null);
      const autoRefresh = props.pluginSettings?.autoRefresh === true;
      const alive = React.useRef(true),closed=React.useRef(false);
      const lifetime = React.useRef(null); if (!lifetime.current) lifetime.current = new AbortController();
      const requestSequence = React.useRef(0), appliedSequence = React.useRef(0), foregroundRequests = React.useRef(0);
      React.useEffect(() => { alive.current = true; if (lifetime.current.signal.aborted) lifetime.current = new AbortController(); return () => { alive.current = false; lifetime.current.abort(); }; }, []);
      const [url, setUrl] = React.useState(props.tab.path || "about:blank"), [state, setState] = React.useState(null), [image, setImage] = React.useState(""), [typing, setTyping] = React.useState(""), [busy, setBusy] = React.useState(false), [error, setError] = React.useState(""), [meta, setMeta] = React.useState(null),[control,setControl]=React.useState(null),[privateInput,setPrivateInput]=React.useState(false),[liveDesktop,setLiveDesktop]=React.useState(true),[annotate,setAnnotate]=React.useState(false);
      const [windowsOpen,setWindowsOpen]=React.useState(false),[windows,setWindows]=React.useState(null),[selectedWindow,setSelectedWindow]=React.useState(null);
      const submitAnnotation = async (annotations, frame) => {
        if (!alive.current || !state?.connected || !frame?.screenshot || !frame?.viewport) throw new Error("当前控制会话没有可提交的画面");
        const result = await json("/__dsh-computer-use/annotation", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ ownerSessionId: props.scope.sessionId, browserSessionId, annotations, ...frame }), signal: lifetime.current.signal });
        if (result?.submitted !== true || result.ownerSessionId !== props.scope.sessionId || result.hasScreenshot !== true) throw new Error("批注尚未被当前对话确认接收，请重试");
        return result;
      };
      const sdkDesktop=meta?.adapter==="uu-desktop";
      const desktop=["native-desktop","uu-desktop"].includes(meta?.adapter),interactive=!!state&&state.interactive!==false&&state.connected!==false;
      const action = async (name, extra, options) => {
        const sequence = ++requestSequence.current, quiet = options?.quiet === true;
        if(name==="close")closed.current=true;else if(name==="start"){closed.current=false;setLiveDesktop(true)}
        try {
          if (!quiet) { foregroundRequests.current += 1; setBusy(true); setError(""); }
          // Desktop startup belongs to the control session, not this mounted
          // viewer. Explicit close still ends it through the Host; the Host's
          // bounded startup timeout handles a viewer that never returns.
          const sessionStart=name==="start"&&(desktop||options?.desktopStart===true);
          const request={ method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ ownerSessionId: props.scope.sessionId, browserSessionId, action: name, includeScreenshot: !sdkDesktop, ...(extra || {}) }), signal: options?.signal || lifetime.current.signal };
          const value = await (name==="start"?desktopStart(props.scope.sessionId,browserSessionId,request,sessionStart):name==="close"?desktopClose(props.scope.sessionId,browserSessionId,request):json("/__dsh-computer-use/action",request));
          if (alive.current && sequence >= appliedSequence.current) {
            appliedSequence.current = sequence;
            if(value.control){setControl(previous=>!previous||(value.control.generation??0)>=(previous.generation??0)?value.control:previous);if(name==="resume_agent")setTyping("");}
            if (value.state) { setState(value.state); if (value.state.url) setUrl(value.state.url); }
            if (value.screenshot?.base64) setImage("data:" + value.screenshot.mediaType + ";base64," + value.screenshot.base64);
            if (name === "close") { setState(null); setImage("");setControl(null);setTyping(""); }
          }
          return value;
        } catch (reason) { if(name==="capture"&&reason?.code==="COMPUTER_USE_CAPTURE_INTERRUPTED")return {};if(quiet&&!options?.input&&reason?.code==="COMPUTER_USE_MANUAL_CONTROL")return {};if (alive.current && sequence >= appliedSequence.current && reason?.name !== "AbortError") {appliedSequence.current=sequence;setError(String(reason.message || reason).split("; controlDiagnostics=")[0]);if(reason?.code==="COMPUTER_USE_EMERGENCY_STOP_UNAVAILABLE")setControl(previous=>({...previous,mode:"manual",pauseReason:"emergency-stop-unavailable"}));if(meta?.adapter==="native-desktop"&&String(reason.code||reason.message).startsWith("COMPUTER_USE_WINDOW_NOT_FOCUSED"))setState(previous=>previous?{...previous,foreground:false}:previous);if(desktop&&["start","capture"].includes(name)){setImage("");setLiveDesktop(false);setState(previous=>previous?{...previous,interactive:false}:previous);}} return null; }
        finally {
          if (!quiet) foregroundRequests.current = Math.max(0, foregroundRequests.current - 1);
          if (alive.current && !quiet && foregroundRequests.current === 0) setBusy(false);
        }
      };
      React.useEffect(() => {
        if (!props.visible) return;
        let active=true,stop=null;
        const controller=new AbortController();
        json("/__dsh-computer-use/meta", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ ownerSessionId: props.scope.sessionId }), signal: controller.signal }).then(async value => {
          if (!active) return;
          setMeta(value);
          if (!value.enabled) { setError("Computer Use 未启用，请在设置 → 插件 → Computer Use 中开启并重启 Host"); return; }
          if (value.available === false) { setError(value.error?.message || "Computer Use 执行器当前不可用，请检查浏览器或外部命令设置"); return; }
          if(closed.current)return;
          // Revealing an established desktop only resumes its video consumer.
          // Restarting the SDK here races the previous socket's asynchronous
          // input release and can cancel a healthy connection's new handshake.
          const reuseDesktop=["native-desktop","uu-desktop"].includes(value.adapter)&&state?.connected===true;
          const started=reuseDesktop?{state}:await action("start", {includeScreenshot:value.adapter!=="uu-desktop",...(props.tab.path && /^https?:/i.test(props.tab.path) ? { url: props.tab.path } : {})}, { signal: controller.signal, desktopStart:["native-desktop","uu-desktop"].includes(value.adapter) });
          if(active&&started&&autoRefresh&&!["native-desktop","uu-desktop"].includes(value.adapter))stop=visiblePoll(async signal=>{if(closed.current)return null;await action("capture",null,{quiet:true,signal});return 3000},3000);
        }).catch(reason => { if (active && reason?.name !== "AbortError") setError(reason.message || String(reason)); });
        return () => { active = false; controller.abort(); stop?.(); };
      }, [props.visible, props.scope.sessionId, browserSessionId, autoRefresh]);
      React.useEffect(()=>{
        if(meta?.adapter!=="native-desktop"||!props.visible||!state?.connected||!liveDesktop||busy)return;
        return visiblePoll(async signal=>{if(closed.current)return null;const value=await action("capture",null,{quiet:true,signal});return value?500:null},500);
      },[desktop,props.visible,state?.connected,liveDesktop,busy]);
      const reconnect=async()=>{const target=selectedWindow===null?state?.windowId:selectedWindow.windowId;await desktopInput.current?.releaseAndFlush();if(state){const value=await action("close",{includeScreenshot:false});if(!value)return;}await action("start",meta?.adapter==="native-desktop"&&Number.isSafeInteger(target)&&target>0?{windowId:target}:{});};
      const openWindows=async()=>{
        if(windowsOpen){setWindowsOpen(false);return;}
        setWindowsOpen(true);setWindows(null);
        const result=await action("list_windows",{includeScreenshot:false});
        if(alive.current)setWindows((Array.isArray(result?.windows)?result.windows:[]).filter(row=>Number.isSafeInteger(row.windowId)&&row.windowId>0&&typeof row.title==="string").slice(0,256));
      };
      React.useEffect(()=>{if(!props.visible)setWindowsOpen(false)},[props.visible]);
      const windowMenu=meta?.adapter==="native-desktop"&&meta.actions?.includes("list_windows")&&h(Menu,{
        open:windowsOpen,onClose:()=>setWindowsOpen(false),portal:true,
        anchor:h(Button,{variant:"outline",size:"sm",disabled:busy||!state?.connected,"aria-label":"选择本机窗口",onClick:openWindows},"选择窗口"),
        items:windows===null?[{id:"loading",label:"正在读取窗口…",disabled:true}]:[{id:"desktop",label:"主显示器"},...windows.map(row=>({id:String(row.windowId),label:row.title.slice(0,512)+(row.windowId===state?.windowId?" · 当前":"")})),...(windows.length?[]:[{id:"empty",label:"没有可用窗口",disabled:true}])],
        onSelect:id=>{const target=id==="desktop"?{windowId:null,title:"主显示器"}:windows?.find(row=>String(row.windowId)===id);if(target){setSelectedWindow(target);setWindowsOpen(false);}}
      });
      const navigate = () => {
        let target = url.trim();
        if (target && !/^https?:\/\//i.test(target)) target = (/^(?:localhost|127\.\d+\.\d+\.\d+|\[::1\])(?::\d+)?(?:[/?#]|$)/i.test(target) ? "http://" : "https://") + target;
        if (target) action("navigate", { url: target });
      };
      const point = event => {
        if (busy||!interactive||!state?.viewport) return;
        const rect = event.currentTarget.getBoundingClientRect();
        const x = Math.max(0, Math.min(Math.max(0,state.viewport.width-1), (event.clientX - rect.left) * state.viewport.width / rect.width));
        const y = Math.max(0, Math.min(Math.max(0,state.viewport.height-1), (event.clientY - rect.top) * state.viewport.height / rect.height));
        action(event.detail > 1 ? "double_click" : "click", { x, y });
      };
      const sendText=async()=>{if(!typing||busy||!interactive)return;const text=typing;const value=await action("type",{text});if(value)setTyping(current=>current===text?"":current);};
      const key=keys=>action("key",{keys});
      const dragStart=React.useRef(null),dragged=React.useRef(false);
      const pointerPoint=event=>{const rect=event.currentTarget.getBoundingClientRect();return {x:Math.max(0,Math.min(Math.max(0,state.viewport.width-1),(event.clientX-rect.left)*state.viewport.width/rect.width)),y:Math.max(0,Math.min(Math.max(0,state.viewport.height-1),(event.clientY-rect.top)*state.viewport.height/rect.height))}};
      return h("section", { className: "dswSuite", "data-tab": "controlled-browser", "data-browser-session": browserSessionId },
        h("div", { className: "dswSuiteBar" }, h(Button,{variant:"outline",size:"sm",disabled:busy,onClick:reconnect},state?"重新连接":"连接"),h(Button,{variant:"outline",size:"sm","aria-pressed":annotate,onClick:()=>setAnnotate(v=>!v)},annotate?"关闭注释":"注释画面"),windowMenu,windowMenu&&selectedWindow&&h("span",{className:"dswSuiteMeta"},selectedWindow.title.slice(0,512),(selectedWindow.windowId??null)===(state?.windowId??null)?"":" · 重新连接后切换"),h(Button,{variant:"outline",size:"sm",disabled:busy||!state,onClick:async()=>{await desktopInput.current?.releaseAndFlush();await action(control?.mode==="manual"?"resume_agent":"takeover",{includeScreenshot:false})}},control?.mode==="manual"?"交还智能体":"人工接管"),h("span",{className:"dswSuiteMeta",role:"status"},!interactive?state?.connected===false?"连接已断开":state?"等待桌面画面":"未连接":control?.mode==="manual"?"人工接管中 · 智能体控制暂停":"智能体可操作"),!desktop&&h(Button, { variant: "outline", size: "sm", disabled: busy||!state, onClick: () => action("click", { x: 0, y: 0, button: "back" }) }, "后退"), !desktop&&h("input", { value: url, "aria-label": "受控浏览器地址", onChange: event => setUrl(event.target.value), onKeyDown: event => { if (event.key === "Enter") navigate(); } }), !desktop&&h(Button, { variant: "outline", size: "sm", disabled: busy || !url.trim(), onClick: navigate }, "转到"),meta?.adapter==="native-desktop"&&state?.windowId&&meta.actions?.includes("focus_window")&&h(Button,{variant:"outline",size:"sm",disabled:busy||!state?.connected,onClick:()=>desktopInput.current?.activateWindow()},"激活窗口"),desktop&&h("strong",{className:"dswSuiteTitle"},state?.targetTitle||state?.title||(meta?.adapter==="uu-desktop"?"远程设备":"本机桌面")),desktop&&h(Button,{variant:"outline",size:"sm","aria-pressed":liveDesktop,disabled:!state?.connected,onClick:()=>setLiveDesktop(v=>!v)},sdkDesktop?"实时画面":"自动刷新"), !sdkDesktop&&h(Button, { variant: "outline", size: "sm", disabled: busy, onClick: () => action("capture") }, "刷新画面"), h(Button, { variant: "outline", size: "sm", disabled:busy, onClick: async () => {await desktopInput.current?.releaseAndFlush();await action("close", { includeScreenshot: false })} }, "关闭会话")),
        desktop && h("div", { className: "dswSuiteStatus", "data-control-scope": sdkDesktop ? "remote" : "local" }, sdkDesktop ? "UU 远程 · 全局急停 " + (state?.emergencyStopShortcut || "尚未确认") + "；画面内 Esc 释放键鼠。" : "本机桌面 · 与本机共用键鼠；操作其他窗口也会暂停智能体，避免争抢鼠标或将内容输入错误窗口。"),
        control?.mode === "manual" && h("div", { className: "dswSuiteStatus", role: "status", "data-control-pause": control.pauseReason || "unknown" }, "智能体控制暂停 · " + controlPauseText(control, state)),
        error && h("div", { className: "dswSuiteStatus dswSuiteError", role: "alert" }, error), h(RuntimeDiagnostics, { meta, state, control, error, image, desktop, sdkDesktop, annotate, liveDesktop }),
        desktop ? h(DesktopVideo,{video:sdkDesktop,snapshot:image,canActivate:meta?.actions?.includes("focus_window"),ownerId:props.scope.sessionId,sessionId:browserSessionId,state,control,visible:props.visible,playing:liveDesktop,send:action,updateState:setState,updateControl:setControl,reportError:setError,api:desktopInput,annotate,onSubmit:submitAnnotation}) : h("div", { className: "dswSuiteBrowser" }, image ? h("div", { className: "dswBrowserFrame", style: { position: "relative" } }, h("img", { ref: browserImage, src: image, alt: state?.title || "受控浏览器画面", draggable: false, tabIndex:0,onClick:event=>{if(dragged.current){dragged.current=false;return}point(event)}, "aria-label":"控制画面，操作即接管；画面聚焦时 Esc 暂停智能体",onKeyDown:event=>{if(event.key==="Escape"){event.preventDefault();void action("takeover",{includeScreenshot:false})}},onPointerDown:event=>{if(event.button!==0||busy||!interactive||!state?.viewport)return;dragStart.current=pointerPoint(event);event.currentTarget.setPointerCapture?.(event.pointerId)},onPointerUp:event=>{const start=dragStart.current;dragStart.current=null;if(!start||!state?.viewport)return;const end=pointerPoint(event);if(Math.hypot(end.x-start.x,end.y-start.y)>6){dragged.current=true;void action("drag",{...start,endX:end.x,endY:end.y});}},onPointerCancel:()=>{dragStart.current=null} }), h(ScreenAnnotation, { enabled: annotate, frameRef: browserImage, onSubmit: annotations => submitAnnotation(annotations, annotationFrame(browserImage.current)), storageKey: annotationStorageKey(props.scope.sessionId, browserSessionId) })) : h("div", { className: "dswSuiteBrowserEmpty" }, closed.current?"连接已关闭。点击连接重新打开。":error?"暂时无法显示画面":"正在连接并获取画面…")),
        !desktop&&h("div", { className: "dswSuiteBar" }, h("input", { type:privateInput?"password":"text",autoComplete:"off",maxLength:8192,value: typing, placeholder: "输入到当前焦点", "aria-label": "发送到受控浏览器", onFocus:()=>{if(state&&control?.mode!=="manual")void action("takeover",{includeScreenshot:false})}, onChange: event => setTyping(event.target.value), onKeyDown: event => { if(event.key==="Enter")sendText(); } }), h(Button, { variant: "outline", size: "sm", disabled: !typing||busy||!interactive, onClick: sendText }, "输入"),h(Button,{variant:"outline",size:"sm","aria-pressed":privateInput,onClick:()=>{setPrivateInput(v=>!v);if(state&&control?.mode!=="manual")void action("takeover",{includeScreenshot:false})}},"私密输入"),...["Enter","Tab","Backspace","Escape"].map(name=>h(Button,{key:name,variant:"outline",size:"sm",disabled:busy||!interactive,onClick:()=>key([name])},name)), h(Button, { variant: "outline", size: "sm", disabled:busy||!interactive,onClick: () => action("scroll", { deltaY: -540 }) }, "向上"), h(Button, { variant: "outline", size: "sm", disabled:busy||!interactive,onClick: () => action("scroll", { deltaY: 540 }) }, "向下"), h("span", { className: "dswSuiteMeta" }, (meta?.adapter || "未连接") + " · " + (state ? (state.title || state.url) + " · " + state.viewport.width + "×" + state.viewport.height : "会话 " + browserSessionId))));
    }
    function ControlledBrowserTab(props) {
      const sidebar=props.sidebar;
      React.useEffect(()=>{if(props.tab.title==="受控浏览器")sidebar?.updateTab?.(props.tab.id,{title:"Computer Use"},props.scope)},[sidebar,props.tab.id,props.tab.title,props.scope.sessionId]);
      const browserSessionId = props.tab.meta?.browserSessionId || "default";
      return h(ControlledBrowserSession, { ...props, browserSessionId, key: props.scope.sessionId + "\u0000" + props.tab.id + "\u0000" + browserSessionId });
    }
    function SettingRow({ scope, snapshot, field, error }) {
      const value = snapshot.value?.[field.key] ?? field.defaultValue;
      const [draft, setDraft] = React.useState(String(value));
      React.useEffect(() => setDraft(String(value)), [value]);
      const save = async next => { try { error(""); await scope.set(field.key, next); } catch (reason) { error(reason.message || String(reason)); } };
      let control;
      if (field.type === "switch") control = h(SettingsSwitch, { label: field.label, checked: value === true, disabled: !snapshot.writable, onChange: save });
      else if (field.type === "select") control = h("select", { "aria-label": field.label, value, disabled: !snapshot.writable, onChange: event => save(event.target.value) }, field.options.map(option => h("option", { key: option.value, value: option.value }, option.label)));
      else control = h("div", { className: "dswSuiteSettingControl" }, h("input", { "aria-label": field.label, type: field.type, min: field.min, max: field.max, value: draft, disabled: !snapshot.writable, onChange: event => setDraft(event.target.value), onKeyDown: event => { if (event.key === "Enter") save(field.type === "number" ? Number(draft) : draft); } }), h(Button, {variant:"outline",size:"sm", disabled: !snapshot.writable || draft === String(value), onClick: () => save(field.type === "number" ? Number(draft) : draft) }, "保存"));
      return h("div", { className: "dswSuiteSetting" }, h("span", null, field.label), control);
    }
    async function deviceRequest(action, payload={}) {
      const response=await fetch(`/__dsh-devices/${action}`,{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(payload)});
      const value=await response.json();if(!response.ok)throw new Error(value.message||`HTTP ${response.status}`);return value;
    }
    function DeviceSettings() {
      const [state,setState]=React.useState(null),[error,setError]=React.useState(""),[busy,setBusy]=React.useState(false);
      const alive=React.useRef(true),pending=React.useRef(false);
      const act=async(action,payload)=>{if(pending.current)return;pending.current=true;setBusy(true);setError("");try{if(action!=="status")await deviceRequest(action,payload);const next=await deviceRequest("status");if(alive.current)setState(next)}catch(error){if(alive.current)setError(error.message||String(error))}finally{pending.current=false;if(alive.current)setBusy(false)}};
      React.useEffect(()=>{alive.current=true;void act("status");return()=>{alive.current=false}},[]);
      return h("div",{className:"dswSuiteSettings","data-settings":"uu-devices"},h("h3",null,"远程设备"),
        h("p",null,"UU 远程桌面使用已绑定设备；本机桌面直接控制运行 Host 的电脑。"),
        !state&&h("p",{role:"status"},"正在读取设备…"),state?.message&&h("p",{role:"status"},state.message),
        state?.installed===false&&h("a",{href:"https://uuyc.163.com/",target:"_blank",rel:"noopener noreferrer",className:"dswSuiteButton"},"安装 UU 远程"),
        h("div",{className:"dswSuiteToolbar"},state?.installed&&h(Button,{variant:"outline",size:"sm",type:"button",disabled:busy,onClick:()=>void act("open-client")},state.signedIn?"打开 UU 远程":"打开 UU 远程并登录"),h(Button,{variant:"outline",size:"sm",type:"button",disabled:busy,onClick:()=>void act("status")},busy?"正在刷新…":"刷新设备")),
        state?.signedIn&&h("p",null,"当前账号：",state.account?.name),
        state?.signedIn&&!state.devices?.length&&h("p",null,"当前账号没有可用设备，请在被控设备安装 UU 远程并登录同一账号。"),
        (state?.devices||[]).map(device=>h("div",{className:"dswSuiteSetting",key:device.id},h("div",null,h("strong",null,device.name),h("p",null,device.local?"当前设备":device.online?"在线":"离线",state.boundDeviceId===device.id?" · 已绑定":"")),h("div",{className:"dswSuiteToolbar"},h(Button,{variant:"outline",size:"sm",type:"button",disabled:busy,onClick:()=>void act(state.boundDeviceId===device.id?"unbind":"bind",{deviceId:device.id})},state.boundDeviceId===device.id?"解除绑定":"绑定"),state.boundDeviceId===device.id&&h("span",{className:"dswSuiteMeta"},"已绑定远程设备")))),
        state?.installed&&h("p",null,"选择执行适配器后，在 Computer Use 工作台连接、查看画面和人工接管。UU 远程桌面由独立进程使用 SDK 控制绑定设备；本机桌面使用 Windows 原生接口。"),
        error&&h("p",{role:"alert",className:"dswSuiteError"},error));
    }
    function ComputerUseSettings({ scope }) {
      const snapshot = React.useSyncExternalStore(listener => scope.subscribe(listener), () => scope.getSnapshot(), () => scope.getSnapshot());
      const [error, setError] = React.useState("");
      const fields = [
        { key: "enabled", label: "启用 Computer Use", type: "switch", defaultValue: false },
        { key: "adapter", label: "执行适配器", type: "select", defaultValue: "auto", options: [{ value: "auto", label: "自动" }, { value: "native-browser", label: "内置浏览器" }, { value: "uu-desktop", label: "UU 远程桌面" }, { value: "native-desktop", label: "本机桌面（Rust 原生）" }, { value: "command", label: "外部命令" }] },
        { key: "browserExecutable", label: "浏览器可执行文件", type: "text", defaultValue: "" },
        { key: "browserHeadless", label: "后台运行浏览器", type: "switch", defaultValue: true },
        { key: "maxBrowserSessions", label: "最大浏览器会话数", type: "number", min: 1, max: 16, defaultValue: 4 },
        { key: "timeoutSeconds", label: "操作超时（秒）", type: "number", min: 5, max: 300, defaultValue: 60 },
        { key: "command", label: "外部控制命令", type: "text", defaultValue: "" }
      ];
      return h("section", { className: "dswSuiteSettings", "data-settings": "computer-use" }, h(DeviceSettings, {}), h("h3", null, "控制环境"), h("p", null, "模型与工作台共用控制环境。Windows 原生桌面和 UU 自连本机均共享这台电脑的桌面；需要同时使用本机其他软件时，可选择另一台 UU 设备或隔离浏览器。更换执行适配器后重启生效。"), snapshot.status !== "ready" ? h("div", { className: "dswSuiteStatus" }, snapshot.status === "error" ? (snapshot.error || "设置读取失败") : "正在读取设置…") : fields.map(field => h(SettingRow, { key: field.key, scope, snapshot, field, error: setError })), error && h("div", { className: "dswSuiteStatus dswSuiteError", role: "alert" }, error));
    }
    function apply(ctx) {
      installStyle();
      SettingsSwitch=ctx.settingsScope.controls.Switch;
      const sidebar = ctx.betterSidebar || ctx.get("betterSidebar");
      if (!sidebar) throw new Error("dsh-sidebar-workbench-suite requires betterSidebar");
      const computerUseScope = ctx.settingsScope.bind({ namespace: "computer-use", decode: value => value && typeof value === "object" && !Array.isArray(value) ? value : undefined });
      const disposers = [
        sidebar.registerFileViewer({ id: "suite:markdown", title: "Markdown 工作台", exts: ["md", "mdx", "markdown"], priority: 120, fetchStrategy: "fsRead", settings: { pluginToggles: [{ key: "outline", title: "显示 Markdown 大纲", type: "switch", defaultValue: true }, { key: "mermaid", title: "渲染 Mermaid 图表", type: "switch", defaultValue: true }] }, component: MarkdownWorkbench }),
        sidebar.registerFileViewer({ id: "suite:structured", title: "结构化数据表", exts: ["json", "csv", "tsv"], priority: 110, fetchStrategy: "fsRead", component: StructuredViewer }),
        sidebar.registerFileViewer({ id: "suite:office", title: "本地文档", exts: ["doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "zip", "7z", "rar"], priority: 100, fetchStrategy: "binary-download", component: DownloadViewer }),
        sidebar.registerFileViewer({ id: "suite:code", title: "CodeMirror 文本编辑器", exts: ["", "txt", "log", "js", "jsx", "mjs", "cjs", "ts", "tsx", "vue", "svelte", "rs", "py", "go", "java", "c", "cc", "cpp", "h", "hpp", "cs", "rb", "php", "sh", "bash", "zsh", "ps1", "sql", "yaml", "yml", "toml", "ini", "conf", "env", "xml", "css", "scss", "less", "html", "htm", "svg", "dockerfile", "makefile"], priority: 90, fetchStrategy: "fsRead", component: CodeWorkbench }),
        sidebar.registerFileViewer({id:"suite:pdf",title:"PDF 预览",exts:["pdf"],priority:130,fetchStrategy:"custom",load:loadPdfBytes,component:PdfViewer}),
        sidebar.registerFileViewer({id:"suite:image",title:"图片预览",exts:["png","jpg","jpeg","webp","gif","bmp","avif","ico"],priority:130,fetchStrategy:"mediaUrl",component:ImageViewer}),
        sidebar.registerTab({ id: "suite:jobs", title: "后台任务", order: 80, single: true, component: JobsTab }),
        sidebar.registerTab({ id: "suite:controlled-browser", title: "Computer Use", order: 100, single: true, component: props=>h(ControlledBrowserTab,{...props,sidebar}), settings: { pluginToggles: [{ key: "autoRefresh", title: "自动刷新浏览器画面", type: "switch", defaultValue: false }] }, onClose: (tab, scope) => { void desktopClose(scope.sessionId,tab.meta?.browserSessionId||"default",{ method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ ownerSessionId: scope.sessionId, browserSessionId: tab.meta?.browserSessionId || "default", action: "close", includeScreenshot: false }) }).catch(() => {}); } })
      ];
      ctx.slots.inject("settings.plugin.item", () => ctx.slots.register({ name: "settings.plugin.item", id: "computer-use", order: 40, label: "Computer Use" }, () => h("details", {className:"dshSettingsDisclosure"},h("summary",null,"Computer Use 与远程设备"),h(ComputerUseSettings, { scope: computerUseScope }))));
      ctx.effect?.(() => () => { clearFileDrafts(); for (const dispose of disposers.reverse()) dispose(); }, "sidebar-workbench-suite: registrations");
    }
    exports.apply = apply;
    exports.inject = inject;
    exports.test = { PdfViewer, ImageViewer, HtmlPreview, isolatedHtml, loadPdfBytes, parseCsv, renderMarkdown, visiblePoll, MermaidDiagram, MarkdownWorkbench, CodeWorkbench, StructuredViewer, JobsTab, ControlledBrowserTab, RuntimeDiagnostics, ScreenAnnotation, ComputerUseSettings, DeviceSettings, rememberFileDraft, fileDraftCacheSnapshot: () => ({ keys: [...fileDrafts.keys()], bytes: fileDraftBytes }), clearFileDrafts };
    return module.exports;
  }
});
})();
