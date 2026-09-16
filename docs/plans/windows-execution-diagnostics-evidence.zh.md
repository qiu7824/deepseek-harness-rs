# Windows 执行诊断实测记录

日期：2026-09-16。

## 测试环境

- 入口：正在运行的 DeepSeek Harness 安装版，`http://127.0.0.1:58080`。
- 模型路由：`devin/swe-2-high`，真实模型请求与工具执行，非模型桩。
- 测试会话：`agent-session-9c035ddd-ad0d-42a3-a09e-dc74270cf303`。
- 沙箱模式：`workspace-write`。
- PowerShell：Windows PowerShell 5.1.26100.9444，ConsoleHost。
- Python：Python38 安装目录；OpenCV 4.10.0，NumPy 1.24.4。
- 安装版二进制 SHA256：`DD6407D9B33EEF4F7E2F32625AB284E53527763F4EC74E91D7DD763DB7F726ED`。
- 操作与原始证据目录：`D:\codex操作目录\execution-diagnostics-20260916`。

测试没有授予提权、切换沙箱模式、安装软件或修改工具链授权。运行器自身按现有实现建立和回收测试沙箱。

## 工具链不可见

事件 204：`Get-Command rustc/cargo` 均返回未找到，Python 可发现。

事件 206：`rustc --version` 返回 `CommandNotFoundException`。

事件 847：PATH 已含 `C:\Users\xs\.cargo\bin`；`Test-Path` 访问该目录下 `rustc.exe` 返回 `ItemExistsUnauthorizedAccessError` 与 `UnauthorizedAccessException`。

结论：本次失败不能由缺少 PATH 解释，工具链文件在受限环境中不可访问。此阶段没有验证授权后的 rustup/toolchain、链接器和完整构建；不能声称 Rust 编译已修复。

## PowerShell 归档

事件 1267：在全新目录执行 Compress-Archive 时出现 `System.Management.Automation.Host.HostException`，读取控制台缓冲区被拒绝；ZIP 未创建，后续 Expand-Archive 因源文件缺失失败。

事件 1760：保持沙箱模式不变，设置 `$ProgressPreference = 'SilentlyContinue'`，在另一全新目录重试：

```text
compress: OK (175 bytes)
expand: OK
extracted exists: True
content match: True
probe.txt (56 bytes)
```

独立使用 Python zipfile 检查：ZIP CRC 正确、仅含 probe.txt、ZIP 内容与源文件相同、解压内容与源文件相同。

结论：该控制台兼容性问题可在不扩大权限的情况下恢复。应用的通用 `Access is denied` 分类会将此类未捕获异常误报为文件沙箱拒绝；控制台异常与文件访问拒绝需要分别处理。

## 图片读取

事件 4655：两个相同内容的 JPEG 分别使用中文文件名和英文文件名，所在父目录均含中文。

| 输入 | Path.read_bytes | cv2.imread(绝对路径) | imdecode | 后续转换 |
|---|---|---|---|---|
| 中文测试图.jpg | 376924 字节 | None | 4491×3173×3 | 成功 |
| ascii-image.jpg | 376924 字节 | None | 4491×3173×3 | 成功 |
| broken.jpg | 14 字节 | None | None | 跳过 |
| missing.jpg | FileNotFoundError | None | 跳过 | 跳过 |

事件 5585：同一工作目录下使用英文相对文件名 `cv2.imread('ascii-image.jpg')` 成功，shape 为 `(4491, 3173, 3)`。

结论：有效图片在受限环境中可读取及解码，含中文的绝对路径接口失败不等于沙箱禁止读取。英文相对路径成功进一步支持路径兼容性诊断。损坏内容和不存在文件具有独立证据，均不应继续传入 cvtColor。英文 basename 不能抵消中文父目录。

## 恢复流程观察

持久终端对照：事件 5583 建立 `shell` 后端终端，事件 5617 执行 `rustc --version` 返回“不是内部或外部命令”，事件 5642 确认终端关闭。切换到 PTY 未恢复 Rust 命令。该用例验证了实际命令表现；完整权限边界等价性仍需专门的正反向访问测试。

- 首个归档命令含条件式递归删除，进入执行前等待；取消后工具记录为 `ABORTED_BEFORE_DISPATCH`。后续使用新唯一目录，无需删除旧产物。
- 归档探测捕获异常后进程可以正常退出，因此 `isError=false` 不能替代逐项业务结果校验。
- 第一版图片诊断打印完整字节，引起输出截断；精简为长度、签名与 shape 后得到完整且可判断的结果。
- 模型在明确的同权限恢复指导下可以完成归档与图片诊断；这不证明未提供指导时一定能自行选择正确恢复路径。

## 修复范围

实测验证了归档命令和图像读取方法的恢复效果。产品级修复仍需完成默认初始化、结构化错误分类、运行时授权、前后台一致性与发行版回归。

实施设计见 [Windows 执行诊断与恢复设计](windows-execution-diagnostics-and-recovery.zh.md)。
