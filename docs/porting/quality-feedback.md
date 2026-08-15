---
schema_version: 1
project: deepseek-harness-rs
baseline_commit: aecb7697adf014db1a7c13c4d78b802c73b8b10e
baseline_round: 75
created_at: 2026-08-16T04:24:06+08:00
purpose: Rust 移植缺陷复盘、质量门改进与移植模型优化数据
---

# Rust 移植质量反馈记录

## 记录规范

- 严重度：`critical`、`high`、`medium`、`low`。
- 状态：`confirmed`、`fixed`、`verified`、`deferred`、`rejected`。
- 每项缺陷必须包含基线证据、可重复检查、根因模式、回归测试和模型改进信号。
- 性能结论必须记录输入规模、构建模式、耗时和对照路径。
- “源码偏差”仅在与 TypeScript 参考实现逐行核对后使用。
- 修复提交在验证提交生成后回填，不使用临时工作树状态作为永久标识。

## 汇总

| ID | 严重度 | 类别 | 组件 | 状态 | 根因模式 |
|---|---|---|---|---|---|
| DSH-RUST-0001 | high | 状态生命周期/性能 | `dsh-token-meter` | fixed | `map_state_removed_instead_of_retained` |
| DSH-RUST-0002 | high | 复杂度/热路径 | `dsh-session`、`dsh-token-meter` | fixed | `full_snapshot_in_per_event_observer` |
| DSH-RUST-0003 | high | 源码语义偏差 | `dsh-token-meter` | fixed | `non_mutating_read_translated_as_option_take` |
| DSH-RUST-0004 | medium | 异步锁 | `dsh-workspace` | fixed | `parking_lot_guard_across_await` |
| DSH-RUST-0005 | low | 静态质量门 | `dsh-brand` | fixed | `clippy_gate_not_run_to_completion` |
| DSH-RUST-0006 | high | 并发竞态 | `dsh-timeout` | fixed | `check_then_act_cancellation_race` |
| DSH-RUST-0007 | medium | 生命周期/并发竞态 | `dsh-schedule` | fixed | `check_then_install_run_handle` |
| DSH-RUST-0008 | medium | 状态原子性 | `dsh-token-meter` | fixed | `destructive_move_before_fallible_validation` |
| DSH-RUST-0009 | high | 异步通知竞态 | `dsh-timeout` | fixed | `notify_waiters_registration_gap` |
| DSH-RUST-0010 | high | 任务风暴/写放大 | `dsh-session-projection-cache` | fixed | `async_prefix_delayed_until_spawn_poll` |
| DSH-RUST-0011 | medium | 身份漂移/复杂度 | `dsh-goal` | fixed | `snapshot_pointer_used_as_session_identity` |
| DSH-RUST-0012 | high | 背压/内存上限 | `dsh-session-persistence` | deferred | `unbounded_write_behind_queue` |

## 验证快照

| 检查 | 结果 |
|---|---|
| `cargo test --workspace --all-targets --no-fail-fast` | 191 个测试目标，1311 项通过，0 失败，Cargo 退出码 0 |
| `cargo clippy -p dsh-timeout -p dsh-brand -p dsh-cosmokit --all-targets --no-deps -- -D warnings` | 三个已清理组件均退出码 0 |
| `cargo clippy -p dsh-session-projection-cache --lib --no-deps -- -D warnings` | 退出码 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 退出码 101；剩余 26 个源码 lint 阻塞项位于 `dsh-schemastery`、`dsh-home-paths`、`cordis`，另有 4 条编译汇总错误 |
| `git diff --check` | 通过 |

严格 Clippy 的 workspace 历史欠账未计作本批修复已通过；后续质量批次必须继续处理或建立逐项豁免，不能用全量测试通过替代静态质量门。

## DSH-RUST-0001：TokenMeter 回放状态在测量后丢失

