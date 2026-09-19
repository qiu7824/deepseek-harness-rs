# Rust WebUI 差异与界面补齐方案

基线：Rust `0.1.3-alpha.22`，提交 `e7bafb32f0fecccb1fd6fb754c0b1e097a5b31fc` 及当前工作树；对照上游 `dsh-v0.1.6-alpha.2`，提交 `ddefc45fbc7f8e46dd73185e68295696d1297887`。

范围：界面组件、注册入口、发布 manifest 与关联 Host 接口。界面可用性以实际装配为准；本核对不构成安装版逐屏视觉验收。

## 1. 结论

**无需重新设计整套 WebUI。建议补齐 5 项产品界面，接通 3 个已有源码但未发布加载的模块，并在现有界面内完成导航与交互改进。**

建议只将“插件中心”作为新的全局页面；回合差异、Office、子代理对话、计划预览放入现有右侧工作台。工具发现与技能版本放在设置中，任务验收放在对应会话内。保持现有主题、按钮、字体、间距和窄屏适配。

## 2. 建议新增的 5 项 UI

| 优先级 | UI 能力 | 当前界面 | 建议入口与内容 | 所需基础 |
|---|---|---|---|---|
| P1 | **回合文件改动卡与审阅面板** | 有交付文件行和 Git 工作台；未找到同等的回合起止差异卡 | 回合结束后显示“修改了 N 个文件 · +X / −Y”；点击在右侧列出该回合文件、前后对比、增删行数及不可比较原因 | Host 捕获回合前后版本；不能直接显示当前 Git 状态并标为该回合改动 |
| P1 | **侧栏子代理对话** | 有成员目录、状态、续聊和主区域跳转；缺少保持主对话同时展开子对话的完整链路 | 成员行提供“在侧栏打开”；右侧显示子代理正文、队列、输入和停止，顶部标明父会话与运行状态 | 独立 Session 视图绑定、草稿、审批、附件和订阅生命周期 |
| P1 | **计划预览标签** | 有计划模式与审批卡，长计划仍主要在对话中查看 | 在计划卡加入“预览计划”；右侧显示标题、状态、Markdown、复制及返回来源，批准 / 拒绝仍绑定同一计划版本 | 读取确切计划提交或待审查记录，不能触发新的计划执行 |
| P1 | **Office 内嵌预览** | Word / Excel / PowerPoint 使用下载查看器或外部 WPS 打开 | 点击文档进入右侧 PDF 预览；显示转换中、页数、缩放、源文件更新、字体 / 格式提示和“用 WPS 打开” | WPS 转 PDF 的有界队列与版本缓存；缺少 WPS 时明确回退 |
| P1 | **统一插件中心** | 插件库存、配置及启停已存在，纯 Web 安装主要通过 CLI | 全局“插件”入口：已安装、可用范围、配置、启停、安装 / 移除、日志、失败重试与生效状态 | 复用 Rust 支持的插件安装和配置服务；任意 Node Host 插件仍不能直接装入 Rust 进程 |

这些能力中，Office 和回合差异有较多 Host 工作；子代理并排对话涉及前端状态生命周期，不能仅添加一个标签按钮。

