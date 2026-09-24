# DeepSeek Harness Rust：v0.1.7-alpha.1 / alpha.2 评估

评估日期：2026-09-23。实施基线为 0.1.3-alpha.34，修复分支基线为 `8ac58ecd1488ea6018b6d058a39b3f0b659e69ab`，主工作区为 `ddd2b255e9b00a71f442c537d1cb3ed59789d052`。上游目标为 v0.1.7-alpha.1（`c36a83ff6bb95e3f82cf79f9be7c724270a8aa61`）和 v0.1.7-alpha.2（`00102833dfaee1da9f48a3a8eae9d34005a75218`）。`ddefc45f` 实际属于 v0.1.6-alpha.2，不作为本次迁移目标。

依据：[alpha.1 发布说明](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.1.7-alpha.1)、[alpha.2 发布说明](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.1.7-alpha.2)及两标签间的实际源码差异。发布说明包含累计变更，不能把全部条目重新算作 alpha.1 到 alpha.2 的新开发量。

## 结论

这两版的主要风险已经从普通 WebUI 差异上升为 Session V4、DeepSeek Messages-only、插件运行时管理和用户终端权限边界。已安装版本的 Session 仍为 V3、官方 DeepSeek 默认仍为 openai-completions，因此不能声明版本兼容。修复分支的 Messages 迁移已进入专项验证；Session V4 的转换、原生校验和明文文件编码已通过专项及五份真实会话副本往返检查；版本选择、会话写入锁、Zstandard 流式迁移和独立版本发布已通过持久化包 50 项检查，五份真实会话副本完成压缩迁移及 38,533 条事件逐值核对；修复源码已默认写入 V4，正常后端、残尾恢复、分叉继承、父子目录和 CLI 导入已接通；新安装版联合验收仍未完成。[运行时接入证据](plans/native-runtime-integration-2026-09-24.json)。

alpha.1 的新增范围包括 Session V4、Messages 默认协议、图片 offload、插件运行时、终端侧栏、归档会话、MCP 资源、Headless stdin/JSON、SSH 工作区、Browser Use、Computer Use、Auto Review、回合文件审阅和 Office 预览。alpha.2 进一步补强插件管理、多会话侧栏、文件差异、Office/URL/Subagent/计划入口及启动和恢复错误处理。

## P0 发布阻断项

| 项目 | Rust 当前状态 | 处理决定 |
|---|---|---|
| Session V4 | 修复源码默认写入 V4；相邻迁移保留原文件，接通压缩残尾、引用解码、冷恢复校验、父子目录及 CLI 导入；正常后端 6 项端到端专项、导入 6 项专项通过，安装版仍为 V3 | 继续完成新安装版、原有会话和客户端联合验收；与上游相同，无法保留原次序的早期历史拒绝迁移并保留原件及导出，不伪造步骤或重排记录 |
| DeepSeek Messages 默认 | 修复分支已实现原生 Messages 请求；旧 Files API 的地址、认证和元数据已同步迁移，专项测试通过 | 正式 Host、真实账号、图片 Files/inline 完整链路及原会话继续执行仍须验收；自定义显式路由保留 |
| 用户终端权限 | 源码已分离用户终端登记和启动入口；无运行中 Agent、模型默认只读时的真实 PTY 输入和文件写入通过，模型策略保持不变 | 新安装版、页面切换、任务停止及多平台联合验收；见 [终端与插件证据](plans/user-terminal-plugin-recovery-2026-09-24.json) |
| 插件失败与配置恢复 | 已接通包与配置事务、进程中断恢复、有效快照、外部修改保护及安装/移除/恢复的取消和回执；可选客户端失败不再阻断核心启动 | 启停取消和操作日志专项已通过；原工作区消失案例、实际 GitHub 下载及安装版全流程仍须验收 |

## P1 主要能力

