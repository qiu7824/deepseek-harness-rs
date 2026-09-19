# Agent 生命周期与持续对话内存增长修复方案

## 1. 目标与结论

修复 Agent 退役后仍被强引用保留的问题，使连续对话、会话恢复、异常退出和重复创建销毁不再累积失效 Agent 及其历史数据。

当前源码中已确认并独立复现两处缺陷：

1. `FactoryOwnership.live_agents` 只增加记录，单个 `PreparedAgent` 销毁后不移除记录。
2. `ReactLoopAgent` 缓存的 `AgentEventDispatch` 反向强引用 Agent，首次派发事件后形成引用环。

两处缺陷必须同时修复。只清空注册表、释放界面缓存、压缩上下文或强制降低工作集，均不能消除上述强引用。

本文为实现与验收规范，尚不代表产品代码已修复或安装版已通过验收。

## 2. 证据与适用边界

### 2.1 独立复现

使用当前工作区的真实 Rust 库构建离线诊断程序，不调用外部模型，不读取或修改用户会话。

| 实验 | 实际结果 | 说明 |
| --- | --- | --- |
| 直接创建 Agent，不发送消息，释放作用域和外部引用 | `Weak::strong_count() == 0` | 对照组能够释放 |
| 直接创建 Agent，发送一次离线诊断消息，等待结束后释放 | `Weak::strong_count() == 1` | 不经过 Agent 工厂也会残留；派发路径存在独立问题 |
| 工厂连续创建、销毁 8 个同会话 ID 的 Agent | Agent、Session 注册表均为 0，8 个旧 Agent 仍存活 | 注册表为空不等于对象释放 |
| 为上述 8 个实例分别附加 1～8 MiB 合成数据 | 销毁后仍可访问全部 36 MiB 数据 | 是可达对象保留，而非仅操作系统工作集滞留 |

直接 Agent 的有消息对照不依赖真实 API 成功返回：状态派发已足以触发循环引用。36 MiB 是合成实验数据总量，不是生产进程泄漏量估算。

独立复现工程：`D:\codex操作目录\harness-memory-audit`。

```powershell
cargo run --manifest-path 'D:\codex操作目录\harness-memory-audit\Cargo.toml' --offline --quiet
```

该程序目前输出观察值，不是最终自动化通过判据；正式回归测试必须使用断言。

### 2.2 安装版边界

已观察的安装程序为 `deepseek-harness-rs-v0.1.3-alpha.22-windows-x86_64-core`，进程以 `web --port 58080` 运行。

已检查二进制 SHA-256：

```text
7A9EDF0066AA5CAA56F8AF11D7E666BEC56D7320B75DAE8DB0528379E4CE9DF2
```

对象残留实验针对当前源码构建的独立进程；尚未建立该安装二进制与当前源码的精确构建对应关系，也未对生产进程完成堆分配归因。因此，两个源码缺陷已经成立，但不能据此声称生产内存的全部增长均来自这两处。

## 3. 根因一：工厂永久持有已退役实例

### 3.1 代码位置

文件：`crates/core/agent-loop/src/index.rs`。

| 符号 | 当前行为 |
| --- | --- |
| `FactoryOwnership.live_agents` | `Mutex<Vec<Arc<PreparedAgent>>>`，保存强引用 |
| `FactoryOwnership::track` | 创建时将 `PreparedAgent` 加入列表 |
| `AgentLoop::prepare` | 每次创建或恢复实例均调用 `track` |
| `PreparedAgent::dispose` | 取消执行、等待空闲、释放作用域、注销注册表，但不解除工厂持有 |
| `FactoryOwnership::dispose` | 仅在工厂整体销毁时取走整个列表 |

当前 `track` 注释把解除跟踪视为可省略操作；在 `Arc` 所有权模型中，这个假设不成立。

持有链如下：

```text
长期存活的 AgentLoop
  → FactoryOwnership.live_agents
    → Arc<PreparedAgent>
      → ReactLoopAgent / Session
        → 完整事件日志、派生消息、上下文状态
```

### 3.2 持续对话为何放大问题

`crates/host/apiproxy/src/proxy.rs` 的提示提交路径调用 `spawn_idle_retirement`，空闲退役随后执行拥有者的 dispose。下一次需要恢复时，`AgentLoop::resume_with` 读取持久化历史并创建新的 Session/Agent 实例。

如果先前实例未释放，就可能同时保留多份不同时间点的历史。假设每轮增加约 Δ 大小的数据且每轮均退役恢复，累计保留量可能接近：

