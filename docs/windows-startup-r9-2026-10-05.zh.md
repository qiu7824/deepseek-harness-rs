# Windows Flutter 启动修订与 r10 发行记录

记录日期：2026-10-05（Asia/Shanghai）。本记录针对 r8 安装版的 `Bad state: 本机服务在启动完成前退出（退出码 1）`，记录故障证据、源码修复、r9 正式发行失败以及 r10 的验收范围。

本记录使用的已公开故障基线为 [`v0.1.3-alpha.38-r8`](https://github.com/qiu7824/deepseek-harness-rs/releases/tag/v0.1.3-alpha.38-r8)，源码提交 `da1c20992f9d66d84ee179cf1c8b00d997729df1`。本记录在建立 r10 正式标签前更新；最终 r10 正式包验证及公开下载结果以对应 GitHub Release 和维护工作区的标签核验产物为准，不回写或移动已发布标签。此记录不表示已经取得用户机器上的完整服务错误。

## r8 正式发行状态

r8 的[正式发行作业](https://github.com/qiu7824/deepseek-harness-rs/actions/runs/37195438620)覆盖四个平台并成功发布。公开资产已完整下载，主核验和独立复核均为 PASS：

| 范围 | 已完成结果 | 边界 |
| --- | --- | --- |
| Windows x86_64、Linux x86_64、macOS x86_64、macOS aarch64 | 各平台正式发行门禁成功 | 不等于所有用户配置或硬件实测 |
| 13 个应用包与 `SHA256SUMS.txt` | 全量下载、长度及 SHA-256 核验；14 个公开资产摘要一致 | 安装包下载完整不等于实际安装后的客户端启动成功 |
| 8 个便携包 | 压缩完整性、构建清单、运行时及对应 Host 身份核验 | 不替代生产安装器冷启动 |
| r8 标签源码 ZIP | 3178 个文件与完整 Git 导出逐文件一致 | 源码 ZIP 不含编译程序 |

维护工作区的证据为 `artifacts/v0.1.3-alpha.38-r8/validation/release-target.json`（`COMPLETE`）、`downloads/verification.json`（`PASS`）和 `diagnostics/final-download-audit.json`（`PASS`）。公开成功页面不展示全部测试 stdout，此记录不推断实际测试通过数量。

## 已定位的故障路径

### 就绪文件的 Windows 长路径边界

Host 先用 Rust `OpenOptions` 创建 `.dsh-ready-<UUID>.tmp`，再用 Win32 `MoveFileExW` 将它发布为 `ready.json`。r8 前者采用 Rust 的 Windows 长路径处理，后者直接编码原始普通路径。r8 正式 Windows Host 的 PE 未包含 `RT_MANIFEST` 或 `longPathAware`；原生发布调用没有显式扩展路径。

临时文件名为 51 个字符，可能在最终 `ready.json` 仍较短时已经超过传统 `MAX_PATH`。例如父目录为 215 个 UTF-16 单元时，带末尾 NUL 的临时路径为 268 个单元，最终就绪路径为 227 个单元。该不一致有源码、Rust 1.97.1、微软 API 文档和正式可执行文件的静态证据，发布失败会进入明确的退出码 1 分支。**Windows 原生回归已验证该边界及修复**，但没有证据说明用户的 TEMP 正好触发该边界。

修复规范化已经存在的父目录，保留原始 `OsStr` 文件名，以系统给出的扩展盘符或 UNC 路径调用原生移动。目标尚未存在时不直接规范化目标；继续采用禁止替换的移动标志，并拒绝 NUL、异常文件名和备用数据流路径。

源码 `b7df802409d435954f976a5b7c63d1ad8bc6541d` 的 [Windows 作业](https://github.com/qiu7824/deepseek-harness-rs/actions/runs/37223677512/job/111498868247)成功；命名步骤“Windows 启动信息长路径与不覆盖发布回归”显示成功，耗时 1 秒。该步骤用 `rustc --test` 直接编译生产 `web_readiness_publish.rs`，以 `/MANIFEST:NO` 排除测试程序长路径声明的影响。测试强断言原始普通路径移动失败，再验证修复辅助函数的真实文件发布、读回、不可覆盖及清理；失败断言没有忽略或跳过分支。公开页面不能读取成功 stdout，本记录只引用精确源码和命名步骤结论，不声称观察到了测试通过数量。该结果验证原生辅助函数，仍不能代替 r9 正式 Host 与实际安装后客户端的组合验证。

### 生产 r8 安装后的长 TEMP 故障已复现

验证器源码 `7c8dcd47af90c3c4b3b83ee9270d2db2a48c91c4` 的 [Windows 实际安装基线](https://github.com/qiu7824/deepseek-harness-rs/actions/runs/37224439106/job/111501060858)执行公开的原 r8 生产安装器并运行实际安装目录中的 GUI。公开失败注解给出了完整的长 TEMP 诊断；**此处已经是 Windows 正式旧包安装后的复现，不再仅有静态推断**。

| 可读取证据 | 结果 |
| --- | --- |
| 实际安装的包身份 | 原 r8 源码 `da1c20992f9d66d84ee179cf1c8b00d997729df1`，安装内容核验通过 |
| 实际 Host SHA-256 | `7be37bf4a39823d3fbd628d7bd73eee637ef161ea79727c8c0dae2208a0a96fd` |
| 就绪文件父目录长度 | 229 个 UTF-16 单元，已观察到实际 GUI 创建的 `dsh-host-ready-*` 目录 |
| GUI 所启动的原 Host | 实际退出码 1，未保存新的 `ownedHost` 就绪记录 |
| 基线故障匹配 | `expectedBaselineFailure: true`，匹配 r8 的就绪发布长路径故障 |

服务日志原文为：

```text
dsh: desktop Host starting
dsh: readiness publication failed: The system cannot find the path specified. (os error 3)
```

同一轮的 `legacy-r6` 用例已报告 `bootstrapVerified: true` 和 `aliveAfterBootstrap: true`。该用例只有旧偏好和空数据根，名称不代表使用已有 r6 会话历史，也不建立历史数据迁移验收。

**该次作业整体为 FAIL。** 长路径复现完成后，后续旧 Core 占用测试在 `Conflict owner must be the clean, published r6 Core` 处失败；验证器把 r6 标签对象 SHA 错用为源提交 SHA。源码 `8cd4230be93c8481424df4c3405cd4b2736f0bf8` 已修正为真实 r6 源提交，其完整新基线已在后述作业通过。不能把已取得的单项故障证据写成完整安装基线通过，也不能把受控 Windows CI 故障直接归因为用户机器的情况。

### Flutter 丢弃早期启动错误

r8 Flutter 排空 Host 的 stdout/stderr 却不保留内容。日志重定向之前的参数校验、日志打开或初始化失败可以退出码 1 结束，此时服务日志可能不存在或为空。成功启动后才保存的 `ownedHost` 也不能提供该次失败日志位置。因此原提示只有退出码，用户无法从提示区分故障阶段。

修复保留有界的 stdout、stderr 和 Host 日志尾部，失败时携带本次日志路径、退出码与原始异常。日志优先，早期管道错误作为补充；诊断使用有界缓存和有界等待，避免大量输出或后代进程继续持有管道阻塞失败处理。启动成功后仍持续排空管道，停止累计诊断。新诊断、可读提示和脱敏折叠详情已通过 Windows Flutter 定点回归，ARM 完整测试、静态分析及正式客户端构建也已通过，正式安装包另经发行门禁验收。

### 旧服务仍持有同一数据根

已用下载的 r8 正式 Linux Core、隔离数据目录和 Flutter 的启动参数复现：第一个 Host 就绪后，第二个 Host 使用同一数据根退出码 1，日志为 `该数据目录正在使用，请先关闭其它 Harness 实例`。这是正式 Rust Host 的数据锁与错误流证据，**不是用户 Windows 机器的复现结果**。

r6 偏好没有 r8 的 `ownedHost` 身份记录。新桌面进程不能仅凭固定 58080 端口认定一个旧进程属于自己；安装器的停止逻辑也只针对被替换安装路径下的随附 Host，另一路径或独立 Core 可能继续持有共享数据根。修复应显示具体占用原因，不应删除锁文件、清空数据或终止未知进程。

上述审计和隔离复现保存在维护工作区 `artifacts/windows-startup-exit-r8/`，包括 `audit.zh.md`、`reproduction.json`、`diagnostics/windows_stdio_api-audit.md` 和 `diagnostics/installer-paths-audit.md`。现有证据不足以把任一候选认定为本次用户故障的唯一根因。

## 已完成 CI 与仍待验证范围

上述 Windows 作业中的原“Windows 后台 Host 标准输出句柄回归”也成功；该轮 Windows、macOS Intel 和 macOS ARM 三个金图作业全部成功。该轮 ARM 构建包含的是本次新诊断修改之前的 Dart 代码，因此不能把这些结果写成本次新 Flutter 代码已通过。

新诊断源码 `71b35fd5aba80a08aec3fc37f7af3c5677672a4f` 的 [Windows 定点步骤](https://github.com/qiu7824/deepseek-harness-rs/actions/runs/37224167975/job/111500275752)“Windows 会话错误与侧栏交互回归”已完成且成功。该步骤运行 `dynamic_host_startup_test.dart`、`error_presentation_test.dart` 与既有定点文件，包含新增 Node 启动错误夹具及呈现测试。公开成功 stdout 的数量仍不推断。该源码的 [ARM 作业](https://github.com/qiu7824/deepseek-harness-rs/actions/runs/37224167975/job/111500275059)中，完整 Flutter 测试（489 秒）和静态分析（33 秒）已成功，正式客户端构建（217 秒）也已成功。该源码四个 CI 作业全部完成且成功；这些结果不代替正式 Host 和实际安装器组合验证。

真实已发布 r8 安装器的基线工作流始于源码 `f20b9447579f00740a7a3aeb6066c62db1dc76bd`，见 [Windows 安装版启动诊断工作流](../.github/workflows/windows-desktop-startup.yml)。后续 `7c8dcd47` 已取得上述正式旧包安装后的长路径复现，但整体因验证器身份常量错误失败；修正后的 `8cd4230be93c8481424df4c3405cd4b2736f0bf8` 的[完整重跑](https://github.com/qiu7824/deepseek-harness-rs/actions/runs/37224554059/job/111501407054)已完成且成功，无失败步骤：下载核验步骤 5 秒，实际生产安装器及 GUI 诊断门禁 99 秒。相同源码的验证程序对正常启动、旧偏好启动、required 旧 Core 占用与释放后重试、精确匹配旧 r8 长 TEMP 故障和卸载进行强断言，整个门禁成功。公开成功 stdout 未显示用例数量，不编造数量。该基线针对旧包，长 TEMP 的预期故障被验证为原 r8 缺陷，不是正式新包验收；r10 正式安装门禁要求新包长 TEMP 启动成功，不使用旧包基线标志。标准账户和只读安装树的实际验证仍没有充分证据。

## r9 正式失败与 r10 修订

r9 正式源码为 `88690a2bc2b7c0df63695e9e8fb582a1f232bd19`。其 macOS Intel“Agent 与预设生命周期回归”门禁返回通用退出码 101，r9 未公开发布，标签不移动。该通用错误没有充分揭示原始失败原因，不能据此认定产品逻辑错误、超时或环境波动。

[精确源码 Intel 诊断](https://github.com/qiu7824/deepseek-harness-rs/actions/runs/37226385390/job/111506786918)固定上述 r9 源码，执行同样的九条原命令并全部成功。该结果说明这一轮诊断未复现原错误；原正式失败原因仍未知，不能把独立诊断成功写成 r9 正式门禁成功，也不能代替 r10 的正式发行。

r10 使用新的不可变标签 `v0.1.3-alpha.38-r10`，保留已有 Windows 与 Flutter 启动修复。新增正式生命周期失败的完整日志保留及 CLI 就绪文件回归，原测试命令、筛选条件和其他门禁全部保留。当前进入构建发布流程；四个平台的构建作业、发布作业和实际 Windows 安装后启动验证必须全部通过。r10 最终源码 SHA、正式运行编号和产物结果将在对应 Release 与维护工作区标签核验中记录，本记录不预填尚未取得的结果。

## 建立 r10 标签前的验收记录

| 验证 | 本记录状态 | 验收要求 |
| --- | --- | --- |
| Windows 原生就绪文件发布 | 已通过，源码 `b7df8024`，命名步骤成功 | 普通长路径、短目标配长临时文件、中文空格、合法原始 UTF-16 文件名；原调用失败强断言、实际文件读回、不可覆盖及临时文件清理；不替代正式 Host 组合验证 |
| r8 生产安装后的长 TEMP 故障 | 已复现，验证器 `7c8dcd47`；作业整体 FAIL | 实际安装包与 GUI、就绪父目录 229 个 UTF-16 单元、原 Host 退出码 1、服务日志错误 3；修正验证器后的完整基线已通过 |
| Flutter 启动失败诊断 | Windows 定点、ARM 完整测试、静态分析及正式客户端构建已通过，源码 `71b35fd5` | 日志未创建、日志不可打开、stdout/stderr 早期错误、大量输出、UTF-8 边界、管道继续被后代持有；保留原因且不挂起 |
| Flutter 错误呈现 | Windows 定点、ARM 完整测试、静态分析及正式客户端构建已通过，源码 `71b35fd5` | 对象异常与持久字符串均显示可读原因；详情折叠、凭据脱敏；业务 JSON 识别和已存在服务错误保持正确 |
| 生产 Flutter 安装器冷启动 | r8 完整诊断基线已通过，源码 `8cd4230b`；r10 正式新包须验收 | 执行实际安装 EXE，运行安装目录客户端，验证 GUI、Host、数据根及随机实例身份；不改写生产安装脚本 |
| 安装与升级边界 | 待完成 | 中文空格安装目录、长 TEMP、安装树只读；旧偏好与另一路径服务占用时保留旧进程和数据，释放自建占用后可重试 |
| Windows 用户权限范围 | 待确认 | 明确执行账户权限；管理员运行的结果不能标为普通用户实测 |
| 四平台构建、发布及安装启动验收 | 待完成 | 保留现有完整回归、真实 Host、组包及摘要门禁，增加实际安装后启动门禁 |
| r10 公开下载 | 待完成 | 新标签固定源码，逐个下载实际公开资产并验证摘要和构建清单；源码与标签一致 |

验证环境使用隔离数据根、偏好及临时目录，只清理验证器创建并核对身份的进程和文件。既有 r8 资产、用户数据、账号设置及未知服务保持原状。源码 README 的当前发布线与链接使用 r10，并明确资产仅在本次全部正式门禁通过后公开；最终公开下载核验应在对应发布说明正文和工作区证据补充，不移动或改写已发布标签。

本次启动修订不改变 [dsh-v0.2.1-alpha.1 上游评估](upstream-dsh-v0.2.1-alpha.1-evaluation.zh.md)中的能力结论，也不建立 Devin / Opus 5.5 外部服务或真实字体问题已解决的证据。
