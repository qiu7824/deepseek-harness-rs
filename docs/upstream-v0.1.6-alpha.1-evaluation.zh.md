# 上游 v0.1.6-alpha.1 兼容性评估

评估日期：2026-09-15；Rust 源码基线：c6e6e27d60，应用版本：0.1.3-alpha.20。

依据：[上游正式发布记录](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.1.6-alpha.1)（2026-09-15 04:57:57 UTC），[版本比较范围](https://github.com/deepseek-ai/deepseek-harness/compare/dsh-v0.1.5-rc.2...dsh-v0.1.6-alpha.1)。发布说明证明变更范围，不替代源码对照与运行验收。

## 结论

Rust 仅完成部分同类能力，尚未全面对齐此上游版本。SSH、MCP 资源、PTC 单次超时、图片 offload 事件及生命周期协议有明确缺口。Cloud 并非这份上游发布说明中的新增工作区功能，不能用 SSH 支持证明 Cloud 已实现。

## 核心协议与执行能力

| 优先级 | 上游变化 | Rust 证据及待完成条件 |
| --- | --- | --- |
| P0 | 本地 Agent 使用 SSH 工作区 | workspace.create 仍拒绝 kind=ssh；需接通远端文件、Shell、PTC、取消和重连，不能把远端路径注册为本机目录 |
| P0 | 沙箱准备异步、可取消并计入超时 | 搜索已增加总预算；其他执行器仍需准备、启动、执行、清理四阶段测试 |
| P0 | Node PTC 独立进程与文件策略 | code-runtime-node 有独立进程；文件策略、环境变量、堆限制、输出限制仍需专项对照 |
| P0 | 异步串行 agent/created | core/agent-loop/src/index.rs 仍发送 agent/session-start；需迁移初始化顺序并验证首次请求等待 |
| P0 | 子代理通知排除推理块 | 工作流已修复失败返回 null，但这是不同问题；需单独验证通知进入 Messages 协议时仅含正文 |
| P1 | MCP 资源及 URI 模板 | mcp-client/src/lib.rs 目前丢弃 resource 内容；需枚举、模板、读取、大小限制和来源约束 |
| P1 | run_code 单次超时 120–600 秒 | core/tools/src/code_mode.rs 的 schema 只有 code、description；需新增超时范围及进程回收测试 |
| P1 | 图片 offload 持久化 | 当前附件与上传缓存不等同 offload 事件；需验证恢复、分叉及重复发送 |
| P1 | DeepSeek 默认 Messages 协议与 Files 复用 | 已有对应传输模块和缓存；默认端点迁移、自定义端点保持及鉴权失败不回退仍需对照 |
| P1 | V4.1 图片质量和 token 估算 | attachment-local 与 llm-deepseek 存在处理路径；默认尺寸、质量、动画及估算尚需逐值核对 |
| P1 | 请求图片缓存目录迁移 | 需验证 DSH_HOME/cache/attachments/request-images 重建、原图保留及旧缓存不误删 |
| P1 | MCP 协商、分页、无工具服务器 | 需分别覆盖工具分页终止、空目录、资源服务器和协议版本；工具调用成功不能替代全部验证 |
| P1 | Headless stdin、session-id、JSONL | 已有 CLI 和 SDK stdio；需按这三个参数契约补黑盒测试 |
| P1 | Linux 进程结束与清理 | subprocess-local 有进程组实现；需在 Linux 验证 TERM、KILL、残留进程和退出判定 |
| P2 | Team 工具与队友上限变化 | 已有 agent-team；需核对 spawn_teammate、禁用冲突工具、默认上限 16 |
| P2 | PTC/工作流改名 | Rust 仍用 code-runtime-node、workflow-node；需迁移映射和旧配置提示 |
| P2 | 默认禁用 Ralph、移除 E2B | Rust 仍保留模块；按 Rust 产品目标决定迁移，不能直接破坏用户配置 |
| P2 | 热更新取消事务回滚 | 需验证解析失败、部分激活、模块路由恢复和必需/可选插件行为 |
| P2 | 同步历史接口弃用 | Rust 仍有同步读取；按并发模型审查热点与生命周期，不能机械改名 |
| P2 | 持久 Bash 历史输出性能 | 需大输出累计与内存测试，不能以小命令执行成功验收 |

## 界面对照

| 上游范围 | Rust 现有位置与验收缺口 |
| --- | --- |
| 侧边栏多标签终端、主题对比度 | release/plugins/dsh-better-sidebar 及终端服务；需验证 Shell 切换、刷新恢复和颜色 |
| 归档会话查看恢复 | ui-workspace.js 已有界面；需验证恢复顺序、归属和正文 |
| 文件/Skill/产物侧边栏预览 | 已有预览面板；需验证树滚动、图片/PDF 缩放、加载状态 |
| 轨迹 JSON、PTC、推理和耗时 | ui-trajectory.js 有展示；需逐项核对展开、复制、默认状态和历史恢复 |
| 输入框加号分组与双语命令 | 已有可见输入、序号和附件回归；菜单重组尚未按上游对齐 |
| 自动/手动重连提示 | 已有重试与恢复；需验证点击重试和闪烁 |
| 关闭模式切换 UI | 已有预设和空白默认；隐藏切换开关尚需核对 |
| 会话排序、按轮分叉 | 已有历史和顺序测试；仍需对应上游触发样本 |
| PTC 工具说明中的双花括号 | 需提示词渲染回归，不能用一般工具调用证明 |
| 超一小时计时、短表格跳动 | 需专项 DOM 与滚动回归 |
| 推理/摘要吸顶、编辑卡片 | 需折叠、复制遮挡、增删行数及大块替换测试 |
| Mermaid 文档站 | 全屏、缩放、平移按文档站范围独立验收 |
| 实验性 Auto review | 需独立审查策略与取消链路，不等同常规审批 |

## 浏览器、桌面与连接

Rust NativeBrowserAdapter 的真实 Edge 点击、输入、滚动、上传和截图测试通过，不代表 Playwright MCP、Chrome DevTools MCP、Stagehand 均已移植。Windows 自有 UIA 控件测试也不能证明 Cua Driver MCP 或 macOS/Linux 桌面能力完成。

本地 Devin CLI 的 ACP 初始化返回 promptCapabilities.image=true，随附文档声明子代理和受组织设置约束的 Web Search。Rust Devin 模型连接与 Devin CLI Agent 是不同层；当前托管搜索调用 OpenAI Responses 接口，接口不兼容不代表 Devin 产品没有搜索。图片输入也不代表图片生成/编辑接口可用。

上游官方端点会话上报涉及数据传输，必须明确目的地、字段、关闭方式和用户配置，不能为版本对齐而静默启用。

## 发布与验收边界

- Git 克隆 RPC 已验证生成 .git 和 Cargo.toml；产品表单、取消、认证失败和路径边界仍需覆盖。
- Cloud 在本产品中定义为云端 Git 仓库来源：克隆到本机后使用本机执行器，不提供托管计算；SSH 指真正的远端目录与远端执行，仍需完整接线。
- v0.1.3-alpha.20-r3 四个平台均失败在版本与产品门禁，未证明平台编译成功。
- 发布工作流已有 contents: write 和发布任务，可使用 Actions 自身凭据；本机缺少 GitHub token 并非自动发布的必要阻塞条件。
- GitHub 托管 runner 测试与用户桌面真机交互是不同验收范围；当前无可用 macOS/Linux 桌面设备，不能声明完成真机交互测试。
- 全量验收须使用相同提交的本机安装版、远程执行器和发布包；代码存在、未执行测试或模糊结果均不计为完成。
