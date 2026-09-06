import { marked } from "marked";
import DOMPurify from "dompurify";

globalThis.__DSH_SIDEBAR_MARKDOWN__ = Object.freeze({
  render(source) {
    const diagrams = [];
    const prepared = String(source || "").replace(/\x60\x60\x60mermaid[^\n]*\n([\s\S]*?)\x60\x60\x60/gi, (_whole, body) => {
      const index = diagrams.push(body) - 1;
      return "\n<div data-dsh-mermaid-index=\"" + index + "\"></div>\n";
    });
    const html = marked.parse(prepared, {
      async: false,
      breaks: false,
      gfm: true
    });
    return {
      diagrams,
      html: DOMPurify.sanitize(html, {
        ADD_ATTR: ["data-dsh-mermaid-index"],
        FORBID_TAGS: ["style", "script", "iframe", "object", "embed", "form"],
        FORBID_ATTR: ["style", "srcdoc"]
      })
    };
  }
});
