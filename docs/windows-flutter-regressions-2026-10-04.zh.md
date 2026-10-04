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

## r8 修订的实现与验证

r8 纳入 `483b9963` 的 UI/错误分类修复和 `ae7bf6a8` 的相关说明，以及 r6 之后的桌面启动、Devin 诊断、提醒权限、草稿与连接切换修复。r7 正式 Windows 回归失败，未公开发布，标签保留 `41ddc4155309e73bd801d8209153c6a5613507f6`；后续修复采用 r8。README 对应 r8 发行线，旧 r6 包保持原标签源码；新增验证和正式包身份分别核对。

自管桌面 Host 现在以 `--port 0` 交给系统绑定端口，不再先探测空闲端口再启动。CLI 用一次性私有就绪报告返回实际地址、PID、实例 UUID 和实际数据根；launcher/Flutter 核对所属进程及探测到的 Host 身份，接受经过验证的数据目录重定向。同一 Host 进程由数据根 A 迁移到 B 后，复用时更新保存的数据根。标准输出与错误持续写入日志文件，避免父窗口退出后 Rust `println!`/`eprintln!` 因管道断开触发 panic。保留原始子进程句柄，启动失败时终止并等待所属进程退出；无法取得可靠句柄时不能接管其他服务。手动连接外部 Host 的入口保留。

旧版偏好没有记录默认 `localhost/127.0.0.1:58080` 是自动保存还是用户主动输入。发现随附 Host 时，本轮按旧版默认的自管行为将该配置迁移为自动管理；需要继续连接固定地址的用户可在设置中手动指定。其他手动地址保持原设置。草稿和用户数据继续保留。

首次手动设置或启动 Host 时，将尚未归属 Host 的草稿迁移到目标作用域，目标已有草稿优先，原草稿键保留；已有 Host 归属的草稿不跨 Host 迁移。慢自动启动和归属设置保存若被新手动连接取代，旧操作只清理自己的子进程，不改写新连接。错误展示只识别有界 Rust 诊断后缀，将其移入折叠详情；真实 HTTP 400 的分类和重试语义不变。

Devin 本轮只增加 `google.rpc.BadRequest` 字段的安全定位和请求形状摘要。协议字段路径采用允许列表；模型 UID 和工具 schema 仅提供哈希，另提供有界计数，不回显原请求、凭据或任意服务端字段描述。该证据用于后续定位被拒字段，不能证明 Opus 5.5 的服务端根因。温度等模型参数保持不变，也未使用用户账号重放请求。

可选提醒工具 `schedule_*` 在 Minimal、blank 和子代理中同时受目录与直接调用边界限制；自定义模式沿用组合所继承的能力。内置持久任务 `scheduled_task_*` 及其他既有工具保持合同。这是提醒权限修复，不是完整上游自动化协议替换。

r7 准备期间本地已通过 CLI Web 7 项、Host 模式综合 3 项、Host `runtime_paths` 16 项、完整 API 126 项、数据根 resolver 1 项、Devin 127 项、提醒生命周期 6 项及独立 stdio 3 项测试。Python 发行合同组合运行 164 项，163 项通过，1 项因本地 Linux 环境跳过 Windows 安装器控制流程；该组合包含此前的 48 项，不能相加为独立测试数。日志为 `/workspace/artifacts/v0.1.3-alpha.38-r7/validation/full-release-contract-tests.log`。缺失 `glib.pc` 的问题通过工作区隔离开发 sysroot 解决，当时原生 launcher 41 项本地测试全部通过。这些保留为历史结果，不代表 r8 新增修复或 Windows 门禁已通过。

源码 `c8add2ef2a7220b2ff2b3a431bac101cb2c1b140` 的 debug Rust Host 已通过隔离开发 smoke：核对 `--port 0` 的实际 URL、PID、实例 nonce、数据根及 RPC；删除 ready 临时目录、关闭父管道后，Host 内部重启保留同一 PID、nonce 和端口，`RuntimePaths` 实例更新，持久日志仍可读。该环境复用已验证的 r6 核心资源并加载当前 Minimal/blank 预设，验证记录保存在 `/workspace/artifacts/v0.1.3-alpha.38-r7/validation/real-host-smoke/verification.json`。这是隔离开发验证，不能替代正式新包和跨端桥接验证；定点工作流因环境跳过的真实 Rust Host 步骤不计为通过。

