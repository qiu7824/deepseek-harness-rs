# 官方 dsh-v0.2.1-alpha.1 与 Rust 版本的推进评估

本次对照官方 [`dsh-v0.2.1-alpha.1`](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.2.1-alpha.1) 的发行说明及源码提交 `5badb15009ae1756c3afe0ae0cef1faafc290ccc`，Rust 基线为 r6 的 `7d8bbf4d44c2d04d1b582927af35ae97b0e5c3a6`。重点是此次相对 `dsh-v0.2.0-rc.2` 的新增和破坏性变化；历史评估中的数量、协议完成度和日期结论不作为本次证据。

这是一份源码评估与开发验收计划，未运行全量 Cargo、未编译上游端点清单，也未更换正式网页。**P0** 是进入适配前必须守住的兼容与权限边界；**P1** 是可独立交付的功能和体验改进。文件存在、静态字符串匹配与已有功能外观均不能独立证明端到端兼容。

## 此次变化的实际覆盖范围

下表保留 r6 标签的对照基线；后续源码改造及尚待完成的验证见末尾“r7 候选源码进展”。

| 官方变化 | Rust 基线状态 | 证据与需要推进的部分 |
| --- | --- | --- |
| 实验 Claude Code Mods | **不可直接迁移，尚无该兼容层** | 官方明确旨在验证 API 能力关系，未承诺完整实际兼容。其 `defineMod`、事件 hook、Host 操作和 UI Remote 依赖 Node/Cordis 服务。Rust 纯 Web 插件及外部 Claude Code 子智能体是不同能力，不能据此宣布支持 Mods。先做接口与生命周期设计，再决定 Rust 事件桥或隔离 sidecar；默认不开启。 |
| 插件页“让 Agent 创建插件” | **部分覆盖** | Rust 已有 `cordis` 创造预设，但其职责是 Rust 预设与 Skills，明确不假定动态 Host JS 可运行。当前插件页的 `createPluginController` 管理安装操作，不能等同官方“保留草稿进入创造模式、用户发送后才执行”的入口。可复用预设选择和新会话流程补入口，须说明可创建的插件类型。 |
| 新会话预填、恢复和跨工作区草稿引用 | **部分覆盖** | Rust Web 有会话草稿、图片 ID 及跨工作区转移；基线仍以 `clipboardDraft()` 字符串配合 `restoreDraft()` 转移文本。官方采用带文件/目录/会话身份、文本区间与失效标记的 `DraftSnapshot`。需验证语义引用身份，而不只检查显示文本仍在；Flutter 草稿使用自己的合同，分开验收。 |
| Web `--public-url` 与代理子路径 | **待实现完整合同** | 当前 Rust CLI/Host 未提供该参数，账号请求等仍使用站点根路径。官方区分监听地址、对外展示/浏览器/模型地址及可信 Host；公开 URL 是广告地址，不扩大信任。应同时统一 HTTP、WebSocket、静态资源、附件和回调的地址解析，不能只改启动时打印的 URL。 |
| 桌面使用系统分配端口 | **底层已覆盖，桌面启动协议不同** | Rust WebServer 可监听端口 0，CLI 也会把实际 readiness URL 回填；但原生 launcher 固定 58080，Flutter 默认同一端口且 `LocalHostProcess.start` 明确拒绝端口 0。官方 Desktop Host 以 `--port 0` 启动后，经 IPC 返回实际认证 URL。差距在桌面地址发现、进程所有权、复用和重启生命周期，不能只更换默认数字；CLI 显式固定端口继续保留。 |
| 自动化纳入 Web、提醒工具按模式提供 | **部分覆盖，组合与模式协议不同** | Rust 的持久 `scheduled_task_*` 任务与可选 `dsh-schedule` 的 `schedule_*` 提醒是两个域；后者默认停用，启用后按 root agent/非子代理挂载，尚无 Standard/Creator/PTC 与 Minimal 的预设判断。该差异需模式 E2E 进一步确认，不能宣称已实测 Minimal 越权。官方已组合 Web Host/UI 并按模式提供提醒；Rust 旧提醒记录目前只提示用 `schedule_create` 重建，不能称自动无损迁移。迁移时保留原数据并给明确处理结果。 |
| 子路径插件从 exports 提供文本、图标 | **协议不同** | Rust 安装器与客户端发现目前以包根 `package.json`、`dsh.client` 和 `exports["./client"]` 识别纯 Web 插件。官方子路径元数据由对应导出提供，不能直接用子路径名再找独立 manifest。需先定义兼容层及缺失导出的诊断，保持路径、符号链接和固定源码身份校验。 |
| 移除运行时 invariant 插件及 `./invariant` 导出 | **默认配置未发现直接依赖；扩展需审查** | Rust 内部 `invariant.rs`、断言和安全验证不是该 JS 诊断插件，不能按名称删除。迁移上游 JS 模块或外部 profile 时需检查旧 import/entry，给明确失效提示；暂不宣布第三方扩展已全部兼容。 |
| 输入统计 `stats` 拆为 `activity`、`usage` | **协议不同** | Rust Web 基线仍向 `conversation.composer.dock` 注册 `id: "stats"`；官方由 ui-chat 分别注册 `activity` 与 `usage`。需迁移扩展 ID 和组合位置，并保留 Rust 对未披露缓存读数、请求计时及回合统计的语义。视觉拆分不等同统计合同迁移。 |
| 大量会话的列表性能与调度机会 | **已有相关机制，尚无等价性能结论** | Rust 有固定元数据读取、metadata cache、投影缓存及有界历史；目录发现使用异步 I/O/阻塞任务。但仍逐会话发现和验证 generation。官方列表在完整行之间主动 yield。不能把 JS 的 yield 代码原样移植，也不能凭缓存存在宣称更快；应测列表冷/热延迟和并发操作响应。 |
| 登录未响应时的网络提示 | **部分覆盖** | Rust 已区分可重试网络/HTTP 失败并提供账号恢复流程；官方 `SignInDialog` 明确按 `errorCode: no-response` 显示网络检查文案。需验证未响应、过期、拒绝、取消和已成功登录分别展示，保留重试/关闭时的世代与取消边界。 |

