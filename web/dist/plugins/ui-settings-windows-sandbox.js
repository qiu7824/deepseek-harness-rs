window.__ModuleLoader__.load({
  id:"@deepseek-ai/dsh-client-ui-settings-windows-sandbox",
  factory:require=>{
    const React=require("react"),h=React.createElement;
    function WindowsSandboxSection(){
      const [state,setState]=React.useState(null),[implementation,setImplementation]=React.useState("elevated"),[network,setNetwork]=React.useState("enabled"),[workspace,setWorkspace]=React.useState(""),[busy,setBusy]=React.useState(false),[error,setError]=React.useState(""),[notice,setNotice]=React.useState("");
      const mounted=React.useRef(true),controller=React.useRef(null);
      const request=async body=>{
        const response=await fetch("/__dsh-windows-sandbox",{method:body?"POST":"GET",credentials:"same-origin",headers:body?{"Content-Type":"application/json"}:undefined,body:body?JSON.stringify(body):undefined,signal:controller.current?.signal});
        const text=await response.text();let value;try{value=JSON.parse(text)}catch{throw Error("沙箱服务响应不完整，请刷新状态；初始化不会因页面断开自动重放。")}
        if(!response.ok)throw Error(value.error||"Windows 沙箱服务不可用");return value;
      };
      const load=async()=>{const value=await request();if(mounted.current){setState(value);setImplementation(value.implementation||"elevated");setNetwork(value.network||"enabled")}};
      React.useEffect(()=>{mounted.current=true;controller.current=new AbortController();load().catch(e=>{if(mounted.current)setError(e.message)});return()=>{mounted.current=false;controller.current.abort()}},[]);
      const act=async action=>{if(busy||!state)return;setBusy(true);setError("");setNotice("");try{const value=await request({action,implementation,network,workspace:workspace.trim(),expectedRevision:state.revision});if(mounted.current){setState(value);setNotice(action==="setup"?"项目初始化完成，后续命令使用所选沙箱。":"设置已保存，后续命令使用所选沙箱。")}}catch(e){if(mounted.current)setError(e.message)}finally{if(mounted.current)setBusy(false)}};
      if(state?.available===false)return null;
      const fieldStyle={display:"flex",flexDirection:"column",gap:6},controlStyle={minHeight:38,padding:"6px 10px",font:"inherit",maxWidth:"100%",boxSizing:"border-box"};
      return h("section",{style:{display:"flex",flexDirection:"column",gap:14,maxWidth:760,paddingBottom:24}},
        h("h2",null,"Windows 沙箱"),
        h("p",null,"默认使用独立低权限账户；受限令牌模式可用于无法完成管理员初始化的环境。"),
        h("label",{style:fieldStyle},"执行模式",h("select",{style:controlStyle,value:implementation,disabled:busy,onChange:e=>setImplementation(e.target.value)},h("option",{value:"elevated"},"独立低权限账户（默认）"),h("option",{value:"unelevated"},"当前用户的受限令牌（备用）"))),
        h("p",null,implementation==="elevated"?"每个可写项目单独初始化，账户与命令进程树保持隔离；初始化可能请求 Windows 管理员授权。":"不创建账户；项目权限初始化不需要管理员授权，读取权限沿用当前用户，文件写入受限，离线网络仅提供环境级限制。"),
        h("label",{style:fieldStyle},"命令网络访问",h("select",{style:controlStyle,value:network,disabled:busy,onChange:e=>setNetwork(e.target.value)},h("option",{value:"enabled"},"允许网络"),h("option",{value:"restricted"},"限制网络"))),
        h("label",{style:fieldStyle},"项目目录",h("input",{style:controlStyle,value:workspace,placeholder:"例如 E:\\projects\\app",disabled:busy,onChange:e=>setWorkspace(e.target.value)})),
        h("div",{style:{display:"flex",gap:8,flexWrap:"wrap"}},h("button",{type:"button",style:controlStyle,disabled:busy||!state,onClick:()=>act("configure")},"保存执行模式"),h("button",{type:"button",style:controlStyle,disabled:busy||!state||!workspace.trim(),onClick:()=>act("setup")},busy?"正在处理…":"初始化项目"),h("button",{type:"button",style:controlStyle,disabled:busy,onClick:()=>load().catch(e=>setError(e.message))},"刷新状态")),
        h("p",null,"变更执行模式后，已有任务需重新核对环境与验收证据；运行中的命令保留启动时的权限。"),
        error&&h("p",{role:"alert"},error),notice&&h("p",{role:"status"},notice),!state&&!error&&h("p",{role:"status"},"正在读取沙箱配置…")
      );
    }
    // Sandbox selection is a Host policy, shared with Codex-style execution;
    // it is deliberately not exposed as a second per-application settings page.
    // Keep the section component export for compatibility with older bundles.
    return {inject:["slots"],apply(){},WindowsSandboxSection};
  }
});
