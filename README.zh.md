# DeepSeek Harness Rust

[简体中文](README.md) | [English](README.en.md)

DeepSeek Harness Rust 是 DeepSeek Harness Host 的 Rust 迁移实现。它使用正式 `dsh web` 入口托管浏览器应用，并保留会话、工具、插件、存储和RPC兼容边界。

> 当前版本仍是预发布版本。功能状态以本README的兼容矩阵和GitHub Release说明为准。

当前发布线：`0.1.3-alpha.13`。

完整更新见 [alpha.13 发布说明](release/notes/v0.1.3-alpha.13.md)。

Rust 版本独立维护分页、超长对话窗口、上下文跳转、原生启动器和主题效果。版本号标识 Rust 发布线，不表示与 Node 版本逐项或磁盘格式完全相同。

双向分页、阅读锚点和实时消息缓冲的设计见 [Rust 对话滚动与分页](docs/rust-conversation-scrolling.zh.md)。

## 0.1.3-alpha.13 能力更新

- **会话与反馈**：永久删除包含所属子智能体历史，保留独立分支；赞踩先确认，可选分类持久化，失败保留草稿。
- **通用附件与导入**：支持普通文件选择、粘贴和拖放；官方 V0/V1/V2/V3 日志及带图片 ZIP 经校验后导入，原件保留。
- **Agent Teams**：显式启用具名成员、持久消息、共享任务版本校验及团队面板。
- **网络与协议**：Host HTTP 客户端共用代理策略；Responses Lite 按模型声明启用，保留端点与账号隔离。
- **资源版本**：前端总资源标识随发布版本更新，源码、dist、模块摘要和二进制身份联合核验。

配置和边界见[会话、附件、团队与网络](docs/session-files-teams-and-network.zh.md)。


- **Windows 执行可靠性**：祖先目录权限更新不再遍历子目录；沙箱准备、实际命令和清理分别计时；取消与超时回收所属进程，保留部分输出和具体错误原因。
- **对话滚动与统计**：短对话和折叠后的内容不再显示多余的“返回底部”按钮；发送后恢复跟随最新回复；修复 V3 系统消息、上下文和缓存统计分类。
- **系统目录选择**：系统文件夹窗口返回的路径正确回填工作区表单，确认后同步工作区；取消不创建目录或工作区，选择器错误保留可见提示。
- **文件阅读与交付**：代码支持行号定位、换行及阅读位置恢复；Markdown、HTML 和 PDF 保留各自滚动位置，PDF 支持页码与缩放恢复；产物按回合关联并通过 `present` 明确交付。
- **账号与模型管理**：验证码登录遇到临时网络错误时保留有效请求并重试；补齐官方 Flash 能力目录和动态系统提示词；模型连接支持搜索、筛选、批量可见性和草稿保护。
- **任务控制与诊断**：明确暂停目标后，模型不能自行恢复；重复执行错误按错误码识别，诊断记录关联具体会话和工具调用；MCP 新配置失败时保留原有可用连接与工具。
- **UU 兼容**：重新发现升级后的客户端安装位置，支持新版终端兼容路径并保持连接归属隔离；保留网页画面、批注、连接复用和人工接管。
- **发布一致性**：核心提供 `--build-info`；打包核对版本、源码提交和修改状态；Windows、Linux、macOS 的安装包与便携包均由同一提交验收，附件提供 SHA-256 校验和。

完整变更见 [alpha.13 发布说明](release/notes/v0.1.3-alpha.13.md)，协议适配范围见 [rc.1 能力评估](docs/upstream-v0.1.5-rc.1-evaluation.zh.md)。

### UU 网页画面与接管

1. 在“设置 → 插件 → Computer Use 与远程设备”选择“UU 远程桌面”，并绑定当前账号下的设备。
2. 在会话右上角打开“显示工作台”，点击 `+`，选择 `Computer Use`，即可在网页中连接和查看 UU 画面。
3. 需要系统验证时保持人工接管，在画面中自行输入；可放大工作台或画面，智能体在此期间暂停操作。
4. 完成后点击“交还智能体”。如果原任务已经停止，再发送继续指令；交还控制权不会擅自重做已结束的任务。

`uu_terminal` 提供远程命令行，桌面画面由上述控制面板提供；终端错误文本本身不是桌面播放器。

## 下载

从 [GitHub Releases](https://github.com/qiu7824/deepseek-harness-rs/releases/tag/v0.1.3-alpha.13) 下载对应平台的完整包：

- `deepseek-harness-rs-v0.1.3-alpha.13-windows-x86_64-{core,skin,free}-portable.zip`
- `deepseek-harness-rs-v0.1.3-alpha.13-linux-x86_64-{core,skin,free}-portable.tar.gz`
- `deepseek-harness-rs-v0.1.3-alpha.13-macos-{x86_64,aarch64}-{core,skin,free}-portable.tar.gz`
- 对应的 Windows `setup.exe`、Linux `.deb` 与 macOS `.pkg` 安装包

完整包包含二进制、`web/dist`、`config/agent-presets`、随附Web插件和安全说明。不要只复制二进制后再期待完整Web界面和随附插件可用。

## 快速启动

默认下载 `core` 包；它不包含扩展皮肤。解压后直接运行三平台统一的 ZSUI 原生启动器：

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

需要扩展皮肤时，另行下载 `skin` 包并运行其中的 `deepseek-harness-rs-skin`（Windows 为 `.exe`）；它只把皮肤资产安装到同目录的 `web/dist/skins`，默认 `core` 包始终不携带皮肤资源。

`free` 包与 `core` 使用同一套正式运行时和 Web 界面，只预置通过发布检查的 OpenCode Zen 免费模型。检查[官方模型目录](https://opencode.ai/zen/v1/models)中的精确 ID、官方输入／输出／缓存读取价格、匿名流式推理、工具调用与工具结果续接，并将最近 24 小时的验证证据绑定到包内运行时校验和。`free-model-verification.json` 列出各候选的实际结果；设置中的免费模型页可刷新目录、重新检测和添加已通过的模型。免费包不包含凭据或皮肤载荷。

可选的 `free` 版按平台与构建执行匿名模型验收，仅在通过后发布。是否提供 `free` 包，以该次 GitHub Release 的实际资产和包内 `free-model-verification.json` 为准；`core` 和 `skin` 独立验收。

普通会话、原生工具和 Web 界面由 Rust 核心提供。JavaScript／TypeScript 代码模式及部分外部工具需要单独配置 Node；设置中的运行环境页显示实际路径、版本及能力检测结果。

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

- 通用pi-ai provider catalog尚未完整移植；
- UU 实机验证覆盖连接与 Windows 锁屏画面，应用内输入和远程终端命令仍需被控端解锁后验收；
- 多账号授权、切换与恢复已通过模拟账号回归，两个真实账号之间的完整授权切换尚未验收；账号服务的高缓存命中率也未得到实测证实；
- LSP仍为库级能力，尚未接入正式Host配置；
- ACP真实prompt/cancel与Python SDK真实turn仍有回归；
- 对话导航使用用户消息索引和定点历史页，完整会话数据保留在 Host；
- Linux和macOS资产只有在GitHub Actions矩阵全部成功后才视为发布完成。

## 许可证

MIT，见[`LICENSE`](LICENSE)。第三方声明见[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)。