```text
Δ + 2Δ + … + nΔ = Δ × n(n + 1) / 2
```

这是满足上述条件时的增长模型，不是生产内存实测公式。存在活跃终端、后台任务或子 Agent 时，退役会被延迟，具体增长形态会不同。

## 4. 根因二：事件派发器循环强引用

### 4.1 代码位置

文件：

- `crates/core/agent-loop/src/agent.rs`：`ReactLoopAgent.dispatch` 和 `dispatcher()`。
- `crates/core/agent/src/dispatch.rs`：`AgentEventDispatch.agent`。

当前关系：

```text
ReactLoopAgent
  → Mutex<Option<AgentEventDispatch>>
    → Arc<dyn Agent>
      → 同一个 ReactLoopAgent
```

`ReactLoopAgent` 虽然通过 `Arc::new_cyclic` 保存了 `Weak<Self>`，但 `dispatcher()` 又把升级后的强引用写回 Agent 自身，重新建立引用环。Rust 的 `Arc` 不会自动回收强引用环。

`emit_status`、`pre_step`、请求决策和异常派发均可能初始化该缓存，因此正常对话及失败对话都在影响范围内。

### 4.2 历史测试为何未发现

现有退役测试 `truly_idle_agent_releases_its_owner_handle` 使用替身 Agent，验证 dispose 被调用以及代理层 owning handle 被删除；并未验证真实 `ReactLoopAgent` 的最后一个强引用消失。

现有连续提示测试验证消息全部被消费、状态恢复空闲，也不等于验证每代对象最终析构。

## 5. 修复设计

### 5.1 消除派发器自持有

优先采用局部、明确的所有权修复：

1. 移除 `ReactLoopAgent.dispatch` 缓存字段及构造初始化。
2. 保留现有调用接口，在 `dispatcher()` 内根据 `self.weak.upgrade()` 创建短生命周期派发器。
3. 派发器仅在当前事件构造或派发阶段持有 Agent，不再写入 Agent 自身。
4. 保留 `AgentEventDispatch` 对外语义，避免无必要地更改其他模块。

示意实现：

```rust
fn dispatcher(&self) -> AgentEventDispatch {
    let agent: Arc<dyn Agent> = self.weak.upgrade().expect("live agent");
    AgentEventDispatch::new(&self.loop_ctx, agent)
}
```

该片段用于说明所有权设计；落地时需同时删除旧字段及初始化并完成编译验证。

异步事件负载可以在处理期间合理持有 Agent，验收应在任务排空后执行。不得为了立即降低引用计数而提前丢弃尚未处理的状态事件、工具回调或错误通知。

不建议仅在 dispose 尾部 `dispatch.take()`：这依赖所有调用者都正确销毁，且晚到事件可能重新建立缓存。消除自持有结构比在终点补清理更可靠。

### 5.2 按实例解除工厂跟踪

工厂仍应负责活跃实例的关闭，不能简单把整个 `live_agents` 改为弱引用列表而丢失拥有者责任。

建议实现：

1. 为 `PreparedAgent` 增加指向 `FactoryOwnership` 的 `Weak` 引用。
2. 增加按确切 `PreparedAgent` 实例身份删除的 `untrack` 操作，使用 `Arc::ptr_eq` 或等价稳定身份。
3. `PreparedAgent` 完成正常清理后解除工厂跟踪。
4. 解除跟踪必须发生在共享完成信号发布之前，使等待 dispose 的调用方观察到一致的已完成状态。
5. 并发重复 dispose 继续共享同一个清理过程，不重复注销、不重复释放资源。

禁止仅按 `session_id` 删除：同一 ID 可能对应不同代运行实例，旧实例清理不得误删新实例。

### 5.3 清理顺序与错误语义

正常退役应遵循以下顺序：

1. 由既有 admission/retirement 机制阻止同一生命周期的新工作进入。
2. 检查终端、后台任务、子 Agent、电脑控制任务及待处理 inbox；存在拥有者活动则保留运行实例。
3. 刷新持久化日志，保留原有失败处理。
4. 标记关闭并取消正在执行的工作，等待 driver 停止。
5. 释放 Agent scope，注销 Agent 和 Session。
6. 从工厂列表移除该确切实例。
7. 发布 dispose 完成信号，释放清理任务的临时引用。

不得跨 `.await` 持有 `parking_lot::Mutex`。删除记录后应在锁外 drop 被移除的对象，避免析构重入锁。工厂整体退出应先关停接收新实例，再在锁内取走待清理集合，在锁外逐一等待。

