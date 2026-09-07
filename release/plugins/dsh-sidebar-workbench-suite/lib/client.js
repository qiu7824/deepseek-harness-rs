(() => {
const suiteAssetBase = document.currentScript?.src.replace(/\.js(?:\?.*)?$/, "/") || "";
globalThis.__DSH_SIDEBAR_SUITE_ASSET_BASE__ = suiteAssetBase;
window.__ModuleLoader__.load({
  id: "dsh-sidebar-workbench-suite",
  factory: (require) => {
    const module = { exports: {} }, exports = module.exports;
    const React = require("react"), h = React.createElement;
    const Button=require("@deepseek-ai/dsh-client-ui-primitives").Button;
    let SettingsSwitch;
    const inject = ["betterSidebar", "connection", "sessions", "settingsScope", "slots"];
    const assetBase = suiteAssetBase;
    const assetVersions = globalThis.__DSH_SIDEBAR_SUITE_ASSET_VERSIONS__ ||= Object.create(null);
    const assetLoads = new Map();
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
      if (!response.ok) throw new Error(value.message || value.error || "HTTP " + response.status);
      return value;
    }
    function installStyle() {
      if (document.querySelector('style[data-plugin-css="dsh-sidebar-workbench-suite"]')) return;
      const style = document.createElement("style");
      style.dataset.pluginCss = "dsh-sidebar-workbench-suite";
      style.textContent = ".dswSuite{box-sizing:border-box;min-width:0;min-height:0;height:100%;display:flex;flex-direction:column;color:var(--dsw-alias-label-primary);background:var(--dsw-alias-bg-base)}.dswSuite *{box-sizing:border-box}.dswSuiteBar{min-height:42px;display:flex;align-items:center;gap:6px;padding:6px 10px;border-bottom:1px solid var(--dsw-alias-border-l2);flex-wrap:wrap}.dswSuiteBar input,.dswSuiteBar select,.dswSuiteBar button,.dswSuiteBar button:disabled,.dswSuiteTitle{font-weight:600;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;margin-right:auto}.dswSuiteEditor{min-height:0;flex:1;display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr)}.dswSuiteSource{min-width:0;min-height:0;display:flex;border-right:1px solid var(--dsw-alias-border-l2)}.dswSuiteSource textarea{width:100%;min-height:0;resize:none;border:0;outline:0;background:var(--dsw-alias-bg-base);color:var(--dsw-alias-label-primary);padding:14px;font:12.5px/1.65 var(--ds-font-family-code);tab-size:2}.dswSuitePreview{min-width:0;min-height:0;overflow:auto;padding:16px 20px;line-height:1.7;overflow-wrap:anywhere}.dswSuitePreview pre{overflow:auto;padding:12px;border-radius:8px;background:var(--dsw-alias-markdown-code-block);font:12px/1.6 var(--ds-font-family-code)}.dswSuiteHtml{display:block;width:100%;height:100%;min-height:0;border:0;background:white}.dswSuiteOutline{max-width:240px;max-height:160px;overflow:auto;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;padding:4px}.dswSuiteOutline button{width:100%;display:block;text-align:left;border:0;background:none;color:var(--dsw-alias-label-secondary);padding:4px 6px;cursor:pointer;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.dswSuiteDiagram{width:100%;overflow:auto;border:1px solid var(--dsw-alias-border-l2);border-radius:10px;padding:10px;margin:10px 0;background:var(--dsw-alias-bg-layer-1)}.dswSuiteDiagram svg{display:block;min-width:420px;max-width:100%;height:auto}.dswSuiteStatus{padding:8px 12px;color:var(--dsw-alias-label-tertiary);font-size:12px}.dswSuiteError{color:var(--dsw-alias-state-error-primary)}.dswSuiteList{min-height:0;flex:1;overflow:auto;padding:8px}.dswSuiteRow{width:100%;display:grid;grid-template-columns:auto minmax(0,1fr) auto;gap:8px;align-items:start;text-align:left;border:1px solid transparent;border-radius:8px;background:none;color:inherit;padding:9px;cursor:pointer}.dswSuiteRow:hover,.dswSuiteRow[data-active=true]{background:var(--dsw-alias-interactive-bg-hover);border-color:var(--dsw-alias-border-l2)}.dswSuiteDot{width:8px;height:8px;margin-top:5px;border-radius:50%;background:var(--dsw-alias-label-caption)}.dswSuiteDot[data-live=true]{background:var(--dsw-alias-state-business-primary)}.dswSuiteDot[data-error=true]{background:var(--dsw-alias-state-error-primary)}.dswSuiteMeta{font-size:11px;color:var(--dsw-alias-label-tertiary)}.dswSuiteSplit{min-height:0;flex:1;display:grid;grid-template-columns:240px minmax(0,1fr)}.dswSuiteDetail{min-width:0;min-height:0;overflow:auto;padding:12px;border-left:1px solid var(--dsw-alias-border-l2);white-space:pre-wrap}.dswSuiteTableWrap{min-height:0;flex:1;overflow:auto}.dswSuiteTable{border-collapse:collapse;width:max-content;min-width:100%;font:12px/1.5 var(--ds-font-family-code)}.dswSuiteTable th,.dswSuiteTable td{padding:7px 9px;border:1px solid var(--dsw-alias-border-l2);text-align:left;max-width:420px;overflow-wrap:anywhere}.dswSuiteTable th{position:sticky;top:0;background:var(--dsw-alias-bg-layer-1)}.dswSuiteDownload{margin:auto;max-width:420px;text-align:center;padding:24px}.dswSuiteDownload a{display:inline-block;padding:8px 12px;border-radius:8px;background:var(--dsw-alias-interactive-bg-hover);color:var(--dsw-alias-label-link)}.dswSuiteBrowser{min-height:0;flex:1;display:grid;place-items:center;overflow:hidden;background:#111}.dswSuiteBrowser img{display:block;max-width:100%;max-height:100%;cursor:crosshair;user-select:none}.dswSuiteBrowserEmpty{color:#ccc;text-align:center;padding:24px}@media(max-width:768px){.dswSuiteEditor,.dswSuiteSplit{grid-template-columns:minmax(0,1fr)}.dswSuiteSource{border-right:0;border-bottom:1px solid var(--dsw-alias-border-l2);min-height:240px}.dswSuitePreview{min-height:240px}.dswSuiteSplit>.dswSuiteList{max-height:180px}.dswSuiteDetail{border-left:0;border-top:1px solid var(--dsw-alias-border-l2)}.dswSuiteBar input{min-width:0;flex:1}.dswSuiteOutline{max-width:100%;width:100%}}";
      style.textContent += ".dswSuiteSettings{border:1px solid var(--dsw-alias-border-l2);border-radius:14px;padding:16px;color:var(--dsw-alias-label-primary);background:var(--dsw-alias-bg-layer-1)}.dswSuiteSettings h3{margin:0 0 6px;font-size:14px}.dswSuiteSettings>p{margin:0 0 10px;color:var(--dsw-alias-label-tertiary);font-size:12px}.dswSuiteSetting{display:grid;grid-template-columns:minmax(0,1fr) minmax(120px,220px);gap:12px;align-items:center;padding:10px 0;border-top:1px solid var(--dsw-alias-border-l2)}.dswSuiteSetting span{font-size:13px}.dswSuiteSetting input,.dswSuiteSetting select{font:inherit;min-height:32px;border:1px solid var(--dsw-alias-border-l2);border-radius:7px;color:inherit;background:var(--dsw-alias-bg-base);padding:4px 8px}.dswSuiteSettingControl{display:flex;gap:6px;justify-content:flex-end}.dswSuiteSettingControl input{min-width:0;width:100%}@media(max-width:520px){.dswSuiteSetting{grid-template-columns:minmax(0,1fr)}.dswSuiteSettingControl{justify-content:stretch}}";
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
    function CodeEditor({ value, path, onChange, onSave }) {
      const host = React.useRef(null), controller = React.useRef(null);
      const latest = React.useRef(value);
      latest.current = value;
      const [fallback, setFallback] = React.useState(true);
      React.useEffect(() => {
        let active = true;
        loadAsset("editor.js", "__DSH_SIDEBAR_EDITOR__").then(runtime => {
          if (!active || !host.current) return;
          controller.current = runtime.mount({ parent: host.current, value: latest.current, path, onChange });
          setFallback(false);
        }).catch(() => { if (active) setFallback(true); });
        return () => { active = false; controller.current?.destroy(); controller.current = null; };
      }, [path]);
      React.useEffect(() => { controller.current?.setValue(value); }, [value]);
      return h("div", { className: "dswSuiteSource", onKeyDown: event => { if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") { event.preventDefault(); onSave(); } } }, h("div", { ref: host, style: { minWidth: 0, minHeight: 0, flex: 1 }, "aria-label": "CodeMirror 编辑器 " + path }), fallback && h("textarea", { value, spellCheck: false, "aria-label": "编辑 " + path, onChange: event => onChange(event.target.value) }));
    }
    function MarkdownPreview({ source, outline, mermaidEnabled, fallback }) {
      const host = React.useRef(null);
      const [rendered, setRendered] = React.useState(null);
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
      if (!rendered) return h("article", { className: "dswSuitePreview" }, fallback);
      return h("article", { ref: host, className: "dswSuitePreview", "data-markdown-runtime": "marked", dangerouslySetInnerHTML: { __html: rendered.html } });
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
        }).catch(reason => { if (active) setError(reason.message || String(reason)); });
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
          showSource && h(CodeEditor, { value: source, path: props.path, onChange: editSource, onSave: save }),
          showPreview && h(MarkdownPreview, { source, outline: rendered.outline, mermaidEnabled: pluginSettings.mermaid !== false, fallback: rendered.content })));
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
        }).catch(reason => { if (active) setError(reason.message || String(reason)); });
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
        h("div", { className: "dswSuiteEditor", style: mode === "split" ? undefined : { gridTemplateColumns: "minmax(0,1fr)" } }, mode !== "preview" && h(CodeEditor, { value: source, path: props.path, onChange: editSource, onSave: save }), previewable && mode !== "source" && h("iframe", { className: "dswSuiteHtml", sandbox: "allow-scripts", srcDoc: source, title: "预览 " + props.path })));
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
        mode === "source" ? h("div", { className: "dswSuiteEditor", style: { gridTemplateColumns: "minmax(0,1fr)" } }, h(CodeEditor, { value: source, path: props.path, onChange: setSource, onSave: save })) : !error && h("div", { className: "dswSuiteTableWrap" }, h("table", { className: "dswSuiteTable" },
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
    function ControlledBrowserSession(props) {
      const browserSessionId = props.browserSessionId;
      const autoRefresh = props.pluginSettings?.autoRefresh === true;
      const alive = React.useRef(true);
      const lifetime = React.useRef(null); if (!lifetime.current) lifetime.current = new AbortController();
      const requestSequence = React.useRef(0), appliedSequence = React.useRef(0), foregroundRequests = React.useRef(0);
      React.useEffect(() => { alive.current = true; if (lifetime.current.signal.aborted) lifetime.current = new AbortController(); return () => { alive.current = false; lifetime.current.abort(); }; }, []);
      const [url, setUrl] = React.useState(props.tab.path || "about:blank"), [state, setState] = React.useState(null), [image, setImage] = React.useState(""), [typing, setTyping] = React.useState(""), [busy, setBusy] = React.useState(false), [error, setError] = React.useState(""), [meta, setMeta] = React.useState(null);
      const action = async (name, extra, options) => {
        const sequence = ++requestSequence.current, quiet = options?.quiet === true;
        try {
          if (!quiet) { foregroundRequests.current += 1; setBusy(true); setError(""); }
          const value = await json("/__dsh-computer-use/action", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ ownerSessionId: props.scope.sessionId, browserSessionId, action: name, includeScreenshot: true, ...(extra || {}) }), signal: options?.signal || lifetime.current.signal });
          if (alive.current && sequence >= appliedSequence.current) {
            appliedSequence.current = sequence;
            if (value.state) { setState(value.state); if (value.state.url) setUrl(value.state.url); }
            if (value.screenshot?.base64) setImage("data:" + value.screenshot.mediaType + ";base64," + value.screenshot.base64);
            if (name === "close") { setState(null); setImage(""); }
          }
          return value;
        } catch (reason) { if (alive.current && reason?.name !== "AbortError") setError(reason.message || String(reason)); return null; }
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
          await action("start", props.tab.path && /^https?:/i.test(props.tab.path) ? { url: props.tab.path } : {}, { signal: controller.signal });
          if(active&&autoRefresh)stop=visiblePoll(async signal=>{await action("capture",null,{quiet:true,signal});return 3000},3000);
        }).catch(reason => { if (active && reason?.name !== "AbortError") setError(reason.message || String(reason)); });
        return () => { active = false; controller.abort(); stop?.(); };
      }, [props.visible, props.scope.sessionId, browserSessionId, autoRefresh]);
      const navigate = () => {
        let target = url.trim();
        if (target && !/^https?:\/\//i.test(target)) target = "https://" + target;
        if (target) action("navigate", { url: target });
      };
      const point = event => {
        if (!state?.viewport) return;
        const rect = event.currentTarget.getBoundingClientRect();
        const x = Math.max(0, Math.min(state.viewport.width, (event.clientX - rect.left) * state.viewport.width / rect.width));
        const y = Math.max(0, Math.min(state.viewport.height, (event.clientY - rect.top) * state.viewport.height / rect.height));
        action(event.detail > 1 ? "double_click" : "click", { x, y });
      };
      return h("section", { className: "dswSuite", "data-tab": "controlled-browser", "data-browser-session": browserSessionId },
        h("div", { className: "dswSuiteBar" }, h(Button, { variant: "outline", size: "sm", disabled: busy, onClick: () => action("click", { x: 0, y: 0, button: "back" }) }, "后退"), h("input", { value: url, "aria-label": "受控浏览器地址", onChange: event => setUrl(event.target.value), onKeyDown: event => { if (event.key === "Enter") navigate(); } }), h(Button, { variant: "outline", size: "sm", disabled: busy || !url.trim(), onClick: navigate }, "转到"), h(Button, { variant: "outline", size: "sm", disabled: busy, onClick: () => action("capture") }, "刷新画面"), h(Button, { variant: "outline", size: "sm", onClick: () => action("close", { includeScreenshot: false }) }, "关闭会话")),
        error && h("div", { className: "dswSuiteStatus dswSuiteError", role: "alert" }, error),
        h("div", { className: "dswSuiteBrowser" }, image ? h("img", { src: image, alt: state?.title || "受控浏览器画面", draggable: false, onClick: point }) : h("div", { className: "dswSuiteBrowserEmpty" }, "正在启动隔离浏览器并获取画面…")),
        h("div", { className: "dswSuiteBar" }, h("input", { value: typing, placeholder: "输入到当前焦点", "aria-label": "发送到受控浏览器", onChange: event => setTyping(event.target.value), onKeyDown: event => { if (event.key === "Enter" && typing) { action("type", { text: typing }).then(() => setTyping("")); } } }), h(Button, { variant: "outline", size: "sm", disabled: !typing, onClick: () => action("type", { text: typing }).then(() => setTyping("")) }, "输入"), h(Button, { variant: "outline", size: "sm", onClick: () => action("scroll", { deltaY: -540 }) }, "向上"), h(Button, { variant: "outline", size: "sm", onClick: () => action("scroll", { deltaY: 540 }) }, "向下"), h("span", { className: "dswSuiteMeta" }, (meta?.adapter || "未连接") + " · " + (state ? (state.title || state.url) + " · " + state.viewport.width + "×" + state.viewport.height : "会话 " + browserSessionId))));
    }
    function ControlledBrowserTab(props) {
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
        h("p",null,"使用 UU 远程账号中的设备，绑定后可在这里发起连接。"),
        !state&&h("p",{role:"status"},"正在读取设备…"),state?.message&&h("p",{role:"status"},state.message),
        state?.installed===false&&h("a",{href:"https://uuyc.163.com/",target:"_blank",rel:"noopener noreferrer",className:"dswSuiteButton"},"安装 UU 远程"),
        h("div",{className:"dswSuiteToolbar"},state?.installed&&h(Button,{variant:"outline",size:"sm",type:"button",disabled:busy,onClick:()=>void act("open-client")},state.signedIn?"打开 UU 远程":"打开 UU 远程并登录"),h(Button,{variant:"outline",size:"sm",type:"button",disabled:busy,onClick:()=>void act("status")},busy?"正在刷新…":"刷新设备")),
        state?.signedIn&&h("p",null,"当前账号：",state.account?.name),
        state?.signedIn&&!state.devices?.length&&h("p",null,"当前账号没有可用设备，请在被控设备安装 UU 远程并登录同一账号。"),
        (state?.devices||[]).map(device=>h("div",{className:"dswSuiteSetting",key:device.id},h("div",null,h("strong",null,device.name),h("p",null,device.local?"当前设备":device.online?"在线":"离线",state.boundDeviceId===device.id?" · 已绑定":"")),h("div",{className:"dswSuiteToolbar"},h(Button,{variant:"outline",size:"sm",type:"button",disabled:busy,onClick:()=>void act(state.boundDeviceId===device.id?"unbind":"bind",{deviceId:device.id})},state.boundDeviceId===device.id?"解除绑定":"绑定"),state.boundDeviceId===device.id&&!device.local&&h(React.Fragment,null,h(Button,{variant:"outline",size:"sm",type:"button",disabled:busy||!device.online,onClick:()=>void act("connect",{deviceId:device.id})},"连接设备"),h(Button,{variant:"outline",size:"sm",type:"button",disabled:busy,onClick:()=>void act("disconnect",{deviceId:device.id})},"断开连接"))))),
        state?.installed&&h("p",null,"远端桌面在 UU 客户端显示，可手动接管和输入密码。当前设备无需远程连接；模型目前支持下方浏览器控制，暂不支持 UU 桌面操作。"),
        error&&h("p",{role:"alert",className:"dswSuiteError"},error));
    }
    function ComputerUseSettings({ scope }) {
      const snapshot = React.useSyncExternalStore(listener => scope.subscribe(listener), () => scope.getSnapshot(), () => scope.getSnapshot());
      const [error, setError] = React.useState("");
      const fields = [
        { key: "enabled", label: "启用 Computer Use", type: "switch", defaultValue: false },
        { key: "adapter", label: "执行适配器", type: "select", defaultValue: "auto", options: [{ value: "auto", label: "自动" }, { value: "native-browser", label: "内置浏览器" }, { value: "command", label: "外部命令" }] },
        { key: "browserExecutable", label: "浏览器可执行文件", type: "text", defaultValue: "" },
        { key: "browserHeadless", label: "后台运行浏览器", type: "switch", defaultValue: true },
        { key: "maxBrowserSessions", label: "最大浏览器会话数", type: "number", min: 1, max: 16, defaultValue: 4 },
        { key: "timeoutSeconds", label: "操作超时（秒）", type: "number", min: 5, max: 300, defaultValue: 60 },
        { key: "command", label: "外部控制命令", type: "text", defaultValue: "" }
      ];
      return h("section", { className: "dswSuiteSettings", "data-settings": "computer-use" }, h(DeviceSettings, {}), h("h3", null, "浏览器控制"), h("p", null, "模型与工作台共用受控浏览器。浏览器运行设置重启后生效。"), snapshot.status !== "ready" ? h("div", { className: "dswSuiteStatus" }, snapshot.status === "error" ? (snapshot.error || "设置读取失败") : "正在读取设置…") : fields.map(field => h(SettingRow, { key: field.key, scope, snapshot, field, error: setError })), error && h("div", { className: "dswSuiteStatus dswSuiteError", role: "alert" }, error));
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
        sidebar.registerTab({ id: "suite:jobs", title: "后台任务", order: 80, single: true, component: JobsTab }),
        sidebar.registerTab({ id: "suite:controlled-browser", title: "受控浏览器", order: 100, single: true, component: ControlledBrowserTab, settings: { pluginToggles: [{ key: "autoRefresh", title: "自动刷新浏览器画面", type: "switch", defaultValue: false }] }, onClose: (tab, scope) => { void fetch("/__dsh-computer-use/action", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ ownerSessionId: scope.sessionId, browserSessionId: tab.meta?.browserSessionId || "default", action: "close", includeScreenshot: false }) }).catch(() => {}); } })
      ];
      ctx.slots.inject("settings.plugin.item", () => ctx.slots.register({ name: "settings.plugin.item", id: "computer-use", order: 40, label: "Computer Use" }, () => h("details", {className:"dshSettingsDisclosure"},h("summary",null,"Computer Use 与远程设备"),h(ComputerUseSettings, { scope: computerUseScope }))));
      ctx.effect?.(() => () => { clearFileDrafts(); for (const dispose of disposers.reverse()) dispose(); }, "sidebar-workbench-suite: registrations");
    }
    exports.apply = apply;
    exports.inject = inject;
    exports.test = { parseCsv, renderMarkdown, visiblePoll, MermaidDiagram, MarkdownWorkbench, CodeWorkbench, StructuredViewer, JobsTab, ControlledBrowserTab, ComputerUseSettings, DeviceSettings, rememberFileDraft, fileDraftCacheSnapshot: () => ({ keys: [...fileDrafts.keys()], bytes: fileDraftBytes }), clearFileDrafts };
    return module.exports;
  }
});
})();
