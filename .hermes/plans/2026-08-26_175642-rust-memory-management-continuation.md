# Rust Harness 内存管理续阶段实施计划

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task. 每个阶段最后必须重新构建 Release、替换唯一正式 `target/release/dsh.exe`、使用原正式配置根启动 58080，并废弃此前所有内存证据。

**Goal:** 在已经完成长历史有界读取与 cold 列表投影的基础上，继续收口刷新重复分配、cold 子代理/附件全量读取、Git/PTY/Preview 生命周期、瞬时大请求和 allocator 高水位，并用真实生产入口证明稳定常驻值不随操作次数线性增长。

**Architecture:** 保留 Rust 原生持久层、固定大小投影、消息边界分页和 owner-scoped 生命周期。先消除活对象与重复物化，再缩小各子系统预算，最后才做 allocator 差分；Working Set、Private Bytes、线程、handles、子进程和浏览器 heap 分开记账。

**Tech Stack:** Rust 1.97、Tokio、JSONL/Zstd、SQLite、SessionProjectionCache、Windows ConPTY、Chrome/CDP、WMIC/Win32 process metrics、正式 `%LOCALAPPDATA%\DeepSeek Harness` 数据根。

---

## 1. 计划边界

### 本计划执行范围

- 正式 Rust 入口：`D:\deepwork\deepseek-harness-rs\target\release\dsh.exe web --port 58080`。
- 正式配置根保持 `%LOCALAPPDATA%\DeepSeek Harness`，不切换隔离 `DSH_HOME` 代替最终验收。
- Node 参考仓库只读。
- 不修改 provider、模型、凭据、remote、账号或默认配置。
- 不用删除正式会话、隐藏历史、禁用 Think/工具、缩短语义来降低内存。
- 不把瞬时 Working Set 回落、重启后的冷值、测试数或代码行数作为完成证据。

### 与旧计划的关系

旧计划 `.hermes/plans/2026-08-25_143500-rust-memory-closure.md` 保留不改。其阶段1–4的核心目标已有实现：

- `session.history` 固定 `4096` 事件预算；
- JSONL/SQLite 范围读取和 `visit_event_chunks`；
- prepared-session LRU 默认 `5`；
- cold `session.list` 使用固定大小投影/metadata；
- 浏览器和 context jump 使用 `afterSeq + maxMessages`；
- 正式长会话20次有界读取未显示线性增长。

旧计划中的以下基线已过时，不得复用：14.72MB 空 Host、68k 事件页、单次 history 增长185MB。当前计划以本文件的最新正式证据为准。

---

## 2. 2026-08-26 正式排查基线

### 正式进程与数据规模

- 入口：`http://127.0.0.1:58080/`。
- 当前正式会话：34个，其中 blank 5个；sessions 数据约2.7MB，storages约172KB。
- 浏览器刷新自动发出19个RPC，其中：
  - `settings.describe` 7次；
  - `host.describe`、`session.list`、`subagent.list`、`workspace.list`、`session.history`、`commands/list`、`skill.list`、`agentPreset.list`、`session.attachment`、`session.models`各1次；
  - `credentials.describe` 2次。
- 刷新后浏览器 JS heap约26.3MB，DOM约900节点。

### Host内存实测（同一正式数据根）

| 阶段 | Working Set | Private Bytes | 线程/handles | 结论 |
|---|---:|---:|---:|---|
| 重启初期、尚未完整浏览器连接 | 32.2MB | 74.8MB | 17/190 | 仅冷启动参考，不能作稳定目标 |
| 浏览器连接和会话读取后 | 94.8MB | 157.9MB | 18/198附近 | 当前真实交互基线 |
| 20次 `session.list` 后 | 97.8MB | 148.3MB | 24/213 | 一次性lazy-init/allocator增长 |
| 再20次有界长历史后 | 97.6MB | 147.9MB | 24/213 | 基本稳定，无线性增长信号 |
| 静置后 | 100.8MB | 150.7MB | 24/204 | allocator/运行时高水位 |
| 再100次 `session.list` 后 | 98.0MB | 148.6MB | 25/205 | 无线性增长 |
| 再100次有界history后 | 97.7MB | 147.9MB | 25/205 | 无线性增长 |
| 100次Git status后静置 | 110.3MB | 164.0MB | 26/214 | Git/native command专项风险 |