```yaml
id: DSH-RUST-0001
severity: high
status: fixed
baseline_commit: aecb7697adf014db1a7c13c4d78b802c73b8b10e
component:
  - crates/llm/token-meter/src/index.rs
source_evidence:
  - crates/llm/token-meter/src/index.rs:193-216
reference:
  - packages/llm/token-meter/src/index.ts:159-180
root_cause_pattern: map_state_removed_instead_of_retained
quality_gate_gap: existing_tests_checked_outputs_but_not_cache_lifetime
regression_test: index::tests::measurement_retains_replay_state_for_incremental_observers
fix_commit: 240b45be656740f93ff225ec6fb11aab858b5614
```

### 证据与影响

`ReplayState` 从 `states` 中 `remove` 后作为返回值交给 `measure()`，处理结束后没有重新写回。首次测量后缓存为空，`session/event` 监听器的“仅同步已观察会话”分支永远不能持续工作，后续测量从事件 0 重新回放。

TypeScript 参考实现使用 `WeakMap.get()` 取得并原位更新同一个对象，不删除状态。回归测试在基线上以“缺少 retained replay state”失败，修复后通过。

### 模型改进信号

1. 将 JavaScript `Map`/`WeakMap` 中的可变对象迁移为 Rust 时，默认保持稳定 entry 身份；除非参考源码明确 `delete`，不得用 `remove` 模拟普通读取。
2. 为缓存类移植增加“连续两次调用状态仍存在”和“事件到达后只消费新增尾部”的生命周期测试。
3. 评审生成代码时搜索 `remove(...); ... return state`，核对是否遗漏回写或破坏事件监听条件。

## DSH-RUST-0002：逐事件同步触发完整日志快照复制

```yaml
id: DSH-RUST-0002
severity: high
status: fixed
baseline_commit: aecb7697adf014db1a7c13c4d78b802c73b8b10e
component:
  - crates/core/session/src/store.rs
  - crates/llm/token-meter/src/index.rs
source_evidence:
  - crates/core/session/src/store.rs:401-410
  - crates/llm/token-meter/src/index.rs:207
  - crates/llm/token-meter/src/index.rs:356
root_cause_pattern: full_snapshot_in_per_event_observer
quality_gate_gap: no_long_session_complexity_test
regression_test: store::tests::incremental_event_reads_do_not_materialize_full_snapshot
fix_commit: 240b45be656740f93ff225ec6fb11aab858b5614
benchmark:
  build: release
  events: 5000
  full_snapshot_ms: 12519.100
  incremental_tail_ms: 31.066
  ratio: 403.0
```

### 证据与影响

`Session::events()` 在每次 append 后失效并重新克隆完整 `Vec<SessionEvent>`。TokenMeter 一旦保持回放状态，其同步监听器会在每个新事件上调用该接口；事件数为 $N$ 时，总复制量为 $1+2+\dots+N$，复杂度为 $O(N^2)$。来源事件解析还通过 `session.events()[seq]` 触发完整快照。

同机 release 对照实测：5000 个 `assistant/chunk` 事件，全量快照路径 12519.100 ms，增量尾读 31.066 ms，耗时比 403.0 倍。

修复增加 `Session::events_from(seq)` 和 `Session::event_at(seq)`；两者只克隆所需事件且不生成完整 `events_snapshot`。TokenMeter 同步改为消费新增尾部，来源事件按序号单点读取。

### 模型改进信号

1. 所有事件监听器必须标注单事件时间复杂度；监听器中禁止调用“完整集合快照”接口，除非集合有严格上限。
2. 移植 getter 时不能只验证返回值等价，还要记录所有权和复制成本；JavaScript 数组引用与 Rust 深克隆不是成本等价。
3. 长日志组件至少加入 1000、5000、10000 事件的增长率检查，比较 $T(2N)/T(N)$，防止线性路径退化为平方路径。
4. 对 `session.events()` 的新增调用执行热路径审查，区分一次性管理操作与逐事件回调。

## DSH-RUST-0003：measure 错误消费 usage anchor

