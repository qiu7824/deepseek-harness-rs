# 执行链路跨平台审计与多轮回归

日期：2026-09-16。

## 验证边界

- Windows：现有安装版，真实模型会话 `agent-session-237c0827-25da-400e-a478-9e0876a08148`；在原有八轮测试后继续追加回归。
- Shell 对照：Windows PowerShell 5.1.26100.9444、PowerShell 7.6.5、Git Bash 5.2.26。
- Shell 对照使用当前仓库的 PowerShell 初始化前缀与 helper，在同一 Windows 宿主运行，不代表 Linux/macOS 的原生沙箱验收。
- 本机 WSL 未安装，未安装新系统、虚拟机或容器；Linux/macOS 结论来自源码与 CI 覆盖检查。
- 安装版 SHA256 保持为 `DD6407D9B33EEF4F7E2F32625AB284E53527763F4EC74E91D7DD763DB7F726ED`。源码中并行出现的环境能力发现改动尚不能视为该二进制的已验证功能。

证据位于 `D:\codex操作目录\execution-diagnostics-20260916`：`engine-matrix.json`、`extended-history.json`。

## 1. Shell 版本直接影响执行正确性

相同初始化逻辑、相同 Python 测试程序、相同 Windows 宿主：

| 用例 | Windows PowerShell 5.1 | PowerShell 7.6.5 |
|---|---|---|
| 原生程序退出 7 后写文件 | 后续文件被创建；最终退出 7 | 后续文件未创建；立即终止 |
| 原生程序退出 7 后运行成功程序 | 两条均执行，最终退出 0 | 第二条未执行 |
| 成功程序 stderr 经 `2>&1` 重定向 | NativeCommandError，中断后续步骤 | 保留警告，后续步骤正常执行 |
| helper 传递嵌入双引号的 `python -c` | 引号丢失，Python NameError | 引号保留，执行成功 |

这些结果证明 Shell 版本是独立故障因素，不能把普通运行和权限重试的全部差异归于沙箱。PowerShell 7 的结果只覆盖上述用例；不能推断升级 Shell 就能解决 AppContainer、GUI、所有模块与 DLL 兼容问题。

## 2. Bash 同样需要明确的执行契约

Git Bash 对照结果：

| 命令 | 退出码 | 后续标记 |
|---|---:|---|
| `false; printf 'AFTER_FAILURE\n'` | 0 | 出现 |
| `false \| cat; printf 'AFTER_PIPELINE\n'` | 0 | 出现 |
| `set -e; false; printf 'SHOULD_NOT_RUN\n'` | 1 | 未出现 |
| `set -e -o pipefail; false \| cat; printf 'SHOULD_NOT_RUN\n'` | 1 | 未出现 |

这是 Shell 默认语义，不是 Bash 自身异常。Agent 如果将脚本最终退出码当作所有子步骤都成功，仍然会误报。当前 `bash-local` 使用 `bash -c`，没有自动开启上述选项；Bash 终端同样不能被假定具有 fail-fast 语义。

`set -e` 对条件、逻辑列表等有上下文语义，不能代替结构化步骤状态或事务式交付。上表只验证具体命令。

## 3. 当前 Host 并未在 Unix 自动切换普通执行器为 Bash

`dsh-host` 无条件注册 LocalPwshExecutor 与 pwsh 工具；非 Windows 默认命令是 `pwsh`。Unix 的持久终端使用 Bash，但它与普通 pwsh 调用是不同入口。

因此，Linux/macOS 的发布验收需要同时验证：

- PowerShell 存在、版本合适，并可在受限环境启动；
- Bash PTY 的交互与完成状态；
- 两个入口的环境、工作目录、权限与日志语义一致。

仅发现 `/bin/bash` 存在不能证明 Host 的普通命令工具可用。

## 4. 跨平台风险矩阵

| 问题 | Windows | Linux/macOS | 证据等级 |
|---|---|---|---|
| PS5 引号、stderr 与非零退出语义 | 已复现 | 同样使用 PS7 时上述具体 PS5 问题不宜直接外推 | Windows 两版本对照；Unix 待原生验证 |
| Write-Progress/ReadConsoleOutput 0x5 | 已复现 | 属于 Windows Console API 路径，不直接适用于 Unix | Windows 原生复现 |
| AppContainer 阻断用户目录工具链/DLL | 已观测 | 不使用 AppContainer，限制机制不同 | 源码与 Windows 实测 |
| stderr 文本误判为系统/运行器故障 | 已复现 | Linux 匹配 `read-only file system`/`bwrap:`，macOS 匹配 `operation not permitted`/`sandbox-exec:`，具有同类风险 | 共用分类逻辑与平台规则检查 |
| stdout/stderr 底层截断信息丢失 | 已复现 | 普通 pwsh 工具共用相同包装路径 | 共用源码，Unix 原生待测 |
| 终端静默被误当命令完成 | 已复现提前返回 | Unix 终端同样使用 inferred_idle 与相同渲染器 | 共用源码，Unix 原生待测 |
| 超时前已经发生的写入 | 需按副作用验证 | 进程终止本身不提供文件事务回滚 | 通用执行模型 |
| GUI 应用发现与权限 | WPS 非默认盘符漏查已确认 | 需要各平台独立发现与操作通道 | Windows 证据；Unix 不能套用 Windows 路径 |
| 图片数组维度/输入校验错误 | 已确认 | 算法形状错误与 OS 无关 | 业务代码与错误栈 |