### 已确认的有界/清理设计

- `crates/host/apiproxy/src/proxy.rs`：history最多4096事件。
- `crates/session/session-persistence-jsonl/src/index.rs`：Zstd frame/事件chunk分块读取。
- `crates/session/session-persistence/src/preparations.rs`：ready LRU；正式配置容量5。
- `crates/session/session-projection-cache/src/index.rs:186-196`：session detach时flush、abort timer并移除dirty row。
- `crates/core/agent/src/registry.rs:452-498`：exact disposer从Agent HashMap移除。
- `crates/terminal/terminal/src/index.rs:675-679`：PTY关闭成功后移除session record。
- `crates/terminal/terminal-bash/src/lib.rs:75-77`：单PTY当前上限10,000行/4MB，read 256KB。
- `crates/host/dsh-host/src/web_preview.rs:776-782`：project日志最多400行。
- better-sidebar：最多16个会话UI状态、8个文件标签、4个网页标签；关闭工作台后iframe、网页标签、历史和DOM均卸载。

### 明确风险

1. `subagent.history` cold路径在 `crates/host/apiproxy/src/proxy.rs:4594-4618` 使用 `persistence.inspect`，先物化完整child事件，再分页。
2. `session.attachment` cold路径在 `proxy.rs:4157-4195` 全量inspect并扫描事件引用。
3. `session.fork`等显式操作仍可能全量inspect；虽然不是自动刷新，但超长会话会造成操作峰值。
4. 至少6个前端插件独立调用 `settings.describe`；正式刷新实测7次。Host `proxy.rs:1640-1695` 每次重新clone/schema→JSON完整快照。
5. 100次Git status后WS、线程、handles上升并静置不完全回落；`dsh_native_command`使用`tokio::process::Command + wait_with_output`，尚未证明第二批斜率或Windows句柄归零。
6. 单Agent最多3个PTY，理论scrollback活对象上限12MB；多live Agent时按Agent乘法增长。
7. Preview `projects` Map在项目settled后只标记状态，未在完成/停止后删除；可能长期保留handle、runtime与日志。
8. Preview challenge只在下一次prepare时清理过期项；长期不再prepare时会保留到Host退出。
9. Host API请求体上限300MB，附件单消息聚合100MB，preview upload/media各64MB，存在可预见瞬时峰值。
10. 仓库没有正式Host内存差分脚本；现有压力测试主要是 `web/stress-tests/reasoning-chunks.stress.ts`，不覆盖Host生命周期。
11. `PersistenceCoordinator.chains` 在 `crates/session/session-persistence/src/coordinator.rs:1257-1273` 对每个历史session ID创建串行化Mutex，未发现delete/retire后的remove；是明确的进程期单调增长表。
12. `SubagentContinuationManager::ChildLock.tails` 在 `crates/subagent/subagent/src/continuation.rs:138-158` 对每个历史child ID创建Mutex，未发现淘汰；与persistence chains同类。
13. `SessionWriteBehind` 在 `crates/session/session-persistence/src/write_behind.rs:95-117,255-289` 的pending事件、失败批次重排和unbounded deadline channel没有事件/字节硬上限；后端持续变慢或失败时可能无界增长。
14. live `SessionStore`无会话数量或单Session事件硬上限，持久化不会裁剪live `events`；长期live会话是完整事件数组的基数乘数。
15. `SessionPreparations`容量5只约束Ready phase；Loading、Reserved、Committing和悬挂loader不计入LRU容量。
16. `LocalJobRegistry`默认10只限制每owner同时活动job；已完成job及output/detail/hooks不会自动淘汰，长期owner可顺序积累。
17. `TerminalSessionService`本身没有全局PTY admission cap；Web Preview的3个限制不是服务级边界。`disposed`是按owner指针的普通HashSet，未发现删除路径，存在进程期增长和地址复用风险。
18. Subagent continuation没有同层活动child总量上限；长期continuable child会保留Agent/Session/owner树。
19. Cordis PluginRuntime和外部Web bundle均为单项有生命周期、总数无全局上限；当前外部bundle单项最大2MiB并常驻，正式Host不执行host-side Node plugin。
20. 当前正式二进制已经启用mimalloc purge/decommit相关策略；缺口是同工作负载A/B和稳定采样，不是“尚未启用allocator优化”。
21. 现有Rust长历史测试规模约2002事件；缺少约68k事件同形fixture、正式58080重启/删除压力和PTY反复创建关闭测试。
22. `target-memory-feature`只是脏工作树的独立Cargo target产物，不是内存报告；README中的`dsh-host --test boot`和`dsh-host-cli --test web`目标在当前树中已不存在，属于陈旧门禁。