```yaml
id: DSH-RUST-0003
severity: high
status: fixed
baseline_commit: aecb7697adf014db1a7c13c4d78b802c73b8b10e
component:
  - crates/llm/token-meter/src/index.rs
source_evidence:
  - crates/llm/token-meter/src/index.rs:153
reference:
  - packages/llm/token-meter/src/index.ts:116-146
root_cause_pattern: non_mutating_read_translated_as_option_take
quality_gate_gap: state_cache_bug_masked_repeated_measurement_semantics
regression_test: anchors_measurement_on_provider_usage_and_tracks_surface_delta
fix_commit: 240b45be656740f93ff225ec6fb11aab858b5614
```

### 证据与影响

TypeScript 使用 `const anchor = state.anchor`，只读取 anchor。Rust 使用 `state.anchor.take()`，首次测量后永久清空 anchor。缓存生命周期修复后，既有集成测试立即暴露该问题：第二次增量测量丢失 provider usage 基线并退回启发式低估。

修复改为借用 anchor 并克隆输出 baseline，不修改回放状态。既有测试先失败、后通过，证明该偏差此前被“每次从零重放”的另一缺陷掩盖。

### 模型改进信号

1. JavaScript 属性读取只能翻译为 Rust 借用或克隆；`Option::take`、`mem::take`、`remove` 均属于额外状态变更，必须有参考源码中的赋空或删除证据。
2. 对状态机执行成对测试：首次调用、无新增事件的重复调用、新增一个事件后的调用。
3. 修复缓存缺陷后必须重跑所有语义测试；缓存缺陷可能通过重建状态掩盖其他持久状态错误。

## DSH-RUST-0004：Workspace 状态查询跨 await 持有 parking_lot 锁

```yaml
id: DSH-RUST-0004
severity: medium
status: fixed
baseline_commit: aecb7697adf014db1a7c13c4d78b802c73b8b10e
component:
  - crates/workspace/workspace/src/entity.rs
source_evidence:
  - crates/workspace/workspace/src/entity.rs:260-264
root_cause_pattern: parking_lot_guard_across_await
quality_gate_gap: sequential_status_tests_do_not_probe_lock_scope
regression_test: entity::tests::status_releases_record_lock_before_filesystem_await
fix_commit: de7a1888a0eef861013ccd691bbe13628ed6f6f5
```

### 证据与影响

`tokio::fs::metadata(self.record.lock().path.as_str()).await` 借用锁内字符串，使临时 `MutexGuard` 存活到文件系统 future 完成。并发状态更新会同步阻塞；在单线程 runtime 中可阻塞执行器恢复，形成死锁条件。

回归测试把 `status()` poll 到 `Pending` 后立即执行 `try_lock`：基线返回失败，修复后成功。修复在 await 前克隆 path，缩短锁作用域。

### 模型改进信号

1. `parking_lot`、`std::sync` 锁内数据传入 async API 前必须转为拥有所有权的值。
2. 生成后扫描包含 `.lock()` 与 `.await` 的表达式，并对临时值生命周期进行编译器级检查。
3. 异步锁测试应将 future poll 到 `Pending`，再验证同步锁可重新取得；仅顺序调用不能证明锁作用域安全。

## DSH-RUST-0005：Branded 的 PartialOrd 阻塞 Clippy 严格门

```yaml
id: DSH-RUST-0005
severity: low
status: fixed
baseline_commit: aecb7697adf014db1a7c13c4d78b802c73b8b10e
component:
  - crates/util/brand/src/lib.rs
source_evidence:
  - crates/util/brand/src/lib.rs:48-52
root_cause_pattern: clippy_gate_not_run_to_completion
quality_gate_gap: workspace_clippy_failed_at_first_crate
verification: cargo clippy -p dsh-brand --all-targets -- -D warnings
fix_commit: d4aec0afdd68604fe5dca86761aafd38a9422bed
```

### 证据与影响

