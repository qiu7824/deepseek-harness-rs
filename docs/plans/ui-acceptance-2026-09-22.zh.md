# UI 功能、布局与状态验收矩阵

核对日期：2026-09-22。验收基准为实际发布清单、组件注册位置、安装包及运行中的 Host；组件存在、DOM 测试存在、静态扫描通过均不等于页面实测通过。

## 发布边界与证据规则

- Web 发布入口为 `web/src/main.ts`，发布资源为 `web/dist`；`tools/stage_release_web.py` 校验清单内文件及内容摘要后复制到发布目录。
- 当前 `web/dist/plugins/manifest.json` 列出 45 个模块。29 个文件具有同名 `web/src/runtime-plugins` 源文件，其余模块以当前发布文件为核对入口。发布前必须再次核对源文件、发布文件与清单摘要的一致性。
- `release/plugins` 另外包含 6 个随包扩展：`dsh-artifacts`、`dsh-better-sidebar`、`dsh-context-jump`、`dsh-sidebar-workbench-suite`、`dsh-skin-center`、`dsh-voice-input`；它们提供的页面、文件查看器和弹层均纳入验收。
- 页面以 slot、主面板 key、会话 view id、工作台 tab id 注册，不能只按 URL 路由枚举。
- 所有矩阵行当前均为 **待完整实测**。测试列表示已找到的回归入口，执行版本、日志、截图和断言结果须另行补齐。已有 DOM harness 主要使用 JSDOM，不能证明真实浏览器文字换行、画布渲染、遮挡或滚动尺寸正确。

## 通用验收维度

| 编号 | 维度 | 验收范围 |
|---|---|---|
| V1 | 宽桌面 | 1920 × 1080；左侧导航、主对话和右侧工作台同时打开 |
| V2 | 常规桌面 | 1280 × 800；工作台展开/收起、底部终端、长标题 |
| V3 | 窄桌面 | 900 × 720；调整面板宽度、设置导航与正文、打开菜单 |
| V4 | 手机 | 390 × 844；侧栏抽屉、设置、输入法占位与底部操作区 |
| V5 | 最窄支持布局 | 320 × 568；长文件名、中文、多行错误、对话与设置操作可达 |
| D1 | 缩放与无障碍 | Windows 125%/150%/200% 缩放及浏览器 200% 缩放；键盘 Tab、可见焦点、Escape、屏幕阅读器名称 |
| T1 | 基础主题 | 浅色、深色；所有页面及状态均覆盖 |
| T2 | 已发布皮肤 | 蓝色幻想、夕港、XP、Minecraft、交易终端、初音未来、DeepSeek Harness 官方；逐组页面核对正常、错误、焦点与弹层 |
| S1 | 初始与空状态 | 首次连接、无任务、无文件、无搜索结果、无配置 |
| S2 | 异步状态 | 加载、保存中、流式内容、长时间等待、重复点击 |
| S3 | 失败状态 | 请求失败、服务不支持、无权限、资源不存在、配额不足、重试 |
| S4 | 导航与恢复 | 会话切换、旧响应迟到、关闭再打开、浏览器刷新、Host 重启、断线重连 |
| S5 | 编辑一致性 | 草稿、取消编辑、版本冲突、保存失败保留输入、成功后重载验证 |
| S6 | 执行控制 | 停止、中断、暂停、恢复、再次发起；控制只影响所选任务，迟到结果不能改变新任务状态 |

共同要求：正文和操作区无非预期水平溢出；不依靠裁切隐藏必要按钮；对话、表格、代码和树视图各自使用合适的滚动容器；同类控件字号、行高、间距、圆角和状态颜色保持一致。文件预览、图谱和终端可内部滚动，页面外层不能因此产生横向滚动。

## 主界面与会话视图

路径缩写：`R` = `web/src/runtime-plugins`，`W` = `web/dist/plugins`，`P` = `release/plugins`，`H` = `tools/tests`。所有行默认覆盖 V1–V5、T1、D1；状态列列出重点并不排除其他适用状态。

