# DeepSeek Harness Rust

DeepSeek Harness Rust 是 DeepSeek Harness Host 的 Rust 迁移实现。它使用正式 `dsh web` 入口托管浏览器应用，并保留会话、工具、插件、存储和RPC兼容边界。

> 当前版本仍是预发布版本。功能状态以本README的兼容矩阵和GitHub Release说明为准。

当前发布线：`0.1.3-alpha.5`。

Rust 版本独立维护分页、超长对话窗口、上下文跳转、原生启动器和主题效果。版本号标识 Rust 发布线，不表示与 Node 版本逐项或磁盘格式完全相同。

## 下载

从 [GitHub Releases](https://github.com/qiu7824/deepseek-harness-rs/releases) 下载对应平台的完整包：

- `deepseek-harness-rs-v0.1.3-alpha.5-windows-x86_64-{core,skin,free}-portable.zip`
- `deepseek-harness-rs-v0.1.3-alpha.5-linux-x86_64-{core,skin,free}-portable.tar.gz`
- `deepseek-harness-rs-v0.1.3-alpha.5-macos-{x86_64,aarch64}-{core,skin,free}-portable.tar.gz`
- 对应的 Windows `setup.exe`、Linux `.deb` 与 macOS `.pkg` 安装包

完整包包含二进制、`web/dist`、`config/agent-presets`、随附Web插件和安全说明。不要只复制二进制后再期待完整Web界面和随附插件可用。

## Windows快速启动

默认下载 `core` 包；它不包含扩展皮肤。解压后直接运行三平台统一的 ZSUI 原生启动器：

```text
Windows: dsh-launcher.exe
Linux/macOS: ./dsh-launcher
```

默认地址：

```text
http://127.0.0.1:58080/
```

启动器由固定 commit 的 ZSUI 构建，不依赖 CMD、PowerShell、WebView 或额外运行时；负责启动、停止、重启正式 `deepseek-harness-rs web` 进程以及打开网页、日志目录。Windows 安装器和启动器按系统 UI 语言自动显示简体中文或英文。 全新安装默认位于 `D:\Program Files (x86)\DeepSeek Harness-rs\<variant>`，升级沿用原安装位置；默认位置不可用时需选择其他目录。

需要扩展皮肤时，另行下载 `skin` 包并运行其中的 `deepseek-harness-rs-skin`（Windows 为 `.exe`）；它只把皮肤资产安装到同目录的 `web/dist/skins`，默认 `core` 包始终不携带皮肤资源。

`free` 包与 `core` 使用同一套正式运行时和 Web 界面，只预置通过发布检查的 OpenCode Zen 免费模型。检查[官方模型目录](https://opencode.ai/zen/v1/models)中的精确 ID、官方输入／输出／缓存读取价格、匿名流式推理、工具调用与工具结果续接，并将最近 24 小时的验证证据绑定到包内运行时校验和。`free-model-verification.json` 列出各候选的实际结果；设置中的免费模型页可刷新目录、重新检测和添加已通过的模型。免费包不包含凭据或皮肤载荷。

普通会话、原生工具和 Web 界面由 Rust 核心提供。JavaScript／TypeScript 代码模式及部分外部工具需要单独配置 Node；设置中的运行环境页显示实际路径、版本及能力检测结果。

账号登录后自动同步可用模型和能力；模型管理中的显示开关保留跨刷新、重启的用户偏好，隐藏模型不会删除已有会话。代码图谱自动索引当前工作区，提供局部调用关系、文件依赖与源码定位；推断关系和未覆盖范围会明确标示。

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
- `dsh-composer-expand`：输入框展开/收起；
- `dsh-context-jump`：参考 Codex 的左侧细轨道显示对话翻页节点；悬停显示标题，点击定位，顶部/底部按钮与 `Alt+↑/↓` 可快速导航，且不占用原生会话顶部菜单；
- `dsh-web-preview-rs`：Rust Host 原生工作区预览，提供会话隔离的文件树、Markdown/源码、图片、音视频、PDF、隔离HTML站点、元素/文本批注回填及拖放落盘。文件访问固定在当前Session关联的工作区内，拒绝目录穿越、符号链接逃逸、敏感目录和超大文件；站点预览使用启动期随机令牌与独立Origin。项目运行只接受Host探测出的固定命令，通过一次性60秒challenge二次确认，并强制采用完整WorkspaceWrite OS沙箱、凭据清洗、受管进程树和有界日志。

## 能力状态

| 能力 | 状态 |
|---|---|
| 会话、持久化、历史分页 | 已实现强类型 `SessionSeq` / `SessionLogOffset`、显式有界读取，并保持 v0 JSONL/Zstd `seedLength` 兼容 |
| DeepSeek长流、reasoning、tool、图片、usage | 已实现，发布后仍需按真实Provider复验 |
| 子智能体 | continuable 直接父子可双向使用 `send_message({ agent_id, message })`；外部Codex/Claude Code提供方未默认安装 |
| 网页抓取 | 已实现 Rust 原生 `web_fetch`，只允许公开 HTTP(S)，并限制重定向、DNS/IP、超时、体积和取消 |
| 模型发现 | 服务端可安全复用 Profile headers 而不向浏览器返回凭据；模型候选支持搜索和仅对可见结果全选 |
| 工作流 | 引擎继续保留；PTC/code 预设刻意不提供通用 `workflow` 工具，但保留 `run_code` 和 Ralph |
| 终端 | 已实现持久终端、输入、关闭和回收 |
| MCP | 已接入正式设置页，支持 stdio/HTTP、工具注册、启停与连接测试 |
| LSP | 底层registry/tool实现；正式Host尚未组合 |
| ACP | 协议入口存在；真实prompt/cancel回归尚未封板 |

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
- LSP仍为库级能力，尚未接入正式Host配置；
- ACP真实prompt/cancel与Python SDK真实turn仍有回归；
- `dsh-context-jump`第一版基于已渲染稳定节点，Turn/Step完整目录需后续由正式slot暴露timeline；
- Linux和macOS资产只有在GitHub Actions矩阵全部成功后才视为发布完成。

## 许可证

MIT，见[`LICENSE`](LICENSE)。第三方声明见[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)。