当前 `DisposeCompletion` 的 Drop 会发布完成状态；如清理任务异常退出，完成信号不应被解释成“所有清理均成功”。实现时应区分正常完成与异常终止，确保错误可观察，并验证不会因已发布完成而跳过残余注销。不要在异常路径中无条件移除工厂最后持有而掩盖仍活跃的资源。

### 5.4 创建与发布失败也必须收敛

除正常退役外，逐一覆盖以下路径：

| 路径 | 必须保证 |
| --- | --- |
| `setup_and_publish` 中 setup 返回错误 | 已 prepare/track 的实例被回收，保留原错误 |
| `sessions.enter` 失败 | 未发布实例和作用域释放 |
| `agents.enter` 失败 | 回退已进入的 Session，并完成实例清理 |
| `sessions.announce` 失败 | 不只 detach 两个注册项，还要解除工厂持有 |
| `agents.announce` 失败 | 通过统一幂等清理释放全部资源 |
| 创建流程 future 被取消 | 已准备实例有明确回滚拥有者 |
| 工厂退出与 prepare 竞争 | 接收状态检查和加入拥有者列表在同一同步协议下完成 |

建议将成功发布前的回滚集中到明确的准备事务或 guard 中，避免散落的提前返回漏掉清理。guard 不得在析构中阻塞执行异步代码；如交给后台任务清理，应确保该任务被生命周期拥有者追踪，并能在停机时等待。

这些路径属于同一生命周期修复的审计与测试范围；未单独复现的路径不得标为已证实的独立泄漏。

## 6. 文件与改动范围

| 文件 | 计划改动 |
| --- | --- |
| `crates/core/agent-loop/src/agent.rs` | 删除强引用派发器缓存，改为短生命周期派发 |
| `crates/core/agent-loop/src/index.rs` | 增加按实例 untrack，收敛正常及失败清理，处理工厂退出竞争 |
| `crates/core/agent/src/dispatch.rs` | 校核持有契约；采用局部修复时无需改变公共结构 |
| `crates/core/agent-loop/tests/agent_memory_retention.rs` | 新增真实 Agent 释放、同 ID 多代生命周期、失败路径回归 |
| `crates/host/apiproxy/src/proxy.rs` 测试区域 | 增加真实生命周期退役验证，保留现有并发保护测试 |
| 独立验收工程及采样工具 | 放在 `D:\codex操作目录`，不向工作区写临时脚本 |

不改变会话磁盘格式、事件顺序、压缩协议、模型请求语义、权限、API 密钥、终端拥有关系和用户文件。

## 7. 回归测试规范

### 7.1 对象释放是首要判据

使用 `Arc::downgrade` 记录真实 Agent，在所有外部强引用、测试句柄和事件任务释放后验证：

```rust
assert!(weak_agent.upgrade().is_none());
```

测试必须确保 driver 真正开始并结束，不能只因初始状态是 Idle 就提前判定完成。使用受控通知、最终事件或测试 adapter 计数建立时序；以有限超时等待收敛，不依赖固定 sleep 作为唯一同步机制。

### 7.2 必须覆盖的测试矩阵

| 编号 | 场景 | 通过条件 |
| --- | --- | --- |
| T01 | 不经工厂创建 Agent，不对话即释放 | Weak 失效 |
| T02 | 不经工厂创建 Agent，完成一次模拟正常响应再释放 | Weak 失效，最终事件完整 |
| T03 | 不经工厂创建 Agent，模拟请求失败后释放 | Weak 失效，错误仍正确上报 |
| T04 | 工厂创建、销毁至少 20 个实例 | 所有已销毁 Weak 失效，工厂记录回到基线 |
| T05 | 同一会话连续至少 20 次退役、持久化恢复 | 只有当前活跃代可被持有，旧代全部释放 |
| T06 | 每代历史增加合成文本、工具结果和流式事件 | 不保留旧代完整历史；磁盘历史仍可恢复 |
| T07 | 两个调用者并发 dispose 同一实例 | 清理仅执行一次，两个等待者均结束 |
| T08 | 旧实例退役与新实例创建交错 | 旧清理不会删除新实例 |
| T09 | prompt 恰好在退役边界到达 | 已接收消息不丢失、不重复执行 |
| T10 | 活跃终端、后台任务、子 Agent、电脑控制任务 | 有活动时不提前退役，活动结束后能回收 |
| T11 | setup/enter/announce 失败及创建取消 | 无残余工厂记录、注册项和拥有者资源 |
| T12 | 工厂退出与 prepare 并发 | 不存在加入已排空集合的漏网实例 |
| T13 | 同一存活 Agent 连续多轮，不执行退役 | 不因派发器重复创建而改变事件顺序和运行行为 |