上游依据：[回合差异](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/deliverables/workspace-changes/src/types.ts)、[子代理侧栏](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/client/ui-subagent/src/client/sidebar-chat/index.tsx)、[计划预览](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/client/ui-plan/src/client/PlanPreview.tsx)、[Office 查看器](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/client/ui-sidebar-documentpreview/src/client/office/OfficeBody.tsx)、[插件中心](https://github.com/deepseek-ai/deepseek-harness/blob/ddefc45fbc7f8e46dd73185e68295696d1297887/packages/client/ui-plugin-manager/src/client/PluginManagerPage.tsx)。

## 3. 先接通的 3 个现有模块

三者源码存在、JavaScript 语法检查通过，但对应文件均不存在于 `web/dist/plugins`，也未进入正式 manifest，不能按已显示在产品中计算。这不是上游要求新增的三个页面，而是 Rust 自身已有功能的交付接线缺口。

| 模块 | 页面已写的内容 | 产品位置 | 后端线索 |
|---|---|---|---|
| [工具发现设置](E:/rust/deepseek-harness-rs/web/src/runtime-plugins/ui-settings-tool-discovery.js) | 按需工具开关、预算、保存 / 放弃、运行状态和需重启提示 | 设置 → 工具发现；与现有技能 / MCP 设置形成清晰分工 | Host 已注册 `/__dsh-tool-discovery` |
| [技能候选与版本](E:/rust/deepseek-harness-rs/web/src/runtime-plugins/ui-settings-skill-revisions.js) | 候选、样本、验证、激活、撤回与版本恢复 | 设置 → 技能与 MCP → 技能版本，避免再创建重复的技能首页 | `capabilities.skillRevision*` 已有服务分派 |
| [任务验收与恢复](E:/rust/deepseek-harness-rs/web/src/runtime-plugins/ui-task-execution.js) | 任务状态、完成条件、验收证据、失败恢复及未知效果核实 | 对话内“任务验收”视图，或当前工作台的会话级标签 | Host 已注册 `/__dsh-task-execution` |

接通步骤应包括 slot 兼容、前端 RPC 形状、Host 权限与返回值验证，再生成 dist、登记 manifest 和哈希；不能只追加 manifest 条目即认定可用。三块页面还需统一使用现有控件样式、本地化、加载与空态，避免源码中的临时表单样式成为另一个视觉体系。

## 4. 已有功能：完善现有 UI，不重复新建

| 已有界面 | 应保留的能力 | 需要完善或验收 |
|---|---|---|
| 右侧工作台 | 多标签、分栏、浮窗、全屏和布局持久化 | 统一入口与激活位置；卸载插件后占位和恢复；小屏不覆盖正文 |
| 网页预览 | URL 导航、前进 / 后退、刷新、沙箱 iframe | 被网站拒绝嵌入时提供外部打开；区别于模型受控浏览器 |
| 终端 | xterm、多终端、固定入口、输入与退出状态 | Shell 选择入口、连接 / 失效状态、刷新重新附着、用户终端与 Agent 终端身份说明 |
| 文件与 Git | 树、文本编辑、Markdown、PDF、图片、HTML、结构化数据和 Git diff | 已有文件直接打开、首次消息前使用、树阅读位置与跨会话隔离；回合差异作为新数据源接入 |
| 归档会话 | 设置页已经注册归档列表及恢复 / 删除操作 | 恢复后定位、列表刷新、正文读取和子代理树处理 |
| 上下文用量 | `ContextMeter` 已在输入框底部工具行，点击有明细 | 只需核对拥挤布局、窄屏与上游样式差异，不再新增一个计量入口 |
| 模型设置 | 已有“支持图片输入”复选框和模型参数 | 明确自动识别 / 手动覆盖，刷新时保留用户选择；这是数据流校验而非缺少控件 |
| 反馈 | 已有确认弹窗、分类、失败保留和独立投递 | 保持现有行为，避免升级时回退到立即记录 |
| 技能、MCP、记忆、账号 | 已有管理和状态入口 | 合并相关设置层级；新增功能进入原有分区，不重复建主页 |
| 代码图谱、Computer Use、UU、团队与后台任务 | Rust 自有界面与联动能力 | 在侧栏更新时保留，避免替换官方 bundle 后丢入口或归属 |

实现依据：[工作台注册](E:/rust/deepseek-harness-rs/release/plugins/dsh-better-sidebar/lib/client.js)、[文档查看器注册](E:/rust/deepseek-harness-rs/release/plugins/dsh-sidebar-workbench-suite/lib/client.js:1041)、[归档入口](E:/rust/deepseek-harness-rs/web/src/runtime-plugins/ui-workspace.js:2809)、[输入框与 ContextMeter](E:/rust/deepseek-harness-rs/web/src/runtime-plugins/ui-conversation.js:4318)、[模型图片输入](E:/rust/deepseek-harness-rs/web/src/runtime-plugins/ui-settings-models.js:1019)。

## 5. 可以集中完成的交互改进

1. **工作区目录树**：在现有工作区列表增加“按目录层级”视图，保留搜索、手动顺序和当前会话定位；相同文件夹名不能混组。
2. **输入框菜单**：当前仍有独立附件按钮。若采用上游紧凑布局，将文件上传并入加号菜单，保留粘贴 / 拖放和上传队列；统一各菜单的方向键、Enter、Tab、Escape、焦点返回及中文输入法行为。
3. **思考内容**：在现有推理组件内完善紧凑 Markdown；不要改变原文本、复制结果或用户折叠偏好。
4. **Trajectory 附件**：统一文字、图片和文件的摘要与预览，缩略图延迟加载、会话授权、失效状态和取消清理配套。
5. **启动与连接诊断**：已有日志和错误入口之上，显示失败服务、仍在等待的服务、日志位置及重试动作；日志脱敏。可做恢复面板，无需常驻独立一级页面。
6. **子代理限制**：在现有子代理设置中显示深度和驻留上限，达到限制时在创建入口给出可理解的状态；不要把并行工具数标成驻留子代理数。
7. **窗口与悬浮提示**：缩小拖宽命中区，避免透明层遮挡内容；统一嵌套 tooltip 的关闭、层级与键盘焦点。

## 6. 推荐的信息组织

| 区域 | 入口 |
|---|---|
| 主导航 | 工作区 / 会话、插件、设置；保留现有可用全局页面 |
| 对话回合末 | 本轮文件改动、交付文件、反馈；任务未完成时给出任务验收入口 |
| 右侧工作台 | 复用文件、Git、终端、网页；按操作打开“本轮改动”“子代理对话”“计划”“Office 文档”标签 |
| 设置 | 在技能 / MCP、执行环境和子代理已有分区内补工具发现、技能版本及限制项 |
| 错误 / 恢复状态 | 在当前任务或窗口就近展示错误、重试与日志，不用新的常驻面板挤占正文 |

这套组织只增加必要入口。文档打开以具体文件名为标签，计划以标题为标签，子对话以成员名为标签，避免把所有新功能都做成一级导航按钮。

## 7. 实施顺序与可见完成标准

**第一批：接通已有 UI。** 完成工具发现、技能版本和任务验收的运行接线、正式资源加载及产品内操作验证。出现页面不等于保存、恢复和权限已工作。

**第二批：补常用工作台能力。** 回合差异、计划预览和统一插件入口；插件页先覆盖 Rust 已支持的操作，持续反馈安装与生效状态。

**第三批：补跨视图和文档能力。** 子代理并排聊天与 WPS Office 预览。前者先做好会话引用生命周期，后者先做好转换服务与缓存，随后完成完整交互。

菜单、目录树、思考、Trajectory 和窗口细节可随相关批次完成。所有新增区域须覆盖加载、空数据、正常、错误、取消、失效及只读状态，并验收键盘、中文、浅深主题和窄屏。

验收必须从正式 manifest 加载页面，确认 source、dist、哈希与包内资源一致；至少执行一次“打开 → 操作 → 切换会话 → 刷新 → 再打开”。涉及保存、终端及反馈的结果须核对 Host 实际状态。当前结论来自代码与注册核对，尚未进行新增页面的真实浏览器逐屏验收。