其它 UI 变化包括 Markdown YAML 头部、目标/队列多行及 IME、停止恢复后再停止的队列、插件样式所有权、列表加载动画、包来源和实际安装版本、工具准备进度。当前只确认 Rust 存在相关页面或机制，**本轮尚未逐项封板**。应作为独立小改动与复现用例推进；不要由“页面已经有”推断上游修复已经覆盖。

## 先守住协议，再迁移网页

Rust 已有 [`dsh-typert-protocol`](../crates/core/typert-protocol/src/lib.rs)，实际提供 `RemoteError`、名称校验和 lookup/host-context 注册等子集。源码说明完整 Remote/registry/codecs 仍属于后续里程碑；本次检查未找到 Rust `RemoteGateway` 或 `RemoteRequester` 实现。已有 [`apiproxy` 消息类型](../crates/host/apiproxy/src/api/rpc.rs) 和浏览器的 gateway 资源不能代替完整 Host 接线。

因此官方网页、ui-chat、插件管理和 Mods UI 均不能作为“拷贝构建资源即可升级”的工作。P0 应从实际构建得到的模块入口、导出及 Remote 描述清单出发，与正式 Rust dispatch、事件流、取消、会话身份和错误信封对照；本轮未生成该清单，不报告推测出的精确端点数量。保留 Rust 的分页、阅读锚点、实时缓冲、附件所属会话、凭据隔离和源项目只读约束。

知识库等 Rust 独有入口还需在新客户端中保留并适配，不能因官方界面没有同名模块而删除现有能力。

## 分阶段交付与验收