早前 `7c12e0b2eaa3e40b5909a23ca5d2fc92b7a6f298` 的[回归运行](https://github.com/qiu7824/deepseek-harness-rs/actions/runs/37189619840)中，定点回归和严格金图通过，但 ARM 完整套件暴露首次草稿 scope 不可见及新增 lint。后续修复产品逻辑并补充连接竞争测试，原失败草稿测试保持原文；该次失败不改写为成功。

前序源码 `f99bb89c765c34a2e32cf4e43b14fed1fd21b61a` 的[Flutter CI](https://github.com/qiu7824/deepseek-harness-rs/actions/runs/37191213764)四个作业全部成功，实际步骤如下：

| 验证范围 | 结果与耗时 |
| --- | --- |
| Windows 后台 stdio | 通过，2 秒 |
| Windows Dart 与十个定点测试文件 | 通过，60 秒；包含诊断折叠、草稿、定时界面及连接竞争 |
| Windows 严格金图 | 通过，8 秒 |
| ARM 完整 Dart / Flutter / Analyze / 正式客户端构建 | 全部通过，分别 19 / 353 / 27 / 160 秒 |
| ARM / Intel Mac 严格金图 | 全部通过，分别 25 / 74 秒 |

终态记录保存在 `/workspace/artifacts/v0.1.3-alpha.38-r7/monitoring/f99-final-report.json`。本地没有 Flutter SDK，上述 Flutter 结论来自该次远程 CI；此前 `483b9963` 结果也保留其历史来源。定点运行的真实 Rust Host fixture 因环境跳过，不能据此宣称真实 Host 与 Dart 桥接已通过。

### r7 正式回归失败

[r7 正式运行](https://github.com/qiu7824/deepseek-harness-rs/actions/runs/37192360300/job/111407050868)的 Windows“Host 与启动器回归”步骤失败。公开日志明确为 `tests::host_verification_uses_the_recorded_port_origin_and_instance_identity`：该启动器测试组 39 项通过、1 项失败，原本应为真的身份验证得到 false。生产包构建和公开发布受此门禁阻止，r7 未公开发布。

原假 HTTP 夹具只读取请求头后关闭带 POST 正文的连接，存在触发 RST 的风险；原断言又通过 `is_ok()` 隐藏了具体验证错误。源码补修完整消费有界 Content-Length 的 POST/RPC JSON 后再响应，增加有界 accept/read/write，并保留成功、错 PID、错 nonce 三组原强断言和原始验证错误诊断。潜在 RST 仍不能表述为已确认的该次失败根因。

独立产品边界改为对现存程序路径作 Windows canonicalize 比对，支持普通及合法 verbatim 盘符/UNC，拒绝设备命名空间；不手动剥离路径前缀，不削弱 PID 创建时间与实例 nonce。新增回归核对真实 canonical/current_exe/系统进程身份及错 PID、时间、程序路径，和原精确失败夹具分别验证。新增 [Windows 启动器定点工作流](../.github/workflows/launcher-regressions.yml)运行完整 launcher bin 测试并保存日志，不筛选、忽略或删除原失败用例。

上述补修已提交为 `64d353d3e35bbf3aa790750a3f51b435c4d7b083`。该源码在本地 Linux 完整 launcher 测试中 41 项通过，launcher/runtime Python 合同 31 项通过，`cargo fmt --all --check` 通过；新增两项 Windows 专属边界测试未在 Linux 执行。

同一源码的[Windows 定点作业](https://github.com/qiu7824/deepseek-harness-rs/actions/runs/37194868952/job/111414532428)全部成功。完整命令为 `cargo test --locked -p dsh-launcher --bin dsh-launcher -- --test-threads=1`，测试步骤 131 秒，格式检查 9 秒，整个作业 223 秒。公开页面未展示成功 stdout，实际通过数量未知，不按源码统计推定实际通过数量。成功作业覆盖完整命令，包含原失败用例及 Windows 新边界用例；这不等同 r8 四平台正式回归或新包验证通过。

正式发行要求四个平台完整回归、真实 Host 启动及跨端桥接、组包与摘要校验全部成功后发布；资产来源和公开下载另以正式运行与发行验证记录核对。用户机器的字体效果和 Devin / Opus 5.5 外部服务仍需分别复验。
