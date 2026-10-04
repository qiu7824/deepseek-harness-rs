# Windows Flutter 桌面问题排查与修复

排查基线为 r6 的 `7d8bbf4d44c2d04d1b582927af35ae97b0e5c3a6`。用户确认客户端为 Windows Flutter，失败的订阅提供商为 Devin，模型为 Opus 5.5。下列提交时间使用上海时间（UTC+8）。

## Devin 错误与 JSON 展示

用户给出的错误是 `kind: error` 会话结束原因，含 `INVALID_REQUEST`、`status: 400` 和服务 trace ID。Dart 的 `projectTranscript` 将 `turn/end.reason` 编码成 JSON；Flutter 会话错误分支随后将它作为 Markdown 正文展示。r6 的工具结果 JSON 格式化没有覆盖会话结束错误，因此仍出现原始 JSON。

本次将会话错误接入 `DshErrorView`：主界面显示可读提示，错误码、状态、trace 和脱敏原文保留在默认折叠的详情中。客户端统一解析明确的失败信封，HTTP/RPC 失败边界保留结构化信息；成功的业务 JSON 不经过错误转换。

还发现 Devin Connect 流的结束帧错误被错误分类：旧代码只区分认证、权限和限额，其他协议码全部合成为 `400 / INVALID_REQUEST`。因此旧记录中的 400 可能是客户端分类，不能据此断言服务实际返回 HTTP 400。现在按 Connect 的结构化协议码分类，例如 `unavailable` 对应 503、`internal` 对应 500、`invalid_argument` 保持 400，并保留原始协议码供后续定位。未知协议码不自动推断请求错误或服务故障；取消、超时和不支持的调用分别处理。真实 HTTP 400 的处理保持原语义，不按文案改成可重试错误。

已有 Claude 根级工具 schema 兼容修复在 r6 中仍存在。本轮没有使用账号凭据重放请求，旧错误记录也没有保留完整 Connect 码，无法确定这次 Opus 5.5 失败的服务端根因。修复错误展示和分类不代表外部模型服务已经恢复。

