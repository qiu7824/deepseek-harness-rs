window.__ModuleLoader__.load({
  id: "dsh-skin-center",
  factory: (require) => {
    const module = { exports: {} };
    const exports = module.exports;
    const React = require("react");
    const jsx = require("react/jsx-runtime");
    const inject = ["slots", "theme", "connection"];

    const SKINS = Object.freeze([
      { id: "light", name: "默认浅色", nameEn: "Default Light", source: "Harness", tone: "light", description: "DeepSeek Harness 内置浅色界面。", colors: ["#ffffff", "#f4f5f7", "#2563eb"] },
      { id: "dark", name: "默认深色", nameEn: "Default Dark", source: "Harness", tone: "dark", description: "DeepSeek Harness 内置深色界面。", colors: ["#17181a", "#24262a", "#60a5fa"] },
      { id: "blue-fantasy", name: "蓝色幻想", nameEn: "Blue Fantasy", source: "dsh-market #2", tone: "adaptive", description: "鲸鱼插画背景、靛蓝调色板与半透明面板。", colors: ["#10152d", "#4a5fa8", "#c6cdf4"] },
      { id: "harbor", name: "夕港", nameEn: "Harbor", source: "dsh-market #3", tone: "adaptive", description: "暮光蓝港、日落橙辉与半透明夜色面板。", colors: ["#141a2e", "#ff9d5c", "#d9d9e8"] },
      { id: "xp", name: "Windows XP (Luna)", nameEn: "Windows XP Luna", source: "dsh-market #4", tone: "adaptive", description: "Luna 蓝窗口条、绿色开始按钮与 Bliss 蓝天桌面。", colors: ["#ece9d8", "#316ac5", "#3d9f43"] },
      { id: "minecraft", name: "Minecraft 方块世界", nameEn: "Minecraft Voxel", source: "dsh-market #6", tone: "adaptive", description: "动态天空盒、方块按钮与告示牌输入框。", colors: ["#232a1d", "#7cbd4b", "#f6e55e"] },
      { id: "trading", name: "交易终端", nameEn: "Trading Terminal", source: "dsh-market #7", tone: "adaptive", description: "行情跑马灯、红涨绿跌与密集交易终端。", colors: ["#060b0d", "#f23645", "#2fde8e"] },
      { id: "miku", name: "初音未来 · 电子歌姬", nameEn: "Hatsune Miku", source: "dsh-market #8", tone: "adaptive", description: "蓝紫双马尾、01 编号与音符波形。", colors: ["#10152a", "#2e9bff", "#ec4bb0"] },
      { id: "deepseek-official", name: "DeepSeek Harness 官方", nameEn: "DeepSeek Harness Official", source: "www.deepseek.com/harness/", tone: "adaptive", description: "完整参照官方 Harness 的品牌蓝、玻璃表面、字体层级与深浅主题。", colors: ["#f9f8f8", "#4d6bfe", "#101113"] }
    ]);

    const CSS = `
.dsh-skin-center{box-sizing:border-box;width:100%;max-width:760px;padding:4px 2px 32px;color:var(--dsw-alias-label-primary)}
.dsh-skin-center__header{display:flex;flex-direction:column;gap:4px;margin-bottom:16px}
.dsh-skin-center__header h2{margin:0;font-size:18px;font-weight:600;line-height:26px}
.dsh-skin-center__header p{max-width:560px;margin:0;color:var(--dsw-alias-label-tertiary);font-size:13px;line-height:20px}
.dsh-skin-center__group{display:flex;flex-direction:column;gap:14px;border-top:1px solid var(--dsw-alias-border-l2);padding-top:16px}
.dsh-skin-center__row{display:grid;grid-template-columns:minmax(0,1fr) minmax(220px,300px);align-items:center;gap:24px}
.dsh-skin-center__copy{min-width:0;display:flex;flex-direction:column;gap:3px}
.dsh-skin-center__label{font-size:14px;font-weight:500;line-height:22px}
.dsh-skin-center__help{max-width:340px;color:var(--dsw-alias-label-tertiary);font-size:12px;line-height:18px}
.dsh-skin-center__select{box-sizing:border-box;width:100%;height:38px;border:1px solid var(--dsw-alias-border-l2);border-radius:8px;background:var(--dsw-alias-bg-layer-1);color:var(--dsw-alias-label-primary);font:inherit;padding:0 32px 0 11px;outline:none}
.dsh-skin-center__select:hover{background:var(--dsw-alias-interactive-bg-hover)}
.dsh-skin-center__select:focus-visible{border-color:var(--dsw-alias-brand-primary);box-shadow:0 0 0 2px color-mix(in srgb,var(--dsw-alias-brand-primary) 20%,transparent)}
.dsh-skin-center__select:disabled{cursor:not-allowed;opacity:.55}
.dsh-skin-center__card{display:grid;grid-template-columns:auto minmax(0,1fr);gap:12px;padding:16px;border:1px solid var(--dsw-alias-border-l2);border-radius:12px;background:var(--dsw-alias-bg-layer-1)}
.dsh-skin-center__swatches{display:flex;align-items:flex-start;gap:4px;padding-top:3px}
.dsh-skin-center__swatch{display:block;width:14px;height:14px;border:1px solid var(--dsw-alias-border-l2);border-radius:50%}
.dsh-skin-center__meta{min-width:0;display:flex;flex-direction:column;gap:3px}
.dsh-skin-center__name{font-size:14px;font-weight:500;line-height:22px}
.dsh-skin-center__source{color:var(--dsw-alias-label-tertiary);font-size:12px;line-height:18px}
.dsh-skin-center__description{color:var(--dsw-alias-label-secondary);font-size:12px;line-height:19px}
.dsh-skin-center__notice{padding:12px 14px;border:1px solid var(--dsw-alias-border-l2);border-radius:10px;background:var(--dsw-alias-bg-layer-1);color:var(--dsw-alias-label-secondary);font-size:12px;line-height:19px}
.dsh-skin-center__status{min-height:20px;color:var(--dsw-alias-state-success-primary);font-size:12px;font-weight:500;line-height:20px}
.dsh-skin-center__status:not(:empty)::before{content:"✓";margin-right:6px}
.dsh-skin-center__status[data-error=true]{color:var(--dsw-alias-state-error-primary)}
.dsh-skin-center__status[data-error=true]::before{content:"!"}
@media(max-width:720px){.dsh-skin-center__row{grid-template-columns:1fr;gap:10px}.dsh-skin-center__select{height:44px}.dsh-skin-center__card{grid-template-columns:1fr}.dsh-skin-center__swatches{padding-top:0}}
`;

    function installCss() {
      if (document.querySelector("style[data-plugin-css='dsh-skin-center']") !== null) return;
      const tag = document.createElement("style");
      tag.dataset.plugin = "dsh-skin-center";
      tag.dataset.pluginCss = "dsh-skin-center";
      tag.textContent = CSS;
      document.head.appendChild(tag);
    }

    async function skinAssetsInstalled() {
      try {
        const response = await fetch("/skins/deepseek-official/skin.json", { method: "HEAD", cache: "no-store" });
        return response.ok && (response.headers.get("content-type") ?? "").toLowerCase().includes("application/json");
      } catch {
        return false;
      }
    }

    function SkinCenter({ theme }) {
      const [snapshot, setSnapshot] = React.useState(() => theme.getTheme());
      const [status, setStatus] = React.useState({ text: "", error: false });
      const [busy, setBusy] = React.useState(false);
      const [assetsReady, setAssetsReady] = React.useState(undefined);
      const pendingRef = React.useRef(false);
      React.useEffect(() => {
        const refresh = () => setSnapshot(theme.getTheme());
        const offThemeChange = theme.ctx.on("theme/change", refresh);
        return offThemeChange;
      }, [theme]);
      React.useEffect(() => {
        let live = true;
        skinAssetsInstalled().then((ready) => { if (live) setAssetsReady(ready); });
        return () => { live = false; };
      }, []);
      const selected = SKINS.find((skin) => skin.id === snapshot.preference) ?? SKINS[0];
      const choose = async (id) => {
        if (pendingRef.current || id === snapshot.preference) return;
        pendingRef.current = true;
        setBusy(true);
        setStatus({ text: "正在应用…", error: false });
        try {
          await Promise.resolve(theme.applyTheme(id));
          const applied = theme.getTheme().preference === id;
          if (!applied) throw new Error("皮肤未完成应用，请检查皮肤资源后重试。");
          setStatus({ text: "已应用并保存", error: false });
        } catch (error) {
          setSnapshot(theme.getTheme());
          setStatus({ text: error instanceof Error ? error.message : String(error), error: true });
        } finally {
          pendingRef.current = false;
          setBusy(false);
        }
      };
      return jsx.jsxs("section", { className: "dsh-skin-center", "data-dsh-skin-center": "", "data-busy": busy || undefined, children: [
        jsx.jsxs("header", { className: "dsh-skin-center__header", children: [
          jsx.jsx("h2", { children: "皮肤" }),
          jsx.jsx("p", { children: "选择界面皮肤。切换会先完成资源加载，再写入当前 Harness 配置。" })
        ] }),
        jsx.jsxs("div", { className: "dsh-skin-center__group", children: [
          jsx.jsxs("div", { className: "dsh-skin-center__row", children: [
            jsx.jsxs("div", { className: "dsh-skin-center__copy", children: [
              jsx.jsx("span", { className: "dsh-skin-center__label", children: "界面皮肤" }),
              jsx.jsx("span", { className: "dsh-skin-center__help", children: "两套默认皮肤、保留的市场预设和 DeepSeek Harness 官方皮肤。" })
            ] }),
            jsx.jsx("select", {
              className: "dsh-skin-center__select",
              value: selected.id,
              disabled: busy,
              "aria-label": "界面皮肤",
              onChange: (event) => { void choose(event.target.value); },
              children: SKINS.map((skin) => jsx.jsx("option", { value: skin.id, "data-dsh-skin-option": skin.id, disabled: assetsReady === false && !["light", "dark"].includes(skin.id), children: skin.name === skin.nameEn ? skin.name : `${skin.name} · ${skin.nameEn}` }, skin.id))
            })
          ] }),
          jsx.jsxs("div", { className: "dsh-skin-center__card", "data-dsh-skin-card": "", children: [
            jsx.jsx("span", { className: "dsh-skin-center__swatches", "aria-hidden": true, children: selected.colors.map((color) => jsx.jsx("i", { className: "dsh-skin-center__swatch", style: { background: color } }, color)) }),
            jsx.jsxs("div", { className: "dsh-skin-center__meta", children: [
              jsx.jsx("span", { className: "dsh-skin-center__name", children: selected.name }),
              jsx.jsxs("span", { className: "dsh-skin-center__source", children: [selected.nameEn, " · ", selected.source] }),
              jsx.jsx("span", { className: "dsh-skin-center__description", children: selected.description })
            ] })
          ] }),
          assetsReady === false && jsx.jsx("div", { className: "dsh-skin-center__notice", role: "note", children: "Skin assets are not installed. 当前只能使用默认浅色和默认深色；请先从启动器安装皮肤资源，然后刷新 Harness。" }),
          jsx.jsx("div", { className: "dsh-skin-center__status", role: "status", "aria-live": "polite", "data-error": status.error || undefined, children: status.text })
        ] })
      ] });
    }

    function apply(ctx) {
      installCss();
      ctx.slots.inject("settings.section", () => ctx.slots.register({
        name: "settings.section",
        id: "skins",
        order: 10,
        label: "皮肤"
      }, () => jsx.jsx(SkinCenter, { theme: ctx.theme })));
    }

    exports.apply = apply;
    exports.inject = inject;
    return module.exports;
  }
});
