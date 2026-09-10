import { basicSetup } from "codemirror";
import { EditorState, Compartment } from "@codemirror/state";
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
  mount({ parent, value, path, onChange, readOnly = false, position, onViewportChange }) {
    let applying = false;
    const wrapping = new Compartment();
    let wrap = position?.wrap !== false;
    const state = EditorState.create({
      doc: String(value || ""),
      extensions: [
        basicSetup,
        keymap.of([indentWithTab]),
        languageFor(path),
        EditorState.readOnly.of(readOnly),
        wrapping.of(wrap ? EditorView.lineWrapping : []),
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
    let restoring = true;
    const snapshot = () => {
      const top=view.scrollDOM.scrollTop, block=view.lineBlockAtHeight(top);
      return {top,left:view.scrollDOM.scrollLeft,line:view.state.doc.lineAt(block.from).number,offset:top-block.top,anchor:view.state.selection.main.anchor,head:view.state.selection.main.head,wrap};
    };
    const report = () => { if(!restoring) onViewportChange?.(snapshot()); };
    view.scrollDOM.addEventListener("scroll",report,{passive:true});
    const line = Math.min(view.state.doc.lines,Math.max(1,Math.floor(position?.line||1)));
    if(position) {
      const bound=value=>Math.min(view.state.doc.length,Math.max(0,Math.floor(value||0)));
      view.dispatch({selection:{anchor:bound(position.anchor),head:bound(position.head)},effects:EditorView.scrollIntoView(view.state.doc.line(line).from,{y:"start",yMargin:0})});
    }
    view.requestMeasure({read:()=>view.lineBlockAt(view.state.doc.line(line).from).top,write:top=>{
      if(position){view.scrollDOM.scrollTop=top+(position.offset||0);view.scrollDOM.scrollLeft=position.left||0;}
      restoring=false;
    }});
    return {
      getPosition: snapshot,
      revealLine(value) { const line=Math.min(view.state.doc.lines,Math.max(1,Math.floor(Number(value)||1)));const at=view.state.doc.line(line).from;view.dispatch({selection:{anchor:at},effects:EditorView.scrollIntoView(at,{y:"start",yMargin:0})});view.focus(); },
      setWrap(value) { wrap=!!value;view.dispatch({effects:wrapping.reconfigure(wrap?EditorView.lineWrapping:[])});report(); },
      setValue(next) {
        const text = String(next || "");
        if (text === view.state.doc.toString()) return;
        applying = true;
        view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } });
        applying = false;
      },
      focus() { view.focus(); },
      destroy() { onViewportChange?.(snapshot());view.scrollDOM.removeEventListener("scroll",report);view.destroy(); }
    };
  }
});
