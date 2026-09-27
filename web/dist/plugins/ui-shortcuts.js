window.__ModuleLoader__.load({
  id: "@deepseek-ai/dsh-client-ui-shortcuts",
  factory: require => {
    "use strict";
    const React = require("react"), { createPortal } = require("react-dom"), h = React.createElement;
    const useSnapshot = service => React.useSyncExternalStore(service.subscribe, service.getSnapshot, service.getSnapshot);
    const defaults = code => Object.fromEntries(["windows", "macos", "linux"].map(platform => [`web:${platform}`, { code, modifiers: ["primary"] }]));
    const css = `.dshShortcutMask{position:fixed;inset:0;z-index:1500;background:#0006;display:grid;place-items:center;padding:16px}.dshShortcutDialog{box-sizing:border-box;width:min(520px,100%);height:min(640px,90dvh);display:flex;flex-direction:column;gap:12px;padding:20px;border:1px solid var(--dsw-alias-border-l2,#aaa);border-radius:16px;background:var(--dsw-alias-bg-base,#fff);color:var(--dsw-alias-label-primary,#222);box-shadow:0 12px 48px #0003}.dshShortcutDialog header,.dshShortcutDialog footer,.dshShortcutRow{display:flex;align-items:center;gap:10px}.dshShortcutDialog h2{font-size:18px;margin:0;flex:1}.dshShortcutDialog button,.dshShortcutEntry button{font:inherit;cursor:pointer;min-height:32px;border:1px solid var(--dsw-alias-border-l2,#aaa);border-radius:8px;background:var(--dsw-specific-selector,#eee);color:inherit;padding:5px 10px}.dshShortcutDialog button:disabled{opacity:.5;cursor:default}.dshShortcutDialog input{box-sizing:border-box;width:100%;border:1px solid var(--dsw-alias-border-l2,#aaa);border-radius:8px;padding:8px 10px;font:inherit;background:transparent;color:inherit}.dshShortcutList{overflow:auto;min-height:0;flex:1}.dshShortcutRow{min-height:42px;margin:3px 0}.dshShortcutName{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.dshShortcutRow kbd{font:inherit;font-size:12px;color:var(--dsw-alias-label-tertiary,#777)}.dshShortcutRow button{font-size:12px}.dshShortcutEditor{border:1px solid var(--dsw-alias-border-l2,#aaa);padding:10px;border-radius:8px;display:flex;gap:8px;flex-wrap:wrap}.dshShortcutEditor p{width:100%;margin:0;font-size:12px}.dshShortcutStatus{font-size:12px;overflow-wrap:anywhere}.dshShortcutEntry{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:14px 0}.dshShortcutEntry small{display:block;color:var(--dsw-alias-label-tertiary,#777);margin-top:4px}.dshShortcutDialog footer{flex-wrap:wrap}.dshShortcutDialog [role=alert]{color:var(--dsw-alias-state-error-primary,#b42318)}`;
    function createVisibility() {
      let state = false; const listeners = new Set();
      return { getSnapshot: () => state, subscribe(fn) { listeners.add(fn); return () => listeners.delete(fn); }, set(value) { if (value === state) return; state = value; for (const fn of [...listeners]) fn(); } };
    }
    function ShortcutEntry({ shortcuts, visibility }) {
      const snapshot = useSnapshot(shortcuts), binding = snapshot.rows.find(row => row.id === "shortcuts.open")?.binding;
      return h("div", { className: "dshShortcutEntry" }, h("div", null, "快捷键", binding && h("small", null, `全局唤起查看 · ${shortcuts.label(binding)}`)), h("button", { type: "button", onClick: () => visibility.set(true), "aria-keyshortcuts": binding ? shortcuts.label(binding).replace(/Cmd/g, "Meta").replace(/Ctrl/g, "Control").replace(/Option/g, "Alt") : undefined }, "编辑快捷键"));
    }
    function ShortcutReference({ shortcuts, visibility }) {
      const open = useSnapshot(visibility), snapshot = useSnapshot(shortcuts);
      const [query, setQuery] = React.useState(""), [editor, setEditor] = React.useState(null), [notice, setNotice] = React.useState(""), [reset, setReset] = React.useState(null);
      const panel = React.useRef(null), search = React.useRef(null), current = React.useRef(null);
      current.current = { open, editor, reset };
      React.useEffect(() => shortcuts.register({ id: "shortcuts.open", label: "查看快捷键", aliases: ["shortcuts", "keyboard shortcuts"], defaults: defaults("Slash"), regions: ["page", "editable", "terminal"], modals: ["settings", "shortcuts"], available: () => !current.current.editor && current.current.reset === null, run: () => visibility.set(!current.current.open) }), [shortcuts, visibility]);
      React.useEffect(() => {
        if (!open) { setQuery(""); setEditor(null); setReset(null); setNotice(""); return; }
        const previous = document.activeElement;
        search.current?.focus();
        return () => { if (previous?.isConnected) previous.focus(); };
      }, [open]);
      React.useEffect(() => editor ? shortcuts.recording() : undefined, [!!editor, shortcuts]);
      React.useEffect(() => { if(reset !== null && reset !== snapshot.revision) { setReset(null); setNotice("配置或操作目录已变化，请重新确认恢复默认。"); } }, [reset, snapshot.revision]);
      const stale = editor !== null && editor.revision !== snapshot.revision;
      const close = () => visibility.set(false);
      const save = operation => {
        const expected = operation.type === "reset-all" ? reset : editor?.revision ?? snapshot.revision;
        const result = shortcuts.edit(operation, expected);
        if (result.status === "saved") { setEditor(null); setReset(null); setNotice("快捷键已保存。"); search.current?.focus(); }
        else { setNotice(result.message || ({ stale: "配置或操作目录已变化，请核对最新键位后再保存。", unreadable: "配置无法读取，不能修改或恢复默认。", unavailable: "该操作已不可用。" }[result.status] || "无法保存此快捷键。")); if (operation.type === "reset-all" && result.status === "stale") setReset(null); }
      };
      const capture = event => {
        if (!editor || reset !== null || event.isComposing || event.nativeEvent?.isComposing || event.keyCode === 229 || event.getModifierState?.("AltGraph")) return;
        if (["Tab", "Escape"].includes(event.key)) return;
        if (event.key === "Process" || event.key === "Unidentified" || (event.key === "Dead" && !(snapshot.platform === "macos" && event.code === "KeyN" && event.altKey && event.metaKey))) return;
        if (["Enter", " "].includes(event.key) && !event.ctrlKey && !event.metaKey && !event.altKey && event.target.closest?.("button")) return;
        event.preventDefault(); event.stopPropagation();
        if (event.repeat || ["Control", "Meta", "Alt", "Shift"].includes(event.key)) return;
        try {
          const binding = shortcuts.normalize({ code: event.code, modifiers: [["control", event.ctrlKey], ["alt", event.altKey], ["shift", event.shiftKey], ["meta", event.metaKey]].filter(([, active]) => active).map(([key]) => key) });
          setEditor({ ...editor, binding }); setNotice("");
        } catch { setNotice("此按键无法用于快捷键。"); }
      };
      if (!open) return null;
      const tokens = query.toLowerCase().replace(/\s+/g, "");
      const rows = snapshot.rows.filter(row => [row.label, row.id, ...row.aliases, row.keys].some(text => String(text).toLowerCase().replace(/\s+/g, "").includes(tokens)));
      return createPortal(h("div", { className: "dshShortcutMask", onPointerDown: event => { if (event.target === event.currentTarget) close(); } }, h("section", { ref: panel, className: "dshShortcutDialog", role: "dialog", "aria-modal": true, "aria-label": "快捷键", "data-shortcut-modal": "shortcuts", onKeyDownCapture: capture, onKeyDown: event => {
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); if (reset !== null) setReset(null); else if (editor) setEditor(null); else close(); }
        if (event.key === "Tab") { event.stopPropagation(); const nodes = [...panel.current.querySelectorAll("button:not([disabled]),input:not([disabled])")]; const first = nodes[0], last = nodes.at(-1); if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); } else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); } }
      } }, h("header", null, h("h2", null, "快捷键"), h("button", { type: "button", "aria-label": "关闭快捷键", onClick: close }, "关闭")), h("input", { ref: search, "aria-label": "搜索快捷键", placeholder: "搜索操作、英文名称或组合键", value: query, onChange: event => setQuery(event.target.value) }), snapshot.error && h("p", { role: "alert", className: "dshShortcutStatus" }, snapshot.error),
        h("div", { className: "dshShortcutList" }, rows.length === 0 && h("p", null, "未找到匹配的快捷键"), rows.map(row => h("div", { key: row.id }, h("div", { className: "dshShortcutRow" }, h("span", { className: "dshShortcutName", title: row.label }, row.label), row.fixed ? h("kbd", null, row.keys) : h("button", { type: "button", disabled: !snapshot.writable, "aria-label": `修改${row.label}`, onClick: () => { setEditor({ id: row.id, binding: row.binding, revision: snapshot.revision }); setNotice(""); setReset(null); } }, row.keys, row.modified ? " *" : "")), editor?.id === row.id && h("div", { className: "dshShortcutEditor" }, h("p", null, `按下新组合键：${shortcuts.label(editor.binding)}；Esc 取消。`), stale && h("p", { role: "alert" }, "配置或操作目录已变化，请核对最新键位。"), row.conflicts.length > 0 && h("p", null, "当前组合与其他操作冲突，已停止分发。"), row.issue && h("p", null, row.issue), stale && h("button", { type: "button", disabled: !snapshot.writable, onClick: () => { setEditor({ ...editor, revision: snapshot.revision }); setNotice("已采用最新配置版本，请核对草稿后保存。"); } }, "核对后重试"), h("button", { type: "button", disabled: stale || !snapshot.writable, onClick: () => save({ type: "set", id: row.id, binding: editor.binding }) }, "保存"), h("button", { type: "button", disabled: stale || !snapshot.writable, onClick: () => save({ type: "set", id: row.id, binding: null }) }, "清除绑定"), h("button", { type: "button", disabled: stale || !snapshot.writable, onClick: () => save({ type: "reset", id: row.id }) }, "恢复此项默认"), h("button", { type: "button", onClick: () => setEditor(null) }, "取消修改"))))),
        notice && h("p", { role: "status", className: "dshShortcutStatus" }, notice), h("footer", null, reset === null ? h("button", { type: "button", disabled: !snapshot.writable || snapshot.customCount === 0, onClick: () => setReset(snapshot.revision) }, "恢复全部默认") : h(React.Fragment, null, h("span", null, "恢复当前平台全部快捷键？"), h("button", { type: "button", autoFocus: true, onClick: () => setReset(null) }, "取消恢复"), h("button", { type: "button", disabled: !snapshot.writable || reset !== snapshot.revision, onClick: () => save({ type: "reset-all" }) }, "确认恢复")), h("small", null, `${snapshot.profile}${snapshot.customCount ? ` · ${snapshot.customCount} 项自定义` : ""}`)))), document.body);
    }
    function apply(ctx) {
      const shortcuts = ctx.shortcuts, visibility = createVisibility();
      ctx.effect(() => { const style = document.createElement("style"); style.dataset.shortcutControls = ""; style.textContent = css; document.head.append(style); return () => style.remove(); }, "shortcuts: styles");
      ctx.slots.inject("settings.general.item", () => ctx.slots.register({ name: "settings.general.item", id: "shortcuts", order: 20, inject: () => ({ shortcuts, visibility }) }, ShortcutEntry));
      ctx.slots.inject("shell.overlay", () => ctx.slots.register({ name: "shell.overlay", id: "shortcuts", inject: () => ({ shortcuts, visibility }) }, ShortcutReference));
    }
    return { inject: ["slots", "shortcuts"], apply, ShortcutEntry, ShortcutReference, createVisibility };
  }
});
