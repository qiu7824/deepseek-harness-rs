# 执行权限与响应解析诊断

## 已确认的失败链

会话 `agent-session-515c2b13-7dcf-40cc-b9e1-3fb2b1f82d25` 使用 `workspace-write` 和 `ask`。事件记录显示：

| 事件序号 | 现象 | 判断 |
| --- | --- | --- |
| 377 | PowerShell 合并 Git stderr 时出现 NativeCommandError | 存在 Shell 包装影响，不能仅由该异常判断 Git 原生退出状态 |
| 434 | 原生 Git 返回 128，无法进入 E:\rust\zsui | 换用原生命令没有解决目录访问权限 |
| 436 | 当前工作区仓库 dubious ownership | 独立的 Git 所有权检查，同时存在用户配置读取失败 |
| 490 | Git 无法读取配置文件 | 不能通过忽略一个 warning 解决 |
| 1177 | ai-context.ps1 的 AuthorizationManager 检查失败 | 脚本授权阶段失败；具体授权原因仍需同一受限环境取证 |
| 4687 | Test-Path 无法访问 C:\Users\xs\.cargo\bin\cargo.exe | 文件存在性未知，不能判定未安装；原始路径没有缺少分隔符 |
| 4729 | cargo --version 经 Rustup 代理尝试写 settings.toml 被拒绝 | 版本探针也可能产生用户目录写入，不能当作纯粹的可执行文件存在性检查 |

宿主 Windows PowerShell 下，同一个 `ai-context.ps1 -List` 成功返回 context packs。脚本未签名，没有 Zone.Identifier 流。宿主 Windows PowerShell 的 CurrentUser 为 Bypass、LocalMachine 为 RemoteSigned；另一 PowerShell 环境显示 CurrentUser 为 Undefined。宿主的检查结果不能替代 AppContainer 内实际生效的执行策略、注册表和文件访问状态。未签名本身不足以解释本次失败。

## 软件修复范围

通用发现机制优先标准安装目录中的 PowerShell 7，再检查绝对 PATH，最后回退 Windows PowerShell；显式配置优先且不静默替换。Host 与 Shell 执行器共享此实现。工具链增加 cargo 的发现、配置和受限验证，默认发现的工具路径进入执行快照和上下文指纹，原生执行使用同一解析路径。

1. 应用错误识别增加 PowerShell 脚本授权、Test-Path 权限失败、Rustup 设置写入失败、Git 用户配置和仓库所有权分类。
2. 支持中文“拒绝访问”和 `os error 5`，诊断仍标为 suspected，避免把应用 stderr 伪装成已确认的沙箱裁决。
3. 模型可见文本和结构化执行结果增加具体恢复建议，区分宿主发现、受限环境验证、路径不存在和路径不可访问。
4. PowerShell 环境启动检查同时返回语言模式和分作用域执行策略，不改系统策略。
5. 多步骤输出不再重复渲染最后一个 execution receipt。
6. 通用 RPC 和任务验收接口显式处理空响应、截断 JSON、响应体读取中断；保留 HTTP 状态和接口上下文，不把未知结果当作成功，也不自动重放写入请求。
7. 重复执行提醒按权限失败族归类，识别不同 argv、不同 Shell 包装下的连续失败；`allow_nonzero` 返回的 `completion=failed` 同样参与。提醒是建议，不代替权限裁决。

## 验证结果

- Shell 及执行工具单元测试：13 项通过。
- PowerShell 真实进程集成测试：10 项通过，包括非零退出、原生 argv、取消、模块加载及 UTF-8 输出。
- 原生执行依赖边界测试：4 项通过，失败时不分派后续写入。
- 重复调用提醒测试：2 项通过。
- 执行环境配置测试：14 项通过，包括 cargo 显式选择、权限分离、失效缓存和取消。
- 宿主能力发现测试：6 项通过，1 项显式 Git 集成测试按既有标记跳过。
- 浏览器连接恢复测试及空/截断/无效 JSON、取消和中断响应用例通过。
- 前端产物及 manifest 已同步，语法与差异空白检查通过。

以上不替代真实安装版模型全流程验收，也未证明任何产品在所有环境下零失败。

## 不属于自动修复的操作

不扩大用户目录 ACL，不关闭 Git 全局配置或所有权校验，不自动使用 `safe.directory=*`，不以读入脚本文本后执行的方式绕过授权，不把 sandbox 权限提升隐藏在重试中。

需要工作区外资源时，应通过现有权限审批确定具体范围。使用可读数据源完成只读目标时，可先审阅脚本语义，再读取其 manifest；不可把这种替代方式表述为脚本已经成功执行。

## 待验证边界

### 上游与执行后端对照

- Node `dsh-v0.1.6-alpha.2` 的 `candidatePwshPaths` 优先检查 ProgramFiles 下的 PowerShell 7；Rust 此前缺少该候选路径。参见 [上游源码](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.1.6-alpha.2/packages/shell/pwsh-local/src/resolve.ts)。
- 上游存在 Windows read-only ConstrainedLanguage 的公开报告，不能推断 Node 完全免疫环境问题。参见 [讨论 4924](https://github.com/deepseek-ai/deepseek-harness/discussions/4924)。
- Codex Windows 有独立沙箱设置、权限和日志流程；执行权限与模型推理应分开评价。参见 [官方说明](https://developers.openai.com/zh-Hans/docs/windows/windows-sandbox)。
- Hermes 默认 local 后端在宿主执行，其他后端有不同隔离边界；持久终端状态可以减少重复环境设置，但不等于消除权限限制。参见 [安全模型](https://github.com/NousResearch/hermes-agent/blob/main/SECURITY.md) 与 [终端实现](https://github.com/NousResearch/hermes-agent/blob/main/tools/terminal_tool.py)。

对照结论属于公开文档与源码核对，不是三个产品在同一机器、同一权限配置下的完整对比实验。通用规则必须保持：显式配置优先、宿主发现不等于受限可用、权限拒绝不等于未安装、启动不等于完成、错误重试不绕过权限、不猜测用户安装目录。

浏览器 `Unexpected end of JSON input` 的原始请求 URL、HTTP 状态和服务端响应体尚未提供，不能确定空响应来源于哪一个处理器。此次改善覆盖通用 RPC 与任务验收接口；其他直接调用 response.json 的独立插件仍须按实际失败端点排查。

源码修改不等于安装版已经更新。需编译、部署和重启后才能验证真实会话的诊断改善。权限阻碍仍需匹配合法执行上下文解决，诊断改善不保证模型一定选择正确下一步。