## 5. 沙箱读取边界存在实现差异

当前 Linux profile 通过 bubblewrap 将 `/` 只读挂载，再开放工作区和托管临时目录写入。macOS Seatbelt profile 默认允许操作，再限制文件写入。Windows 使用 AppContainer 及显式授权。

因此，宿主用户本来能读取的工具链在 Linux/macOS 下通常不经过 Windows 相同的 capability 授权路径；但宿主 ACL、挂载、平台隐私限制、运行器可用性仍可能导致失败。源码差异不能替代平台上的正反向访问测试。

不应为了对齐表面成功率而无条件放宽 Windows 权限。应定义清晰的文件读取、写入、运行时和桌面能力契约，再分别实现和验收。

## 6. 新增真实对话结果

### I：应用文本伪装运行器错误

事件 2182：测试 Python 程序只向 stderr 输出 `dsh-sandbox-windows:` 前缀并退出 1；工具返回 `SANDBOX_RUNNER_FAILED`，宣称沙箱启动或清理失败。

这是已确认的误分类。Linux/macOS 的运行器识别也使用自由文本前缀，需增加对应回归。可信运行器状态应通过独立协议或受控通道传递，不与应用 stderr 混合建立权限与重试结论。

### J：300ms 超时

事件 2374：返回 SHELL_TIMEOUT。事件 2451：前后两个写入标记均不存在，说明这次预算不足以执行到第一个标记。不能由此推断所有超时都会回滚，也不能声称已经验证了“先写入再中止”的用例。

### K：可选缺失程序探测

事件 2696：helper 返回 ExitCode=127、保留命令不存在说明，并输出 AFTER_MISSING_PROBE。该路径正确地把可选依赖缺失保留为可检查诊断，不是缺陷。

### L：工作目录状态隔离

事件 3055：持久终端执行 `cd extended` 后，目录保持为工作区的 extended 子目录。事件 3057：独立 pwsh 的 Get-Location 仍返回工作区根目录。事件 3092：终端正常关闭。

这是正确的会话隔离行为。Agent 必须显式传递跨工具的 workdir，不能假定前一个终端的 cd 会影响后续独立工具调用。该规则也应在 Unix 平台验证。

### M：3 秒超时与已发生副作用

事件 3354：3 秒预算下，在第一个文件写入并输出 BEFORE_TIMEOUT2 后触发超时；8 秒等待后的第二次写入未完成。事件 3448：第一个文件存在，第二个不存在。

这是预期的进程超时语义，而非事务回滚。恢复策略必须检查已发生的副作用；盲目重放可能重复写入或执行外部动作。事件 3650 记录该轮正常结束，没有留下测试终端。

新增测试共五轮，全部完成；Shell 版本与 Bash 对照共 15 个用例。

## 7. 原生平台验收要求

现有 release CI 包含 Linux/macOS 构建与测试，但 `pwsh-local/tests/execution.rs` 的多个真实执行用例带 `cfg(windows)`，不会在 Unix 上覆盖同样的行为。CI 通过不能据此证明跨平台 Shell 行为一致。

新增原生矩阵至少覆盖：

1. Ubuntu：bubblewrap 可用与不可用、受限命令、真实/伪造拒绝文本、工作区内外正反向读写。
2. macOS：Seatbelt 实际执行、伪造 sandbox-exec 文本、工作区内外写入、PTY 提前返回。
3. 各平台 PowerShell：版本、argv 往返、非零退出、stderr 警告、长输出及完整资源、超时和取消。
4. 各平台 Bash：序列和管道失败、显式检查、PTY cwd 与独立命令 cwd。
5. 各平台运行时：可执行文件、动态库、缓存写入、缺失依赖；分别验证宿主发现与沙箱可用性。
6. Agent 多轮：失败后诊断、同权限恢复、产物验证、禁止盲目重试与误报成功。

原生结果需记录 OS、架构、二进制哈希、Shell 版本、实际沙箱模式、tool call/result 与文件校验。平台环境缺失时应记录未验证，不以 Windows Git Bash 或源码检查标记为通过。