---

## 3. 统一测量合同

每个阶段必须使用同一采样程序和正式PID：

```text
真实 dsh.exe 监听PID（不是bash wrapper）
→ WorkingSetSize
→ PrivatePageCount
→ ThreadCount
→ HandleCount
→ dsh/git/pwsh子进程数量
→ 浏览器 used/total JS heap、DOM节点、iframe数量
```

### 固定采样点

1. Host启动后、浏览器未连接；
2. HTTP warm；
3. 正式浏览器刷新；
4. session list；
5. 长history 1次/20次/100次；
6. Git status 1次/20次/100次；
7. PTY 1个/3个填满scrollback；
8. 关闭PTY、关闭工作台、结束/删除会话；
9. 静置10秒、30秒、120秒；
10. 同数据根Host重启并重复浏览器恢复。

### 斜率门

- 不能只比较第一次与最后一次；至少比较批次`0→20→100→第二个100`。
- 第二批100次后的WS/Private增长必须不显著大于第一批稳定值，线程/handles不能继续按批次增长。
- Working Set允许系统回收波动；Private Bytes用来判断活对象/allocator高水位。
- 关闭/删除验收看稳定窗口中位数，不看单个瞬时最低值。

---

## 4. 阶段50%→60%：建立正式内存门禁与可归因基线

**Objective:** 将本次手工WMIC/curl差分固化成可重复、只读、正式PID感知的验证工具。

**Files:**

- Create: `tools/memory_probe.py`
- Create: `tools/memory_scenarios.py`
- Create: `tools/tests/test_memory_probe.py`
- Modify: `tools/verify_product_surface.py`（只增加工具存在/不含secret输出门）
- Create: `docs/memory/production-baseline.md`

**Steps:**

1. 写RED：wrapper PID与监听PID不同时，probe必须选中监听58080的`dsh.exe`。
2. 写RED：指标输出必须同时包含WS、Private、threads、handles、child processes，禁止只输出一个“memory”。
3. 实现Windows WMIC/Win32采样和JSONL结果文件；每行包含阶段、时间、PID、binary SHA、配置根hash（仅路径hash，不读凭据）。
4. 实现场景：warm/list/history/git/refresh/settle；所有RPC正文固定且不含用户内容。
5. 增加response count/bytes/latency和浏览器network inventory导入字段。
6. 新增约68k事件同形fixture生成器：消息数量有限、reasoning/tool/chunk事件密集；同时支持JSONL和SQLite，不能复用正式用户会话正文。
7. 增加正式规模场景：36个会话、25次cold list、20次跨页和return-latest；现有约2002事件单测只作为算法门，不代替该场景。
8. 在当前正式入口跑一遍基线，保存到`docs/memory/production-baseline.md`，并标注本文件中的数字为首次基线。
9. 验证脚本不写正式session、不恢复Agent、不调用模型、不执行Git写操作；fixture只写隔离测试目录，最终生产验收仍使用原配置根只读路径。
10. 修订或移除README中当前不存在的`--test boot`/`--test web`门禁；将`target-memory-feature`明确标记为构建目录而非测试证据。

**Acceptance:**