| 页面/入口 | 实际组件与注册 | 重点状态 | 已有回归入口 | 未完成的实测证据 |
|---|---|---|---|---|
| 应用框架、主面板切换 | `R/ui-layout.js`：`AppFrame`、主面板 `conversation`；`R/ui-sidebar.js` | S1、S4；切换插件中心后返回草稿 | `H/global_panel_dom_harness.cjs`、`H/mobile_layout_dom_harness.cjs` | 实际窗口、缩放、键盘焦点、中心列不被两侧挤压 |
| 工作区与任务导航 | `R/ui-workspace.js`、`R/ui-sidebar.js` | S1–S5；工作区创建/重命名、任务搜索/归档 | `H/workspace_browser_dom_harness.cjs`、`H/workspace_sources_dom_harness.cjs` | 长名称、海量任务、远端断连、重启恢复 |
| 对话 `chat` | `R/ui-conversation.js`；`conversation.view/chat` | S1–S6；流式文本、输入编辑、重新发送、停止 | `H/conversation_interactions_dom_harness.cjs`、`H/composer_empty_dom_harness.cjs`、`H/chat_scroll_dom_harness.cjs`、`H/response_rendering_dom_harness.cjs` | 同一安装版本多轮真实对话、停止延迟、历史分页、长 Markdown |
| 执行轨迹 `trajectory` | `R/ui-trajectory.js`；`conversation.view/trajectory` | S2–S4、S6；折叠、分页、失败与取消 | `H/ptc_tool_cards_dom_harness.cjs`、`H/test_trajectory_builder_perf.mjs` | 大量工具事件时滚动、保留锚点、内存释放 |
| 产物 `artifacts` | `P/dsh-artifacts/lib/client.js`：`ArtifactView`；`conversation.view/artifacts` | S1–S5；刷新、重命名、预览、下载 | `H/artifacts_dom_harness.cjs`、`H/generated_images_dom_harness.cjs` | 文件真实可打开、重命名冲突、长路径、右键菜单不越界 |
| 项目任务 `project-tasks` | `R/ui-productivity.js`：`ProjectTasks` | S1–S5；新增/编辑标题、详情、优先级、状态 | `H/productivity_dom_harness.cjs` | `PROJECT_TASKS.md` 与页面保存/重启一致；长详情编辑布局 |
| 代码图谱 `code-graph` | `R/ui-code-graph.js`：`CodeGraphView` | S1–S4；过滤、聚焦、源文件跳转 | `H/file_actions_graph_dom_harness.cjs` | 真实 Canvas/SVG、窄窗口、节点详情遮挡、颜色可辨 |
| 上下文 `context` | `R/ui-code-graph.js`：`ContextView`；`R/ui-settings-general.js`：`ExperiencePanel` | S1–S5；模型切换、经验过滤、内容刷新 | `H/context_rail_dom_harness.cjs`、`H/learning_dom_harness.cjs`；`tools/e2e_context_stability.py` | 多轮、子任务、技能、停止、恢复、压缩后上下文注入数量与内容 |
| 子智能体完整对话 | `R/ui-subagent.js`；`client-runtime.js` 的 `sessions.openSubagent` | S2–S6；主入口一步打开、返回父任务、成员切换 | `H/collaboration_reliability_dom_harness.cjs`、`H/subagent_progress_dom_harness.cjs`、`H/agent_team_dom_harness.cjs`；`tools/e2e_collaboration_controls.py` | 实际协同任务入口、返回路径、消息和停止准确归属 |
| 插件中心主面板 | `R/ui-settings-plugins.js`：`main/plugin-center`；`PluginInstallControls` | S1–S5；查询、校验、安装/更新、失败 | `H/global_panel_dom_harness.cjs`、`H/settings_controls_dom_harness.cjs` | 与设置弹层样式一致、安装信息可读、关闭后草稿不丢失 |

## 设置导航

