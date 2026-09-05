# DeepSeek Harness Rust 实现与兼容状态

> 基线：`@deepseek-ai/dsh-root 0.1.0-rc.5`
> 源码（只读）：`D:\HermesTemp\deepseek-harness`
> Rust 项目：`D:\deepwork\deepseek-harness-rs`

## 当前产品边界

当前发布线为 `0.1.3-alpha.6`。Rust Host 使用独立的有界双向历史窗口、目标页跳转、context-jump、原生启动器与主题扩展。运行时接口、存储格式和视觉效果按 Rust 产品契约维护。

Session 保持 JSONL/Zstd v0 格式和稳定事件坐标；投影缓存使用逐记录 v5。未声明对新版 Node Session v1/v2 工件的导入支持。支持的协议、入口及限制以 README 和协议矩阵为准。

## 0. `0.1.2-alpha.4` 同步说明

- Session projection cache 使用 per-record v5，兼容 v3/v4；坏记录与未来版本显式上报，派生缓存先备份再重建，权威数据保持 fail-loud。继承身份包含 `isSeeded` 与 `inheritedEventCount`，删除后的记录不会在重启时从旧布局重新导入。
- 图片能力未知时允许请求到达 Provider，明确不支持图片时本地拒绝；动画 PNG/GIF/WebP 在预算内保留媒体类型和完整字节，超预算返回明确错误。
- 发布矩阵验证正式包的模型设置持久化、凭证格式、监听边界和图片请求链路。
- Session 坐标已拆分为 `SessionSeq` 与 `SessionLogOffset`，事件身份和日志间隙不再混用；历史读取改为显式有界快照/窗口。
- 逻辑 Session 头使用 `is_seeded`，继承事件数量独立携带；JSONL/Zstd v0 物理头仍兼容缺失、零和非零 `seedLength`。
- continuable Agent 的模型消息统一为 `agent-message`，`send_message({ agent_id, message })` 只允许直接父子双向投递；旧 report 体系不再作为正式路径。
- Rust Host 的模型发现可在服务端安全复用 Profile headers；浏览器候选列表支持搜索和可见结果选择。
- Rust 原生 `web_fetch` 已接入公开 HTTP(S) 抓取，并对 scheme、DNS/IP、逐跳重定向、超时、响应体和取消执行 fail-closed 边界。
- PTC/code 默认不公开通用 `workflow` 工具，但 workflow engine 仍供 Ralph/其他预设使用。
- 正式发布分为 `core`、`skin`、`free`，由四平台 GitHub Actions 生成便携包、安装包和最终校验和。

## 1. 规模与范围

自动统计文件：`docs/porting/loc.json`（脚本 `docs/porting/count-loc.mjs`）。

| 指标 | 数值 |
| --- | ---: |
| workspace/package 记录 | 241 |
| 全仓源码行 | 237,817 |
| 全仓测试行 | 293,638 |
| Host/后端及共享基础包（排除纯浏览器包） | 约 192 |
| Host/后端及共享基础源码行 | 161,745 |
| Host/后端及共享基础测试行 | 206,707 |

宏阶段 3 最终验收快照：`cargo test --workspace` 共 428 个 test-result 分组， **1885 passed / 0 failed / 1 ignored**。最终日志 `C:\Users\Administrator\AppData\Local\Temp\dsh-rs-macro3-workspace-final-v3.log` 的 SHA-256 为 `46799e45386e6aacbe8e08fe1928d8d0eefc14d9f7fe6c94147adcfdf3442e58`。 MCP 已具备真实 stdio、loopback streamable HTTP、模型工具注册、schema/命名、一次 受控重连及有界进程树清理；LSP 已具备有界 Content-Length framing、四类查询、 transient document、canonical workspace 单飞进程池和模型工具。正式 `dsh.exe` 承载 SDK JSON-RPC 与 ACP stdio 入口：上游 Python SDK 已通过生产 Host 完成真实 turn， ACP prompt、assistant 更新、整 Agent idle 和立即 cancel 均有产品 E2E。默认 spawn subagent 与出进程 Codex provider 已接入；Codex 使用预解析绝对路径，拒绝工作区 shim 劫持。DeepSeek transport 对成功流施加总量预算，模型取消会关闭真实 TCP；远端明文 MCP/DeepSeek 均在网络 I/O 前 fail-closed。最终独立短复核为 `passed: true`，无 P0/P1。 按可运行入口、核心运行时、Host API/CLI、wire/存储兼容、平台安全与发布切换 加权审计，当前可记账真实移植完成度约 **86%（保守区间 83%–89%）**；该估算不以 文件数、LOC、轮次或清单勾选率代替行为兼容。

本项目的"后端"范围包含：

- Cordis、loader/include/hmr/timer/schemastery/cosmokit；
- app boot、profile bundles、CLI/composition 引导；
- agent/session/LLM/tools/system prompt/compaction；
- storage/fs/workspace/spill/sandbox/subprocess/terminal/jobs/schedule；
- settings/credentials/attachment/feedback/hooks/guards；
- goal/plan/todo/skill/subagent/workflow/MCP/LSP/ACP/extensions；
- Host apiproxy/webserver/frontend-static/directory-picker/plugin-inventory；
- SDK/stdio JSON-RPC、Python SDK runtime、Linux landlock launcher；
- 原有 Web GUI 的静态托管和后端 wire 协议。

## 2. 权威盘点文档

| 分组 | 文档 | 内容 |
| --- | --- | --- |
| A 基础运行时 | `docs/porting/inventory/group-a-foundation.md` | cordis 生态、core、boot、bundle，共 22 包 |
| B 类型/API | `docs/porting/inventory/group-b-types-api.md` | typert/util/api/sdk/preset/identity/settings/session-query，共 25 包 |
| C Host 外壳 | `docs/porting/inventory/group-c-host-infra.md` | apiproxy/webserver/static/picker/CLI/Web，含 56 条路由 |
| D 会话/模型 | `docs/porting/inventory/group-d-session-llm.md` | session/llm/context/compaction/interaction/attachment/feedback/hooks/guard，共 40 包 |
| E 执行/沙箱 | `docs/porting/inventory/group-e-execution-sandbox.md` | sandbox/subprocess/fs/workspace/storage/spill/jobs 等，约 44 包 |
| F 功能特性 | `docs/porting/inventory/group-f-features.md` | goal/skill/subagent/workflow/MCP/LSP/ACP/extensions/python/native |

`loc.json` 是包清单与行数权威；上述文档是导出面、服务、事件、工具、存储格式和依赖顺序权威。

## 3. 兼容性验收标准

### 3.1 Cordis/组合

- Context child/extend/isolate/intercept 作用域一致；
- service provide/get/set、same-scope 冲突、active 可见性一致；
- inject 缺失→PENDING、服务出现后自动加载、实现变化后卸载/重载；
- Fiber 状态：PENDING→LOADING→ACTIVE / FAILED→UNLOADING→DISPOSED；
- effect 先注册、逆序清理、可重入/单次释放、异步 setup/dispose 可 join；
- event emit/parallel/serial/bail/waterfall、filter/global/prepend/once 一致；
- `internal/*` 生命周期事件与 update/config waterfall 一致；
- loader include/patch/interpolate/HMR 与现有 cordis.yml 行为一致。

### 3.2 数据与协议

- Host API 52 条一致 RPC + 2 SSE + download + respond 的路径、信封、错误码一致；
- Session JSONL/zstd/SQLite 字节/事件兼容，可读写既有数据；
- storage JSON/SQLite、spill、attachments、settings/credentials 路径与格式一致；
- stdio NDJSON JSON-RPC SDK 与 Python SDK 兼容；
- LLM OpenAI/DeepSeek/pi-ai 协议、流、block assembly、retry/token metering 一致。

### 3.3 安全与系统

- Sandbox 模式和升级：`read-only / workspace-write / danger-full-access`；
- Windows AppContainer + 临时 ACL package SID，失败不透传；
- Linux bwrap/landlock、macOS seatbelt 选择与探测 fail-closed；
- subprocess 进程树、PTY、kill、stdio/spill 行为一致；
- 凭证脱敏、路径边界、批准栈与会话所有权授权一致。

## 4. 阶段计划与进度

### M0 — 建仓与全量盘点

- [x] 创建独立项目 `deepseek-harness-rs`；
- [x] 复制 `apps/web` → `web/`（排除 node\_modules，保留 dist）；
- [x] 复制 CLI config、examples、LICENSE/第三方声明；
- [x] 建立 Cargo workspace 和工具链固定；
- [x] 统计 241 个 package 的源码/测试行数；
- [x] 完成六组包级依赖、导出面、格式、难点盘点。

### M1 — Cordis 生态底座

- [x] `cordis`：Context / Service / Reflect / Registry / Fiber / Events / Logger；
- [x] cordis 核心行为测试（13 项：插件生命周期、依赖注入与变更重载、service 提供/隔离、

      events 五种分发、effect 幂等清理）；

- [x] `cosmokit`（`crates/vendor/cosmokit`，crate `dsh-cosmokit`，15 项测试全绿）；
- [x] `schemastery`（`crates/vendor/schemastery`，crate `dsh-schemastery`，21 项测试全绿；

      20 节点类型、meta 链、simplify/i18n/toString、Standard Schema V1；       toJSON/fromJSON 留待 M2 settings 持久化）；

- [x] `timer`（`crates/vendor/timer`，crate `dsh-cordis-timer`，9 项测试全绿）；
- [x] `loader`（`crates/vendor/loader`，crate `dsh-cordis-loader`，12 项测试全绿；

      **插件模块解析改为静态注册表，**`!!js`** 表达式显式报错（留待嵌入式 JS 运行时）**）；

- [x] `include`（`crates/vendor/include`，crate `dsh-cordis-include`，13 项测试全绿）；
- [x] `logger-console`（`crates/vendor/logger-console`，7 项测试全绿）；
- [x] `group`（`crates/vendor/group`，loader 的 GroupPlugin 注册别名）；
- [ ] 对照上游 `vendor/cordis` TS 测试建立 conformance fixture；
- [ ] `hmr`（按计划裁剪后置）；
- [ ] profile patch、interpolate、entry tree 与状态投影（patch/interpolate 已落地，profile 编排待 app-boot）。

