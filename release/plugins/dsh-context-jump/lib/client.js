window.__ModuleLoader__.load({
  id: "dsh-context-jump",
  factory: (require) => {
    const module = { exports: {} };
    const exports = module.exports;
    const React = require("react");
    const inject = ["slots"];
    const CSS = ".ctxjump-bar{display:flex;align-items:center;width:100%;height:10px;padding:0 16px;box-sizing:border-box}.ctxjump-track{position:relative;display:flex;align-items:center;gap:1px;width:100%;height:4px;border-radius:999px;background:rgba(127,127,137,.16);overflow:hidden}.ctxjump-segment{height:100%;min-width:2px;flex:1;border:0;padding:0;background:rgba(127,127,137,.3);cursor:pointer}.ctxjump-segment:hover{background:var(--dsw-alias-label-primary,#344054)}.ctxjump-segment[data-active=true]{background:var(--dsw-alias-interactive-primary,#175cd3)}";

    function installStyle() {
      if (document.querySelector('style[data-plugin-css="dsh-context-jump/client.css"]')) return;
      const style = document.createElement("style");
      style.dataset.plugin = "dsh-context-jump";
      style.dataset.pluginCss = "dsh-context-jump/client.css";
      style.textContent = CSS;
      document.head.appendChild(style);
    }

    function readRows() {
      const scroll = document.querySelector("[data-conversation-scroll]");
      if (!(scroll instanceof HTMLElement)) return { scroll: null, rows: [] };
      return { scroll, rows: [...scroll.querySelectorAll("[data-chat-anchor-key]")] };
    }

    function jump(row) {
      if (row instanceof HTMLElement) row.scrollIntoView({ block: "start", behavior: "smooth" });
    }

    function ContextJumpBar() {
      const [snapshot, setSnapshot] = React.useState({ rows: [], current: -1 });
      React.useEffect(() => {
        const refresh = () => {
          const { scroll, rows } = readRows();
          if (!scroll) return setSnapshot({ rows: [], current: -1 });
          const top = scroll.getBoundingClientRect().top + 10;
          const current = rows.findIndex((row) => row.getBoundingClientRect().bottom > top);
          setSnapshot({ rows, current: current < 0 ? rows.length - 1 : current });
        };
        refresh();
        const scroll = document.querySelector("[data-conversation-scroll]");
        scroll?.addEventListener("scroll", refresh, { passive: true });
        const observer = new MutationObserver(refresh);
        if (scroll) observer.observe(scroll, { childList: true, subtree: true });
        window.addEventListener("resize", refresh);
        return () => {
          scroll?.removeEventListener("scroll", refresh);
          observer.disconnect();
          window.removeEventListener("resize", refresh);
        };
      }, []);
      const rows = snapshot.rows;
      if (rows.length < 2) return null;
      return React.createElement("div", { className: "ctxjump-bar", role: "navigation", "aria-label": "会话位置" },
        React.createElement("div", { className: "ctxjump-track" }, rows.map((row, index) => React.createElement("button", {
          key: row.dataset.chatAnchorKey || index,
          type: "button",
          className: "ctxjump-segment",
          "data-active": index === snapshot.current ? "true" : "false",
          title: `跳转到第 ${index + 1} 个节点`,
          "aria-label": `跳转到第 ${index + 1} 个节点`,
          onClick: () => jump(row)
        })))
      );
    }

    function apply(ctx) {
      installStyle();
      ctx.slots.inject("conversation.session.header", () => ctx.slots.register({
        name: "conversation.session.header",
        id: "context-jump-bar",
        priority: -80,
        label: "Context jump bar"
      }, ContextJumpBar));
    }

    exports.apply = apply;
    exports.inject = inject;
    return module.exports;
  }
});
