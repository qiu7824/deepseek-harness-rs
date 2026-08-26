# Rust Harness 内存彻底收口实施计划

> **For Hermes:** 按本计划逐阶段实施；任何代码修改后重新构建Release、重启正式Host、Fresh Chrome验收和独立P0/P1审查。

**目标：** 仅使用Rust原生持久层、范围分页、投影元数据和allocator策略，消除超长会话与cold列表造成的高内存常驻，并在真实58080生产入口完成多轮、超长会话、返回尾页、刷新、重启和会话删除后的稳定常驻验收。

**架构：** 先消除不必要的数据物化：`session.list`不读取完整cold日志，`session.history`不加载完整会话后再分页。持久层提供有界范围读取与轻量元数据，ApiProxy直接消费有界结果；浏览器仍保持连续事件窗口、折叠语义和按需跳转。确认所有大对象释放后，再评估Windows Rust allocator归还策略；allocator只负责释放后的OS归还，不掩盖活对象或错误数据路径。

**技术栈：** Rust 1.97.1、Tokio、Serde/serde_json、现有SessionPersistence API、JSONL/SQLite持久后端、SessionProjectionCache、真实Chrome CDP、Windows Private Bytes/Working Set采样。

---

## 已确认红灯基线

- 真正Host进程必须取`dsh.exe`子进程，不能使用Hermes后台bash包装PID。
- 内存指标严格分开：Working Set表示当前物理驻留，用于和用户原先约20MB的观察对比；Private Bytes表示进程私有承诺/常驻高水位，用于判断Rust分配与释放后的保留。禁止把两者互相比较。
- 当前Rust空Host（同正式数据根、无浏览器）实测：Working Set 14.72MB、Private Bytes 49.53MB；旧`v0.1.0-rc.8`与当前版本使用相同mimalloc选项和Tokio多线程runtime，空Host固定架构未在本次升级中改变。
- 正式Host运行约1小时后的错误测量已作废；正确进程PID测量如下：
  - 重启后、浏览器已连接的基线：Private 128.55MB。
  - 仅5次`session.list`后：149.09MB，稳定增加20.54MB；25次后148.50MB，非线性泄漏，是首次高水位。
  - 隔离同正式数据根、无浏览器Host：49.53MB。
  - 仅3个`session.history`目标页后：235.38MB，稳定增加185.85MB。
- 历史页实测：`maxMessages=50`时两页分别返回14,865和10,736个事件；当前实现完整读取约68,000事件，再复制窗口并序列化RPC。
- 结论：主因是Rust数据路径的大规模重复物化和系统allocator高水位保留；不是handles/threads泄漏，也不是单纯浏览器DOM。

## 总体验收预算

- 空闲隔离Host双基线：Private Bytes与Working Set分别记录5次稳定采样中位数；Working Set必须保持原约20MB量级，Private Bytes单独记账。
- `session.list` 25次后：稳定Private增长目标≤5MB，单次P95目标≤250ms（36个当前正式会话）。
- 三个超长历史目标页后：稳定Private增长目标≤20MB；任一RPC返回事件数必须受明确预算约束。
- 同一长会话20次跨页/返回尾页：Private不随次数线性增长，最后5次采样波动≤5MB。
- 正式58080 Fresh Chrome：左轨全索引、目标页按需跳转、Think/工具/上下文折叠、无右轨、控制台0异常。
- 多轮真实请求完成后、刷新后、关闭Chrome后、Host重启后、临时验收会话删除后分别采样稳定Private/Working Set/handles/threads。
- 不以瞬时回落、cold页面、测试数或allocator强制collect后的数字代替生产验收。

---

### 阶段1（约20%）：建立Rust范围读取契约与RED门禁

**目标：** 在持久层定义不物化完整日志的范围读取能力，并用真实68k事件会话同形fixture证明旧路径内存/事件预算失败。

**文件：**
- 修改：`crates/session/session-persistence/src/index.rs`
- 修改：`crates/session/session-persistence/src/coordinator.rs`
- 修改：`crates/session/session-persistence-jsonl/src/index.rs`
- 修改：`crates/session/session-persistence-sqlite/src/index.rs`
- 测试：对应crate现有测试模块/新增范围读取测试模块
- 修改：`crates/host/apiproxy/src/api/sessions.rs`（仅当RPC需要新增显式事件预算字段）

**步骤：**
1. 为`SessionPersistenceApi`设计只读方法，例如`read_window(id, before_seq, max_messages, max_events)`，返回header、连续事件窗口、`has_more`与必要水位；禁止内部调用`read_from(id,0)`后再切片。
2. RED：构造“消息少、工具/流事件多”的日志；断言现有路径返回>10k事件/读取全日志，新契约必须把物化事件数限制在预算内。
3. RED：JSONL和SQLite分别断言只扫描/解码目标范围；通过测试计数器证明没有读取前缀全文。
4. 定义连续性规则：不得切断同一message group、tool call/result配对、compaction summary与replacement引用；预算命中安全边界时停止。
5. 运行目标crate测试并保存红/绿证据。

**风险：** 工具事件和compaction的连续性比简单`seq LIMIT`复杂；必须先编码边界测试，不能先实现再补语义。

---

### 阶段2（约20%）：JSONL/SQLite原生有界读取实现

**目标：** 让两个Rust后端都从存储层直接读取目标窗口，不创建完整`Vec<SessionEvent>`。

**文件：**
- `crates/session/session-persistence/src/coordinator.rs`
- `crates/session/session-persistence-jsonl/src/index.rs`
- `crates/session/session-persistence-sqlite/src/index.rs`
- 相关后端索引/测试文件

