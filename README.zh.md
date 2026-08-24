# DeepSeek Harness Rust

DeepSeek Harness Rust 是 DeepSeek Harness Host 的 Rust 迁移实现。它使用正式 `dsh web` 入口托管浏览器应用，并保留会话、工具、插件、存储和RPC兼容边界。

> 当前版本仍是预发布版本。功能状态以本README的兼容矩阵和GitHub Release说明为准。

## 下载

从 [GitHub Releases](https://github.com/qiu7824/deepseek-harness-rs/releases) 下载对应平台的完整包：

- `dsh-windows-x86_64.zip`
- `dsh-linux-x86_64.tar.gz`
- `dsh-macos-x86_64.tar.gz`
- `dsh-macos-aarch64.tar.gz`

完整包包含二进制、`web/dist`、`config/agent-presets`、随附Web插件和安全说明。不要只复制二进制后再期待完整Web界面和随附插件可用。

## Windows快速启动

解压完整包后运行：

```powershell
.\dsh.exe web
```

默认地址：

```text
http://127.0.0.1:58080/
```

Windows包同时提供PowerShell 7 + WPF管理器：

```powershell
pwsh -NoProfile -STA -WindowStyle Hidden -File .\windows\DshServiceManager.ps1
```

它只负责启动、停止、重启正式`dsh.exe web`进程以及打开网页、日志目录，不负责配置Provider、模型或凭据。

## Linux和macOS

```bash
./dsh web
```

如需指定端口：

```bash
./dsh web --port 58080
```

## 数据、Profile与工作区

默认数据根：

```text
Windows: %LOCALAPPDATA%\DeepSeek Harness
Linux/macOS: 由平台数据目录与DSH_HOME决定
```

可通过`DSH_HOME`显式指定数据根。工作区是会话的唯一项目目录来源；产品不提供重复的“工作目录”设置。

Profile插件位于：

```text
<DSH_HOME>/profiles/<profile>/node_modules
```

正式会话、附件、缓存、设置和插件库存都属于用户数据，不应随升级包覆盖或清理。

## Provider与协议

项目不会替用户配置Provider、API Key、默认模型或账户。认证信息必须由用户通过正式配置入口自行提供。

| 协议/API | 状态 |
|---|---|
| DeepSeek/OpenAI-compatible Chat Completions | 已接入正式Rust适配器 |
| OpenAI Responses | 已提供显式`api: openai-responses`入口；工具、推理、图片、usage和SSE已覆盖fixture，真实Provider仍需使用者配置后验收 |
| Azure OpenAI Responses | 未完成正式Provider闭环 |
| OpenAI Codex Responses | 未完成正式Provider闭环 |
| Anthropic Messages | 未完成 |
| Bedrock Converse Stream | 未完成 |

完整证据见[`docs/protocol-matrix.md`](docs/protocol-matrix.md)。存在文件名或crate不代表生产能力已完成。

## Web插件

纯Web插件无需Node、npm或pnpm。Rust Host负责校验、发现、登记和静态服务预构建的`client.js`。

GitHub插件安装必须固定完整40位commit SHA：

```powershell
.\dsh.exe plugin --profile web add github:owner/repository#0123456789abcdef0123456789abcdef01234567
```

管理命令：

```powershell
.\dsh.exe plugin --profile web list
.\dsh.exe plugin --profile web remove package-name
```

兼容范围：

- 纯Web插件：支持；
- Web + Node Host插件：只加载Web部分并明确跳过Host部分；
- 纯Node Host/native插件：Rust Host不执行。

Web插件与主应用同源运行，拥有页面级JavaScript能力。只安装来源可信、固定commit并完成审计的插件。

随附插件：

- `dsh-voice-input`：浏览器语音输入；
- `dsh-composer-expand`：输入框展开/收起；
- `dsh-context-jump`：超长会话顶部、底部、上一节点、下一节点快速跳转，并按需触发更早历史分页。

## 能力状态

| 能力 | 状态 |
|---|---|
| 会话、持久化、历史分页 | 已接入正式Host |
| DeepSeek长流、reasoning、tool、图片、usage | 已实现，发布后仍需按真实Provider复验 |
| 子智能体 | 内置spawn/fork可用；外部Codex/Claude Code提供方未默认安装 |
| 工作流 | 引擎和工具已实现，最新Release仍需真实模型E2E |
| 终端 | 已实现持久终端、输入、关闭和回收 |
| MCP | 底层客户端实现；正式Host尚未组合配置入口 |
| LSP | 底层registry/tool实现；正式Host尚未组合 |
| ACP | 协议入口存在；真实prompt/cancel回归尚未封板 |

## 构建

工具链固定为Rust 1.97.1：

```bash
cargo build --release -p dsh-host-cli --bin dsh
```

基础门禁：

```bash
cargo fmt --all -- --check
python tools/verify_product_surface.py
cargo test -p dsh-llm-deepseek --all-targets
cargo test -p dsh-host --test boot -- --test-threads=1
cargo test -p dsh-host-cli --test web -- --test-threads=1
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
- MCP和LSP只有库级能力，尚未接入正式Host配置；
- ACP真实prompt/cancel与Python SDK真实turn仍有回归；
- `dsh-context-jump`第一版基于已渲染稳定节点，Turn/Step完整目录需后续由正式slot暴露timeline；
- Linux和macOS资产只有在GitHub Actions矩阵全部成功后才视为发布完成。

## 许可证

MIT，见[`LICENSE`](LICENSE)。第三方声明见[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)。