| 页面 ID | 实际文件/组件 | 重点验收 | 已有回归入口 |
|---|---|---|---|
| `general` | `R/ui-settings-general.js`：`GeneralSection`；`W/ui-theme.js`、`W/locale.js` 注入项目 | 语言、外观、目录选择方式；未保存提示与主题即时更新 | `H/settings_controls_dom_harness.cjs`、`H/mobile_layout_dom_harness.cjs` |
| `models` | `R/ui-settings-models.js`：`ModelsSection`、`TaskModelsPanel`、`AccountAuthorizationBrowser` | 账号授权、现有连接、任务角色模型、网络诊断；机密字段、保存冲突 | `H/model_management_rpc_dom_harness.cjs`、`H/task_models_dom_harness.cjs`、`H/account_browser_login_dom_harness.cjs`、`H/hosted_search_settings_dom_harness.cjs` |
| `free-models`，仅 free 变体 | `R/ui-settings-models.js`：`FreeModelsSection`；受 `__DSH_BOOT__.variant` 控制 | free 包显示、core 包不误显示；目录加载与服务不可用 | 模型相关 harness；变体真实入口待验收 |
| `plugins` | `R/ui-settings-plugins.js`：`PluginsSettingsSection`；`R/ui-settings-plugin-inventory.js` | 可配置/清单子页、插件开关和配置、独立加载样式、错误恢复 | `H/settings_controls_dom_harness.cjs`、`H/sidebar_suite_dom_harness.cjs` |
| `workbench-context` | `W/ui-workbench.js`：`ContextSettings` | 数据/缓存/环境目录、路径迁移、运行环境状态、检查失败 | `H/settings_controls_dom_harness.cjs`；真实环境/路径迁移待验收 |
| `memory` | `R/ui-settings-general.js`：`MemorySection`、`ExperiencePanel`；`R/ui-productivity.js`：`MemoryImport` | 记忆分类、经验诊断、确认、删除、版本冲突、导入预览；删除数量有证据 | `H/learning_dom_harness.cjs`、`H/productivity_dom_harness.cjs`、`H/mobile_layout_dom_harness.cjs` |
| `agent-presets` | `R/ui-agent-preset.js`：`AgentPresetSection` | 选择、编辑、默认预设、冲突与未保存草稿 | `H/agent_preset_dom_harness.cjs` |
| `agent-teams` | `R/ui-subagent.js`：`TeamSettings`、子智能体默认配置 | 协同开关、并行数、模型继承、成员控制、跨会话保存 | `H/agent_team_dom_harness.cjs`、`H/collaboration_reliability_dom_harness.cjs` |
| `security` | `R/ui-settings-general.js`：`SecuritySection` | 审批超时、无人确认、权限预设；保存与运行行为一致 | `H/approval_rpc_dom_harness.cjs`、`H/settings_controls_dom_harness.cjs` |
| `capabilities` | `W/ui-settings-capabilities.js`：`CapabilitiesSection` | 技能与 MCP 列表、启停、缺依赖、认证错误、长配置 | `H/mobile_layout_dom_harness.cjs`；完整功能待实测 |
| `tool-discovery` | `R/ui-settings-tool-discovery.js`：`ToolDiscoverySection` | 预算、勾选、保存/失败；独立挂载不依赖别页 CSS | `H/settings_controls_dom_harness.cjs` |
| `windows-sandbox` | `R/ui-settings-windows-sandbox.js`：`WindowsSandboxSection` | 后端、网络、初始化/检查、配置版本冲突、失败消息；不再显示已退役 AppContainer 为默认可用能力 | 专用后端/RPC 回归；页面真实操作待验收 |
| `archived-sessions` | `R/ui-workspace.js`：`ArchivedSessionsSection` | 空列表、查询、恢复、删除；恢复后列表/对话一致 | 工作区相关 harness；归档真实恢复待验收 |
| `mini-menu` | `R/ui-productivity.js`：`Menus` | 小菜单项目与开关、修改/取消/刷新 | `H/productivity_dom_harness.cjs` |
| `workspace-scratch` | `P/dsh-artifacts/lib/client.js`：`Settings` | 资源列表、保留/释放、路径、占用状态；不可清理保护资源 | `H/artifacts_dom_harness.cjs`；`tools/e2e_scratch_context.py` |
| `skins` | `P/dsh-skin-center/lib/client.js`：`SkinCenter` | 基础主题与 7 个附加皮肤，切换、刷新、重启、下载失败 | 主题/移动布局 harness；所有实际皮肤组合待验收 |

