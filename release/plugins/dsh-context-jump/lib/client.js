window.__ModuleLoader__.load({
  id: "dsh-context-jump",
  factory: (require) => {
    const module = { exports: {} };
    const exports = module.exports;
    const React = require("react");
    const inject = ["slots"];
    const CSS = ".ctxjump{display:inline-flex;gap:3px}.ctxjump button{height:24px;min-width:24px;padding:0 6px;border:1px solid rgba(127,127,137,.35);border-radius:6px;background:transparent;color:inherit;cursor:pointer;font-size:12px}.ctxjump button:hover{background:rgba(127,127,137,.12)}";

    function installStyle() {
      if (document.querySelector('style[data-plugin-css="dsh-context-jump/client.css"]')) return;
      const style = document.createElement("style");
      style.dataset.plugin = "dsh-context-jump";
      style.dataset.pluginCss = "dsh-context-jump/client.css";
      style.textContent = CSS;
      document.head.appendChild(style);
    }

    function state() {
      const scroll = document.querySelector("[data-conversation-scroll]");
      const rows = scroll ? [...scroll.querySelectorAll("[data-chat-anchor-key]")] : [];
      if (!(scroll instanceof HTMLElement)) return { scroll: null, rows, index: -1 };
      const top = scroll.getBoundingClientRect().top;
      let index = rows.findIndex((row) => row.getBoundingClientRect().bottom > top + 8);
      if (index < 0 && rows.length) index = rows.length - 1;
      return { scroll, rows, index };
    }

    function reveal(row) {
      if (row instanceof HTMLElement) row.scrollIntoView({ block: "start", behavior: "smooth" });
    }

    function ContextJump() {
      const top = () => {
        const { scroll, rows } = state();
        if (!scroll) return;
        if (rows[0]) reveal(rows[0]);
        else scroll.scrollTo({ top: 0, behavior: "smooth" });
        const older = [...scroll.querySelectorAll("button")].find((button) => /加载更早|Load earlier/i.test(button.textContent || ""));
        if (older instanceof HTMLButtonElement && !older.disabled) older.click();
      };
      const previous = () => {
        const { rows, index } = state();
        reveal(rows[Math.max(0, index - 1)]);
      };
      const next = () => {
        const { rows, index } = state();
        reveal(rows[Math.min(rows.length - 1, index + 1)]);
      };
      const bottom = () => {
        const { scroll, rows } = state();
        if (!scroll) return;
        if (rows.length) reveal(rows[rows.length - 1]);
        scroll.scrollTo({ top: scroll.scrollHeight, behavior: "smooth" });
      };
      return React.createElement("div", { className: "ctxjump", role: "group", "aria-label": "会话快速跳转" },
        React.createElement("button", { type: "button", title: "跳到已加载内容顶部；必要时加载更早历史", onClick: top }, "⇈"),
        React.createElement("button", { type: "button", title: "上一个节点", onClick: previous }, "↑"),
        React.createElement("button", { type: "button", title: "下一个节点", onClick: next }, "↓"),
        React.createElement("button", { type: "button", title: "回到底部", onClick: bottom }, "⇊")
      );
    }

    function apply(ctx) {
      installStyle();
      ctx.slots.inject("conversation.input.right", () => ctx.slots.register({
        name: "conversation.input.right",
        id: "context-jump",
        order: 70,
        label: "Context jump"
      }, ContextJump));
    }

    exports.apply = apply;
    exports.inject = inject;
    return module.exports;
  }
});