**步骤：**
1. SQLite：按session identity和seq使用倒序/范围查询，先定位消息边界，再读取连续事件区间；检查/补充必要索引。
2. JSONL：利用现有seek-capable suffix/chunk索引，从目标chunk反向定位边界；不得整文件`read_to_string`或全量serde反序列化。
3. 对压缩chunk定义最大解压窗口；单chunk异常巨大时返回明确`history-window-too-large`，不静默OOM。
4. 复用缓冲区并尽早drop中间结构；避免`events.to_vec()`、`filter().collect()`、`serde_json::Value`多份复制。
5. 压测：68k同形日志跨首/中/尾页，记录物化事件数、解码字节数、耗时和Private变化。

**绿灯：** 三个目标页稳定Private增长≤20MB，事件数受预算约束，页面连续性测试全绿。

---

### 阶段3（约20%）：cold会话列表零全文读取

**目标：** `session.list`只用Rust header和持久投影元数据，不读取cold完整事件日志。

**文件：**
- `crates/host/apiproxy/src/proxy.rs`
- `crates/host/apiproxy/src/api/sessions.rs`
- `crates/session/session-projection-cache/src/index.rs`
- 必要时新增独立crate/模块承载`sessionListMetadata`投影，避免cache依赖业务crate继续扩大

**步骤：**
1. 注册Rust原生`sessionListMetadata`投影：`blank`单调true→false、`last_prompt_at`仅真实用户消息更新；state为小固定结构，apply O(1)。
2. attached会话用registry snapshot；cold会话用projection checkpoint＋header构造summary。
3. 缓存缺失时不得对所有cold会话同步全文回填：采用有界后台迁移/按文件大小阈值探测，缺失元数据时保守显示会话，不隐藏数据。
4. `userMessageRail`首次迁移与列表元数据迁移解耦；列表不因导航索引缺失而阻塞读取全部日志。
5. RED/GREEN：36个正式规模会话，25次`session.list`；断言日志全文读取计数为0（仅允许明确的小文件blank探测）。

**绿灯：** 25次列表后稳定Private增长≤5MB，P95≤250ms，标题/blank/updatedAt语义与重启后一致。

---

### 阶段4（约20%）：ApiProxy/浏览器有界分页闭环

**目标：** RPC和浏览器只接收目标窗口，不再传输/保留1万级事件页，同时保持现有UI兼容。

**文件：**
- `crates/host/apiproxy/src/proxy.rs`
- `crates/host/apiproxy/src/api/sessions.rs`
- `web/dist/plugins/client-runtime.js`
- `release/plugins/dsh-context-jump/lib/client.js`
- 必要的Rust/JS测试

**步骤：**
1. `session.history`和`subagent.history`调用持久层范围读取；attached会话也使用同一有界分页算法，避免`events.to_vec()`。
2. 增加显式`maxEvents/maxBytes`内部预算（若不暴露RPC，则Host固定安全值），错误码可诊断且不返回半截语义。
3. `loadAround`验证目标seq在页内；超预算时按安全边界缩小，不循环加载中间历史。
4. `returnLatest`失败仍保留历史窗口/liveBuffer；成功后只安装一个尾页并排空buffer。
5. Fresh Chrome真实点击：左轨索引完整、DOM消息显著少于全量、目标页出现、hasMore语义正确、折叠/滚动/控制台0异常。

**绿灯：** 20次跨页/返回尾页后Host Private无次数线性增长，Chrome关闭后Host稳定值回落到预算内。

---

### 阶段5（约20%）：Rust allocator归还策略、删除生命周期与最终封板

**目标：** 在活对象和错误数据路径清除后，解决Windows allocator保留的剩余高水位，并完成真实生命周期验收。

**文件：**
- `crates/host/dsh-cli/Cargo.toml`或最终二进制crate manifest（仅在证据支持时）
- `crates/host/dsh-cli/src/main.rs`或专用allocator模块
- 会话删除/registry/projection cache清理路径及测试

**步骤：**
1. 对比Rust系统allocator与`mimalloc`的同一固定工作负载；只比较Private/Working Set、吞吐和稳定值，不凭博客选择。
2. 若采用mimalloc：Windows启用purge decommit；仅在明确的“大批次完成/会话删除/Host长期空闲”窄边界评估`mi_collect(false/true)`，禁止每请求强制collect。
3. 证明Session、Agent、projection checkpoint、history window、attachment/tool view在关闭/删除后无强引用残留；用Arc计数/Drop探针做测试，不在正式日志输出正文。
4. 真实临时会话：多轮请求→完成→刷新→删除→稳定采样；确认删除后常驻值回到预算且第二Host无法恢复已删会话。
5. 完整target tests、Release重建、唯一58080、Fresh Chrome、Host重启、最终独立P0/P1审查、提交推送/tag。

**最终绿灯：** 无P0/P1；正式Host在全部真实路径后的稳定Private满足预算，handles/threads无增长，功能/UI/持久化无回归。

---

## 明确不采用的“假修复”

- 不把bash包装PID当作Host PID。
- 不只加`mimalloc`或定时`mi_collect(true)`掩盖全量数据物化。
- 不通过隐藏scrollbar、减少导航索引、删除历史或禁用工具/Think降低内存。
- 不把`maxMessages=50`当作事件/字节预算。
- 不切换Provider、模型、凭据或正式`DSH_HOME`。
- 不删除正式会话、`.dsh-runtime/`、`video/`或用户未知文件。
- 不用瞬时Working Set下降宣称Private常驻问题完成。

## 每阶段交付格式

- 可记账完成：真实生产链已验证的行为和稳定数值。
- 已实现未验证：代码存在但缺生产门禁。
- 阻断：具体RPC/路径/权限/测试失败，不用比例或测试数替代。