- 一条命令可重复当前全部基线；
- 输出能区分一次性lazy-init与第二批线性斜率；
- 68k fixture首/中/尾页、cold list和跨页场景可重复；
- 任何Release SHA变化会使旧结果明确过期；
- 文档不再引用不存在的测试目标。

---

## 5. 阶段60%→70%：自动刷新去重与cold全量读取清零

**Objective:** 让浏览器打开/reconnect的每个自动RPC保持固定或有界内存，不重复构建大schema，不全量物化子代理/附件日志。

**Files:**

- Modify: `web/dist/plugins/ui-agent-preset.js`
- Modify: `web/dist/plugins/ui-permission.js`
- Modify: `web/dist/plugins/ui-settings-general.js`
- Modify: `web/dist/plugins/ui-settings-models.js`
- Modify: `web/dist/plugins/ui-settings.js`
- Modify: relevant client runtime/shared settings store
- Modify: `crates/host/apiproxy/src/proxy.rs`
- Modify: `crates/session/session-persistence/src/index.rs`
- Modify: `crates/session/session-persistence-jsonl/src/index.rs`
- Modify: `crates/session/session-persistence-sqlite/src/index.rs`
- Test: target crate tests and browser network regression

**Steps:**

1. RED：正式刷新出现7次`settings.describe`；断言同一generation只允许一次in-flight请求并共享结果。
2. 设计revision-aware settings snapshot store：并发去重；写操作后按revision失效；断线/reconnect重新获取。禁止永久缓存credential secret值。
3. RED：构造大child日志，`subagent.history(maxMessages=8)`旧路径inspect完整日志。
4. 将subagent history改为与session.history同一`read_history_window`契约；attached路径也避免`events().to_vec()`。
5. RED：`session.attachment`查找一个attachment引用时不得全量events Vec；增加分块visitor并允许找到后提前停止。
6. 为`PersistenceCoordinator.chains`实现闲置安全回收：锁使用期间保留exact Arc，最后waiter退出且session已delete/retire后移除；用并发barrier证明不会拆分同一ID串行域。
7. 为`SubagentContinuationManager::ChildLock.tails`实现同类exact-id/Arc闲置回收；child release/disposal后不保留历史Mutex。
8. 为`SessionWriteBehind`增加事件数和字节背压、失败暂停上限与可诊断错误；禁止丢事件或静默覆盖。对持续后端失败建立RED，确认pending不无界增长且flush仍可重试。
9. 为`SessionPreparations`增加全phase admission/budget和悬挂loader取消/超时；容量不再只约束Ready。
10. 审计live `SessionStore`事件所有权：在不破坏Agent语义前提下定义live会话上限/冷化策略；至少先增加live session和event-bytes观测门，不能直接裁剪。
11. 审计所有自动刷新RPC，确认`session.models`继续使用fixed-state model selection，不恢复Agent。
12. 浏览器刷新网络门：`settings.describe`目标1次；所有cold RPC response bytes有显式预算。
13. 正式同PID跑刷新20次和100次；检查WS/Private/threads/handles第二批斜率。

**Acceptance:**

- 正式刷新`settings.describe`从7次降至1次；
- subagent/attachment cold读取不创建全日志Vec；
- chains和child tails在delete/retire后回到稳定数量，且并发串行语义不变；
- write-behind失败场景有硬预算和可重试错误，不丢持久事件；
- Loading/Reserved/Committing preparations不能绕过全局预算；
- 100次刷新无handles/threads线性增长；
- UI preset、permission、settings、models语义与刷新/重启一致。

---

## 6. 阶段70%→80%：Git、PTY、Preview运行态生命周期

**Objective:** 收口原生命令、PTY scrollback和Preview project的进程/句柄/日志所有权。

**Files:**

- Modify: `crates/util/native-command/src/index.rs`
- Modify: `crates/host/dsh-host/src/web_preview.rs`
- Modify: `crates/terminal/terminal/src/index.rs`
- Modify: `crates/terminal/terminal-bash/src/lib.rs`
- Test: respective crate tests plus Windows production scenario

**Steps:**

