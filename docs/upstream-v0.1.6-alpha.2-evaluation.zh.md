# DeepSeek Harness Rust：v0.1.6-alpha.2 适配评估

评估日期：2026-09-19。Rust 工作树版本：`0.1.3-alpha.22`；HEAD：`e7bafb32f0fecccb1fd6fb754c0b1e097a5b31fc`。终端、启动器及 ZSUI 等文件存在未提交修改，运行和发布状态需与具体候选构建分别核对。

上游目标：`dsh-v0.1.6-alpha.2`，提交 `ddefc45fbc7f8e46dd73185e68295696d1297887`。比较起点为 `dsh-v0.1.6-alpha.1`，不是更早的 rc.2。

## 1. 判断与建议

**该版本是插件管理与多会话侧栏的架构升级，同时包含值得优先处理的运行可靠性修复。建议分批吸收，保留 Rust 已有工作台、文件权限和执行体系。**

[官方发布说明](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.1.6-alpha.2)共有 29 项用户及开发者变更。[alpha.1 → alpha.2 比较](https://github.com/deepseek-ai/deepseek-harness/compare/dsh-v0.1.6-alpha.1...dsh-v0.1.6-alpha.2)包含 2,622 个变更路径，其中还有桌面安装更新、构建与包元数据，不能以路径数估算 Rust 功能工作量。

最值得优先开展的工作是：

1. **可靠性修复**：Messages 历史工具输入、相同权限模式重复请求、视觉能力元数据及队列冷恢复验证。
2. **Host 能力**：回合级文件差异、用户终端生命周期与权限分离、子代理驻留限额。
3. **多实例侧栏**：主对话与侧栏子对话独立绑定，计划预览、会话布局恢复及首次发送前的操作入口。
4. **独立插件与 Office 批次**：在 Rust 支持边界内扩展插件管理；Office 使用 WPS 转 PDF 接入已有预览器。

上游和 Rust 的 Session 格式编号目前均为 **3**。该版本增加 `workspace/changes` 等事件与消费者接口，不要求机械升级为 V4；同编号也不证明两个实现的全部日志可以互读。

## 2. 发布项目逐项映射

P1 为可靠性或主要能力补齐；P2 为可独立交付的体验优化；“已有”只表示已找到对应实现，未用本次静态核对代替真实运行验收。

| 编号 | 上游变化 | Rust 当前证据与处理 |
|---|---|---|
| 01 | 插件管理页、安装、配置、实时启停 | 已有插件库存、设置、启停和纯 Web 插件 CLI 安装；缺少上游完整 profile 包管理器。**P1 分范围扩展**，先复用库存和启停，不直接承诺 npm Host 插件可运行 |
| 02 | 回合结束文件改动卡、逐文件比较 | 已有 artifacts 索引、Git 差异和 `present`，未找到对应回合起止快照与 `workspace/changes` 服务。**P1 新增** |
| 03 | Word / Excel / PowerPoint 侧栏预览 | 当前主要提供外部 WPS 打开及下载；没有完整 Office 内嵌渲染链。**P1 新增 WPS 转换服务**，不引入上游 LibreOffice 后端 |
| 04 | 侧栏 URL 浏览器 | 已有多标签网页预览、导航及沙箱 iframe。**已有，回归**；受控浏览器另有功能，不能混为同一实现 |
| 05 | 侧栏 Subagent 会话 | 已有子代理目录、续聊与主界面导航；未发现上游 embedded conversation 独立引用链。**P1**，依赖客户端多实例隔离 |
| 06 | 侧栏提交计划预览 | 已有计划审批卡；缺少独立计划资源和侧栏预览。**P1**，绑定确切提交记录与版本 |
| 07 | 工作区按目录层级分组 | 已有工作区分组、排序和搜索，但不是目录前缀树。**P2**，处理盘符根、UNC、符号链接及手动顺序 |
| 08 | 布局持久化、刷新保持终端 | 布局、分栏、浮窗及固定终端已有基础。**P1 验证终端进程连续性**，不能以恢复标签外观代替恢复同一 PTY |
| 09 | 打开已有文件；首次消息前预览与终端 | 文件预览已有；当前终端入口要求可用的 Agent owner。**P1 核对冷会话 / 空白会话绑定**，不得为预览启动模型请求 |
| 10 | 输入菜单统一键盘操作 | 输入框已有多类菜单。**P2**，逐项验证方向键、Enter、Tab、Escape 和中文输入法，不整体替换编辑器 |
| 11 | Trajectory 文本、图片、文件与缩略图 | 当前存在轨迹和附件投影。**P2**，逐类核对混排、权限、缩略图生命周期及恢复 |
| 12 | 思考内容紧凑 Markdown | 已有推理行和 Markdown 基础。**P2**，保留全文、复制及折叠状态，限制展开 DOM 成本 |
| 13 | 上下文用量移至输入框底部 | 已有上下文与 token 统计。**P2 布局适配**，不改变统计口径或把未披露缓存视为零 |
| 14 | CLI / Web 启动提速 | 上游包含 Node 依赖窄入口及运行时解析。**P2 按 Rust 启动剖析优化**，不照搬 Node resolver |
| 15 | 启动错误分类、等待服务及完整日志 | Rust 有启动错误输出和日志，但需补对应结构化分类与依赖链。**P1 诊断增强** |
| 16 | 视觉模型发现与手动输入类型 | Rust 已读部分 input metadata，设置页有“支持图片输入”。**已有部分能力，P1 验证发现→保存→请求全链**，补字段映射遗漏 |
| 17 | 重启恢复 Inbox | Rust `Inbox::new` 已重放本会话 splice，发布后唤醒 pending；结构不同于上游投影注册。**P1 冷重启回归**，不预设同一缺陷仍存在 |
| 18 | Messages URL 与历史工具输入 | Rust Messages URL 已避免通常的重复 `/v1`；历史 arguments 非法 JSON / 非对象仍导致请求失败。**P1 明确补齐历史发送兼容**，Files 地址另行核对 |
| 19 | 重复申请当前权限模式免审批 | Rust 公共 escalation 仍要求严格扩大，重复当前模式会报错。**P1 明确修复**，新扩大权限继续走审批 |
| 20 | Windows PTC / Shell 不闪控制台 | 原生命令和普通进程已有 `CREATE_NO_WINDOW`。**已有部分能力，回归完整启动链**，特别是 helper / 取消路径 |
| 21 | 会话被其他实例占用时引导重试 | 本次未验证跨进程写占用完整链路。**P1**，用两个独立 Host 验证拒绝、诊断和释放后恢复，内存锁不计为跨进程锁证据 |
| 22 | 宽度拖拽遮挡与嵌套提示重叠 | Rust 有自有调宽与提示逻辑。**P2 真实浏览器验证**，覆盖窄屏、浮窗和皮肤 |
| 23 | Web 用户终端使用系统用户权限 | 当前 Web 终端复用 Agent owner 和 ShellTerminalBackend 的沙箱策略，且相关文件有未提交修改。**P1 单独设计权限边界**，不能全局取消工具沙箱 |
| 24 | 默认列表移除旧 Flash / Vision Exp | Rust 内置目录仍保留旧条目。**P1 缺省目录调整**，保留已有显式配置与历史模型身份 |
| 25 | 插件运行时解析与卸载 | 纯 Web 插件已有加载、禁用与依赖处理；Rust 编译进程不能直接加载任意 Node Host 模块。**架构适配项**，保持能力声明边界 |
| 26 | `dsh <profile>` 简写 | 当前 parser 仅将 `web` 作为专用别名，其余主要使用 `--profile`。**P2 小功能**，保留现有 history / plugin 子命令及参数错误规则 |
| 27 | continuable 默认 8 个驻留子代理、深度 1 | Rust 有子代理深度、maxParallel 和 Team 限制，但默认与语义不同。**P1 根链共享驻留配额**，不只改 UI 数字 |
| 28 | 创造模式改为持久插件安装 | Rust 创造模式已定位为维护预设与技能，不假设动态 JS；缺少上游 `plugin_manager` 工具闭环。**随插件管理批次评估** |
| 29 | 客户端 Session 多实例与 slot 变化 | Rust 有 Session 缓存和目录，但仍以主选中会话为主要 UI 绑定；未见同等消费者 retain/release 契约。**P1 前端基础改造** |

## 3. 优先处理的可靠性与兼容性

### 3.1 Messages：发送兼容不等于放宽执行校验

上游将历史工具调用中非法 JSON、数组、null 或标量的 input 在**出站历史重放**时降为 `{}`，保留原日志；当前 Rust [Messages 序列化](E:/rust/deepseek-harness-rs/crates/llm/llm-deepseek/src/anthropic.rs) 对这些值直接返回 `INVALID_REQUEST`。

建议区分历史序列化与新工具执行：只在适用 Messages 路由的历史转换边界做兼容，保留诊断；新模型刚生成的坏参数继续拒绝，不能以空对象调用实际工具。兼容代码还应覆盖 lossless replay 的分支，避免某一路径绕过处理。

Rust [Messages endpoint](E:/rust/deepseek-harness-rs/crates/llm/llm-deepseek/src/anthropic_transport.rs) 已处理尾部 `/v1`，不应把这一点列为全新能力。需统一验证 Messages 和 Files 的根地址、尾斜杠、已有版本段、beta header 及自定义端点，避免出现 `/v1/v1` 或漏版本；[Files API](E:/rust/deepseek-harness-rs/crates/llm/llm-deepseek/src/files_api.rs) 不能用普通 Messages 请求通过替代验收。

依据：[共享 API root](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/llm/llm-deepseek/src/common/messages-api.ts)、[历史工具输入](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/llm/llm-deepseek/src/protocols/messages/serialize.ts)。

### 3.2 权限重复申请

[Rust approve_escalation](E:/rust/deepseek-harness-rs/crates/sandbox/sandbox/src/escalation.rs) 当前依据严格扩大集合判断，尚没有相同有效模式直接返回的分支。应在调用级有效模式解析后判断相等，再执行原有扩大权限规则；不要只比较配置缺省值。

测试同模式不询问、不更改会话权限；更宽模式仍要求原有批准；无效或不受支持的目标仍拒绝；一次调用批准不会变成永久权限。该项不同于设置页重复选中同一 preset 的去重。

依据：[上游 escalation](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/sandbox/sandbox/src/escalation.ts)。

### 3.3 Inbox 恢复与会话占用

上游缺陷的精确修复是把 Inbox 投影注册从单 Agent 构造提升到 AgentLoop 服务初始化。Rust 使用 [Inbox 日志重放](E:/rust/deepseek-harness-rs/crates/core/agent/src/inbox.rs)，并在 [release_publication](E:/rust/deepseek-harness-rs/crates/core/agent-loop/src/agent.rs) 后唤醒 pending，所以不能机械移动同名注册函数。

验收应使用独立进程：持久化未领取的 next-turn / next-step 消息及附件、关闭 Host、重启恢复，确认顺序与去重；已领取或已取消消息不能重跑，fork 的继承前缀不能变成本地待执行输入。并用第二个 Host 同时申请同一会话，确认不会双写，错误能说明占用、保留原文件，并在释放后重试成功。

依据：[上游 Inbox](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/core/agent-loop/src/inbox.ts)、[初始化注册](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/core/agent-loop/src/index.ts)。

## 4. 回合文件差异：新增记录服务，复用 Git 查看器

当前 [artifacts](E:/rust/deepseek-harness-rs/crates/host/dsh-host/src/artifacts.rs) 描述产物与文件状态，[Git 查看器](E:/rust/deepseek-harness-rs/release/plugins/dsh-better-sidebar/lib/git-terminal.js) 展示工作区 / 提交差异；它们都不能单独证明“某回合开始到结束改了什么”。

应增加以 `sessionId + turn` 标识的 before / after 捕获和只读对比服务，再通过回合末卡片打开现有侧栏：

- Git 仓库用私有 index 和私有对象目录生成快照，包含适用的未跟踪文件；不改变用户 index、工作树、refs 或未完成合并。
- 非 Git 工作区回退为文件工具前后捕获，并明确其不覆盖任意 Shell 副作用的范围。
- 二进制、超大文件、比较超时分别标记，不伪造增删行数；为文件数、快照、输出和耗时设预算。
- 并发外部编辑或多个 Agent 共享工作区时，显示的是时间区间内变化，不声称每一行都由本 Agent 产生。
- `workspace/changes` 在上游只持久化 turn 通知；摘要和 diff 默认由 live Session 持有，释放后可能不可用。若 Rust 要长期保留审阅，需要另外实现版本化快照存储，不能依据事件存在便宣称历史 diff 永久可读。

依据：[回合差异服务](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/deliverables/workspace-changes/src/index.ts)、[事件与结果结构](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/deliverables/workspace-changes/src/types.ts)、[快照隔离](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/deliverables/workspace-changes/src/git.ts)。

## 5. 多实例侧栏与终端

### 5.1 消费者持有会话

上游 [SessionReference](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/api/session-controller/src/client/contract/sessions.ts) 将每个消费者的 ready、取消和 release 分开；多个视图可以持有同一客户端 generation，最后一个引用释放时才清理本地 scope / history。这不意味着多个 Host Agent 可以同时拥有一个会话。

Rust [client-runtime](E:/rust/deepseek-harness-rs/web/src/runtime-plugins/client-runtime.js) 已缓存多个 Session，不能误称完全没有多会话基础。需要补的是主对话、侧栏子对话及多个窗格的明确持有关系：草稿、队列、审批、附件、阅读位置和订阅按会话隔离；关闭一个标签不能销毁其他视图仍使用的状态；旧连接回调不能写入新 generation。保持现有历史窗口和驻留预算。

子代理侧栏继续使用直接父子地址与 one-shot / continuable 权限。计划预览绑定确切的 `exit_plan_mode` 调用，区分待批准、已通过和过期计划，不能重新执行计划工具以取得预览。

依据：[侧栏子对话](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/client/ui-subagent/src/client/sidebar-chat/index.tsx)、[计划资源](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/client/ui-plan/src/client/plan-resource.ts)。

### 5.2 用户终端独立于 Agent 工具

当前 [Web terminal_action](E:/rust/deepseek-harness-rs/crates/host/dsh-host/src/web_preview.rs) 通过 Agent owner 创建 `shell`，后端读取会话沙箱策略。同步上游行为需新增可信用户终端的分配路径，保留 Host 鉴权、会话归属和输入控制；模型的 `terminal_*` / Shell 工具仍受 Agent 权限约束。不能让模型或普通可写参数把 Agent 工具转成用户终端。

页面刷新、视图解绑与 PTY 关闭应分别处理。保持同一 Host 进程存活时，应能重新附着到原终端身份与输出游标；Host 重启后的旧 ID 应明确失效，不能暗中创建新终端并显示为恢复成功。无人持有的终端只在确认闲置且没有工作时回收，观察失败不能视为闲置，清理失败可重试。

上游新增 window hold 和保守 idle 回收，默认无人持有且确认闲置两小时才回收；可吸收这个生命周期设计，不必强制采用相同时间常量。Rust 终端相关文件目前有未提交修改，应在最终候选上验证。

依据：[用户终端控制](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/api/terminal-controller/src/index.ts)、[保留与回收](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/api/terminal-controller/src/retention.ts)。

## 6. Office 与插件管理的 Rust 实现边界

### Office：WPS 转 PDF

上游 [office-to-pdf](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/document/office-to-pdf/src/index.ts) 使用 LibreOffice kit，支持 doc/docx、xls/xlsx、ppt/pptx。Rust 采用 **WPS Office** 转换后端，复用已随包提供的 PDF 查看器。

Host 在授权读取后复制输入到独立临时目录，以源版本 / 内容摘要缓存 PDF；限制输入体积、排队数、转换时间和缓存总量，转换过程中源文件变更则不显示过期结果。禁用宏与外部链接更新，不修改原文件，不复用或结束用户正在编辑的 WPS 进程。缺少 WPS 或平台不支持时显示明确不可用状态，并保留现有 WPS 外部打开 / 下载入口。

验收 Word 分页与字体、Excel 打印区域和跨页表格、PowerPoint 幻灯片方向及页数；界面说明这是预览而非编辑。该项是独立 Host 服务工作量，不是增加扩展名映射即可完成。

### 插件管理：保留可运行范围

Rust 已有 [纯 Web 安装器](E:/rust/deepseek-harness-rs/crates/host/dsh-cli/src/native_plugin.rs)、[插件库存界面](E:/rust/deepseek-harness-rs/web/src/runtime-plugins/ui-settings-plugin-inventory.js) 与依赖禁用 / 恢复。上游 [Plugin Manager](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/boot/plugin-manager/README.zh.md) 则管理 profile 组合包、依赖安装、构建脚本、配置文件及运行中的 Host 代码，影响同 profile 的全部会话，HMR 未开启时仍需重启。

推荐先将 Rust 已支持的安装、配置和实时启停汇入统一页面，补请求身份、取消、输出日志、配置写锁、失败恢复和卸载释放；明确区分停用、卸载文件、重启生效与运行时生效。Node Host 插件需要单独的运行环境或 Rust 对应模块，不能由一次 UI 对齐隐式引入任意 npm 执行能力。

Rust 创造模式当前用于维护预设与技能，并未提供等价的动态 Node Host 执行。若加入 `plugin_manager` 工具，应只暴露已实现的操作及明确权限；原 UI 中保留历史 Cordis 卡片不构成可执行后端已存在的证据。

## 7. 配置与默认值

- **子代理限额**：上游默认深度 1、最多 8 个驻留 continuable 子代理，额度由连续父子链共享池管理，在重建前占位，失败和释放时归还。它不同于一次模型请求的并行工具数、正在运行的回合数或 Team 成员总数。Rust 当前 `tool-subagent` 默认深度 3，Host 还有自有 `maxParallel` / depth 选项，应统一语义再迁移缺省；显式用户配置保留。
- **旧 Flash 目录**：仅调整新装缺省和建议目录，不能删除旧会话中的模型 ID、凭据、用户手工条目或已有可用自定义路由。缺省移除不代表协议必须拒绝用户显式使用。
- **CLI 简写**：增加 `dsh <profile>` 时，保持 `web`、`plugin`、`history` 等已有子命令优先；重复指定 profile 明确报错，profile 内参数及 `--help` 继续正确转发。
- **插件依赖解析**：Node 的 runtime resolver 不是 Rust crate 的运行时替代物。只对插件加载协议和外部配置名称做必要兼容，内部继续遵循 Rust 所有权及释放模型。

依据：[子代理配置](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/subagent/subagent/src/index.ts)、[驻留池](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/subagent/subagent/src/continuation-activation.ts)、[缺省模型](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/llm/llm-deepseek/src/common/models.ts)、[CLI 参数](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/apps/cli/src/args.ts)。

## 8. 实施顺序和验收范围

| 批次 | 范围 | 完成条件 |
|---|---|---|
| A：可靠性 | 历史 Messages 输入、权限重复请求、视觉发现、Inbox 冷恢复及会话占用 | 错误路径和恢复 fixture；独立进程恢复与冲突测试；新工具执行仍严格校验 |
| B：Host 能力 | 回合差异、用户终端、子代理驻留配额、WPS 转换 | 不改用户原始文件 / Git 状态；资源和权限隔离；终端不断连及转换取消可验证 |
| C：多视图界面 | Session 消费者引用、子对话、计划、冷会话、持久布局 | 同时打开主 / 子会话，关闭、重连、刷新后无草稿、权限或输出串线，内存有界 |
| D：插件与体验 | 统一插件页、CLI 简写、默认目录、菜单、Trajectory、思考、上下文和启动诊断 | 受支持插件完整安装 / 取消 / 卸载；键盘、视觉和设置回归；正式资源同步 |

可先交付批次 A 和不依赖多视图的小修复。回合差异及 Office 可使用现有 `betterSidebar` 注册接口独立开发，侧栏子对话必须先解决 Session 持有与销毁边界。禁止通过整体覆盖上游 bundle 丢失 Rust 的分页、账号、执行和插件能力。

## 9. 验证证据与边界

- 完成 29 项发布范围与 alpha.1→alpha.2 关键源码对照；此前 alpha.1 的 MCP、Headless、PTC 等进展以当前代码和独立验收为准，不重复列为此版本新功能。
- 执行 `session_queue_harness.cjs`：通过主 / 子代理投递、sending、重连、确认合并、重试及交接场景。
- 执行 `subagent_resume_harness.cjs`：通过子代理 idle 回收、重开、目录刷新、旧 / 不可用父归属及 one-shot 隔离场景。
- 两个 harness 属于客户端夹具验证，不证明真实 Host 重启后 Inbox 已完整恢复，也不证明多个客户端视图已经隔离。
- `client-runtime.js`、`ui-subagent.js`、`ui-settings-plugin-inventory.js`、`ui-conversation.js`、`ui-settings-models.js` 的 source / dist 一致，manifest 修订匹配其 dist 哈希。
- 工作树还有其他终端、启动器等改动；上述验证范围不包括 Office 转换、插件安装、外部模型请求或完整跨平台发布验收。

最终候选仍需将代码提交、二进制身份、前端资源及 Windows / Linux / macOS 验收绑定。当前源码版本号或历史文档中的通过记录，不等于对应能力已在公开安装包交付。
