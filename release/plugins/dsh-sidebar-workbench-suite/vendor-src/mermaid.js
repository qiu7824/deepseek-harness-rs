import mermaid from "mermaid";

mermaid.initialize({
  startOnLoad: false,
  securityLevel: "strict",
  suppressErrorRendering: true,
  deterministicIds: true,
  deterministicIDSeed: "dsh-sidebar"
});

globalThis.__DSH_SIDEBAR_MERMAID__ = Object.freeze({
  async render(id, source, dark) {
    const config = {
      startOnLoad: false,
      securityLevel: "strict",
      suppressErrorRendering: true,
      theme: dark ? "dark" : "default"
    };
    mermaid.initialize(config);
    const result = await mermaid.render(id, String(source || ""));
    return result.svg;
  }
});