| 优先级与阶段 | 可独立交付的工作 | 通过条件 |
| --- | --- | --- |
| **P0-A：先完成当前 Windows 问题闭环** | 本轮错误展示、Devin Connect 分类、账号/侧栏/字体改动单独验证；完整记录源码与安装包身份。 | 原始错误可读，详情默认折叠并脱敏，成功业务 JSON 保持数据语义；结构化协议码不被统一改成 400；Windows widget/golden/交互与真实字体复核分别给证据。源码修复与新包发布状态分开。 |
| **P0-B：网页与插件协议基线** | 从真实编译产物生成入口/导出/Remote 对照，再选最小模块适配；先冻结既有资料和会话格式。 | 每个迁移模块的导出、调用参数、结果、事件、取消和会话权限都有双端 fixture；不得以空对象、无效成功或忽略错误填补不支持能力；旧数据可读，完整包资源与源码身份通过。 |
| **P0-C：草稿与模式权限** | 定义可兼容旧字符串的语义草稿；明确提醒工具的模式和父子代理边界。 | 恢复、工作区切换、失败/取消和重启后保持引用身份；失效引用有提示；预填不发送；其它会话附件不被草稿元数据授权；极简和子代理无法绕过工具注册限制，已有定时任务保留。 |
| **P0-D：独立桌面启动可靠性** | launcher/Flutter 的自管 Host 使用系统分配端口，并由受管进程回传实际 readiness URL；保留显式连接已有 Host 的入口。该任务可独立推进，不等待完整 RemoteGateway。 | Windows 保留端口、多实例、启动失败、关闭/重启及外部 Host 复用有回归；只停止所属子进程，认证/监听地址校验和启动超时保持有效；不依赖“先探测空闲端口再绑定”的竞态流程。 |
| **P1-A：插件创建与统计入口** | 补创造入口；拆分 activity/usage；子路径元数据、旧扩展 ID 和 invariant import 给迁移诊断。 | 进入创造保留草稿且不启动执行，发送仅触发一次；停用/卸载不移除其它插件样式；单独覆盖一个统计入口不影响另一个；不宣称动态 Host JS 已可运行。 |
| **P1-B：部署与性能** | 统一 public URL/子路径；测并优化会话列表公平调度。 | 带路径代理下静态资源、HTTP、WebSocket、附件和登录流程均通；展示地址不扩大 trusted-host；列表优化有冷/热、并发/取消和损坏资料场景对照。 |
| **P1-C：实验 Mods 可行性** | 给 hook/Host 操作/UI band 定义权限、取消、卸载和支持范围；选择一个受控示例验证。 | 生命周期映射、hook 超时、工具改写权限、存储隔离、卸载清理均有证据；未服务事件和缺失 Host 操作明确失败。通过单个示例仍只标实验子集。 |

实际下一步是完成 **P0-A 的实机复核**、**P0-D 的当前源码 CI 与跨端启动验收**，并行整理 **P0-B 的实际产物清单**及提醒模式边界证据。桌面启动任务不受完整 RemoteGateway 里程碑阻塞。随后推进语义草稿，再交付插件创建入口等 P1 小功能。此计划不把全部上游包一次性纳入当前发布范围。

## 证据入口与本轮状态

