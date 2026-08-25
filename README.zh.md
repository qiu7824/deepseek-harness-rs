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

最简单的方式是直接双击：

```text
windows\启动DSH管理器.cmd
```

如果机器没有PowerShell 7，CMD会显示并自动复制一键安装命令。当前Windows x64安装命令为：

```cmd
powershell -NoProfile -ExecutionPolicy Bypass -Command "$u='https://github.com/PowerShell/PowerShell/releases/download/v7.6.5/PowerShell-7.6.5-win-x64.msi'; $p='$env:TEMP\PowerShell-7.6.5-win-x64.msi'; Invoke-WebRequest $u -OutFile $p; Start-Process msiexec.exe -Verb RunAs -Wait -ArgumentList '/i',$p,'/qn','ADD_EXPLORER_CONTEXT_MENU_OPENPOWERSHELL=1','ENABLE_PSREMOTING=1','REGISTER_MANIFEST=1'; Remove-Item $p -Force"
```

安装需要管理员授权。完成后重新双击`启动DSH管理器.cmd`。

也可以从终端启动：

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
