# Flutter 桌面设计系统与图标校验

日期：2026-09-29。适用组件：`apps/desktop_flutter`。

## 设计令牌

`DshTokens` 是 Material、Shad 与桌面基础组件的共同来源，通过 `ThemeExtension` 注入主题。旧的 `DshColors` 接口读取同一令牌。正文、辅助文字、背景、悬停、边框、强调、焦点以及成功／警告／错误／信息状态采用语义颜色。

| 项目 | 规格 |
| --- | --- |
| 间距 | 4、8、12、16、24、32 |
| 控件／卡片／弹窗圆角 | 8／12／16 |
| 普通／主要操作点击高度 | 至少 36／40；触摸密度至少 48 |
| 浅色交互背景 | 悬停 `#f2f3f4`；选中 `#eceef0`；选中控件悬停时保持选中背景 |
| 细边框／关闭开关 | 浅色 `#dedede`／`#e0e0e0`；开启轨道与焦点保持蓝色语义 |
| 开关 | 轨道 36×22；实际点击区域桌面 36×36、触摸密度 48×48 |
| 文字缩放 | 按完整 TextScaler 计算控件最小高度，覆盖 100%、125%、150%、200% |
| 动效 | 状态 120 ms、面板 180 ms、弹窗 240 ms；系统减少动画或无障碍导航时为零 |
| 断点 | 900 以下侧栏收纳，1100 以下工作台覆盖，最小窗口 720×520 |
| 阅读宽度 | 自动模式上限 920；手动拖动范围 520—1100；空间不足时随容器缩小 |

浅色辅助文字 `#626872` 在基础、侧栏、卡片、悬停、选中和用户消息背景中的最低计算对比度为 **4.82:1**；深色 `#c0c5cd` 为 **5.53:1**。设计测试同时检查普通文字、强调按钮及四种状态文字的 4.5:1 对比度目标，以及焦点在各背景上的 3:1 对比度。

## 字体与内容

界面字体角色为 12／13／14／15／16／18／22／26。对话默认 15／24，输入区 16／24，代码字体角色为 13／20；终端和部分紧凑日志使用 12—13 号等宽字。Windows 使用 Segoe UI、微软雅黑回退；macOS 使用系统字体、PingFang SC；Linux 使用 Noto Sans 与 CJK 回退。等宽字体分别使用 Consolas／Cascadia Mono、SF Mono／Menlo、DejaVu Sans Mono，并保留中文和通用等宽回退。

对话 Markdown 标题采用 22／30、18／26、16／24，四至六级标题采用 15／24；随用户正文偏好等比派生。独立文档从 26／32 开始。代码、表格、公式和图表保留独立布局边界。

中文入口为 `lib/l10n/zh.dart`、`lib/l10n/conversation_zh.dart` 和 `lib/l10n/runtime_zh.dart`，分别组织界面、会话与工作台、运行层提示。模型正文、协议键和用户提供的权限说明维持原值。识别模型回答中“推荐”后缀的正则保留在问题组件中。

## 图标注册与资源边界

`DshIcons` 定义 **103 个语义标识**，引用 **87 个唯一 SVG**。`DshGlyph` 根据本地标识选取资源；未知本地标识显示问号轮廓。`DshIcons.*.data` 用于仍接受 `IconData` 的接口，本地标识不映射第三方字体码点。`icon_assets.dart` 的 Lucide 兼容适配使用具名 API。

桌面资源目录保留 **144 个 SVG**，包括业务图标、状态拼接资源及兼容轮廓。动态资源必须覆盖：

- `todo-pending.svg`、`todo-in_progress.svg`、`todo-completed.svg`。
- `web-permission-read-only.svg`、`web-permission-workspace-write.svg`、`web-permission-danger-full-access.svg`。

`cg-*` 图标由代码画布使用，其中备用地图和搜索轮廓继续保留。应用品牌、Windows ICO、macOS AppIcon 和 Linux hicolor 资源与功能图标分开管理。Shad 组件仍依赖 Lucide 字体，因此保留该传递依赖。

检查范围覆盖语义注册表、兼容适配、Dart 静态路径和状态拼接、测试、Rust／Web 代码、工具脚本与发布配置。下列 25 个无消费者的旧别名与保留资源 SHA-256 完全一致，资源集使用对应的唯一文件，消除 32,758 字节重复内容；其余独立轮廓和尺寸变体保留。

