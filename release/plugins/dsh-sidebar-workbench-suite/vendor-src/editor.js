import { basicSetup } from "codemirror";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { indentWithTab } from "@codemirror/commands";
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { python } from "@codemirror/lang-python";
import { rust } from "@codemirror/lang-rust";
import { html } from "@codemirror/lang-html";
import { css } from "@codemirror/lang-css";

const languageFor = (path) => {
  const extension = String(path || "").split(".").pop().toLowerCase();
  if (["js", "jsx", "ts", "tsx", "mjs", "cjs", "vue"].includes(extension)) return javascript({ typescript: ["ts", "tsx", "vue"].includes(extension), jsx: ["jsx", "tsx"].includes(extension) });
  if (extension === "json") return json();
  if (["md", "mdx", "markdown"].includes(extension)) return markdown();
  if (extension === "py") return python();
  if (extension === "rs") return rust();
  if (["html", "htm", "svg", "xml"].includes(extension)) return html();
  if (["css", "scss", "less"].includes(extension)) return css();
  return [];
};

globalThis.__DSH_SIDEBAR_EDITOR__ = Object.freeze({
  mount({ parent, value, path, onChange, readOnly = false }) {
    let applying = false;
    const state = EditorState.create({
      doc: String(value || ""),
      extensions: [
        basicSetup,
        keymap.of([indentWithTab]),
        languageFor(path),
        EditorState.readOnly.of(readOnly),
        EditorView.lineWrapping,
        EditorView.updateListener.of((update) => {
          if (update.docChanged && !applying) onChange(update.state.doc.toString());
        }),
        EditorView.theme({
          "&": { height: "100%", background: "var(--dsw-alias-bg-base)", color: "var(--dsw-alias-label-primary)" },
          ".cm-scroller": { overflow: "auto", fontFamily: "var(--ds-font-family-code)" },
          ".cm-gutters": { background: "var(--dsw-alias-bg-layer-1)", color: "var(--dsw-alias-label-caption)", border: "0" }
        })
      ]
    });
    const view = new EditorView({ state, parent });
    return {
      setValue(next) {
        const text = String(next || "");
        if (text === view.state.doc.toString()) return;
        applying = true;
        view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } });
        applying = false;
      },
      focus() { view.focus(); },
      destroy() { view.destroy(); }
    };
  }
});
