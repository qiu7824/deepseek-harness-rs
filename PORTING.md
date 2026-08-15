# DeepSeek Harness Host → Rust 全量 1:1 移植状态

> 基线：`@deepseek-ai/dsh-root 0.1.0-rc.5`
> 源码（只读）：`D:\HermesTemp\deepseek-harness`
> Rust 项目：`D:\deepwork\deepseek-harness-rs`

## 1. 规模与范围

自动统计文件：`docs/porting/loc.json`（脚本 `docs/porting/count-loc.mjs`）。

| 指标 | 数值 |
|---|---:|
| workspace/package 记录 | 241 |
| 全仓源码行 | 237,817 |
| 全仓测试行 | 293,638 |
| Host/后端及共享基础包（排除纯浏览器包） | 约 192 |
| Host/后端及共享基础源码行 | 161,745 |
| Host/后端及共享基础测试行 | 206,707 |

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
|---|---|---|
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
- Windows 受限令牌 + WRITE_RESTRICTED + ACL 能力 SID，失败不透传；
- Linux bwrap/landlock、macOS seatbelt 选择与探测 fail-closed；
- subprocess 进程树、PTY、kill、stdio/spill 行为一致；
- 凭证脱敏、路径边界、批准栈与会话所有权授权一致。

## 4. 阶段计划与进度

### M0 — 建仓与全量盘点

- [x] 创建独立项目 `deepseek-harness-rs`；
- [x] 复制 `apps/web` → `web/`（排除 node_modules，保留 dist）；
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
      20 节点类型、meta 链、simplify/i18n/toString、Standard Schema V1；
      toJSON/fromJSON 留待 M2 settings 持久化）；
- [x] `timer`（`crates/vendor/timer`，crate `dsh-cordis-timer`，9 项测试全绿）；
- [x] `loader`（`crates/vendor/loader`，crate `dsh-cordis-loader`，12 项测试全绿；
      **插件模块解析改为静态注册表，`!!js` 表达式显式报错（留待嵌入式 JS 运行时）**）；
- [x] `include`（`crates/vendor/include`，crate `dsh-cordis-include`，13 项测试全绿）；
- [x] `logger-console`（`crates/vendor/logger-console`，7 项测试全绿）；
- [x] `group`（`crates/vendor/group`，loader 的 GroupPlugin 注册别名）；
- [ ] 对照上游 `vendor/cordis` TS 测试建立 conformance fixture；
- [ ] `hmr`（按计划裁剪后置）；
- [ ] profile patch、interpolate、entry tree 与状态投影（patch/interpolate 已落地，profile 编排待 app-boot）。

