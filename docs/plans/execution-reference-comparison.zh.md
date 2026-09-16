# Hermes、Node DSH、Codex 与 cc-haha 执行机制对照

## 样本与证据边界

| 项目 | 检查版本 |
|---|---|
| Hermes Agent | 官方仓库快照 `682a95258ce9e877cfb607a5ada6436183efdebb` |
| DeepSeek Harness Node | 本地上游副本 `0.1.3-alpha.1` |
| Codex | 本地源码 `bb5054f`（2026-08-03），并核对官方 Windows 沙箱文档 |
| cc-haha | 官方仓库快照 `01c14906`（2026-09-14） |

结论来自源码检查；没有把四个产品的全部故障用例都重新运行一遍，也不代表任何项目永远不会遇到相同问题。Codex 源码快照不是当前桌面二进制的精确版本。

## 1. Hermes：后端明确、环境持续、原始日志可追回

Hermes terminal 默认使用 local 后端，也支持 Docker 等隔离后端。local 适配器直接启动宿主 Shell；不能把 local 的可执行性与 AppContainer 下的可执行性等同。

- Windows local 选择 Git Bash，并处理 Windows/MSYS 路径转换、Git Bash 二进制目录和参数路径转换。
- 初始化通过 login shell/配置的初始化文件获取环境快照，覆盖 nvm、asdf、pyenv 一类依赖。后续会话保留 cwd 与导出环境。
- 返回真实 `exit_code`、cwd 与后台会话标识；后台通过 process 工具继续管理。
- 有命令退出语义测试：grep/rg 退出 1 可以表示没有匹配，diff 退出 1 可以表示不同，不一概解释为系统故障。
- 底层输出收集保留 40% 开头、60% 结尾，溢出时从流开头回填到 spill 文件；文件有 5,000,000 字符上限，并标明封顶。
- cwd 消失或不可访问时有明确恢复逻辑；该恢复会改变目录，需要结果中的 cwd 提示共同使用。

适合借鉴：运行环境描述、后台状态与命令状态区分、原始输出先落盘再截断、具有上下文的退出码解释。

局限：Git Bash 避开部分 PS5 特有问题，但普通 Bash 同样不保证复合命令中每一步成功；local 的较少权限阻碍也不等于更强的安全边界。它不能自动修正中文路径库兼容性或数组算法错误。

