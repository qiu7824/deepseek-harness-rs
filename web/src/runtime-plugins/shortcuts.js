window.__ModuleLoader__.load({
  id: "@deepseek-ai/dsh-client-shortcuts",
  factory: () => {
    "use strict";
    const STORAGE_KEY = "dsh.keybindings.v1";
    const modifiers = ["control", "alt", "shift", "meta"];
    const object = value => value !== null && typeof value === "object" && !Array.isArray(value);
    const codePattern = /^(Key[A-Z]|Digit[0-9]|F([1-9]|1[0-9]|2[0-4])|Arrow(Up|Down|Left|Right)|Backquote|Backslash|BracketLeft|BracketRight|Comma|Period|Slash|Semicolon|Quote|Minus|Equal|Enter|Escape|Tab|Space|Backspace|Delete|Insert|Home|End|PageUp|PageDown)$/;
    const detectPlatform = nav => /Mac|iPhone|iPad/.test(nav?.userAgentData?.platform || nav?.platform || "") ? "macos" : /Win/.test(nav?.userAgentData?.platform || nav?.platform || "") ? "windows" : "linux";
    function normalize(binding, platform) {
      if (binding === null) return null;
      if (!object(binding) || Object.keys(binding).some(key => !["code", "modifiers", "secondCode"].includes(key)) || !codePattern.test(binding.code) || !Array.isArray(binding.modifiers) || binding.modifiers.some(key => ![...modifiers, "primary"].includes(key)) || new Set(binding.modifiers).size !== binding.modifiers.length || (binding.secondCode !== undefined && (!codePattern.test(binding.secondCode) || binding.secondCode === binding.code))) throw Error("无效的快捷键配置");
      const expanded = binding.modifiers.map(key => key === "primary" ? platform === "macos" ? "meta" : "control" : key);
      if (new Set(expanded).size !== expanded.length) throw Error("重复的修饰键");
      return { code: binding.code, modifiers: modifiers.filter(key => expanded.includes(key)), ...(binding.secondCode ? { secondCode: binding.secondCode } : {}) };
    }
    const bindingKey = binding => binding === null ? "" : [...binding.modifiers, binding.code, binding.secondCode || ""].join("+");
    function parseDocument(raw) {
      if (raw === null) return { schemaVersion: 1, profiles: {} };
      const value = JSON.parse(raw);
      if (!object(value) || ![1, 2].includes(value.schemaVersion) || !object(value.profiles) || Object.keys(value).some(key => !["schemaVersion", "profiles"].includes(key))) throw Error("快捷键配置无法读取，请保留并修复 dsh.keybindings.v1 后重新加载页面。");
      for (const [profile, overrides] of Object.entries(value.profiles)) {
        if (!/^(web|desktop):(windows|macos|linux)$/.test(profile) || !object(overrides)) throw Error("无效的快捷键配置分组");
        for (const [id, binding] of Object.entries(overrides)) {
          if (!/^[a-z][a-zA-Z0-9-]*(\.[a-zA-Z][a-zA-Z0-9-]*)+$/.test(id)) throw Error("无效的快捷键操作标识");
          normalize(binding, profile.split(":")[1]);
          if (value.schemaVersion === 1 && binding?.secondCode) throw Error("快捷键配置版本不匹配");
        }
      }
      return value;
    }
    function bindingIssue(binding, platform) {
      if (binding === null) return null;
      if (binding.secondCode) return "浏览器不支持双普通键组合。";
      const m = binding.modifiers, primary = platform === "macos" ? "meta" : "control";
      if (platform !== "linux" && (m.length >= 3 || (m.length === 1 && ((m[0] === primary && ["Comma", "Backslash"].includes(binding.code)) || (m[0] === "control" && binding.code === "Backquote"))) || (m.length === 2 && m.includes(primary) && (m.includes("alt") || m.includes("shift"))))) return null;
      if ((m.length === 1 && m[0] === primary && binding.code === "Slash") || (m.length === 2 && m.includes(primary) && m.includes("shift") && ["Comma", "Period"].includes(binding.code))) return null;
      return "此组合由浏览器、系统或文字编辑使用，请选择受支持的组合。";
    }
    function label(binding, platform) {
      if (!binding) return "未绑定";
      const names = { control: "Ctrl", alt: platform === "macos" ? "Option" : "Alt", shift: "Shift", meta: platform === "macos" ? "Cmd" : "Win" };
      const key = code => ({ Slash: "/", Comma: ",", Period: ".", Backslash: "\\", Backquote: "`", Space: "Space" }[code] || code.replace(/^(Key|Digit)/, ""));
      return [...binding.modifiers.map(part => names[part]), key(binding.code), ...(binding.secondCode ? [key(binding.secondCode)] : [])].join("+");
    }
    function createShortcuts(win, platform = detectPlatform(win.navigator)) {
      const profile = `web:${platform}`, commands = new Map(), listeners = new Set();
      let raw = null, documentValue = { schemaVersion: 1, profiles: {} }, readError = "", revision = 0, recording = 0, disposed = false, composing = false, snapshot;
      const emit = () => { for (const listener of [...listeners]) listener(); };
      const rebuild = () => {
        const overrides = documentValue.profiles[profile] || {};
        const rows = [...commands.values()].map(command => {
          const modified = !command.fixed && Object.hasOwn(overrides, command.id);
          const binding = normalize(command.fixed ? command.bindings[0] : modified ? overrides[command.id] : command.defaults?.[profile] ?? null, platform);
          return { id: command.id, label: typeof command.label === "function" ? command.label() : command.label, aliases: command.aliases || [], binding, modified, fixed: !!command.fixed, keys: command.keys || label(binding, platform), conflicts: [], issue: command.fixed ? null : bindingIssue(binding, platform) };
        });
        for (const row of rows.filter(row => !row.fixed && row.binding)) {
          for (const other of rows.filter(other => other.id !== row.id)) {
            const bindings = other.fixed ? commands.get(other.id).bindings.map(binding => normalize(binding, platform)) : [other.binding];
            if (!bindings.some(binding => binding && bindingKey(binding) === bindingKey(row.binding))) continue;
            if (!other.fixed && row.modified && !other.modified) continue;
            row.conflicts.push(other.id);
          }
        }
        snapshot = Object.freeze({ revision: ++revision, platform, profile, rows, writable: !readError, error: readError, customCount: Object.keys(overrides).length });
        emit();
      };
      const read = () => {
        try {
          const nextRaw = win.localStorage.getItem(STORAGE_KEY), next = parseDocument(nextRaw);
          const changed = nextRaw !== raw || !!readError;
          raw = nextRaw; documentValue = next; readError = "";
          if (changed || !snapshot) rebuild();
          return true;
        } catch (error) { readError = String(error.message || error); rebuild(); return false; }
      };
      const register = command => {
        if (disposed || commands.has(command.id)) throw Error(`重复或已关闭的快捷键操作：${command.id}`);
        if (command.fixed && !command.bindings?.length) throw Error("固定操作必须声明组合键");
        for (const binding of command.fixed ? command.bindings : Object.values(command.defaults || {})) normalize(binding, platform);
        commands.set(command.id, command); rebuild();
        let active = true;
        return () => { if (!active) return; active = false; commands.delete(command.id); rebuild(); };
      };
      const service = {
        platform, profile, getSnapshot: () => snapshot,
        subscribe: listener => { listeners.add(listener); return () => listeners.delete(listener); },
        register, registerFixed: command => register({ ...command, fixed: true }),
        label: binding => label(binding, platform), normalize: binding => normalize(binding, platform),
        recording: () => { recording++; let active = true; return () => { if (active) { active = false; recording--; } }; },
        reload: read,
        edit(operation, expected) {
          if (disposed) return { status: "unavailable" };
          if (!read()) return { status: "unreadable" };
          if (snapshot.revision !== expected) return { status: "stale" };
          const next = JSON.parse(JSON.stringify(documentValue));
          const overrides = next.profiles[profile] ||= {};
          if (operation.type === "reset-all") next.profiles[profile] = {};
          else {
            const command = commands.get(operation.id);
            if (!command || command.fixed) return { status: "unavailable" };
            if (operation.type === "reset") delete overrides[operation.id];
            else if (operation.type === "set") {
              let binding;
              try { binding = normalize(operation.binding, platform); } catch (error) { return { status: "invalid", message: error.message }; }
              const issue = bindingIssue(binding, platform);
              if (issue) return { status: "invalid", message: issue };
              if (binding) {
                const conflicts = snapshot.rows.filter(row => row.id !== operation.id && (row.fixed ? commands.get(row.id).bindings : [row.binding]).some(other => other && bindingKey(normalize(other, platform)) === bindingKey(binding)));
                if (conflicts.length) return { status: "conflict", message: `此组合已用于：${conflicts.map(row => row.label).join("、")}` };
              }
              overrides[operation.id] = binding;
            } else return { status: "invalid" };
          }
          try {
            const nextRaw = JSON.stringify(next);
            win.localStorage.setItem(STORAGE_KEY, nextRaw);
            raw = nextRaw; documentValue = next; rebuild();
            return { status: "saved" };
          } catch { return { status: "write-failed", message: "快捷键保存失败，原有绑定仍然有效，请重试。" }; }
        },
        dispose() { if (disposed) return; disposed = true; win.removeEventListener("keydown", keydown); win.removeEventListener("storage", storage); win.removeEventListener("compositionstart", compositionStart, true); win.removeEventListener("compositionend", compositionEnd, true); win.removeEventListener("blur", compositionEnd); listeners.clear(); commands.clear(); }
      };
      function keydown(event) {
        if (disposed || recording || composing || event.isComposing || event.keyCode === 229 || event.defaultPrevented || event.getModifierState?.("AltGraph")) return;
        if (event.key === "Process" || event.key === "Unidentified") return;
        if (event.key === "Dead" && !(platform === "macos" && event.code === "KeyN" && event.altKey && event.metaKey)) return;
        const binding = { code: event.code, modifiers: modifiers.filter(key => event[`${key === "control" ? "ctrl" : key}Key`]) };
        const target = event.composedPath?.()[0] || event.target;
        const region = target?.closest?.(".xterm,[data-terminal],[data-shortcut-region=terminal]") ? "terminal" : target?.closest?.("input,textarea,select,[contenteditable=true]") ? "editable" : "page";
        const modals = [...win.document.querySelectorAll('[role="dialog"][aria-modal="true"]')].filter(element => !element.hidden && element.getAttribute("aria-hidden") !== "true");
        const modal = modals.at(-1)?.dataset.shortcutModal || (modals.length ? "other" : null);
        const row = snapshot.rows.find(row => !row.fixed && row.binding && !row.issue && !row.conflicts.length && bindingKey(row.binding) === bindingKey(binding));
        if (!row) return;
        const command = commands.get(row.id);
        if (!(command.regions || ["page", "editable"]).includes(region) || (modal && !(command.modals || []).includes(modal))) return;
        if (command.available && !command.available({ target, modal, region })) return;
        event.preventDefault();
        if (!event.repeat) { try { Promise.resolve(command.run({ target, modal, region })).catch(error => console.error("Shortcut command failed", error)); } catch (error) { console.error("Shortcut command failed", error); } }
      }
      function storage(event) { if ((event.key === STORAGE_KEY || event.key === null) && (!event.storageArea || event.storageArea === win.localStorage)) read(); }
      function compositionStart() { composing = true; }
      function compositionEnd() { composing = false; }
      read();
      win.addEventListener("keydown", keydown);
      win.addEventListener("storage", storage);
      win.addEventListener("compositionstart", compositionStart, true);
      win.addEventListener("compositionend", compositionEnd, true);
      win.addEventListener("blur", compositionEnd);
      return service;
    }
    return { inject: [], apply(ctx) { ctx.effect(() => { const service = createShortcuts(window); const release = ctx.reflect.provide("shortcuts", service); return () => { service.dispose(); release(); }; }, "shortcuts: service"); }, createShortcuts, normalize, parseDocument, bindingIssue, label, STORAGE_KEY };
  }
});
