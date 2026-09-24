import { renderAsync } from "docx-preview";
import JSZip from "jszip";

const MAX_INPUT = 64 * 1024 * 1024;
const MAX_EXPANDED = 128 * 1024 * 1024;
const aborted = () => Object.assign(new Error("文档预览已取消"), { name: "AbortError" });

// Admission reads only the central directory, before any ZIP expansion.
function validateArchive(data) {
  if (!(data instanceof Uint8Array) || data.length > MAX_INPUT || data.length < 22) throw new Error("DOCX 文件无效或超过 64 MiB 预览上限");
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  let end = -1;
  for (let i = data.length - 22; i >= Math.max(0, data.length - 65557); i--) {
    if (view.getUint32(i, true) === 0x06054b50 && i + 22 + view.getUint16(i + 20, true) === data.length) { end = i; break; }
  }
  if (end < 0) throw new Error("DOCX 压缩包不完整");
  const count = view.getUint16(end + 10, true), size = view.getUint32(end + 12, true), start = view.getUint32(end + 16, true);
  if (view.getUint16(end + 4, true) || view.getUint16(end + 6, true) || view.getUint16(end + 8, true) !== count || count > 10000 || start + size !== end) throw new Error("DOCX 压缩包结构不受浏览器预览支持");
  let at = start, expanded = 0, documentFound = false;
  const decoder = new TextDecoder();
  for (let index = 0; index < count; index++) {
    if (at + 46 > end || view.getUint32(at, true) !== 0x02014b50) throw new Error("DOCX 文件目录损坏");
    const nameLength = view.getUint16(at + 28, true), extraLength = view.getUint16(at + 30, true), commentLength = view.getUint16(at + 32, true);
    const next = at + 46 + nameLength + extraLength + commentLength;
    if (next > end || view.getUint16(at + 8, true) & 1) throw new Error("加密或损坏的 DOCX 无法直接预览");
    expanded += view.getUint32(at + 24, true);
    if (expanded > MAX_EXPANDED) throw new Error("文档展开后超过浏览器预览上限，请使用打印版式预览");
    if (decoder.decode(data.subarray(at + 46, at + 46 + nameLength)) === "word/document.xml") documentFound = true;
    at = next;
  }
  if (at !== end || !documentFound) throw new Error("文件不是有效的 DOCX 文档");
}

async function validateContents(data, signal) {
  const archive = await JSZip.loadAsync(data);
  let total = 0;
  for (const entry of Object.values(archive.files)) {
    if (signal?.aborted) throw aborted();
    if (entry.dir) continue;
    await new Promise((resolve, reject) => {
      const stream = entry.internalStream("uint8array");
      let done = false;
      const finish = error => {
        if (done) return; done = true; signal?.removeEventListener("abort", cancel);
        if (error) { stream.pause(); reject(error); } else resolve();
      };
      const cancel = () => finish(aborted());
      signal?.addEventListener("abort", cancel, { once: true });
      stream.on("data", chunk => {
        total += chunk.length;
        if (total > MAX_EXPANDED) finish(new Error("文档展开后超过浏览器预览上限，请使用打印版式预览"));
      }).on("error", finish).on("end", () => finish()).resume();
      if (signal?.aborted) cancel();
    });
  }
}