后续应先获取保留原始协议码的新错误，再核对该账号目录返回的实际模型 UID。上游锁定的 `@earendil-works/pi-ai@0.87.1` 中，直连 Anthropic Opus 5.5 描述标记为不支持温度参数；Devin 则使用不同的 protobuf 请求，[公开协议定义](https://github.com/can1357/oh-my-pi/blob/v18.1.18/packages/ai/src/providers/devin/proto/exa/codeium_common_pb/codeium_common.proto#L1985)中的 `temperature` 和 `first_temperature` 是普通 double，现有客户端默认均为 0.4。尚无证据证明 Devin 如何转发或要求省略这些字段；省略 protobuf double 也不等于省略 Anthropic JSON 参数，因此本轮保持模型参数不变。

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

源码 `483b99637db0fe194e3510e00bb1a62b8857978c` 的 [Windows CI](https://github.com/qiu7824/deepseek-harness-rs/actions/runs/37183661041/job/111381072002) 已完整成功：完整 Dart 客户端及七个桌面测试文件的步骤通过，严格图标金图通过，日志保存与作业终态成功。同次运行的 Intel Mac 和 ARM Mac 严格金图也通过。此结论不包含用户机器的实际字体视觉复核，也不代表发布了新的安装包。

同次 [完整 Flutter 回归](https://github.com/qiu7824/deepseek-harness-rs/actions/runs/37183661041/job/111381071926) 的 Dart 客户端测试、完整 Flutter 测试和静态分析均通过。

上游 `dsh-v0.2.1-alpha.1` 的源码对照、开发优先级和验收要求见 [上游评估](upstream-dsh-v0.2.1-alpha.1-evaluation.zh.md)。源码修复的验证状态和新安装包是否已经发布应分别核对；r6 安装包不会随 main 的源码修改而改变。

## r7 候选的后续源码进展

下一修订计划纳入 `483b9963` 的 UI/错误分类修复和 `ae7bf6a8` 的相关说明，并追加桌面启动、Devin 诊断及提醒权限边界。当前下载仍指向 r6；r7 尚待本轮 CI 与新包验收。

自管桌面 Host 现在以 `--port 0` 交给系统绑定端口，不再先探测空闲端口再启动。CLI 用一次性私有就绪报告返回实际地址、PID、实例 UUID 和实际数据根；launcher/Flutter 核对所属进程及探测到的 Host 身份，接受经过验证的数据目录重定向。同一 Host 进程由数据根 A 迁移到 B 后，复用时更新保存的数据根。标准输出与错误持续写入日志文件，避免父窗口退出后 Rust `println!`/`eprintln!` 因管道断开触发 panic。保留原始子进程句柄，启动失败时终止并等待所属进程退出；无法取得可靠句柄时不能接管其他服务。手动连接外部 Host 的入口保留。

旧版偏好没有记录默认 `localhost/127.0.0.1:58080` 是自动保存还是用户主动输入。发现随附 Host 时，本轮按旧版默认的自管行为将该配置迁移为自动管理；需要继续连接固定地址的用户可在设置中手动指定。其他手动地址保持原设置。草稿和用户数据继续保留。

Devin 本轮只增加 `google.rpc.BadRequest` 字段的安全定位和请求形状摘要。协议字段路径采用允许列表；模型 UID 和工具 schema 仅提供哈希，另提供有界计数，不回显原请求、凭据或任意服务端字段描述。该证据用于后续定位被拒字段，不能证明 Opus 5.5 的服务端根因。温度等模型参数保持不变，也未使用用户账号重放请求。

可选提醒工具 `schedule_*` 在 Minimal、blank 和子代理中同时受目录与直接调用边界限制；自定义模式沿用组合所继承的能力。内置持久任务 `scheduled_task_*` 及其他既有工具保持合同。这是提醒权限修复，不是完整上游自动化协议替换。

当前本地已通过 CLI Web 7 项、Host 模式综合 3 项、Host `runtime_paths` 16 项、完整 API 126 项、数据根 resolver 1 项、Devin 127 项、提醒生命周期 6 项及独立 stdio 3 项测试。正式发行 Python 合同组合运行 164 项，163 项通过，1 项因本地 Linux 环境跳过 Windows 安装器控制流程；该组合包含此前的 48 项，不能相加为独立测试数。日志为 `/workspace/artifacts/v0.1.3-alpha.38-r7/validation/full-release-contract-tests.log`。缺失 `glib.pc` 的问题已通过工作区隔离开发 sysroot 解决，原生 launcher 41 项测试实际全部通过。

源码 `c8add2ef2a7220b2ff2b3a431bac101cb2c1b140` 的 debug Rust Host 已通过隔离开发 smoke：核对 `--port 0` 的实际 URL、PID、实例 nonce、数据根及 RPC；删除 ready 临时目录、关闭父管道后，Host 内部重启保留同一 PID、nonce 和端口，`RuntimePaths` 实例更新，持久日志仍可读。该环境复用已验证的 r6 核心资源并加载当前 Minimal/blank 预设，验证记录保存在 `/workspace/artifacts/v0.1.3-alpha.38-r7/validation/real-host-smoke/verification.json`。这是隔离开发验证，尚未验证正式新包；四平台正式发行环境的真实 Host 启动用例仍未运行，旧定点工作流跳过的 real Host 步骤不计为通过。

源码 `7c12e0b2eaa3e40b5909a23ca5d2fc92b7a6f298` 的[本轮 CI](https://github.com/qiu7824/deepseek-harness-rs/actions/runs/37189619840)中，Windows stdio、九组定点回归和严格金图通过，两种 Mac 架构的严格金图也通过。ARM 完整 Flutter 套件在步骤 10 失败，已定位首次草稿 scope 不可见及新增静态分析 lint，正在修复并等待完整套件复验。本地没有 Flutter SDK，不能以本地 Rust 结果或此前 `483b9963` CI 替代当前完整 Flutter 结果。实机字体、外部模型服务和最终新包公开下载仍分别验收；当前 r6 下载链接保持不变。
