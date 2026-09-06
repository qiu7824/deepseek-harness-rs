var __DSH_BETTER_SIDEBAR_GIT_TERMINAL_URL__ = typeof document === "undefined" ? "" : (document.currentScript?.src || "");
window.__ModuleLoader__.load({
  id: "dsh-better-sidebar/git-terminal",
  factory: (require) => {
    const module = { exports: {} };
    const exports = module.exports;
    const React = require("react");
    const PIN_KEY = "dsh-better-sidebar:v2:pinned-terminals";
    const STYLE_ID = "dsh-better-sidebar/git-terminal";
    const ESC = "\u001b";
    const XTERM_ASSET_URL = __DSH_BETTER_SIDEBAR_GIT_TERMINAL_URL__.replace(/\/git-terminal\.js(?:\?.*)?$/, "/xterm.js");
    let xtermLoading = null;

    function loadXterm() {
      if (typeof window.Terminal === "function") return Promise.resolve(window.Terminal);
      if (xtermLoading) return xtermLoading;
      if (!XTERM_ASSET_URL || XTERM_ASSET_URL === __DSH_BETTER_SIDEBAR_GIT_TERMINAL_URL__) return Promise.reject(new Error("xterm asset URL is unavailable"));
      xtermLoading = new Promise((resolve, reject) => {
        const script = document.createElement("script");
        script.src = XTERM_ASSET_URL;
        script.async = true;
        script.dataset.pluginAsset = "dsh-better-sidebar/xterm";
        script.onload = () => typeof window.Terminal === "function" ? resolve(window.Terminal) : reject(new Error("xterm did not register Terminal"));
        script.onerror = () => reject(new Error("xterm asset failed to load"));
        document.head.appendChild(script);
      }).catch(error => { xtermLoading = null; throw error; });
      return xtermLoading;
    }

    function endpoint(operation, sessionId, values) {
      const query = new URLSearchParams({ sessionId });
      for (const [key, value] of Object.entries(values || {})) {
        if (value !== undefined && value !== null && value !== "") query.set(key, String(value));
      }
      return `/__dsh-preview/${operation}?${query}`;
    }

    async function json(url, init) {
      const response = await fetch(url, init);
      const value = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(value.message || `HTTP ${response.status}`);
      return value;
    }

    function post(path, body) {
      return json(path, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
    }

    function installStyles() {
      if (document.querySelector(`style[data-plugin="${STYLE_ID}"]`)) return;
      const style = document.createElement("style");
      style.dataset.plugin = STYLE_ID;
      style.textContent = `
.dgt-shell{min-width:0;min-height:0;flex:1;display:flex;flex-direction:column;background:var(--dsw-alias-bg-base)}
.dgt-toolbar{min-height:38px;display:flex;align-items:center;gap:6px;padding:5px 10px;border-bottom:1px solid var(--dsw-alias-border-l1);color:var(--dsw-alias-label-secondary)}
.dgt-toolbar select,.dgt-toolbar input,.dgt-toolbar button{height:28px;min-width:0;border:1px solid var(--dsw-alias-border-l1);border-radius:6px;background:var(--dsw-alias-bg-base);color:var(--dsw-alias-label-primary);font-size:12px;padding:0 8px}.dgt-toolbar button{cursor:pointer}.dgt-toolbar button:hover:not(:disabled),.dgt-toolbar button[data-active=true]{background:var(--dsw-alias-interactive-bg-hover)}.dgt-toolbar button:disabled{opacity:.42;cursor:default}.dgt-toolbar .dgt-grow{flex:1}.dgt-toolbar .dgt-compact{max-width:180px}
.dgt-error,.dgt-note{padding:7px 10px;border-bottom:1px solid var(--dsw-alias-border-l1);font-size:12px}.dgt-error{color:var(--dsw-alias-state-error-primary)}.dgt-note{color:var(--dsw-alias-label-tertiary)}
.dgt-git-body{min-height:0;flex:1;display:grid;grid-template-columns:minmax(0,1fr) 248px}.dgt-git-main{min-width:0;min-height:0;display:flex;flex-direction:column}.dgt-git-rail{min-width:0;min-height:0;overflow:auto;border-left:1px solid var(--dsw-alias-border-l1);background:var(--dsw-alias-bg-sunken,rgba(0,0,0,.018));padding:6px}.dgt-section-title{height:28px;display:flex;align-items:center;padding:0 7px;color:var(--dsw-alias-label-tertiary);font-size:11px;font-weight:600;text-transform:uppercase;letter-spacing:.04em}.dgt-change{width:100%;min-height:30px;display:grid;grid-template-columns:24px minmax(0,1fr) 28px;align-items:center;gap:5px;border:0;border-radius:5px;padding:0 4px;background:transparent;color:var(--dsw-alias-label-secondary);font-size:12px;text-align:left;cursor:pointer}.dgt-change:hover,.dgt-change[data-active=true]{background:var(--dsw-alias-interactive-bg-hover);color:var(--dsw-alias-label-primary)}.dgt-change span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.dgt-change .dgt-change-action{height:24px;padding:0;border:0;border-radius:5px;background:transparent;color:inherit;cursor:pointer}.dgt-change:hover .dgt-change-action{background:var(--dsw-alias-bg-base)}
.dgt-compose{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:7px;padding:8px 10px;border-bottom:1px solid var(--dsw-alias-border-l1)}.dgt-compose textarea{grid-column:1/-1;box-sizing:border-box;min-height:54px;max-height:120px;resize:vertical;border:1px solid var(--dsw-alias-border-l1);border-radius:7px;padding:7px 9px;background:var(--dsw-alias-bg-base);color:var(--dsw-alias-label-primary);font:12px/1.45 inherit}.dgt-actions{display:flex;gap:5px;flex-wrap:wrap}.dgt-actions button{height:27px;border:0;border-radius:6px;padding:0 8px;background:var(--dsw-alias-interactive-bg-hover);color:var(--dsw-alias-label-primary);font-size:11px;cursor:pointer}.dgt-actions button[data-primary=true]{background:var(--dsw-alias-state-business-primary);color:white}.dgt-actions button:disabled{opacity:.4;cursor:default}
.dgt-empty{min-height:0;flex:1;display:grid;place-items:center;padding:24px;text-align:center;color:var(--dsw-alias-label-tertiary);font-size:13px}.dgt-diff{min-height:0;flex:1;display:flex;flex-direction:column;background:#111419;color:#d7dce3}.dgt-diffbar{min-height:36px;display:flex;align-items:center;gap:5px;padding:4px 8px;border-bottom:1px solid #2b3139;background:#181c22}.dgt-diffbar span{min-width:0;flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font:11.5px var(--ds-font-family-code)}.dgt-diffbar button{height:25px;border:1px solid #343b45;border-radius:5px;background:#20252c;color:#cbd1da;font-size:11px;cursor:pointer}.dgt-diffbar button[data-active=true]{border-color:#5888db;background:#263b5c}.dgt-diff-scroll{min-height:0;flex:1;overflow:auto;font:12px/1.55 var(--ds-font-family-code)}.dgt-hunk{border-bottom:1px solid #292e36}.dgt-hunk-head{position:sticky;top:0;z-index:1;padding:4px 10px;background:#19293a;color:#84b7e8;white-space:pre-wrap}.dgt-line{display:grid;grid-template-columns:48px 48px minmax(0,1fr);min-height:19px;white-space:pre}.dgt-line[data-kind=add]{background:#183526}.dgt-line[data-kind=del]{background:#442126}.dgt-line[data-selected=true]{outline:1px solid #e2b84d;outline-offset:-1px}.dgt-ln{padding:0 6px;text-align:right;user-select:none;color:#69717e;border-right:1px solid rgba(255,255,255,.05)}.dgt-code{padding:0 8px;overflow:visible}.dgt-split{display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr)}.dgt-split-side{min-width:0;border-right:1px solid #2b3139}.dgt-split-side:last-child{border-right:0}.dgt-split .dgt-line{grid-template-columns:48px minmax(0,1fr)}
.dgt-history{min-height:0;flex:1;overflow:auto}.dgt-commit{width:100%;display:grid;grid-template-columns:74px minmax(0,1fr);gap:7px;padding:8px 10px;border:0;border-bottom:1px solid var(--dsw-alias-border-l1);background:transparent;color:var(--dsw-alias-label-primary);text-align:left;cursor:pointer}.dgt-commit:hover,.dgt-commit[data-active=true]{background:var(--dsw-alias-interactive-bg-hover)}.dgt-hash{font:11px var(--ds-font-family-code);color:var(--dsw-alias-state-business-primary)}.dgt-subject{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:12px}.dgt-meta{grid-column:2;color:var(--dsw-alias-label-tertiary);font-size:10.5px}.dgt-load{width:calc(100% - 20px);height:30px;margin:8px 10px;border:1px solid var(--dsw-alias-border-l1);border-radius:6px;background:transparent;color:var(--dsw-alias-label-secondary);cursor:pointer}
.dgt-terminal-tabs{min-height:38px;display:flex;align-items:center;gap:3px;padding:4px 8px;border-bottom:1px solid #2b3139;background:#181c22;overflow-x:auto}.dgt-terminal-tab{flex:none;max-width:190px;height:29px;display:flex;align-items:center;gap:5px;border:1px solid transparent;border-radius:6px;padding:0 8px;background:transparent;color:#aeb5c0;font-size:11px;cursor:pointer}.dgt-terminal-tab[data-active=true]{border-color:#424b57;background:#242a32;color:#f0f2f5}.dgt-terminal-tab[data-pinned=true]::before{content:"⌖";color:#82aaff}.dgt-terminal-tab span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.dgt-terminal{position:relative;min-height:0;flex:1;display:flex;flex-direction:column;background:#0f1216;color:#d8dee9}.dgt-screen{min-height:0;flex:1;overflow:auto;padding:8px 10px;outline:0;font:13px/1.45 var(--ds-font-family-code);white-space:pre}.dgt-screen-line{min-height:1.45em}.dgt-cursor{outline:1px solid #d8dee9;background:rgba(216,222,233,.22)}.dgt-terminal-capture{position:absolute;left:10px;bottom:5px;width:2px;height:2px;opacity:.01;resize:none;border:0;padding:0}.dgt-terminal-status{display:flex;align-items:center;gap:8px;min-height:25px;padding:0 9px;border-top:1px solid #2b3139;background:#181c22;color:#929aa6;font:10.5px var(--ds-font-family-code)}.dgt-terminal-status .dgt-grow{flex:1}.dgt-pin-select{max-width:120px!important}
@media(max-width:768px){.dgt-git-body{grid-template-columns:minmax(0,1fr)}.dgt-git-rail{max-height:34vh;border-left:0;border-top:1px solid var(--dsw-alias-border-l1)}.dgt-toolbar{overflow-x:auto}.dgt-toolbar .dgt-compact{max-width:135px}.dgt-compose{padding:7px}.dgt-diffbar{overflow-x:auto}.dgt-terminal-tabs{padding-left:5px}.dgt-screen{font-size:12px;padding:6px}}
.dgt-xterm-host{position:relative;min-width:0;min-height:0;flex:1;padding:7px 8px;background:#0f1216;overflow:hidden}.dgt-xterm-host>.xterm{height:100%}
.xterm{position:relative;cursor:text;user-select:none;-ms-user-select:none;-webkit-user-select:none}.xterm.focus,.xterm:focus{outline:none}.xterm .xterm-helpers{position:absolute;top:0;z-index:5}.xterm .xterm-helper-textarea{position:absolute;opacity:0;left:-9999em;top:0;width:0;height:0;z-index:-5;padding:0;border:0;margin:0;white-space:nowrap;overflow:hidden;resize:none}.xterm .composition-view{background:#000;color:#fff;display:none;position:absolute;white-space:nowrap;z-index:1}.xterm .composition-view.active{display:block}.xterm .xterm-viewport{background-color:#0f1216;overflow-y:scroll;cursor:default;position:absolute;inset:0}.xterm .xterm-screen{position:relative}.xterm .xterm-screen canvas{position:absolute;left:0;top:0}.xterm .xterm-scroll-area{visibility:hidden}.xterm-char-measure-element{display:inline-block;visibility:hidden;position:absolute;top:0;left:-9999em;line-height:normal}.xterm.enable-mouse-events{cursor:default}.xterm.xterm-cursor-pointer,.xterm .xterm-cursor-pointer{cursor:pointer}.xterm.column-select.focus{cursor:crosshair}.xterm .xterm-accessibility:not(.debug),.xterm .xterm-message{position:absolute;inset:0;z-index:10;color:transparent;pointer-events:none}.xterm .xterm-accessibility-tree:not(.debug) *::selection{color:transparent}.xterm .xterm-accessibility-tree{user-select:text;white-space:pre}.xterm .live-region{position:absolute;left:-9999px;width:1px;height:1px;overflow:hidden}.xterm-dim{opacity:1!important}.xterm-underline-1{text-decoration:underline}.xterm-underline-2{text-decoration:double underline}.xterm-underline-3{text-decoration:wavy underline}.xterm-underline-4{text-decoration:dotted underline}.xterm-underline-5{text-decoration:dashed underline}.xterm-overline{text-decoration:overline}.xterm-strikethrough{text-decoration:line-through}.xterm-screen .xterm-decoration-container .xterm-decoration{z-index:6;position:absolute}.xterm-screen .xterm-decoration-container .xterm-decoration.xterm-decoration-top-layer{z-index:7}.xterm-decoration-overview-ruler{z-index:8;position:absolute;top:0;right:0;pointer-events:none}.xterm-decoration-top{z-index:2;position:relative}
`;
      document.head.appendChild(style);
    }

    function useInstalledStyles() {
      React.useEffect(installStyles, []);
    }

    function safePins(value) {
      if (!Array.isArray(value)) return [];
      const seen = new Set();
      const result = [];
      for (const pin of value) {
        if (!pin || typeof pin !== "object") continue;
        if (pin.scope !== "workspace" && pin.scope !== "global") continue;
        if (![pin.terminalId, pin.homeSessionId, pin.name].every(item => typeof item === "string" && item.length > 0 && item.length <= 200)) continue;
        if (pin.scope === "workspace" && (typeof pin.workspaceKey !== "string" || !pin.workspaceKey)) continue;
        const key = `${pin.homeSessionId}:${pin.terminalId}`;
        if (seen.has(key)) continue;
        seen.add(key);
        result.push({
          terminalId: pin.terminalId,
          homeSessionId: pin.homeSessionId,
          workspaceKey: typeof pin.workspaceKey === "string" ? pin.workspaceKey : "",
          name: pin.name,
          scope: pin.scope,
          pid: Number.isInteger(pin.pid) ? pin.pid : null,
        });
        if (result.length >= 32) break;
      }
      return result;
    }

    function readPins() {
      try { return safePins(JSON.parse(localStorage.getItem(PIN_KEY) || "[]")); }
      catch { return []; }
    }

    function writePins(pins) {
      const value = safePins(pins);
      localStorage.setItem(PIN_KEY, JSON.stringify(value));
      window.dispatchEvent(new CustomEvent("dsh-better-sidebar:pins", { detail: value }));
      return value;
    }

    function usePins() {
      const [pins, setPins] = React.useState(readPins);
      React.useEffect(() => {
        const update = event => setPins(safePins(event.detail || readPins()));
        const storage = event => { if (event.key === PIN_KEY) setPins(readPins()); };
        window.addEventListener("dsh-better-sidebar:pins", update);
        window.addEventListener("storage", storage);
        return () => {
          window.removeEventListener("dsh-better-sidebar:pins", update);
          window.removeEventListener("storage", storage);
        };
      }, []);
      return [pins, writePins];
    }

    function queryTarget(sessionId, operation, target, extra) {
      return endpoint(operation, sessionId, {
        repository: target.repository,
        worktree: target.worktree,
        ...(extra || {}),
      });
    }

    function targetBody(sessionId, target, extra) {
      return { sessionId, repository: target.repository, worktree: target.worktree, ...(extra || {}) };
    }

    function parseDiff(text) {
      const files = [];
      let file = null;
      let hunk = null;
      let oldLine = 0;
      let newLine = 0;
      for (const raw of String(text || "").split("\n")) {
        if (raw.startsWith("diff --git ")) {
          file = { header: [raw], path: raw.split(" b/").pop() || raw, hunks: [] };
          files.push(file);
          hunk = null;
          continue;
        }
        if (!file) {
          file = { header: [], path: "diff", hunks: [] };
          files.push(file);
        }
        if (raw.startsWith("@@")) {
          const match = raw.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
          oldLine = Number(match?.[1] || 0);
          newLine = Number(match?.[2] || 0);
          hunk = { header: raw, lines: [] };
          file.hunks.push(hunk);
          continue;
        }
        if (!hunk) {
          file.header.push(raw);
          continue;
        }
        let kind = "context", left = oldLine, right = newLine;
        if (raw.startsWith("+")) { kind = "add"; left = null; newLine += 1; }
        else if (raw.startsWith("-")) { kind = "del"; right = null; oldLine += 1; }
        else if (raw.startsWith("\\")) { kind = "meta"; left = null; right = null; }
        else { oldLine += 1; newLine += 1; }
        hunk.lines.push({ raw, kind, left, right });
      }
      return files;
    }

    function DiffLine({ line, index, selected, onSelect, side }) {
      const number = side === "left" ? line.left : side === "right" ? line.right : null;
      const visible = side === "left" ? line.kind !== "add" : side === "right" ? line.kind !== "del" : true;
      if (side) return React.createElement("div", { className: "dgt-line", "data-kind": line.kind, "data-selected": selected || undefined, onClick: event => onSelect(index, event.shiftKey) },
        React.createElement("span", { className: "dgt-ln" }, number ?? ""),
        React.createElement("span", { className: "dgt-code" }, visible ? line.raw : ""));
      return React.createElement("div", { className: "dgt-line", "data-kind": line.kind, "data-selected": selected || undefined, onClick: event => onSelect(index, event.shiftKey) },
        React.createElement("span", { className: "dgt-ln" }, line.left ?? ""),
        React.createElement("span", { className: "dgt-ln" }, line.right ?? ""),
        React.createElement("span", { className: "dgt-code" }, line.raw));
    }

    function DiffViewer({ value, label, truncated }) {
      const files = React.useMemo(() => parseDiff(value), [value]);
      const hunks = React.useMemo(() => files.flatMap(file => file.hunks.map(hunk => ({ ...hunk, path: file.path }))), [files]);
      const rows = React.useMemo(() => hunks.flatMap(hunk => hunk.lines), [hunks]);
      const [mode, setMode] = React.useState("unified");
      const [selected, setSelected] = React.useState(() => new Set());
      const anchor = React.useRef(null);
      const nodes = React.useRef([]);
      React.useEffect(() => { setSelected(new Set()); anchor.current = null; }, [value]);
      const select = (index, range) => {
        setSelected(previous => {
          const next = new Set(previous);
          if (range && anchor.current !== null) {
            const start = Math.min(anchor.current, index), end = Math.max(anchor.current, index);
            for (let value = start; value <= end; value += 1) next.add(value);
          } else if (next.has(index)) next.delete(index);
          else next.add(index);
          anchor.current = index;
          return next;
        });
      };
      const copy = async () => {
        const chosen = rows.filter((_line, index) => selected.has(index));
        const text = (chosen.length ? chosen : rows).map(line => line.raw).join("\n");
        await navigator.clipboard?.writeText(text);
      };
      const jump = delta => {
        const current = nodes.current.findIndex(node => node && node.getBoundingClientRect().top >= 0);
        const next = Math.max(0, Math.min(hunks.length - 1, (current < 0 ? 0 : current) + delta));
        nodes.current[next]?.scrollIntoView({ block: "start" });
      };
      let rowIndex = 0;
      return React.createElement("section", { className: "dgt-diff", "aria-label": `差异 ${label}` },
        React.createElement("div", { className: "dgt-diffbar" },
          React.createElement("span", { title: label }, `${label}${truncated ? " · 已截断" : ""}`),
          React.createElement("button", { "data-active": mode === "unified" || undefined, onClick: () => setMode("unified") }, "统一"),
          React.createElement("button", { "data-active": mode === "split" || undefined, onClick: () => setMode("split") }, "并排"),
          React.createElement("button", { disabled: !hunks.length, onClick: () => jump(-1), title: "上一处差异" }, "↑"),
          React.createElement("button", { disabled: !hunks.length, onClick: () => jump(1), title: "下一处差异" }, "↓"),
          React.createElement("button", { disabled: !rows.length, onClick: copy }, selected.size ? `复制 ${selected.size} 行` : "复制差异")),
        React.createElement("div", { className: "dgt-diff-scroll" },
          files.flatMap((file, fileIndex) => [
            React.createElement("div", { key: `file-${fileIndex}`, className: "dgt-hunk-head" }, file.header.filter(Boolean).join("\n") || file.path),
            ...file.hunks.map((hunk, hunkIndex) => {
              const start = rowIndex;
              rowIndex += hunk.lines.length;
              return React.createElement("section", { key: `${fileIndex}-${hunkIndex}`, className: "dgt-hunk", ref: node => { nodes.current[files.slice(0, fileIndex).reduce((sum, item) => sum + item.hunks.length, 0) + hunkIndex] = node; } },
                React.createElement("div", { className: "dgt-hunk-head" }, hunk.header),
                mode === "split"
                  ? React.createElement("div", { className: "dgt-split" },
                      React.createElement("div", { className: "dgt-split-side" }, hunk.lines.map((line, index) => React.createElement(DiffLine, { key: index, line, index: start + index, side: "left", selected: selected.has(start + index), onSelect: select }))),
                      React.createElement("div", { className: "dgt-split-side" }, hunk.lines.map((line, index) => React.createElement(DiffLine, { key: index, line, index: start + index, side: "right", selected: selected.has(start + index), onSelect: select }))))
                  : hunk.lines.map((line, index) => React.createElement(DiffLine, { key: index, line, index: start + index, selected: selected.has(start + index), onSelect: select })));
            }),
          ])));
    }

    function GitWorkbench({ sessionId }) {
      useInstalledStyles();
      const [target, setTarget] = React.useState({ repository: "", worktree: "" });
      const [git, setGit] = React.useState(null);
      const [view, setView] = React.useState("changes");
      const [diff, setDiff] = React.useState(null);
      const [history, setHistory] = React.useState([]);
      const [hasMore, setHasMore] = React.useState(false);
      const [selectedCommit, setSelectedCommit] = React.useState(null);
      const [message, setMessage] = React.useState("");
      const [busy, setBusy] = React.useState("");
      const [error, setError] = React.useState("");
      const generation = React.useRef(0);

      const refresh = React.useCallback(async override => {
        const nextTarget = { ...target, ...(override || {}) };
        const request = ++generation.current;
        const value = await json(queryTarget(sessionId, "git-status", nextTarget));
        if (request !== generation.current) return value;
        const repository = nextTarget.repository || value.repository || value.repositories?.[0]?.path || "";
        const worktree = nextTarget.worktree || value.worktree || value.worktrees?.find(entry => entry.current)?.path || value.worktrees?.[0]?.path || "";
        setTarget({ repository, worktree });
        setGit(value);
        setError("");
        return value;
      }, [sessionId, target.repository, target.worktree]);

      React.useEffect(() => {
        const emptyTarget = { repository: "", worktree: "" };
        setTarget(emptyTarget); setGit(null); setDiff(null); setHistory([]); setSelectedCommit(null);
        refresh(emptyTarget).catch(failure => setError(failure.message));
      }, [sessionId]);

      const changeTarget = async next => {
        setDiff(null); setHistory([]); setSelectedCommit(null);
        setTarget(next);
        try { await refresh(next); }
        catch (failure) { setError(failure.message); }
      };

      const action = async (name, extra) => {
        try {
          setBusy(name); setError("");
          await post("/__dsh-preview/git-action", targetBody(sessionId, target, { action: name, ...(extra || {}) }));
          await refresh();
          if (name === "commit" || name === "commit-push") setMessage("");
          if (name === "revert" || name === "cherry-pick") await loadHistory(true);
        } catch (failure) { setError(failure.message); }
        finally { setBusy(""); }
      };

      const openDiff = async (entry, staged) => {
        const request = generation.current;
        try {
          setError("");
          const value = await json(queryTarget(sessionId, "git-diff", target, { path: entry.path, staged: staged ? 1 : 0 }));
          if (request === generation.current) setDiff({ ...value, staged });
        } catch (failure) { setError(failure.message); }
      };

      const loadHistory = async reset => {
        const request = generation.current;
        const skip = reset ? 0 : history.length;
        try {
          setBusy("history");
          const value = await json(queryTarget(sessionId, "git-log", target, { skip, count: 30 }));
          if (request !== generation.current) return;
          setHistory(previous => reset ? (value.entries || []) : [...previous, ...(value.entries || [])]);
          setHasMore(value.hasMore === true);
        } catch (failure) { setError(failure.message); }
        finally { setBusy(""); }
      };

      React.useEffect(() => { if (view === "history" && history.length === 0 && git) void loadHistory(true); }, [view, git?.worktree]);

      const openCommit = async entry => {
        const request = generation.current;
        try {
          setSelectedCommit(entry);
          const value = await json(queryTarget(sessionId, "git-commit-diff", target, { revision: entry.hash }));
          if (request === generation.current) setDiff({ ...value, history: true });
        } catch (failure) { setError(failure.message); }
      };

      const statusEntries = git?.entries || [];
      const staged = statusEntries.filter(entry => entry.indexStatus !== " " && entry.indexStatus !== "?");
      const unstaged = statusEntries.filter(entry => entry.worktreeStatus !== " " || entry.status === "??");
      const repositories = git?.repositories || [];
      const worktrees = git?.worktrees || [];
      const confirmAction = (name, text, extra) => { if (window.confirm(text)) void action(name, extra); };

      const selectors = React.createElement("div", { className: "dgt-toolbar" },
        React.createElement("select", { className: "dgt-compact", value: target.repository, "aria-label": "Git 仓库", onChange: event => changeTarget({ repository: event.target.value, worktree: "" }) }, repositories.map(entry => React.createElement("option", { key: entry.path, value: entry.path }, `${entry.label} · ${entry.relativePath}`))),
        React.createElement("select", { className: "dgt-compact", value: target.worktree, "aria-label": "Git 工作树", onChange: event => changeTarget({ ...target, worktree: event.target.value }) }, worktrees.map(entry => React.createElement("option", { key: entry.path, value: entry.path }, `${entry.current ? "● " : ""}${entry.branch} (${entry.changes})`))),
        React.createElement("select", { className: "dgt-compact", value: git?.branch || "", "aria-label": "Git 分支", disabled: !git?.branches?.length || busy, onChange: event => action("checkout", { branch: event.target.value }) }, (git?.branches || []).map(branch => React.createElement("option", { key: branch, value: branch }, branch))),
        React.createElement("span", { className: "dgt-grow" }),
        React.createElement("button", { "data-active": view === "changes" || undefined, onClick: () => setView("changes") }, "更改"),
        React.createElement("button", { "data-active": view === "history" || undefined, onClick: () => setView("history") }, "历史"),
        React.createElement("button", { disabled: !!busy, onClick: () => refresh().catch(failure => setError(failure.message)), title: "刷新" }, "↻"));

      const compose = React.createElement("div", { className: "dgt-compose" },
        React.createElement("textarea", { value: message, maxLength: 500, placeholder: "输入提交说明…", onChange: event => setMessage(event.target.value) }),
        React.createElement("div", { className: "dgt-actions" },
          React.createElement("button", { disabled: !!busy || !unstaged.length, onClick: () => action("stage-all") }, "全部暂存"),
          React.createElement("button", { disabled: !!busy || !staged.length, onClick: () => action("unstage-all") }, "取消全部"),
          React.createElement("button", { disabled: !!busy || !staged.length || !message.trim(), onClick: () => action("commit", { message }) }, busy === "commit" ? "提交中…" : "提交"),
          React.createElement("button", { "data-primary": true, disabled: !!busy || !staged.length || !message.trim() || !git?.upstream, onClick: () => action("commit-push", { message }) }, busy === "commit-push" ? "提交并推送中…" : "提交并推送"),
          React.createElement("button", { disabled: !!busy || !git?.upstream || !git?.ahead, onClick: () => action("push") }, "推送")),
        React.createElement("span", { className: "dgt-note" }, git?.upstream ? `${git.upstream} · ↑${git.ahead || 0} ↓${git.behind || 0}` : "未配置上游"));

      const changeRail = React.createElement("aside", { className: "dgt-git-rail", "aria-label": "Git 更改" },
        [["已暂存", staged, true], ["待处理", unstaged, false]].map(([label, rows, isStaged]) => React.createElement(React.Fragment, { key: label },
          React.createElement("div", { className: "dgt-section-title" }, `${label} · ${rows.length}`),
          rows.map(entry => React.createElement("div", { key: `${label}:${entry.path}`, className: "dgt-change", "data-active": diff?.path === entry.path && diff?.staged === isStaged || undefined, title: entry.path, onClick: () => openDiff(entry, isStaged) },
            React.createElement("span", null, isStaged ? entry.indexStatus : entry.worktreeStatus),
            React.createElement("span", null, entry.path),
            React.createElement("button", { className: "dgt-change-action", title: isStaged ? "取消暂存" : "暂存", onClick: event => { event.stopPropagation(); void action(isStaged ? "unstage" : "stage", { path: entry.path }); } }, isStaged ? "−" : "+"),
            !isStaged && entry.status !== "??" && React.createElement("button", { className: "dgt-change-action", title: "放弃工作区更改", onClick: event => { event.stopPropagation(); confirmAction("discard", `确定放弃 ${entry.path} 的工作区更改？`, { path: entry.path }); } }, "×"))))));

      const historyRail = React.createElement("aside", { className: "dgt-git-rail", "aria-label": "Git 历史" },
        React.createElement("div", { className: "dgt-section-title" }, "最近提交"),
        React.createElement("div", { className: "dgt-history" }, history.map(entry => React.createElement("button", { key: entry.hash, className: "dgt-commit", "data-active": selectedCommit?.hash === entry.hash || undefined, onClick: () => openCommit(entry) },
          React.createElement("span", { className: "dgt-hash" }, entry.shortHash),
          React.createElement("span", { className: "dgt-subject", title: entry.subject }, entry.subject),
          React.createElement("span", { className: "dgt-meta" }, `${entry.author} · ${new Date(entry.date).toLocaleString()}`))),
          hasMore && React.createElement("button", { className: "dgt-load", disabled: busy === "history", onClick: () => loadHistory(false) }, busy === "history" ? "载入中…" : "加载更多")));

      let mainBody = diff
        ? React.createElement(DiffViewer, { value: diff.diff, label: selectedCommit?.subject || diff.path, truncated: diff.truncated })
        : React.createElement("div", { className: "dgt-empty" }, view === "history" ? "选择提交查看完整差异。" : "选择文件查看工作区或暂存区差异。");
      if (view === "history" && selectedCommit) {
        mainBody = React.createElement(React.Fragment, null,
          React.createElement("div", { className: "dgt-toolbar" },
            React.createElement("strong", { className: "dgt-grow", title: selectedCommit.subject }, selectedCommit.subject),
            React.createElement("button", { disabled: !!busy, onClick: () => confirmAction("cherry-pick", `确定拣选提交 ${selectedCommit.shortHash}？`, { revision: selectedCommit.hash }) }, "拣选"),
            React.createElement("button", { disabled: !!busy, onClick: () => confirmAction("revert", `确定创建反向提交以还原 ${selectedCommit.shortHash}？`, { revision: selectedCommit.hash }) }, "还原提交")),
          mainBody);
      }

      return React.createElement("section", { className: "dgt-shell", "aria-label": "Git 工作台" }, selectors,
        error && React.createElement("div", { className: "dgt-error", role: "alert" }, error),
        view === "changes" && compose,
        React.createElement("div", { className: "dgt-git-body" }, React.createElement("main", { className: "dgt-git-main" }, mainBody), view === "changes" ? changeRail : historyRail));
    }

    const BASIC_COLORS = ["#2e3440", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0", "#4c566a", "#d57780", "#b1d196", "#f0d399", "#8fadd0", "#c19acb", "#95ccd5", "#eceff4"];
    function xtermColor(index) {
      if (index < 16) return BASIC_COLORS[index];
      if (index < 232) {
        const value = index - 16, r = Math.floor(value / 36), g = Math.floor(value % 36 / 6), b = value % 6;
        const level = item => item === 0 ? 0 : 55 + item * 40;
        return `rgb(${level(r)},${level(g)},${level(b)})`;
      }
      const gray = 8 + (index - 232) * 10;
      return `rgb(${gray},${gray},${gray})`;
    }

    function defaultAttr() { return { fg: null, bg: null, bold: false, dim: false, italic: false, underline: false, inverse: false, link: null }; }
    function cloneAttr(value) { return { ...value }; }
    function blankCell(attr) { return { ch: " ", attr: cloneAttr(attr) }; }

    class AnsiTerminalModel {
      constructor(cols, rows) {
        this.cols = Math.max(10, cols || 120); this.rows = Math.max(2, rows || 30);
        this.lines = [[]]; this.x = 0; this.y = 0; this.saved = [0, 0]; this.attr = defaultAttr(); this.cursor = true; this.title = "";
      }
      line(y = this.y) { while (this.lines.length <= y) this.lines.push([]); return this.lines[y]; }
      move(y, x) { this.y = Math.max(0, y); this.x = Math.max(0, Math.min(this.cols - 1, x)); this.line(); }
      put(ch) {
        if (this.x >= this.cols) { this.x = 0; this.y += 1; }
        this.line()[this.x] = { ch, attr: cloneAttr(this.attr) }; this.x += 1;
      }
      eraseLine(mode) {
        const line = this.line();
        if (mode === 2) { this.lines[this.y] = []; return; }
        const start = mode === 1 ? 0 : this.x, end = mode === 1 ? this.x : Math.max(line.length, this.cols);
        for (let index = start; index <= end; index += 1) line[index] = blankCell(this.attr);
      }
      eraseDisplay(mode) {
        if (mode === 2 || mode === 3) { this.lines = [[]]; this.x = 0; this.y = 0; return; }
        if (mode === 0) { this.eraseLine(0); this.lines.length = this.y + 1; }
        else { for (let row = 0; row < this.y; row += 1) this.lines[row] = []; this.eraseLine(1); }
      }
      sgr(values) {
        if (!values.length) values = [0];
        for (let index = 0; index < values.length; index += 1) {
          const code = values[index] || 0;
          if (code === 0) this.attr = defaultAttr();
          else if (code === 1) this.attr.bold = true;
          else if (code === 2) this.attr.dim = true;
          else if (code === 3) this.attr.italic = true;
          else if (code === 4) this.attr.underline = true;
          else if (code === 7) this.attr.inverse = true;
          else if (code === 22) { this.attr.bold = false; this.attr.dim = false; }
          else if (code === 23) this.attr.italic = false;
          else if (code === 24) this.attr.underline = false;
          else if (code === 27) this.attr.inverse = false;
          else if (code >= 30 && code <= 37) this.attr.fg = xtermColor(code - 30);
          else if (code >= 90 && code <= 97) this.attr.fg = xtermColor(code - 90 + 8);
          else if (code === 39) this.attr.fg = null;
          else if (code >= 40 && code <= 47) this.attr.bg = xtermColor(code - 40);
          else if (code >= 100 && code <= 107) this.attr.bg = xtermColor(code - 100 + 8);
          else if (code === 49) this.attr.bg = null;
          else if ((code === 38 || code === 48) && values[index + 1] === 5) { this.attr[code === 38 ? "fg" : "bg"] = xtermColor(values[index + 2] || 0); index += 2; }
          else if ((code === 38 || code === 48) && values[index + 1] === 2) { this.attr[code === 38 ? "fg" : "bg"] = `rgb(${values[index + 2] || 0},${values[index + 3] || 0},${values[index + 4] || 0})`; index += 4; }
        }
      }
      csi(final, raw) {
        const privateMode = raw.startsWith("?");
        const values = raw.replace(/^\?/, "").split(";").map(value => Number(value || 0));
        const amount = values[0] || 1;
        if (final === "A") this.move(this.y - amount, this.x);
        else if (final === "B") this.move(this.y + amount, this.x);
        else if (final === "C") this.move(this.y, this.x + amount);
        else if (final === "D") this.move(this.y, this.x - amount);
        else if (final === "E") this.move(this.y + amount, 0);
        else if (final === "F") this.move(this.y - amount, 0);
        else if (final === "G") this.move(this.y, amount - 1);
        else if (final === "H" || final === "f") this.move((values[0] || 1) - 1, (values[1] || 1) - 1);
        else if (final === "J") this.eraseDisplay(values[0] || 0);
        else if (final === "K") this.eraseLine(values[0] || 0);
        else if (final === "m") this.sgr(values);
        else if (final === "s") this.saved = [this.y, this.x];
        else if (final === "u") this.move(this.saved[0], this.saved[1]);
        else if ((final === "h" || final === "l") && privateMode && values.includes(25)) this.cursor = final === "h";
        else if ((final === "h" || final === "l") && privateMode && values.some(value => value === 1047 || value === 1049)) {
          if (final === "h") { this.savedScreen = [this.lines, this.x, this.y]; this.lines = [[]]; this.x = 0; this.y = 0; }
          else if (this.savedScreen) { [this.lines, this.x, this.y] = this.savedScreen; this.savedScreen = null; }
        }
      }
      feed(text) {
        for (let index = 0; index < text.length;) {
          const ch = text[index];
          if (ch === ESC && text[index + 1] === "[") {
            let end = index + 2;
            while (end < text.length && !(text.charCodeAt(end) >= 0x40 && text.charCodeAt(end) <= 0x7e)) end += 1;
            if (end >= text.length) break;
            this.csi(text[end], text.slice(index + 2, end)); index = end + 1; continue;
          }
          if (ch === ESC && text[index + 1] === "]") {
            let end = index + 2;
            while (end < text.length && text[end] !== "\u0007" && !(text[end] === ESC && text[end + 1] === "\\")) end += 1;
            const body = text.slice(index + 2, end);
            if (body.startsWith("0;") || body.startsWith("2;")) this.title = body.slice(2);
            if (body.startsWith("8;;")) this.attr.link = body.slice(3) || null;
            index = end + (text[end] === ESC ? 2 : 1); continue;
          }
          if (ch === ESC && (text[index + 1] === "7" || text[index + 1] === "8")) {
            if (text[index + 1] === "7") this.saved = [this.y, this.x]; else this.move(this.saved[0], this.saved[1]);
            index += 2; continue;
          }
          if (ch === "\r") { this.x = 0; index += 1; continue; }
          if (ch === "\n") { this.y += 1; this.line(); index += 1; continue; }
          if (ch === "\b") { this.x = Math.max(0, this.x - 1); index += 1; continue; }
          if (ch === "\t") { this.x = Math.min(this.cols - 1, (Math.floor(this.x / 8) + 1) * 8); index += 1; continue; }
          if (ch.charCodeAt(0) < 32 || ch === "\u007f") { index += 1; continue; }
          const point = text.codePointAt(index); const value = String.fromCodePoint(point); this.put(value); index += value.length;
        }
        if (this.lines.length > 4_000) { const removed = this.lines.length - 4_000; this.lines.splice(0, removed); this.y = Math.max(0, this.y - removed); }
        return this;
      }
    }

    function renderRuns(line, cursorColumn, cursorVisible) {
      const cells = line.slice();
      const length = cursorVisible ? Math.max(cells.length, cursorColumn + 1) : cells.length;
      while (cells.length < length) cells.push(blankCell(defaultAttr()));
      const runs = [];
      let current = null;
      cells.forEach((cell, index) => {
        const cursor = cursorVisible && index === cursorColumn;
        const attr = cell.attr || defaultAttr();
        const key = JSON.stringify([attr.fg, attr.bg, attr.bold, attr.dim, attr.italic, attr.underline, attr.inverse, attr.link, cursor]);
        if (!current || current.key !== key) { current = { key, attr, cursor, text: "" }; runs.push(current); }
        current.text += cell.ch || " ";
      });
      return runs;
    }

    function TerminalScreen({ text, cols, rows, onInput, status }) {
      const model = React.useMemo(() => new AnsiTerminalModel(cols, rows).feed(text), [text, cols, rows]);
      const capture = React.useRef(null);
      const scroll = React.useRef(null);
      const atBottom = React.useRef(true);
      React.useLayoutEffect(() => { if (atBottom.current && scroll.current) scroll.current.scrollTop = scroll.current.scrollHeight; }, [text, cols]);
      const key = event => {
        if (event.isComposing) return;
        let data = "";
        const map = { Enter: "\r", Backspace: "\u007f", Tab: "\t", Escape: ESC, ArrowUp: `${ESC}[A`, ArrowDown: `${ESC}[B`, ArrowRight: `${ESC}[C`, ArrowLeft: `${ESC}[D`, Home: `${ESC}[H`, End: `${ESC}[F`, Delete: `${ESC}[3~`, PageUp: `${ESC}[5~`, PageDown: `${ESC}[6~`, Insert: `${ESC}[2~` };
        if (map[event.key]) data = map[event.key];
        else if (event.ctrlKey && !event.altKey && !event.metaKey && /^[a-z@\[\\\]\^_]$/i.test(event.key)) data = String.fromCharCode(event.key.toUpperCase().charCodeAt(0) & 31);
        else if (!event.ctrlKey && !event.metaKey && !event.altKey && event.key.length === 1) data = event.key;
        if (data) { event.preventDefault(); onInput(data); }
      };
      const input = event => { const value = event.currentTarget.value; if (value) onInput(value); event.currentTarget.value = ""; };
      const paste = event => { const value = event.clipboardData?.getData("text"); if (value) { event.preventDefault(); onInput(value); } };
      const lines = model.lines.map((line, row) => React.createElement("div", { className: "dgt-screen-line", key: row }, renderRuns(line, model.x, model.cursor && row === model.y).map((run, index) => {
        let foreground = run.attr.fg, background = run.attr.bg;
        if (run.attr.inverse) [foreground, background] = [background || "#d8dee9", foreground || "#0f1216"];
        const style = { color: foreground || undefined, backgroundColor: background || undefined, fontWeight: run.attr.bold ? 700 : undefined, opacity: run.attr.dim ? .65 : undefined, fontStyle: run.attr.italic ? "italic" : undefined, textDecoration: run.attr.underline ? "underline" : undefined };
        const props = { key: index, style, className: run.cursor ? "dgt-cursor" : undefined };
        return run.attr.link && /^https?:\/\//i.test(run.attr.link)
          ? React.createElement("a", { ...props, href: run.attr.link, target: "_blank", rel: "noreferrer" }, run.text)
          : React.createElement("span", props, run.text);
      })));
      return React.createElement("div", { className: "dgt-terminal", onClick: () => capture.current?.focus() },
        React.createElement("div", { className: "dgt-screen", ref: scroll, role: "log", "aria-live": "off", tabIndex: 0, onKeyDown: key, onPaste: paste, onScroll: event => { const node = event.currentTarget; atBottom.current = node.scrollHeight - node.scrollTop - node.clientHeight < 24; } }, lines),
        React.createElement("textarea", { ref: capture, className: "dgt-terminal-capture", "aria-label": "终端输入", onKeyDown: key, onInput: input, onPaste: paste }),
        React.createElement("div", { className: "dgt-terminal-status" }, React.createElement("span", null, status), React.createElement("span", { className: "dgt-grow" }), React.createElement("span", null, `${cols}×${rows}`), model.title && React.createElement("span", null, model.title)));
    }

    function XtermScreen({ Terminal, text, cols, rows, onInput, status }) {
      const host = React.useRef(null);
      const instance = React.useRef(null);
      const input = React.useRef(onInput);
      const previous = React.useRef("");
      input.current = onInput;
      React.useEffect(() => {
        const terminal = new Terminal({
          cols, rows, cursorBlink: true, cursorStyle: "block", convertEol: false,
          fontFamily: "var(--ds-font-family-code), Consolas, monospace", fontSize: 13,
          lineHeight: 1.2, scrollback: 4000, smoothScrollDuration: 80,
          theme: { background: "#0f1216", foreground: "#d8dee9", cursor: "#eceff4", selectionBackground: "#3b506d99" },
          linkHandler: { activate: (_event, uri) => { if (/^https?:\/\//i.test(uri)) window.open(uri, "_blank", "noopener,noreferrer"); } },
        });
        terminal.open(host.current);
        const data = terminal.onData(value => {
          // The native ConPTY transport answers DSR cursor-position probes
          // even before a Web viewer attaches. Suppress xterm's duplicate
          // reply while preserving user input and every other device reply.
          if (/^\u001b\[\d+;\d+R$/.test(value)) return;
          input.current(value);
        });
        instance.current = terminal;
        previous.current = "";
        terminal.focus();
        return () => { data.dispose(); terminal.dispose(); instance.current = null; previous.current = ""; };
      }, [Terminal]);
      React.useEffect(() => {
        const terminal = instance.current; if (!terminal) return;
        const current = String(text || ""), prior = previous.current;
        if (prior && current.startsWith(prior)) terminal.write(current.slice(prior.length));
        else { terminal.reset(); terminal.write(current); }
        previous.current = current;
      }, [text, Terminal]);
      React.useEffect(() => { instance.current?.resize(cols, rows); }, [cols, rows, Terminal]);
      return React.createElement("div", { className: "dgt-terminal" },
        React.createElement("div", { className: "dgt-xterm-host", ref: host, "data-terminal-engine": "xterm.js-5.5.0" }),
        React.createElement("div", { className: "dgt-terminal-status" }, React.createElement("span", null, status), React.createElement("span", { className: "dgt-grow" }), React.createElement("span", null, `${cols}×${rows} · xterm.js`)));
    }

    function RichTerminalScreen(props) {
      const [Terminal, setTerminal] = React.useState(() => typeof window.Terminal === "function" ? window.Terminal : null);
      React.useEffect(() => { let live = true; loadXterm().then(value => { if (live) setTerminal(() => value); }).catch(() => {}); return () => { live = false; }; }, []);
      return Terminal ? React.createElement(XtermScreen, { ...props, Terminal }) : React.createElement(TerminalScreen, props);
    }

    function terminalKey(entry) { return `${entry.homeSessionId}:${entry.terminalId}`; }

    function WorkbenchTerminal({ sessionId }) {
      useInstalledStyles();
      const [meta, setMeta] = React.useState(null);
      const [entries, setEntries] = React.useState([]);
      const [pins, savePins] = usePins();
      const [active, setActive] = React.useState("");
      const [text, setText] = React.useState("");
      const [error, setError] = React.useState("");
      const [size, setSize] = React.useState({ rows: 30, cols: 120 });
      const [busy, setBusy] = React.useState("");
      const terminalRef = React.useRef(null);
      const inputQueue = React.useRef(Promise.resolve());
      const inputBuffer = React.useRef("");
      const inputTimer = React.useRef(0);
      const selectedRef = React.useRef(null);
      const ownerSession = React.useRef(sessionId);
      const refreshSequence = React.useRef(0);
      if (ownerSession.current !== sessionId) {
        ownerSession.current = sessionId;
        refreshSequence.current += 1;
      }

      const visiblePins = React.useMemo(() => pins.filter(pin => pin.scope === "global" || !meta || pin.workspaceKey === meta.workspaceKey), [pins, meta?.workspaceKey]);
      const ownEntries = React.useMemo(() => entries.map(entry => ({ ...entry, terminalId: entry.id, homeSessionId: sessionId, name: entry.name || `终端 ${entry.id}`, own: true })), [entries, sessionId]);
      const descriptors = React.useMemo(() => {
        const ownKeys = new Set(ownEntries.map(terminalKey));
        return [...ownEntries, ...visiblePins.filter(pin => !ownKeys.has(terminalKey(pin))).map(pin => ({ ...pin, pinned: true, own: pin.homeSessionId === sessionId }))];
      }, [ownEntries, visiblePins, sessionId]);
      const selected = descriptors.find(entry => terminalKey(entry) === active) || descriptors[0] || null;
      selectedRef.current = selected;

      const refresh = React.useCallback(async () => {
        const owner = sessionId, request = ++refreshSequence.current;
        let metadata, list;
        try {
          [metadata, list] = await Promise.all([json(endpoint("meta", owner)), json(endpoint("terminal-list", owner))]);
        } catch (failure) {
          if (ownerSession.current !== owner || request !== refreshSequence.current) return false;
          throw failure;
        }
        if (ownerSession.current !== owner || request !== refreshSequence.current) return false;
        setMeta(metadata); setEntries(list.entries || []); setError("");
        return true;
      }, [sessionId]);

      React.useEffect(() => {
        const owner = sessionId;
        setMeta(null); setEntries([]); setActive(""); setText(""); setError("");
        refresh().catch(failure => { if (ownerSession.current === owner) setError(failure.message); });
        return () => { refreshSequence.current += 1; };
      }, [sessionId]);
      React.useEffect(() => { if (selected && active !== terminalKey(selected)) setActive(terminalKey(selected)); }, [selected && terminalKey(selected)]);
      React.useEffect(() => () => { clearTimeout(inputTimer.current); inputBuffer.current = ""; }, [selected && terminalKey(selected)]);

      React.useEffect(() => {
        if (!selected) { setText(""); return; }
        let live = true;
        const read = async () => {
          try {
            const value = await json(endpoint("terminal-read", selected.homeSessionId, { terminalId: selected.terminalId, count: 2000 }));
            if (live) { setText(value.text || ""); setError(""); }
          } catch (failure) { if (live) setError(`终端已断开：${failure.message}`); }
        };
        void read(); const timer = setInterval(read, 350);
        return () => { live = false; clearInterval(timer); };
      }, [selected && terminalKey(selected)]);

      React.useEffect(() => {
        if (!terminalRef.current || !selected || typeof ResizeObserver === "undefined") return;
        let timer = 0, last = "";
        const observer = new ResizeObserver(records => {
          const box = records[0]?.contentRect; if (!box) return;
          const next = { cols: Math.max(10, Math.min(500, Math.floor((box.width - 20) / 7.85))), rows: Math.max(2, Math.min(500, Math.floor((box.height - 28) / 18.85))) };
          setSize(next);
          const signature = `${next.rows}:${next.cols}`; if (signature === last) return; last = signature;
          clearTimeout(timer); timer = setTimeout(() => post("/__dsh-preview/terminal-action", { sessionId: selected.homeSessionId, terminalId: selected.terminalId, action: "resize", ...next }).catch(failure => { if (ownerSession.current === sessionId) setError(failure.message); }), 120);
        });
        observer.observe(terminalRef.current); return () => { clearTimeout(timer); observer.disconnect(); };
      }, [selected && terminalKey(selected)]);

      const terminalAction = (entry, action, extra) => post("/__dsh-preview/terminal-action", { sessionId: entry.homeSessionId, terminalId: entry.terminalId, action, ...(extra || {}) });
      const open = async () => {
        const owner = sessionId;
        try {
          setBusy("open"); setError("");
          const name = `终端 ${entries.length + 1}`, value = await post("/__dsh-preview/terminal-action", { sessionId: owner, action: "open", name });
          if (ownerSession.current !== owner) return;
          if (await refresh()) { setActive(`${owner}:${value.id}`); setText(value.motd || ""); }
        } catch (failure) { if (ownerSession.current === owner) setError(failure.message); }
        finally { if (ownerSession.current === owner) setBusy(""); }
      };
      const close = async entry => {
        if (!entry || !window.confirm(`关闭 ${entry.name} 并终止其 PTY 进程？`)) return;
        const owner = sessionId;
        try {
          setBusy("close"); await terminalAction(entry, "close");
          if (ownerSession.current !== owner) return;
          savePins(pins.filter(pin => terminalKey(pin) !== terminalKey(entry)));
          if (entry.homeSessionId === sessionId) await refresh();
          if (ownerSession.current !== owner) return;
          setActive(""); setText("");
        } catch (failure) { if (ownerSession.current === owner) setError(failure.message); }
        finally { if (ownerSession.current === owner) setBusy(""); }
      };
      const flushInput = () => {
        clearTimeout(inputTimer.current); inputTimer.current = 0;
        const entry = selectedRef.current;
        const data = inputBuffer.current.slice(0, 4096);
        inputBuffer.current = inputBuffer.current.slice(data.length);
        if (!entry || !data) return;
        inputQueue.current = inputQueue.current
          .catch(() => {})
          .then(() => terminalAction(entry, "input", { text: data }))
          .catch(failure => { if (ownerSession.current === sessionId) setError(failure.message); });
        if (inputBuffer.current) inputTimer.current = setTimeout(flushInput, 12);
      };
      const sendInput = data => {
        if (!selectedRef.current || !data) return;
        inputBuffer.current += data;
        if (inputBuffer.current.length >= 4096) flushInput();
        else if (!inputTimer.current) inputTimer.current = setTimeout(flushInput, 12);
      };
      const setPin = scope => {
        if (!selected || !meta) return;
        const without = pins.filter(pin => terminalKey(pin) !== terminalKey(selected));
        if (!scope) { savePins(without); return; }
        savePins([...without, { terminalId: selected.terminalId, homeSessionId: selected.homeSessionId, workspaceKey: selected.workspaceKey || meta.workspaceKey, name: selected.name, scope, pid: selected.pid }]);
      };
      const currentPin = selected ? pins.find(pin => terminalKey(pin) === terminalKey(selected)) : null;

      return React.createElement("section", { className: "dgt-shell", "aria-label": "终端工作台" },
        React.createElement("div", { className: "dgt-terminal-tabs" }, descriptors.map(entry => React.createElement("button", { key: terminalKey(entry), className: "dgt-terminal-tab", "data-active": selected && terminalKey(selected) === terminalKey(entry) || undefined, "data-pinned": pins.some(pin => terminalKey(pin) === terminalKey(entry)) || undefined, title: `${entry.name} · ${entry.homeSessionId === sessionId ? "当前会话" : "固定终端"}`, onClick: () => setActive(terminalKey(entry)) }, React.createElement("span", null, entry.name))),
          React.createElement("button", { className: "dgt-terminal-tab", disabled: entries.length >= 3 || busy === "open", onClick: open }, busy === "open" ? "启动中…" : "+ 新建")),
        React.createElement("div", { className: "dgt-toolbar" },
          React.createElement("strong", { className: "dgt-grow" }, selected?.name || "终端"),
          React.createElement("select", { className: "dgt-pin-select", value: currentPin?.scope || "", disabled: !selected || !meta, "aria-label": "固定终端范围", onChange: event => setPin(event.target.value || null) },
            React.createElement("option", { value: "" }, "不固定"), React.createElement("option", { value: "workspace" }, "固定到工作区"), React.createElement("option", { value: "global" }, "固定到全局")),
          React.createElement("button", { disabled: !selected, onClick: () => terminalAction(selected, "signal", { signal: "SIGINT" }).catch(failure => { if (ownerSession.current === sessionId) setError(failure.message); }), title: "中断当前前台命令" }, "Ctrl+C"),
          React.createElement("button", { disabled: !selected || !!busy, onClick: () => close(selected) }, "关闭"),
          React.createElement("button", { disabled: !!busy, onClick: () => refresh().catch(failure => { if (ownerSession.current === sessionId) setError(failure.message); }), title: "刷新终端列表" }, "↻")),
        error && React.createElement("div", { className: "dgt-error", role: "alert" }, error),
        selected
          ? React.createElement("div", { ref: terminalRef, style: { minHeight: 0, flex: 1, display: "flex" } }, React.createElement(RichTerminalScreen, { text, cols: size.cols, rows: size.rows, onInput: sendInput, status: `${/^Win/i.test(navigator.platform || "") ? "cmd.exe" : (selected.type || "shell")} · ${selected.status || (error ? "disconnected" : "running")} · PID ${selected.pid || "—"}` }))
          : React.createElement("div", { className: "dgt-empty" }, "新建终端后可直接输入、运行交互式程序，并固定到当前工作区或全部会话。"));
    }

    exports.GitWorkbench = GitWorkbench;
    exports.WorkbenchTerminal = WorkbenchTerminal;
    exports.installStyles = installStyles;
    exports.parseDiff = parseDiff;
    exports.AnsiTerminalModel = AnsiTerminalModel;
    exports.safePins = safePins;
    exports.PIN_KEY = PIN_KEY;
    return module.exports;
  },
});
