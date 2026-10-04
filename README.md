# DeepSeek Harness Rust

[简体中文](README.md) | [English](README.en.md)

DeepSeek Harness Rust 是 DeepSeek Harness Host 的 Rust 迁移实现。它使用正式 `dsh web` 入口托管浏览器应用，并保留会话、工具、插件、存储和RPC兼容边界。

> 当前版本仍是预发布版本。功能状态以本README的兼容矩阵和GitHub Release说明为准。

当前发布线：[`0.1.3-alpha.38-r6`](https://github.com/qiu7824/deepseek-harness-rs/releases/tag/v0.1.3-alpha.38-r6)，完整变更见 [alpha.38 发布说明](release/notes/v0.1.3-alpha.38.md)。源码版本与安装包身份可通过 `--build-info` 和包内构建清单核对。

文件隔离、执行回执、手动技能版本与消息恢复的实现及验证范围见[可靠性对照记录](docs/hermes-agent-reliability-review-20260928.zh.md)。该记录保留 alpha.37 的历史测试，任务验收与样本验证机制已在 alpha.38 退役。

跨版本适配与未完成项见 [v0.1.7-rc.2 评估](docs/upstream-v0.1.7-rc.2-evaluation.zh.md)及[更新计划](docs/plans/更新计划.md)。

官方 [dsh-v0.2.1-alpha.1 源码评估](docs/upstream-dsh-v0.2.1-alpha.1-evaluation.zh.md)列明已覆盖范围、协议差异和 P0/P1 验收计划；实验 Claude Code Mods 与上游 Web 适配仍待分项开发。[Windows Flutter 本轮源码修复](docs/windows-flutter-regressions-2026-10-04.zh.md)已通过 Windows 回归，实机字体和模型服务复验待完成；现有 r6 安装包仍对应原始标签源码。

Rust 版本独立维护分页、超长对话窗口、上下文跳转、原生启动器和主题效果。版本号标识 Rust 发布线，不表示与 Node 版本逐项或磁盘格式完全相同。

双向分页、阅读锚点和实时消息缓冲的设计见 [Rust 对话滚动与分页](docs/rust-conversation-scrolling.zh.md)。

## 0.1.3-alpha.38 近期变化

- **Flutter 桌面体验**：统一深浅主题、文字缩放与语义图标，增加命令搜索、焦点和快捷键配置；保留工作区草稿、历史阅读位置，隐藏预览按需回收。模型与思考等级共用紧凑入口，分层菜单即时显示缓存并保留最后一次切换意图。
- **桌面交互修复**：修复关闭窗口崩溃，关闭前保存草稿，后台 Host 与任务继续运行；账号入口、插件主页面和侧栏菜单重新整理。回合结束后在回复下方显示产物卡，可直接预览和操作文件。
- **模型协议兼容**：Devin / Claude 请求使用兼容的工具输入结构，修复根级组合结构被拒绝的问题；本地继续按原始工具规则校验参数，Devin 错误保留实际协议错误码。
- **Office 与文件整理**：`office_read` 原生读取 DOCX 段落、表格和 XLSX 单元格，`office_write` 直接生成真实 DOCX/XLSX；覆盖与文件整理需人工批准。结构检查不替代内容核对或视觉验收。
- **图片生成与编辑**：生成与编辑结果以当前会话正式图片附件返回，可继续编辑或识图。明确仅支持文本的主模型接收图片引用和文字说明；视觉模型仍可读取原图。
- **文件隔离与执行**：按会话隔离私有目录、附件和受管临时文件；原生沙箱支持精确读取根与 Windows 长路径。PowerShell 在受管执行副本中正确初始化当前目录，源项目保持只读保护。人工审批等待不消耗执行预算，重复环境启动失败形成有界停止。
- **执行流程简化**：退役项目任务板、任务契约、任务验收接口、完成门禁和自动验收续接；目标、计划、后台作业及定时任务继续独立运行，技能版本由用户手动管理。

桌面实际验证范围见[平台矩阵](docs/desktop-platforms.zh.md)与[体验升级记录](docs/desktop-fidelity/flutter-upgrade-2026-09-29.md)。旧版变更保留在 [alpha.36](release/notes/v0.1.3-alpha.36.md)、[alpha.31](release/notes/v0.1.3-alpha.31.md)及 [alpha.22](release/notes/v0.1.3-alpha.22.md) 发布说明中。

### UU 网页画面与接管

1. 在“设置 → 插件 → Computer Use 与远程设备”选择“UU 远程桌面”，并绑定当前账号下的设备。
2. 在会话右上角打开“显示工作台”，点击 `+`，选择 `Computer Use`，即可在网页中连接和查看 UU 画面。
3. 需要系统验证时保持人工接管，在画面中自行输入；可放大工作台或画面，智能体在此期间暂停操作。
4. 完成后点击“交还智能体”。如果原任务已经停止，再发送继续指令；交还控制权不会擅自重做已结束的任务。

`uu_terminal` 提供远程命令行，桌面画面由上述控制面板提供；终端错误文本本身不是桌面播放器。

## 下载

Windows x86_64 完整包：

| 版本 | 安装包 | 便携包 |
|---|---|---|
| Flutter 桌面版 | [下载 EXE](https://github.com/qiu7824/deepseek-harness-rs/releases/download/v0.1.3-alpha.38-r6/deepseek-harness-rs-v0.1.3-alpha.38-windows-x86_64-flutter-setup.exe) | [下载 ZIP](https://github.com/qiu7824/deepseek-harness-rs/releases/download/v0.1.3-alpha.38-r6/deepseek-harness-rs-v0.1.3-alpha.38-windows-x86_64-flutter-portable.zip) |
| Web 核心版 | [下载 EXE](https://github.com/qiu7824/deepseek-harness-rs/releases/download/v0.1.3-alpha.38-r6/deepseek-harness-rs-v0.1.3-alpha.38-windows-x86_64-core-setup.exe) | [下载 ZIP](https://github.com/qiu7824/deepseek-harness-rs/releases/download/v0.1.3-alpha.38-r6/deepseek-harness-rs-v0.1.3-alpha.38-windows-x86_64-core-portable.zip) |

Linux、macOS 及 SHA-256 校验文件见 [alpha.38 发布页](https://github.com/qiu7824/deepseek-harness-rs/releases/tag/v0.1.3-alpha.38-r6)的 **Assets**；请以实际上传的资产为准。源码 ZIP 不含编译后的运行程序。

完整包包含 Rust Host、`web/dist`、`config/agent-presets`、随附 Web 插件、Node 与 ripgrep 运行时及安全说明；Flutter 桌面包还包含客户端、Flutter 运行库和资源，并在 `host` 子目录中附带完整 Host。保留整个安装或解压目录，避免缺失资源与随附运行时。

源码构建或自定义安装需确保文件搜索工具能找到 [ripgrep（rg）](https://github.com/BurntSushi/ripgrep#installation)。Linux 的系统沙箱依赖见下方说明。


## 快速启动

Windows 使用 Flutter 桌面包时，解压后运行 `dsh_desktop.exe`；客户端会启动随附 Host，或连接已运行的本机服务。关闭桌面窗口会保留后台服务及其任务。

使用 Web 核心包时，解压后运行 ZSUI 原生启动器，再打开浏览器界面：

```text
Windows: dsh-launcher.exe
Linux/macOS: ./dsh-launcher
```

Linux 的受限 Shell 与原生终端使用系统 `bubblewrap` 沙箱；DEB 包声明该依赖，使用便携包时请通过发行版包管理器安装 `bubblewrap`。缺少沙箱或系统不允许创建沙箱时，受限执行会明确失败。
启用用户命名空间限制的 Ubuntu 系统可能需要管理员配置发行版推荐的 [bwrap AppArmor 规则](https://discourse.ubuntu.com/t/understanding-apparmor-user-namespace-restriction/58007)，程序不会自动修改系统防护策略。

默认地址：

```text
http://127.0.0.1:58080/
```

启动器由固定 commit 的 ZSUI 构建，不依赖 CMD、PowerShell、WebView 或额外运行时；负责启动、停止、重启正式 `deepseek-harness-rs web` 进程以及打开网页、日志目录。Windows 安装器和启动器按系统 UI 语言自动显示简体中文或英文。 全新安装默认位于 `D:\Program Files (x86)\DeepSeek Harness-rs\<variant>`，升级沿用原安装位置；默认位置不可用时需选择其他目录。

发行结构为 **Web 核心版与 Flutter 桌面客户端**，共用 Rust Host 及 HTTP/WebSocket 协议。Web 仅提供 `core` 包；模型目录与连接管理在核心版设置中提供，不按模型另设安装版本。Flutter 源码位于 [`apps/desktop_flutter`](apps/desktop_flutter)，各平台的构建与验收状态见[桌面平台矩阵](docs/desktop-platforms.zh.md)。

普通会话、原生工具和 Web 界面由 Rust 核心提供。完整发行包随附用于 JavaScript／TypeScript 代码模式的 Node；源码构建与部分外部工具仍需配置相应运行环境。设置中的运行环境页显示实际路径、版本及能力检测结果。

账号登录后自动同步可用模型和能力；模型管理中的显示开关保留跨刷新、重启的用户偏好，隐藏模型不会删除已有会话。代码图谱按需索引当前工作区，提供局部调用关系、文件依赖与源码定位；打开会话不会自动扫描整个项目，推断关系和未覆盖范围会明确标示。

## 命令行入口

```bash
./deepseek-harness-rs web
```

如需指定端口：

```bash
./deepseek-harness-rs web --port 58080
```

## 数据、Profile与工作区

默认数据根：

```text
Windows: %LOCALAPPDATA%\DeepSeek Harness
Linux/macOS: 由平台数据目录与DSH_HOME决定
```

可通过 `DSH_HOME` 指定数据根，也可在“设置 → 目录与运行环境”中选择目录并重启应用。迁移会先复制并逐文件校验，成功后切换目录，保留原数据；失败时恢复原目录设置并显示原因。工作区仍是会话的项目目录来源，项目文件不会随应用数据迁移。

Profile插件位于：

```text
<DSH_HOME>/profiles/<profile>/node_modules
```

正式会话、附件、缓存、设置和插件库存都属于用户数据，不应随升级包覆盖或清理。

投影缓存采用逐记录 v5 格式，并兼容可解码的 v3/v4 数据；坏缓存先备份再重建，权威数据读取异常保持明确失败。详见[存储兼容与恢复](docs/storage-compatibility.md)。

## Provider与协议

在“设置 → 模型”中配置 API Key 或完成账号登录；凭据保存在本机，账号令牌支持续期和退出登录。模型条目提供显示开关，推理等级优先读取提供商元数据。

| 协议/API | 状态 |
|---|---|
| DeepSeek/OpenAI-compatible Chat Completions | 已接入正式Rust适配器 |
| OpenAI Responses | 已提供显式`api: openai-responses`入口；工具、推理、图片、usage和SSE已覆盖fixture，真实Provider仍需使用者配置后验收 |
| Azure OpenAI Responses | 未完成正式Provider闭环 |
| OpenAI Codex Responses | 账号设备码登录、令牌续期与 Responses 路由；使用者在设置中完成账号授权 |
| Anthropic Messages | 原生请求与流式文本、工具、图片、thinking、usage 转换，使用 API Key；Claude 订阅由官方 Claude Code 子智能体使用 |
| Bedrock Converse Stream | 未完成 |

账号额度与单次请求的缓存 token 属于不同统计。缓存命中率仅使用响应明确披露的缓存读数；没有数据时显示“缓存数据未提供”，明确返回 0 时保留 0。ChatGPT / Codex 账号服务不直接套用公开 Responses API 的显式缓存断点参数。稳定缓存键有助于复用，但不保证命中；正式账号通道的长前缀连续请求已成功，实测缓存读取仍为 0。

完整证据见[`docs/protocol-matrix.md`](docs/protocol-matrix.md)。存在文件名或crate不代表生产能力已完成。

## 技能、MCP 与记忆

“设置 → 技能与 MCP”管理技能文件和 MCP 服务器，支持启停、编辑和连接测试。“记忆与上下文”支持检索、启停和维护已知错误经验。详见[技能、工具与经验记忆](docs/learning-and-capabilities.zh.md)。

## Web插件

纯Web插件无需Node、npm或pnpm。Rust Host负责校验、发现、登记和静态服务预构建的`client.js`。

### 安装第三方插件

Rust版本只直接安装符合以下结构的纯Web插件：

```text
package.json
lib/client.js
```

`package.json`必须声明Web客户端导出。插件安装来源必须固定到完整40位Git commit，不能使用分支名、tag或可变默认分支：

GitHub插件安装必须固定完整40位commit SHA：

```powershell
.\dsh.exe plugin --profile web add github:owner/repository#0123456789abcdef0123456789abcdef01234567
```

安装后重启`dsh web`，然后在“设置 → 插件”中确认插件已启用。管理命令：

```powershell
.\dsh.exe plugin --profile web list
.\dsh.exe plugin --profile web remove package-name
```

升级插件时，先审计新commit，再卸载旧版本并使用新commit SHA重新安装。Rust安装器会校验包名、入口路径、符号链接、文件大小和目录越界；校验失败时拒绝安装。

兼容范围：

- 纯Web插件：支持；
- Web + Node Host插件：只可能加载独立的Web部分，Node Host部分不会运行；
- 纯Node Host/native插件：Rust Host不执行。

如果社区插件依赖`require()`、npm生命周期脚本、Node服务、native addon或Host侧JS，它不能直接装进纯Rust进程。应使用插件提供的纯Web构建，或者把Host部分作为独立sidecar程序运行。

Web插件与主应用同源运行，拥有页面级JavaScript能力。只安装来源可信、固定commit并完成审计的插件。

随附插件：

- `dsh-voice-input`：浏览器语音输入；
- `dsh-context-jump`：按完整用户消息索引定位，按需加载目标附近的有界历史，支持悬停预览和键盘导航；
- `dsh-better-sidebar`：可调宽度的工作台、工作区文件、终端与网页预览，融入原生对话界面；
- `dsh-sidebar-workbench-suite`：Markdown、源码、结构化数据查看器、后台任务及共用的浏览器／桌面控制面板。

## 能力状态

| 能力 | 状态 |
|---|---|
| 会话、持久化、历史分页 | 已实现强类型 `SessionSeq` / `SessionLogOffset`、显式有界读取，并保持 v0 JSONL/Zstd `seedLength` 兼容 |
| DeepSeek长流、reasoning、tool、图片、usage | 已实现，发布后仍需按真实Provider复验 |
| 子智能体 | continuable 直接父子可双向使用 `send_message({ agent_id, message })`；外部Codex/Claude Code提供方未默认安装 |
| 网页抓取 | 已实现 Rust 原生 `web_fetch`，只允许公开 HTTP(S)，并限制重定向、DNS/IP、超时、体积和取消 |
| 模型发现 | 服务端可安全复用 Profile headers 而不向浏览器返回凭据；模型候选支持搜索和仅对可见结果全选 |
| 工作流 | 引擎继续保留；PTC/code 预设刻意不提供通用 `workflow` 工具，但保留 `run_code` 和 Ralph |
| 终端 | 已实现持久终端、输入、关闭和回收；UU 远程终端另受被控端登录、解锁及终端能力约束 |
| UU 桌面控制 | 已实现独立控制进程、动态客户端识别、实时画面与输入通道；实机已验证连接和锁屏画面，应用输入仍需解锁后验收 |
| 画面批注 | 当前截图、文字及归一化坐标进入持久用户消息，草稿按会话隔离 |
| MCP | 已接入正式设置页，支持 stdio/HTTP、工具注册、启停与连接测试 |
| LSP | 底层registry/tool实现；正式Host尚未组合 |
| ACP | 协议入口存在；真实prompt/cancel回归尚未封板 |


侧栏与上游插件的兼容范围见[侧栏能力清单](docs/sidebar-capabilities.md)；浏览器执行器、模型工具及 UU 远程接入方式见[浏览器控制说明](docs/browser-control-and-model-tools.zh.md)。

## 构建

工具链固定为Rust 1.97.1：

```bash
cargo build --release -p dsh-host-cli --bin dsh -p dsh-launcher --bin dsh-launcher
```

基础门禁：

```bash
cargo fmt --all -- --check
python tools/verify_product_surface.py
python -m unittest discover -s tools/tests -p "test_memory_*.py" -v
python tools/validate_memory_baseline.py --report docs/memory/production-baseline.jsonl --markdown docs/memory/production-baseline.md
cargo test -p dsh-llm-deepseek --all-targets
cargo test -p dsh-host --lib -- --test-threads=1
cargo test -p dsh-host-cli --lib -- --test-threads=1
```

## 安全边界

- 远程明文HTTP默认拒绝，只允许受控loopback测试；
- 凭据只通过credential service按需解析，不写入源码、测试录制或Release；
- 插件入口、包名、符号链接、大小和路径越界均fail-closed；
- Windows工具通过AppContainer和批准策略运行；
- Release只包含运行资源，不包含源码测试、会话、缓存或凭据。

更多插件边界见`PLUGIN_SECURITY.md`。

## 已知限制

- 上游 v0.2.0-rc.1 Web 客户端已完成构建与 Remote 契约清单，整体换装仍在后续适配阶段，当前发行使用现有 Web 界面；进度见[Web 换装计划](docs/plans/web-upstream-v0.2.0-rebase.zh.md)；
- 一键自动更新、完整首次引导，以及原生输入法、读屏和跨平台桌面实机验收仍有未完成项；平台状态见[桌面平台矩阵](docs/desktop-platforms.zh.md)；
- 通用pi-ai provider catalog尚未完整移植；
- UU 实机验证覆盖连接与 Windows 锁屏画面，应用内输入和远程终端命令仍需被控端解锁后验收；
- 多账号授权、切换与恢复已通过模拟账号回归，两个真实账号之间的完整授权切换尚未验收；账号服务的高缓存命中率也未得到实测证实；
- LSP仍为库级能力，尚未接入正式Host配置；
- ACP真实prompt/cancel与Python SDK真实turn仍有回归；
- 对话导航使用用户消息索引和定点历史页，完整会话数据保留在 Host；
- Linux和macOS资产只有在GitHub Actions矩阵全部成功后才视为发布完成。

## 许可证

MIT，见[`LICENSE`](LICENSE)。第三方声明见[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)。