`Branded<B>` 已实现 `Ord`，但 `PartialOrd::partial_cmp` 直接委托内部字符串，触发 `clippy::non_canonical_partial_ord_impl`，使 workspace 严格 Clippy 在首个 crate 终止，遮蔽后续问题。

修复为 `Some(self.cmp(other))`，单 crate 严格 Clippy 退出码由 101 变为 0。

### 模型改进信号

1. 每轮完成条件增加目标 crate 的 `cargo clippy --all-targets -- -D warnings`，不能只运行测试。
2. Workspace 质量门应持续采集首错、修复、重跑，直到完整 workspace 通过或所有剩余项有明确豁免。
3. 对成对 trait（`Ord`/`PartialOrd`、`Eq`/`PartialEq`）使用标准委托模板，避免各自独立实现造成语义漂移。

## DSH-RUST-0006：并发取消可覆盖首个超时原因

```yaml
id: DSH-RUST-0006
severity: high
status: fixed
baseline_commit: aecb7697adf014db1a7c13c4d78b802c73b8b10e
component:
  - crates/util/timeout/src/lib.rs
source_evidence:
  - crates/util/timeout/src/lib.rs:93-101
reference:
  - packages/util/timeout/src/index.ts:107-109
root_cause_pattern: check_then_act_cancellation_race
quality_gate_gap: no_deterministic_concurrent_first_wins_test
regression_test: tests::concurrent_cancel_preserves_the_first_reason
fix_commit: 59e159fb0ee3748d32737f7f5d754fab3ddc13d6
```

### 证据与影响

`DeadlineSignal::cancel` 先读取 `cancelled`，随后独立写入 `reason` 和 `cancelled`。两个线程可同时通过初始检查，后写线程覆盖先写线程的原因，违反接口注释和 TypeScript `AbortSignal.any` 的“第一个原因获胜”语义。

确定性回归测试让两个线程都越过初始观察点，固定 `FIRST` 完成后再释放 `SECOND`：基线最终原因错误地变为 `SECOND`。修复在 reason 锁内使用原子 `swap` 认领唯一取消权；后续调用不再写入状态。9 项 `dsh-timeout` 测试全部通过。

### 模型改进信号

1. 原子布尔量的“load/check → 修改其他字段 → store”不是原子状态转换；一次性状态必须使用 `swap` 或 `compare_exchange`。
2. 原子标志与关联载荷必须定义发布顺序；载荷需在同一锁或等价同步协议下写入，读方不能观察到“已取消但原因尚未发布”。
3. “first wins”语义必须有确定性并发测试，通过闸门强制两个竞争者越过旧检查点，不能依赖概率性线程压力测试。

## DSH-RUST-0007：调度驱动任务的检查与安装不原子

```yaml
id: DSH-RUST-0007
severity: medium
status: fixed
baseline_commit: aecb7697adf014db1a7c13c4d78b802c73b8b10e
component:
  - crates/schedule/schedule/src/runtime.rs
source_evidence:
  - crates/schedule/schedule/src/runtime.rs:153-164
reference:
  - packages/schedule/schedule/src/runtime.ts:102-127
root_cause_pattern: check_then_install_run_handle
quality_gate_gap: no_concurrent_run_slot_claim_test
regression_test: runtime::tests::vacant_run_slot_is_claimed_once_under_concurrency
fix_commit: 038cc49d668c5146f297380f2c9214e1996711c8
```

### 证据与影响

`request_drive()` 先用一次 mutex 获取检查 `run.is_some()`，释放锁后创建任务，再通过第二次 mutex 获取写入 handle。并发调用可同时观察空槽、启动两个 `run_requested` 任务并让后写 handle 覆盖先写 handle。被覆盖任务不再受 `dispose()` 的 handle 等待约束，且破坏“一个 runtime 只有一个驱动任务”的合并语义。

TypeScript 参考实现运行于单线程事件循环，`this.run` 的检查和赋值之间不存在多线程抢占；Rust 移植不能直接复制该假设。修复使用通用 `install_if_vacant` 在同一次锁持有中检查并创建任务。16 个并发竞争者的回归测试确认初始化函数只执行一次；`dsh-schedule` 26 项测试全部通过。