- 官方实验说明与实现：[Claude Code Mods](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/packages/experimental/claude-code-mods/src/index.ts)；[创造入口](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/packages/client/ui-agent-preset/src/client/CreatePluginMenuItem.tsx)；[语义草稿](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/packages/client/ui-conversation/src/client/draft.ts)。
- 官方部署、统计和性能：[public URL](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/packages/bundle/web-app/src/public-url.ts)；[activity/usage](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/packages/client/ui-chat/src/client/apply.ts)；[列表让出调度](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/packages/api/session-controller/src/list.ts)；[登录网络提示](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/packages/client/ui-settings-account/src/client/SignInDialog.tsx)。
- Rust 基线：[创造预设](../config/agent-presets/cordis/preset.yml)、[Web 草稿和统计](../web/dist/plugins/ui-conversation.js)、[客户端发现](../crates/host/dsh-host/src/client_plugins.rs)、[纯 Web 安装边界](../crates/host/dsh-cli/src/native_plugin.rs)、[会话目录与元数据](../crates/session/session-persistence-jsonl/src/index.rs)、[投影缓存](../crates/session/session-projection-cache/src/index.rs)、[账号请求边界](../web/dist/plugins/ui-settings-models.js)。
- 桌面与自动化：[官方 Desktop Host](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/apps/desktop-host/src/index.ts)、[官方 Web 组合](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/packages/bundle/web-app/cordis.patch.yml)、[官方提醒子代理拒绝](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/packages/schedule/tool-schedule/src/index.ts)；Rust [Flutter 端口配置/启动](../apps/desktop_flutter/lib/src/preferences.dart#L147)、[launcher 默认端口](../crates/host/dsh-launcher/src/main.rs#L50)、[可选提醒的注册和旧记录提示](../crates/schedule/schedule/src/host_plugin.rs#L90)、[默认停用配置](../crates/host/dsh-host/src/client_plugins.rs#L163)。
- 本轮 [Windows Flutter 问题排查与源码修复](windows-flutter-regressions-2026-10-04.zh.md) 已完成 122 项后端测试、22 项产品/打包合同的本地验证；Windows CI 的完整 Dart 客户端、七组桌面回归及三个平台的严格图标金图通过；完整 Flutter 测试和静态分析也已通过。实机字体与真实模型服务复验仍待完成。r6 标签对应的安装包不会因后续 main 源码修改而更新。本次未使用账号凭据重放 Devin / Opus 5.5 请求，错误分类/显示修复不代表外部服务恢复。

## r7 候选源码进展

本轮在 r6 之后推进独立的 P0-D 桌面启动和 P0-C 提醒权限子项；当前仍未发布新包，README 下载保持 r6。原始基线及上述历史 CI 不改写为新修订的结果。

- **桌面端口和所有权**：launcher/Flutter 的自管 Host 使用 OS 分配端口，CLI 通过一次性私有报告返回实际 URL、PID、实例 UUID 和数据根。客户端核对实际 Host 身份及合法数据目录重定向，同一进程迁移数据根后更新复用记录；标准输出与错误写入持久日志，避免父窗口退出后断管道触发 panic。保留原始子进程句柄，启动失败时终止并等待所属进程退出；手动连接外部 Host 保留。旧版随附 Host 的默认 58080 配置迁移为自动管理，需要固定地址可在设置中手动指定。这是 Rust 启动协议的独立修复，不等同完整 RemoteGateway 或官方桌面 IPC 协议迁移。
- **提醒模式边界**：Minimal、blank 和子代理限制可选 `schedule_*` 工具的目录与直接调用，自定义模式按组合继承已有能力；内置 `scheduled_task_*` 持久任务保持原合同。模式范围不由预设名称的允许列表推断，工具出现、直接调用及切换模式后的生命周期须分别验证。此进展不改变旧提醒记录需明确处理的迁移结论。
- **Devin 诊断**：新增允许的 `BadRequest` 字段路径、计数及模型 UID/工具 schema 哈希，不回显原请求或任意服务端字段描述。温度等模型参数未改，未确认 Devin / Opus 5.5 外部服务故障的根因；该工作为定位提供证据，不能宣称供应商兼容问题已经解决。

本轮本地已通过 CLI Web 7 项、Host 模式综合 3 项、Host `runtime_paths` 16 项、完整 API 126 项、数据根 resolver 1 项、Devin 127 项、提醒生命周期 6 项及独立 stdio 3 项测试。完整发行 Python 合同组合 164 项中，163 项通过，1 项因本地 Linux 环境跳过 Windows 安装器控制流程；已涵盖此前 48 项，不能重复计数。日志为 `/workspace/artifacts/v0.1.3-alpha.38-r7/validation/full-release-contract-tests.log`。`glib.pc` 缺失已通过工作区隔离开发 sysroot 解决，原生 launcher 41 项测试实际全部通过。

`c8add2ef2a7220b2ff2b3a431bac101cb2c1b140` 的 debug Rust Host 在隔离开发 smoke 中通过实际端口、PID/nonce、数据根及 RPC 核对；移除 ready 目录并关闭父管道后，内部重启保持 PID/nonce/端口，更新 `RuntimePaths` 实例且持久日志仍可读。该环境使用已验证 r6 核心资源与当前 Minimal/blank 预设，记录为 `/workspace/artifacts/v0.1.3-alpha.38-r7/validation/real-host-smoke/verification.json`。正式四平台发行环境的真实 Host 用例仍未运行，旧定点 real Host 的 skip 不作为通过，也不能将隔离开发结果称为正式新包验证。

候选 `7c12e0b2eaa3e40b5909a23ca5d2fc92b7a6f298` 的[本轮 CI](https://github.com/qiu7824/deepseek-harness-rs/actions/runs/37189619840)已通过 Windows stdio、九组定点回归和严格金图，以及 Intel/ARM Mac 严格金图；ARM 完整 Flutter 套件在步骤 10 失败，首次草稿 scope 不可见及新增 lint 正在修复，完整套件待修复后复验。本地没有 Flutter SDK，历史 `483b9963` CI 不替代本轮结果。源码实现、模式 E2E、跨端启动和公开下载分别留证；r6 下载保持原标签，新包尚未发布。
