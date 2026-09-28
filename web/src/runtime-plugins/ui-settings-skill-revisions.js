window.__ModuleLoader__.load({
  id: "@deepseek-ai/dsh-client-ui-settings-skill-revisions",
  factory: require => {
    const React = require("react"), h = React.createElement;
    async function rpc(method, payload = {}, signal) {
      const response = await fetch("/api/" + method, { method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ type: "client-request", rpcId: crypto.randomUUID(), method, payload }), signal });
      const value = await response.json();
      if (!response.ok || value.result?.ok !== true) throw Error(value.result?.error?.message || value.error || "技能版本操作失败");
      return value.result.value;
    }
    async function taskSamples(candidate, signal) {
      const response = await fetch("/__dsh-task-execution", { method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ action: "list", sessionId: candidate.ownerSessionId, summaryOnly: true }), signal });
      const value = await response.json();
      if (!response.ok) throw Error(value.error || "无法读取验证样本");
      return (value.tasks || []).filter(task => task.state === "completed" && task.spec?.validationSubject?.kind === "skill" && task.spec.validationSubject.identity === candidate.contentHash);
    }
    function SkillRevisionsSection() {
      React.useEffect(() => {
        const style=document.createElement("style");
        style.textContent=".dshSkillRevisions button{font:inherit;padding:7px 12px;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;background:var(--dsw-alias-bg-layer-1);color:inherit;cursor:pointer}.dshSkillRevisions button:disabled{opacity:.45;cursor:default}.dshSkillRevisions :focus-visible{outline:2px solid var(--dsw-alias-state-business-primary);outline-offset:2px}.dshSkillRevisions input{accent-color:var(--dsw-alias-state-business-primary)}.dshSkillRevisions h2,.dshSkillRevisions h3{margin:0}.dshSkillRevisions summary{cursor:pointer}";
        document.head.appendChild(style);return()=>style.remove();
      },[]);
      const [state, setState] = React.useState(null), [detail, setDetail] = React.useState(null), [samples, setSamples] = React.useState([]);
      const [selected, setSelected] = React.useState([]), [error, setError] = React.useState(""), [notice, setNotice] = React.useState(""), [busy, setBusy] = React.useState(false);
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
        return () => { alive.current = false; generation.current++; pending.current=0; abort.current?.abort(); };
      }, []);
      const inspect = async id => {
        if (pending.current) return;
        const token = ++generation.current; pending.current = token;
        abort.current?.abort(); const controller = new AbortController(); abort.current = controller;
        setBusy(true); setError("");
        try {
          const candidate = await rpc("capabilities.skillRevisionRead", { id }, controller.signal);
          const tasks = await taskSamples(candidate, controller.signal);
          if (alive.current && token === generation.current) { setDetail(candidate); setSamples(tasks); setSelected((candidate.samples || []).map(s => s.taskId)); }
        } catch (e) { if (alive.current && token === generation.current && e.name !== "AbortError") setError(e.message); }
        finally { if(pending.current===token)pending.current = 0; if (alive.current && token === generation.current) setBusy(false); }
      };
      const perform = async (action, extra = {}) => {
        if (pending.current || !state) return;
        const token=++generation.current; pending.current = token; abort.current?.abort(); setBusy(true); setError(""); setNotice("");
        try {
          const value = await rpc("capabilities.skillRevision" + action, { expectedRevision: state.revision, ...extra });
          if (alive.current && token===generation.current) {
            setState(value.state || value); setNotice("操作已保存");
            if (action === "Validate") setDetail(old => old ? { ...old, validation: value.evidence } : old);
            if (["Withdraw", "Restore", "Activate"].includes(action)) setDetail(null);
          }
        } catch (e) {
          if (alive.current && token===generation.current) {
            setError(e.message);
            try { const value = await rpc("capabilities.skillRevisionList"); if (alive.current && token===generation.current) setState(value); } catch {}
          }
        } finally { if(pending.current===token)pending.current = 0; if (alive.current && token===generation.current) setBusy(false); }
      };
      const refs = samples.filter(task => selected.includes(task.taskId)).map(task => ({ taskId: task.taskId, revision: task.revision, expectedSuccess: task.spec.validationSubject.expectedOutcome === "success" }));
      const completeSamples = refs.some(s => s.expectedSuccess) && refs.some(s => !s.expectedSuccess);
      return h("section", { className: "dshSkillRevisions", "aria-label": "技能版本与验证", style: { display: "grid", gap: 14, fontSize: 14, lineHeight: 1.6 } },
        h("h2", null, "技能版本与验证"), h("p", null, "候选版本经正向与反向样本验证后启用，适用范围限定为对应项目和运行环境；撤回与恢复保留版本记录。"),
        error && h("p", { role: "alert" }, error), notice && h("p", { role: "status" }, notice),
        state ? h(React.Fragment, null,
          h("label", null, h("input", { type: "checkbox", role: "switch", checked: state.enabled, disabled: busy, onChange: e => perform("Toggle", { enabled: e.target.checked }) }), " 启用经过验证的技能版本"),
          h("button", { type: "button", disabled: busy, onClick: () => load().catch(e => setError(e.message)) }, "刷新版本"),
          !(state.candidates || []).length && h("p", null, "暂无候选版本。可在任务会话中创建技能候选，并为该版本建立正向、反向验收任务。"),
          ...(state.candidates || []).map(candidate => h("article", { key: candidate.id, style: { border: "1px solid var(--dsw-alias-border-l2)", borderRadius: 8, padding: 12 } },
            h("strong", null, candidate.name), h("p", null, candidate.description), h("small", null, candidate.project),
            h("p", null, candidate.withdrawn ? "已撤回" : candidate.active ? "已启用，使用前复核适用性" : candidate.validation ? "有验证记录，尚未启用" : "待验证"),
            h("div", { style: { display: "flex", gap: 8, flexWrap: "wrap" } },
              h("button", { type: "button", disabled: busy, onClick: () => inspect(candidate.id) }, "查看与验证"),
              h("button", { type: "button", disabled: busy || !state.enabled || !candidate.validation || candidate.active || candidate.withdrawn, onClick: () => perform("Activate", { id: candidate.id }) }, "验证并启用"),
              candidate.withdrawn ? h("button", { type: "button", disabled: busy || !state.enabled || !candidate.validation, onClick: () => perform("Restore", { id: candidate.id }) }, "复核并恢复此版本") :
                h("button", { type: "button", disabled: busy, onClick: () => perform("Withdraw", { id: candidate.id }) }, "撤回版本"))))) : h("p", { role: "status" }, "正在读取版本…"),
        detail && h("article", { style: { border: "1px solid var(--dsw-alias-border-l2)", borderRadius: 8, padding: 12 } },
          h("h3", null, detail.name), h("p", null, "选择属于此版本的已验收任务；至少包含一个正向样本和一个反向样本。"),
          h("details", null, h("summary", null, "技能正文"), h("pre", { style: { whiteSpace: "pre-wrap", overflowWrap: "anywhere", maxHeight: 320, overflow: "auto" } }, Array.from(String(detail.content||"")).slice(0,64000).join("")),
            String(detail.content||"").length>64000&&h("p",null,"正文较长，显示前 64000 个字符。"),
            h("button",{type:"button",onClick:()=>{const url=URL.createObjectURL(new Blob([detail.content||""],{type:"text/markdown;charset=utf-8"}));const a=document.createElement("a");a.href=url;a.download=(detail.name||"skill")+".md";a.click();setTimeout(()=>URL.revokeObjectURL(url),0);}},"保存完整正文")),
          !samples.length && h("p", null, "尚无符合条件的验收任务。"),
          ...samples.map(task => h("label", { key: task.taskId, style: { display: "block" } }, h("input", { type: "checkbox", checked: selected.includes(task.taskId), disabled: busy, onChange: e => setSelected(old => e.target.checked ? [...old, task.taskId] : old.filter(id => id !== task.taskId)) }),
            (task.spec.validationSubject.expectedOutcome === "success" ? "正向：" : "反向：") + task.spec.objective)),
          h("button", { type: "button", disabled: busy || detail.withdrawn || !completeSamples, onClick: () => perform("Validate", { id: detail.id, samples: refs }) }, "核验所选样本"),
          h("button", { type: "button", disabled: busy, onClick: () => setDetail(null) }, "关闭详情")));
    }
    return { inject: ["slots"], SkillRevisionsSection, apply(ctx) { ctx.slots.inject("settings.section", () => ctx.slots.register({ name: "settings.section", id: "skill-revisions", order: 22, label: () => "技能版本" }, SkillRevisionsSection)); } };
  }
});