### 模型改进信号

1. JavaScript 事件循环中的“检查字段后赋值”迁移到 `Send + Sync` Rust 类型时，必须重新评估为并发临界区。
2. `mutex.lock().is_some()` 后再次 `mutex.lock()` 写入是高风险模式；检查和安装必须在同一 guard 生命周期内完成。
3. 存储 `JoinHandle`、取消句柄或 disposer 的槽位必须测试并发单次初始化，避免句柄覆盖后失去生命周期控制。

## DSH-RUST-0008：TokenMeter 失败 fold 会破坏既有 anchor

```yaml
id: DSH-RUST-0008
severity: medium
status: fixed
baseline_commit: aecb7697adf014db1a7c13c4d78b802c73b8b10e
component:
  - crates/llm/token-meter/src/index.rs
source_evidence:
  - crates/llm/token-meter/src/index.rs:225-330
root_cause_pattern: destructive_move_before_fallible_validation
quality_gate_gap: no_failed_fold_state_atomicity_test
regression_test: index::tests::failed_fold_preserves_the_previous_anchor
fix_commit: 240b45be656740f93ff225ec6fb11aab858b5614
```

### 证据与影响

`fold_event()` 声明“在变更 replay state 前完成全部可失败准备”，但入口使用 `state.anchor.take()`，随后才解析请求头、usage 和 surface。任一解析错误都会提前返回并把原 anchor 永久清空；监听器包含 panic 后，meter 状态已发生部分提交。

回归测试预置 anchor 后提交畸形 `request/header`，基线按预期返回错误但 anchor 变为 `None`。修复为克隆候选 anchor，并保持函数末尾统一提交；TokenMeter 12 项测试全部通过。

### 模型改进信号

1. 标注“失败不改变状态”的 fold、事务和解析函数中，禁止在最后成功提交点之前使用 `take`、`drain`、`remove` 或原位写入。
2. 状态机回归必须为每个可失败分支保存调用前快照，并在错误后断言关键字段完全一致。
3. Rust 所有权便利操作不等于事务语义；为避免 clone 而提前 move 状态时，必须提供显式回滚守卫。

## DSH-RUST-0009：DeadlineSignal 可在检查与等待之间丢失通知

```yaml
id: DSH-RUST-0009
severity: high
status: fixed
baseline_commit: aecb7697adf014db1a7c13c4d78b802c73b8b10e
component:
  - crates/util/timeout/src/lib.rs
source_evidence:
  - crates/util/timeout/src/lib.rs:113-120
root_cause_pattern: notify_waiters_registration_gap
quality_gate_gap: no_cancellation_between_check_and_wait_test
regression_test: tests::cancellation_between_check_and_wait_is_not_lost
fix_commit: 59e159fb0ee3748d32737f7f5d754fab3ddc13d6
```

### 证据与影响

`cancelled()` 先检查原子标志，再创建并 await `Notify::notified()`。若 `cancel()` 恰好在两步之间调用 `notify_waiters()`，当时没有已注册 waiter，通知不会保留为 permit；随后 future 可永久等待，表现为取消已发生但等待方不返回。

确定性回归测试在检查后暂停 waiter，期间执行取消，再释放 waiter；基线稳定在 200 ms 超时。修复先创建、pin 并 `enable()` `Notified`，然后检查原子状态；10 项 `dsh-timeout` 测试全部通过。

### 模型改进信号

1. `Notify::notify_waiters()` 不保存未来 waiter 的 permit；状态条件等待必须采用“注册 waiter → 检查条件 → await”的顺序。
2. 任何“检查原子条件后 await 通知”的循环都要审查 lost-wakeup 窗口，并用闸门把通知固定发生在该窗口内。
3. 从具备 sticky `aborted` 状态的 JavaScript `AbortSignal` 迁移时，Rust 通知原语必须同时保留状态和无丢失等待协议。