function open(data, frame, signal, options = {}) {
  let disposed = false, observer, pageNodes = [], doc, scrollListener, scrollTick = 0;
  const dispose = () => {
    if (disposed) return;
    disposed = true; observer?.disconnect(); signal?.removeEventListener("abort", dispose);
    if (doc) { doc.removeEventListener("scroll", scrollListener); doc.defaultView?.cancelAnimationFrame(scrollTick); doc.body?.replaceChildren(); doc.head?.replaceChildren(); }
    pageNodes = [];
  };
  const check = () => { if (disposed || signal?.aborted) throw aborted(); };
  const ready = (async () => {
    check(); validateArchive(data); await validateContents(data, signal); check();
    signal?.addEventListener("abort", dispose, { once: true });
    doc = frame.contentDocument;
    if (!doc) throw new Error("文档预览区域未就绪");
    doc.open();
    doc.write('<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src \'none\'; img-src data:; font-src data:; style-src \'unsafe-inline\'; base-uri \'none\'; form-action \'none\'"><meta name="referrer" content="no-referrer"></head><body></body></html>');
    doc.close();
    // A separate style node keeps the CSP meta outside renderAsync's clearing.
    const styles = doc.createElement("div"), body = doc.createElement("main");
    doc.body.append(styles, body);
    await renderAsync(data, body, styles, {
      className: "docx", inWrapper: true, ignoreWidth: false, ignoreHeight: false,
      breakPages: true, ignoreLastRenderedPageBreak: false, renderHeaders: true,
      renderFooters: true, renderFootnotes: true, renderEndnotes: true,
      renderAltChunks: false, renderComments: false, renderChanges: false,
      // Data URLs have document lifetime and need no shared/global URL hooks.
      useBase64URL: true,
    });
    if (disposed || signal?.aborted) { body.replaceChildren(); styles.replaceChildren(); throw aborted(); }
    const layout = doc.createElement("style");
    layout.textContent = "html{color-scheme:light;background:#eef0f3}body{margin:0;color:#171717}main{min-width:0}.docx-wrapper{padding:16px!important;background:transparent!important;min-width:0;box-sizing:border-box}.docx-wrapper>section.docx{margin:0 auto 16px!important;box-shadow:0 1px 5px #0002}a{color:#2563eb}*{scrollbar-width:thin}";
    doc.head.appendChild(layout);
    pageNodes = [...body.querySelectorAll("section.docx")];
    if (!pageNodes.length) throw new Error("文档没有可显示的内容");
    // The iframe denies scripts and navigation. Links require a real user click.
    doc.addEventListener("click", event => {
      const link = event.target.closest?.("a"); if (!link) return;
      event.preventDefault();
      const href = link.getAttribute("href") || "";
      if (href.startsWith("#")) { doc.getElementById(href.slice(1))?.scrollIntoView?.(); return; }
      try { const url = new URL(href); if (["https:", "http:", "mailto:"].includes(url.protocol)) window.open(url.href, "_blank", "noopener,noreferrer"); } catch {}
    });
    const wrapper = body.querySelector(".docx-wrapper") || body;
    let zoom = "fit", currentPage = 1;
    const goToPage = page => {
      currentPage = Math.min(pageNodes.length, Math.max(1, page));
      pageNodes[currentPage - 1]?.scrollIntoView?.({ block: "start" });
    };
    const applyZoom = () => {
      if (disposed) return;
      const width = Math.max(1, ...pageNodes.map(page => page.offsetWidth || parseFloat(doc.defaultView?.getComputedStyle(page).width) || 816));
      const value = zoom === "fit" ? Math.min(1, Math.max(.15, (frame.clientWidth - 32) / width)) : Number(zoom);
      wrapper.style.zoom = String(value);
      goToPage(currentPage);
    };
    scrollListener = () => {
      if (disposed || scrollTick) return;
      scrollTick = doc.defaultView.requestAnimationFrame(() => {
        scrollTick = 0; if (disposed) return;
        let page = currentPage, largest = 0;
        for (let index = 0; index < pageNodes.length; index++) {
          const rect = pageNodes[index].getBoundingClientRect();
          const visible = Math.max(0, Math.min(frame.clientHeight, rect.bottom) - Math.max(0, rect.top));
          if (visible > largest) { largest = visible; page = index + 1; }
        }
        if (currentPage !== page) { currentPage = page; options.onPageChange?.(page); }
      });
    };
    doc.addEventListener("scroll", scrollListener, { passive: true });
    if (typeof ResizeObserver !== "undefined") { observer = new ResizeObserver(applyZoom); observer.observe(frame); }
    applyZoom();
    return {
      numPages: pageNodes.length,
      setZoom(value) { if (value === "fit" || [.5, .75, 1, 1.25, 1.5, 2].includes(Number(value))) { zoom = value; applyZoom(); } },
      goToPage,
    };
  })().catch(error => { dispose(); throw error; });
  return { ready, dispose };
}

globalThis.__DSH_SIDEBAR_DOCX__ = Object.freeze({ open, validateArchive });
