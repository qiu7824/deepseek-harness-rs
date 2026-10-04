# Windows Flutter 桌面问题排查与修复

排查基线为 r6 的 `7d8bbf4d44c2d04d1b582927af35ae97b0e5c3a6`。用户确认客户端为 Windows Flutter，失败的订阅提供商为 Devin，模型为 Opus 5.5。下列提交时间使用上海时间（UTC+8）。

## Devin 错误与 JSON 展示

用户给出的错误是 `kind: error` 会话结束原因，含 `INVALID_REQUEST`、`status: 400` 和服务 trace ID。Dart 的 `projectTranscript` 将 `turn/end.reason` 编码成 JSON；Flutter 会话错误分支随后将它作为 Markdown 正文展示。r6 的工具结果 JSON 格式化没有覆盖会话结束错误，因此仍出现原始 JSON。

本次将会话错误接入 `DshErrorView`：主界面显示可读提示，错误码、状态、trace 和脱敏原文保留在默认折叠的详情中。客户端统一解析明确的失败信封，HTTP/RPC 失败边界保留结构化信息；成功的业务 JSON 不经过错误转换。

还发现 Devin Connect 流的结束帧错误被错误分类：旧代码只区分认证、权限和限额，其他协议码全部合成为 `400 / INVALID_REQUEST`。因此旧记录中的 400 可能是客户端分类，不能据此断言服务实际返回 HTTP 400。现在按 Connect 的结构化协议码分类，例如 `unavailable` 对应 503、`internal` 对应 500、`invalid_argument` 保持 400，并保留原始协议码供后续定位。未知协议码不自动推断请求错误或服务故障；取消、超时和不支持的调用分别处理。真实 HTTP 400 的处理保持原语义，不按文案改成可重试错误。

已有 Claude 根级工具 schema 兼容修复在 r6 中仍存在。本轮没有使用账号凭据重放请求，旧错误记录也没有保留完整 Connect 码，无法确定这次 Opus 5.5 失败的服务端根因。修复错误展示和分类不代表外部模型服务已经恢复。

## 界面改动来源

| 项目 | 历史提交与时间 | 可核实的变化 | 本次处理 |
| --- | --- | --- | --- |
| Rust 标识 | [`023f3b9d`](https://github.com/qiu7824/deepseek-harness-rs/commit/023f3b9d58225369360692c40dad26d0c911910b)，2026-10-01 12:42:59 | 中文词条 `rustEdition` 从原来的“Rust 版”改成“预览版” | 恢复“Rust版” |
| 登录绿点 | [`6f249dfa`](https://github.com/qiu7824/deepseek-harness-rs/commit/6f249dfa0491843f10e199a5465826affd746f7b)，2026-10-03 16:48:04 | 已关联且不需重新登录的账号入口及账号行新增绿色状态点 | 移除正常连接绿点，保留文字状态和重新登录提醒 |
| 定时、知识、插件按钮 | 同 `6f249dfa` | 描边按钮改成无边框，窄侧栏允许图标位于文字上方 | 恢复此前描边、水平图文布局和大字号换行；三个图标没有在此次改动中更换 |
| Windows 字体 | 同 `023f3b9d` | 中文回退改成 Microsoft YaHei UI 优先，代码字体改成 Cascadia Mono 优先 | 恢复 Microsoft YaHei、Consolas 优先，保留其他平台与字号设置 |

这两个提交的 Author 和 Committer 均记录为 `Hermes Agent <hermes-agent@localhost>`；`6f249dfa` 还包含 `Claude Opus 5.5` 联名信息。Git 署名不能独立证明实际操作者。

这两批改动已经存在于本次获取的 main 基线 `c9d877809509f3e0cb791c1b653667e1d7a9473e`，从该提交到 r6 上述四个 UI 文件没有变化。它们来自获取最新源码时继承的仓库提交；发行工作流没有生成或重写这些 UI 定义。

按钮还在 `023f3b9d` 中发生过短标签、语义 SVG 和插件主页入口调整。本次恢复的是有明确前后证据的按钮外观，保留现有功能入口。Windows 主字体 Segoe UI 并未被更换。应用依赖系统正文字体，视觉测试把两个微软雅黑名字映射到同一字体文件，不能验证用户机器上的实际字体选择；字体效果仍需 Windows 实机复核。

## 验证与开发推进

新增验证覆盖用户原始错误、HTTP/RPC 失败、成功业务 JSON、详情折叠、取消和操作结果未知；复用账号交互、侧栏窄窗口及大字号测试。Flutter 回归工作流现在也监听常规 UI 和 Dart 客户端源码、测试及依赖锁文件，并在 Windows 上运行错误展示、账号、字体和侧栏测试。

本地 `cargo test --locked -p dsh-llm-deepseek --lib` 的 122 项测试通过，包括真实 HTTP 200 Connect 流结束帧的 16 种错误码、真实 HTTP 400、未知码、凭据脱敏、旧工具 schema 和限额规则。产品与打包相关的 22 项检查通过，YAML、测试文件引用及文档链接检查通过。当前环境没有可执行的 Flutter SDK，Dart/Flutter 的执行结果由远程 CI 验证，不能将上述本地检查算作 Flutter 测试通过。

上游 `dsh-v0.2.1-alpha.1` 的源码对照、开发优先级和验收要求见 [上游评估](upstream-dsh-v0.2.1-alpha.1-evaluation.zh.md)。源码修复的验证状态和新安装包是否已经发布应分别核对；r6 安装包不会随 main 的源码修改而改变。