## DSH-RUST-0010：Projection cache 阈值后产生 flush 任务风暴

```yaml
id: DSH-RUST-0010
severity: high
status: fixed
baseline_commit: aecb7697adf014db1a7c13c4d78b802c73b8b10e
component:
  - crates/session/session-projection-cache/src/index.rs
source_evidence:
  - crates/session/session-projection-cache/src/index.rs:156-164
  - crates/session/session-projection-cache/src/index.rs:263-283
reference:
  - packages/session/session-projection-cache/src/index.ts:205-219
  - packages/session/session-projection-cache/src/index.ts:246-263
root_cause_pattern: async_prefix_delayed_until_spawn_poll
quality_gate_gap: no_no_yield_threshold_burst_test
regression_test: coalesces_count_threshold_flushes_before_spawned_tasks_are_polled
fix_commit: 1999b94200432966cab6483e6bef69defd2f2c01
reproduction:
  runtime: tokio-current-thread
  events: 500
  write_every_events: 100
  expected_flushes: 5
  baseline_flushes: 401
```

### 证据与影响

TypeScript 的 `flushSoft()` 调用 `write()` 后，会在第一个 `await` 前同步执行 checkpoint 和 `markClean()`；计数窗口立即归零。Rust 把整个 `write()` 放入 detached task，任务获得 poll 前 `pending` 一直保持在阈值以上，后续每个事件继续生成 flush task。

current-thread 回归在不 yield 的情况下追加 500 个事件，基线产生 401 次 `session/flush`，而每 100 个新脏事件只应产生一次，共 5 次。修复把 checkpoint 截取和 dirty 清零恢复为 spawn 前同步前缀，异步任务仅执行 durability barrier 与落盘；目标 crate 19 项测试通过。

### 模型改进信号

1. JavaScript `async` 函数调用时会同步运行到首个 `await`；Rust `async fn` 在首次 poll 前完全不运行，迁移时必须显式识别同步前缀。
2. 计数阈值、debounce、single-flight 和 write-behind 触发器必须加入“同一 current-thread tick 内连续触发且不 yield”的回归。
3. 创建 detached task 前应原子认领工作并重置触发状态，不能依赖任务未来某次调度完成认领。

## DSH-RUST-0011：Goal invariant 使用快照地址作为 Session 身份

```yaml
id: DSH-RUST-0011
severity: medium
status: fixed
baseline_commit: aecb7697adf014db1a7c13c4d78b802c73b8b10e
component:
  - crates/goal/goal/src/invariant.rs
source_evidence:
  - crates/goal/goal/src/invariant.rs:22-24
  - crates/goal/goal/src/invariant.rs:134-154
reference:
  - packages/goal/goal/src/invariant.ts:41-70
root_cause_pattern: snapshot_pointer_used_as_session_identity
quality_gate_gap: no_identity_stability_test_across_append
regression_test: invariant::tests::invariant_session_key_uses_stable_session_identity
fix_commit: 0a47bcf2bb2a951d12a82f2757c129fef1a1591c
```

### 证据与影响

`session_key()` 通过 `session.events()` 创建或取得完整事件快照，再把快照 `Arc` 地址作为 key。每次 append 都使 snapshot 失效；commit listener 随后重新克隆全日志，连续事件形成 $O(N^2)$ 复制，同时新快照地址不是稳定 Session 身份。

回归测试持有旧 snapshot 以禁止分配器复用地址，基线稳定显示 key 与 `Session::identity()` 不同且 append 后漂移。修复直接使用已有稳定 identity；`dsh-goal` 13 项测试通过。

### 模型改进信号

1. 领域对象身份不得从缓存、快照、序列化产物或临时分配地址派生，应使用对象自身稳定 identity。
2. `Arc::as_ptr` 只在该 Arc 本身就是生命周期所有者时可作为进程内 key；对 getter 返回的可重建 Arc 必须判为高风险。
3. invariant 和诊断代码同样位于事件热路径，不能因“仅校验”而免除复杂度预算。