当前 Rust 代码：`crates/vendor/{cordis,cosmokit,schemastery,timer,loader,include,
logger-console,group}` + `crates/core/{scope,llm,typert-protocol,session,system-prompt,agent,agent-default-model,tools,agent-tool-presentation,agent-loop}` +
`crates/session/{session-persistence,session-persistence-jsonl,session-persistence-sqlite,session-projection,session-projection-cache,session-stats,session-telemetry,session-title,session-title-llm,session-title-all-prompts-llm,session-title-first-prompt-llm}` +
`crates/settings/settings` + `crates/llm/{llm-retry,token-meter}` +
`crates/storage/{storage,storage-domain,storage-json,storage-sqlite,storage-test-support}` +
`crates/workspace/workspace` +
`crates/spill/{spill,spill-local,spill-policy}` +
`crates/credentials/{credentials,credentials-local}` +
`crates/sandbox/{sandbox,sandbox-policy}` +
`crates/fs/{fs,fs-local,fs-observation-policy,fs-sandbox}` +
`crates/subprocess/{subprocess,subprocess-local}` +
`crates/terminal/terminal` +
`crates/shell/{shell,bash-local}` +
`crates/code-runtime/code-runtime` +
`crates/jobs/{jobs,jobs-local}` +
`crates/goal/goal` +
`crates/util/{brand,timeout,atomic-write,home-paths,output-retention,invariants,launch-environment,native-command}` +
`crates/session/session-checkpoint-policy` +
`crates/context/time-context` +
`crates/guard/{repeat-tool-reminder,timeout-policy}` +
`crates/identity/anonymous-user-id` +
`crates/todo/tool-todo` +
`crates/attachment/{attachment,attachment-local}` +
`crates/session/session-query` +
`crates/context/session-reference` +
`crates/interaction/commands` +
`crates/feedback/command-feedback` +
`crates/feedback/message-feedback` +
`crates/host/dsh-host`（可启动主程序） +
`crates/compaction/compaction` +
`crates/interaction/{user-questions,tool-ask-user,user-approval,permission-presets}` +
`crates/skill/{skill,tool-skill,skill-badge,skill-filesystem}` +
`crates/plan/plan-mode` +
`crates/session/session-query-sqlite` +
`crates/schedule/schedule` +
`crates/subagent/subagent` +
`cargo test --workspace` 1302 项全绿（第 30–33 轮 storage Hub/domain/json/sqlite 落地；
第 34 轮 dsh-workspace 落地：49 项测试，workspace.spec.ts + invariant.spec.ts 全部移植；
第 35 轮 spill 三包落地：spill-local 11 项 + spill-policy 16 项；
第 36 轮 launch-environment 7 项 + credentials seam 9 项；
第 37 轮 credentials-local 落地：49 项测试，local/drain/watcher/review-fixes 四个 spec
全部移植（行级注释保留 YAML 编辑、分层解析、写锁读改写、包含扇出、fake watcher
管道、真实 notify 热重载）；
第 38 轮 dsh-sandbox seam 落地：15 项测试，vocabulary/escalation/roots 三个 spec
全部移植（严格加宽梯、参数配对校验、模型面 marker、approveEscalation 有序
fail-closed 序列、canonicalPath/writableRoots 派生）；
第 39 轮 dsh-fs seam 落地：12 项测试，service.spec + invariant.spec 全部移植
（FsTargetKey/FsVersion 品牌、FsInfo/FsPathInfo/FsDirEntry 词表、FsError 码表 +
cause 链、FileSystem 抽象服务 14 原语、internal/dispatch 事件数据不变式）；
第 40 轮 dsh-fs-local 落地：20 项测试，fsio.spec + filesystem.spec 核心子集移植
（realpath 身份、祖先回退解析、探针/列表、UTF-8 严格读/跨块流、字节上限、
字面编辑+行尾往返、私有 staging 原子发布、hard-link 守卫创建、per-target 锁
并发写/编辑确定性）；
第 41 轮 dsh-fs-observation-policy 落地：18 项测试，policy.spec 全部移植
（观察态 gate 的 write/edit 意图派生、present→absent→present 转移、多 owner
隔离、单槽 first-wins 短路、dispose 状态释放与监听器移除）；
第 42 轮 sandbox-policy + fs-sandbox 落地：21 项测试（政策服务默认/会话解析/
审批覆盖优先级/sandbox-mode 会话套件/事件不变式 + 包含判定矩阵 + 每调用
策略栅栏：只读拒绝、工作区包含、`..`/symlink 逃逸拒绝、TOCTOU 方向重解析、
升级覆盖）；
第 43 轮 dsh-subprocess seam 落地：4 项测试，service.spec 全部移植
（完整 spawn 词表：三态 stdin/输出模式、有界收集 + spill、offset 读取器、
树级终止语义、terminal 原语六法、scrubbedParentEnv 双擦洗）；
第 44 轮 dsh-subprocess-local 落地：14 项测试，spawn.spec + local.spec 核心
子集移植（childEnv 擦洗合并/Windows 大小写折叠、OutputCollector 字节精确
尾窗 + 惰性 spill 溢出丢弃、全隔离进程树 spawn、SIGTERM→grace→SIGKILL
升级、abort 谓词反应、批次 stdin、可执行解析、服务注册/释放与 fiber 处置
终止整树；spawnTerminal 桩留待 PTY 里程碑）；
第 45 轮 dsh-terminal 落地：23 项测试，service.spec 全部 23 例移植
（后端注册精确贡献释放、owner 栅栏、spawn 发布/回滚、调用者取消、
owner/服务处置对未发布 setup 的 abort+await、后端侧清理失败保留至处置、
关闭幂等与聚合、处置清注册表；invariant 伴生 no-op 注册）；
第 46 轮 dsh-shell + dsh-bash-local 落地：28 项测试（shell seam 7 项——
render.spec 退出标记解析契约 + service.spec 抽象执行器桩；bash-local 21
项——executor.spec 前台运行/超时/abort 分类/stdin-env 线程/后台进程句柄
增量读/损失标记/spill 路径/kill 升级/失败结算 + settings.spec 设置段
user 层解析/写入校验/存储段服务/供应商脱落回退/无供应商入口/命名空间
释放）；
第 47 轮 dsh-code-runtime 落地：9 项测试，reserved.spec + service.spec
全部移植（可移植标识符排除集：RESERVED_BINDING_GLOBALS/RESERVED_ERROR_
MEMBERS/DUNDER_MEMBER/PORTABLE_RESERVED_WORDS 全仓共享契约 + 抽象
CodeRuntime 桩：language/isolation/run 三原语、失败为结果字段、预中止
abort 失败、fiber 卸载移除、重复注册 fail-loud）；worker-thread 后端
（bootstrap/worker-json，需嵌入 JS 运行时）留待后续里程碑；
第 48 轮 dsh-jobs + dsh-jobs-local 落地：19 项测试（jobs seam 5 项——
service.spec + invariant.spec 全部移植：抽象 JobRegistry 九法、JobId
品牌、快照跨字段不变式 + jobs-inject 安装器；jobs-local 14 项——
jobs.spec 核心子集：入站预检（无控制器/空 kind/空 label/非法
outputLimitBytes/owner 未注册）、按 kind 顺序 id、每 owner 并发上限、
session 栅栏、流式/终态读与 reported 标记、kill 两态、首胜结算 + 监听
通知、有界 wait（结算/超时/中止）、teardown 取消与抛错强制失败、
owner 处置取消并删除记录）；
第 49 轮 dsh-goal 落地：12 项测试，goal.spec 核心子集 + 严格折叠套件
（事件溯源 goal/change 全量快照 + 清除墓碑、CAS revision 比对、create/
edit/pause/resume/complete/block/clear 七动词 + 阶段梯校验、目标边界
校验（objective/maxGoalRounds/blockReason）、进程本地 activation
（armed/disarmed + session-start disarm 边）、round 准入折叠（user/
message goal 源）、goal/changed scoped emit、strict 解码器 fail-loud
（坏版本/坏目标/字段漂移/跳过 revision/预算耗尽））；@Remote 注解与
投影单元注册留待 typert/投影集成；
第 50 轮 dsh-native-command + dsh-session-checkpoint-policy 落地：8 项
测试（native-command 4 项——无 shell 执行器 utf8 捕获/非零退出 code/
ENOENT/abort 传播 + Windows console hide；checkpoint-policy 4 项——
语义持久化检查点：llm/stream 工厂包装（flush 先于首块、失败 fail-closed
终态块）、tools/execute 顶层检查点 + 预分派 abort 规范结果、agent/
pre-step 边界 flush）；
第 51 轮 dsh-time-context 落地：20 项测试（timestamp/request-zone/
index/invariant 四个 spec 核心全量——ICU 级 IANA 规范化（jiff + 内置
tzdb 2026c 链接表 + CLDR Etc/UTC 折叠）、ISO 形时间戳格式化、浏览器
请求时区派生（resolved/mixed/missing 排序去重）、preceding/latest
事件扫描、prepended pre-step 瀑布监听器 + snapshot 形式注入与 refresh
间隔抑制、fiber 处置移除监听器、纯函数不变式 + 增量历史缓存的
伴生注册（internal/dispatch 内联钩子在 append 持锁下运行，伴生自维护
会话历史避免锁重入）；MessageSource::User 扩展 rpcId/clientTimeZone
合并增强字段（线格式 skip-if-None 保持兼容）；
第 52 轮 dsh-repeat-tool-reminder + dsh-timeout-policy 落地：22 项测试
（repeat-tool-reminder 13 项——per-agent 连续重复链（deep key-sort
规范化 + JSON.stringify 整数格式、通配符字面转义、阈值 fail-loud
校验/升序归一、gentle@thresholds[0]→detailed 升级、include/exclude
透明谓词、用户 pre-step 重置、block/accept 决策折叠保留下游元数据）；
timeout-policy 9 项——tools/execute 包装：无预算透传、派生 deadline
信号换入/还原、自有 TOOL_TIMEOUT 结构化替换（协作工具与提供方 abort
错误）、上游先中止保留注册表 ABORTED、deadline 先赢保留超时、fiber
处置移除监听器；全局工具对无 agent 直调也可解析预算）；
第 53 轮 dsh-anonymous-user-id 落地：10 项测试，anonymous-user-id.spec +
invariant.spec 全部移植（harness-home 作用域匿名身份：bare UUID 行持久化
到 `.anonymous-user-id`、缺失主目录递归创建、空白容忍、损坏覆盖、
wx 独占创建并发胜者采纳、只读 home best-effort 内存 id、按解析路径
进程级 memo、默认进程 env、空安装器伴生注册 + 包名保留 fail-loud）；
第 54 轮 dsh-tool-todo 落地：12 项测试（`todo_write` 工具全量：
注册 schema 形态、整表替换追加 todo/write、content trim 规范化、
单/并行 in_progress 策略 + 描述文案差异、schema 级拒绝（未知键/
坏状态/非数组）+ 值级拒绝（空/重复）、无 agent 拒绝、presentCall
presentation、fiber 处置注销、`todos` 投影单元（整表折叠 + turn/start
清除 + 无关事件同引用）+ 持久化形状不变式伴生（trim/唯一/状态枚举）；
参数校验在体内用共享 JSON Schema 引擎执行——Rust 工具运行期尚未在
dispatch 前校验输入（已记录偏差））；
第 55 轮 dsh-attachment + dsh-attachment-local 落地：8 项测试（attachment
seam 词表 + 错误码类 + 不可变存储抽象三原语（imageLimits/validate/
save/read + 谓词取消）；attachment-local 内容寻址后端：四格式光栅
解码（探测头 vs 全解码准入、像素上限先于解码）、sha256 对象存储
（对象/桶/暂存布局、独占创建 + 硬链接去重、EEXIST 冲突校验、临时
清理）、displayName 双分隔符剥离、嵌套 home 创建、失败封闭（缺失/
损坏/非法引用/元数据不匹配/写失败映射稳定码）、abort 谓词取消、
服务边界默认限值 + 校验不落盘；POSIX 目录 fsync 顺序断言留待
（实现保留相同 sync 结构，Windows 无目录 fsync 可观测面）；
第 56 轮 dsh-session-query 落地：7 项测试（组合会话查询服务 seam：
17 码错误类 + 配置/游标品牌、字面大小写不敏感空白弹性文本过滤器
（regex 注入安全）、AND 会话/事件谓词（id/cwd/created-at/parent/
availability + seq/time/type/surface/text）、一阶语义文本抽取（消息/
工具调用结果/todo/turn-end 分派）、规范化 surface 折叠分类
（current/shadowed/log-only）、live 优先逻辑语料（sessions 服务 +
可选 persistence 擦除绑定 + 头兼容断言 + 并发投影）、系谱/事件关系
追踪、标题折叠/表面读取/事件窗口等引擎原语 + 抽象搜索面
（sqlite 后端后续接入）；SessionPersistenceApi 增加 dyn 擦除服务注册
（fs 同款抽象服务约定）；
第 57 轮 dsh-session-reference 落地：6 项测试（跨会话快照引用服务：
dsh-session: URI 规范化编解码（base64url of JSON 字符串 + 往返
canonical 校验）、Markdown mention 转义解析 + 裸 URI 提取、tag-safe
JSON 序列化（< 转 \u003c）、字节预算保留（最老非 checkpoint 优先丢 +
最长消息 head/tail 截断 + 省略通知）、prepare 流程（normalize 去重/
自引用/上限、readSurface 快照、recall 形式插件源 + 不可信提示信封）、
候选排序（cwd 亲缘 + 标题标签 + 排除自身）；SessionReferenceSource
结构化源暂以 plugin 源表示（MessageSource 未扩展 session-reference
kind——记录偏差）；
第 58 轮 dsh-commands 落地：8 项测试（插件所有的人类命令注册表：
斜杠行解析（名称/边界校验）、注册规范化 fail-loud（名称模式/描述/
输入提示/重复名 panic）、ScopedLayers 分层 + 作用域遮蔽 + 名称排序
描述符、execute 生命周期（command/run + command/done 配对 commandId、
args 与 recordInput:false 省略、来源标注）、handler 失败/abort 结算为
error 记录并重抛、实例令牌前缀的配对 id 铸造、pairing 不变式伴生
（run 唯一 + done 配对 + sourceEventSeq 校验，增量历史规避 append 锁
重入）；handler 为 async Result 闭包（TS 同步/异步 throw 塌缩为 Err
通道——记录偏差）；
第 59 轮 dsh-command-feedback 落地：7 项测试（会话反馈域事件 +
人类 `/feedback` 生产者：注册（描述/输入提示/recordInput:false）、
确认文案 + 三种共享政策披露（sessionTelemetry 服务新增 dyn 擦除注册
补齐 TS ctx.sessionTelemetry 契约）、trim 规范化、空输入 usage 错误、
command/run → feedback/record → command/done 事件序、独立记录与
log-only 保证、反馈载荷仅出现一次）；
第 60 轮 dsh-host 可启动主程序落地：2 项测试（M6 骨架——核心服务
组合（sessions/agents/systemPrompt/tools/invariants）+ 包属不变式伴生
挂载 + 启动报告（服务清单/会话 seq/工具计数）+ `dsh-host` 二进制
实际运行输出报告 exit 0，目标「可启动运行」达成首个可执行产物；
webserver/apiproxy/CLI 继续叠加）；
第 61 轮 dsh-message-feedback 落地：4 项测试（生命周期绑定的逐消息
评分侧车：storage-domain 域声明（行 schema 校验：评分枚举/uuid 版本/
时间序/唯一 id+版本/非空白 note）、inspect 存活优先 + 快照目录存在性
权威、hasFeedbackTarget（append 源 assistant 消息派生）、版本门控
put/delete + 无变更 no-op + 冲突当前项回传、note-blank/too-large、
target/session-not-found、per-session 串行队列；域 open 在安装期
block_on、关闭同步 block_on（Domain close future 非 Send——记录偏差））；
第 62 轮 dsh-compaction 落地：4 项测试（抽象压缩服务 seam：
CompactionId 品牌 + compact checkpoint 来源构造/谓词（MessageSource::
Plugin 扩展 compactionId/sourceCommandId 合并增强字段，线格式
skip-if-None 兼容）、ManualCompactionError 六码 + CompactionTrigger、
CompactionResult 词表、tool-pairing 平衡折叠（assistant tool-call
计数/结果递减/负余额与坏 seq fail-loud + 缓存按 session id 键控）、
compaction/start-summary-end 括号状态机不变式（重叠/无配对/
checkpoint 关联/回合围栏/seed 边界陈旧 start）核心子集；summary
邻接与影子定价交叉校验留待后续；
第 63 轮 dsh-user-questions + dsh-tool-ask-user 落地：4 项测试
（用户提问能力 seam：AskUserQuestion 词表（问题/选项/多选/意图）、
单一活动 UI provider 注册/重复 fail-loud/处置释放、ask 校验梯
（空问题/无 provider/意图 approve 标签与 detail/中止谓词/代理
liveness + roots 校验）、模型面 `ask_user_question` 工具（questions
投影到 answers、body 内 schema 校验、provider 往返）；
SessionStore.create/fork 与 InvariantRegistry.register 改为显式 caller 参数修复 Proxy 重绑语义）。
第 64 轮 dsh-user-approval 落地：26 项测试（`ctx.approval` 一次性授权
seam：ApprovalOutcome/ApprovalPolicy 闭词表 + ApprovalRequestId 品牌、
approval/asked + approval/decided 审计对（开放回合围栏 + 可选字段省略 +
每请求新 id）、turn-enclosed 前置检查（空转/回合间拒绝且零追加）、
作用域瀑布分派（全局 + agent 作用域监听器、外国作用域永不听见）、
首答单槽 + next() 委托到 fail-closed 默认、同步/异步抛错回答器包含为
unavailable、非词表回答归一化、policy 折叠（最后 approval/policy 事件 +
配置默认 ask 回退）、never 策略在分派前确定性拒绝（注册顺序不可绕过、
回答器不被咨询）、会话覆盖双向压制配置默认、setPolicy 注入下次模型
步的切换通知（幂等、plugin 源）、approval:policy 系统提示上下文注册与
fiber 处置释放、invariant 伴生（asked/decided 按 id 配对 + 开放回合
围栏 + 政策/结果闭词表；增量 trace 规避 append 锁重入）；
策略上下文解析为 TS 无 agent 空分支（AssembleContext 尚未携带 agent——
记录偏差）、回答器失败按监听器包含（不否决追加）；fs-sandbox 的
EscalationApprover 通道自此有真实 ctx.approval 服务可接）。
第 65 轮 dsh-permission-presets 落地：32 项测试（permission-presets.spec +
invariant.spec + projection.spec 全部移植：preset 表词表（sandbox+
approval 绑定 + 可选 name/description）、保留名 custom 拒绝、非约束
执行器组合拒绝、derive 数学（共享绑定平局先取上次选中、陈旧折叠回退
表序、无匹配 → custom）、set 写链（permission/preset + 仅变化的
sandbox/mode + approval/policy 经规范 setter、当前值 no-op、漂移重选
修复单旋钮）、optionOf 标签回退/custom 固定/未知 fail-loud、settings
段默认 preset（defaultPreset union-of-consts schema + validate 钩子 +
setSource 源 thunk + 未知存储值拒绝）、新会话 pin（session/created +
存量 list 双通道、seed 会话保留有效旋钮仅补缺失事实、空 seed 走组合
默认、legacy 缺失 policy 物化 ask）、`permissions` 投影单元（三旋钮
JSON 态折叠 + custom 仅当前追加 + change feed 每旋钮通知 + 无关事件
同引用零通知 + HMR 挂载/卸载键释放）、`/permission` 命令（set 写链 +
setPolicy 活切换注入通知、裸调用报告当前值、未知 preset 错误记录
不动日志）；ApprovalService 补 config() 公开访问器（TS public config）；
settings 布线 + 投影/命令两子 fiber 经 ready() 可结算）。
第 66 轮 dsh-skill 落地：30 项测试（skill.spec.ts 核心全量移植：
provider 注册/处置 + 保留名 runtime 拒绝 + 工厂失败回滚、rank→
providerOrder→localOrder 三键去重 + 跨层最近层遮蔽（无视 rank）、
invocation 政策中立目录 + model/user 谓词独立、runtime 技能默认
provider/invocation + 层内 first-wins 重复告警 + no-op 处置、候选/
定义校验（名称语法/非空描述/provider 归属、类型不变量在 Rust 为
编译期事实——记录偏差）、lookup options 借用（cwd + signal 指针
身份）、目录缓存（cwd+scope 链+revision 键、容量 LRU、不完整观察
不缓存、失败 provider skip+warn 且不可缓存、在飞失效重试上限 2
次留未缓存结果、晚到 invalidation 忽略）、skills/change 通知
（每次注册/处置/失效发射、监听器失败包含 + 告警）、中止竞速
（缓存后发现后加载前重查、不合作 provider 竞速、统一 SKILL_
ABORTED 消息——谓词无 reason 载荷记录偏差）、定义身份漂移失效、
消失候选返回 None、加载失败传播、渲染（目录/URL/opaque/无基回退
四种资源提示 + 属性转义 + 正文逐字 + 转义函数）、作用域层
（scoped provider/runtime 归属层、链继承 + rebind 重挂、层内
provider 名唯一 + 作用域重复文案、处置掉层 + 通知、作用域 control
仅在注册存活期失效）；invariant 伴生 no-op（TS 同款）；workspace
成员列表新增 crates/skill/*）。
第 67 轮 dsh-tool-skill 落地：24 项测试（tool-skill.spec.ts 的 runtime
技能子集全量移植：`skill` 工具 schema/处置/重挂 + presentCall 形状、
首次步进稳定持久目录（按 digest 判重、描述规范化/截断/转义、来源
不泄漏 whenToUse/正文）、空基线/不完整发现跳过并在后续边界重试、
同一步提案目录去重/替换、陈旧提案在空基线前移除、匹配提案保留、
增删触发的完整替换目录与空墓碑、按持久 entries 恢复 + 外来 lookalike
不压制、压缩隐藏后重建目录、未知/非法/模型禁用技能错误、加载前
政策检查 + 加载后重查、provider 资源提示渲染（opaque/url/无基）、
描述上限校验、restrict 屏蔽与作用域同名遮蔽的目录门控（register_arc
精确身份比对）、`/name` 手势注入（首 token/句中、路径分数边界拒绝、
未知/用户禁用保持普通文本、非 user 源不扫描 + 去重、下游 reject
透传、仅文本块）；dsh-llm MessageSource 扩展 SkillCatalog/
SkillInvocation 两个 kind + SkillCatalogEntry（线格式与 TS 一致）；
dsh-tools 新增 register_arc（注册指针身份比对）；invariant 伴生
no-op；pre-step 载荷无 signal（dsh-agent 偏差——查找无中止谓词）；
skill-filesystem 依赖用例（cwd 项目技能/正文刷新）随该包后续落地）。
第 68 轮 dsh-skill-badge + dsh-plan-mode 落地：16 项测试（skill-badge
2 项——内置 `dsh-badge` 技能 provider 注册/列出/加载/处置 + 官方 PNG
字节不变（sha256 + IHDR 尺寸）；plan-mode 14 项——plan-mode.spec +
invariant.spec 核心全量：plan/mode 折叠（最后者胜/无则 inactive/end
界）、配置校验（非空 section）、首标题提取、set 状态机（空闲提交 +
按最后 request/header 叙述注入、开回合排队 + 边界提交、相反选择
cancelled + 边界清除、noop）、exit_plan_mode 评审流（approve →
approved + 静默选择下一边界提交、keep-planning 反馈、ASK_ABORTED
驳回文案、非 plan 模式/无标题/无 userQuestions 渠道错误）、
`/plan` 命令（on 进入 + 非 off 消息 steer、off 退出）、`plan` 投影
单元（command/run 意图 + plan/mode 提交的双事件折叠 → {active,
pending}）、plan/mode 布尔载荷 invariant 伴生；工作区成员新增
crates/plan/*；计划:policy 段 provider 解析为 TS 无 agent 空分支
（偏差）、评审驳回码塌缩为 ASK_ABORTED（user-questions 偏差）。
第 69 轮 dsh-skill-filesystem 落地：7 项测试（skill-filesystem.spec
发现/解析子集：目录捆绑 + 扁平 .md 双形态发现（排序/来源/rank）、
目录资源基与正文加载、.system 目录跳过、YAML frontmatter 解析
（name/description/whenToUse/metadata + 调用政策禁用键与布尔词表 +
legacy 键拒绝 + 缺失必填/非法名/无 frontmatter 的 warn-and-skip）、
git 根项目技能发现（cwd 敏感）、custom/bundled 根 + 稳定 rank、
缺失根空目录；notify 递归监视 + 去抖失效（chokidar 的祖先监视/
轮询模式未移植——缺失根靠下一次发现拾取，偏差）；fs/observed
突变钩未接线（Rust 演员柄无工具名，偏差）；fs 服务存在时经
FsError 码表含缺失/非文本路径、无 fs 时 std 回退；invariant 伴生
no-op；技能生态四包自此完整（seam→provider→tool→badge）。
第 70 轮 dsh-session-query-sqlite 落地：25 项测试（query.spec 全量 +
sqlite.spec 核心子集：config 默认/校验（path/openAt/页上限/片段
上限/并发）、startup/first-search/never 三种开启边界（never 不触
文件系统 + 继承读/追溯可用）、live-only FTS5 unicode61 两字符词
元搜索 + 全头部往返（cwd/seedLength/delegationDepth/agentPreset）、
推理内容排除/可见文本入索引、全 surface 默认搜索 + 元数据先过滤
后排名（seq/time/type/surface 组合）、短语词元 + 稳定并列序（match
_count 降/文档长升/time 降/session_id 升/seq 降）、曲音/标点片段
定位（码点裁剪 + 空白归一）、游标绑定与失效（instance/scope/
fingerprint/generation 四键 + 偏移安全整数、目标会话追加→STALE、
请求指纹不符→INVALID、损坏偏移→INVALID）、持久源动态挂载/活
shadow/卸载揭示（erased 注册 + 指针身份 binding cell）、live 遮蔽
跳过 inspect（计数 0→detach 后 1）、快照变化重试（两次稳定观察
+ 一次重试上限→PERSISTENCE_FAILED）、live/persisted 头部冲突→
SOURCE_CONFLICT、瞬时拓扑变化→STALE、FTS5 外层谓词预算 14 +
固定谓词、32766 便携绑定上限、重开丢 temp.live 保 persisted、
schema 版本/application_id 守护）；rusqlite bundled FTS5 同步句柄
仅同步段持有（tokio 门闩序列化 + parking_lot 无 await 持有——
rusqlite Connection 非 Sync）、可选持久化绑定改轮询 + inject 子
fiber 复位（挂载/卸载 identity 变化→epoch→cursor 失效，mount 竞速
容差）、Rust 持久化 API String 错误→统一 PERSISTENCE_FAILED 包装
（无类型透传，偏差）、查询期 SQL 错误→INDEX_FAILED（偏差）；
真实 sqlite 持久化后端注册具体类型与 erased 查询面注册互斥，
组合集成暂以 erased 假后端覆盖（偏差）；invariant 伴生 no-op。
第 71 轮 dsh-schedule 落地：25 项测试（domain.spec 全量 21 项 + runtime/
tools 核心 4 项：v1 变更解码（create/delete/dispatch + acceptedAt 可选项 +
精确键集合/版本/操作词表 + kind 判别）与冻结语义（Rust 值语义）、全部
畸形持久化数据拒绝表（26 例：空/错版本/未知操作/多余键/空白 id/非法
acceptedAt/记录形状错误/prompt 空白/after 0 与 1.5/every 299、300.5、
字符串、MAX_SAFE（interval 安全整数检查）/非真实日历日期/五位数年份/
null schedule/kind 张冠李戴）、按创建序折叠 + id 复用/未激活 delete/
dispatch 拒绝、fork 后缀边界（seedLength 越界）、可读 id 分配防撞、
after 记录规范（trim/31s 目标/scheduled-overdue 视图/输入词表错误）、
注入防护 framing 逐字节（JSON 转义 id/prompt）、every 首个锚定目标 +
下限/安全整数/最新错过出现次选择 + 不可前置/NaN/区间溢出、无积压
推进 + 单向 dispatch 的 acceptedAt 约束、9999 边界终止 + 多记录批量
framing、严格偏移解析（±时区/毫秒 1-3 位归一）、非法偏移 10 例、
not_future/time_out_of_range 区分（now 与 epoch 双区间检查）、IANA
规范化（UTC/America/New_York/US-Eastern 别名）+ 缩写/偏移/未知拒绝、
本地日历解析（Asia/Shanghai 毫秒、UTC、DST 重叠取第一瞬间、DST 空洞
invalid_rule、9999 本地越界）、确定性 recurrence 性质循环 300 轮
（fast-check 属性同构）、runtime 驱动（one-shot 经 maintenance 边界
followup + dispatch 追加 + 折叠清空、every 批量 acceptedAt + 记录推进、
损坏流 faulted 不派发）、三工具（create 校验词表/选择器互斥/trim、
list 创建序视图、delete 真删 + not-found + 非法 id、跨 agent 内部
错误）；时间库 chrono-tz：正则 look-around 改写为显式 0000 前缀检查、
毫秒补零 `{:0<3}`（左填充 bug 修正）、JS 安全整数 = i64 值 ≤ 2^53-1
的 checked_mul 过滤；时区别名仅内置常用 backward 表（ICU 全别名
未嵌入，偏差）；Agent::run_maintenance 结果擦除 → 共享槽读回（偏差）；
flush 需真实 session/flush 监听器（测试注册 no-op 确认器）；
invariant 伴生按 dispatch 内联约束改用增量折叠 trace（锁内禁读
session.events，偏差）；workspace 成员新增 crates/schedule/*。
第 72 轮 dsh-host M6 组合升级 + 持久化 erased 注册统一：① 两个持久化
后端（jsonl/sqlite）的 install 改为注册 erased
`Arc<dyn SessionPersistenceApi>`（此前注册具体类型，与 session-query/
schedule/corpus 的 erased 查询面互斥——第 70 轮偏差修复；全仓无
get_typed 具体类型消费者，零破坏）；② dsh-host 组合从 5 服务骨架
升级为 10 服务真实启动：invariants/sessions/agents/systemPrompt/
tools/commands/userQuestions/sessionPersistence(JSONL zstd)/
sessionQuery(SQLite FTS5)/schedule(函数插件 apply)；③ 启动报告新增
端到端探针：store 会话 append + flush 经 JSONL coordinator 真实持久化
（flushAcknowledged=true、快照数=2）、live 与 persisted-only 双源日志
分别被 FTS5 命中（各 1 条）；④ 挂载 session/schedule/session-query-
sqlite/llm 四组 invariant 伴生；⑤ `cargo run -p dsh-host` 实测 exit 0
输出完整探针报告；boot 测试因组合内嵌 block_on（同步安装器）改为
multi_thread flavor（current_thread 会死锁，测试头记录偏差）。
第 73 轮 dsh-subagent 契约层落地：6 项测试（descriptor.spec 契约子集：
v2 描述符快照/往返（one-shot/continuable 双模式 + toolFilter allow）、
首个 descriptor 事件权威折叠 + 版本闸门（v1 不可分类 → None）+ 无事件
空日志、13 例畸形当前版本载荷拒绝表（非对象/缺 version/版本字符串/
未知 mode/多余键/provider 非串/label 非串/continuable 缺 label/
agentProvider 非串/toolFilter 非对象/未知键/数组含非串/空对象）、
seed 暂存（Session::create + 单条模型隐藏 descriptor + end-seed、
seq 0、无 surfaceOp）、runId 品牌串透明；类型层全量：SubagentRunId
品牌、run/end 观察载荷、能力旗标、启动请求（signal=中止谓词——
偏差：TS AbortSignal）、结果/运行/提供者 trait（prepareContinuable
默认拒绝实现对应 TS 可选方法能力）；AgentOptions 增 subagent_depth
字段（TS module augmentation）；ToolRestriction 补 serde/PartialEq
（descriptor 持久化需要）；error 码类；depth 单调地板（header 权威 +
runtime 加深）；runtime/continuation/registry/backends/tools 留待
后续轮次（偏差）。
第 74 轮 dsh-subagent 服务核心落地：新增 5 项测试（service.spec 核心
子集：provider 注册/列表/get + 重名 DUPLICATE_PROVIDER + 处置后
NO_PROVIDER、能力旗标先于委派校验（maxDepth/persona 缺失 → 
UNSUPPORTED_CAPABILITY 且 provider 未收到启动）、一次性 start 全链路
（label 透传 + descriptor 快照解析 + 发布 run + 结果）、生命周期
start/end 事件对（父作用域 carrier 过滤派发 + 终局观察者、runId
UUID 配对）、助理输出选择折叠（非空 assistant/message 替换流式
回退、text-delta 累积、空输入 None）+ settle_run 三态（completed 带
文本/killed/refusal → failed 带 detail）；模块：assistant_output
（AssistantOutputFold + finalAssistantOutput）、run_settlement
（settleRun → JobOutcome 映射 + dispose 失败合并 detail）、lifecycle
（LifecycleEdge + emit_lifecycle_edge 逐监听器包含 + observe_run
终局配对）、index（SubagentRuntime 服务：providers 注册表 + start
能力闸门 + prepareContinuable 代理 + register_provider 效应作用域）；
continuation manager/listChildren/listDescendants/投影暂拒
CONTINUATION_UNAVAILABLE/UNSUPPORTED_CAPABILITY（偏差）；
生命周期载波 = 父 agent scope_key（TS scopeTarget(service,parent)
的 Rust 近似，偏差）；dsh-subagent 合计 11 项测试。
第 75 轮 subagent 进程内驱动 + fork 后端落地：① dsh-subagent 新增
child_agent（共享子代理组合：resolveChildDepth 上限/安全整数检查 +
SubagentDepthError、resolveChildAgentOptions 父路由继承 + 请求覆盖 +
深度盖章、childSessionMeta（cwd/parentSession/origin subagent/
delegationDepth/seedLength；agentPreset 未移植 → None，偏差）、
SUBAGENT_DELEGATION_CONTEXT + applyChildComposition（delegation 上下文
order 120 + persona section order 0 + tools.restrict 作用域；preset
composeFrom 未移植，偏差）、capture/appendDelegatedPolicyOverrides
（sandboxPolicy.overrideOf + approval 存在即 never，source:
delegation 双事件））；② in_process 共享驱动（startInProcessRun：
中止预检 → 深度 → UUID 子会话 → 政策捕获 → agents.create → 发布后
组合（TS 在未发布创建窗内执行，偏差）→ drivePublishedRun（中止
移交/一次性 followup + whenIdle/readResult：foldConsumedWork 终局 +
finalAssistantOutput + cancelled 覆盖 aborted；structured 捕获未移植
→ 驱动 structured=None，偏差））；③ 新 crate
`crates/subagent/subagent-fork`（dsh-subagent-fork-in-process：balanced
completedTurnPrefix 切片（最后一个 turn/end 含、在飞回合排除、无完成
回合空）+ ForkInProcessProvider（capabilities 除 outputSchema 全开
——结构化未移植 + inheritsParentContext）+ prepareContinuable 一次性
前缀捕获 + invariant 伴生 no-op）；fork 2 项测试（前缀三形态 +
注册/能力/上下文契约）；dsh-subagent 合计 11 项、subagent 组合计
13 项。
关键移植决策与偏差见 `docs/porting/cordis-rust-notes.md`、`docs/porting/schemastery-rust-notes.md`。

### M2 — 类型、契约与共享工具

- [x] `dsh-scope`（`crates/core/scope`，3 项测试全绿）；
- [x] `dsh-brand`（`crates/util/brand`，1 项测试；`Branded<B>` 名义类型 + PhantomData 标记 + serde 透明）；
- [x] `dsh-timeout`（`crates/util/timeout`，9 项测试全绿）；
- [x] `dsh-atomic-write`（`crates/util/atomic-write`，3 项测试全绿）；
- [x] `dsh-home-paths`（`crates/util/home-paths`，4 项测试全绿）；
- [x] `dsh-launch-environment`（`crates/util/launch-environment`，7 项测试全绿——
      launch-environment.spec.ts 全部移植：三层信任序分层快照（process >
      project-env > user-env）、getFrom 层过滤不动信任序、构造期拷贝冻结、
      Windows 大小写折叠、launchEnvironmentOf 提供/回退进程环境）；
- [x] `dsh-output-retention`（`crates/util/output-retention`，7 项测试全绿）；
- [x] `dsh-invariants`（`crates/util/invariants`，3 项测试全绿；
      InvariantRegistry：enabled/allowlist/blocklist 正则选择、包名保留、
      子 fiber 安装器、失败回收；安装器失败通道为 `Arc<dyn Fn(&str)+Send+Sync>`）；
- [x] `dsh-llm`（`crates/core/llm`，59 项测试全绿——第 19 轮补齐运行时层；
      类型层线格式与 TS 逐字节一致 + LlmRuntime 运行时 + llm-invariant 伴生；
      provider adapters 留待后续里程碑）；
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
      投影/不变式伴生/工具调用调度器/ReactLoopAgent 机器/AgentLoop 服务；
      request-reconstruction/resume 深链路随 backend 里程碑补全）；
- [x] `dsh-session-projection`（`crates/session/session-projection`，17 项测试全绿）；
- [x] `dsh-session-stats`（`crates/session/session-stats`，15 项测试全绿）；
- [x] `dsh-session-telemetry`（`crates/session/session-telemetry`，7 项测试全绿）；
- [x] `dsh-session-title`（`crates/session/session-title`，46 项测试全绿——
      SessionTitleService/确定回退/provider 契约/rename 钉住/服务契约/投影单元/
      不变式伴生/JSONL+SQLite 持久化往返；AbortSignal.any 同步谓词改为
      fused source 扫描）；
- [x] `dsh-session-title-llm`（`crates/session/session-title-llm`，12 项测试全绿——
      共享 LLM 策略：route 解析/JSON 帧输入/超时/装配校验）；
- [x] `dsh-session-title-all-prompts-llm`（`crates/session/session-title-all-prompts-llm`，1 项测试）；
- [x] `dsh-session-title-first-prompt-llm`（`crates/session/session-title-first-prompt-llm`，1 项测试）；
- [x] `dsh-session-projection-cache`（`crates/session/session-projection-cache`，18 项测试全绿——
      turn/end 与 detach 双强制写点、count/interval 节流、fail-soft 自愈、
      cachedSnapshot 零 I/O 列表读、coldSnapshot 阶梯读（缓存行 + readFrom 尾读 +
      registry restore + 写回；ver 失配/日志收缩/生命周期不符降级为全量重读）；
      storage-domain 数据表单先行落地，见 M4）；
- [x] `dsh-llm-retry`（`crates/llm/llm-retry`，7 项测试全绿；dsh-llm retry-policy 类型层 +3 项）；
- [x] `dsh-token-meter`（`crates/llm/token-meter`，10 项测试全绿）；
- [ ] dsh-tools code-mode（run_code transport/ts-types/py-types，依赖 code-runtime）；
- [ ] DeepSeek/OpenAI/pi-ai 适配器；
- [ ] context、compaction、interaction、attachment、feedback、hooks、guards。

### M4 — 执行/文件/沙箱

- [x] `dsh-storage`（`crates/storage/storage`，6 项测试全绿——storage Hub：BackendRegistry
      （重复名拒绝/过期 disposer 守卫）、form 挂载/解析、storageBackendServiceKey、
      StorageError 码表、KvFacet/KvUnit 后端契约的单一家）；
- [x] `dsh-storage-domain`（`crates/storage/storage-domain`，6 项测试全绿——域声明/写链/
      domain-changed 事件/版本戳记/关闭排空；facility 经 Hub 路由后端并挂载 `domain`
      form；zod 记录 schema 塌缩为 JSON 校验闭包）；
- [x] `dsh-storage-test-support`（`crates/storage/storage-test-support`，内存 KV 后端测试双）；
- [x] `dsh-storage-json`（`crates/storage/storage-json`，17 项测试全绿——每单元一个
      人类可读文件、临时文件+fsync+rename 原子整文件发布（spawn_blocking 上跑
      Node 线程池等价物）、失败回滚、延迟物化、关闭排空/阻塞在飞 open、
      Hub 注册与生命周期服务键、invariant 伴生）；
- [x] `dsh-storage-sqlite`（`crates/storage/storage-sqlite`，19 项测试全绿——单库承载
      全部路由单元、`u_<unit>_<table>` STRICT 记录表 + units/unit_globals 元数据、
      user_version 物理版戳（失败留 0 可修复重开）、journal_mode 白名单、
      prepare_cached 复用语句、未解析 JSON → malformed-medium、pending open 关闭排空）；
- [x] `dsh-workspace`（`crates/workspace/workspace`，49 项测试全绿——
      workspace.spec.ts（43 项）+ invariant.spec.ts（6 项）全部移植：
      仅头引导与稳定排序、create/delete 的 pending-marker 双写恢复与注入故障回滚、
      registry 级 insertBefore、会话 attach/move/detach 写链判定、canonical-cwd
      头校验投影、archive/unarchive/deleteArchivedSession、cache/table invariant
      伴生（domain/changed 监听）；`sessionPersistence.delete` 尚未在 Rust
      persistence 侧落地，deleteArchivedSession 经 caller 提供的闭包 seam）；
- [x] `dsh-spill`（`crates/spill/spill`，0 项测试——Service Definition：`ctx.spillStore`
      抽象 seam（`saveText` + SpillLocator 品牌 + owner/source/ref 词表），no-op
      invariant 伴生）；
- [x] `dsh-spill-local`（`crates/spill/spill-local`，11 项测试全绿——
      spill-local.spec.ts 全部移植：UTF-16 code-unit 注入式 encodeSegment、
      sha256 会话目录、0700 私有根、0600 独占创建（`create_new` = `'wx'`）、
      随机前缀防 symlink 种植、遍历形 suggestedName 中和、相对 root 解析、
      存储故障拒绝）；
- [x] `dsh-spill-policy`（`crates/spill/spill-policy`，16 项测试全绿——
      spill-policy.spec.ts 的模型侧 arm 全部移植：tools/post-execute prepend
      waterfall、read/嵌套/值替换/非文本直通、notice 预算预留、超 cap 保留内联、
      best-effort 三降级、下游组合与 disposer 卸载；durable
      `tools/code-dispatch-log` arm 待 dsh-code-runtime 里程碑）；
- [x] `dsh-credentials`（`crates/credentials/credentials`，9 项测试全绿——
      credentials.spec.ts + invariant.spec.ts 全部移植：POSIX 标识符 ref 校验、
      空值即缺席的 seam 规则、memory provider 端到端、notifyUpdated 包含分发
      （每个监听器都跑、普通失败告警、INVARIANT 失败聚合后上抛）、
      commit-event 生命周期不变式伴生（无 live 服务不得 emit））；
- [x] `dsh-credentials-local`（`crates/credentials/credentials-local`，49 项测试全绿——
      local/drain/watcher/review-fixes 四个 spec 全部移植：`.credentials.yaml` 严格
      校验（非映射根/序列根/非法 ref/非字符串/空值/重复键/畸形 YAML，错误不泄值）、
      行级注释保留编辑（set 只改目标行、unset 连同上注删除、空文档 `{}`、兄弟块标量
      原样、结构形值引号转义）、写锁读改写折叠外部编辑、0600/0700、包含的
      credentials/updated 扇出、dispose drain（在飞写落盘、排队写拒绝）、fake watcher
      管道与真实 notify 热重载、自写抑制、缺失/损坏文档的 warn-and-keep；
      无 runtime 线程上的 watcher 事件经 channel + runtime 任务入队）；
- [x] `dsh-fs`（`crates/fs/fs`，12 项测试全绿——
      service.spec + invariant.spec 全部移植：FsTargetKey/FsVersion 不透明品牌、
      FsObservation/FsInfo/FsPathInfo/FsDirEntry/写意图/编辑请求结果词表、
      FsError 13 码表 + cause 链、FileSystem 抽象服务（resolve/processPath/
      fileUrl/contains/stat/lstat/readText/streamText/readBytes/listDir/writeText/
      editText + sandboxMode 默认）、internal/dispatch 预钩校验三个事件数据
      （空 targetKey/displayPath/version 拒绝）；`streamText` 为 BoxStream、
      AbortSignal 塌缩为取消谓词）；
- [x] `dsh-fs-local`（`crates/fs/fs-local`，20 项测试全绿——
      fsio.spec + filesystem.spec 核心子集移植：resolveLocalTarget 的 realpath
      身份 + 缺失文件的最近祖先回退（symlink 别名同 key、ENOTDIR 结构化）、
      probe/probeNoFollow、listDirectory 稳定序无内容读、readWholeText（NUL 样本
      + 严格 UTF-8）、readWholeBytes 字节上限（stat 短路 + 增长检测）、
      streamWholeText 跨块增量 UTF-8、readForEdit/readTextForDiff 行尾归一与
      有界 diff basis、applyLiteralEdit 字面替换、writeFileAtomic 私有 staging
      （0700/0600、独占 create、sync、原子发布、hard-link 守卫创建、清理
      失败不翻转已提交写）、LocalFileSystem per-targetKey 锁（并发守卫写
      一胜一 stale、写/编辑确定性）；win32 DACL 拷贝/安全替换为简化边界
      （真实实现随 sandbox-windows-acl 里程碑），版本 token 在 Windows 为
      近似组成）；
- [x] `dsh-fs-observation-policy`（`crates/fs/fs-observation-policy`，18 项测试全绿——
      policy.spec 全部移植：观察态 gate（owner key → targetKey → 观察记录）、
      write-intent（未观察/无 owner → createIfAbsent，已观察 → 观察版本 CAS）、
      edit-intent（未读 FS_NOT_OBSERVED、缺席 FS_NOT_FOUND、观察版本守卫）、
      present→absent→present 转移、多 owner 隔离、单槽 first-wins（不调 next）
      短路、dispose 释放记录并移除监听器；owner 用最小 handle 的 opaque key
      （TS WeakMap 对象身份的 Rust 形）；edit 拒绝经 waterfall 的 panic 通道
      携带结构化 FsError）；
- [x] `dsh-fs-sandbox`（`crates/fs/fs-sandbox`，11 项测试全绿——
      containment.spec 全部 + fs-sandbox.spec 核心子集移植：isPathUnder 词法
      快路径 + 文件系统身份回退（Unix dev:ino；Windows canonicalize 等价）、
      SandboxedFileSystem 每调用策略栅栏（read-only 拒绝 FS_SANDBOX_DENIED、
      workspace-write 现时重解析 + writableRoots 包含、danger 直通）、`..`/
      symlink 逃逸拒绝、TOCTOU 方向（stale targetKey 不写）、审批模式升级覆盖；
      继承 dsh-fs-local 全部存储机制（build 不注册变体））；
- [x] `dsh-sandbox-policy`（`crates/sandbox/sandbox-policy`，10 项测试全绿——
      policy.spec 服务子集 + session-mode 套件全部移植：defaultMode 回退/
      workspaceRoot 绝对化、resolve 的 会话 cwd+override 组合与审批模式最高
      优先、sandbox/mode 事件套件（fold 最后切换、append 恰一条）、
      sandbox/mode 事件不变式（unknown mode 拒绝）；`systemPrompt.context`
      的 sandbox:policy 请求上下文注入留待 agent 字段进入 assemble context）；
- [x] `dsh-sandbox`（`crates/sandbox/sandbox`，15 项测试全绿——
      vocabulary/escalation/roots 三个 spec 全部移植：SandboxMode/Policy/
      ConfinedArgv/denial 方言/RunnerFailureRule 词表、SandboxUnavailableError
      （SANDBOX_UNAVAILABLE 结构化码）、严格加宽梯 WIDER_MODES +
      闭集 ESCALATION_TARGETS、validateEscalationArgs 参数配对、
      模型面 denial/hint marker、approveEscalation 有序 fail-closed 序列
      （非加宽不提问、无审批服务/无 agent 各自文案、四态 outcome 映射）、
      canonicalPath（解析失败保原拼写）+ writableRoots 规范化去重派生）；
- [x] `dsh-subprocess`（`crates/subprocess/subprocess`，4 项测试全绿——
      service.spec 全部移植：完整 spawn 词表（三态 stdin ignore/pipe/data、
      输出模式 pipe/inherit/有界收集+spill、offset 无消费读取器、树级终止
      SIGTERM→grace→SIGKILL 唯一终止动词、显式 env 墓碑）、terminal 原语
      （六法：output 字节流/done/write/inspectForeground/signalForeground/
      terminate 全会话静默）、scrubbedParentEnv 双擦洗（凭据形 KEY/
      PASSWORD/SECRET/TOKEN + DSH_ 前缀均大小写不敏感）、解析可执行
      （绝对验证/裸名 PATH/含分隔符相对路径拒绝）；AbortSignal 塌缩为
      取消谓词、Node 流塌缩为 tokio 字节流）；
- [x] `dsh-subprocess-local`（`crates/subprocess/subprocess-local`，14 项测试
      全绿——spawn.spec + local.spec 核心子集移植：childEnv 擦洗合并（墓碑
      移除/显式凭据幸存/Windows 大小写不敏感）、OutputCollector 字节精确
      尾窗与惰性 spill（溢出不丢先头、超 cap 丢文件保尾窗）、detached 进程
      树 spawn（POSIX 进程组/Windows taskkill /T /F 树终止）、SIGTERM→
      grace→SIGKILL 升级（TERM 陷阱幸存者仍被杀）、abort 谓词 15ms 轮询
      反应（TS 事件目标塌缩）、批次 stdin 上写即关、可执行解析（绝对验证/
      裸名 PATH+PATHEXT/相对路径拒绝/稳定错误文案）、服务释放与 fiber
      处置终止整树；`spawnTerminal`（node-pty）桩留待 PTY 里程碑；done
      结算自 spawn 起即被驱动（TS 事件驱动 vs Rust future 惰性））；
- [x] `dsh-terminal`（`crates/terminal/terminal`，23 项测试全绿——
      service.spec 全部 23 例移植：backend 注册精确贡献释放（ptr 身份清理）、
      owner 精确栅栏（FOREIGN_SESSION/NO_SESSION/OWNER_NOT_LIVE）、spawn
      发布/回滚（未发布 close 回滚 + 双失败聚合）、调用者取消（Aborted
      塌缩）、owner/服务处置对未发布 setup 的 abort+await（sync 前缀语义）、
      后端侧清理失败保留至处置聚合、close 幂等 fence 合并与代数守卫、
      处置 best-effort 清注册表 + 跑 owner cleanup；invariant 伴生 no-op；
      spawnTerminal 的 PTY 后端（terminal-bash）留待后续）；
- [x] `dsh-shell`（`crates/shell/shell`，7 项测试全绿——
      render.spec 全部 + service.spec 全部移植：`[exit code: N]`/
      `[killed by signal: X]` 标记解析逆契约（前置换行+结尾锚定）、
      task-free 抽象执行器（resolve/run/start 三原语 + sandboxMode 默认
      无沙箱 + 重复注册 fail-loud）；DshEnvironment/DshEnvironmentKey
      补入 dsh-subprocess 词表）；
- [x] `dsh-bash-local`（`crates/shell/bash-local`，21 项测试全绿——
      executor.spec + settings.spec 移植：ENV_OVERRIDES 终端环境、
      Config→ResolvedConfig 默认与 assertServiceable 校验（正值 + graceMs
      定时器上界）、clampTimeout 上限、deadline 融合信号（timeout/abort
      首因互斥分类）、stdin/env/dshEnv 三明治合并（覆盖 > 调用者 > 终端）、
      stdout 独立预算、后台 ShellProcess（消费式增量读、[stderr] 段合并、
      损失标记 + 双 spill 路径、kill 幂等 + grace 升级、spec.signal abort
      结算 killed、spawn 失败 killed + note 一次投递）、settings 段
      user 层解析/写入校验/存储段服务/供应商脱落回退/无供应商入口/
      命名空间释放；onProcessDone 钩子注入化（Rust 无子类化）；
      Windows+WSL 下 POSIX 路径/env 转发/引号语义不可靠的用例 cfg(unix)
      门控，引用免费子集全平台运行）；
- [x] `dsh-code-runtime`（`crates/code-runtime/code-runtime`，9 项测试全绿——
      reserved.spec + service.spec 全部移植：可移植标识符排除集四个共享
      契约（绑定全局保留槽 console/__dsh_main__/__builtins__/__name__/
      __debug__、错误成员保留集、dunder 形式正则（空中间不匹配）、
      ECMAScript∪Python 保留字并集——一仓契约保证跨后端可移植）、抽象
      CodeRuntime（language/isolation/run 三原语，失败为结果字段永不
      rejection、AbortSignal→谓词塌缩、重复注册 fail-loud、fiber 卸载
      移除服务）；invariant 伴生 no-op；`code-runtime-worker-thread`
      （TypeScript bootstrap/worker JSON 协议，需嵌入 JS 运行时）留待
      后续里程碑；
- [x] `dsh-jobs`（`crates/jobs/jobs`，5 项测试全绿——
      service.spec + invariant.spec 全部移植：抽象 JobRegistry 九法
      （start/list/get/read/kill/wait/onJobDone/onJobsChanged/
      attachController）、JobId 品牌（`<kind>-N`）、JobHooks 三法
      （cancel/done/readOutput 消费游标）、JobOutcome 终态三分类、
      快照跨字段不变式（id 前缀+正序数、标签非空、startedAt 非负、
      finishedAt 恰在终态、ownerSession 与完成 owner 一致）+
      jobs-inject 安装器（校验现有 unowned 记录 + 订阅终态快照）；
      抽象 seam 挂载栅栏（TS new.target 检查）在 Rust 为编译期事实；
      invariant 伴生含真实安装器）；
- [x] `dsh-jobs-local`（`crates/jobs/jobs-local`，14 项测试全绿——
      jobs.spec 核心子集移植：入站预检链（控制器服务/空 kind/空 label/
      非法 outputLimitBytes/owner 必须当前注册实例）、按 kind 顺序 id
      计数器、每 exact-owner 并发上限、session-id 授权栅栏（异主
      get/read/kill 拒绝、无主任务开放）、流式 readOutput 消费游标与
      终态 output 幂等读、reported 标记、kill 两态（取消先于状态转移）、
      首胜结算（晚到 outcome 忽略）+ settled 广播 + 监听通知
      （onJobDone/onJobsChanged 包含式投递）、有界 wait（结算/超时
      返回快照/中止拒绝、waiter 计数）、teardown 取消与抛错强制失败
      （possible orphan 报告）、owner 处置取消并删除记录、服务处置清空
      与跨 fiber effect 分离；ScopedLayers 全局+scope 链分层控制器/监听；
      `dsh-tool-jobs` 工具层留待后续）；
- [x] `dsh-goal`（`crates/goal/goal`，12 项测试全绿——
      goal.spec 核心子集 + 严格折叠套件移植：事件溯源 goal/change
      （全量快照 + clear 墓碑，Session.append 持久化）、CAS ref 比对、
      七动词 + 阶段梯（create 仅可替换 completed；pause/resume/
      complete/block 的 allowed 集合；resume 的 armed 拒绝与预算耗尽）、
      边界校验（objective 规范化/maxGoalRounds 正整数/blockReason
      lower-kebab）、进程本地 activation（pending-activation 跨 append
      边界 + session-start disarm 边）、round 准入折叠（user/message 的
      goal 源：仅活动目标的下一个轮次、上限）、goal/changed scoped
      emit、strict 解码器 fail-loud（版本/字段集/规范化/跳过 revision/
      定义漂移/时间戳回退）；缓存键用 agent id（session 事件快照指针
      跨 append 不稳定）；@Remote 注解与 goal 投影单元注册留待
      typert/session-projection 集成；
- [x] `dsh-session-checkpoint-policy`（`crates/session/session-checkpoint-policy`，
      4 项测试全绿——语义持久化检查点核心：llm/stream 工厂包装
      （flush 先于首块、失败 fail-closed 终态 finish 块且阻止适配器分派）、
      tools/execute 顶层 owned 调用检查点 + 预分派 abort 规范结果
      （ABORTED_BEFORE_DISPATCH）、agent/pre-step 边界 flush；NextFn
      单次延续语义、llm/stream cell 的双 Arc downcast、flat_map 分流
      （失败不链下游）；
- [x] `dsh-native-command`（`crates/util/native-command`，4 项测试全绿——
      无 shell 执行器：utf8 stdio 捕获、非零退出 code+stdio 附加、
      ENOENT、abort 谓词传播终止、Windows CREATE_NO_WINDOW hide）；
- [ ] tool-jobs（工具控制器）；
- [ ] subprocess E2B、PTY、process tree（PTY 需 portable-pty/ConPTY；
      E2B 需外部沙箱 SDK）；
- [ ] code-runtime-worker-thread（TypeScript bootstrap，需嵌入 JS/TS
      运行时——boa/deno_core 级依赖，独立里程碑）；
- [ ] sandbox-local（bwrap/Landlock/Seatbelt 方言——Linux/macOS-only）；
- [ ] Windows ACL restricted-token backend；
- [ ] credentials-encrypted backend（DPAPI/keychain）。

### M5 — 产品功能

- [x] goal（`crates/goal/goal`，见上）；
- [x] time-context（`crates/context/time-context`，20 项测试全绿——
      `@deepseek-ai/dsh-time-context`：可选出每步持久化时钟上下文；见上）；
- [x] repeat-tool-reminder（`crates/guard/repeat-tool-reminder`，13 项测试
      全绿——`@deepseek-ai/dsh-repeat-tool-reminder`：建议型重复调用
      检测器；见上）；
- [x] timeout-policy（`crates/guard/timeout-policy`，9 项测试全绿——
      `@deepseek-ai/dsh-tool-call-timeout-policy`：协作式工具超时执行器；
      见上）；
- [ ] goal-round-driver/tool-goal/command-goal；
- [x] tool-todo（`crates/todo/tool-todo`，12 项测试全绿——
      `@deepseek-ai/dsh-tool-todo`：整表替换 todo 工具 + `todos` 投影
      单元；见上）；
- [x] user-approval（`crates/interaction/user-approval`，26 项测试全绿——
      `@deepseek-ai/dsh-user-approval`：一次性授权 seam + 审计对 +
      会话级 ask/never 政策；见上）；
- [x] permission-presets（`crates/interaction/permission-presets`，32 项
      测试全绿——`@deepseek-ai/dsh-permission-presets`：sandbox/approval
      双旋钮预置 + `/permission` 命令 + `permissions` 投影；见上）；
- [x] skill（`crates/skill/skill`，30 项测试全绿——
      `@deepseek-ai/dsh-skill`：分层技能提供者注册表 + 目录缓存 +
      渲染；见上）；
- [x] tool-skill（`crates/skill/tool-skill`，24 项测试全绿——
      `@deepseek-ai/dsh-tool-skill`：`skill` 加载器工具 + 持久会话
      目录 + `/name` 手势注入；见上）；
- [x] skill-badge（`crates/skill/skill-badge`，2 项测试全绿——
      内置 `dsh-badge` 技能 + 官方资产；见上）；
- [x] plan-mode（`crates/plan/plan-mode`，14 项测试全绿——
      `@deepseek-ai/dsh-plan-mode`：logged 协作状态 + plan:policy 段 +
      `/plan` 命令 + exit_plan_mode 评审工具 + `plan` 投影；见上）；
- [x] skill-filesystem（`crates/skill/skill-filesystem`，7 项测试全绿——
      `@deepseek-ai/dsh-skill-filesystem`：本地项目/用户/custom/bundled
      根技能发现 + frontmatter 解析 + notify 监视；见上）；
- [x] session-query-sqlite（`crates/session/session-query-sqlite`，25 项
      测试全绿——`@deepseek-ai/dsh-session-query-sqlite`：SQLite FTS5
      派生物化索引 + 全量搜索/游标/对账后端；见上）；
- [x] schedule（`crates/schedule/schedule`，25 项测试全绿——
      `@deepseek-ai/dsh-schedule`：会话内一次性/固定频率提醒 + 三个
      管理工具 + 逐 agent 定时运行时 + 注入防护 framing；见上）；
- [x] subagent 契约层（`crates/subagent/subagent`，6 项测试全绿——
      `@deepseek-ai/dsh-subagent`：run/result/能力类型 + v2 持久化
      描述符 + 深度记账 + 提供者 trait；runtime/registry/backends/
      tools 留待后续；见上）；
- [x] subagent 服务核心 + fork 后端（第 74–75 轮：注册表 + 一次性
      start 生命周期 + 共享进程内驱动 + `dsh-subagent-fork-in-process`；
      continuation/listing/投影暂拒；见上）；
- [ ] plan/todo 剩余（plan 系列其余包）；
- [ ] subagent registry/backends/tools；
- [ ] workflow engine/worker/tool/ralph；
- [ ] MCP/LSP/ACP；
- [ ] dynamic Cordis extensions、host/client runner、inspect/define/run 工具。

### M6 — Host 外壳与 CLI

- [x] 可启动主程序骨架（`crates/host/dsh-host`——核心服务组合 +
      启动报告，`cargo run -p dsh-host` 实际运行 exit 0；见上）；
- [x] dsh-host M6 组合升级（第 72 轮：10 服务真实启动 + JSONL
      持久化 + SQLite FTS5 搜索 + schedule + 端到端 durability/search
      探针；见上）；
- [ ] webserver 路由服务；
- [ ] frontend-static + SPA fallback/index injection；
- [ ] directory-picker browse/native/auto；
- [ ] plugin-inventory；
- [ ] apiproxy 52 RPC + SSE/download/respond；
- [ ] Rust CLI、profile bundles、composition/HMR/信号退出；
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
   `start_returns_immediately_with_a_running_handle_that_settles_as_completed`
   与 `dsh-credentials-local` 的
   `keeps_both_refs_when_two_providers_write_the_same_document_concurrently`
   在全量并行负载下偶发时序抖动（单独运行必过），与移植无关；第 70 轮
   机器重载时曾连续 3 次全量命中（150ms 启动时限断言）、隔离运行 14/14
   绿、第 4 次全量即通过——重试直到绿即可，后续加宽容差或串行化。
1. **动态 JS 插件与 workflow 脚本**：TS 版依赖 `node:vm`；Rust 需要嵌入式 JS runtime
   （优先评估 `deno_core`，备选 Boa）并保持授权、隔离、模块解析和私有 RPC。
2. **Typert/TS 类型生成**：TS conditional/keyof/template literal 类型无 Rust 直接对应，
   需 trait/associated types + build-time codegen。
3. **Cordis Proxy 调用重绑**：TS 会将 service method 的 `this.ctx` 重绑到调用方；
   Rust 当前采用"caller Context 显式参数 + Context 固有 mixin 方法"，
   最终必须通过 conformance 测试证明行为等价。
4. **事件同步边界**：TS `emit/bail/on` 有同步语义，Rust listener 是 Future；
   当前实现保持顺序/结果语义，但存在 async boundary，需要进一步做同步/异步 listener 适配。
5. **Windows 安全**：受限令牌、能力 SID、DACL、COM picker、Job Object/PTY 均需真实 Windows 集成测试。
6. **存储兼容**：JSONL/zstd/SQLite 必须通过现有 fixture，不能只做"相似格式"。

## 6. 下一步（第 73 轮收尾状态）

已落地：workspace 成员 60+ 个 crate（vendor/cordis 生态 7 包 + core +
util + session 12 + settings + llm + skill 4 + plan + schedule +
subagent 契约层 + storage/workspace/spill/credentials/sandbox/fs/
subprocess/terminal/shell/code-runtime/jobs/goal/context/guard/identity/
todo/attachment/interaction/feedback/compaction 各分组核心 +
dsh-host 可启动组合），`cargo test --workspace` 全绿（总数见 §1），
`cargo run -p dsh-host` 实际运行并输出 10 服务 + 端到端
durability/search 探针。

未完成（按剩余工作量排序）：

1. subagent 运行时层（registry/backends/in-process/spawn/acp + tool-
   subagent 三件套 + list-children/continuation/投影——TS 约 4400 行
   源码 + 4700 行测试）；
2. workflow（engine/worker/tool/ralph——依赖嵌入式 JS 运行时，独立
   里程碑）与 MCP/LSP/ACP 系列；
3. M4 收尾：sandbox-local（Linux/macOS-only）、Windows ACL restricted
   token、credentials-encrypted（DPAPI/keychain）、subprocess PTY、
   E2B、code-runtime-worker-thread（嵌入 JS/TS 运行时）、tool-jobs；
4. M6 外壳剩余：webserver 路由、frontend-static、directory-picker、
   plugin-inventory、apiproxy 52 RPC + SSE/download/respond、Rust CLI
   与 profile bundles；
5. M7 全量一致性：conformance fixture、golden wire/storage/session
   数据、CI 矩阵、Rust Host 默认入口与 1:1 完成声明。
