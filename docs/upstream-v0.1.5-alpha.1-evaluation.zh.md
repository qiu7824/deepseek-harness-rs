# DeepSeek Harness Rust 对上游 v0.1.5-alpha.1 的评估

评估对象：`dsh-v0.1.5-alpha.1`，上游发布提交 `5dda764ed3aa172535a7967b06ff95d9cbfe536a`。

评估基线：Rust 工作树当前发布线 `0.1.3-alpha.10`（包含 alpha.2 适配、热启动修复和真实用户数据回归）。本评估依据[官方发布说明](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.1.5-alpha.1)及[dsh-v0.1.3-alpha.2 到 dsh-v0.1.5-alpha.1 的源码差异](https://github.com/deepseek-ai/deepseek-harness/compare/dsh-v0.1.3-alpha.2...dsh-v0.1.5-alpha.1)。

## 结论

这次上游版本不能作为一次 Web 资源更新直接合入 Rust。最重要的变化是**会话格式 V3 和系统提示词进入消息历史**，它会改变 Rust 当前的事件模型、请求重建、表面投影、持久化读取和降级策略；应作为独立的 P0 数据协议批次处理。alpha.10 已先完成不依赖 V3 的稳定性和数据完整性修复，保留 Rust 自有分页、超长对话和原生进程边界。

当前 Rust 已经具备或部分具备右侧工作台、工作区文件打开、目标暂停、图片附件、消息反馈、队列交互和长会话优化的相邻能力，但协议形状与上游不同。建议先完成 V3 设计和迁移 fixture，再选择性同步界面行为。

## 逐项评估

| 上游项目 | Rust 当前状态 | 结论 |
|---|---|---|
| 动态修改系统提示词且保持 KV Cache | Rust 可在每步重新组装 `SystemPrompt`，但 `request/header` 仍含 `system`，没有 `system/message` 表面节点和 `systemPromptUpdate` 能力协商 | **P0 必做**：先改事件与请求契约，再实现支持模型的 in-history 增量更新；不支持的模型必须新系列或重建提示词 |
| 会话格式升级 V3 | `crates/core/session/src/types.rs` 仍为 `SESSION_FORMAT_VERSION = 0`；官方 V3 将系统提示词、表面替换和旧 PTC/code 迁移纳入日志 | **P0 必做**：建立 V0/Rust → V3 的显式迁移或导入器；保留原文件，升级后禁止降级读取 |
| 实验性右侧 Sidebar、多标签、分栏、全屏 | 当前 Web 已有 `ui-sidebar`、`ui-layout` 和工作区工作台，但布局和插件是 Rust 自有实现，不能证明与上游右侧 Sidebar 协议一致 | **P1 选择性同步**：先验收状态、标签、分栏、窄屏和全屏；保留 Rust 工作区分页与权限边界 |
| 文件链接、产出文件在 Sidebar 打开 | Rust 已有 `open_path`、产出文件、预览和编辑器打开路径 | **P1 回归**：统一“侧栏预览 / 原生应用打开 / 浏览器回退”，验证工作区外绝对路径和权限 |
| 发送按钮与 Enter 遵循同一忙碌设置 | Rust 已有队列、Steer 和发送状态的部分实现 | **P1 必做**：确认按钮、快捷键、主会话和 continuable 子代理共用同一 `queue/steer` 决策 |
| 目标暂停后模型不可自行恢复 | Rust 支持 `pause/resume`，但 `tool-goal` 的 `resume` 路径需要加入“暂停目标必须由用户恢复”的权限检查 | **P1 必做**：模型 `update_goal(resume)` 拒绝；`/goal resume` 和 UI 用户动作允许 |
| 本地图片绝对路径显示 | Rust 有内容寻址附件和工作区文件打开；需确认 assistant Markdown 中的 POSIX 绝对路径是否经过文件读取授权并能显示工作区外截图 | **P1 必做**：新增受认证的 bounded file/media route，失败显示替代文字或原路径，禁止绕过 Host 权限 |
| 空消息与空白队列编辑拒绝 | Rust 某些 Agent / 子代理入口已检查空文本，需统一 Host prompt、队列编辑和图片/文件单独发送 | **P1 必做**：正文与附件至少一个非空；空白编辑必须拒绝；图片或文件单独发送仍允许 |
| 项目根目录发现错误直接报告 | Rust 已有代码图谱、技能文件和工作区根路径逻辑 | **P1 回归**：权限 / I/O 错误不得继续向父目录寻找并误用上级指令文件 |
| 折叠思考摘要去除 Markdown 粗体标记 | Rust Web 已有 reasoning 折叠与自有渲染 | **P2 小修**：只清理摘要显示，展开内容保持原文 |
| Codex / Claude Code 子代理版本更新 | Rust 子代理后端是独立 Rust/外部进程组合，不读取上游 npm 版本 | **不直接同步**；仅核对协议、停止、超时和默认模型行为 |
| macOS/Linux `fs-ext` 本地编译修复 | Rust 不依赖 Node `fs-ext` | **不适用** |
| `ctx.agent` 移除、Inbox 改为类型接口 | Rust API 本来要求显式 `Agent`，但内部仍有 `SubagentInbox` 等 Rust 运行时抽象 | **语义回归**：检查所有插件调用显式 Agent、子代理归属和定时调度排除；不机械照搬 TypeScript API 形状 |

## alpha.10 已完成的稳定性补齐

| 项目 | 当前结果 | 验收证据 |
|---|---|---|
| 填充投影缓存热启动 | 在 Tokio 任务中使用不受协作预算影响的有限初始化，并在多线程运行时让出 worker；300 条持久记录回归通过 | `dsh-session-projection-cache` 7 项测试；真实用户缓存 100 条热启动通过 |
| Rust Ultra 事件读取 | 登记 `ultra-child`、`ultra-admitted`、`ultra-settled`、`ultra-budget-exhausted`，未知未来事件仍拒绝 | JSONL 事件往返与真实 `session.list` 58 个会话通过 |
| 历史与配置保留 | 替换候选运行时后 58 个会话、工作区、`settings.json` 和凭据文件摘要保持一致 | `deployment-verified.json` |
| Rust 分页和滚动 | 双向历史窗口、上下文跳转和短页滚轮保留 Rust 实现，避免按官方单向列表替换 | `chat_scroll_dom_harness.cjs`、上下文条回归 |
| 原生运行库授权 | Python 安装目录只读 capability 缓存，workspace 写入权限仍按每次执行授予并回收 | Python 读写/越界探针与 sandbox 单测 |

这些修复不改变会话格式版本；V3 迁移仍按下节的 P0 计划单独实施。

## P0：会话 V3 与动态系统提示词

官方 V3 的关键变化不是版本号本身，而是以下数据契约：

1. `system/message` 成为可投影的表面事件；首个系统节点位于表面头部。
2. `request/header` 不再保存完整 `system` 文本，而保存路由、工具和提示词更新能力。
3. 支持 in-history 更新的模型在同一 series 中追加非空系统提示词变化；不支持的模型或新 series 需要规范化为首个系统节点。
4. 系统提示词为空时必须清除所有当前有效系统节点，不能恢复旧文本。
5. 表面替换字段从旧的 `start/end` 迁移到 `startSeq/endSeq`，旧 PTC 事件和 `code` 预设需要显式迁移。

Rust 当前 `EpochHeader.system`、`RequestContext`、`derive_messages` 和 `surface` 都以 V0 模型为中心。不能直接把常量改成 3；必须先定义 V3 事件、旧版本读取器、迁移输出、V3 写入校验和失败恢复策略。

推荐实现顺序：

1. 固化官方 V2/V3 真实日志 fixture，以及 Rust V0 现有 fixture；验证序列、seed/fork、压缩、截断恢复和未知事件策略。
2. 在 Rust 增加 `system/message`、提示词更新能力字段和表面替换新字段，同时暂不改变旧 V0 写入。
3. 增加 V0 → V3 迁移器：系统提示词从 header 变成首个系统事件；旧 `code` / PTC 事件转换为当前 Rust 事件；原日志先原子保留。
4. 在 Agent loop 中根据模型能力选择增量更新、新 series 或首节点替换；把最终有效提示词写入日志而不是只存在内存。
5. 更新 Host、Web 读取器、token 统计、消息导出、子代理历史和反馈扫描，最后才切换新写入版本。

验收要求：升级后旧 Rust 会话可读；升级后的 V3 会话不能被旧二进制误读；提示词修改后的下一次请求、恢复、分页、导出、fork 和子代理历史一致；取消、重启或迁移失败不会覆盖原日志。

## P1：交互与安全补齐

### Sidebar 和文件媒体

Rust 当前已有 `web/src/runtime-plugins/ui-sidebar.js`、`ui-layout.js`、`ui-workspace.js` 以及 Host 的 `open_path`。先用现有 Rust 协议完成：

- 文件链接和产出文件可以选择侧栏预览；
- 编辑器、文件管理器和浏览器打开仍走受认证 Host 路由；
- 工作区外绝对路径必须由 Host 的文件授权决定；
- 文件读取有大小、MIME、超时和取消上限；
- 远端浏览器明确操作 Host 所在机器。

不建议直接替换整个上游 Web bundle；当前 Rust 有自己的分页、历史窗口、上下文跳转和工作区权限。

### 目标暂停

Rust `GoalService` 已有 `Paused` 和 `resume`，差异集中在 authority：模型工具不得恢复用户暂停的目标，只有显式用户命令或 UI 动作可以恢复。需要为 `update_goal`、`/goal resume`、目标轮询驱动和重启恢复分别加测试。

### 空消息、图片和项目根

统一在 Host admission 层拒绝“无非空文本且无附件”的消息，不能只在 UI 过滤。队列编辑也必须经过同一校验。图片绝对路径需要转为受认证媒体引用，读取失败只显示替代文字或原始路径。项目根查找要区分“未找到根标记”和“权限/I/O 失败”，后者直接返回错误。

## 不应直接移植的内容

- Node/Electron 桌面打包、自动更新、签名和 `fs-ext` 修复；Rust 有独立的 ZSUI 启动器和发布链。
- Codex / Claude Code npm 版本号；只核对外部子代理协议和生命周期。
- TypeScript 的 `ctx.agent`、Inbox 类型接口和包级导出形状；Rust 只吸收显式 Agent、归属和调度语义。
- 官方完整前端 bundle 的所有 CSS、注释和内部目录调整；只移植已验证的用户行为。

## 发布判断

当前不建议把 Rust 版本号直接标为“对齐 v0.1.5-alpha.1”。在完成 V3 迁移前，最安全的产品声明是：Rust 保持自己的会话格式和协议边界，选择性提供 Sidebar、工作区文件打开、目标暂停保护、消息校验和图片显示。

进入下一候选版前必须通过：V0→V3 迁移 fixture、V3 新写入 / 恢复 / fork、动态提示词能力矩阵、Sidebar 文件权限、目标暂停 authority、空消息 admission、项目根错误传播，以及 Windows/Linux/macOS 的完整包启动回归。
