window.__ModuleLoader__.load({
  id: "@deepseek-ai/dsh-client-ui-settings-capabilities",
  factory: (require) => {
    const React = require("react");
    const { jsx, jsxs } = require("react/jsx-runtime");
    const css = ".dshCaps{display:flex;flex-direction:column;gap:16px;color:var(--dsw-alias-label-primary);padding:4px 2px 24px}.dshCaps h2{font-size:18px;line-height:26px;margin:0;font-weight:600}.dshCaps p{margin:0;line-height:20px}.dshCapsHint{font-size:12px;color:var(--dsw-alias-label-tertiary);overflow-wrap:anywhere}.dshCapsBar{display:flex;gap:8px;align-items:center;flex-wrap:wrap}.dshCaps button{font:inherit;font-size:13px;cursor:pointer;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;padding:6px 12px;color:inherit;background:var(--dsw-alias-bg-layer-1)}.dshCaps button:hover{background:var(--dsw-alias-interactive-bg-hover)}.dshCaps button:disabled{opacity:.5;cursor:wait}.dshCaps button[aria-selected=true]{background:var(--dsw-specific-sidebar-nav-item-active);border-color:var(--dsw-alias-border-l1)}.dshCaps input,.dshCaps textarea,.dshCaps select{box-sizing:border-box;color:inherit;background:var(--dsw-alias-bg-layer-1);border:1px solid var(--dsw-alias-border-l2);border-radius:8px;padding:8px 10px;font:inherit;font-size:13px;min-width:0}.dshCaps input:focus-visible,.dshCaps textarea:focus-visible,.dshCaps button:focus-visible,.dshCaps select:focus-visible{outline:2px solid var(--dsw-alias-state-business-primary);outline-offset:1px}.dshCaps input[type=search]{flex:1;min-width:150px}.dshCaps input[type=checkbox]{width:16px;height:16px;accent-color:var(--dsw-alias-state-business-primary)}.dshCapsList{display:flex;flex-direction:column;gap:10px}.dshCapsCard{border:1px solid var(--dsw-alias-border-l2);border-radius:12px;padding:14px;display:flex;flex-direction:column;gap:9px;background:var(--dsw-alias-bg-layer-1)}.dshCapsName{font-size:14px;font-weight:600;flex:1;overflow-wrap:anywhere}.dshCapsState{font-size:11px;border-radius:6px;padding:2px 7px;background:var(--dsw-alias-bg-layer-2);color:var(--dsw-alias-label-secondary)}.dshCapsState[data-state=connected]{color:var(--dsw-alias-state-success-primary)}.dshCapsState[data-state=error],.dshCapsError{color:var(--dsw-alias-state-error-primary)}.dshCapsError{font-size:13px;line-height:20px;overflow-wrap:anywhere}.dshCapsEditor{display:flex;flex-direction:column;gap:12px;border:1px solid var(--dsw-alias-border-l1);border-radius:12px;padding:16px}.dshCapsEditor label{display:flex;flex-direction:column;gap:5px;font-size:13px}.dshCapsEditor textarea{resize:vertical;min-height:100px;font-family:var(--ds-font-family-code,monospace)}.dshCapsEditor label.dshCapsCheck{flex-direction:row;align-items:center}.dshCapsDetails{white-space:pre-wrap;font-size:13px;color:var(--dsw-alias-label-secondary);overflow-wrap:anywhere}.dshCapsNotice{font-size:13px;color:var(--dsw-alias-state-success-primary)}";
    if (typeof document !== "undefined" && !document.querySelector("style[data-plugin-css='dsh-capabilities']")) {
      const tag = document.createElement("style"); tag.dataset.pluginCss = "dsh-capabilities"; tag.textContent = css; document.head.appendChild(tag);
    }
    const statusText = { connected: "已连接", disabled: "已停用", error: "连接失败", pending: "等待连接" };
    function parseObject(text, label) {
      if (!text.trim()) return undefined;
      const value = JSON.parse(text);
      if (!value || Array.isArray(value) || typeof value !== "object" || Object.values(value).some(item => typeof item !== "string")) throw new Error(`${label} 必须为字符串键值组成的 JSON 对象`);
      return value;
    }
    function CapabilitiesSection({ rpc }) {
      const [tab, setTab] = React.useState("skills"), [query, setQuery] = React.useState(""), [state, setState] = React.useState(null);
      const [draft, setDraft] = React.useState(null), [busy, setBusy] = React.useState(false), [error, setError] = React.useState(null), [notice, setNotice] = React.useState(null);
      const [expanded, setExpanded] = React.useState(null), [confirm, setConfirm] = React.useState(null);
      const upload = React.useRef(null), mounted = React.useRef(true), generation = React.useRef(0);
      const load = React.useCallback(async () => {
        const turn = ++generation.current;
        const value = await rpc("capabilities.list", {});
        if (mounted.current && turn === generation.current) setState(value);
      }, [rpc]);
      React.useEffect(() => { mounted.current = true; load().catch(cause => setError(cause.message)); return () => { mounted.current = false; generation.current++; }; }, [load]);
      const act = async operation => {
        if (busy) return; setBusy(true); setError(null); setNotice(null);
        try { await operation(); await load(); } catch (cause) { if (mounted.current) setError(cause instanceof Error ? cause.message : String(cause)); }
        finally { if (mounted.current) setBusy(false); }
      };
      const mutate = (method, payload) => rpc(`capabilities.${method}`, { ...payload, expectedRevision: state?.revision });
      const field = (key, label, type = "text", placeholder = "") => jsx("label", { children: [label, type === "textarea" ? jsx("textarea", { value: draft[key] ?? "", placeholder, onChange: e => setDraft({ ...draft, [key]: e.target.value }) }) : jsx("input", { type, value: draft[key] ?? "", disabled: key === "name" && draft.edit, placeholder, onChange: e => setDraft({ ...draft, [key]: e.target.value }) })] }, key);
      const changeTab = value => { setTab(value); setQuery(""); setDraft(null); setConfirm(null); setNotice(null); setError(null); };
      const addSkill = () => setDraft({ kind: "skill", name: "", content: "---\nname: my-skill\ndescription: 描述此技能的用途与适用场景\n---\n\n# 技能说明\n\n具体步骤与验证要求。\n" });
      const editSkill = skill => act(async () => { const value = await rpc("capabilities.skillRead", { name: skill.name }); setDraft({ kind: "skill", name: skill.name, content: value.content, edit: true }); });
      const save = () => act(async () => {
        if (draft.kind === "skill") await mutate("skillSave", { name: draft.name.trim(), content: draft.content, overwrite: Boolean(draft.edit) });
        else {
          const args = JSON.parse(draft.argsText || "[]");
          if (!Array.isArray(args) || args.some(value => typeof value !== "string")) throw new Error("参数必须为字符串组成的 JSON 数组");
          const env = parseObject(draft.envText || "", "环境变量"), headers = parseObject(draft.headersText || "", "请求头");
          const value = await mutate("serverSave", { server: { name: draft.name.trim(), transport: draft.transport, command: draft.command || "", args, cwd: draft.cwd || "", endpoint: draft.endpoint || "", enabled: Boolean(draft.enabled), ...(env === undefined ? {} : { env }), ...(headers === undefined ? {} : { headers }) } });
          if (value.status === "error") setError(value.error || "配置已保存，连接失败");
        }
        setDraft(null);
      });
      const rows = (state?.[tab] || []).filter(item => `${item.name} ${item.description || ""} ${item.endpoint || item.command || ""}`.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
      return jsxs("section", { className: "dshCaps", children: [
        jsx("h2", { children: "技能与 MCP" }),
        jsx("p", { className: "dshCapsHint", children: "管理技能与外部工具；开关保存后直接作用于工具目录和后续调用。" }),
        jsxs("div", { className: "dshCapsBar", role: "tablist", "aria-label": "能力类型", children: [["skills", "Skills"], ["servers", "MCP"]].map(([value, label]) => jsx("button", { role: "tab", "aria-selected": tab === value, onClick: () => changeTab(value), children: label }, value)) }),
        jsxs("div", { className: "dshCapsBar", children: [
          jsx("input", { type: "search", "aria-label": "搜索技能或 MCP", placeholder: "搜索名称与描述", value: query, onChange: e => setQuery(e.target.value) }),
          jsx("button", { disabled: busy, onClick: () => act(load), children: "刷新" }),
          jsx("button", { disabled: busy, onClick: tab === "skills" ? addSkill : () => setDraft({ kind: "server", name: "", transport: "stdio", command: "", argsText: "[]", cwd: "", endpoint: "", envText: "", headersText: "", enabled: false }), children: tab === "skills" ? "添加 Skill" : "添加 MCP" }),
          tab === "skills" && jsx("button", { disabled: busy, onClick: () => upload.current?.click(), children: "导入 SKILL.md" }),
          jsx("input", { ref: upload, type: "file", accept: ".md", style: { display: "none" }, onChange: async e => { const file = e.target.files?.[0]; if (!file) return; if (file.size > 256 * 1024) { setError("Skill 文件超过 256 KiB"); return; } const content = await file.text(); const name = content.match(/^name:\s*["']?([a-z0-9-]+)["']?\s*$/m)?.[1] || ""; setDraft({ kind: "skill", name, content }); e.target.value = ""; } })
        ] }),
        error && jsx("div", { className: "dshCapsError", role: "alert", children: error }),
        notice && jsx("div", { className: "dshCapsNotice", role: "status", children: notice }),
        busy && jsx("div", { className: "dshCapsHint", role: "status", children: "正在处理…" }),
        draft && jsxs("form", { className: "dshCapsEditor", onSubmit: e => { e.preventDefault(); save(); }, children: [
          field("name", "名称", "text", draft.kind === "skill" ? "my-skill" : "my-server"),
          draft.kind === "skill" ? field("content", "SKILL.md", "textarea") : jsxs(React.Fragment, { children: [
            jsx("label", { children: ["连接方式", jsxs("select", { value: draft.transport, onChange: e => setDraft({ ...draft, transport: e.target.value }), children: [jsx("option", { value: "stdio", children: "本地命令（stdio）" }), jsx("option", { value: "http", children: "HTTP / HTTPS" })] })] }),
            draft.transport === "stdio" ? jsxs(React.Fragment, { children: [field("command", "可执行命令", "text", "npx / python / 可执行文件路径"), field("argsText", "参数（JSON 数组）", "textarea", '["-y", "package-name"]'), field("cwd", "工作目录", "text", "留空使用运行目录"), field("envText", "环境变量（JSON 对象）", "textarea", draft.edit ? "留空保留已有值；{} 清空" : '{"API_KEY":"..."}')] }) : jsxs(React.Fragment, { children: [field("endpoint", "服务器地址", "url", "https://example.com/mcp"), field("headersText", "请求头（JSON 对象）", "textarea", draft.edit ? "留空保留已有值；{} 清空" : '{"Authorization":"Bearer ..."}')] }),
            jsx("label", { className: "dshCapsCheck", children: [jsx("input", { type: "checkbox", checked: draft.enabled, onChange: e => setDraft({ ...draft, enabled: e.target.checked }) }), "启用服务器"] }),
            jsx("p", { className: "dshCapsHint", children: "保存并启用将启动本地命令或连接服务器，加载其工具。凭证保存在本机，列表仅显示配置状态。" })
          ] }),
          jsxs("div", { className: "dshCapsBar", children: [jsx("button", { type: "submit", disabled: busy || !draft.name.trim(), children: "保存" }), jsx("button", { type: "button", disabled: busy, onClick: () => setDraft(null), children: "取消" })] })
        ] }),
        !state && !error && jsx("p", { className: "dshCapsHint", children: "正在读取能力目录…" }),
        state && rows.length === 0 && jsx("p", { className: "dshCapsHint", children: query ? "没有匹配的能力" : tab === "skills" ? "暂无技能，可添加或导入 SKILL.md。" : "暂无 MCP 服务器。" }),
        jsx("div", { className: "dshCapsList", children: rows.map(item => jsxs("article", { className: "dshCapsCard", children: [
          jsxs("div", { className: "dshCapsBar", children: [jsx("strong", { className: "dshCapsName", children: item.name }), jsx("span", { className: "dshCapsState", "data-state": item.status, children: tab === "skills" ? item.enabled ? "已启用" : "已停用" : statusText[item.status] || item.status }), jsx("input", { type: "checkbox", role: "switch", "aria-label": `${item.enabled ? "停用" : "启用"} ${item.name}`, checked: item.enabled, disabled: busy, onChange: e => act(() => mutate(tab === "skills" ? "skillToggle" : "serverToggle", { name: item.name, enabled: e.target.checked })) })] }),
          jsx("p", { className: "dshCapsDetails", children: tab === "skills" ? item.description : item.transport === "stdio" ? [item.command, ...(item.args || [])].join(" ") : item.endpoint }),
          jsx("p", { className: "dshCapsHint", children: tab === "skills" ? item.source : `${item.toolCount || 0} 个工具${item.hasSecrets ? " · 已配置凭证" : ""}` }),
          item.error && jsx("p", { className: "dshCapsError", children: item.error }),
          jsxs("div", { className: "dshCapsBar", children: [
            tab === "skills" && item.path && jsx("button", { onClick: () => setExpanded(expanded === item.name ? null : item.name), children: expanded === item.name ? "收起位置" : "查看位置" }),
            (tab === "servers" || item.managed) && jsx("button", { disabled: busy, onClick: () => tab === "skills" ? editSkill(item) : setDraft({ ...item, kind: "server", edit: true, argsText: JSON.stringify(item.args || [], null, 2), envText: "", headersText: "" }), children: "编辑" }),
            tab === "servers" && jsx("button", { disabled: busy, onClick: () => act(async () => { const value = await mutate("serverTest", { name: item.name }); if (value.status === "error") throw new Error(value.error || "连接失败"); setNotice(`${item.name} 连接成功，可用工具 ${value.toolCount} 个`); }), children: "测试连接" }),
            (tab === "servers" || item.managed) && jsx("button", { disabled: busy, onClick: () => setConfirm(item.name), children: "移除" })
          ] }),
          expanded === item.name && jsx("p", { className: "dshCapsHint", children: item.path }),
          confirm === item.name && jsxs("div", { className: "dshCapsBar", children: [jsx("span", { className: "dshCapsHint", children: tab === "skills" ? "Skill 将移入本机回收目录。" : "移除配置并断开服务器。" }), jsx("button", { disabled: busy, onClick: () => act(async () => { await mutate(tab === "skills" ? "skillRemove" : "serverRemove", { name: item.name }); setConfirm(null); }), children: "确认移除" }), jsx("button", { onClick: () => setConfirm(null), children: "取消" })] })
        ] }, item.name)) }),
        tab === "skills" && state && jsx("p", { className: "dshCapsHint", children: `受管技能目录：${state.skillDirectory}` })
      ] });
    }
    return {
      inject: ["slots", "connection"],
      apply(ctx) {
        const connection = ctx.get("connection");
        const rpc = async (method, payload) => { const reply = await connection.rpc.call("/api", method, payload); const result = reply.result || reply; if (!result.ok) throw new Error(result.error?.message || "请求失败"); return result.value; };
        ctx.slots.inject("settings.section", () => ctx.slots.register({ name: "settings.section", id: "capabilities", order: 25, label: () => "技能与 MCP", inject: () => ({ rpc }) }, CapabilitiesSection));
      },
      CapabilitiesSection, parseObject
    };
  }
});
