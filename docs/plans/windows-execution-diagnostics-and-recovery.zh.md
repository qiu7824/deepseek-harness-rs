# Windows 执行诊断与恢复设计

## 目标

使命令发现、文件权限、控制台兼容、图像解码和运行器故障具有不同的诊断与恢复路径。普通命令、后台任务和持久终端共享权限边界；恢复操作不得隐式关闭沙箱。成功必须由目标产物或操作结果验证，不能仅以进程退出码为依据。

## 实施顺序

1. P0：关闭非交互 PowerShell 进度绘制，改正控制台错误分类和无条件提权提示。
2. P0：增加工具链的宿主发现与受限可访问性双向探测，支持经过授权的精确运行时读取范围。
3. P1：将执行事实、失败阶段和恢复建议作为结构化数据返回，统一前台、后台、终端行为。
4. P1：提供中文路径兼容的图片读写范式和明确的失败检查。
5. P1：在真实安装版、真实模型对话和受限进程中进行回归验收。

## 1. 非交互 PowerShell

落点：`crates/shell/pwsh-local/src/lib.rs` 的 `ENCODING_PREAMBLE` 和 `pwsh_argv`。

- 初始化 `$ProgressPreference = 'SilentlyContinue'`，仅影响本次子进程；保留 `$ErrorActionPreference = 'Stop'`。
- 保留 `-NoProfile`，避免执行用户启动脚本引入不可预测的副作用；不要把加载 profile 作为工具链发现的通用修复。
- 返回实际 shell 路径、版本与 PSEdition；工具名 `pwsh` 不能被解释为必然使用 PowerShell 7。
- 对 Windows PowerShell 5.1 和 PowerShell 7 分别验证 UTF-8、原生命令 stderr、LASTEXITCODE 和模块加载。
- 不全局忽略 stderr、HostException 或所有非零退出码。
- 用户命令显式修改 ProgressPreference 后，其行为以命令本身为准。

归档验收必须检查：源文本、ZIP 有效性、解压后的文件数量及内容。仅看到 ZIP 文件存在或 PowerShell 返回 0 不足以判定成功。

## 2. 结构化诊断

落点：`crates/shell/shell/src/types.rs`、`crates/shell/tool-pwsh/src/lib.rs`、`crates/sandbox/sandbox-local/src/lib.rs`。

逐步替代 `denied: bool` 对所有访问错误的概括。建议诊断包含：

```text
category: command_not_found | file_access_denied | console_host_error |
          input_not_found | decode_failed | runner_setup_failed |
          runner_cleanup_failed | timed_out | cancelled | command_failed
phase: resolve | setup | spawn | execute | collect | cleanup
confidence: confirmed | suspected | unknown
source: policy | runner | shell_error_record | application_output
commandStarted: true | false | unknown
effects: none | possible | verified
retry: none | same_policy_after_change | scoped_permission_required
```

保留退出码、signal、stdout/stderr、原始 ErrorRecord 标识和截断日志资源引用。错误输出不得成为权限授权依据。

分类规则：

- `ReadConsoleOutput` 与 `HostException` 等相符证据归为控制台异常；`Write-Progress` 只是辅助证据，不作为单独判定条件。
- `ItemExistsUnauthorizedAccessError`、`UnauthorizedAccessException` 和明确目标路径说明文件访问被拒绝，但是否由沙箱策略造成仍需结合实际执行上下文。
- `CommandNotFoundException` 只证明受限进程没有解析到命令，不证明宿主未安装软件。
- 单独的 `Access is denied` 只能作为低置信度线索，不能直接宣称文件沙箱拒绝。
- 同一输出同时包含控制台错误和文件权限错误时，保留多个诊断；不能因发现一个控制台错误就过滤掉整个 stderr 的权限信息。
- 运行器协议或进程状态提供的启动、清理事实优先于应用程序自由文本；应用打印伪造标记不能升级为可信运行器事实。
- 正常退出时打印包含 `permission denied` 的文本，不应形成权限失败。

错误文案只描述已证实的阶段及对象。提示明确可执行的下一步，不统一追加 `require_escalated`。

## 3. 工具链发现与精确授权

落点：宿主运行时注册、`subprocess-local`、`sandbox-local/runtime_read_access.rs`。

建立一个共享的 ToolchainDescriptor，描述可执行文件、来源、版本、必要只读依赖目录、独立可写缓存、适用平台与验证状态。供普通 shell、后台任务、PTY 使用。

发现优先级：显式配置 > 项目已声明工具链 > 宿主 PATH > 受控的标准安装目录。探测必须有时间和输出上限；不扫描整个磁盘，不执行任意 profile，不为所有 PATH 目录自动授予读取权限。

Rust 至少分开识别：

- `cargo` / `rustc` / `rustup` 代理可执行文件所在目录；
- 实际选定的 toolchain 目录及必要 DLL；
- rustup 配置与工具链选择文件；
- Cargo registry/git 下载缓存、锁文件和构建输出；
- MSVC linker、Windows SDK 等后续编译依赖。

不得给整个用户目录或整个 `.cargo` 无差别授权：其中可能存在注册表凭据及其他敏感配置。只授权必要文件或安全子目录；现有目录级授权无法表达时，必须扩展权限模型或使用独立托管配置，不能扩大到包含凭据的父目录。

诊断分为宿主发现和沙箱探测两部分：宿主能找到、沙箱无法 stat/执行时报告 `installed_but_inaccessible`，而非建议重装。探测至少比较裸命令和绝对路径；PATH 中已有目录时不重复追加。

运行时读取授权必须通过既有产品权限策略与明确的运行时信任配置。写权限仅给受控缓存和目标输出；安装、更新、下载另行评估。不得因工具链可用性问题自动切换 `danger-full-access`。