每个设置页均执行 S1–S5：加载失败不能覆盖草稿，保存失败不得显示成功，版本冲突必须保留输入，保存完成后重新读取权威状态；关闭弹层、切换会话和恢复连接不得混入其他页面响应。

## 工作台、查看器与嵌入交互

| 入口/类型 | 实际文件/组件 | 重点验收 | 已有回归入口 |
|---|---|---|---|
| 文件/Git/网页/终端 | `P/dsh-better-sidebar/lib/client.js`：`explorer`、`git`、`browser`、`terminal`；`lib/git-terminal.js` | 面板宽度、分屏、底部栏、悬浮、切换任务；布局重载、资源关闭 | `H/better_sidebar_dom_harness.cjs`、`H/git_terminal_dom_harness.cjs`、`H/sidebar_suite_dom_harness.cjs` |
| Markdown | `P/dsh-sidebar-workbench-suite/lib/client.js`：`MarkdownWorkbench`、`suite:markdown` | 正文、标题大纲、Mermaid、切换源文件、长表格 | `H/sidebar_suite_dom_harness.cjs`、`H/document_preview_registry_harness.cjs` |
| JSON/CSV/TSV | 同上：`StructuredViewer`、`suite:structured` | 大表格、横向内部滚动、引号/分隔符、无数据与格式错误 | 工作台 harness；真实数据表渲染待验收 |
| 文本和代码 | 同上：`CodeWorkbench`、`suite:code` | CodeMirror、编辑草稿、撤销、版本冲突、行号定位 | `H/sidebar_suite_dom_harness.cjs`、`H/file_actions_graph_dom_harness.cjs` |
| DOCX/XLSX/PPTX | 同上：`OfficeViewer`、`suite:office` | WPS 导出、页码、缩放、错误、切换文件、取消/关闭、再次打开 | `H/pdf_preview_dom_harness.cjs`、`H/document_preview_registry_harness.cjs`；必须补实际页面截图与源文件一致性 |
| PDF | 同上：`PdfViewer`、`suite:pdf` | 原 PDF 加载、页码、缩放、渲染取消、损坏/空文件 | `H/pdf_preview_dom_harness.cjs`；必须补实际 PDF.js 画布渲染 |
| 图片 | 同上：`ImageViewer`、`suite:image`；`R/ui-tool.js` | 图片尺寸、缩放、透明背景、错误、生成结果、切换会话 | `H/generated_images_dom_harness.cjs`、`H/file_upload_dom_harness.cjs` |
| 原生打开/下载 | 同上：`DownloadViewer`、`suite:download` | DOC/XLS/PPT、归档文件等；文件链接和原生打开结果可验证 | 查看器注册/文件操作 harness；原生动作待实测 |
| 后台任务 | 同上：`JobsTab`、`suite:jobs`；`W/ui-jobs.js` | 运行、结束、失败、取消、日志分页、关闭面板不误终止任务 | 工作台与协同 harness；长时间运行和恢复待实测 |
| Computer Use 与设备 | 同上：`ControlledBrowserTab`、`suite:controlled-browser`、`ComputerUseSettings` | 浏览器/窗口、截图、刷新开关、断线、切换任务、关闭归属 | 工作台 harness；真实受控浏览器/窗口待实测 |
| 子对话侧预览 | `R/ui-workbench-previews.js`：`SideConversation`、`suite:child-chat` | 辅助预览、转主区、输入草稿、停止、旧轮询迟到、关闭清理 | `H/collaboration_reliability_dom_harness.cjs` |
| 计划预览 | 同上：`PlanPreview`、`suite:plan-preview`；`W/ui-plan.js` | 长计划、来源跳转、编辑/确认边界、切换任务 | 工作台 harness；计划流程待实测 |
| 目标、工作流、定时计划 | `W/ui-goal.js`、`W/ui-workflow-run.js`、`W/ui-schedule.js` 的会话节点、输入栏和标题动作 | 创建、运行、暂停、中断、恢复、完成、Host 重启 | 协同/对话 harness；目标和定时计划完整闭环待实测 |
| 审批与问题 | `R/ui-permission.js`、`R/ui-user-questions.js` | 多问题、超时、拒绝、断线恢复、重复提交、控制只作用于目标请求 | `H/approval_rpc_dom_harness.cjs`、`H/question_wait_dom_harness.cjs` |
| 输入触发、文件上传、语音 | `R/ui-input-trigger.js`；`P/dsh-voice-input/lib/client.js`：`VoiceInputButton` | 按住/松开、实时中间文本、识别结束、麦克风失败、输入草稿、会话切换 | `H/file_upload_dom_harness.cjs`、`H/voice_input_dom_harness.cjs`、`H/composer_empty_dom_harness.cjs`；真实音频延迟须另测 |
| 模型、预设、技能与命令选择 | `R/ui-model-selection.js`、`R/ui-agent-preset.js`、`W/ui-skill.js`、`W/ui-commands.js` | 搜索、当前项、不可用项、键盘选择、菜单定位、切换会话后的选择一致性 | `H/model_current_label_dom_harness.cjs`、`H/agent_preset_dom_harness.cjs`、`H/conversation_interactions_dom_harness.cjs` |
| 目录选择与文件动作 | `W/directory-picker-browse.js`、`W/ui-file-actions.js` | 浏览/原生方式、根目录、长路径、无权限、取消、创建目录 | `H/workspace_browser_dom_harness.cjs`、`H/file_actions_graph_dom_harness.cjs` |
| 反馈、引用与导航轨道 | `R/ui-message-feedback.js`；`P/dsh-context-jump/lib/client.js`；`W/session-log-download.js` | 反馈提交、引用跳转、历史回载、导出、长文字提示 | `H/session_feedback_dom_harness.cjs`、`H/feedback_submission_dom_harness.cjs`、`H/context_rail_dom_harness.cjs` |