| 旧别名 | 保留资源 |
| --- | --- |
| `arrowUp.svg` | `web-IconSendOutline16.svg` |
| `chevronDown.svg` | `web-IconChevronDownOutline14.svg` |
| `chevronRight.svg` | `web-IconChevronRightOutline14.svg` |
| `circleAlert.svg` | `web-IconWarningOutline16.svg` |
| `circlePlus.svg` | `web-IconNewChatOutline16.svg` |
| `composer-permission.svg` | `web-permission-workspace-write.svg` |
| `database.svg` | `web-IconDataOutline16.svg` |
| `download.svg` | `web-IconDownloadOutline16.svg` |
| `ellipsis.svg` | `web-IconEllipsisOutline16.svg` |
| `folderOpen.svg` | `web-IconFolderOpenOutline16.svg` |
| `folderPlus.svg` | `web-IconProjectAddOutline16.svg` |
| `gitBranch.svg` | `web-IconBranchOutline16.svg` |
| `link.svg` | `web-IconLinkOutline16.svg` |
| `loaderCircle.svg` | `web-IconLoadingOutline16.svg` |
| `moon.svg` | `web-IconDarkOutline16.svg` |
| `panelLeft.svg` | `web-IconPanelLeftOutline16.svg` |
| `panelLeftClose.svg` | `web-IconPanelLeftOutline16.svg` |
| `paperclip.svg` | `web-IconPaperclipOutline16.svg` |
| `pencil.svg` | `web-IconEditOutline16.svg` |
| `refreshCw.svg` | `web-IconRefreshOutline16.svg` |
| `rotateCw.svg` | `web-IconRefreshOutline16.svg` |
| `square.svg` | `web-IconStopFill16.svg` |
| `sun.svg` | `web-IconLightOutline16.svg` |
| `trash2.svg` | `web-IconTrashOutline16.svg` |
| `workflow.svg` | `web-IconAgentPresetOutline16.svg` |

## 回归证据

设计组件专项 25 个用例通过，覆盖文字缩放、颜色对比、平台字体、最小点击区、加载防重复激活、外部焦点、减少动画、隐藏指示器停止、全部语义 SVG 和动态状态资源。富文本专项 8 个用例通过，覆盖标题与行距、公式、Mermaid、已完成区块复用、链接更新、分页和 UTF-16 边界。

图标黄金图按 16／20／24 逻辑像素排列，浅色与深色各覆盖 DPR 1.0 和 1.5。截图显式按设备像素比采样：1.0 为 700×760，1.5 为 1050×1140。四张基线均完成图形完整性和边界检查。

| 浅色 | 深色 |
| --- | --- |
| [DPR 1.0](../../apps/desktop_flutter/test/goldens/icons_light_1.0x.png) | [DPR 1.0](../../apps/desktop_flutter/test/goldens/icons_dark_1.0x.png) |
| [DPR 1.5](../../apps/desktop_flutter/test/goldens/icons_light_1.5x.png) | [DPR 1.5](../../apps/desktop_flutter/test/goldens/icons_dark_1.5x.png) |

系统真实字体、原生窗口、输入法、平台图标缓存与进程内存属于桌面实机验收范围，组件测试不替代这些测量。

## 平台应用图标

应用图标统一使用 `packaging/windows/deepseek-app.svg` 母版：黑色鲸形、白色圆角底框、灰色细边及透明外角。Web favicon 与母版逐字节一致；Linux hicolor 安装直接引用该母版。

macOS AppIcon 包含 16、32、64、128、256、512、1024 像素的七个 RGBA PNG；`Contents.json` 的十个 1×／2× 声明均匹配实际像素尺寸，资源集中无多余 PNG。所有尺寸通过矢量路径独立渲染和 4× 抗锯齿采样导出。

Windows ICO 包含 16、20、24、32、40、48、64、128、256 像素九帧。安装资源与 Flutter Runner ICO 的 SHA-256 均为 `9fd43d7fe21e46b74344f6f91cde2fdd4b9590fbe0c290bcc016eee44d55f059`；九帧解码后的 RGBA 像素均与母版同尺寸导出完全一致。

16／20／24／32 像素资源已在浅色与深色底色检查原始尺寸和像素放大图。白色底框、主体及尾部轮廓保持可辨认，无图形越界或透明度丢失；16 像素中的精细眼部细节受像素尺寸限制。macOS 验证覆盖资源结构、尺寸、透明通道与母版一致性，未执行 macOS 实机运行和系统图标缓存验收。


## 错误恢复、代码阅读与初次加载

文件、目录、Git、终端读取和后台任务保留结构化错误对象，读取重试先取消上一代作用域，再校验 Host、会话和目标路径。旧 Host 的迟到响应不能写回当前视图或文档缓存。终端读取恢复不会重放输入、创建或关闭命令；自动读取成功不会清除尚未确认的输入操作结果。

产物、项目任务、子任务、控制面板、计划审批、反馈、附件和导出使用统一错误卡。摘要保持正文尺寸，技术详情默认折叠，复制前脱敏。文件操作、朗读和权限切换的短提示提供明确的“查看详情”动作。提示根据应用环境使用 Material 消息条或 Shad 通知；未知操作结果不提供自动提交重试。

普通代码块支持换行开关，横向阅读模式仍可恢复；复制内容保持原始换行和字符。字号和深浅主题沿用同一代码字体角色。

会话、模型、技能／MCP、知识库和定时任务首次加载显示稳定占位行和明确加载文字。占位行不进入焦点顺序，无持续动画；存在旧数据时刷新保留原内容并显示局部进度。

专项回归覆盖 18 个测试文件、116 个用例，包含新增加载／焦点／文字缩放／原文复制检查，以及读取重试、迟到响应、Host 隔离、未知提交结果和既有生命周期回归。专项校验镜像静态分析为零问题。