当前 Rust 代码：`crates/vendor/{cordis,cosmokit,schemastery,timer,loader,include, logger-console,group}` + `crates/core/{scope,llm,typert-protocol,session,system-prompt,agent,agent-default-model,tools,agent-tool-presentation,agent-loop}` + `crates/session/{session-persistence,session-persistence-jsonl,session-persistence-sqlite,session-projection,session-projection-cache,session-stats,session-telemetry,session-title,session-title-llm,session-title-all-prompts-llm,session-title-first-prompt-llm}` + `crates/settings/settings` + `crates/llm/{llm-retry,token-meter}` + `crates/storage/{storage,storage-domain,storage-json,storage-sqlite,storage-test-support}` + `crates/workspace/workspace` + `crates/spill/{spill,spill-local,spill-policy}` + `crates/credentials/{credentials,credentials-local}` + `crates/sandbox/{sandbox,sandbox-policy}` + `crates/fs/{fs,fs-local,fs-observation-policy,fs-sandbox}` + `crates/subprocess/{subprocess,subprocess-local}` + `crates/terminal/terminal` + `crates/shell/{shell,bash-local}` + `crates/code-runtime/code-runtime` + `crates/jobs/{jobs,jobs-local}` + `crates/goal/goal` + `crates/util/{brand,timeout,atomic-write,home-paths,output-retention,invariants,launch-environment,native-command}` + `crates/session/session-checkpoint-policy` + `crates/context/time-context` + `crates/guard/{repeat-tool-reminder,timeout-policy}` + `crates/identity/anonymous-user-id` + `crates/todo/tool-todo` + `crates/attachment/{attachment,attachment-local}` + `crates/session/session-query` + `crates/context/session-reference` + `crates/interaction/commands` + `crates/feedback/command-feedback` + `crates/feedback/message-feedback` + `crates/host/dsh-host`（可启动主程序） + `crates/compaction/compaction` + `crates/interaction/{user-questions,tool-ask-user,user-approval,permission-presets}` + `crates/skill/{skill,tool-skill,skill-badge,skill-filesystem}` + `crates/plan/plan-mode` + `crates/session/session-query-sqlite` + `crates/schedule/schedule` + `crates/subagent/{subagent,subagent-fork,subagent-spawn,tool-subagent}` + `cargo test --workspace` 1440 项全绿（第 30–33 轮 storage Hub/domain/json/sqlite 落地； 第 34 轮 dsh-workspace 落地：49 项测试，workspace.spec.ts + invariant.spec.ts 全部移植； 第 35 轮 spill 三包落地：spill-local 11 项 + spill-policy 16 项； 第 36 轮 launch-environment 7 项 + credentials seam 9 项； 第 37 轮 credentials-local 落地：49 项测试，local/drain/watcher/review-fixes 四个 spec 全部移植（行级注释保留 YAML 编辑、分层解析、写锁读改写、包含扇出、fake watcher 管道、真实 notify 热重载）； 第 38 轮 dsh-sandbox seam 落地：15 项测试，vocabulary/escalation/roots 三个 spec 全部移植（严格加宽梯、参数配对校验、模型面 marker、approveEscalation 有序 fail-closed 序列、canonicalPath/writableRoots 派生）； 第 39 轮 dsh-fs seam 落地：12 项测试，service.spec + invariant.spec 全部移植 （FsTargetKey/FsVersion 品牌、FsInfo/FsPathInfo/FsDirEntry 词表、FsError 码表 + cause 链、FileSystem 抽象服务 14 原语、internal/dispatch 事件数据不变式）； 第 40 轮 dsh-fs-local 落地：20 项测试，fsio.spec + filesystem.spec 核心子集移植 （realpath 身份、祖先回退解析、探针/列表、UTF-8 严格读/跨块流、字节上限、 字面编辑+行尾往返、私有 staging 原子发布、hard-link 守卫创建、per-target 锁 并发写/编辑确定性）； 第 41 轮 dsh-fs-observation-policy 落地：18 项测试，policy.spec 全部移植 （观察态 gate 的 write/edit 意图派生、present→absent→present 转移、多 owner 隔离、单槽 first-wins 短路、dispose 状态释放与监听器移除）； 第 42 轮 sandbox-policy + fs-sandbox 落地：21 项测试（政策服务默认/会话解析/ 审批覆盖优先级/sandbox-mode 会话套件/事件不变式 + 包含判定矩阵 + 每调用 策略栅栏：只读拒绝、工作区包含、`..`/symlink 逃逸拒绝、TOCTOU 方向重解析、 升级覆盖）； 第 43 轮 dsh-subprocess seam 落地：4 项测试，service.spec 全部移植 （完整 spawn 词表：三态 stdin/输出模式、有界收集 + spill、offset 读取器、 树级终止语义、terminal 原语六法、scrubbedParentEnv 双擦洗）； 第 44 轮 dsh-subprocess-local 落地：14 项测试，spawn.spec + local.spec 核心 子集移植（childEnv 擦洗合并/Windows 大小写折叠、OutputCollector 字节精确 尾窗 + 惰性 spill 溢出丢弃、全隔离进程树 spawn、SIGTERM→grace→SIGKILL 升级、abort 谓词反应、批次 stdin、可执行解析、服务注册/释放与 fiber 处置 终止整树；spawnTerminal 桩留待 PTY 里程碑）； 第 45 轮 dsh-terminal 落地：23 项测试，service.spec 全部 23 例移植 （后端注册精确贡献释放、owner 栅栏、spawn 发布/回滚、调用者取消、 owner/服务处置对未发布 setup 的 abort+await、后端侧清理失败保留至处置、 关闭幂等与聚合、处置清注册表；invariant 伴生 no-op 注册）； 第 46 轮 dsh-shell + dsh-bash-local 落地：28 项测试（shell seam 7 项—— render.spec 退出标记解析契约 + service.spec 抽象执行器桩；bash-local 21 项——executor.spec 前台运行/超时/abort 分类/stdin-env 线程/后台进程句柄 增量读/损失标记/spill 路径/kill 升级/失败结算 + settings.spec 设置段 user 层解析/写入校验/存储段服务/供应商脱落回退/无供应商入口/命名空间 释放）； 第 47 轮 dsh-code-runtime 落地：9 项测试，reserved.spec + service.spec 全部移植（可移植标识符排除集：RESERVED\_BINDING\_GLOBALS/RESERVED\_ERROR\_ MEMBERS/DUNDER\_MEMBER/PORTABLE\_RESERVED\_WORDS 全仓共享契约 + 抽象 CodeRuntime 桩：language/isolation/run 三原语、失败为结果字段、预中止 abort 失败、fiber 卸载移除、重复注册 fail-loud）；worker-thread 后端 （bootstrap/worker-json，需嵌入 JS 运行时）留待后续里程碑； 第 48 轮 dsh-jobs + dsh-jobs-local 落地：19 项测试（jobs seam 5 项—— service.spec + invariant.spec 全部移植：抽象 JobRegistry 九法、JobId 品牌、快照跨字段不变式 + jobs-inject 安装器；jobs-local 14 项—— jobs.spec 核心子集：入站预检（无控制器/空 kind/空 label/非法 outputLimitBytes/owner 未注册）、按 kind 顺序 id、每 owner 并发上限、 session 栅栏、流式/终态读与 reported 标记、kill 两态、首胜结算 + 监听 通知、有界 wait（结算/超时/中止）、teardown 取消与抛错强制失败、 owner 处置取消并删除记录）； 第 49 轮 dsh-goal 落地：12 项测试，goal.spec 核心子集 + 严格折叠套件 （事件溯源 goal/change 全量快照 + 清除墓碑、CAS revision 比对、create/ edit/pause/resume/complete/block/clear 七动词 + 阶段梯校验、目标边界 校验（objective/maxGoalRounds/blockReason）、进程本地 activation （armed/disarmed + session-start disarm 边）、round 准入折叠（user/ message goal 源）、goal/changed scoped emit、strict 解码器 fail-loud （坏版本/坏目标/字段漂移/跳过 revision/预算耗尽））；@Remote 注解与 投影单元注册留待 typert/投影集成； 第 50 轮 dsh-native-command + dsh-session-checkpoint-policy 落地：8 项 测试（native-command 4 项——无 shell 执行器 utf8 捕获/非零退出 code/ ENOENT/abort 传播 + Windows console hide；checkpoint-policy 4 项—— 语义持久化检查点：llm/stream 工厂包装（flush 先于首块、失败 fail-closed 终态块）、tools/execute 顶层检查点 + 预分派 abort 规范结果、agent/ pre-step 边界 flush）； 第 51 轮 dsh-time-context 落地：20 项测试（timestamp/request-zone/ index/invariant 四个 spec 核心全量——ICU 级 IANA 规范化（jiff + 内置 tzdb 2026c 链接表 + CLDR Etc/UTC 折叠）、ISO 形时间戳格式化、浏览器 请求时区派生（resolved/mixed/missing 排序去重）、preceding/latest 事件扫描、prepended pre-step 瀑布监听器 + snapshot 形式注入与 refresh 间隔抑制、fiber 处置移除监听器、纯函数不变式 + 增量历史缓存的 伴生注册（internal/dispatch 内联钩子在 append 持锁下运行，伴生自维护 会话历史避免锁重入）；MessageSource::User 扩展 rpcId/clientTimeZone 合并增强字段（线格式 skip-if-None 保持兼容）； 第 52 轮 dsh-repeat-tool-reminder + dsh-timeout-policy 落地：22 项测试 （repeat-tool-reminder 13 项——per-agent 连续重复链（deep key-sort 规范化 + JSON.stringify 整数格式、通配符字面转义、阈值 fail-loud 校验/升序归一、gentle@thresholds\[0\]→detailed 升级、include/exclude 透明谓词、用户 pre-step 重置、block/accept 决策折叠保留下游元数据）； timeout-policy 9 项——tools/execute 包装：无预算透传、派生 deadline 信号换入/还原、自有 TOOL\_TIMEOUT 结构化替换（协作工具与提供方 abort 错误）、上游先中止保留注册表 ABORTED、deadline 先赢保留超时、fiber 处置移除监听器；全局工具对无 agent 直调也可解析预算）； 第 53 轮 dsh-anonymous-user-id 落地：10 项测试，anonymous-user-id.spec + invariant.spec 全部移植（harness-home 作用域匿名身份：bare UUID 行持久化 到 `.anonymous-user-id`、缺失主目录递归创建、空白容忍、损坏覆盖、 wx 独占创建并发胜者采纳、只读 home best-effort 内存 id、按解析路径 进程级 memo、默认进程 env、空安装器伴生注册 + 包名保留 fail-loud）； 第 54 轮 dsh-tool-todo 落地：12 项测试（`todo_write` 工具全量： 注册 schema 形态、整表替换追加 todo/write、content trim 规范化、 单/并行 in\_progress 策略 + 描述文案差异、schema 级拒绝（未知键/ 坏状态/非数组）+ 值级拒绝（空/重复）、无 agent 拒绝、presentCall presentation、fiber 处置注销、`todos` 投影单元（整表折叠 + turn/start 清除 + 无关事件同引用）+ 持久化形状不变式伴生（trim/唯一/状态枚举）； 参数校验在体内用共享 JSON Schema 引擎执行——Rust 工具运行期尚未在 dispatch 前校验输入（已记录偏差））； 第 55 轮 dsh-attachment + dsh-attachment-local 落地：8 项测试（attachment seam 词表 + 错误码类 + 不可变存储抽象三原语（imageLimits/validate/ save/read + 谓词取消）；attachment-local 内容寻址后端：四格式光栅 解码（探测头 vs 全解码准入、像素上限先于解码）、sha256 对象存储 （对象/桶/暂存布局、独占创建 + 硬链接去重、EEXIST 冲突校验、临时 清理）、displayName 双分隔符剥离、嵌套 home 创建、失败封闭（缺失/ 损坏/非法引用/元数据不匹配/写失败映射稳定码）、abort 谓词取消、 服务边界默认限值 + 校验不落盘；POSIX 目录 fsync 顺序断言留待 （实现保留相同 sync 结构，Windows 无目录 fsync 可观测面）； 第 56 轮 dsh-session-query 落地：7 项测试（组合会话查询服务 seam： 17 码错误类 + 配置/游标品牌、字面大小写不敏感空白弹性文本过滤器 （regex 注入安全）、AND 会话/事件谓词（id/cwd/created-at/parent/ availability + seq/time/type/surface/text）、一阶语义文本抽取（消息/ 工具调用结果/todo/turn-end 分派）、规范化 surface 折叠分类 （current/shadowed/log-only）、live 优先逻辑语料（sessions 服务 + 可选 persistence 擦除绑定 + 头兼容断言 + 并发投影）、系谱/事件关系 追踪、标题折叠/表面读取/事件窗口等引擎原语 + 抽象搜索面 （sqlite 后端后续接入）；SessionPersistenceApi 增加 dyn 擦除服务注册 （fs 同款抽象服务约定）； 第 57 轮 dsh-session-reference 落地：6 项测试（跨会话快照引用服务： dsh-session: URI 规范化编解码（base64url of JSON 字符串 + 往返 canonical 校验）、Markdown mention 转义解析 + 裸 URI 提取、tag-safe JSON 序列化（< 转 u003c）、字节预算保留（最老非 checkpoint 优先丢 + 最长消息 head/tail 截断 + 省略通知）、prepare 流程（normalize 去重/ 自引用/上限、readSurface 快照、recall 形式插件源 + 不可信提示信封）、 候选排序（cwd 亲缘 + 标题标签 + 排除自身）；SessionReferenceSource 结构化源暂以 plugin 源表示（MessageSource 未扩展 session-reference kind——记录偏差）； 第 58 轮 dsh-commands 落地：8 项测试（插件所有的人类命令注册表： 斜杠行解析（名称/边界校验）、注册规范化 fail-loud（名称模式/描述/ 输入提示/重复名 panic）、ScopedLayers 分层 + 作用域遮蔽 + 名称排序 描述符、execute 生命周期（command/run + command/done 配对 commandId、 args 与 recordInput:false 省略、来源标注）、handler 失败/abort 结算为 error 记录并重抛、实例令牌前缀的配对 id 铸造、pairing 不变式伴生 （run 唯一 + done 配对 + sourceEventSeq 校验，增量历史规避 append 锁 重入）；handler 为 async Result 闭包（TS 同步/异步 throw 塌缩为 Err 通道——记录偏差）； 第 59 轮 dsh-command-feedback 落地：7 项测试（会话反馈域事件 + 人类 `/feedback` 生产者：注册（描述/输入提示/recordInput:false）、 确认文案 + 三种共享政策披露（sessionTelemetry 服务新增 dyn 擦除注册 补齐 TS ctx.sessionTelemetry 契约）、trim 规范化、空输入 usage 错误、 command/run → feedback/record → command/done 事件序、独立记录与 log-only 保证、反馈载荷仅出现一次）； 第 60 轮 dsh-host 可启动主程序落地：2 项测试（M6 骨架——核心服务 组合（sessions/agents/systemPrompt/tools/invariants）+ 包属不变式伴生 挂载 + 启动报告（服务清单/会话 seq/工具计数）+ `dsh-host` 二进制 实际运行输出报告 exit 0，目标「可启动运行」达成首个可执行产物； webserver/apiproxy/CLI 继续叠加）； 第 61 轮 dsh-message-feedback 落地：4 项测试（生命周期绑定的逐消息 评分侧车：storage-domain 域声明（行 schema 校验：评分枚举/uuid 版本/ 时间序/唯一 id+版本/非空白 note）、inspect 存活优先 + 快照目录存在性 权威、hasFeedbackTarget（append 源 assistant 消息派生）、版本门控 put/delete + 无变更 no-op + 冲突当前项回传、note-blank/too-large、 target/session-not-found、per-session 串行队列；域 open 在安装期 block\_on、关闭同步 block\_on（Domain close future 非 Send——记录偏差））； 第 62 轮 dsh-compaction 落地：4 项测试（抽象压缩服务 seam： CompactionId 品牌 + compact checkpoint 来源构造/谓词（MessageSource:: Plugin 扩展 compactionId/sourceCommandId 合并增强字段，线格式 skip-if-None 兼容）、ManualCompactionError 六码 + CompactionTrigger、 CompactionResult 词表、tool-pairing 平衡折叠（assistant tool-call 计数/结果递减/负余额与坏 seq fail-loud + 缓存按 session id 键控）、 compaction/start-summary-end 括号状态机不变式（重叠/无配对/ checkpoint 关联/回合围栏/seed 边界陈旧 start）核心子集；summary 邻接与影子定价交叉校验留待后续； 第 63 轮 dsh-user-questions + dsh-tool-ask-user 落地：4 项测试 （用户提问能力 seam：AskUserQuestion 词表（问题/选项/多选/意图）、 单一活动 UI provider 注册/重复 fail-loud/处置释放、ask 校验梯 （空问题/无 provider/意图 approve 标签与 detail/中止谓词/代理 liveness + roots 校验）、模型面 `ask_user_question` 工具（questions 投影到 answers、body 内 schema 校验、provider 往返）； SessionStore.create/fork 与 InvariantRegistry.register 改为显式 caller 参数修复 Proxy 重绑语义）。 第 64 轮 dsh-user-approval 落地：26 项测试（`ctx.approval` 一次性授权 seam：ApprovalOutcome/ApprovalPolicy 闭词表 + ApprovalRequestId 品牌、 approval/asked + approval/decided 审计对（开放回合围栏 + 可选字段省略 + 每请求新 id）、turn-enclosed 前置检查（空转/回合间拒绝且零追加）、 作用域瀑布分派（全局 + agent 作用域监听器、外国作用域永不听见）、 首答单槽 + next() 委托到 fail-closed 默认、同步/异步抛错回答器包含为 unavailable、非词表回答归一化、policy 折叠（最后 approval/policy 事件 + 配置默认 ask 回退）、never 策略在分派前确定性拒绝（注册顺序不可绕过、 回答器不被咨询）、会话覆盖双向压制配置默认、setPolicy 注入下次模型 步的切换通知（幂等、plugin 源）、approval:policy 系统提示上下文注册与 fiber 处置释放、invariant 伴生（asked/decided 按 id 配对 + 开放回合 围栏 + 政策/结果闭词表；增量 trace 规避 append 锁重入）； 策略上下文解析为 TS 无 agent 空分支（AssembleContext 尚未携带 agent—— 记录偏差）、回答器失败按监听器包含（不否决追加）；fs-sandbox 的 EscalationApprover 通道自此有真实 ctx.approval 服务可接）。 第 65 轮 dsh-permission-presets 落地：32 项测试（permission-presets.spec + invariant.spec + projection.spec 全部移植：preset 表词表（sandbox+ approval 绑定 + 可选 name/description）、保留名 custom 拒绝、非约束 执行器组合拒绝、derive 数学（共享绑定平局先取上次选中、陈旧折叠回退 表序、无匹配 → custom）、set 写链（permission/preset + 仅变化的 sandbox/mode + approval/policy 经规范 setter、当前值 no-op、漂移重选 修复单旋钮）、optionOf 标签回退/custom 固定/未知 fail-loud、settings 段默认 preset（defaultPreset union-of-consts schema + validate 钩子 + setSource 源 thunk + 未知存储值拒绝）、新会话 pin（session/created + 存量 list 双通道、seed 会话保留有效旋钮仅补缺失事实、空 seed 走组合 默认、legacy 缺失 policy 物化 ask）、`permissions` 投影单元（三旋钮 JSON 态折叠 + custom 仅当前追加 + change feed 每旋钮通知 + 无关事件 同引用零通知 + HMR 挂载/卸载键释放）、`/permission` 命令（set 写链 + setPolicy 活切换注入通知、裸调用报告当前值、未知 preset 错误记录 不动日志）；ApprovalService 补 config() 公开访问器（TS public config）； settings 布线 + 投影/命令两子 fiber 经 ready() 可结算）。 第 66 轮 dsh-skill 落地：30 项测试（skill.spec.ts 核心全量移植： provider 注册/处置 + 保留名 runtime 拒绝 + 工厂失败回滚、rank→ providerOrder→localOrder 三键去重 + 跨层最近层遮蔽（无视 rank）、 invocation 政策中立目录 + model/user 谓词独立、runtime 技能默认 provider/invocation + 层内 first-wins 重复告警 + no-op 处置、候选/ 定义校验（名称语法/非空描述/provider 归属、类型不变量在 Rust 为 编译期事实——记录偏差）、lookup options 借用（cwd + signal 指针 身份）、目录缓存（cwd+scope 链+revision 键、容量 LRU、不完整观察 不缓存、失败 provider skip+warn 且不可缓存、在飞失效重试上限 2 次留未缓存结果、晚到 invalidation 忽略）、skills/change 通知 （每次注册/处置/失效发射、监听器失败包含 + 告警）、中止竞速 （缓存后发现后加载前重查、不合作 provider 竞速、统一 SKILL\_ ABORTED 消息——谓词无 reason 载荷记录偏差）、定义身份漂移失效、 消失候选返回 None、加载失败传播、渲染（目录/URL/opaque/无基回退 四种资源提示 + 属性转义 + 正文逐字 + 转义函数）、作用域层 （scoped provider/runtime 归属层、链继承 + rebind 重挂、层内 provider 名唯一 + 作用域重复文案、处置掉层 + 通知、作用域 control 仅在注册存活期失效）；invariant 伴生 no-op（TS 同款）；workspace 成员列表新增 crates/skill/*）。 第 67 轮 dsh-tool-skill 落地：24 项测试（tool-skill.spec.ts 的 runtime 技能子集全量移植：*`skill`* 工具 schema/处置/重挂 + presentCall 形状、 首次步进稳定持久目录（按 digest 判重、描述规范化/截断/转义、来源 不泄漏 whenToUse/正文）、空基线/不完整发现跳过并在后续边界重试、 同一步提案目录去重/替换、陈旧提案在空基线前移除、匹配提案保留、 增删触发的完整替换目录与空墓碑、按持久 entries 恢复 + 外来 lookalike 不压制、压缩隐藏后重建目录、未知/非法/模型禁用技能错误、加载前 政策检查 + 加载后重查、provider 资源提示渲染（opaque/url/无基）、 描述上限校验、restrict 屏蔽与作用域同名遮蔽的目录门控（register\_arc 精确身份比对）、*`/name`* 手势注入（首 token/句中、路径分数边界拒绝、 未知/用户禁用保持普通文本、非 user 源不扫描 + 去重、下游 reject 透传、仅文本块）；dsh-llm MessageSource 扩展 SkillCatalog/ SkillInvocation 两个 kind + SkillCatalogEntry（线格式与 TS 一致）； dsh-tools 新增 register\_arc（注册指针身份比对）；invariant 伴生 no-op；pre-step 载荷无 signal（dsh-agent 偏差——查找无中止谓词）； skill-filesystem 依赖用例（cwd 项目技能/正文刷新）随该包后续落地）。 第 68 轮 dsh-skill-badge + dsh-plan-mode 落地：16 项测试（skill-badge 2 项——内置 *`dsh-badge`* 技能 provider 注册/列出/加载/处置 + 官方 PNG 字节不变（sha256 + IHDR 尺寸）；plan-mode 14 项——plan-mode.spec + invariant.spec 核心全量：plan/mode 折叠（最后者胜/无则 inactive/end 界）、配置校验（非空 section）、首标题提取、set 状态机（空闲提交 + 按最后 request/header 叙述注入、开回合排队 + 边界提交、相反选择 cancelled + 边界清除、noop）、exit\_plan\_mode 评审流（approve → approved + 静默选择下一边界提交、keep-planning 反馈、ASK\_ABORTED 驳回文案、非 plan 模式/无标题/无 userQuestions 渠道错误）、 *`/plan`* 命令（on 进入 + 非 off 消息 steer、off 退出）、*`plan`* 投影 单元（command/run 意图 + plan/mode 提交的双事件折叠 → {active, pending}）、plan/mode 布尔载荷 invariant 伴生；工作区成员新增 crates/plan/*；计划:policy 段 provider 解析为 TS 无 agent 空分支 （偏差）、评审驳回码塌缩为 ASK\_ABORTED（user-questions 偏差）。 第 69 轮 dsh-skill-filesystem 落地：7 项测试（skill-filesystem.spec 发现/解析子集：目录捆绑 + 扁平 .md 双形态发现（排序/来源/rank）、 目录资源基与正文加载、.system 目录跳过、YAML frontmatter 解析 （name/description/whenToUse/metadata + 调用政策禁用键与布尔词表 + legacy 键拒绝 + 缺失必填/非法名/无 frontmatter 的 warn-and-skip）、 git 根项目技能发现（cwd 敏感）、custom/bundled 根 + 稳定 rank、 缺失根空目录；notify 递归监视 + 去抖失效（chokidar 的祖先监视/ 轮询模式未移植——缺失根靠下一次发现拾取，偏差）；fs/observed 突变钩未接线（Rust 演员柄无工具名，偏差）；fs 服务存在时经 FsError 码表含缺失/非文本路径、无 fs 时 std 回退；invariant 伴生 no-op；技能生态四包自此完整（seam→provider→tool→badge）。 第 70 轮 dsh-session-query-sqlite 落地：25 项测试（query.spec 全量 + sqlite.spec 核心子集：config 默认/校验（path/openAt/页上限/片段 上限/并发）、startup/first-search/never 三种开启边界（never 不触 文件系统 + 继承读/追溯可用）、live-only FTS5 unicode61 两字符词 元搜索 + 全头部往返（cwd/seedLength/delegationDepth/agentPreset）、 推理内容排除/可见文本入索引、全 surface 默认搜索 + 元数据先过滤 后排名（seq/time/type/surface 组合）、短语词元 + 稳定并列序（match \_count 降/文档长升/time 降/session\_id 升/seq 降）、曲音/标点片段 定位（码点裁剪 + 空白归一）、游标绑定与失效（instance/scope/ fingerprint/generation 四键 + 偏移安全整数、目标会话追加→STALE、 请求指纹不符→INVALID、损坏偏移→INVALID）、持久源动态挂载/活 shadow/卸载揭示（erased 注册 + 指针身份 binding cell）、live 遮蔽 跳过 inspect（计数 0→detach 后 1）、快照变化重试（两次稳定观察

- 一次重试上限→PERSISTENCE\_FAILED）、live/persisted 头部冲突→

SOURCE\_CONFLICT、瞬时拓扑变化→STALE、FTS5 外层谓词预算 14 + 固定谓词、32766 便携绑定上限、重开丢 temp.live 保 persisted、 schema 版本/application\_id 守护）；rusqlite bundled FTS5 同步句柄 仅同步段持有（tokio 门闩序列化 + parking\_lot 无 await 持有—— rusqlite Connection 非 Sync）、可选持久化绑定改轮询 + inject 子 fiber 复位（挂载/卸载 identity 变化→epoch→cursor 失效，mount 竞速 容差）、Rust 持久化 API String 错误→统一 PERSISTENCE\_FAILED 包装 （无类型透传，偏差）、查询期 SQL 错误→INDEX\_FAILED（偏差）； 真实 sqlite 持久化后端注册具体类型与 erased 查询面注册互斥， 组合集成暂以 erased 假后端覆盖（偏差）；invariant 伴生 no-op。 第 71 轮 dsh-schedule 落地：25 项测试（domain.spec 全量 21 项 + runtime/ tools 核心 4 项：v1 变更解码（create/delete/dispatch + acceptedAt 可选项 + 精确键集合/版本/操作词表 + kind 判别）与冻结语义（Rust 值语义）、全部 畸形持久化数据拒绝表（26 例：空/错版本/未知操作/多余键/空白 id/非法 acceptedAt/记录形状错误/prompt 空白/after 0 与 1.5/every 299、300.5、 字符串、MAX\_SAFE（interval 安全整数检查）/非真实日历日期/五位数年份/ null schedule/kind 张冠李戴）、按创建序折叠 + id 复用/未激活 delete/ dispatch 拒绝、fork 后缀边界（seedLength 越界）、可读 id 分配防撞、 after 记录规范（trim/31s 目标/scheduled-overdue 视图/输入词表错误）、 注入防护 framing 逐字节（JSON 转义 id/prompt）、every 首个锚定目标 + 下限/安全整数/最新错过出现次选择 + 不可前置/NaN/区间溢出、无积压 推进 + 单向 dispatch 的 acceptedAt 约束、9999 边界终止 + 多记录批量 framing、严格偏移解析（±时区/毫秒 1-3 位归一）、非法偏移 10 例、 not\_future/time\_out\_of\_range 区分（now 与 epoch 双区间检查）、IANA 规范化（UTC/America/New\_York/US-Eastern 别名）+ 缩写/偏移/未知拒绝、 本地日历解析（Asia/Shanghai 毫秒、UTC、DST 重叠取第一瞬间、DST 空洞 invalid\_rule、9999 本地越界）、确定性 recurrence 性质循环 300 轮 （fast-check 属性同构）、runtime 驱动（one-shot 经 maintenance 边界 followup + dispatch 追加 + 折叠清空、every 批量 acceptedAt + 记录推进、 损坏流 faulted 不派发）、三工具（create 校验词表/选择器互斥/trim、 list 创建序视图、delete 真删 + not-found + 非法 id、跨 agent 内部 错误）；时间库 chrono-tz：正则 look-around 改写为显式 0000 前缀检查、 毫秒补零 `{:0<3}`（左填充 bug 修正）、JS 安全整数 = i64 值 ≤ 2^53-1 的 checked\_mul 过滤；时区别名仅内置常用 backward 表（ICU 全别名 未嵌入，偏差）；Agent::run\_maintenance 结果擦除 → 共享槽读回（偏差）； flush 需真实 session/flush 监听器（测试注册 no-op 确认器）； invariant 伴生按 dispatch 内联约束改用增量折叠 trace（锁内禁读 session.events，偏差）；workspace 成员新增 crates/schedule/\*。 第 72 轮 dsh-host M6 组合升级 + 持久化 erased 注册统一：① 两个持久化 后端（jsonl/sqlite）的 install 改为注册 erased `Arc<dyn SessionPersistenceApi>`（此前注册具体类型，与 session-query/ schedule/corpus 的 erased 查询面互斥——第 70 轮偏差修复；全仓无 get\_typed 具体类型消费者，零破坏）；② dsh-host 组合从 5 服务骨架 升级为 10 服务真实启动：invariants/sessions/agents/systemPrompt/ tools/commands/userQuestions/sessionPersistence(JSONL zstd)/ sessionQuery(SQLite FTS5)/schedule(函数插件 apply)；③ 启动报告新增 端到端探针：store 会话 append + flush 经 JSONL coordinator 真实持久化 （flushAcknowledged=true、快照数=2）、live 与 persisted-only 双源日志 分别被 FTS5 命中（各 1 条）；④ 挂载 session/schedule/session-query- sqlite/llm 四组 invariant 伴生；⑤ `cargo run -p dsh-host` 实测 exit 0 输出完整探针报告；boot 测试因组合内嵌 block\_on（同步安装器）改为 multi\_thread flavor（current\_thread 会死锁，测试头记录偏差）。 第 73 轮 dsh-subagent 契约层落地：6 项测试（descriptor.spec 契约子集： v2 描述符快照/往返（one-shot/continuable 双模式 + toolFilter allow）、 首个 descriptor 事件权威折叠 + 版本闸门（v1 不可分类 → None）+ 无事件 空日志、13 例畸形当前版本载荷拒绝表（非对象/缺 version/版本字符串/ 未知 mode/多余键/provider 非串/label 非串/continuable 缺 label/ agentProvider 非串/toolFilter 非对象/未知键/数组含非串/空对象）、 seed 暂存（Session::create + 单条模型隐藏 descriptor + end-seed、 seq 0、无 surfaceOp）、runId 品牌串透明；类型层全量：SubagentRunId 品牌、run/end 观察载荷、能力旗标、启动请求（signal=中止谓词—— 偏差：TS AbortSignal）、结果/运行/提供者 trait（prepareContinuable 默认拒绝实现对应 TS 可选方法能力）；AgentOptions 增 subagent\_depth 字段（TS module augmentation）；ToolRestriction 补 serde/PartialEq （descriptor 持久化需要）；error 码类；depth 单调地板（header 权威 + runtime 加深）；runtime/continuation/registry/backends/tools 留待 后续轮次（偏差）。 第 74 轮 dsh-subagent 服务核心落地：新增 5 项测试（service.spec 核心 子集：provider 注册/列表/get + 重名 DUPLICATE\_PROVIDER + 处置后 NO\_PROVIDER、能力旗标先于委派校验（maxDepth/persona 缺失 →  UNSUPPORTED\_CAPABILITY 且 provider 未收到启动）、一次性 start 全链路 （label 透传 + descriptor 快照解析 + 发布 run + 结果）、生命周期 start/end 事件对（父作用域 carrier 过滤派发 + 终局观察者、runId UUID 配对）、助理输出选择折叠（非空 assistant/message 替换流式 回退、text-delta 累积、空输入 None）+ settle\_run 三态（completed 带 文本/killed/refusal → failed 带 detail）；模块：assistant\_output （AssistantOutputFold + finalAssistantOutput）、run\_settlement （settleRun → JobOutcome 映射 + dispose 失败合并 detail）、lifecycle （LifecycleEdge + emit\_lifecycle\_edge 逐监听器包含 + observe\_run 终局配对）、index（SubagentRuntime 服务：providers 注册表 + start 能力闸门 + prepareContinuable 代理 + register\_provider 效应作用域）； continuation manager/listChildren/listDescendants/投影暂拒 CONTINUATION\_UNAVAILABLE/UNSUPPORTED\_CAPABILITY（偏差）； 生命周期载波 = 父 agent scope\_key（TS scopeTarget(service,parent) 的 Rust 近似，偏差）；dsh-subagent 合计 11 项测试。 第 75 轮 subagent 进程内驱动 + fork 后端落地：① dsh-subagent 新增 child\_agent（共享子代理组合：resolveChildDepth 上限/安全整数检查 + SubagentDepthError、resolveChildAgentOptions 父路由继承 + 请求覆盖 + 深度盖章、childSessionMeta（cwd/parentSession/origin subagent/ delegationDepth/seedLength；agentPreset 未移植 → None，偏差）、 SUBAGENT\_DELEGATION\_CONTEXT + applyChildComposition（delegation 上下文 order 120 + persona section order 0 + tools.restrict 作用域；preset composeFrom 未移植，偏差）、capture/appendDelegatedPolicyOverrides （sandboxPolicy.overrideOf + approval 存在即 never，source: delegation 双事件））；② in\_process 共享驱动（startInProcessRun： 中止预检 → 深度 → UUID 子会话 → 政策捕获 → agents.create → 发布后 组合（TS 在未发布创建窗内执行，偏差）→ drivePublishedRun（中止 移交/一次性 followup + whenIdle/readResult：foldConsumedWork 终局 + finalAssistantOutput + cancelled 覆盖 aborted；structured 捕获未移植 → 驱动 structured=None，偏差））；③ 新 crate `crates/subagent/subagent-fork`（dsh-subagent-fork-in-process：balanced completedTurnPrefix 切片（最后一个 turn/end 含、在飞回合排除、无完成 回合空）+ ForkInProcessProvider（capabilities 除 outputSchema 全开 ——结构化未移植 + inheritsParentContext）+ prepareContinuable 一次性 前缀捕获 + invariant 伴生 no-op）；fork 2 项测试（前缀三形态 + 注册/能力/上下文契约）；dsh-subagent 合计 11 项、subagent 组合计 13 项。 第 76 轮 subagent spawn 后端 + tool-subagent 落地：① 新 crate `crates/subagent/subagent-spawn`（dsh-subagent-spawn-in-process：fresh child 零种子 + prepareContinuable 空 spec + 能力旗标同 fork + invariant 伴生）；② 新 crate `crates/subagent/tool-subagent` （dsh-tool-subagent：模型面 `subagent` 工具——provider 上下文敏感 文案（inheritsParentContext 双版本 description/prompt 描述）、 foreground 全链路（start → settleForegroundRun：结果/处置双失败 合并 AggregateError 语义、非 completed → ToolBodyError + 部分输出 保留 `Partial output before the run ended:`）、后台 one-shot （JobStart kind=subagent + SettledRunHooks 封装 start 即 settleRun + cancel 置 killed、JobRegistry 缺失即错误）、enableRunInBackground 关闭时拒绝强制后台、maxDepth 能力先于挂载校验（provider 无 depthLimit → 挂载错误）、输出 schema 前景/后台 oneOf + render； `backgroundMode: continuable` 挂载即拒（continuation manager 未移植， 偏差）；`items: {type: json}` 输出 schema 在 Rust 校验器不支持 → 改 `{type: array}`（偏差））；tool-subagent 4 项测试（前台输出/ 处置、非 completed 错误 + 部分文本、后台 job id、禁用后台拒绝）。 第 77 轮 subagent 投影 + 子代理枚举落地：① `projection.rs`（subagentTiming 纯投影：turn 边界围绕子代理自有 descriptor 折叠——descriptor 重置 settled、pendingTurnStart 提升、turn/end 结算 settledMs、active 截 through 随每个事件推进（含 end-seed，与 TS 一致）；subagent 身份投影： descriptor 事件 last-wins、畸形/未知版本 → null 哨兵、stateVersion=2 带 seq；状态为纯 JSON ArcValue + 同引用零工作）；② `list_children.rs` （live-preferred 语料合并（persistence.list + sessions.list）+ subagentParents 集合、活子代理走注册表 watermark 快照（schema 失败 → corrupt 诊断）、冷子代理三阶梯（投影缓存 seq 门 → inspect 折叠 + sameLifecycle 见证键 → corrupt/unavailable 诊断）、后代稳定前序 + parentId/depth、createdAt-then-id 排序、取消检查点 CANCELLED）； SubagentRuntime 挂载两个投影单元（inject sessionProjections 子 fiber）+ listChildren/listDescendants 接入；③ dsh-subagent 3 项新测试 （timing 折叠含 end-seed through 语义、live 子代理按投影身份排序/ 创建窗省略/普通会话不解释、冷子代理 inspect + corrupt 诊断）；dsh- subagent 合计 14 项。 关键移植决策与偏差见 `docs/porting/cordis-rust-notes.md`、`docs/porting/schemastery-rust-notes.md`。

第 78 轮 subagent continuation 管理器落地：① `continuation.rs`（TS `continuation.ts` 移植：ContinuableStartSpec/Start、FollowupOptions、 ReportDelivery/Options、InterruptAuthority 契约 + SubagentContinuationManager ——startContinuable 全链：persistence 前置断言 → 深度校验 → 保留 UUID 子会话 → descriptor 快照 → provider prepareContinuable 贡献 → 信号取消 → descriptor seed（prepared.seed + descriptor 事件）→ ChildLock 逐子代串行化（lock\_owned Send 守卫）→ materialize（注册表 create + 委托策略追加 + 组合应用 + lineage/ancestry 指针集 + Activation 安装 + 所有权登记 + watchSettlement 观察者）；followup：resident 直投、未知子代 NOT\_RESUMABLE、exact-live-parent 双重校验（注册表 ptr\_eq + header 父会话）、child-lock 提交截止重试； interrupt：User/Ancestor 两权校验（ancestry 指针集）；reportFrom： Quiet→inject / Wakeup→followup + SubagentReport 源；drain：全量关停（根 激活发现 + 递归子先释放 + 失败聚合 ACTIVATION\_TEARDOWN\_FAILED + DRAINING 后拒新）；drainDescendants：exact-live 根作用域剪枝（roots 指针 + ancestry 过滤）；settlement：notifySettlement（Idle→followup / Running→steer + SubagentSettled 源 + boundContextSummary）；管理器监听 agent/disposed 清 closing scope）；② dsh-llm MessageSource 增 Coordinator/SubagentReport/ SubagentSettled 三变体 + kind() 覆盖；③ lifecycle.rs 增 ActivationTerminal/ActivationObserver/createActivationObserver（start 边界 快照 → capture（foldConsumedWork + finalAssistantOutput + epochStopReason） → terminal → settle 一次性 End 边）；④ SubagentRuntime 挂接（continuations OnceLock<Weak> + RuntimeContinuationHost（prepareContinuable 走 provider 能力）+ startContinuable/followup/interrupt/reportFrom/ drainContinuableDescendants 转发）；⑤ dsh-subagent 18 项新测试（start 身份/ 描述符/元数据、persistence 前置、非 continuable provider、准备期信号回滚、 深度拒绝、组合记录、工厂失败回滚、followup 顺序/未知/陌生父/同 id 替换、 settle→父通知 + subagent/end、drain 关停 + DRAINING 拒新、drainDescendants 森林剪枝、interrupt 双权、reportFrom 双投递、runtime 级转发）；dsh-subagent 合计 32 项。 偏差：冷恢复未移植（followup 非 resident 即 NOT\_RESUMABLE，TS 会从持久化 冷物化）；inbox claimed/discarded 记账未接线（settlement 观察者只看 status + owned\_children 静止，本侧测试以显式 teardown 触发）；创建窗 setup 注册表 未移植（materialize 在发布后组合，与 in\_process 共享偏差）；Activation 的 accepted 集合只增不减（无 inbox claim 清账回调，自动 settle 依赖后续接线）。

第 79 轮 dsh-tool-jobs 落地 + 三处基础设施修复：① 新 crate `crates/jobs/tool-jobs`（TS `tool-jobs` 全量：Config schema 校验 （schemastery number min 1 + union quiet/wakeup + 默认补全；apply 期 waitTimeoutMs>cap 拒绝、maxConsecutiveWakes 整数性拒绝——2.5/1e300 拒， JS Infinity 不可表示记录偏差）、publicJob/statusLine、TextRetainer 截断 族（fitWithSuffix/fitCompletionNotice 保留 job id + 收集动作尾）、三个 控制工具（job\_output：wait 钳制 min(timeout??default, cap) + consuming delta/终态输出 + finalize 保留 canonical status 分割；job\_list：注册序 渲染 + 空态；job\_kill：already-finished 非消费快照）、tools/pre-execute prepend 捕获 outputLimit（exec token 键控 WeakMap 塌缩）、 finalizeContent 对 pre/around/post 各阶段 policy 结果统一截断、 attachController + onJobDone（caller ctx 显式传递）完成通知（Idle→ followup 预算内、Running→inject、claimed 用户消息复位预算、reported 抑制 kill/wait 已报、teardown reported 抑制、exact-owner 直投）+ 系统 提示 tool:jobs 段 + 三个 Generic 呈现卡 + invariant companion；② dsh-jobs seam 重构：on\_job\_done/on\_jobs\_changed/attach\_controller 增 显式 caller Context（TS Proxy 重绑塌缩——修复第 48 轮遗漏，scoped mount 测试暴露：listener/controller 此前恒挂 global 层）；③ jobs-local 注册 erased `Arc<dyn JobRegistry>`（get\_typed 查询面修复）+ list 按 单调 ordinal 注册序（此前 HashMap+同毫秒 startedAt 排序不稳定）+ onJobDone/onJobsChanged listener 包含与 teardown cancel 抛错改走 logger.warn（TS 通道）；④ dsh-scope `scope_of` 沿 fiber 父链上溯 + 环 检测（root 自父绑定）——scope ctx 下挂载插件的子 fiber 现在解析到包围 scope，systemPrompt.section/tools.register 的 scoped 层因此正确； ⑤ dsh-tool-jobs 42 项测试全绿（TS 33 例 + 分步断言适配：watch channel send\_replace 存值（spawned done 驱动尚未 subscribe）、wait 同步前缀 futures::poll! 驱动、cancel settle 注入、1e300 替代 Infinity）。

第 80 轮 dsh-tool-str-replace-editor 落地（M4 收尾项校验：TS 仓库并无 credentials-encrypted/OS-keychain 包——README 明言 deferred，该项自 backlog 移除）：① 新 crate `crates/fs/tool-str-replace-editor`（TS `tool-str-replace-editor` 全量：view（cat -n 行号 + view\_range 校验/ \[start,-1\] 尾部 + 截断标记）/create（存在即拒 + write-intent 瀑布）/ str\_replace（唯一性匹配 + FS\_EDIT\_NOT\_FOUND/FS\_AMBIGUOUS\_EDIT 行号报告

- 字面替换）+ insert（行界校验 + 单次 join 语义——三段 join 会多出尾

换行，本轮实测修正）+ list（2 层深、隐藏/node\_modules/\_\_pycache\_\_ 过滤

- 路径排序）+ MutationPolicy（sandbox\_mode 能力 → sandboxPolicy 缺失

启动期拒绝、per-call SandboxExecutionPolicy、FS\_SANDBOX\_DENIED → sandboxDenialMarker 重标）+ presentCall 四形态（Generic read/Edit + Diff diff 卡片）+ schemastery Config schema + invariant companion）；② dsh-fs-sandbox 拆 `build`（构造不注册,fs-local 同构——测试注入转发 包装器避免同服务名双注册）；③ 14 项测试全绿（TS 14 例全量：schema/ presentation/dispose、canonical 四命令、absence 恢复、字面替换、列表 截断、空行/范围/尾插、歧义/多行/CRLF/相对路径、18 项非法参数不动 文件、observation-policy 读前编辑门、sandbox policy 拒绝 + ownerless、 tab 保留、write 失败映射、配置拒绝）。 关键点：fs/edit-intent 的 FsError 拒绝以 panic 走 Rust waterfall（无 错误通道），工具层 catch\_unwind 恢复结构化 {name,code} info（测试断言 error.info.code）；MockFs 转发包装器 + 覆写槽（TS 的 ctx.fs 方法补丁 等价）；session policy 请求用 Arc::new(session.clone())。

第 81 轮 subprocess-local 终端层落地（M4 尾项 PTY 的可移植半）：① `process_inspector.rs`（TS process-inspector.ts 全量：ProcessIdentity （pid+started 身份）、ProcessInspector trait 7 法、ProcessInspectorInternals 注入边界（readFile/readDir/open/read/close/exec/kill）、parseProcStat 完整字段（含 comm 括号、state 单字符、started）、processTree 纯函数 （children-first + visited 防环）、linuxProcessGroupHasLiveMembers、 LinuxProcessInspector（tpgid 前台组、syscall 表 x64/arm64 的 read/ select/pselect/poll/ppoll/epollWait/epollPwait 等待检测——/proc/mem fd\_set、pollfd 小端解析、fdinfo tfd 匹配）、MacProcessInspector（ps 表解析）、createProcessInspector（平台门控 win32 拒）；② `terminal.rs`（TS terminal.ts 全量：PtyTerminal trait（node-pty IPty 塌缩）+ LocalTerminalHandle——onData 桥接 unbounded 输出流（exit 时 sender drop 结束流）、onExit 一次性结算（信号反查表 1/2/9/15/20 + exitCode/signal 语义）、write/inspectForeground/signalForeground （SIGKILL 自组拒绝）、terminateForHostExit 同步三扫、descendants 采纳 的 rootIdentity started 验证（PID 复用防串树）、TERM→wait→KILL→wait 升级 + 幸存报告、terminate 幂等共享 future（失败重置槽）、disposers 释放；③ 25 项新测试全绿（TS terminal.spec 18 例 + process-inspector. spec 7 例全量：fake timers 塌缩为真实短 grace、缓存 promise 身份断言 塌缩为同结果语义）；dsh-subprocess-local 合计 39 项。 偏差：真实 OS 绑定（DEFAULT\_INTERNALS）与 node-pty 后端仍留待 PTY 里程碑（无真实后端消费者）；subprocess-local 的 spawnTerminal 保持 桩拒绝。

第 82 轮 dsh-e2b 落地（E2B 三件套的第一件）：① 新 crate `crates/e2b/e2b`（TS e2b 全量：quoteE2bShellArg 单引号转义 （'"'"'）、e2bControlEnvs 随机 HOME 隔离（/.dsh-e2b-control-uuid + 不可覆盖）、Config 校验（apiKey 配置/E2B\_API\_KEY 环境回退——环境查找 注入化免进程全局 env 竞态、cwd POSIX 绝对、timeoutMs 正整数）、 E2bRuntime（注册 ctx.e2b、cwd/runtimeRoot、getSandbox disposing 双查、 open 创建窗：makeDir cwd + runtimeRoot + getInfo 真目录校验（symlink/ 非目录拒）+ chmod 700 + 失败单次回滚 kill、dispose teardown effect： kill 的 NotFound 吞/其他 logger.error）；② npm SDK 边界塌缩为 E2bSdk::create + E2bSandbox trait（makeDir/getInfo/run/kill）+ E2bSdkError{kind: NotFound/Other}（SandboxNotFoundError 重导出）； 共享创建用 tokio OnceCell<Result> 缓存（并发首调串行化，TS ready promise 等价——Shared<BoxFuture> 推断问题绕开）；③ 13 项测试全绿 （TS 12 例 + quote 拆分：控制 HOME、共享沙箱创建/处置、处置期创建竞 速（oneshot 门 + poll! 驱动）、env/cwd/timeout、NotFound 吞/他错 logger.error、setup 失败回滚、回滚再败保原错、symlink/文件根拒、 配置 3 例拒 + env 缺失拒、invariant companion）；dsh-e2b 合计 13 项。 偏差：eager ready 变惰性（首个 getSandbox/dispose 触发创建，可观察 契约不变）；真实 HTTP SDK 后端留待 e2b SDK 里程碑。

第 83 轮 dsh-fs-e2b 落地（E2B 三件套第二件）：① dsh-e2b SDK 边界 扩展：E2bEntryInfo 全字段（name/path/type/size/mode/modifiedTimeMs/ symlinkTarget/metadata）、E2bSandbox 增 readBytes/readStream/list/ write(metadata)/rename/remove + makeDir 返回 bool、E2bSdkErrorKind 增 CommandExit{exitCode}（stderr 载荷）；② 新 crate `crates/e2b/fs-e2b` （TS fs-e2b 全量：posix resolve/relative 手写、canonicalPath 经

`realpath -mz | base64 -w0` 命令 + 严格 NUL framing/base64 往返/UTF-8 校验、entryVersion = sha256(metadata.dsh-version+path+type+size+ mode+modifiedTime+symlinkTarget)、resolve/stat/lstat/readText/ readBytes（stat 预检 + 流式增量上限 + 取消）/streamText（前 8192 字节 NUL 采样 + 手写增量 UTF-8 解码器 TextDecoder{stream:true} 语义）/ listDir（1 层 + symlink 单独 canonicalize + 稳定序）/writeText（per- key 锁 + checkWriteIntent + readForDiff（非文本→null）+ writeAtomic： 随机 staging 目录 chmod 700 → write(metadata) → chmod 模式 → createIfAbsent 走 guardedLink 命令 / 否则 rename → remove staging）/ editText（version 校验 + CRLF normalize/detect/restore + literalEdit replaceAll）；permission denied/operation not permitted 匹配→ FS\_PERMISSION\_DENIED；③ 19 项测试全绿（TS 25 例核心子集：身份/ symlink/列表、URL/包含、多字节路径、framing 5 拒、整读+跨块流、 SDK 空流怪癖、二进制/无效/缺失/非正规映射、readBytes 上限、abort、 创建元数据/模式保留/CRLF 归一/版本变化、意图强制、guardedLink 竞速、字面编辑+CRLF 恢复、编辑失败码、并发串行化、staging 清理+ 命令/权限映射、canonicalization 失败、list 仅 symlink canonicalize）； fs-e2b 合计 19 项。 关键点：测试 FakeRemote 完整移植远端节点树+命令解析（realpath/ chmod/guardedLink/mv 引号对）——parking\_lot 锁内嵌套 raw\_info 死锁 （list 的 filter/map 链持锁）、chmod 模式必须八进制解析、runtime 真实挂载的 creation-window 副作用（.dsh-e2b 节点/chmod 命令）污染 断言——setup 预热 open + 断言前删节点；TS 测试子集（剩余 stream cancel/binary 采样边界等用例留待后续轮次）。

  第 84 轮 dsh-host-webserver 落地：   ① 新 crate `crates/host/webserver`（TS `@deepseek-ai/dsh-host-webserver`   全量接口：`WebServer` 注册 `ctx.webServer`；`Config { host, port }`   仅接受 `127.0.0.1` / `0.0.0.0` 与 0–65535；`register` exact/prefix   路由、`registerUpgrade`、`registerFallback` 唯一座位、`tapIndex` /   `applyIndexTaps`；exact 先于 prefix、最长前缀优先、未认领 404、   per-request panic/Err 收口为 400 且服务器继续服务；upgrade 精确匹配，   hyper `on_upgrade` 101 握手后把 `TokioIo<Upgraded>` 交给 handler；   teardown abort accept/connection/upgrade 任务以显式关闭 upgraded   socket；EADDRINUSE 作为 PluginError fail-loud）；② invariant companion   （`internal/plugin` 上 route/upgrade disposer 对称探针）；③   `tests/webserver.rs` 2 项全绿：路由优先级、fallback 语义、index tap、   index tap、malformed `%zz` 400、重复注册 panic / disposer 恢复、   upgrade 101 与重复拒绝、失败 upgrade 不影响服务、teardown 关闭   upgraded socket、端口占用失败；④ 偏差：未匹配 upgrade 返回 400 而非   裸 socket destroy；node `ServerResponse` 塌缩为返回 `axum::body::Body`。

### M2 — 类型、契约与共享工具

- [x] `dsh-scope`（`crates/core/scope`，3 项测试全绿）；
- [x] `dsh-brand`（`crates/util/brand`，1 项测试；`Branded<B>` 名义类型 + PhantomData 标记 + serde 透明）；
- [x] `dsh-timeout`（`crates/util/timeout`，9 项测试全绿）；
- [x] `dsh-atomic-write`（`crates/util/atomic-write`，3 项测试全绿）；
- [x] `dsh-home-paths`（`crates/util/home-paths`，4 项测试全绿）；
- [x] `dsh-launch-environment`（`crates/util/launch-environment`，7 项测试全绿——

      launch-environment.spec.ts 全部移植：三层信任序分层快照（process >       project-env > user-env）、getFrom 层过滤不动信任序、构造期拷贝冻结、       Windows 大小写折叠、launchEnvironmentOf 提供/回退进程环境）；

- [x] `dsh-output-retention`（`crates/util/output-retention`，7 项测试全绿）；
- [x] `dsh-invariants`（`crates/util/invariants`，3 项测试全绿；

      InvariantRegistry：enabled/allowlist/blocklist 正则选择、包名保留、       子 fiber 安装器、失败回收；安装器失败通道为 `Arc<dyn Fn(&str)+Send+Sync>`）；

- [x] `dsh-llm`（`crates/core/llm`，59 项测试全绿——第 19 轮补齐运行时层；

      类型层线格式与 TS 逐字节一致 + LlmRuntime 运行时 + llm-invariant 伴生；       provider adapters 留待后续里程碑）；

- [x] `dsh-typert-protocol`（`crates/core/typert-protocol`，3 项测试全绿；

      protocol 子集；registry/loader/generator 留待 typert 里程碑）；

- [ ] typert registry/loader/generator；
- [ ] api gateway/remotes；
- [ ] SDK protocol/client/jsonrpc-server；
- [x] `dsh-settings`（`crates/settings/settings`，17 项测试全绿）；
- [ ] settings-file、preset/persona；
- [x] `dsh-anonymous-user-id`（`crates/identity/anonymous-user-id`，10 项测试

      全绿——harness-home 作用域匿名用户 id，telemetry/feedback 共享）；

- [x] `dsh-attachment`（`crates/attachment/attachment`，seam——不可变附件

      存储抽象 + AttachmentError 码类 + 品牌 id）；

- [x] `dsh-attachment-local`（`crates/attachment/attachment-local`，8 项

      测试全绿——内容寻址本地后端；见上）。

### M3 — Core agent/session/LLM

- [x] `dsh-session`（`crates/core/session`，66 项测试全绿）；
- [x] `dsh-system-prompt`（`crates/core/system-prompt`，29 项测试全绿）；
- [x] `dsh-agent`（`crates/core/agent`，34 项测试全绿）；
- [x] `dsh-session-persistence`（`crates/session/session-persistence`，16 项测试全绿）；
- [x] `dsh-session-persistence-jsonl`（`crates/session/session-persistence-jsonl`，22 项测试全绿）；
- [x] `dsh-session-persistence-sqlite`（`crates/session/session-persistence-sqlite`，30 项测试全绿）；
- [x] `dsh-session-query`（`crates/session/session-query`，7 项测试全绿——

      组合会话查询服务 seam：精确读/过滤器/追踪/语料 + 抽象搜索面；见上）；

- [x] `dsh-session-reference`（`crates/context/session-reference`，6 项测试

      全绿——跨会话快照引用 + 不可信模型上下文；见上）；

- [x] `dsh-commands`（`crates/interaction/commands`，8 项测试全绿——

      插件所有的人类命令注册表 + run/done 生命周期；见上）；

- [x] `dsh-agent-default-model`（`crates/core/agent-default-model`，5 项测试全绿）；
- [x] `dsh-tools`（`crates/core/tools`，39 项测试全绿——schema 层 + 运行时层；

      code/both 呈现依赖 dsh-code-runtime 留待后续）；

- [x] `dsh-agent-tool-presentation`（`crates/core/agent-tool-presentation`，5 项测试全绿）；
- [x] `dsh-agent-loop`（`crates/core/agent-loop`，16 项测试全绿——常量/运行时上下文

      投影/不变式伴生/工具调用调度器/ReactLoopAgent 机器/AgentLoop 服务；       request-reconstruction/resume 深链路随 backend 里程碑补全）；

- [x] `dsh-session-projection`（`crates/session/session-projection`，17 项测试全绿）；
- [x] `dsh-session-stats`（`crates/session/session-stats`，15 项测试全绿）；
- [x] `dsh-session-telemetry`（`crates/session/session-telemetry`，7 项测试全绿）；
- [x] `dsh-session-title`（`crates/session/session-title`，46 项测试全绿——

      SessionTitleService/确定回退/provider 契约/rename 钉住/服务契约/投影单元/       不变式伴生/JSONL+SQLite 持久化往返；AbortSignal.any 同步谓词改为       fused source 扫描）；

- [x] `dsh-session-title-llm`（`crates/session/session-title-llm`，12 项测试全绿——

      共享 LLM 策略：route 解析/JSON 帧输入/超时/装配校验）；

- [x] `dsh-session-title-all-prompts-llm`（`crates/session/session-title-all-prompts-llm`，1 项测试）；
- [x] `dsh-session-title-first-prompt-llm`（`crates/session/session-title-first-prompt-llm`，1 项测试）；
- [x] `dsh-session-projection-cache`（`crates/session/session-projection-cache`，18 项测试全绿——

      turn/end 与 detach 双强制写点、count/interval 节流、fail-soft 自愈、       cachedSnapshot 零 I/O 列表读、coldSnapshot 阶梯读（缓存行 + readFrom 尾读 +       registry restore + 写回；ver 失配/日志收缩/生命周期不符降级为全量重读）；       storage-domain 数据表单先行落地，见 M4）；

- [x] `dsh-llm-retry`（`crates/llm/llm-retry`，7 项测试全绿；dsh-llm retry-policy 类型层 +3 项）；
- [x] `dsh-token-meter`（`crates/llm/token-meter`，10 项测试全绿）；
- [ ] dsh-tools code-mode（run\_code transport/ts-types/py-types，依赖 code-runtime）；
- [ ] DeepSeek/OpenAI/pi-ai 适配器；
- [ ] context、compaction、interaction、attachment、feedback、hooks、guards。

### M4 — 执行/文件/沙箱

- [x] `dsh-storage`（`crates/storage/storage`，6 项测试全绿——storage Hub：BackendRegistry

      （重复名拒绝/过期 disposer 守卫）、form 挂载/解析、storageBackendServiceKey、       StorageError 码表、KvFacet/KvUnit 后端契约的单一家）；

- [x] `dsh-storage-domain`（`crates/storage/storage-domain`，6 项测试全绿——域声明/写链/

      domain-changed 事件/版本戳记/关闭排空；facility 经 Hub 路由后端并挂载 `domain`       form；zod 记录 schema 塌缩为 JSON 校验闭包）；

- [x] `dsh-storage-test-support`（`crates/storage/storage-test-support`，内存 KV 后端测试双）；
- [x] `dsh-storage-json`（`crates/storage/storage-json`，17 项测试全绿——每单元一个

      人类可读文件、临时文件+fsync+rename 原子整文件发布（spawn\_blocking 上跑       Node 线程池等价物）、失败回滚、延迟物化、关闭排空/阻塞在飞 open、       Hub 注册与生命周期服务键、invariant 伴生）；

- [x] `dsh-storage-sqlite`（`crates/storage/storage-sqlite`，19 项测试全绿——单库承载

      全部路由单元、`u_<unit>_<table>` STRICT 记录表 + units/unit\_globals 元数据、       user\_version 物理版戳（失败留 0 可修复重开）、journal\_mode 白名单、       prepare\_cached 复用语句、未解析 JSON → malformed-medium、pending open 关闭排空）；

- [x] `dsh-workspace`（`crates/workspace/workspace`，49 项测试全绿——

      workspace.spec.ts（43 项）+ invariant.spec.ts（6 项）全部移植：       仅头引导与稳定排序、create/delete 的 pending-marker 双写恢复与注入故障回滚、       registry 级 insertBefore、会话 attach/move/detach 写链判定、canonical-cwd       头校验投影、archive/unarchive/deleteArchivedSession、cache/table invariant       伴生（domain/changed 监听）；`sessionPersistence.delete` 尚未在 Rust       persistence 侧落地，deleteArchivedSession 经 caller 提供的闭包 seam）；

- [x] `dsh-spill`（`crates/spill/spill`，0 项测试——Service Definition：`ctx.spillStore`

      抽象 seam（`saveText` + SpillLocator 品牌 + owner/source/ref 词表），no-op       invariant 伴生）；

- [x] `dsh-spill-local`（`crates/spill/spill-local`，11 项测试全绿——

      spill-local.spec.ts 全部移植：UTF-16 code-unit 注入式 encodeSegment、       sha256 会话目录、0700 私有根、0600 独占创建（`create_new` = `'wx'`）、       随机前缀防 symlink 种植、遍历形 suggestedName 中和、相对 root 解析、       存储故障拒绝）；

- [x] `dsh-spill-policy`（`crates/spill/spill-policy`，16 项测试全绿——

      spill-policy.spec.ts 的模型侧 arm 全部移植：tools/post-execute prepend       waterfall、read/嵌套/值替换/非文本直通、notice 预算预留、超 cap 保留内联、       best-effort 三降级、下游组合与 disposer 卸载；durable       `tools/code-dispatch-log` arm 待 dsh-code-runtime 里程碑）；

- [x] `dsh-credentials`（`crates/credentials/credentials`，9 项测试全绿——

      credentials.spec.ts + invariant.spec.ts 全部移植：POSIX 标识符 ref 校验、       空值即缺席的 seam 规则、memory provider 端到端、notifyUpdated 包含分发       （每个监听器都跑、普通失败告警、INVARIANT 失败聚合后上抛）、       commit-event 生命周期不变式伴生（无 live 服务不得 emit））；

- [x] `dsh-credentials-local`（`crates/credentials/credentials-local`，49 项测试全绿——

      local/drain/watcher/review-fixes 四个 spec 全部移植：`.credentials.yaml` 严格       校验（非映射根/序列根/非法 ref/非字符串/空值/重复键/畸形 YAML，错误不泄值）、       行级注释保留编辑（set 只改目标行、unset 连同上注删除、空文档 `{}`、兄弟块标量       原样、结构形值引号转义）、写锁读改写折叠外部编辑、0600/0700、包含的       credentials/updated 扇出、dispose drain（在飞写落盘、排队写拒绝）、fake watcher       管道与真实 notify 热重载、自写抑制、缺失/损坏文档的 warn-and-keep；       无 runtime 线程上的 watcher 事件经 channel + runtime 任务入队）；

- [x] `dsh-fs`（`crates/fs/fs`，12 项测试全绿——

      service.spec + invariant.spec 全部移植：FsTargetKey/FsVersion 不透明品牌、       FsObservation/FsInfo/FsPathInfo/FsDirEntry/写意图/编辑请求结果词表、       FsError 13 码表 + cause 链、FileSystem 抽象服务（resolve/processPath/       fileUrl/contains/stat/lstat/readText/streamText/readBytes/listDir/writeText/       editText + sandboxMode 默认）、internal/dispatch 预钩校验三个事件数据       （空 targetKey/displayPath/version 拒绝）；`streamText` 为 BoxStream、       AbortSignal 塌缩为取消谓词）；

- [x] `dsh-fs-local`（`crates/fs/fs-local`，20 项测试全绿——

      fsio.spec + filesystem.spec 核心子集移植：resolveLocalTarget 的 realpath       身份 + 缺失文件的最近祖先回退（symlink 别名同 key、ENOTDIR 结构化）、       probe/probeNoFollow、listDirectory 稳定序无内容读、readWholeText（NUL 样本

      - 严格 UTF-8）、readWholeBytes 字节上限（stat 短路 + 增长检测）、

      streamWholeText 跨块增量 UTF-8、readForEdit/readTextForDiff 行尾归一与       有界 diff basis、applyLiteralEdit 字面替换、writeFileAtomic 私有 staging       （0700/0600、独占 create、sync、原子发布、hard-link 守卫创建、清理       失败不翻转已提交写）、LocalFileSystem per-targetKey 锁（并发守卫写       一胜一 stale、写/编辑确定性）；win32 DACL 拷贝/安全替换为简化边界       （真实实现随 sandbox-windows-acl 里程碑），版本 token 在 Windows 为       近似组成）；

- [x] `dsh-fs-observation-policy`（`crates/fs/fs-observation-policy`，18 项测试全绿——

      policy.spec 全部移植：观察态 gate（owner key → targetKey → 观察记录）、       write-intent（未观察/无 owner → createIfAbsent，已观察 → 观察版本 CAS）、       edit-intent（未读 FS\_NOT\_OBSERVED、缺席 FS\_NOT\_FOUND、观察版本守卫）、       present→absent→present 转移、多 owner 隔离、单槽 first-wins（不调 next）       短路、dispose 释放记录并移除监听器；owner 用最小 handle 的 opaque key       （TS WeakMap 对象身份的 Rust 形）；edit 拒绝经 waterfall 的 panic 通道       携带结构化 FsError）；

- [x] `dsh-fs-sandbox`（`crates/fs/fs-sandbox`，11 项测试全绿——

      containment.spec 全部 + fs-sandbox.spec 核心子集移植：isPathUnder 词法       快路径 + 文件系统身份回退（Unix dev:ino；Windows canonicalize 等价）、       SandboxedFileSystem 每调用策略栅栏（read-only 拒绝 FS\_SANDBOX\_DENIED、       workspace-write 现时重解析 + writableRoots 包含、danger 直通）、`..`/       symlink 逃逸拒绝、TOCTOU 方向（stale targetKey 不写）、审批模式升级覆盖；       继承 dsh-fs-local 全部存储机制（build 不注册变体））；

- [x] `dsh-sandbox-policy`（`crates/sandbox/sandbox-policy`，10 项测试全绿——

      policy.spec 服务子集 + session-mode 套件全部移植：defaultMode 回退/       workspaceRoot 绝对化、resolve 的 会话 cwd+override 组合与审批模式最高       优先、sandbox/mode 事件套件（fold 最后切换、append 恰一条）、       sandbox/mode 事件不变式（unknown mode 拒绝）；`systemPrompt.context`       的 sandbox:policy 请求上下文注入留待 agent 字段进入 assemble context）；

- [x] `dsh-sandbox`（`crates/sandbox/sandbox`，15 项测试全绿——

      vocabulary/escalation/roots 三个 spec 全部移植：SandboxMode/Policy/       ConfinedArgv/denial 方言/RunnerFailureRule 词表、SandboxUnavailableError       （SANDBOX\_UNAVAILABLE 结构化码）、严格加宽梯 WIDER\_MODES +       闭集 ESCALATION\_TARGETS、validateEscalationArgs 参数配对、       模型面 denial/hint marker、approveEscalation 有序 fail-closed 序列       （非加宽不提问、无审批服务/无 agent 各自文案、四态 outcome 映射）、       canonicalPath（解析失败保原拼写）+ writableRoots 规范化去重派生）；

- [x] `dsh-subprocess`（`crates/subprocess/subprocess`，4 项测试全绿——

      service.spec 全部移植：完整 spawn 词表（三态 stdin ignore/pipe/data、       输出模式 pipe/inherit/有界收集+spill、offset 无消费读取器、树级终止       SIGTERM→grace→SIGKILL 唯一终止动词、显式 env 墓碑）、terminal 原语       （六法：output 字节流/done/write/inspectForeground/signalForeground/       terminate 全会话静默）、scrubbedParentEnv 双擦洗（凭据形 KEY/       PASSWORD/SECRET/TOKEN + DSH\_ 前缀均大小写不敏感）、解析可执行       （绝对验证/裸名 PATH/含分隔符相对路径拒绝）；AbortSignal 塌缩为       取消谓词、Node 流塌缩为 tokio 字节流）；

- [x] `dsh-subprocess-local`（`crates/subprocess/subprocess-local`，14 项测试

      全绿——spawn.spec + local.spec 核心子集移植：childEnv 擦洗合并（墓碑       移除/显式凭据幸存/Windows 大小写不敏感）、OutputCollector 字节精确       尾窗与惰性 spill（溢出不丢先头、超 cap 丢文件保尾窗）、detached 进程       树 spawn（POSIX 进程组/Windows taskkill /T /F 树终止）、SIGTERM→       grace→SIGKILL 升级（TERM 陷阱幸存者仍被杀）、abort 谓词 15ms 轮询       反应（TS 事件目标塌缩）、批次 stdin 上写即关、可执行解析（绝对验证/       裸名 PATH+PATHEXT/相对路径拒绝/稳定错误文案）、服务释放与 fiber       处置终止整树；`spawnTerminal`（node-pty）桩留待 PTY 里程碑；done       结算自 spawn 起即被驱动（TS 事件驱动 vs Rust future 惰性））；

- [x] `dsh-terminal`（`crates/terminal/terminal`，23 项测试全绿——

      service.spec 全部 23 例移植：backend 注册精确贡献释放（ptr 身份清理）、       owner 精确栅栏（FOREIGN\_SESSION/NO\_SESSION/OWNER\_NOT\_LIVE）、spawn       发布/回滚（未发布 close 回滚 + 双失败聚合）、调用者取消（Aborted       塌缩）、owner/服务处置对未发布 setup 的 abort+await（sync 前缀语义）、       后端侧清理失败保留至处置聚合、close 幂等 fence 合并与代数守卫、       处置 best-effort 清注册表 + 跑 owner cleanup；invariant 伴生 no-op；       spawnTerminal 的 PTY 后端（terminal-bash）留待后续）；

- [x] `dsh-shell`（`crates/shell/shell`，7 项测试全绿——

      render.spec 全部 + service.spec 全部移植：`[exit code: N]`/       `[killed by signal: X]` 标记解析逆契约（前置换行+结尾锚定）、       task-free 抽象执行器（resolve/run/start 三原语 + sandboxMode 默认       无沙箱 + 重复注册 fail-loud）；DshEnvironment/DshEnvironmentKey       补入 dsh-subprocess 词表）；

- [x] `dsh-bash-local`（`crates/shell/bash-local`，21 项测试全绿——

      executor.spec + settings.spec 移植：ENV\_OVERRIDES 终端环境、       Config→ResolvedConfig 默认与 assertServiceable 校验（正值 + graceMs       定时器上界）、clampTimeout 上限、deadline 融合信号（timeout/abort       首因互斥分类）、stdin/env/dshEnv 三明治合并（覆盖 > 调用者 > 终端）、       stdout 独立预算、后台 ShellProcess（消费式增量读、\[stderr\] 段合并、       损失标记 + 双 spill 路径、kill 幂等 + grace 升级、spec.signal abort       结算 killed、spawn 失败 killed + note 一次投递）、settings 段       user 层解析/写入校验/存储段服务/供应商脱落回退/无供应商入口/       命名空间释放；onProcessDone 钩子注入化（Rust 无子类化）；       Windows+WSL 下 POSIX 路径/env 转发/引号语义不可靠的用例 cfg(unix)       门控，引用免费子集全平台运行）；

- [x] `dsh-code-runtime`（`crates/code-runtime/code-runtime`，9 项测试全绿——

      reserved.spec + service.spec 全部移植：可移植标识符排除集四个共享       契约（绑定全局保留槽 console/\_\_dsh\_main\_\_/\_\_builtins\_\_/\_\_name\_\_/       \_\_debug\_\_、错误成员保留集、dunder 形式正则（空中间不匹配）、       ECMAScript∪Python 保留字并集——一仓契约保证跨后端可移植）、抽象       CodeRuntime（language/isolation/run 三原语，失败为结果字段永不       rejection、AbortSignal→谓词塌缩、重复注册 fail-loud、fiber 卸载       移除服务）；invariant 伴生 no-op；`code-runtime-worker-thread`       （TypeScript bootstrap/worker JSON 协议，需嵌入 JS 运行时）留待       后续里程碑；

- [x] `dsh-jobs`（`crates/jobs/jobs`，5 项测试全绿——

      service.spec + invariant.spec 全部移植：抽象 JobRegistry 九法       （start/list/get/read/kill/wait/onJobDone/onJobsChanged/       attachController）、JobId 品牌（`<kind>-N`）、JobHooks 三法       （cancel/done/readOutput 消费游标）、JobOutcome 终态三分类、       快照跨字段不变式（id 前缀+正序数、标签非空、startedAt 非负、       finishedAt 恰在终态、ownerSession 与完成 owner 一致）+       jobs-inject 安装器（校验现有 unowned 记录 + 订阅终态快照）；       抽象 seam 挂载栅栏（TS new.target 检查）在 Rust 为编译期事实；       invariant 伴生含真实安装器）；

- [x] `dsh-jobs-local`（`crates/jobs/jobs-local`，14 项测试全绿——

      jobs.spec 核心子集移植：入站预检链（控制器服务/空 kind/空 label/       非法 outputLimitBytes/owner 必须当前注册实例）、按 kind 顺序 id       计数器、每 exact-owner 并发上限、session-id 授权栅栏（异主       get/read/kill 拒绝、无主任务开放）、流式 readOutput 消费游标与       终态 output 幂等读、reported 标记、kill 两态（取消先于状态转移）、       首胜结算（晚到 outcome 忽略）+ settled 广播 + 监听通知       （onJobDone/onJobsChanged 包含式投递）、有界 wait（结算/超时       返回快照/中止拒绝、waiter 计数）、teardown 取消与抛错强制失败       （possible orphan 报告）、owner 处置取消并删除记录、服务处置清空       与跨 fiber effect 分离；ScopedLayers 全局+scope 链分层控制器/监听；       `dsh-tool-jobs` 工具层留待后续）；

- [x] `dsh-goal`（`crates/goal/goal`，12 项测试全绿——

      goal.spec 核心子集 + 严格折叠套件移植：事件溯源 goal/change       （全量快照 + clear 墓碑，Session.append 持久化）、CAS ref 比对、       七动词 + 阶段梯（create 仅可替换 completed；pause/resume/       complete/block 的 allowed 集合；resume 的 armed 拒绝与预算耗尽）、       边界校验（objective 规范化/maxGoalRounds 正整数/blockReason       lower-kebab）、进程本地 activation（pending-activation 跨 append       边界 + session-start disarm 边）、round 准入折叠（user/message 的       goal 源：仅活动目标的下一个轮次、上限）、goal/changed scoped       emit、strict 解码器 fail-loud（版本/字段集/规范化/跳过 revision/       定义漂移/时间戳回退）；缓存键用 agent id（session 事件快照指针       跨 append 不稳定）；@Remote 注解与 goal 投影单元注册留待       typert/session-projection 集成；

- [x] `dsh-session-checkpoint-policy`（`crates/session/session-checkpoint-policy`，

      4 项测试全绿——语义持久化检查点核心：llm/stream 工厂包装       （flush 先于首块、失败 fail-closed 终态 finish 块且阻止适配器分派）、       tools/execute 顶层 owned 调用检查点 + 预分派 abort 规范结果       （ABORTED\_BEFORE\_DISPATCH）、agent/pre-step 边界 flush；NextFn       单次延续语义、llm/stream cell 的双 Arc downcast、flat\_map 分流       （失败不链下游）；

- [x] `dsh-native-command`（`crates/util/native-command`，4 项测试全绿——

      无 shell 执行器：utf8 stdio 捕获、非零退出 code+stdio 附加、       ENOENT、abort 谓词传播终止、Windows CREATE\_NO\_WINDOW hide）；

- [x] tool-jobs（后台任务列表、输出、等待与终止工具）；
- [x] 本地 subprocess PTY / Windows ConPTY / process tree；
- [ ] subprocess E2B terminal（需外部沙箱 SDK 的远端终端能力）；
- [ ] code-runtime-worker-thread（TypeScript bootstrap，需嵌入 JS/TS

      运行时——boa/deno\_core 级依赖，独立里程碑）；

- [x] sandbox-local（Linux bwrap、macOS Seatbelt 方言与 fail-closed 选择）；
- [x] Windows AppContainer + ACL package-SID backend；
- [ ] credentials-encrypted backend（DPAPI/keychain）。

### M5 — 产品功能

- [x] goal（`crates/goal/goal`，见上）；
- [x] time-context（`crates/context/time-context`，20 项测试全绿——

      `@deepseek-ai/dsh-time-context`：可选出每步持久化时钟上下文；见上）；

- [x] repeat-tool-reminder（`crates/guard/repeat-tool-reminder`，13 项测试

      全绿——`@deepseek-ai/dsh-repeat-tool-reminder`：建议型重复调用       检测器；见上）；

- [x] timeout-policy（`crates/guard/timeout-policy`，9 项测试全绿——

      `@deepseek-ai/dsh-tool-call-timeout-policy`：协作式工具超时执行器；       见上）；

- [x] tool-goal（第 128 轮：`crates/goal/tool-goal` 将模型侧

      `get_goal/create_goal/update_goal` 接到真实 `ToolRuntime + GoalService`；       严格 schema 与标准 `ToolArgsError/INVALID_ARGS`、JS safe-integer wire       边界、exact live Agent/ambient initiator/root-child authority、open-turn       与 strict replay 防伪、GoalRef CAS、条件参数/empty filler、直接人类与       autonomous complete/blocked 策略、三轮 blocker 下限、终态 wrap-up、       预分派取消和 `GOAL_COMMIT_FAILED` typed fail-loud 均有真实执行证据；       修复工具注册半安装、round 0/重复 round 越权及 complete 后 wrap-up       panic。AgentLoop 整个 driver 继承 exact initiator，ReactLoopAgent 使用       独立真实 ScopeKey，Cordis `ctx.agent` accessor 可解析最近 live agent，       子代理不再误判为 root；生产 dsh-host 安装 `LlmRuntime + AgentLoop`、       三个 Goal 工具和 goal/command-goal/tool-goal/agent-loop companions，       脚本模型真实产生 durable Goal + tool/call + tool/result；追加同步 prepare/commit       跨注册表事务，prompt/工具冲突均返回 `Err` 且零 partial 残留，listener 抛错       rollback 发布第二次 change 通知。tool-goal 24 项、dsh-scope 10 项、       SystemPrompt 29 项、Tools 15 项、Host 12 项全绿）；

- [x] goal-round-driver（第 129 轮：新增 `crates/goal/goal-round-driver` 并

      接入 production dsh-host；实现 active+armed Goal 的 durable checkpoint、       canonical positive round、Queued/Claimed/Admitted reservation、pre-step       前后 exact authority fence、human/plugin 工作优先、预算/terminal/error/       max-tokens/checkpoint-failure 收敛、session-start/hot-load 撤权、owned       teardown 与 in-flight task drain；companion invariant 以 listener-first +       durable-prefix 关闭 late-load 安装窗口。同步补 Agent emit 重入前缀、       AgentLoop MaintenanceGuard/Running wake latch、Goal mutation release 的       disarm-wins 及 Session append 原子 publication claim。第 129 轮最终       workspace 为 1800 passed / 0 failed / 1 ignored；当前覆盖 production       纵向主链与高风险边界，不以 Rust 测试数冒充上游 50 项逐项全覆盖）；

- [x] command-goal（第 127 轮：新建 `crates/goal/command-goal`，将真实

      `/goal` 人类命令接到 `CommandRuntime + GoalService`：show/create/       CAS edit/pause/resume/clear、complete 后创建新 identity、精确控制词       解析、Unicode objective、active/paused/blocked/complete 与 activation/       blocker/可用命令呈现、预期 GoalError→人类错误、command run/done 与       durable `goal/change` 闭环；append 失败新增 `GOAL_COMMIT_FAILED` 原子       回滚（不更新 cache/不广播），并从 command handler 作为基础设施错误       逃逸；补 Cordis Plugin、package-owned no-op invariant、direct/plugin       disposer 生命周期与缺依赖 fail-loud；生产 dsh-host 安装 goals、注册       `/goal`、挂载 goal domain + command adapter 两个 invariant，boot report       服务数 12→13。command-goal 19 项（lib 2 + integration 17）、dsh-goal       14 项、Host boot/真实网络/安全 10 项全绿）；

- [x] tool-todo（`crates/todo/tool-todo`，12 项测试全绿——

      `@deepseek-ai/dsh-tool-todo`：整表替换 todo 工具 + `todos` 投影       单元；见上）；

- [x] user-approval（`crates/interaction/user-approval`，26 项测试全绿——

      `@deepseek-ai/dsh-user-approval`：一次性授权 seam + 审计对 +       会话级 ask/never 政策；见上）；

- [x] permission-presets（`crates/interaction/permission-presets`，32 项

      测试全绿——`@deepseek-ai/dsh-permission-presets`：sandbox/approval       双旋钮预置 + `/permission` 命令 + `permissions` 投影；见上）；

- [x] skill（`crates/skill/skill`，30 项测试全绿——

      `@deepseek-ai/dsh-skill`：分层技能提供者注册表 + 目录缓存 +       渲染；见上）；

- [x] tool-skill（`crates/skill/tool-skill`，24 项测试全绿——

      `@deepseek-ai/dsh-tool-skill`：`skill` 加载器工具 + 持久会话       目录 + `/name` 手势注入；见上）；

- [x] skill-badge（`crates/skill/skill-badge`，2 项测试全绿——

      内置 `dsh-badge` 技能 + 官方资产；见上）；

- [x] plan-mode（`crates/plan/plan-mode`，14 项测试全绿——

      `@deepseek-ai/dsh-plan-mode`：logged 协作状态 + plan:policy 段 +       `/plan` 命令 + exit\_plan\_mode 评审工具 + `plan` 投影；见上）；

- [x] skill-filesystem（`crates/skill/skill-filesystem`，7 项测试全绿——

      `@deepseek-ai/dsh-skill-filesystem`：本地项目/用户/custom/bundled       根技能发现 + frontmatter 解析 + notify 监视；见上）；

- [x] session-query-sqlite（`crates/session/session-query-sqlite`，25 项

      测试全绿——`@deepseek-ai/dsh-session-query-sqlite`：SQLite FTS5       派生物化索引 + 全量搜索/游标/对账后端；见上）；

- [x] schedule（`crates/schedule/schedule`，25 项测试全绿——

      `@deepseek-ai/dsh-schedule`：会话内一次性/固定频率提醒 + 三个       管理工具 + 逐 agent 定时运行时 + 注入防护 framing；见上）；

- [x] subagent 契约层（`crates/subagent/subagent`，6 项测试全绿——

      `@deepseek-ai/dsh-subagent`：run/result/能力类型 + v2 持久化       描述符 + 深度记账 + 提供者 trait；runtime/registry/backends/       tools 留待后续；见上）；

- [x] subagent 服务核心 + fork 后端（第 74–75 轮：注册表 + 一次性

      start 生命周期 + 共享进程内驱动 + `dsh-subagent-fork-in-process`；       continuation/listing/投影暂拒；见上）；

- [ ] plan/todo 剩余（plan 系列其余包）；
- [ ] subagent registry/backends/tools；
- [ ] workflow engine/worker/tool/ralph；
- [ ] MCP/LSP/ACP；
- [ ] dynamic Cordis extensions、host/client runner、inspect/define/run 工具。

### M6 — Host 外壳与 CLI

- [x] 可启动主程序骨架（`crates/host/dsh-host`——核心服务组合 +

      启动报告，`cargo run -p dsh-host` 实际运行 exit 0；见上）；

- [x] dsh-host M6 组合升级（第 72 轮：10 服务真实启动 + JSONL

      持久化 + SQLite FTS5 搜索 + schedule + 端到端 durability/search       探针；见上）；

- [x] webserver 路由服务（`crates/host/webserver`，第 84 轮，2 项测试全绿）；
- [x] frontend-static + SPA fallback/index injection

      （`crates/host/frontend-static`，第 85 轮：403 穿越/200 回退/405/HEAD/       index taps，2 项测试全绿）；

- [x] directory-picker seam（`crates/host/directory-picker`，第 85 轮：

      native/browse 判别能力 + 三值错误码闭集 + AbortSignal，3 项测试全绿）；

- [x] directory-picker browse 后端（`crates/host/directory-picker-browse`，

      第 85 轮：有界窗口流式列表 + 完全限定路径栅栏 + 非递归创建 +       raceAbort，14 项测试全绿）；

- [ ] directory-picker native/auto 后端；
- [x] directory-picker auto 判定（第 115 轮：resolve 纯函数（bind/SSH/

      平台/显示/chooser 矩阵）+ PATH 探针（zenity/kdialog 注入谓词）       3 项测试全绿；静态组合偏差记录）；

- [x] directory-picker native 后端（第 115 轮：macOS osascript + Linux

      zenity/kdialog 子进程 + 取消观察，1 项测试全绿；Windows       IFileOpenDialog COM 对话留待 win32-dialog 里程碑）；

- [x] plugin-inventory（`crates/host/plugin-inventory`，第 86 轮：非组

      条目只读投影 + FiberState→phase 映射 + Remote 注解留待 typert，       2 项测试全绿；顺带修复 loader `update` 缺失的禁用即释放 fiber       分支——上游 entry.ts 181–192 行语义）；

- [x] apiproxy 契约层（`crates/host/apiproxy`，第 87 轮：四象限

      RpcMessage + 40 码 RpcError 判别联合 + bool 字面量 RpcResult +       54 条方法注册表 + 载体 receipt，12 项测试全绿）；

- [x] apiproxy 载体层（第 87 轮：fetch/handler 状态机——404/415/400/500

      载体纪律 + envelope 解析与 rpcId 抢救 + SSE 帧化 + respond receipt

      - session.export 转发，ApiProxyCarrier trait 待组合层实现，

      8 项测试全绿）；

- [x] apiproxy 事件帧词汇（第 88 轮：MuxFrame 11 变体 + HostFrame 10

      变体判别联合 + ToolEventView/QueuedInboxItem + EventsApi trait +       JobView/WorkspaceView wire 视图，4 项测试全绿；顺带授予       dsh-user-approval::ApprovalOutcome wire serde）；

- [x] apiproxy sessions 契约层（第 88 轮：12 方法 SessionsApi trait +

      SessionSummary/HistoryEntry/ModelSelection 系列/SessionModels +       PromptContentPart/QueueAction 判别联合 + 全部请求/响应 wire 结构，       3 项测试全绿）；

- [x] apiproxy 域契约补齐（第 89 轮：host 5 方法 + skills + goals 6 动词
      - credentials 3 方法 + subagents 4 方法 + settings 5 方法 + llm 3

      方法，各自请求/响应 wire 结构一次编译全绿；存量 27 项测试保持       全绿）；

- [x] apiproxy 契约层收官（第 90 轮：agent-presets 6 方法 + downloads +

      approvals + questions，api/ 全部 15 域 + rpc + rpc-map + fetch/       handler 齐备，27 项测试全绿）；

- [x] apiproxy 组合层 host 域 + skill.list + credentials（第 91–92 轮：

      ApiProxyService 骨架 + host 5 方法全接线（8 项端到端测试）+        skill.list 会话解析/用户可调用过滤（4 项测试）+ credentials 3       方法（REF\_PATTERN 校验/bad-request/credential-rejected），累计       39 项测试全绿）；

- [x] apiproxy agent 解析器 + goal 域（第 93–94 轮：AgentResolver 单飞

      冷恢复/子代理归属 fence/错误分类（4 项测试全绿）+ goal 6 动词       接线（mutateGoal + goalServiceFor + goalError 映射），第 95 轮       goal 端到端 3 项全绿（create→pause→clear CAS 链 + session-not-       found + 无服务 internal）；累计 52 项测试全绿）；

- [x] apiproxy llm + settings 域（第 96–97 轮：llm.providers 目录合并/

      models catalog/discoverModels 失败词汇 4 项测试全绿 + settings 5       方法（describe 全量映射/update/replace/mutate/冲突判定/openDocument）       6 项测试全绿；顺带补 SettingsProvider::writable 转发；累计 59 项）；

- [x] apiproxy workspace 域（第 98 轮：list/create/rename/delete/

      insertBefore/archiveSession/unarchiveSession 7 方法 + WorkspaceApi       trait 与全部请求/响应 wire 结构；第 99 轮 memory domain 装配下       5 项端到端全绿（create 复用 created 位/rename/delete 链 +       workspace-invalid-path + archive 会话词汇）；累计 64 项）；

- [x] apiproxy session.list/create（第 100 轮：attached/cold 汇总合并

      （updatedAt 降序、blank/running/header 透传，cold blank 保守       false 偏差）+ create 全链路（sessions.create + agents.create +       工厂），3 项测试全绿；累计 67 项）；

- [x] apiproxy session.rename/cancel（第 101 轮：session-title 服务

      rename（title-invalid/internal 词汇）+ agents.get cancel（User       cause + keepInbox + 子代理 fence），2 项测试全绿；累计 69 项）；

- [x] apiproxy session.history/models/selectModel（第 102 轮：paginate

      消息对齐分页（sourceEventSeqs groupStart 切割 + beforeSeq 窗口 +       hasMore）3 项测试全绿；models（selectionFor 进程内选择 + catalog +       routable）与 selectModel（resolveCallConfig 校验 + selections       记录，图片准入留待 attachment 里程碑）3 项测试全绿；累计 75 项）；

- [x] apiproxy session.fork（第 103 轮：turn/end 边界定位（anchored/

      末尾回退）+ out-of-band 切割扩展 + 子会话创建（seed 前缀 +       parentSession/seedLength/cwd/agentPreset 继承），3 项测试全绿；       workspace 附加留待 attach 里程碑；累计 78 项）；

- [x] apiproxy session.updateQueue（第 104 轮：收件箱 edit/remove/steer
      - 非文本编辑 attachment-error/未决 item queue-item-not-found/steer

      前置 steer-unavailable 词汇，3 项测试全绿；累计 81 项）；

- [x] apiproxy session.prompt（第 105 轮：时区规范化（invalid-time-zone

      词汇）+ queue/steer 投递（rpcId/clientTimeZone 源透传）+ 图片准入       暂拒，3 项测试全绿；顺带修 workspace 测试并行时间戳撞车；累计       84 项）；

- [x] apiproxy session.attachment（第 106 轮：会话引用授权（递归图片

      引用扫描 + ATTACHMENT\_NOT\_REFERENCED）+ 图片读取 base64 应答，       2 项测试全绿；累计 86 项）；

- [x] apiproxy session.search（第 107 轮：可见性授权边界 + 预算循环

      （INVALID\_LIMIT 半减/STALE\_CURSOR 重置/游标去重/调用上限）+       snippet 截断，2 项测试全绿；**session 域 12 方法全部接线并测试       闭环**；累计 88 项）；

- [x] apiproxy subagent 域（第 108–109 轮：list/history/prompt/interrupt

      4 方法接线 + 4 项测试全绿（空目录与父可用性翻转/中断火忘确认/       父缺失词汇/服务缺失）；累计 92 项）；

- [x] apiproxy mux 事件流（第 110 轮：subscribed 基线 + session/event

      订阅转发（SSE 端到端 1 项全绿）；approval/question/queue/jobs/       projection 基线随各里程碑；累计 93 项）；

- [x] apiproxy host 事件流（第 124 轮：events\_host 从空占位升级为完整

      帧流——session/created → host/session-added（blank + header 字段       投影）、session/disposed 与 workspace/session-deleted →       host/session-removed、domain/changed → workspace 帧家族（global 表       Put：新增 workspace/顺序变化/归档集合变化；workspaces 表       Deleted/Put：移除/既有实体变化，经 workspace\_record\_view 从 record       JSON 构造视图）、11 个 allowlist 事件（TS API\_REMOTE\_FORWARDED\_       EVENTS 原样清单）→ host/remote-event（args 转 JSON，不可序列化       参数跳过）；基线去重状态与 TS 同构；1 项组合测试全绿（session-       added 帧 + remote-event 帧）。顺带修复实现期缺陷：13 处       ctx.on(...) 注册漏 await 导致监听器从未注册；偏差：agent/status       与 agent/error 帧等待 dsh-agent 的状态/错误事件发布（Rust       registry 目前只发 created/disposed）、registry 缺失 workspace 时       跳过而非 throw）；

- [x] apiproxy native-path-opener（第 111 轮：跨平台打开器 1:1（浏览器

      意图/macOS 默认浏览器/WSL 翻译/PowerShell 字面量/canOpenNativePath/       text-editor 意图），6 项测试全绿（命令构造断言 + 可达矩阵）；累计       99 项）；

- [x] apiproxy downloads.sessionLog 真实接线（第 114 轮：deps 解析

      500/501/404 语义 + flush 屏障 + root artifact 读取 + zip 组装 +       content-disposition；顺带修复 dsh-session-query corpus.rs 锁跨       await 的 !Send future 缺陷）；**apiproxy 组合层全部实质接线完成**；       累计 103 项）；

- [x] apiproxy agentPreset 域接线（第 122 轮：6 方法

      list/select/read/copy/open\_document/remove 全部接线——list 空部署       空 roster + trust/isDefault/authorable/hasDocument；select per-session       单飞链 + 链内重读 blank（started → agent-preset-locked）+ recompose

      - 成功才 append agent-preset/selected；read resolve 类型化 +

      read\_composition；copy/remove 服务方法 + 错误分类器（not-found/       read-only/invalid/exists 按 thiserror 固定模板精确映射）；       openDocument trust 门（shipped → read-only）+ dirname +       can\_open\_paths 三级回退（注入 can → 注入 opener → 平台探测）+       open\_target（abort → cancelled）；9 项组合测试全绿。顺带修复       dsh-agent-presets recompose 同线程重入死锁：`match       self.bindings.lock().get(...)` 的 parking\_lot guard 存活到整个       match 表达式，None 分支的 insert 重锁同锁永久挂起——改为先       cloned() 快照再 match（dsh-scope 的 ScopeParentBinding 补       #\[derive(Clone)\]）；原 64 项测试未覆盖 recompose None 分支故       第 121 轮全绿时未暴露）；

- [x] apiproxy approval/question respond + 物理 HTTP/SSE 闭环（第 126 轮：

      pending registry、requested 重放、rpcId 一次性认领、严格 question       answer/approval 关联校验、resolved/cancelled 广播、并行审批配对与       response-vs-abort 首次认领；mux stream Drop/idle abort 同步释放       listener，`session/subscribed.lastSeq` 修正为 `session.seq - 1`；       dsh-host 挂载 ApprovalService 与 `/api` fetch bridge，160 MiB body cap、       bytes/SSE 流映射、Host + Sec-Fetch-Site + Origin loopback trust fence；       真实端口验证 POST `/api/respond` 200 receipt 与 GET `/api/events.mux`       200 `text/event-stream`。交互 12 项、Host boot 8 项、approval 26 项、       mux 聚焦 1 项全绿）；

- [x] Rust CLI、静态 shipped profile、production runProfile 与信号退出（宏阶段 1：

      `dsh web`/headless/plugin 真实入口；Web 任意 cwd + SPA boot data；       DeepSeek HTTP/SSE；Ctrl+C 早到信号锁存、Pending stream 取消、durable flush、       Agent/Session 完整解绑及 Host 有界 shutdown；动态 `!!js`/HMR 仍归后续里程碑）；

- [x] dsh CLI 入口骨架（第 116 轮：parseDshArgs 1:1（profile/web/plugin/

      dump-config 模式 + 内参透传 + help/version/错误语义）6 项测试全绿

      - bin 冒烟三路径（--version/-h/缺 profile 均正确退出码）；profile

      boot 与 plugin 转发留待 profile-boot 里程碑）；

- [x] dsh profile-boot 组合层（第 119+ 轮：home\_patch\_path/

      resolve\_telemetry\_patch（任意非空值禁用）/prepare\_profile（空根       配置重写）/ComposedProfile + compose\_profile（bundle→profile→       home→--patch overlay→telemetry 五层栈 + rows 索引）/all\_patches/       compose\_live（用户层每代重读）；4 项测试全绿（insert 数组形状       对齐 TS applyEntryPatches）；累计 10 项。宏阶段 1 已补齐静态 shipped       web/headless runProfile、首次初始化、生产 DeepSeek 与信号生命周期接线；       剩余偏差为动态 `!!js`、完整 Node 模块解析、HMR 与       healProfilesModuleFallback）；

- [x] dsh-e2b 命令面扩展 + subprocess-e2b 适配器（第 125 轮：

      dsh-e2b 的 E2bSandbox::run 升级为 E2bCommandOptions（envs/cwd/       流式 on\_stdout/on\_stderr 回调/signal），新增 E2bBackgroundOptions +       E2bCommandHandle（pid/wait/kill/send\_stdin/close\_stdin）与       run\_background（TS commands.run 的 background 模式）；新建       crates/e2b/subprocess-e2b（TS packages/e2b/subprocess-e2b 核心子集：       environment（远程环境 base64 探针/scrub/bootstrap/serialize）、       remote（wait\_tick/signalRemoteGroups 容错信号）、output       （E2bBase64Decoder 帧协议 + E2bOutputReader 尾窗/spill 广告）、       process（E2bSubprocessHandle：bootstrap shell + 构造即启动的 run       状态机——环境探针→state 目录→run\_background→pid 校验→批量       stdin→pgid/exit-code 轮询→outcome；TERM→grace→KILL 阶梯）、       index（E2bSubprocessRuntime：install/dispose/resolve\_executable       1:1/spawn 校验链）；6 项聚焦测试全绿（resolve 两路径、spawn       校验、exit-code 结算、terminate 阶梯、环境词表）。偏差：       spawnTerminal 与 terminal 阶梯留待 terminal 里程碑（fail-loud       占位）、pipe 流为 mpsc 桥接（TS PassThrough 背压为尽力语义）、       输出编码器远端注入留待真实 SDK 后端）；

- [x] dsh-host 生产组合挂载 agent-presets（第 123 轮：compose\_host 在

      loader 之后挂 AgentPresets——default=standard、shipped root 为       manifest 锚定的 config/agent-presets system 根（cwd 无关）、       include\_user\_root=true + 生产 env；HostSpine 增 agent\_presets 字段；       boot\_report services 增至 11 项并新增 probe.presetCount（真实       discovery 读）；boot 集成测试断言服务可解析 + presetCount≥4；       `cargo run -p dsh-host` 冒烟输出 11 服务 + presetCount 5（4       shipped + 1 user）；dsh-cli 的 compose\_profile 同步解除       shipped-root 跳过偏差——resolve\_agent\_presets\_patch 1:1 TS       composeProfile 159-167 行（保留行内 config、仅覆盖 roots，经       AGENT\_PRESETS\_ROW\_ID 常量 + shipped\_preset\_root() manifest 锚定）；       dsh-cli 11 项测试全绿）；

- [x] dsh-app-boot render\_config\_dump + dsh dump-config（第 120 轮：

      ConfigDumpLayer/render\_config\_dump 1:1（逐层前缀快照 positional       diff provenance、`# == origin, patched by ...` 分组注释、unmatched       patch 按层标签 warn、!!js 经 yaml.rs 反向转 Tagged verbatim 输出）

      - load\_profile\_with\_user\_layer（defaultOnly 恢复诊断跳过解析损坏

      用户层）+ dsh-host-cli run\_dump\_config（bundle 层→用户层→home 层       →--patch overlay 标注输出与警告分离）；app-boot 12 项/dsh-cli       11 项测试全绿；偏差：%C printf 替换无对应（Rust warn 为字面量））；

- [x] dsh-host 外壳组合升级（第 117 轮：loader + webserver（OS 端口）+

      frontend-static（web/dist 托管）+ directory-picker browse +       plugin-inventory + apiproxy 网关；cargo run exit 0 且 durability/       search 探针全过；顺带处理 D 盘 172 GB 构建产物积压（cargo clean））；       浏览器级 GUI 验证留待 CLI profile-boot 组合后）；

- [x] dsh-app-boot profile 核心（第 118 轮：profile 目录解析/名称校验/

      初始化脚手架（manifest/patch/pnpm 工作区幂等）/manifest 解析/       bundle 层目录式解析/patch 文件解析/optional+overlay 装载语义/       compose\_entries 扁平应用；3 项测试全绿；Node 模块解析与 watchUser       Patches/boot 组合留待 app-boot 完整里程碑）；

- [x] dsh-app-boot boot 核心（第 118+ 轮：mount\_entries/assert\_entries\_

      loaded/assert\_entries\_activated/boot 落地；错误链 1:1 对齐 TS——       mount 与 settle 失败统一 `plugin tree failed to load` 标签，import       失败透传 loader 的 `failed to import loader entry {id} ({name})`       诊断（LoaderError Display 与 TS updateError 逐字一致），fiber-less       entry 审计输出 TS 格式 `plugin(s) failed to load: {names}`；5 项       测试全绿（新增 boot 成功/未知插件名错误链、fiber-less 审计）；       install\_fail\_loud/watch\_user\_patches 与 profile-boot CLI 接线       留待后续）；

- [x] dsh-app-boot install\_fail\_loud（第 119 轮：FailLoudProcess trait +

      FailLoudGuard；单次 latch 只报告首个 rejection、诊断先于 release、       release 竞速 FAIL\_LOUD\_RELEASE\_TIMEOUT\_MS 超时后强制 exit(1)、       release 自身失败吞掉、uninstall 摘除；3 项测试全绿（无 release       立即退出/挂起 release 虚拟时钟超时/卸载失效）；累计 8 项测试；       TS assembledActivationRejections checkpoint 集合留待 activation       审计改写为 rejection 收集后接线）；

- [x] dsh-app-boot watch\_user\_patches + include patches-only 更新（第

      119+ 轮：修复 dsh-cordis-include internal/update 监听与 TS 相反的       移植 bug——TS index.ts:206-213 是 path 相同才重应用 patches（消费       事件）、path 不同透传；Rust 原实现反了。新增       apply\_patches\_with\_config（已读 data 重应用不重读文件）+       config\_update\_reapplies\_patches\_without\_rereading 集成测试（14/14       全绿）；app-boot 落地 refresh\_user\_patches（保留 include 非 patch       配置/compose 应用/entry config 更新驱动重应用）与       watch\_user\_patches（UserPatchWatcher 注入 HMR register\_config 面、       INACTIVE\_EFFECT → no-op disposer、hmr 缺失错误文案逐字对齐；3 项       测试全绿（端到端 patch 文件变化 probe 配置 1→2→3 + path 保留/       hmr 必需/INACTIVE\_EFFECT no-op）；累计 11 项测试。偏差：root       Include entry 直接传入（TS bootstrapIncludes WeakMap 注册表随       root-include mount 移植）、HMR 服务注入式（dsh-cordis-hmr crate       尚未移植））；

- [x] dsh-agent-presets 落地（第 121 轮）：新 crate

      `crates/preset/agent-presets`（TS `@deepseek-ai/dsh-agent-presets`       全量：词表（PresetTrust/AgentPreset/PresetRoot/Config/PRESET\_ID/       UnknownPresetError/PresetMountError）、metadata 显示元数据       （read/render：坏 YAML/非文本/空白/identity 不可携带 + integral       order 渲染）、session 解析（resolveSessionPreset：header 创建值 +       agent-preset/selected 事件末次胜）、discovery（scanRoot/       discoverPresets：id 模式门控、缺失/坏 YAML/形状错误 → broken 行、       order+id 排序、first-root-wins）、authoring（copy 整目录递归解引用       symlink + tightenModes 0600/0700 + metadata 重写（描述保留/名字       丢弃）+ 失败清理、delete 的 shipped/containment 拒绝、writableRoot       首个 user root）、mount（PresetTreePlugin fiber 承载 Include 子树、       readonly + no-op write-back（self-dispose 不截断文件）、inactiveRows       审计（inject 缺失诊断）、leakedServices 根 realm 泄漏审计（isolate       realm 放行）、standingMountFor/serviceForAgent/livePresetMounts）、       AgentPresets 服务（settings 段注册（base default 层）/agent/created       警告（advisory）/session/event 转发 agent-preset/selected/standing       单飞 + compositionStamp 换代/挂载/composeFrom/composedPreset/       recompose/standingKeyFor/copy 查重/remove 清默认）、invariant 伴生       （internal/service 重查泄漏 + assemble 未 join 检查（无 agent 接线       暂惰性））；顺带修复 dsh-cordis-loader 两处移植缺口：       EntryOptions.isolate 接受 TS 的 `true`（entry-local realm）形式 +       LoaderError::Aggregate Display 展开每行 cause（对齐 TS       mountDetail）；64 项测试全绿（lib 30 + mount 8 + authoring 7 +       roster 8 + invariant 2 + 词表/元数据/发现）。偏差：ScopeKey 为       不透明身份（TS `{agentPreset:id}` 结构化键）；bindings 强引用表       （TS WeakMap）；dsh\_home\_path 经注入 env 闭包（测试隔离 DSH\_HOME，       生产传进程 env）；localeCompare 塌缩为码位序；Windows 无 chmod       语义（cfg(unix) 收紧）；settings 默认读取经 to\_json 投影；       AssembleContext 无 agent 字段（unjoined-agent 检查惰性）；inject       回调 config 参数与 TS 依赖值不同（服务经 ctx 读取）。       **apiproxy agentPreset 域阻塞自此解除**（wire 契约第 90 轮已落地，       下一步接线 6 方法）。

- [ ] 托管现有 `web/dist`，用现有 GUI 完成浏览器验证。

### M7 — 全量一致性与切换

- [ ] 导入/移植上游后端测试；
- [ ] golden wire/storage/session fixture；
- [ ] Windows/Linux/macOS CI；
- [ ] 性能与稳定性回归；
- [ ] Rust Host 作为默认可执行程序，TS Host 退为兼容参考；
- [ ] 1:1 移植完成声明。

## 5. 已知高风险项

0. **负载敏感抖动测试**：`dsh-bash-local` 的

   `start_returns_immediately_with_a_running_handle_that_settles_as_completed`    与 `dsh-credentials-local` 的    `keeps_both_refs_when_two_providers_write_the_same_document_concurrently`    （第 111 轮起另见 `dsh-tool-jobs` 的    `keeps_the_complete_pty_job_id_and_collection_action_at_the_minimum_pty_limit`    ——全量并发下 owner job 计数串扰报 "background job limit reached"，    单 crate 42 项必过）    （第 82 轮起另见 `lets_a_non_empty_process_environment_win_read_only_over_the_file`    ——同一二进制的并行测试共享进程 env，`unsafe set_var` 探针在全量    并发下偶发读不到，单独运行必过；第 83 轮起另见 dsh-settings 的    `mutate_applies_path_ops_on_the_current_section`——共享全局 storage    桩在全量并发下偶发串扰，单 crate/单测必过；第 124 轮起另见    dsh-host-apiproxy 的    `list_merges_attached_and_cold_sessions_sorted_by_updated_at`——同一    测试二进制的三个并行测试共享进程级 session 格式状态，v1 日志偶发    报 "reads only v0"，单独运行 3/3 必过）在全量并行负载下偶发    时序抖动（单独运行必过），与移植无关；第 70 轮    机器重载时曾连续 3 次全量命中（150ms 启动时限断言）、隔离运行 14/14    绿、第 4 次全量即通过——重试直到绿即可，后续加宽容差或串行化。

1. **动态 JS 插件与 workflow 脚本**：TS 版依赖 `node:vm`；Rust 需要嵌入式 JS runtime

   （优先评估 `deno_core`，备选 Boa）并保持授权、隔离、模块解析和私有 RPC。

2. **Typert/TS 类型生成**：TS conditional/keyof/template literal 类型无 Rust 直接对应，

   需 trait/associated types + build-time codegen。

3. **Cordis Proxy 调用重绑**：TS 会将 service method 的 `this.ctx` 重绑到调用方；

   Rust 当前采用"caller Context 显式参数 + Context 固有 mixin 方法"，    最终必须通过 conformance 测试证明行为等价。

4. **事件同步边界**：TS `emit/bail/on` 有同步语义，Rust listener 是 Future；

   当前实现保持顺序/结果语义，但存在 async boundary，需要进一步做同步/异步 listener 适配。

5. **Windows 安全**：受限令牌、能力 SID、DACL、COM picker、Job Object/PTY 均需真实 Windows 集成测试。
6. **存储兼容**：JSONL/zstd/SQLite 必须通过现有 fixture，不能只做"相似格式"。

## 6. 下一步（宏阶段 1 封板状态）

已落地：workspace 成员 65+ 个 crate（vendor/cordis 生态 7 包 + core + util + session 12 + settings + llm + skill 4 + plan + schedule + subagent 契约层/服务核心/进程内驱动/fork/spawn/tool-subagent/投影/枚举/ continuation + jobs 三件套（seam/local/tool-jobs）+ fs 四件套（seam/ local/observation-policy/sandbox）+ tool-str-replace-editor + storage/workspace/spill/credentials/sandbox/subprocess/terminal/shell/ code-runtime/goal/context/guard/identity/todo/attachment/interaction/ feedback/compaction 各分组核心 + preset/agent-presets（第 121 轮）+ apiproxy agentPreset 域 6 方法接线（第 122 轮，9 项组合测试）+ dsh-host 生产组合挂载 agent-presets + profile-boot shipped-root 偏差 解除（第 123 轮）+ apiproxy host 事件流（第 124 轮）+ **subprocess-e2b 适配器（第 125 轮，6 项聚焦测试）** + **ApiProxy approval/question respond 与真实 HTTP/SSE、安全 fence 闭环（第 126 轮）** + **command-goal 人类命令与生产 Host goals/invariant 闭环（第 127 轮）** + **tool-goal 模型入口、真实 AgentLoop/Host 生产闭环（第 128 轮）** + dsh-host 可启动组合），第 128 轮当时的 `cargo test --workspace` 快照为 1747 passed / 0 failed / 1 ignored（374 个结果分组）；当前权威验收见 §1。

宏阶段 1–3 已完成 **Rust 产品入口、本地执行与外部协议闭环**：静态 shipped web/headless profile、production runProfile、DeepSeek HTTP/SSE、Host 生命周期与 Ctrl+C 收敛、 PowerShell foreground/background、job 工具、Windows ConPTY、持久终端模型工具和 AppContainer/ACL 文件边界均已落地。Agent/Session/PTY/进程树在 shutdown 后完成解绑 和有界清理；MCP/LSP/ACP、Python SDK、默认 spawn 与 Codex 出进程 subagent 均通过 真实协议 fixture。最终 workspace 为 1885 passed / 0 failed / 1 ignored，独立 P0/P1 复核通过。动态 `!!js`、完整 Node 模块解析、workflow/HMR 与浏览器级 GUI 仍保留到 最终里程碑。

未完成（按剩余工作量排序）：

1. 宏阶段 4：JS runtime 决策与 code-runtime worker；
2. workflow（engine/worker/tool/ralph——依赖嵌入式 JS 运行时，独立

   里程碑）；

3. 平台收尾：Linux Landlock 实际 launcher；E2B 三件套

   已完成 core + fs 适配器（第 82–83 轮）+ subprocess-e2b 适配器    （第 125 轮），真实 HTTP SDK 后端留待后续；subprocess-e2b 的    spawnTerminal/terminal 阶梯与 PTY 真实 node-pty/ConPTY 后端    （第 81 轮落地终端 handle 与进程检查层逻辑，真实 OS 绑定留待    后端里程碑）；credentials-encrypted/OS-keychain 自 backlog 移除    （TS README 明言 deferred，仓库无此包）；

4. M6 外壳剩余：动态 profile/HMR 与托管现有 `web/dist` 的浏览器级 GUI 验证；
5. M7 全量一致性：conformance fixture、golden wire/storage/session

   数据、CI 矩阵、Rust Host 默认入口与 1:1 完成声明。