对运行时路径做规范化、重解析点和替换检测；权限缓存与运行时身份关联。卸载、替换、版本更新和授权撤销需要明确失效策略。共享缓存的并发访问和取消后清理必须可验证。

## 4. 图片与路径兼容

图片输入先读取字节，再解码：

```python
from pathlib import Path
import cv2
import numpy as np

def read_image(path):
    raw = Path(path).read_bytes()
    if not raw:
        raise ValueError(f"图片文件为空：{path}")
    image = cv2.imdecode(np.frombuffer(raw, dtype=np.uint8), cv2.IMREAD_COLOR)
    if image is None:
        raise ValueError(f"图片无法解码：{path}")
    return image
```

- 保留 FileNotFoundError、PermissionError 等原始异常；不统一转换为“沙箱失败”。
- 检查空文件和解码失败，禁止把 None 继续传入 cvtColor。
- 编码写出先检查 `cv2.imencode` 成功，再通过 `Path.write_bytes` 保存；正式交付采用临时文件验证后替换。
- 读取字节不能绕过真实 ACL；仍失败时按权限路径处理。
- Windows 路径使用原始字符串、Path 或结构化参数；不要把 PowerShell UTF-8 设置当成 OpenCV 文件名兼容性的修复。
- 长路径、UNC、中文路径、空格和括号分别测试，不能由中文路径用例推断全部通过。
- 全英文文件名的绝对路径若父目录含中文，仍属于非 ASCII 路径；对照实验应使用相同工作目录下的英文相对路径或真正全 ASCII 路径。
- 诊断只输出字节长度、必要文件签名、shape、dtype、异常类型；不得打印完整图片字节或像素矩阵。限制输出应发生在诊断生成端，日志 spill 只作为最后兜底。

优先放入正式图片工具或可复用模板；若只有通用 shell，则给模型提供短小、可验证的操作指导，不给每条命令自动注入 OpenCV 代码。

## 5. 恢复协议

1. 识别失败阶段与原始错误，记录当前 shell、目录和实际沙箱模式。
2. 在相同权限边界执行最小只读诊断。
3. 使用明确的兼容性修复或正确路径，避免没有信息增益的重复执行。
4. 重试写入操作之前检查是否已经产生部分副作用；使用全新唯一输出目录，不为重试先递归删除旧目录。
5. 真正缺少权限时，申请精确目录、访问类型和生命周期；当前策略禁止审批则返回明确阻塞原因。
6. 仅在策略允许且授权成立时扩大权限。审批被拒绝后不能换另一工具或终端完成同一受限操作。
7. 验证最终产物；退出码为 0 但操作内部 catch 后报告失败，仍应判定业务步骤失败。

不通用自动重放任意失败命令。对只读探测、独立临时目录操作可有限恢复；未知副作用、安装、删除或覆盖操作必须先核实状态。

## 6. 持久终端与后台一致性

- 三类执行入口使用同一个环境构造器、ToolchainDescriptor 与权限描述。
- 建立终端时记录权限快照；权限收紧或撤销后不能继续复用权限更宽的旧终端，需要停止或重建。
- PTY 改变的是终端协议与交互能力，不代表绕过文件沙箱。
- 后台任务完成时保留同样的诊断分类；不能只保留 `exit code: 1`。
- 终端读取、取消与超时不得导致后台进程或权限租约残留。

## 7. 验收矩阵

| 场景 | 应验证的结果 |
|---|---|
| rustc 未安装 | command_not_found；不错误申请文件授权 |
| 宿主已安装、受限进程不可访问 | installed_but_inaccessible；包含必要目录诊断 |
| PATH 缺少工具链目录 | 本次环境修复后裸命令成功；无系统 PATH 修改 |
| rustc 代理可读、toolchain 不可读 | 明确识别第二阶段依赖权限，不误报命令未安装 |
| Rust 完整离线构建 | rustc/cargo、链接器及缓存均可用；禁止范围保持拒绝 |
| 默认进度显示的归档操作 | 能复现旧问题的环境留存原始错误 |
| 禁用进度显示的归档操作 | 压缩、解压、文本校验通过；未扩大权限 |
| 真正禁止的文件访问 | 仍拒绝；控制台排除规则不能吞掉真实权限错误 |
| 多个 ErrorRecord 混合 | 保留多个类别与证据，不按整个 stderr 简单排除 |
| 普通 stderr 含拒绝字符串 | 不升级为可信沙箱事实 |
| 中文与 ASCII 同内容图片 | 字节解码均成功，尺寸相同 |
| 缺失、损坏、零字节图片 | 三者与权限错误明确区分，无后续空图像断言 |
| 前台、后台、PTY | 相同授权边界及工具链可用性；PTY 不成为逃逸路径 |
| 权限撤销、运行时升级、并发、取消 | 缓存失效正确、无遗留进程或越权授权 |
| 策略禁止审批 | 不生成不可执行的提权建议 |
| 重试期间已存在部分输出 | 不删除源数据，不重复危险副作用 |

分层验证：分类单元测试 → Windows 真实进程集成测试 → 已安装发行版的真实模型对话。模型桩可验证协议，但不能替代真实模型对错误提示的理解与恢复验收。

## 8. 发布门槛

先合入兼容性与分类修复，再引入可审计的工具链授权能力。测试新二进制后通过正常安装或升级流程部署，并核对正在运行的文件路径、构建身份和哈希；源码测试通过不等于安装版已修复。

验收证据至少包含：构建身份、模型路由、沙箱模式、tool call/result 对应关系、退出状态、结果校验、是否申请额外权限。原始完整日志保留在测试操作目录，公开报告只保留必要技术证据。