## 静态核对发现及待复现项

| 编号 | 位置 | 已观察事实 | 后续判定依据 |
|---|---|---|---|
| U01 | `R/ui-task-execution.js`：`TaskExecutionView`、文件末尾 `apply()` | 已接入会话页头“任务验收”入口；真实浏览器已核验空状态、关闭及焦点返回，宽屏浅色和窄屏明暗主题可读；完整任务状态仍待实测 | 任务合约、验收项、执行效果恢复的实际用户入口；不能用仅组件测试证明入口可用 |
| U02 | `R/ui-productivity.js`、`R/ui-settings-tool-discovery.js`、`P/dsh-artifacts/lib/client.js`、`R/ui-workbench-previews.js` | 同类按钮分别使用 34 px/38 px/无最小高度，圆角分别为 18 px/8 px/7 px/8 px | 在同主题下并列截图与控件 computed style，确定统一基础规格后局部修正 |
| U03 | `R/ui-settings-plugins.js`：`main/plugin-center` | 主面板采用内联 `padding:24`、独立原生 `h1` 与按钮；设置页有另一套组件样式 | V3–V5 与设置弹层、导航头部对比；不以全局 CSS 强制覆盖修补 |
| U04 | `P/dsh-artifacts/lib/client.js`：`.dsa-dialog`、`.dsa-menu` | 弹层固定 `top:35%`、高 z-index，未给对话框正文设置高度上限/内部滚动 | V5、200% 缩放、输入法、长错误和按钮键盘可达性；当前仅标为风险候选 |
| U05 | `P/dsh-better-sidebar/lib/client.js`：`MIN_WIDTH=420`、`MOBILE_BREAKPOINT=768`、`visibleWidth` | 有移动断点和宽度限制，组件依赖整窗宽度；单看常量不能判定溢出 | V3 窄桌面三栏和 V4/V5 抽屉、全屏、底部区组合实际尺寸 |
| U06 | `R/ui-settings-general.js`、模型/协同设置注册 | 多个设置页复用相同 `order` 值，页面排列取决于注册顺序 | 冷启动、热重载、插件启停后顺序必须稳定且可理解 |
| U07 | `W/ui-settings-skill-revisions.js` | 发布文件为无操作兼容模块；无有效设置注册 | 不应虚构为“已验收页面”，也不能因文件存在擅自恢复已撤销功能 |

