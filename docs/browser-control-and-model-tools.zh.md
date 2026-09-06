# 浏览器控制与模型工具

## 当前能力

侧边栏的网页页面继续使用 iframe 展示网站，供用户手动预览。模型控制使用独立的 Edge、Chrome 或 Chromium 会话，不依赖 iframe 的跨源权限，也不会读取用户日常浏览器配置。

启用 `computer-use.enabled` 后，Host 会注册 `computer_use` 函数工具。内置执行层支持 `start`、`status`、`capture`、`navigate`、`click`、`double_click`、`type`、`scroll`、`list_sessions` 和 `close`。每次操作返回当前 URL、标题、加载状态、滚动位置、视口和焦点元素；URL 与标题分别限制为 8192 和 4096 个字符。需要视觉反馈的操作同时返回 PNG 截图，执行层在 base64 解码前后执行 16 MiB 边界检查，随后再由附件服务验证，模型会收到真正的图像内容块。

浏览器会话使用各自的临时配置目录和 CDP 连接，先按所属应用会话隔离，再按 `sessionId` 隔离；默认 `sessionId` 为 `default`，最多同时运行四个会话。关闭浏览器会话或所属应用会话退役时，会终止对应浏览器进程并清理配置目录。后台回收器独立于模型调用和侧栏刷新检查浏览器进程；意外退出后会移除失效会话、清理配置目录并通知 Host 释放所属会话的存活保护。Host 退出时会清理其余所有会话。

模型工具与侧栏使用同一个 `ComputerUseRuntime`。Host 将应用会话 ID 作为不可由模型覆盖的所有者边界，再在该边界内解析浏览器 `sessionId`；两个应用会话即使使用相同浏览器会话名，也不能列出或读取彼此的页面。侧栏通过受同源请求限制的 `/__dsh-computer-use` 路由执行同一组操作并取得截图，因此人工操作和模型后续操作可以继续使用同一受控页面。

## 接入方式

需要区分两个接口：模型接口负责提出操作，本地浏览器接口负责真正执行操作和返回观察结果。可以继续使用现有模型接口，通过函数工具或 MCP 对接浏览器执行器，不必再购买另一套模型服务。账号、供应商和网关必须支持相应工具调用；使用截图时还需支持图像输入。