| 功能 | 当前判断 | 建议 |
|---|---|---|
| MCP Resources | 已有 resources/list、templates/list、read 模型工具、作用域路由与 2 MiB 结果限制；resource/resource_link 作为 JSON 保留 | 核验真实服务器、分页、权限隔离、取消通知、恢复及正式包；不重复创建已有资源入口 |
| Inbox 冷恢复与会话占用 | Inbox 会重放 durable splice，缺少跨进程 writer lock 的完整证据 | 双 Host 同占用时拒绝第二写者；重启恢复未领取消息，已领取/取消消息不重复 |
| Messages 历史工具输入 | 历史非法 JSON/非对象已仅在 Messages 序列化边界降为空对象；原消息不改写，新工具输入保持严格校验 | 原始失败会话及正式包复验；截断工具参数通过 MaxTokens 路径保留文本并过滤不可执行调用 |
| 回合文件改动审阅 | 当前工作树已有 dsh-host turn_changes，使用有界 Git/文件快照；UI 与正式包联动仍需验证 | 完成 workspace/changes UI、逐文件前后对比和不完整状态展示；不改变用户 Git index、refs 和工作树 |
| 子代理驻留配额 | 默认深度 1、8 个驻留 continuable 子代理及亲缘共享额度已实现并通过真实运行时专项 | 完成默认模型、回合/超时限制与安装验收 |
| Headless | 普通 Headless 已接通多词任务、stdin、session-id 和有界 NDJSON；15 项隔离程序流程通过 | 继续安装版与各平台中断、子会话拒绝及完整验收 |

## P2 / 实验能力

- Office 预览：Rust 分支的 DOCX 默认使用 docx-preview 在浏览器显示，打印版式显式调用 WPS→PDF；XLSX/PPTX 及视觉验收继续使用 WPS。宽窄窗口、分页、取消与 DOM 回收已专项验证，正式安装包、整体持续运行与其他 Office 格式仍须验收。
- 插件中心：复用现有纯 Web 安装、库存和启停，不开放未经审核的 Node Host 执行。
- 子代理和计划侧栏：主会话与子会话使用独立 Session 引用；计划预览读取确切提交，不重复执行计划。
- Browser Use：当前 Rust 有受控浏览器/CDP 和 Computer Use，但不等同 Playwright MCP、Chrome DevTools MCP、Stagehand。
- Auto Review：默认关闭的会话级插件已接入逐调用审查、来源过滤、PTC 与子任务继承、取消/卸载及界面确认；专项和完整 Host 启停回归通过，安装版与实际提供方验收仍待完成。
- SSH 远端工作区：独立实现远端文件、执行器、主机密钥、取消和权限隔离。

## WebUI 建议

建议增加或接通：

1. 回合文件改动卡与右侧逐文件审阅；
2. 侧栏子代理对话；
3. 计划预览标签；
4. WPS Office 内嵌预览；
5. 统一插件中心。

工具发现设置、技能候选/版本和任务验收模块当前已进入工作树的 source、dist 和 manifest；下一步应核对正式安装包中的 Host 接口、权限、保存、恢复与失败状态。归档列表、上下文用量、终端、网页预览、模型图片输入开关和现有工作台不应重复新建入口。

## 实施顺序

1. V4 迁移、Messages 默认、权限重复申请、历史工具输入兼容；
2. MCP Resources、Inbox 冷恢复、跨进程会话占用；
3. 回合文件差异、用户终端生命周期和子代理配额；
4. 插件中心、子代理/计划侧栏、默认目录和启动诊断；
5. WPS Office、Headless 完整参数、SSH、Browser Use 和 Auto Review 实验批次。

专项验证：Messages 协议库 98 项、真实本地 HTTP 4 项、流式异常/取消 12 项、Host 设置 4 项、Host 任务契约 8 项通过。HTTP 使用本地协议服务器，不能替代真实账号验证。完整验收仍覆盖 Session V4、原始会话恢复、插件操作、MCP 资源服务器、跨进程写锁、WPS 正式安装包、UI 和长会话与连续运行；上述条件及 Host 主进程 70 MB 门槛全部满足后才能最终提交、推送和发布。