T05 必须走真实恢复链路，而不只复用相同字符串 ID 创建空会话；独立 8 轮诊断仅证明保留缺陷，不替代该验收。

若需要验证 Session 的最终释放，应增加测试范围的 Weak/析构观察设施，不要为此长期增加生产调试接口或把会话内容写入日志。

### 7.3 编译及测试命令

以下命令在仓库根目录运行，测试输出目录放在操作目录：

```powershell
$env:CARGO_TARGET_DIR = 'D:\codex操作目录\harness-memory-fix-target'
cargo test -p dsh-agent-loop --test agent_memory_retention
cargo test -p dsh-agent-loop --test lifecycle_failures
cargo test -p dsh-agent-loop
cargo test -p dsh-agent
cargo test -p dsh-host-apiproxy idle_retirement_tests
cargo test -p dsh-host-apiproxy
cargo check -p dsh-host-cli
```

逐条检查退出码；任一失败时先诊断原因，不因后续命令成功而忽略前面的失败。新增测试文件存在后再运行对应目标。离线缓存齐全时可添加 `--offline`。

## 8. 内存验收

### 8.1 隔离运行

使用独立数据目录、端口和本地模拟模型，禁止把压力测试消息写进日常工作区。验收工具、数据和编译缓存均置于 `D:\codex操作目录` 的专用子目录。

完成以下工作负载：

1. 固定历史大小的 100 次创建、响应、退役。
2. 同一会话逐轮增加数据的 50 次真实恢复与退役。
3. 10 个会话轮流交互，检查跨会话残留。
4. 正常响应、请求失败、取消和工具调用混合运行。
5. 活跃任务延迟退役，任务结束后再采样。

### 8.2 采样字段

记录二进制路径、哈希、构建提交、PID、时间、完成轮数、Working Set、Private Bytes、线程数、句柄数、活跃 Agent 数、工厂持有数、仍存活的退役代数及测试载荷大小。

仅记录统计量，不导出用户消息、工具输出、密钥和完整环境变量。

### 8.3 通过条件

- 所有已完成清理且无合法在途拥有者的旧 Agent，其 Weak 必须失效。
- 工厂持有数只随活跃生命周期变化，不随累计对话轮数增长。
- 同一会话恢复后不存在多代完整历史同时被失效 Agent 持有。
- 固定规模工作负载预热后，私有内存呈有界波动，不出现稳定的逐轮累积趋势。
- 增长历史场景需把当前合法历史与失效副本分开核算，不要求总内存恒定。
- 工作集回落不能单独作为通过依据；工作集不立即回到初始值也不能单独判定泄漏。
- 若上述两项修复通过而生产增长仍存在，继续做堆快照差分、分配调用栈和事件队列深度分析，不能把剩余增长直接归咎于分配器。

数值内存阈值应由相同构建、相同负载的多次基线运行确定，并写入验收结果；不得用随意指定的固定 MB 数掩盖对象残留。

## 9. 构建、安装与回退

1. 完成源码级回归及独立 Host 验收。
2. 按现有打包流程生成对应 Windows 安装产物，记录文件哈希和构建提交。
3. 更新前等待现有会话、终端和后台任务安全结束；不要强杀正在执行的任务。
4. 替换产品程序并重启，确认实际运行路径、版本与哈希指向新产物。
5. 进行少量正常交互和会话恢复验证，再观察长对话内存趋势。
6. 发生行为回归时回退上一版程序；保留用户会话数据，不执行缓存或会话的无差别删除。

程序更新不会改变旧进程已经建立的引用关系，必须启动新进程后才能验证修复效果。

## 10. 完成清单

- [ ] `ReactLoopAgent` 不再缓存反向强引用自己的派发器。
- [ ] 单实例清理按确切身份解除工厂持有。
- [ ] 创建失败、发布失败和取消路径完成统一回滚。
- [ ] 重复清理、旧新实例竞争和工厂退出竞争通过测试。
- [ ] 原有终端、后台任务、子 Agent 与电脑控制拥有关系保持正确。
- [ ] 正常与错误对话均通过 Weak 释放断言。
- [ ] 同会话真实恢复测试证明旧代不保留完整历史。
- [ ] 固定规模压力测试不再出现退役对象数量增长。
- [ ] 安装版运行身份和修复构建对应关系已核实。
- [ ] 用户会话、配置及正在执行的任务未被测试破坏。
