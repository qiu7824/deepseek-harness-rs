window.__ModuleLoader__.load({
  id: "@deepseek-ai/dsh-client-ui-file-actions",
  factory: () => {
    const displayPath = path => String(path ?? "").replace(/^\\\\\?\\UNC\\/i, "\\\\").replace(/^\\\\\?\\/, "");
    const textExtensions = new Set("txt md rs ts tsx js jsx mjs cjs json jsonl yml yaml toml py go java c h cpp hpp cs css scss sql sh ps1 bat cmd xml csv tsv vue svelte ini log gitignore dockerfile".split(" "));
    const officeExtensions = new Set("doc docx docm xls xlsx xlsm ppt pptx pps ppsx odt ods odp".split(" "));
    let active = null;
    const css = `.dshFileShade{position:fixed;inset:0;z-index:1400;background:#0005;display:flex;align-items:center;justify-content:center;padding:24px;box-sizing:border-box}.dshFileDialog{width:min(1080px,95vw);height:min(860px,90vh);display:flex;flex-direction:column;background:var(--dsw-alias-bg-base,#fff);color:var(--dsw-alias-label-primary,#202124);border:1px solid var(--dsw-alias-border-l2,#ddd);border-radius:14px;box-shadow:0 16px 60px #0003;overflow:hidden}.dshFileHeader{display:flex;align-items:center;gap:8px;padding:12px 16px;border-bottom:1px solid var(--dsw-alias-border-l1,#eee);flex-wrap:wrap}.dshFileTitle{font-size:13px;line-height:20px;flex:1;min-width:180px;overflow-wrap:anywhere;margin:0}.dshFileButton{font:inherit;font-size:12px;border:1px solid var(--dsw-alias-border-l2,#ddd);border-radius:16px;padding:5px 11px;background:transparent;color:inherit;cursor:pointer;white-space:nowrap}.dshFileButton:hover{background:var(--dsw-alias-interactive-bg-hover,#eee)}.dshFileButton:focus-visible,.dshFileMenu button:focus-visible{outline:2px solid var(--dsw-alias-brand-primary,#4769d8);outline-offset:2px}.dshFileBody{min-height:0;flex:1;overflow:auto;padding:14px;position:relative}.dshFileBody iframe{height:100%;width:100%;border:0;background:white}.dshFileBody img,.dshFileBody video{max-width:100%;max-height:100%;display:block;margin:auto}.dshFileBody audio{width:100%}.dshFileStatus{font-size:13px;line-height:22px;color:var(--dsw-alias-label-tertiary,#666);padding:12px;white-space:pre-wrap}.dshFileError{color:var(--dsw-alias-state-error-primary,#b3261e)}.dshFileSource{font:12px/20px ui-monospace,Consolas,monospace;min-width:max-content;tab-size:4}.dshFileLine{display:flex;min-height:20px;white-space:pre}.dshFileLine[data-focus=true]{background:var(--dsw-alias-interactive-bg-hover,#e9edf8)}.dshFileNumber{width:54px;flex:none;text-align:right;padding-right:14px;user-select:none;color:var(--dsw-alias-label-tertiary,#888)}.dshFileMenu{position:fixed;z-index:1450;min-width:176px;padding:5px;background:var(--dsw-alias-bg-base,#fff);color:var(--dsw-alias-label-primary,#202124);border:1px solid var(--dsw-alias-border-l2,#ddd);border-radius:9px;box-shadow:0 5px 24px #0002}.dshFileMenu button{display:block;width:100%;text-align:left;border:0;background:transparent;color:inherit;border-radius:5px;padding:8px 12px;font:13px/20px inherit;cursor:pointer}.dshFileMenu button:hover{background:var(--dsw-alias-interactive-bg-hover,#eee)}`;
    function element(tag, className, text) { const node = document.createElement(tag); if (className) node.className = className; if (text != null) node.textContent = text; return node; }
    function url(operation, sessionId, path) { const query = new URLSearchParams({ sessionId }); if (path != null) query.set("path", path); return `/__dsh-preview/${operation}?${query}`; }
    async function json(request, options) { const response = await fetch(request, options); const value = await response.json(); if (!response.ok) throw new Error(value.message || value.error || `HTTP ${response.status}`); return value; }
    function close() { if (!active) return; const previous = active; active = null; previous.abort.abort(); previous.node.remove(); document.removeEventListener("keydown", previous.keys, true); document.removeEventListener("pointerdown", previous.outside, true); if (previous.focus?.isConnected) previous.focus.focus(); }
    function own(node, dismissOutside = true) {
      close();
      const state = { node, focus: document.activeElement, abort: new AbortController() };
      state.keys = event => {
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); return; }
        const buttons = [...node.querySelectorAll("button:not(:disabled),a[href],input,textarea,[tabindex='0']")];
        if (!buttons.length) return;
        if (["ArrowDown", "ArrowUp"].includes(event.key) && node.classList.contains("dshFileMenu")) { event.preventDefault(); const next = buttons.indexOf(document.activeElement) + (event.key === "ArrowDown" ? 1 : -1); buttons[(next + buttons.length) % buttons.length].focus(); }
        if (event.key === "Tab") { const first = buttons[0], last = buttons[buttons.length - 1]; if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); } else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); } }
      };
      state.outside = event => { if (dismissOutside && !node.contains(event.target)) close(); };
      active = state; document.body.appendChild(node); document.addEventListener("keydown", state.keys, true); document.addEventListener("pointerdown", state.outside, true);
      queueMicrotask(() => node.querySelector("button")?.focus());
      return state;
    }
    function button(label, action) { const node = element("button", "dshFileButton", label); node.type = "button"; node.addEventListener("click", action); return node; }
    async function resolve(options, signal) { return json(url("file-resolve", options.sessionId, options.path), { signal }); }
    async function action(options, intent, signal) {
      const file = await resolve(options, signal);
      if (intent === "copy") { await navigator.clipboard.writeText(file.absolutePath); return file; }
      await json("/__dsh-preview/file-action", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ sessionId: options.sessionId, path: file.path || ".", intent }), signal });
      return file;
    }
    function failure(container, error) { container.replaceChildren(element("div", "dshFileStatus dshFileError", error instanceof Error ? error.message : String(error))); }
    async function preview(options) {
      const shade = element("div", "dshFileShade"), dialog = element("section", "dshFileDialog"), header = element("header", "dshFileHeader"), body = element("div", "dshFileBody");
      dialog.setAttribute("role", "dialog"); dialog.setAttribute("aria-modal", "true"); dialog.setAttribute("aria-label", "文件预览");
      const title = element("h2", "dshFileTitle", displayPath(options.path)); header.appendChild(title);
      const ext = String(options.path).split(".").pop().toLowerCase();
      for (const [label, intent] of [[officeExtensions.has(ext) ? "WPS 打开" : "本地应用打开", officeExtensions.has(ext) ? "office" : "open"], ["在文件夹中显示", "reveal"], ["复制路径", "copy"]]) {
        header.appendChild(button(label, async event => { const control = event.currentTarget; control.disabled = true; try { await action(options, intent, state.abort.signal); if (intent === "copy") { control.textContent = "已复制"; } } catch (error) { if (error.name !== "AbortError") failure(body, error); } finally { control.disabled = false; } }));
      }
      header.appendChild(button("关闭", close)); dialog.append(header, body); shade.appendChild(dialog); const state = own(shade, false); shade.addEventListener("click", event => { if (event.target === shade) close(); });
      body.appendChild(element("div", "dshFileStatus", "正在读取文件…"));
      try {
        const file = await resolve(options, state.abort.signal); if (active !== state) return; title.textContent = displayPath(file.absolutePath);
        if (file.kind === "directory") { body.replaceChildren(element("div", "dshFileStatus", "这是文件夹，可在资源管理器中打开。")); return; }
        const source = url("file", options.sessionId, file.path);
        if (["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "svg"].includes(ext)) { const img = element("img"); img.alt = file.path; img.onerror = () => failure(body, new Error("图片预览失败，可使用本地应用打开")); img.src = source; body.replaceChildren(img); }
        else if (["mp4", "webm", "ogg", "mp3", "wav", "m4a"].includes(ext)) { const media = element(["mp4", "webm"].includes(ext) ? "video" : "audio"); media.controls = true; media.src = source; body.replaceChildren(media); }
        else if (["pdf", "html", "htm"].includes(ext)) {
          const frame = element("iframe"); frame.title = file.path;
          if (ext !== "pdf") { const meta = await json(url("meta", options.sessionId), { signal: state.abort.signal }); frame.setAttribute("sandbox", "allow-scripts"); frame.src = `/__dsh-preview/site/${encodeURIComponent(meta.siteToken)}/${encodeURIComponent(options.sessionId)}/${file.path.split("/").map(encodeURIComponent).join("/")}`; }
          else frame.src = source;
          if (active === state) body.replaceChildren(frame);
        } else if (textExtensions.has(ext) || !String(file.path).split("/").pop().includes(".")) {
          const value = await json(url("source", options.sessionId, file.path), { signal: state.abort.signal }); if (active !== state) return;
          const lines = value.text.split("\n"), focus = Math.max(1, Math.min(lines.length, Number(options.line) || 1)); let start = Math.max(0, focus - 100);
          const render = () => {
            body.replaceChildren(); const end = Math.min(lines.length, start + 500);
            if (start) body.appendChild(button("前 400 行", () => { start = Math.max(0, start - 400); render(); }));
            const code = element("div", "dshFileSource");
            for (let i = start; i < end; i++) { const row = element("div", "dshFileLine"); row.dataset.line = String(i + 1); if (i + 1 === focus) row.dataset.focus = "true"; row.append(element("span", "dshFileNumber", String(i + 1)), element("code", "", lines[i])); code.appendChild(row); }
            body.appendChild(code); body.appendChild(element("div", "dshFileStatus", `${start + 1}–${end} / ${lines.length} 行`));
            if (end < lines.length) body.appendChild(button("后 400 行", () => { start = Math.min(lines.length - 1, start + 400); render(); }));
            queueMicrotask(() => code.querySelector('[data-focus="true"]')?.scrollIntoView({ block: "center" }));
          }; render();
        } else { body.replaceChildren(element("div", "dshFileStatus", officeExtensions.has(ext) ? "此文件可使用 WPS Office 打开。" : "此格式暂不支持内嵌预览，可使用本地应用打开。")); }
      } catch (error) { if (error.name !== "AbortError" && active === state) failure(body, error); }
    }
    function menu(options) {
      const panel = element("div", "dshFileMenu"); panel.setAttribute("role", "menu"); panel.setAttribute("aria-label", "文件操作");
      panel.style.left = `${Math.max(8, Math.min(Number(options.x) || 20, innerWidth - 205))}px`; panel.style.top = `${Math.max(8, Math.min(Number(options.y) || 60, innerHeight - 200))}px`;
      const ext = String(options.path).split(".").pop().toLowerCase();
      for (const [label, intent] of [["预览", "preview"], [officeExtensions.has(ext) ? "WPS 打开" : "本地应用打开", officeExtensions.has(ext) ? "office" : "open"], ["在文件夹中显示", "reveal"], ["复制路径", "copy"]]) {
        const control = button(label, async () => { close(); if (intent === "preview") return preview(options); try { await action(options, intent); } catch (error) { await preview(options); if (active) failure(active.node.querySelector(".dshFileBody"), error); } }); control.setAttribute("role", "menuitem"); panel.appendChild(control);
      }
      own(panel);
    }
    const api = { displayPath, open: options => { if (!options?.sessionId || !options?.path) return Promise.reject(new Error("缺少会话或文件路径")); if (options.intent === "menu") { menu(options); return Promise.resolve(); } return preview(options); }, close };
    function apply() {
      if (!document.querySelector("style[data-dsh-file-actions]")) { const style = element("style"); style.dataset.dshFileActions = ""; style.textContent = css; document.head.appendChild(style); }
      globalThis.__DSH_FILE_ACTIONS__ = api;
    }
    return { apply };
  }
});