1. Git RED：运行100次和第二个100次status，记录git子进程、线程、handles；确认是IOCP池高水位还是句柄未释放。
2. 给`run_native_command`增加测试 seam/diagnostic counters，仅记录active/finished/aborted child数量，不记录argv内容。
3. 若证据为重复进程成本而非泄漏：Git status增加短TTL（如250–500ms）单flight缓存，写操作后强制失效；浏览器只允许一个刷新在途。
4. 若证据为句柄残留：修正Child/stdout/stderr关闭与wait完成边界，加入100次后active child=0测试。
5. Preview project完成/stop后移出active map；如需状态展示，只保留固定大小轻量recent status LRU（例如8条），不保留handle。
6. Preview challenge增加定时/请求入口过期清扫和总数量上限。
7. PTY压力RED：1个/3个终端填满scrollback；记录每个owner和总Host增长。
8. 将Web层“每Agent 3个”提升为`TerminalSessionService`服务级每owner及全局admission cap；所有调用面共享同一权威限制。
9. 将`disposed`历史owner指针HashSet改为生命周期generation/token或可回收身份，owner cleanup后删除；加入地址复用/新owner不被误判测试。
10. 按实测调整单PTY预算，候选初始目标：2MB、5,000行、read 128KB；必须验证编译日志/中文输出/长行语义，不先拍脑袋下调。
11. 为`LocalJobRegistry`增加已完成记录的数量/字节LRU和显式retain语义；每owner并发10不能代替历史记录上限。
12. 为continuable subagent增加每父活动宽度与全局admission cap；settlement后释放activation/accepted/owned_children，不只依赖depth。
13. 验证Agent detach、terminal close、Host shutdown后session/owner/pending/disposed Map、job records、subagent activations和pwsh进程归零。
14. 增加PluginRuntime/fiber数量和外部web bundle总字节观测门；单bundle 2MiB不代表总量有界，动态fiber未dispose必须可诊断。

**Acceptance:**

- 第二批100次Git status线程/handles不继续增长；active git child=0；
- Preview settled项目不保留SubprocessHandle；
- 3个PTY的总内存增长符合预算，close/detach后子进程与registry归零；
- terminal disposed owner、completed jobs、continuable children和plugin fibers都有总量边界及回收证明；
- 终端真实命令、scrollback分页和关闭语义不回归。

---

## 7. 阶段80%→90%：全局字节预算与峰值抑制

**Objective:** 避免单个合法请求、附件或预览文件造成100–300MB级瞬时多份复制。

**Files:**

- Modify: `crates/host/dsh-host/src/lib.rs`
- Modify: `crates/host/dsh-host/src/web_preview.rs`
- Modify: attachment crates
- Modify: relevant RPC body readers/serialization paths
- Create tests for streaming/bounded body reads

**Steps:**

1. 枚举当前预算：API body 300MB、单图5MB、单消息图片100MB、preview upload/media 64MB、text 8MB、Git diff 2MB。
2. RED：构造接近上限请求，测body bytes→serde tree→domain clone→response期间峰值；禁止真的向外部provider发送。
3. 将不需要整体JSON的大上传改为流式写入/哈希；保留路径containment和原子提交。
4. 为JSON RPC建立按method预算；普通控制RPC不得继承300MB全局上限。
5. 附件请求避免base64、bytes、serde Value同时多份常驻；明确单请求聚合上限与并发信号量。
6. 为history/subagent/Git diff增加response byte预算，而不仅是事件数。
7. 加全Host“重操作并发预算”：history、附件解码、Git diff、preview upload分别有semaphore，避免各自合法但同时峰值叠加。
8. 验证超预算返回可诊断413/业务错误，不截断成损坏数据。

**Acceptance:**

- 单请求峰值有可计算上界；
- 4个并发重操作不会简单叠加到OOM；
- 正常附件、HTML/Markdown保存、Git diff、history功能不回归。

---

## 8. 阶段90%→100%：allocator差分与真实生命周期封板

**Objective:** 在活对象和数据路径收口后，选择Windows allocator策略并完成多轮、超长会话、完成、删除、刷新和重启的最终生产证明。

**Files:**

