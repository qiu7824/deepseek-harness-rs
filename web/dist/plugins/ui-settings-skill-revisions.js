window.__ModuleLoader__.load({
  id: "@deepseek-ai/dsh-client-ui-settings-skill-revisions",
  factory: require => {
    const React = require("react"), h = React.createElement;
    let currentProject = () => "";
    async function rpc(method, payload = {}, signal) {
      const response = await fetch("/api/" + method, { method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ type: "client-request", rpcId: crypto.randomUUID(), method, payload }), signal });
      const value = await response.json();
      if (!response.ok || value.result?.ok !== true) throw Error(value.result?.error?.message || value.error || "技能版本操作失败");
      return value.result.value;
    }
    function SkillRevisionsSection() {
      React.useEffect(() => {
        const style = document.createElement("style");
        style.textContent = ".dshSkillRevisions button,.dshSkillRevisions input,.dshSkillRevisions textarea{font:inherit;padding:7px 12px;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;background:var(--dsw-alias-bg-layer-1);color:inherit}.dshSkillRevisions button{cursor:pointer}.dshSkillRevisions button:disabled{opacity:.45;cursor:default}.dshSkillRevisions :focus-visible{outline:2px solid var(--dsw-alias-state-business-primary);outline-offset:2px}.dshSkillRevisions input{accent-color:var(--dsw-alias-state-business-primary)}.dshSkillRevisions h2,.dshSkillRevisions h3{margin:0}.dshSkillRevisions summary{cursor:pointer}.dshSkillRevisions form,.dshSkillRevisions form label{display:grid;gap:8px}.dshSkillRevisions form{gap:14px}.dshSkillRevisions textarea{resize:vertical;min-height:90px}";
        document.head.appendChild(style); return () => style.remove();
      }, []);
      const [state, setState] = React.useState(null), [detail, setDetail] = React.useState(null), [draft, setDraft] = React.useState(null);
      const [error, setError] = React.useState(""), [notice, setNotice] = React.useState(""), [busy, setBusy] = React.useState(false);
      const generation = React.useRef(0), alive = React.useRef(true), pending = React.useRef(0), abort = React.useRef(null);
      const load = async () => {
        if (pending.current) return;
        const token = ++generation.current;
        abort.current?.abort(); const controller = new AbortController(); abort.current = controller;
        const value = await rpc("capabilities.skillRevisionList", {}, controller.signal);
        if (alive.current && token === generation.current) { setState(value); setError(""); }
      };
      React.useEffect(() => {
        alive.current = true; load().catch(e => { if (alive.current) setError(e.message); });
        return () => { alive.current = false; generation.current++; pending.current = 0; abort.current?.abort(); };
      }, []);
      const inspect = async id => {
        if (pending.current) return;
        const token = ++generation.current; pending.current = token;
        abort.current?.abort(); const controller = new AbortController(); abort.current = controller;
        setBusy(true); setError("");
        try {
          const candidate = await rpc("capabilities.skillRevisionRead", { id }, controller.signal);
          if (alive.current && token === generation.current) { setDetail(candidate); setDraft(null); }
        } catch (e) { if (alive.current && token === generation.current && e.name !== "AbortError") setError(e.message); }
        finally { if (pending.current === token) pending.current = 0; if (alive.current && token === generation.current) setBusy(false); }
      };
      const perform = async (action, extra = {}) => {
        if (pending.current || !state) return;
        const token = ++generation.current; pending.current = token; abort.current?.abort(); setBusy(true); setError(""); setNotice("");
        try {
          const value = await rpc("capabilities.skillRevision" + action, { expectedRevision: state.revision, ...extra });
          if (alive.current && token === generation.current) {
            setState(value.state || value); setNotice(action === "Create" ? "版本已保存，尚未启用" : "操作已保存");
            setDetail(null); if (action === "Create") setDraft(null);
          }
        } catch (e) {
          if (alive.current && token === generation.current) {
            setError(e.message);
            try { const value = await rpc("capabilities.skillRevisionList"); if (alive.current && token === generation.current) setState(value); } catch {}
          }
        } finally { if (pending.current === token) pending.current = 0; if (alive.current && token === generation.current) setBusy(false); }
      };
      const edit = value => { setDraft({ name: value?.name || "", description: value?.description || "", project: value?.project || currentProject(), content: value?.content || "" }); setDetail(null); setError(""); };
      const actionStyle = { display: "flex", gap: 8, flexWrap: "wrap" }, cardStyle = { border: "1px solid var(--dsw-alias-border-l2)", borderRadius: 8, padding: 12 };
      return h("section", { className: "dshSkillRevisions", "aria-label": "技能版本", style: { display: "grid", gap: 14, fontSize: 14, lineHeight: 1.6 } },
        h("h2", null, "技能版本"), h("p", null, "手动选择对应项目使用的技能版本。编辑会保存新版本，启用、撤回和恢复均由你选择。"),
        error && h("p", { role: "alert" }, error), notice && h("p", { role: "status" }, notice),
        state ? h(React.Fragment, null,
          h("label", null, h("input", { type: "checkbox", role: "switch", checked: state.enabled, disabled: busy, onChange: e => perform("Toggle", { enabled: e.target.checked }) }), " 启用项目技能版本"),
          h("div", { style: actionStyle }, h("button", { type: "button", disabled: busy, onClick: () => load().catch(e => setError(e.message)) }, "刷新版本"),
            h("button", { type: "button", disabled: busy || !state.enabled, onClick: () => edit(null) }, "创建技能版本")),
          !(state.candidates || []).length && h("p", null, "暂无技能版本。"),
          ...(state.candidates || []).map(candidate => h("article", { key: candidate.id, style: cardStyle },
            h("strong", null, candidate.name), h("p", null, candidate.description), h("small", null, candidate.project),
            h("p", null, candidate.withdrawn ? "已撤回" : candidate.active ? "已启用" : "未启用"),
            h("div", { style: actionStyle },
              h("button", { type: "button", disabled: busy, onClick: () => inspect(candidate.id) }, "查看版本"),
              h("button", { type: "button", disabled: busy || !state.enabled || candidate.active || candidate.withdrawn, onClick: () => perform("Activate", { id: candidate.id }) }, "启用版本"),
              candidate.withdrawn ? h("button", { type: "button", disabled: busy || !state.enabled, onClick: () => perform("Restore", { id: candidate.id }) }, "恢复版本") :
                h("button", { type: "button", disabled: busy, onClick: () => perform("Withdraw", { id: candidate.id }) }, "撤回版本"))))) : h("p", { role: "status" }, "正在读取版本…"),
        draft && h("form", { onSubmit: event => { event.preventDefault(); perform("Create", draft); }, style: cardStyle },
          h("h3", null, "保存技能版本"), h("p", null, "同名技能的旧版本仍会保留，保存后需要单独启用。"),
          ...[["name", "技能名称", 80], ["description", "说明", 1024], ["project", "项目目录", 8192], ["content", "技能正文", 262144]].map(([key, label, maxLength]) =>
            h("label", { key }, label, h(key === "content" ? "textarea" : "input", { required: true, value: draft[key], maxLength, rows: key === "content" ? 12 : undefined, disabled: busy, onChange: e => setDraft(old => ({ ...old, [key]: e.target.value })) }))),
          h("div", { style: actionStyle }, h("button", { type: "submit", disabled: busy || !state?.enabled }, "保存新版本"), h("button", { type: "button", disabled: busy, onClick: () => setDraft(null) }, "取消编辑"))),
        detail && h("article", { style: cardStyle },
          h("h3", null, detail.name), h("p", null, detail.project),
          h("details", null, h("summary", null, "技能正文"), h("pre", { style: { whiteSpace: "pre-wrap", overflowWrap: "anywhere", maxHeight: 320, overflow: "auto" } }, Array.from(String(detail.content || "")).slice(0, 64000).join("")),
            String(detail.content || "").length > 64000 && h("p", null, "正文较长，显示前 64000 个字符。")),
          h("div", { style: actionStyle }, h("button", { type: "button", disabled: busy || !state?.enabled, onClick: () => edit(detail) }, "编辑为新版本"),
            h("button", { type: "button", onClick: () => { const url = URL.createObjectURL(new Blob([detail.content || ""], { type: "text/markdown;charset=utf-8" })); const a = document.createElement("a"); a.href = url; a.download = (detail.name || "skill") + ".md"; a.click(); setTimeout(() => URL.revokeObjectURL(url), 0); } }, "保存完整正文"),
            h("button", { type: "button", disabled: busy, onClick: () => setDetail(null) }, "关闭详情"))));
    }
    return { inject: ["slots"], SkillRevisionsSection, apply(ctx) {
      currentProject = () => { const state = ctx.get?.("sessions")?.list?.getSnapshot(); return state?.byId?.[state.current]?.cwd || ""; };
      ctx.slots.inject("settings.section", () => ctx.slots.register({ name: "settings.section", id: "skill-revisions", order: 22, label: () => "技能版本" }, SkillRevisionsSection));
    } };
  }
});
