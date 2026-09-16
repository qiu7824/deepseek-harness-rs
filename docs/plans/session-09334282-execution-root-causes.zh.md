# 会话 09334282 执行故障归因

审计日期：2026-09-16。

会话 ID：`agent-session-09334282-f02c-469f-93bc-19d4ceddc240`。

审计快照包含 52 次工具调用、8 个工具错误结果，以及返回正常但内部探测失败的诊断。会话仍在执行，计数仅对应截至事件 11393 的快照。原始业务文件与该会话均未被修改。

## 结论

故障来自三层叠加：Rust 版执行器与沙箱的兼容性缺口、宿主 Python/办公软件能力发现不足、模型生成代码与恢复策略错误。扩大沙箱权限不能解决全部问题，也不能替代正确的运行时和应用能力选择。

## 逐项证据

| 事件 | 故障 | 归因与证据 | 处理方向 |
|---|---|---|---|
| 398 | Expand-Archive 报 SANDBOX_DENIED | 实际 ErrorRecord 为 Write-Progress / ReadConsoleOutput / HostException；真实独立沙箱对话已验证关闭进度显示即可成功 | PowerShell 非交互兼容修复；纠正文件拒绝误分类 |
| 979 | 检测 PIL 等模块时报 NameError | 嵌入引号在 PS5 原生参数传递中丢失；无沙箱复测也失败 | 修复 argv/脚本执行，不重装 PIL |
| 1644 | imread 失败后 cvtColor 空图断言 | 中文路径读入失败且没有检查 None；字节解码与英文相对路径对照均成功 | 通用图片读取接口、输入检查 |
| 5518 | NumPy broadcasting ValueError | 三通道 `(1300,2200,3)` 与二维 mask `(1300,2200)` 相乘；属于算法实现错误 | 显式 mask 维度校验和最小数组用例 |
| 9401 | PyMuPDF `_extra` DLL 拒绝访问 | 受限环境下的 DLL 装载问题，具体拒绝的依赖尚未定位 | 按真实 DLL 依赖定位权限，不直接扩大整个 Python/用户目录 |
| 9546 | 扩大权限后仍无法 import fitz | `_mupdf` 报“找不到指定的程序”，随后 fallback 报缺少 mupdf；同一 Python38 在当前无沙箱环境中也复现 | 宿主 Python DLL/依赖装载环境需修复；不能归为纯沙箱问题 |
| 9667 | pip list 警告中止后续查找 | invalid distribution 警告被 Windows PowerShell 5.1 stderr 重定向转换为 NativeCommandError；独立成功原生命令也复现 | 区分警告与退出状态，采用可靠诊断执行层 |
| 8982、9161 等 | 多次 COM/办公程序查找失败 | catch 只打印 `WPS COM fail`、`WPS.Application fail`，丢失异常详情；大部分路径搜索限于 C 盘 | 宿主能力发现与可用性探测，保留原始异常 |
| 10552 | pypandoc 找不到 pandoc | Python 包包装器存在不代表原生程序存在 | 运行时能力描述区分 wrapper、binary、功能探测 |

## WPS 已安装的宿主证据

宿主 App Paths 注册信息指向：

```text
D:\Program Files\WPS Office\12.1.0.28505\office6\wps.exe
```

该文件实际存在，版本 `12.1.0.28505`。D 盘还存在旧 WPS 安装目录。

这证明“PATH 没有找到、C 盘几处搜索无结果、沙箱内 COM 失败”不足以判断 WPS 没安装。文件存在也不证明 COM 或自动导出已可用；后续应通过经授权的桌面自动化通道进行功能探测。

建议由可信宿主读取显式应用配置、HKCU/HKLM App Paths 与必要的注册表视图，向 Agent 返回可执行路径、版本、是否可在沙箱运行、需要哪种操作通道。避免模型反复递归扫描不同磁盘来猜测安装位置。

## PyMuPDF 能力选择

环境实测：`C:\Users\xs\AppData\Local\Programs\Python\Python38\python.exe`，PyMuPDF 1.24.11。无沙箱 `import fitz` 同样失败，说明存在沙箱之外的 DLL/依赖装载问题；具体依赖冲突尚未完成定位，不能只凭 fallback 的 `No module named mupdf` 判定安装那个同名包即可解决。

此外，普通 PyMuPDF 不能被当作已验证的 DOCX 渲染器；官方把 Office 文档支持列在 PyMuPDF Pro 中。应先确认产品版本和能力，再执行文档转换。[官方支持格式](https://pymupdf.readthedocs.io/en/latest/how-to-open-a-file.html)

办公文件转换应使用已指定的 WPS 通道；读取 DOCX XML 或转 HTML 后重新排版不能自动满足原文档分页和版式一致性要求。替代方案必须经过独立的版式验收。

## Rust 版与 Node 源码副本的差异

本地 Node 源码副本优先选择 PowerShell 7、最后才回退 5.1；沙箱执行复用该 Shell。当前 Rust 源码在受限模式优先选择 5.1，放宽权限后又可能选择 PATH 中的 PowerShell 7。两者在引号、stderr、退出状态和内置模块上并不等价。

Node 源码副本采用 restricted token + ACL；当前 Rust 源码采用 AppContainer。更强的访问限制需要同时完善已安装工具、原生 DLL 和桌面应用的能力通道。不能将 Node 环境中可运行直接当成 Rust 版应无条件放行的依据。

Node 版同样存在基于 stderr 关键词识别沙箱拒绝的设计，因此切回 Node 或改用持久终端不能作为这些问题的统一修复。

## 产品修复优先级

1. 修复失败后继续执行与退出码被覆盖，防止产生错误产物却报告成功。
2. 固定 Shell 选择、纠正原生 argv、关闭非交互进度绘制、可靠捕获原生退出状态。
3. 提供宿主工具链和办公应用能力清单；细分沙箱读取、DLL 装载、桌面自动化权限。
4. 根据结构化证据生成恢复动作，避免将不存在、软件损坏和程序逻辑错误统一导向提权。
5. 给 Agent 提供可靠的图片输入、数组检查、文件转换与产物验证接口；记录失败根因并避免重复同类探测。

完整回归证据见 [执行链路缺陷审计](windows-execution-agent-bug-audit.zh.md)，架构方案见 [执行诊断与恢复设计](windows-execution-diagnostics-and-recovery.zh.md)。
