# Windows Agent 执行链路缺陷审计

日期：2026-09-16。

## 范围与方法

审计覆盖命令执行语义、退出状态、诊断分类、日志保留和终端完成状态。运行目标为现有 DeepSeek Harness 安装版，采用真实 `devin/swe-2-high` 对话和独立 PowerShell 复测。测试文件均位于独立操作目录，没有执行业务删除、提权或软件安装。

真实测试会话：`agent-session-237c0827-25da-400e-a478-9e0876a08148`。

证据目录：`D:\codex操作目录\execution-diagnostics-20260916`。`extended-history.json` 保存会话事件，`local-replay.json` 保存独立复测。独立复测直接读取仓库 PowerShell 初始化前缀和诊断函数，使用 Windows PowerShell 5.1 执行；该复测本身不启用沙箱，不能替代权限边界测试。

## F01 — P1：声明 fail-fast，原生命令失败后仍执行后续操作

状态：真实安装版与独立复测均确认。

复现命令：

```powershell
python .\extended\exit7.py
Set-Content -LiteralPath .\extended\after-failure.txt -Value AFTER_FAILURE
```

Python 测试程序固定退出 7。事件 100 显示工具报错退出 7，事件 174 证明后续文件已经创建。因此“工具最终报错”不代表失败后没有副作用。

第二种复现：

```powershell
python .\extended\exit7.py
python -c "print('SECOND_COMMAND_SUCCESS')"
```

事件 408 显示两个输出标记均出现，工具 `isError=false`；独立复测确认最终退出 0。第二条原生命令覆盖了第一条的 LASTEXITCODE。

根因：Windows PowerShell 5.1 下，仅设置 `$ErrorActionPreference='Stop'` 和 `$PSNativeCommandUseErrorActionPreference=$true` 不提供所声明的原生命令 fail-fast 语义。执行器只在整个脚本末尾检查 LASTEXITCODE。工具描述中的 `Commands fail fast` 与实际行为不一致。

影响：构建失败后仍执行打包、覆盖、移动或发布步骤；第二条命令成功时可能产生整体成功误报。

修复要求：

1. 立即收窄工具契约，明确 PowerShell 版本差异；不能继续承诺所有原生命令都 fail-fast。
2. 提供检查原生退出码的执行原语。业务执行的非零状态必须立即中断后续依赖步骤；只读探测则显式返回诊断结果。
3. 优先采用结构化 executable/argv/cwd 执行接口，分步记录退出状态；不要用正则重写任意 PowerShell 脚本。
4. Shell 混合脚本在每个关键原生命令之后显式检查 LASTEXITCODE，或调用严格检查的 helper；诊断 helper 返回对象后也必须检查对象中的 ExitCode。
5. 对需要事务性的产物采用候选文件、内容校验和最后提交，避免半成品覆盖正式文件。

验收：故意失败的第一步之后不得创建标记文件，后续成功不能掩盖失败；预期的非零探测仍可被调用方显式处理。

## F02 — P2：应用自由文本被赋予系统故障含义

状态：真实安装版故障注入确认。事件 575 将没有文件访问的模拟 stderr 判为 SANDBOX_DENIED 并建议提权；事件 724 将固定退出 0 的 COM 示例文本判为 COM_ACCESS_DENIED。

`ShellSandboxInfo::observe` 使用 stderr 子串匹配识别拒绝。任意程序打印 `Access is denied` 并以非零退出，即可被分类为文件沙箱拒绝；不需要实际发生文件操作。

追加故障注入事件 2182：应用只打印 `dsh-sandbox-windows:` 前缀并退出 1，也会被误报为 SANDBOX_RUNNER_FAILED。运行器启动/清理诊断同样需要可信来源隔离。

`tool-pwsh` 对 COM 关键词的检查没有以失败退出作为必要条件。即使子程序退出 0，stderr 中出现 `CLSID`、`80070005`、`COMObject` 的示例文本，也可能被返回为 `COM_ACCESS_DENIED`。

同一 stderr 包含 COM 文本与真实文件拒绝时，当前整体 COM 排除还可能掩盖文件权限诊断。

修复要求：保留应用输出与可信运行器事件的来源差异；关键词只提供低置信度线索。成功进程的普通警告不能自动变成操作失败。按 ErrorRecord 分别分类混合异常，不按整个 stderr 做排他判断。权限扩大建议必须依赖真实对象、操作和有效审批策略。

## F03 — P2：成功原生命令的重定向 stderr 可中止脚本

状态：独立复测和真实安装版均确认。事件 901 返回 NativeCommandError、退出 1，后续 AFTER_WARNING 未执行。

测试程序固定退出 0，但向 stderr 写普通警告。

```powershell
python .\extended\warning_success.py 2>&1
Write-Output AFTER_WARNING
```

独立复测返回退出 1、NativeCommandError，AFTER_WARNING 未出现；不重定向 stderr 时退出 0 并出现 AFTER_WARNING。通过 Invoke-DshNativeProbe 执行时也正确保留原生退出 0 与完整警告。

修复要求：统一使用可区分原生退出状态与 PowerShell ErrorRecord 的受控执行层。现有诊断 helper 可以作为只读探测方案，但不能在忽略 ExitCode 后直接用于业务执行。`allow_nonzero` 只影响结果包装，不能恢复已中断的脚本，不应当作通用解决方案。