来源：[local 后端](https://github.com/NousResearch/hermes-agent/blob/682a95258ce9e877cfb607a5ada6436183efdebb/tools/environments/local.py)、[输出收集器](https://github.com/NousResearch/hermes-agent/blob/682a95258ce9e877cfb607a5ada6436183efdebb/tools/environments/base_output.py)、[terminal 契约](https://github.com/NousResearch/hermes-agent/blob/682a95258ce9e877cfb607a5ada6436183efdebb/tools/terminal_tool.py)。

## 2. Node DSH：已有部分正确设计在 Rust 工具层未保留

- Shell 解析优先 PowerShell 7，5.1 为末级回退；沙箱包装沿用选定的 Shell。
- Windows 沙箱采用 restricted token + ACL，当前 Rust 版采用 AppContainer；不可只比较成功率而忽略访问边界差异。
- 结果保留 stdout/stderr 各自的 `truncated` 和 `spillPath`；渲染函数明确输出被截断及完整流位置。
- 后台增量结果同样保留 dropped-output 提示和原始流路径。
- 退出码、signal、timeout 分别显示。非零退出作为诊断结果返回，并不等于业务成功；模型仍需读取退出标记。
- 提权提示受工具是否暴露升级能力控制，不是对所有结果固定拼接。

这是最适合直接恢复的同源契约：Rust 前台 pwsh 当前只拼接 stdout.text/stderr.text，丢失了底层截断元数据与真实 spill 引用。

局限：Node 同样依赖 stderr 关键词识别沙箱拒绝，不能把它当作误分类问题的完整答案；其 Shell 启动前缀也不提供所有原生命令的 fail-fast 保证。

源码位置：`packages/shell/pwsh-local/src/resolve.ts`、`packages/shell/tool-pwsh/src/render.ts`、`packages/shell/pwsh-sandbox/src/index.ts`、`packages/sandbox/sandbox-local/src/index.ts`。

## 3. Codex：执行状态与审批协调集中管理，但识别仍含启发式

- Shell 类型与可执行路径显式建模，支持 Bash/Zsh/Sh、PowerShell、Cmd；login/profile 选择是独立参数。
- unified exec 跟踪真实进程状态与进程 ID；可以区分命令仍运行、进程退出、输出分段及后续读取，而不是仅凭终端静默宣称完成。
- 沙箱与审批由工具 orchestrator 统一协调。是否允许无沙箱重试要受实际审批策略与文件系统策略约束。
- Windows 官方提供 elevated 沙箱与 unelevated 后备模式，分别使用专用低权限用户等机制或当前用户派生的 restricted token；提供定向读取目录授权。
- 拒绝识别函数名为 `is_likely_sandbox_denied`，代码明确承认无法完全确定。它先排除没有沙箱和退出 0 的情况，再结合关键词及部分信号判断。
- unified exec 还接收 executor 上报的 denial 状态，因此不只依赖输出字符串。

适合借鉴：可信执行器事件、结构化生命周期、集中审批与重试策略、明确“疑似”而非过度确定的分类。

局限：Codex 仍保留关键词启发式，也不能保证所有非零退出归因正确；Shell 的复合命令语义仍存在。它不会自动修复 NumPy 广播或把损坏的 Python 环境变为可用。

来源：[Windows 官方文档](https://developers.openai.com/codex/windows/)、本地 `codex-rs/sandboxing/src/denial.rs`、`codex-rs/core/src/tools/orchestrator.rs`、`codex-rs/core/src/unified_exec/process.rs`。

## 4. cc-haha：Windows Shell 兼容处理较细，但沙箱前提不同

- 优先 PowerShell 7，回退 5.1；处理 Windows 启动别名及 Linux Snap 路径等发现问题。
- 沙箱 adapter 的支持平台为 macOS、Linux 或 WSL2；该源码版本的原生 Windows 路径不启用这套 OS 沙箱。不能以“Windows 没报拒绝”证明它解决了 AppContainer 兼容性。
- Unix 沙箱重新包装 PowerShell 命令时使用 UTF-16LE Base64 `-EncodedCommand`，避免外层 `/bin/sh -c` 与多次引号处理破坏命令。
- 这只保护外层脚本运输，不等于自动解决 PS5 内部向原生程序传递任意 argv 的问题。
- 退出状态优先 LASTEXITCODE，再回退 `$?`，专门规避 PS5 下成功程序 stderr 重定向导致 `$?=false` 的误报。
- 源码同时明确记录代价：`native-ok; cmdlet-fail` 可能仍返回 0，因此不应直接复制为完整的 fail-fast 方案。
- 大输出在 ExecResult 中携带 outputFilePath、outputFileSize、outputTaskId。
- 沙箱配置已启用但平台或依赖不支持时有明确的不可用原因提示，避免无反馈地让用户误以为隔离生效。

来源：[PowerShell provider](https://github.com/NanmiCoder/cc-haha/blob/01c14906/src/utils/shell/powershellProvider.ts)、[Shell 检测](https://github.com/NanmiCoder/cc-haha/blob/01c14906/src/utils/shell/powershellDetection.ts)、[沙箱 adapter](https://github.com/NanmiCoder/cc-haha/blob/01c14906/src/utils/sandbox/sandbox-adapter.ts)、[输出结果](https://github.com/NanmiCoder/cc-haha/blob/01c14906/src/utils/ShellCommand.ts)。

## 对 Rust 版的具体采用顺序

1. 恢复 Node 同源结果契约：真实退出码、signal、timeout、截断和原始流引用必须到达模型。
2. 参考 Codex 集中管理启动、运行、退出、取消及授权重试；应用自由文本不能升级为可信运行器事实。
3. 固定同一受控 Shell 与版本，优先解决 PS5 兼容语义；权限重试不顺带更换 Shell。
4. 参考 Hermes 和 cc-haha 提供可靠环境发现、参数运输与输出收集；精确检查宿主能力与沙箱能力。
5. 为关键业务步骤提供显式检查退出码的原语；不要依赖“最后一个命令返回 0”判断全部成功。
6. 图片、文档转换与数组运算建立独立输入检查、能力检测和产物验收；它们属于应用正确性层，不能统一交给沙箱重试处理。

会话 `agent-session-09334282-f02c-469f-93bc-19d4ceddc240` 的逐项错误证据见 [专项归因](session-09334282-execution-root-causes.zh.md)。