OpenAI 官方说明支持为 GPT-6 Astra 提供自有函数工具或 MCP 工具，也支持 `computer` 工具，并推荐以隔离环境中的代码执行完成复杂操作。执行环境仍由应用提供和管理。[Computer use](https://developers.openai.com/api/docs/guides/tools-computer-use)

当前控制流程为：

1. 为当前任务创建独立浏览器会话，使用自己的配置目录与标签页标识。
2. 向模型声明工具名称、参数、操作范围与当前可用状态。
3. 返回当前 URL、标题、页面状态和截图。
4. 执行点击、输入、滚动或导航后重新观察页面，验证操作结果。
5. 在模型调用与浏览器执行之间保留审批、取消、超时和会话边界。

内置执行层使用 Chromium CDP。模型工具与执行器之间保留公开的 `ComputerUseAdapter` 边界，后续可以接入桌面、MCP 或远程设备控制器，而不改变模型侧工具和截图通道。独立执行层不能靠移除 iframe 限制来替代；移除限制也不会让模型自动获得页面状态。

## 设置

| 字段 | 行为 |
| --- | --- |
| `enabled` | 开启后注册执行层和模型工具；关闭时不会探测或启动浏览器。浏览器可执行文件延迟到元数据检查或首次启动受控会话时探测。 |
| `adapter` | `auto`、`native-browser` 或 `command`。`auto` 在已有非空命令时沿用旧命令适配，否则使用内置浏览器。 |
| `browserExecutable` | 可选的 Edge、Chrome 或 Chromium 可执行文件；留空时自动查找。 |
| `browserHeadless` | 默认开启。关闭后受控浏览器显示为独立窗口。 |
| `maxBrowserSessions` | 同时运行的独立浏览器会话数，范围 1–16，默认 4。 |
| `command` | 旧版外部命令适配器路径；仅 `command` 模式或带非空命令的 `auto` 模式使用。 |
| `timeoutSeconds` | 启动与单次工具操作的上限，范围 5–300 秒。 |

找不到兼容浏览器或显式路径失效时，Host 仍会正常启动，设置页保持可用；`/__dsh-computer-use/meta` 返回 `enabled: true`、`available: false` 以及结构化错误，首次 `start`／`navigate` 也返回同一明确错误。显式配置的文件不存在时不会静默回落到其他浏览器。

侧栏调用同源接口时，以应用会话作为所有者，以工作台内的浏览器标签作为受控会话。`POST /__dsh-computer-use/meta` 接收 `ownerSessionId` 并返回可用适配器；`POST /__dsh-computer-use/action` 接收 `ownerSessionId`、`browserSessionId` 和工具动作参数。操作结果的 `screenshot` 为短期响应数据，前端不应持久化其中的 base64；模型路径会改存为受校验的附件引用。

## 如何让模型知道

工具由启用后的运行时实际注册并随请求发送，而不是只在提示词中写“可以控制浏览器”。工具描述解释受控对象、输入格式、状态读取方式和执行后的检查方法。浏览器暂不可用时工具 schema 仍保持注册，使 Host 与设置页可以正常启动；元数据会明确返回 `available: false`，`start`／`navigate` 返回同一结构化诊断，模型收到该结果后不应继续重试操作，直到用户修正配置并重启 Host。

内置浏览器截图已经接入图像附件通道，工具调用与图像结果保持关联。外部命令也可以在 JSON 的 `screenshot` 字段中返回 `{ "base64", "mediaType", "name" }`；Host 会移除原始 base64、校验图像并用附件引用替换它，避免把大段图片编码当作普通文本发送给模型。

外部命令协议继续使用原有的两个 argv：`<action> <json>`。Host 在 JSON 中写入不可由模型覆盖的 `dshProtocolVersion: 2`、`ownerId` 和 `clientSessionId`，并把传给适配器的 `sessionId` 变为由所有者和客户端会话名派生的稳定不透明 ID。现有按 `sessionId` 保存状态、并忽略未知 JSON 字段的适配器无需改变即可获得跨应用会话隔离；新版适配器可以读取 `ownerId` 做远端租户隔离，并用 `clientSessionId` 显示原始名称。没有应用会话所有者的旧版 Host 直连调用仍保留原 `sessionId`。命令进程使用无 shell 的有界执行器：stdout 上限 24 MiB、stderr 上限 1 MiB，超时沿用 Computer Use 操作上限；超限、超时、取消和管道错误都会终止并回收子进程，再返回结构化错误。

Host 只把当前所有者已观察到的活动会话保留在 `list_sessions` 结果中；适配器返回的其他所有者或未知会话会被丢弃。外部命令成功执行 `start`、`navigate`、观察或输入操作后，Host 会为所属应用会话保留存活保护；`close` 成功后释放。适配器也可以返回 `sessionActive: true/false` 显式更新状态。所属会话销毁时 Host 逐个调用 `close`；关闭失败的会话仍保留为活动状态，以便后续清理重试，而不会被误判为已经退出。

## 运行依赖

Rust 主程序、普通会话与内置 CDP 控制均不依赖 Node.js。内置执行层需要本机安装 Edge、Chrome 或 Chromium。其他 Playwright、MCP 或远程适配器的依赖由各自实现决定；现有 PTC 代码模式仍需要兼容的 Node.js。

## WebView 与远程桌面

原生 WebView 可以作为浏览器显示与执行容器。Windows WebView2 提供 CDP 调用接口，应用可以基于它实现浏览器状态读取、输入操作及截图，再接到模型工具层。网页中的 iframe 与应用原生 WebView 是不同的集成方式，不能将现有 iframe 预览直接视为原生浏览器控制器。[Microsoft WebView2 CDP](https://learn.microsoft.com/en-us/microsoft-edge/webview2/how-to/chromium-devtools-protocol)

远程桌面也可以显示在网页中。noVNC 提供可集成的浏览器 VNC 客户端；Apache Guacamole 提供支持 RDP、VNC 和 SSH 的网页网关及集成 API。这些项目说明网页远控已有成熟实现，但仍需由应用把模型操作接到相同的远程会话。[noVNC](https://novnc.com/info.html)、[Apache Guacamole](https://guacamole.apache.org/)

Anthropic 的 Computer Use Demo 已提供在同一网页中展示 Agent 对话与受控 Linux 桌面的实例，桌面使用 X11 与 VNC。它证明对话和可交互桌面能够组合在同一界面；具体客户端、模型工具与远程传输层仍需要各自适配。[Computer Use Demo](https://github.com/anthropics/claude-quickstarts/tree/main/computer-use-demo)

网易 UU 远程已公开 CLI 和配套 Skill，支持设备查询、连接、状态及远程终端会话管理，要求对应客户端运行。其公开 CLI 文档未列出桌面截图、坐标点击、按键注入、画面流或 WebView 嵌入 API，因此 CLI 接通不能作为完成桌面 Computer Use 的证明。[UU 远程 CLI](https://uuyc.163.com/help/cli.html)

远程桌面画面通常是像素流。嵌入网页能够读取的 DOM 属于播放器界面，不会包含远端浏览器或桌面程序的控件结构；需要通过截图识别和坐标映射操作，或在远端增加能返回应用状态的执行器。远程延迟、缩放和人工接管都必须纳入操作后验证。

独立浏览器会话的工具调用与截图观察循环已经完成。UU CLI 可作为后续连接管理适配器；UU 的桌面画面和输入控制在取得并验证相应接口前保持未接入状态。