## F04 — P2：真实缺失文件也会收到提权建议

状态：真实安装版确认。事件 1148 对授权工作区内刻意不存在的文件附加了 require_escalated 建议。

工具在受限模式下，只要失败输出包含 FileNotFoundError、中文“不存在”或 No such file，就附加外部文件可能被沙箱隐藏的提示，并建议 scoped approval 或 require_escalated。授权工作区内真正不存在的文件也满足这一条件。

修复要求：先区分路径解析、对象不存在与已存在但不可访问；未知时表述为待诊断，不预设沙箱原因。只对被证实的权限缺口给出适用的授权动作。不要通过变更 cwd 暗示任意丢失文件都能恢复。

## F05 — P2：底层日志截断信息与完整流引用在工具层丢失

状态：真实安装版确认。事件 1320 输出约 110KB 测试流后，模型结果只有约 12KB，首标记丢失、尾标记保留；事件 1413 读取返回的“完整”结果资源开头，仍从第 970 条附近开始。该资源不含原始流的首标记。

`CollectedOutput` 含 text、truncated、spill_path；text 在超过收集上限时只保留末尾。前台 pwsh 工具只拼接 stdout.text/stderr.text，没有透传底层 truncated 与 spill_path。

后续工具结果 spill 只能保存已经被截断的文本，不能恢复早已丢失的开头。若将这个资源称为完整日志，会让 Agent 在缺少根因信息时继续判断。

修复要求：从收集器到模型展示保留分流截断标记、保留区间、原始字节数及完整流资源引用；资源必须真实对应原始流。底层完整流也有上限时显式说明；截断输出不足以建立成功结论或穷尽性搜索结论。

## F06 — P2：终端返回内容隐藏完成状态的不确定性

状态：真实终端确认提前返回。事件 1837 在 terminal_send 调用后约 307ms 返回，此时 4 秒延迟任务尚未完成；事件 1907 后续读取才出现独立完成输出，事件 1943 关闭终端。

terminal_send 结构化返回含 waitReason、sessionStatus、truncated，但模型可见文本只渲染 viewport。`inferred_idle` 基于短暂输出静默，不代表进程退出或业务成功。pty-send 后台作业也会在等待操作结束后显示 Completed，必须解释为发送/等待完成，而非任意 shell 子命令成功。

修复要求：模型可见结果携带等待原因、终端存活状态和截断提示；未观察到命令退出时返回 `completion=unknown`。长任务通过明确完成标记或可追踪的执行进程确认，不把空输出或静默当成完成。

完成标记同时出现在输入命令回显与窗口标题中，因此不能通过简单的 substring 搜索判定完成。应使用协议级命令 ID、退出码与独立输出帧。本用例中模型继续读取并关闭了终端，没有把首次返回误判为任务完成；缺陷在于工具反馈没有明确表达状态边界。

## F07 — P2：原生参数传递丢失嵌入引号

状态：业务会话事件 979 与独立复测均确认。

Invoke-DshNativeProbe 将包含 `["PIL","numpy",...]` 的 Python `-c` 代码交给 Windows PowerShell 5.1 的原生参数传递后，Python 报 `NameError: name 'PIL' is not defined`。使用同一 helper 和参数的独立复测得到相同结果，不需要沙箱即可复现。

这是字符串参数传递问题，不是 PIL 未安装。仅增加 UTF-8 设置、重装包或扩大文件权限无效。

修复要求：受控的 Windows argv 序列化及 Python 脚本文件执行；覆盖嵌入引号、空参数、尾部反斜杠、空格、中文、括号等参数往返测试。不要根据该 NameError 触发依赖安装。

## F08 — P2：权限模式改变时可能同时改变 Shell 版本

状态：当前源码确认。

`default_powershell` 在受限模式优先选择 Windows PowerShell 5.1；在 danger-full-access 下先搜索 PATH 中的 pwsh。权限重试因此可能同时改变沙箱策略和 Shell，导致引号、原生命令非零退出、stderr 和模块行为一起变化。

修复要求：普通与授权重试固定同一个经过兼容性验证的 Shell；若需要改变 Shell，作为独立恢复动作明确报告。每次执行返回 shell 路径、版本、实际权限策略和运行时身份，不能把多变量变化全部归因为“提权修好了”。

## 修复与回归顺序

1. 优先 F01：停止错误的 fail-fast 保证，阻止失败后依赖写入和退出码覆盖。
2. 同批处理 F02/F03/F04：统一原生执行状态与分类证据，移除误导性恢复提示。
3. 处理 F05/F06：保证 Agent 实际接收到完整的状态边界和可追溯日志。
4. 将原有归档进度异常、工具链读取权限、中文路径读图一并纳入回归矩阵。

回归必须同时覆盖工具底层与模型可见文本；Rust 内部结构存在某字段，不代表模型真正收到了它。真实模型判断与实际文件、退出码、日志证据分别记录，避免仅凭模型总结验收。

追加多轮与平台适用性见 [执行链路跨平台审计](execution-cross-platform-audit.zh.md)。