- Modify only if evidence requires: final binary allocator configuration/module
- Create: `tools/memory_acceptance.py`
- Update: `docs/memory/production-baseline.md`
- Add focused Drop/Arc lifecycle tests to relevant crates

**Steps:**

1. 固定同一Release功能代码，分别构建系统allocator和当前allocator版本；使用同一正式数据根只读工作负载比较。
2. 比较WS/Private、P95 latency、CPU、handles、threads；不使用博客或单次最低值选allocator。
3. 只有在确认无活对象残留后才评估mimalloc purge/decommit或窄边界collect；禁止每请求collect。
4. 真实生产旅程：
   - Fresh Host；
   - Fresh Chrome reconnect；
   - session list；
   - 20/100次长history；
   - 子代理list/history；
   - Git 20/100/第二100；
   - 3 PTY填充/关闭；
   - 多轮真实模型请求（保留用户provider/model，不改凭据）；
   - 完成后稳定采样；
   - 临时验收会话删除；
   - Chrome关闭；
   - 同数据根Host重启恢复。
5. 验证删除后：AgentRegistry、SessionStore、projection dirty、prepared LRU、PTY、Preview project、subagent owner均无残留；第二Host不能恢复已删除临时会话。
6. 运行focused tests、target suites、正式Release、真实浏览器网络/DOM/控制台、独立P0/P1审查。
7. 清理只属于验收的临时会话/文件/进程；不清理用户工作树或正式历史。

**Final acceptance:**

- `session.list/history`、Git、PTY、刷新均无第二批线性增长；
- 线程/handles/子进程在各生命周期边界归零或稳定在明确池大小；
- Browser close解除iframe/DOM/轮询强引用；
- Host稳定Private/WS预算由最终基线文档给出，不能沿用本计划中的暂定数字；
- 多轮、超长会话、完成、删除、刷新、重启全部通过真实58080入口。

---

## 9. 优先级与当前可记账状态

### 已可记账

- session list/history重复调用未显示线性增长；
- cold history使用有界4096事件窗口；
- prepared session LRU=5；
- projection dirty/Agent/PTY存在明确detach清理；
- browser工作台关闭会卸载iframe和DOM；前端session state LRU=16。

### 已实现但仍需生产门

- Git UI已有全部暂存、提交、提交并推送、推送；但native command第二批斜率未验证。
- PTY有4MB/10k行上限和owner close；真实命令功能及填满后回收仍未封板。

### 明确未完成

- 正式内存自动化基线工具；
- settings.describe共享快照；
- subagent.history和session.attachment cold分块读取；
- Preview settled project移除；
- Git/native command handles/threads归因；
- 大请求/附件全局峰值预算；
- allocator A/B；
- 完成/删除/重启后的最终稳定值。

当前内存治理按真实生产证据保守记账为约50%；不是功能总体完成度。只有每个后续10个百分点阶段通过其正式入口门后才上调。

---

## 10. 风险与禁止事项

- 不将Private Bytes与Working Set混为一个“内存”。
- 不用重启后的32MB值作为浏览器连接后目标。
- 不因WS上升就直接判Git泄漏；先测第二批斜率、子进程归零和handles来源。
- 不给settings快照做永不过期缓存；写后revision必须失效。
- 不用Event count代替response byte和解压字节预算。
- 不为降低PTY内存破坏真实命令、中文、ANSI或scrollback语义。
- 不在正式日志记录请求正文、会话内容、凭据或Git提交信息。
- 不修改另一个配置根，不用隔离环境结果替代正式58080结果。
- 不覆盖或删除旧计划；执行时在新基线文档记录迁移关系。

---

## 11. 每阶段交付格式

每个阶段结束只报告：

1. **可记账完成：** 当前Release SHA和正式58080已证明的纵向行为；
2. **已实现未验证：** 代码存在但缺哪个生命周期门；
3. **阻断：** 具体RPC、进程、测试或外部条件；
4. **数值：** WS、Private、threads、handles、children的基线/20/100/第二100/settled；
5. **下一阶段授权范围：** 不借一个阶段扩张到provider、凭据、账号、remote或用户数据。