## Flutter 集成边界

独立工作区中的 `E:\rust\deepseek-harness-rs\apps\desktop_flutter` 与 `packages\dsh_client_dart` 当前仍是未跟踪开发目录，不属于上述隔离 Web 发布树。它们通过同一 Host 的 HTTP RPC/WebSocket 连接服务；共享的任务、取消、历史分页、审批与设置协议必须兼容。

已核对的真实入口为 `lib/main.dart` → `lib/src/app.dart` → `lib/features/shell.dart`：使用 `ShadApp`，具有浅深主题、可调整侧栏/对话/工作台、任务菜单和连接弹层。`lib/features/settings/settings_shell.dart` 声明 14 个设置入口：general、models、plugins、environment、memory、presets、collaboration、security、skills、discovery、sandbox、archive、menu、trash。文件和依赖正在变化，旧 README 的“仅文本任务”说明不能替代当前代码的能力验收。

| 集成面 | 独立代码位置 | 验收要求 |
|---|---|---|
| 服务连接和客户端资源 | `lib/src/controller.dart`、`lib/src/preferences.dart`；`packages/dsh_client_dart` | 关闭客户端不误终止 Host；断连、旧响应、取消与草稿隔离；Host 内存与 Flutter 进程分别记录 |
| 对话和任务控制 | `lib/src/conversation.dart`、`lib/src/interactions.dart`、`lib/features/shell.dart` | 与 Web 的任务选择、停止、审批、编辑、重连语义一致 |
| 设置 | `lib/features/settings/settings_shell.dart`、`models_page.dart`、`resource_page.dart` | 设置版本冲突、未保存输入、凭据隐藏、Host 重启项提示一致 |
| 已有测试入口 | `test/controller_test.dart`、`test/workbench_test.dart`、`test/launcher_test.dart`、`integration_test/desktop_flow_test.dart` | 在该客户端负责人集成后的实际源码和产物上重跑；旧 exe 或依赖清单不能证明当前源码可构建 |

磁盘上存在旧 `build/windows/x64/runner/Release/dsh_desktop.exe`，时间戳为 2026-09-21 19:08:49；当前源码已有后续修改，因此旧二进制不能作为当前 Flutter 页面完成证据。该目录的代码、测试、构建和发布由其所属开发任务集成；Web 修复不覆盖或删除它的修改。

## 三轮实测记录结构

1. 正常功能轮：完成矩阵全部入口、正常编辑与导航，记录安装版本、源版本、清单摘要、视口、主题、操作和截图。
2. 异常与恢复轮：故障注入、取消、迟到响应、冲突、关闭重开、断线及 Host 重启；检查对应任务、草稿和资源归属。
3. 连续运行轮：长对话、多子任务、多文档与工作台反复开关、设置反复编辑；同步记录 Host 内核峰值工作集、当前工作集、私有提交、子进程及客户端资源。

每项结果以“通过、失败、不适用”记录；不适用须给出已核实的发布变体或平台条件。未运行项目维持待验收。发布前逐行补齐结果；任何必需项目失败或 Host 执行峰值超过 50,000,000 字节均不满足验收门槛。