## DSH-RUST-0012：Persistence write-behind 缺少容量与字节上限

```yaml
id: DSH-RUST-0012
severity: high
status: deferred
baseline_commit: aecb7697adf014db1a7c13c4d78b802c73b8b10e
component:
  - crates/session/session-persistence/src/write_behind.rs
  - crates/session/session-persistence/src/coordinator.rs
source_evidence:
  - crates/session/session-persistence/src/write_behind.rs:50-70
  - crates/session/session-persistence/src/write_behind.rs:97-118
  - crates/session/session-persistence/src/write_behind.rs:260-295
  - crates/session/session-persistence/src/coordinator.rs:1327-1341
root_cause_pattern: unbounded_write_behind_queue
quality_gate_gap: no_slow_backend_memory_bound_test
fix_commit: null
required_design:
  - max_pending_events_or_bytes
  - explicit_block_merge_reject_or_degrade_policy
  - slow_and_failing_backend_memory_curve
```

### 证据与影响

每个事件直接追加到 `Vec<SessionEvent>`，deadline 使用 `unbounded_channel`，producer 不等待后端；配置只限制 batching 时间，没有 event 数或 payload 字节上限。后端持续变慢或失败时，pending 可无限增长，写入还会执行 `batch.clone()` 形成额外峰值副本。

该项已确认结构性无界，但尚未用慢后端采集内存曲线，也没有明确兼容的超限语义。修改为 bounded channel 会改变同步事件入口和持久化契约，因此保留为待设计项，禁止用任意丢弃策略直接修补。

### 模型改进信号

1. 名称或注释中的“bounded batching”不能替代真实容量字段；审查必须追踪 event 数、字节数和所有无界 channel。
2. write-behind 移植必须显式回答：容量单位、超限动作、失败保留、flush barrier 与 producer 背压如何交互。
3. 验收需用可控慢后端持续生产，记录 pending 上限、RSS 曲线、吞吐和事件顺序，不以短时功能测试代替容量证明。

## 聚合改进规则

以下规则可直接用于移植模型提示、训练样本筛选和生成后审查：

1. **状态身份规则**：Map/WeakMap 中的状态对象默认原位持久；普通读取不得转换成删除所有权。
2. **非变异读取规则**：参考实现没有赋值、删除或清空时，禁止生成 `take`、`remove`、`replace`。
3. **热路径复杂度规则**：逐事件、逐 token、逐 chunk 回调不得复制或扫描完整历史；必须使用游标、尾读或增量 fold。
4. **异步锁规则**：同步锁守卫不得跨 `.await`；await 前提取拥有所有权的数据。
5. **三门验收规则**：目标测试、目标 Clippy、workspace 回归是相互独立的完成条件；任一缺失均不得标记稳定完成。
6. **缺陷解掩规则**：修复缓存、重试、回退或重建逻辑后，必须重跑全部状态机测试，查找此前被恢复路径掩盖的语义错误。
7. **原子发布规则**：一次性并发状态必须原子认领写入权，并在可观察状态生效前建立关联载荷的同步关系。
8. **事件循环假设重审规则**：从单线程 JavaScript 迁移的 check-and-set 代码在 Rust 中必须使用原子操作或单次锁作用域重新建模。
9. **失败原子性规则**：可失败状态转换应先在候选副本中完成计算，成功后统一提交；错误路径不得留下部分变更。
10. **条件等待规则**：状态加通知的等待循环必须先注册通知 future，再检查状态，防止通知落入未注册窗口。
11. **Async 前缀规则**：TypeScript `async` 首个 `await` 前的同步副作用迁移到 Rust 时，必须在 spawn 前显式执行或用同步 claim 方法建模。
12. **身份来源规则**：状态表 key 必须来自领域对象稳定 identity，不得使用可重建 snapshot 或缓存分配地址。
13. **容量证明规则**：所有队列、批处理和 write-behind 都必须有可配置上限与超限策略，并通过慢消费者内存测试。
