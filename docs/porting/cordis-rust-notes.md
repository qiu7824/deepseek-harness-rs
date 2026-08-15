# cordis Rust 移植关键决策与偏差记录

> 供后续轮次和上层 crate 移植参考。基线:`vendor/cordis` v4.0.1。

## 1. 类型映射

| TS | Rust |
|---|---|
| `Context`（Proxy 容器） | `context::Context`(Clone,`Deref` 到 `ContextInner`) |
| `this.ctx` 调用方重绑 | 显式 `caller: &Context` 参数（reflect/events/logger 方法第一个参数） |
| `any` 动态服务值 | `util::ArcValue = Arc<dyn Any + Send + Sync>` |
| `Fiber` 类 | `fiber::FiberCore`（`Arc<FiberCore>`）+ 每个 fiber 一个 `Context` |
| `DisposableList`/WeakMap | `util::DisposableList`（Arc 指针身份删除） |
| isolation/intercept 原型链 | `util::OverlayMap<V>`（entries 在 `Mutex` 内,child 指向 parent） |
| `symbol` isolation label | `u64`（`AtomicU64` 全局递增） |
| 同步 listener | `Arc<dyn Fn(&Context, Vec<ArcValue>) -> BoxFuture<...>>`(async 偏差,见 §4) |
| plugin 函数/类/对象 | `registry::Plugin` trait（`async_trait`） |

## 2. 关键语义对齐点

- **epoch**:`None` = TS `INACTIVE`(缺失依赖,不加载);`Some("")` = 无依赖,立即加载;
  `Some(":uid1:uid2")` = 依赖 uid 按**注入声明顺序**拼接(`IndexMap` 保序,对应
  `Object.keys(inject)`)。
- **fiber 生命周期**:PENDING → (epoch 可用) LOADING → ACTIVE → UNLOADING →
  DISPOSED;依赖变更 = epoch 变化 → 先 unload(逆序跑 disposer)再 reload。
- **isolation label 归属**:`ctx.isolate(name)` 把 label 写到**子** overlay;
  `provide()` 首次注册时把 label 写到**根** overlay(`this.ctx.root[isolate][name] ??=`)。
  并发首次注册用 `OverlayMap::insert_if_absent` 保证唯一。
- **intercept**:`ctx.intercept(name, config)` 建子 overlay;子 context 的 inject 配置
  在 `extend_with_fiber` 时并入 intercept overlay;`intercept_chain(name)` 祖先优先。
- **fiber ↔ ctx 引用环**:TS 是真实环,`dispose()` 断开(`fiber.context = undefined`)。
  Rust 用 `ctx: Mutex<Option<Context>>`,dispose 结束前置 `None`。**注意**:插件
  Context 必须由 fiber 持有强引用,否则 `fiber.ctx()` 失效、插件静默不加载(曾经
  的 bug:Weak 引用导致 Context 被提前释放)。
- **effect**:TS 同步执行 setup;Rust 的 setup 是 future,在 `tokio::spawn` 中执行。
  wrapper 先入 fiber disposables 再跑 setup;`dispose()` 用 `tokio::sync::Mutex` 串行化
  (第一个调用者 drain,后续 join),setup 未完成时等待 `Notify`。disposer 幂等。
- **reload/unload 互递归**:两个函数都返回 `BoxFuture`(不是 `async fn`),避免
  opaque 类型循环(E0391)。
- **inertia 链**:`spawn_inertia` 用 `Notify` + 队列;链上任务的 panic 被
  `catch_unwind` 吞掉并 `tracing::error`,保证 settle/drain 永不悬挂。
- **plugin apply panic**:`AssertUnwindSafe(apply).catch_unwind()` → `Failed` 状态,
  而不是卡死(对应 TS 的 promise rejection 路径)。
- **logger**:`LoggerService` 通过 `ctx.get("logger")` 反查自己的 `Arc`;根 Context
  注册 logger 后 `clear()` 掉内置 disposer,保持服务常驻(与 TS ctor 相同)。

## 3. 已实现模块

`crates/vendor/cordis/src/`:`context.rs` / `service.rs` / `reflect.rs` /
`registry.rs` / `fiber.rs` / `events.rs` / `logger.rs` / `util.rs` / `error.rs`。

测试(`lib.rs` + `util.rs`,13 项):
- 插件加载/卸载/依赖等待/依赖变更重载/重复 provide 失败;
- events emit/serial(bail)/waterfall/once;
- effect 幂等;isolate 作用域隔离;logger 服务暴露。

## 4. 已知偏差(需在 conformance 阶段复核)

1. **同步边界**:TS `emit/bail/on` 是同步调用;Rust listener 是 future,`on/once`
   因此是 async 方法。顺序/结果语义保持一致,但存在 async boundary。
2. **`waterfall` 返回值**:listener 返回 `None`(TS `undefined`)时瀑布结果为 dummy
   值 `()`;TS 会返回 `undefined`。
3. **`ctx.set` 错误**:TS 抛错;Rust 返回 `Result<(), String>`。
4. **服务读取**:TS `ctx.get(name)` 直接拿实例;Rust 存储的是 `Arc<实例>`,
   `ctx.get_typed::<Arc<T>>(name)` 是等价位(注册时 `arc(Arc<T>)`)。
5. **`mixin`/accessor**:TS Proxy 转发的成员访问在 Rust 不可动态化,仅保留
   `mixin(source, keys)` 与 `accessor` 的显式 API;核心便捷方法(`ctx.on`、
   `ctx.provide`…)是 `Context` 固有方法。
6. **`parallel` panic**:TS 抛 `AggregateError`;Rust 同样 panic(消息内含计数)。

## 5. 调试经验(踩坑记录)

- `parking_lot::Mutex` 不是 `Clone`;需要共享时用 `Arc<Mutex<...>>`。
- `Arc::as_ptr(&dyn Trait)` 是胖指针,不能直接 `as usize`;先 `as *const ()`。
- `Arc<dyn Any + Send + Sync>` 的 downcast 方法是 `Arc::downcast::<T>`(不是
  `downcast_arc`)。
- `dyn Fn` 不满足 `Any`;需要在 hook/事件参数里传递 trait object 时包一层
  具名 struct(`events::ListenerWrap`)。
- `JoinHandle` 不是 `Clone`;不能存起来再克隆等待。
- 运行时不应用 `parking_lot::Mutex` 的 `blocking_lock`(tokio 线程 panic)。
- tokio 的 `Notify` 只存一个 permit;多个顺序等待者用 `watch` 或 async Mutex
  串行化,不能靠单个 `notified()`。**`Notify` future 还会伪唤醒(spurious
  wakeup)**:依赖 notify 后读标志判断真伪的代码必须循环等待,或用 oneshot。
- `impl Fn(&self, ...)` 的参数不能带名,要写 `impl Fn(&Self, ...)`。
- 测试并行时 `tracing_subscriber::fmt().try_init()` 每个测试进程只成功一次,不要
  `expect`。
- **events dispatch 的监听器 ctx 是 thisArg ?? hook.ctx**(TS 语义):loader 等
  依赖"调用方 fiber ctx"的监听器必须拿到 thisArg,而不是注册方 ctx。
- **锁守卫绝不能跨 await**:`&entry.options.lock().field` 作为实参会把临时守卫
  延长到语句结束(包括 `.await`),同线程重入 `parking_lot::Mutex` 直接死锁;
  先取值再 await。`async fn` 内的 `if let Some(x) = mutex.lock().take()` 同样会
  让守卫活过后续 await,导致 future 不 `Send`。
- `Arc::new_cyclic` 的闭包里 `weak.upgrade()` 恒为 `None`;树/组互引用用
  `OnceLock` 延迟绑定。
- waterfall 监听器"先 `next()` 再决定返回值"才能保持链值;返回 `None` 会让
  结果坍缩成 dummy(与 TS `undefined` 不同,更糟——链值丢失)。
- `fiber.ctx` 必须强引用(Weak 会被提前释放导致插件静默不加载);dispose 末尾
  置 `None` 断环。
- **`Arc<T> → Arc<dyn Any>` 的 unsize 强转把 `T` 本体作为存储类型**:要存
  `Arc<T>` 句柄必须显式 `Arc::new(arc_t.clone())`(双包);否则
  `downcast::<Arc<T>>` 失败而 `downcast::<T>` 成功。
- **serde_yaml 0.9 会静默丢弃未知标签**(`!!js` 标量坍缩成普通字符串);
  saphyr 0.0.11 需要 `early_parse(false)` 才能把标量标签保留在
  `Representation(_, _, Some(tag))` 里(early parse 会丢标量标签)。
- **yaml-rust2 0.10 / saphyr 0.0.11 都换了新 API**(0.0.11 是 Node 树 +
  `Parser::new_from_str` + `Yaml::Value(Scalar)`),与老 yaml-rust 教程不符,
  必须以 registry 源码为准。
- `!!js` 标签两种归一化:`handle="tag:yaml.org,2002:", suffix="js"`(短形式)与
  `handle="", suffix="tag:yaml.org,2002:js"`(长形式);serde_yaml dump 输出
  `!tag:yaml.org,2002:js` 裸标签(语义等价,不是 `!<...>` 形式)。
- loader 的 config-only 更新走 `noSave` 路径,不写盘;文件持久化只发生在
  tree 级变更(create/remove/update/自 dispose)——与 TS 一致,勿"修复"。
- **隔离标签只有一个命名空间**:所有 label 必须来自
  `cordis::allocate_isolation_label()`(loader 的 Realm 曾自持计数器,从 1
  开始与 logger 的 label 1 撞车 → "service has been registered at <root>"
  这类只在特定进程序下才出现的伪随机失败)。
- ScopedLayers 的 effect 语义:layer 创建 + action 必须**同步**发生在
  `effect()` 调用侧(TS generator 是同步的),异步执行会造成调用后立即
  peek 不到 layer 的竞态。

## 6. dsh-session/dsh-llm/dsh-scope 轮次追加(第 5 轮)

- **服务读取双包约定**:`ctx.register_service(Arc<T>)` 走 `arc(Arc<T>)`(双包),
  读取必须 `ctx.get_typed::<Arc<T>>("name", false)`;写 `get_typed::<T>` 会静默
  返回 `None`(downcast 类型不匹配,不报错——invariants 伴生曾因此 expect 炸掉)。
- **panic 载荷渲染**:`catch_unwind` 的 `Box<dyn Any + Send>` 上
  `downcast_ref::<&str>()` 在 `&(dyn Any + Send)` 视图上**匹配失败**(同一
  payload 在 Box 视图上成功);渲染一律**按值** `payload.downcast::<&str>()` →
  `downcast::<String>()` 链式消费 Box。
- **Session::append 的观察者必须内联执行**(`futures::executor::block_on` +
  catch_unwind),不能 `tokio::spawn` 后置:TS 的同步观察者运行在
  `appending` 发布窗口内,spawn 会让窗口提前关闭——重入 append 不再被拒、
  观察者顺序也漂移。持久化监听器需要异步 I/O 时自行 spawn,listener future
  快速返回(TS 同步回调 + 返回 promise 的等价形态)。
- **重入保护窗口**:`entry.appending = true` 必须在拿 session 状态锁**之前**
  置位(internal/dispatch 内联监听器在锁内运行,同 session 重入 append 才能
  在锁前被拒,而不是死锁)。
- **listener 快照在锁内解析**:`events.collect()`(internal/dispatch 内联预钩)
  与 `log.push` 必须原子(surface 校验 → 快照 → push 同一临界区);internal 监听
  器不得回读同一 session 的状态锁(文档化)。
- **`session/created` 否决**:announce 逐监听器内联执行,首个 panic 作为否决
  `Err` 返回;create 收到否决后调 detach 回滚(announced 已置位 → 成对发出
  `session/disposed`)。
- **dsh-scope 全局注册表不能用裸指针当键**:SCOPE_PARENTS 按 key 指针、条目
  进程级保留,并行测试下分配器地址复用会让"新 key"撞上死 key 的父链记录
  (随机 panic "already bound")。改为 key 构造时分配单调 id(`AtomicU64`),
  SCOPE_PARENTS/ScopedLayers 全按 id 键控;SCOPE_TAGS 仍按 fiber 指针,但
  `createScope` 的 dispose 路径补 `forget_scope_fiber` 防地址复用串味。
- **invariant 伴随的 trace 表**:按 session 指针键控的表必须存
  `Weak<SessionInner>` 哨兵,查表时 upgrade 失败即清条目并按 TS
  `traceFor→seedSession` 语义重放 `session.events()` 重新播种(TS WeakMap
  自动清理的等价)。
- **invariant 预提交在 stage 时即应用**:Rust 端 internal/dispatch 监听器 panic
  被 containment 吞掉(TS veto 无法取消 append),所以"发布后才提交 trace"
  与"stage 时提交"等价;为保顺序语义(trace 必须逐事件顺序演化,spawn 观察者
  会乱序)选择 stage 时提交,session/event 监听器只做 staged 条目消费校验。
- **envelope 数值偏差**:TS `isSafeInteger` 接受整数浮点(`1.0`);Rust serde
  `u64/i64` 拒绝浮点。种子/头部校验对"整数浮点"更严格(文档化;真实数据不
  产生该形状)。chunk-row 的 turn/step/index 同样要求 JSON 整数。
- **JSON 数字相等**:serde_json `Number` 的 `PartialEq` 区分表示形式(`1 ≠ 1.0`);
  surface 的 tool-result 重写比较必须走 `is_deep_equal_json`(as_f64 比较,
  与 JS `===` 语义一致,`1 == 1.0`、`-0 == 0`)。
- **fork 边界**:boundary 落在 open turn 内拒绝(OPEN_TURN);`fork()` 的 create
  失败以 `ForkError::Store(String)` 透传(TS 直接抛普通 Error)。
- **invariant 安装器失败通道**:`InvariantInstaller.install` 改为接收
  `Arc<dyn Fn(&str) + Send + Sync>`(原来 `&dyn Fn(&str)` 无法移入 'static
  监听器且不 Send)。
- **dsh-llm 线格式**:标签用 kebab-case(`tool-call`/`max-tokens`),字段用
  camelCase(`callId`/`reasoningEffort`/`argumentsDelta`…),serde 用
  `#[serde(tag=…, rename_all="kebab-case")]` + 逐字段 `rename`。
  `MessageSource`/`ContentBlock` 为内部标签枚举;`ContextForm` 的
  snapshot/notice 伴生字段(sections/summary)以宽松 Option 携带(文档化偏差)。

## 7. dsh-system-prompt 轮次追加(第 6 轮)

- **`this.ctx` Proxy 重绑的 Rust 契约 = 显式 `caller: &Context` 首参**:
  TS `ctx.systemPrompt.section(...)` 会把服务方法的 `this.ctx` 重绑到
  **访问方 ctx**——effect 归属调用方 fiber(fiber dispose 即撤回注册),
  作用域注册按 `scope_of(caller)` 落层。Rust 所有注册方法
  (section/context/tools/variable/suppressRuntimeContext/assemble)都必须
  收 `caller` 并原样下传 `ScopedLayers::effect(caller, …)`,绝不能存服务
  自己的 ctx(fiber dispose 撤不掉的静默泄漏,首版测试因此失败)。
- **`#[derive(Default)]` 对"schema 默认值非零"的配置结构体是陷阱**:
  TS schema 默认 `includeHarnessIdentity: true`;Rust 派生 Default 给
  `false`,内置 identity 段和 runtime context 全部静默消失。凡是 TS
  schema `.default(...)` 非空的结构体,Default 必须手写并逐字段对齐
  schema 默认值(或强制走 `parse_config`)。
- **同步 throw 回滚(P1-1)**:TS 注册路径的 `system-prompt/change` 监听器
  同步抛错会回滚注册(gen effect 已 yield 的 undo 在 throw 时 dispose)。
  Rust 侧两层配合:(1) 该事件用**内联分发**(collect + block_on +
  resume_unwind),不能走 fire-and-forget emit;(2)
  `ScopedLayers::effect` 的 notify 调用用 catch_unwind 包住,panic 时先
  执行 undo + 回收空 layer 再 resume_unwind。注销路径 TS 顺序是先 undo
  后 notify,同步抛错同样回滚已完成,无需回退。
- **waterfall 共享可变对象**:TS 监听器原地改 `assembly.sections.push(...)`
  后 `return next()`——所有监听器看到同一对象。Rust 传值快照会丢改
  动;必须把装配包成 `SharedAssembly(Arc<Mutex<PromptAssembly>>)` 走
  瀑布,fallback 返回同一句柄,assemble 最后 lock 快照出来。
- **live 迭代语义**:TS `NamedEntries.entries()` 返回活 Map 迭代器——provider
  执行期间新注册的变量/条目**同轮生效**(spec 明确钉住)。Rust 的
  `entries()` 快照不满足;新增 `NamedEntries::len/get_index` 按位置逐条
  读(锁在每步内释放,插入追加在尾部,`for position in 0..` 自然扫到)。
  注意:工具 provider 是**快照**(TS `[...values()]` spread),不能共用
  live 迭代。
- **插值器诊断逐字对齐**:malformed 预览取 `text[open..open+16]` 加 `…`;
  `{{}}` 空名走 malformed 分支;lone `{{` 只有当其后**完全无** `}}` 才
  按字面量消费(否则 malformed);替换值不回扫。
- **scope effect 回滚 + fiber 地址复用**(延续第 6 节):SCOPE_TAGS 在
  `create_scope` 的 dispose 路径调用 `forget_scope_fiber` 清理,防并行
  测试下 fiber 指针复用串味。

## 8. dsh-agent 轮次追加(第 7 轮)

- **`Arc<T> → Arc<dyn Trait>` 的值位置强转自动,引用位置不自动**:
  `registry.enter(agent, …)`(按值)隐式强转 ✓;`registry.announce(&agent)`
  是 `&Arc<TestAgent> → &Arc<dyn Agent>`,**不会**链式强转——测试里
  `let agent: Arc<dyn Agent> = test_agent(...)` 显式标注一次即可。
- **`unwrap_err()` 要求 Ok 侧 `Debug`**:`Result<Disposer, _>`(dyn Fn 不
  Debug)和 `Result<Arc<dyn Agent>, _>`(trait 对象不 Debug)都会编译失败;
  用 `.err().expect(...)`。含 `BoxFuture` 字段的句柄(AgentHandle)需要
  手写 `Debug`(finish_non_exhaustive 跳过 future 字段)。
- **否决回滚语义**:`announce` 的监听器否决(panic→Err)**不会**自行
  detach——TS 里回滚由调用方 generator effect 的 throw 路径 dispose
  已 yield 的 disposer 完成。测试和工厂路径必须在 Err 后显式调
  detach()(announced 已置位 → 成对发出 disposed)。
- **setFactory 的重复检查必须同步**:TS `ctx.effect(() => {...})` 的
  generator 体同步执行、同步抛错;Rust fiber.effect 的 execute 是
  spawn 的 future,体内 panic 会被吞掉。重复注册检查提前到
  `set_factory` 本体(锁内检查+置位),槽的清理仍挂 effect disposer。
- **AsyncLocalStorage → tokio task_local**:initiator 环境槽用进程级
  `tokio::task_local!`(值随 `tokio::spawn` 子任务继承);嵌套边界链用
  同一 task_local 存 `Arc<InitiatorRun>`(parent 指针),关闭期排空用
  oneshot + 计数器;`futures::executor::block_on` 内联运行的非 spawn
  代码**看不到** task_local 值(文档化偏差,当前消费方全在 tokio 任务
  内)。
- **inbox splice 的坐标钳制**:`start`/`deleteCount` 先 `Math.trunc`、
  NaN→0、负 start 从尾部倒数、越界钳制到列表长度;`validate` 校验的是
  **归一化后**的坐标(与 TS 一致——超大 start 落在空列表上合法插入)。
- **`agent/inbox/spliced` 是非表面事件**:append 不带 SurfaceIntent,
  表面层不参与;durable 事件先于投影变异(session/event 同步观察者看
  到 pre-splice 列表)。
- **服务读取双包约定的再次确认**:`ctx.get_typed::<Arc<T>>(name)` 是唯一
  正确读法(register_service 双包存储);测试里写 `get_typed::<T>` 会
  `unwrap` panic——registry 插件 fiber 与 typert 读取两处都踩过。
- **trait 对象的 identity 比较**:`Arc<dyn Agent>` 的 ptr_eq 比较胖指针
  (data+vtable),同类实现下等价于 TS `===`;跨实现类型永不相等(TS 同)。

## 9. dsh-session-persistence 轮次追加(第 8 轮)

- **`BoxOpFuture` 闭包必须捕获 `Arc<Self>`,不能捕获 `&self`**:
  `BoxOpFuture<T>` 别名是 `'static` 的 `Pin<Box<dyn Future + Send>>`,
  `fn core(&self) -> BoxOpFuture` 里 `let coordinator = self;` 会把非
  'static 借用带进 future。约定:`self: &Arc<Self>` 接收者 +
  `let coordinator = Arc::clone(self);`(与 write-behind 的写闭包同型)。
- **tokio oneshot::Receiver 不 Clone**:并发 flush 共享屏障不能克隆
  Receiver;改为 `futures::future::Shared<FlushFuture>`(`.shared()`,
  `Shared` 是 Clone + Future,多等待者安全)。`Shared::new` 是私有的——
  只能经 `FutureExt::shared()`。
- **`.clone()` 在 `&Receiver` 上克隆的是引用**:`match &state.barrier {
  Some(barrier) => barrier.clone() }` 里 `barrier` 已是 `&Receiver`,
  `.clone()` 走 `Clone for &T` 得 `&Receiver`——后续 `.await` 报
  "&Receiver is not a future"。显式 `Receiver::clone(barrier)` 或先解引用。
- **锁守卫跨 await**:`match coordinator.states.lock().get(...).cloned() {
  Some(..) => .., None => ..await? }`——MutexGuard 活到 match 语句结束
  (含 await 分支) → future 不 Send。先把取值收进块表达式再 await。
- **写路径的回滚语义**:SessionWriteBehind 失败路径 batch 按序回插 +
  automaticPaused;flush 屏障失败先清 barrier 再拒绝(下一个 flush 开新
  屏障重试);TS 的 setTimeout 取消在 tokio 下改为 deadline 泵任务 +
  空队列 no-op 守卫。
- **惰性物化**:`create()` 只登记状态,首个 append 才落盘(物化+写必须
  原子);空会话 flush 后 backend 无任何 artifact——测试断言写
  `is_none()` 而非空列表。
- **load 修复后的 revision**:commitRepair 使 durable revision 改变,
  commitPrepared 返回 `None` 触发重读新前缀(TS 同),不能把旧内存视图
  挂到新 revision 上。
- **reservation 归属比较**:PreparedSource 的 `session()` 与候选 Session
  用 dsh-session 新增的 `Session::ptr_eq`/`identity()` 做对象身份比较
  (跨 crate 不能碰 `inner` 私有字段)。

## 10. dsh-session-persistence-jsonl 轮次追加(第 9 轮)

- **已解压明文 ≠ zstd 帧**:`decode_zstd_header_line(frame)` 内部先
  `decompress_zstd_frame`;对**已经解压的** header 明文再调用它会把
  JSON 明文当作帧解压 → `zstd decode failed: Unknown frame descriptor`。
  拆成两层:`decode_zstd_header_line`(帧字节)+
  `parse_zstd_header_plaintext`(明文),read 路径两者各用其一。
  排查时先用同形状的独立调试测试确认"扫描+解码"本身没问题,
  再怀疑调用处传错阶段(压缩/明文)。
- **TS scanner 的 seq-gap throw 条件看"当前行"**:`consumeEventLine`
  的 gap 分支是 `decoded.some(c => c.type === 'turn/end')` 才 throw,
  检查的是**正在解码的这一行**含不含 turn/end,不是已提交事件列表;
  不含则静默截断提交前缀并记录 issue。Rust 里 TS 的 `throw` 统一改
  成 `write()` 返回 `Err`(panic 会绕过 `scan_log().unwrap_err()`)。
- **`encodeSegment` 只特判精确 `.` / `..`**:`../etc` → `..~002Fetc`
  (点号保持字面),只有整段等于 `.`/`..` 才转义为 `~002E…`——路径穿越
  由分隔符转义兜底。测试期望照抄 TS 而不是自行"改进"。
- **UFCS 重名 trait 方法**:`SessionPersistenceApi` 与 `PersistenceBackend`
  同型同方法名(create/append/load/list/locate)→ 方法调用歧义(E0034),
  且 UFCS 下 `&Arc<T>` 不自动解引用——写
  `SessionPersistenceApi::locate(backend.as_ref(), …)`。
- **torn 修复的物理帧数**:commitRepair 把 recoveredEvents + closers
  **合成一批**追加 → 修复后文件 = header 帧 + 一个修复批帧(2 帧),
  不是"每段一帧"。断言按批而非按事件数。
- **集成测试的懒物化断言**:`create()` 后 `find_log` 为 None(首个 append
  才落盘);seq 连续性校验发生在任何字节落盘之前(先失败、磁盘零残留)。
- **zstd 帧尾截断**:只截掉帧尾 3 字节(校验和尾部)时,流解码器在报错
  前仍会吐出**全部**明文 → recoveredEvents 完整、修复路径确定;但断言
  应围绕"torn 起点 + 修复后完整帧数 + 二次 load 幂等",不依赖压缩
  字节数的单调性。

## 11. dsh-session-persistence-sqlite 轮次追加(第 10 轮)

- **`OnceCell` 就绪期与 `&self` 的矛盾**:`tokio::sync::OnceCell::get_or_init`
  的闭包必须 `'static`,而 trait 实现只有 `&self` 拿不到 `Arc<Self>`。
  解法:把就绪期要写的目标(`db`、`store_identity`)从 `Mutex<T>` 改成
  `Arc<Mutex<Option<T>>>`,闭包只捕获这些 Arc 克隆 + `path`/`journal_mode`
  值,开库逻辑下沉为自由函数。不要为了拿 Arc 去 `Arc::from_raw(self)`。
- **rusqlite 与 `node:sqlite` 对照**:`PRAGMA` 读取用 `query_row`,写入用
  `pragma_update`(避免 `execute_batch` 对返回行的 PRAGMA 报
  "Execute returned results");`query_row(...).optional()` 需要
  `use rusqlite::OptionalExtension`;`Connection` 不是 `Sync` → 单连接
  配 parking_lot 互斥,全部同步执行(等价 TS 的 `DatabaseSync`);
  阻塞开库经 `spawn_blocking` 转 async(TS 的 `ready` promise)。
- **`scanRows` 与 JSONL scanner 的语义差**:SQLite 版的 committed 边界是
  **最后一个 type 列为 `turn/end` 的行**(用原始行 type 列,不是解析后
  事件),洞/间隙 ≤ 该行 → 拒绝,之后 → 静默截断 + `tornFrom = base +
  保留行数`。seq 期望是 `base + 行下标`(循环内等价于保留长度,但照抄
  TS 公式更不易错)。
- **事务与错误路径**:TS 每个写原语都是 `BEGIN` + 闭包 + catch ROLLBACK;
  Rust 同样在互斥闭包内手动 `BEGIN`/`COMMIT`/`ROLLBACK`(rusqlite
  transaction guard 反而难表达"先提交再保留原错误")。UNIQUE 冲突的
  rusqlite 错误文本含 `UNIQUE constraint failed: events.session_id,
  events.seq`——测试断言 `/UNIQUE/` 与 TS 相同。
- **测试基建**:临时库路径必须**先建父目录**(rusqlite 不自动建);
  Windows 下删除临时目录前先显式 `backend.close()`(打开句柄会挡
  remove_dir_all,而 ctx drop 触发的 dispose 是异步的、竞态)。
  服务读取沿用双包装约定:`get_typed::<Arc<SessionStore>>("sessions")`
  返回 `Arc<Arc<SessionStore>>`(不是 `Arc<Arc<...>>` 参数)。
- **Windows 文件身份**:`MetadataExt` 无 inode/birthtime;store 身份用
  `dev:ino:created_ns`(unix)/`len:created_ns`(windows)+ 随机 store_id,
  revision 再叠 `incarnation:revision` 计数——同文件重开稳定、跨库/复现
  必不同(与 TS 的 dev/ino/birthtimeNs 语义对齐,记为偏差)。

## 12. dsh-session-projection 轮次追加(第 11 轮)

- **`Object.is` 变化门 ↔ `Arc::ptr_eq`**:单元 `apply` 的"同一引用 = 零
  下游工作"契约用 `Arc::ptr_eq(新旧 state)` 表达;单元不感兴趣时必须
  返回 `state.clone()`(Arc 克隆同分配 → 仍判相等,甚至比 TS 的
  Object.is 更稳)。注意 `&ArcValue` 上 `.clone()` 克隆的是**引用**——
  要 `Arc::clone(state)`。
- **声明合并类型表无 Rust 对应**:TS `SessionProjectionMap` 是空接口供
  域包 declare-merge;Rust 用"key=String、value=无损 JSON"的开放表替代,
  `ProjectionCheckpoint` 用 `indexmap::IndexMap<String, Row>`(serde_json
  的 Map 只收 `Value`,装不下类型行)。
- **单元 state 约束收紧为 plain JSON**:checkpoint 的 TS `structuredClone`
  换 Rust 深克隆 → state 约定为 `Arc<JsonValue>`(测试单元的 TS
  `{marks}|null` / `number` state 相应改写为 JSON 表示);非 JSON state
  在 checkpoint/restore 处大声失败。
- **WeakMap 语义**:per-session cell 按 `Session::identity()`(Arc 指针)
  作键;TS 靠 GC 释放,Rust 靠 `session/disposed` 监听器删除——只在内存
  管理上等价,读面不可观察。
- **register 的同步可见性 vs cordis effect 的异步 setup**:我的 cordis
  effect 在 `tokio::spawn` 里跑 setup,但 TS 测试要求 register 后立即
  snapshot 可见。解法:注册表**同步**突变(插入/refs+1)+ effect 只挂
  卸载 disposer;stateVersion 冲突的同步 throw 改走 `Result::Err`
  (`unwrap_err` 需要 `Ok: Debug`,Disposer 不是 → `.err().expect(…)`)。
- **onChanged 的 Set 语义**:闭包不可比,Rust 按注册 id 记账(同一闭包
  注册两次会通知两次,TS Set 去重)——记为偏差。
- **listener 捕获与锁**:`drive` 先克隆 listener 快照再遍历注册表,
  回调在注册表锁外执行,避免用户回调重入注册表死锁;测试闭包捕获
  `Mutex` 要包 `Arc` 再 clone 进闭包(move 后不可再用)。

## 13. dsh-session-stats 轮次追加(第 12 轮)

- **注册表 effect 的调用方绑定修正**:TS `register` 里 `this.ctx.effect`
  经 cordis Proxy 把 `this.ctx` 重绑到**调用方**上下文——第 11 轮误用
  了 registry 自己的 ctx,导致插件卸载时注册不被移除(HMR 测试会挂)。
  修正:`register(caller: &Context, definition)`/`on_changed(caller, …)`
  显式传调用方,effect 挂在调用方 fiber 上;session-stats 的
  `no_key_without_plugin_and_dropped_when_plugin_unloads` 用
  `ctx.plugin(StatsPlugin)` + `fiber.dispose()` 端到端钉死该语义。
- **域插件 = 纯折叠 + 注册一行**:session-stats 单元 state 即 plain
  JSON(state 折叠用 `serde_json::Value` 拷贝修改,无变化分支返回
  `Arc::clone(state_value)` 保持 ptr_eq);`StatsPlugin` 实现 cordis
  `Plugin` trait(name/inject/apply 三件套,`PluginError::new(arc(String))`),
  作为 Rust 侧插件入口。
- **Wire 命名**:TS view 输出 camelCase(`llmMs/ttftMs/decodeMs/...`);
  Rust `SessionStatsProjection` 用 `#[serde(rename_all = "camelCase")]`
  + 字段 snake_case,测试断言序列化回 TS 形状。
- **`tool/result` 的 own-key 语义**:TS 用 `Object.hasOwn` 防 callId 撞
  `Object.prototype`(constructor/toString);Rust 的 JSON Map 天然无
  原型链,行为自动等价——测试仍钉死两种名字(未配对忽略、已记录正常
  配对)。
- **分数 outputTokens 偏差**:TS `usageOutputTokens` 接受有限非负
  number(含小数);Rust 收紧为 `as_u64`(非负整数),小数被当作无效
  报告跳过——记为偏差。

## 14. dsh-settings 轮次追加(第 13 轮)

- **抽象类 → trait + 组合**:TS `SettingsProvider` 抽象类(Service.init
  generator + load/persist 抽象)在 Rust 拆成
  `SettingsStorage` trait(存储钩子)+ `SettingsProvider` struct(全部
  解析/校验/队列/事件逻辑)。`Service.init` 的"注入前 publish"改由
  `ready()` OnceCell 门控(load → publish 在 spawn 任务),测试 boot 后
  先 `provider.ready().await`。
- **调用方重绑再次显式化**:`register` 的效果挂调用方 fiber(TS
  `this.ctx.effect` Proxy 重绑),Rust 用 `register(caller: &Context,
  ns, schema, options)`;`install_settings_section` 的注入回调里
  `sctx` 要 owned clone(async move 'static)。
- **TS promise 链 → futures 链 + spawn**:per-ns 写队列用
  `Shared<BoxOpFuture>` 链(coordinator 模式);watcher 的逐回调串行 tail
  同构,但 TS promise 微任务自调度,Rust 的 segment 不 spawn 就永远
  poll 不到——**每个 segment 必须 `tokio::spawn`**,pending_tails 再存
  Shared clone 供 dispose 排空。测试断言改为轮询等待(wait_until),因为
  tokio spawn 与 await 恢复无确定顺序。
- **Data 作为通用值域**:settings 内部值统一 `schemastery::Data`
  (deepEqualJson → `Data::deep_equal(a, b, false)`;schema 校验
  `Schema::validate`;redact 走 `Schema::node()` 的 object/dict/array +
  `meta().role == Some("secret")`)。入口用 `serde_json::Value`
  (cloneJsonShaped 的 JSON 拒绝语义由类型天然满足)。
- **`describe().schema` 占位 null**:schemastery 的 `toJSON` 未移植,
  descriptor.schema 返回 null——记录偏差,配置 UI 面在 toJSON 落地前
  不可用。
- **`mutate` 的路径操作**用 `SettingsPathOp`(serde tag=op)经
  `{ops: [...]}` 包装走同一条写队列;`applyPathOp` 逐一复刻 TS
  (空路径=整体、unset 穿越缺失路径即满足、set 穿越非对象自动建中间
  对象)。
- **INVARIANT 检测**:listener panic payload `downcast_ref::<InvariantError>()`
  且 code=="INVARIANT" → 收集后重抛;其余 panic/异步拒绝包含为 warn。
- **fiber 状态镜像**:`is_unloading(ctx)` 用 `ctx.fiber.state()` 匹配
  `FiberState::Unloading | Disposed`(TS 的 const 枚举值镜像)。

## 15. dsh-agent-default-model 轮次追加(第 14 轮)

- **`installSettingsSection` 是异步接线**:TS 插件 `await ctx.plugin(...)`
  时 inject 回调已完成(服务已在),Rust 的 `ctx.inject` 创建依赖 fiber
  异步激活——服务安装后立即 `saveSelection` 会撞上"namespace 未注册"。
  解法:服务保存 wiring fiber,`ready()` await `fiber.settle()`(no-settings
  场景 fiber 永远 pending,ready 不可调用——与 TS 一致:无 settings 时
  接线永不发生)。
- **seam 值域适配**:settings 的 `setSource` 给的是 `Arc<dyn Fn() -> Data>`;
  消费方(agent-default-model)在 thunk 外层做 `from_data()` 投影回类型化
  设置——TS 泛型 hooks 的等价物。
- **fallback 端到端验证**:settings provider 装在独立插件 fiber 上,
  `fiber.dispose()` 触发 inject fiber unload → installSettingsSection 的
  effect disposer 跑 fallback(set_source(entry)+on_change)——测试断言
  `currentSelection()` 回退 entry,顺带钉死 settings 的接线语义。

## 16. dsh-llm-retry 轮次追加(第 15 轮)

- **waterfall 扩展点**:`ctx.waterfall(name, args, fallback)` 给 listener
  追加 `NextFn` 作最后参数;payload 经 `arc(Arc<T>)` 双重包装传入,listener
  侧必须 `downcast_arc::<Arc<RequestErrorPayload>>`(双包装约定的又一次
  现身)。listener 不调 `next.call()` 即裁决,调则透传下游决策。
- **AbortSignal 替代**:`CancellationSignal`(AtomicBool + Notify)承载
  请求与插件生命周期;`AbortSignal.any` = `tokio::select!` 两个
  `cancelled()`;可取消 delay 同型 select。
- **policyKey 稳定性**:TS `JSON.stringify` 把整数浮点序列化为 `0` 而非
  `0.0`;Rust serde 默认 `0.0` → 跨实现 key 不同、持久化事件会失配。
  用 `number_token(f64)`(整数 → `as i64`)对齐 stringify。
- **测试时间推进**:`#[tokio::test(flavor="current_thread", start_paused=true)]`
  + `tokio::time::advance`(需 workspace tokio 开 `test-util` feature;
  multi_thread 不支持 pause)。断言"等待前已持久化 llm/retry"先把
  drive 塞 `tokio::spawn`(future 需 'static → ctx/agent 传 owned
  clone),advance 1ms 观察事件,再 advance 满 delay 收决策。
- **Agent trait 测试桩**:15 方法 mock(OnceLock 静态 OPTIONS/KEY +
  Inbox::new(session, Default))——retry 执行器只用 `session()`,其余
  桩化;上游全链路 spec 依赖未移植的 LlmRuntime/AgentLoop,单元层在
  waterfall 扩展点等价驱动。

## 17. dsh-token-meter 轮次追加(第 16 轮)

- **投影单元 state 必须是 JsonValue**:checkpoint 深克隆要求 plain JSON
  (session-projection 第 11 轮的约束)——tokenUsage/contextPressure/
  contextBreakdown 三单元的 state 全部用 `serde_json::Value` 对象表达
  (totals/last/claim 槽),apply 里拷贝修改、无变化返回 `Arc::clone`
  保持 ptr_eq 变化门。
- **枚举 wire 形状陷阱**:`TokenMeasurementBaseline` 的 serde
  `tag="kind"` + 变体 `None { tokens }`/`Usage { tokens, usage }` 要与
  TS 的 discriminated union 逐字段一致;`ResolvedRetryPolicy` 的 flatten
  backoff 同理——wire 测试钉死。
- **`is_surface_event` + surfaceOp 强制**:user/message/assistant/message
  等 surface 事件 append 必须携带 SurfaceIntent(否则 session 边界拒绝);
  替换必须携带 sourceEventSeqs 覆盖被遮蔽节点——测试 helper 按事件类型
  自动配 intent。
- **EpochHeader 反序列化要求完整形状**:`request/header` 的 header 经
  serde 解析到 EpochHeader,任何缺失字段(如 ToolSchema.name)都使 fold
  报 malformed——测试 fixture 的 tools 形状必须与 wire 完全一致
  (name/description/parameters)。
- **BlockAssembler 未移植**:`_estimateProviderAssistant` 引用 chunk seqs
  重装 provider 输出,用本地精简组装器(BlockStart/TextDelta/
  ReasoningDelta/ToolCallDelta/BlockEnd 五类)代替完整 BlockAssembler;
  未知块变体保守跳过——记录偏差,完整版随 LlmRuntime 里程碑落地。
- **weak 语义复用**:meter 的 per-session 状态与 projection 同型
  (identity HashMap + session/disposed 清理);`sync` 用
  remove→fold→insert→remove 的换出模式避免长期持锁。

## 18. dsh-session-telemetry 轮次追加(第 18 轮)

- **同步捕获契约 vs 异步 waterfall 的矛盾**:TS 的
  `session-telemetry/record` waterfall 同步 dispatch;Rust 的
  `ctx.waterfall` 是 async(内部逐个 `block_on` listener)——在 tokio
  runtime 线程上 `futures::executor::block_on` 会报
  "cannot execute LocalPool from within another executor" (EnterError)。
  解法:coordinator 的 `redact` 把 waterfall 搬到**专用线程**
  (`std::thread::spawn` + join,线程内 block_on 合法),保持对外同步
  签名;线程池化记为后续优化。`ctx.emit` 同病——测试里用
  `ctx.parallel(...).await` 驱动事件。
- **handoff 游标起点可为 -1**:TS `session.firstLiveSeq - 1` 对空日志
  是 -1;Rust `first_live_seq() as u64 - 1` 会 overflow——游标用 `i64`,
  `first_live_seq() as i64 - 1`。
- **agent/error 双重包装**:payload 经 `arc(Arc<AgentErrorPayload>)`
  传递,listener 里 `downcast_arc::<Arc<AgentErrorPayload>>`。
- **chunk 投影的 WeakMap**:chunkSeen 按会话 identity 分 HashSet
  (`Mutex<HashMap<usize, HashSet>>`),`MutexGuard::map` 惰性插入后借用
  单会话集合——注意 map 返回 `MappedMutexGuard`(类型别写错)。
- **dispose 关停标记**:adopted 集合只存 identity;dispose 时要从
  sessions 服务列表反查 Session 对象发 shutdown 记录——TS 直接持
  Session 强引用(adopted Set),Rust 选择 identity + store 反查,避免
  coordinator 持强引用影响会话生命周期。

## 19. dsh-llm 运行时层(第 19 轮)

- **llm/stream waterfall 载荷改为 StreamFactory**:TS 的瀑布交换
  `AsyncIterable<StreamChunk>`;Rust 交换 `Arc<dyn Fn(GenerateOptions) ->
  BoxStream + Send + Sync>`(工厂),`stream()` 在**专用线程**上
  `block_on(ctx.waterfall(...))` 后同步返回(沿用 §18 线程模式)。监听器
  取 `downcast_arc::<StreamFactory>(&next.call().await)` 包一层再
  `Some(arc(wrapped))` 短路。
- **请求经共享 cell 传递**:TS 监听器原地改写 options 对象
  (`options.provider = 'routed'`)且改动能到达 adapter;Rust 里 waterfall
  args[0] 是 `arc(Arc<Mutex<GenerateOptions>>)` cell,监听器改 cell,fallback
  工厂忽略自身参数、调用时读 cell——原地改写语义的等价物。
- **adapter 边界恐慌归一**:adapter 的 `stream()` 同步抛出与迭代期 panic
  都归一为终局 `finish` chunk:`AssertUnwindSafe(...).catch_unwind()`
  包裹 dispatch 与每次 `next()`;LlmError 走 `failure` 载荷保 code,
  裸 panic 走 `normalize_llm_failure`(UNKNOWN)。取消谓词为真或 code 为
  ABORTED 时映射为 `aborted`。middleware/消费者侧的失败照常抛出。
- **INVARIANT 重抛的 panic 载荷陷阱**:`ctx.collect(DispatchMode::Emit, ...)`
  + 逐监听器 `catch_unwind` + INVARIANT 重抛的模式取自 dsh-settings;但
  **MSVC 下 `panic!("{}", InvariantError)` 的载荷经 `&(dyn Any + Send)`
  视角 downcast 不到 `String`(同一值 type_id 不同)**,而 `Box<dyn Any +
  Send>` 视角的 downcast 正常——`is_invariant_failure` 必须收
  `&Box<dyn Any + Send>` 而不是 `&(dyn Any + Send)`(settings 现有实现
  的潜伏缺陷,后续统一修复)。
- **注册句柄的 replace/释放**:registerAdapter/registerConfigurableProviders
  返回 `{ dispose: Disposer, replace: Arc<dyn Fn(...) -> Result<(), LlmError>> }`,
  释放标记用 AtomicBool(replace([]) 合法,须与 dispose 区分);routes
  在 `prepareRoutes`(先整集校验)与 `commitRoutes`(单一同步段换出)
  之间保持 all-or-nothing。
- **PrepareLlmCall 一次性分派**:`stream` 字段是
  `Arc<dyn Fn(GenerateOptions) -> Result<ChunkStream, LlmError>>`——闭包内
  Mutex<bool> 检查 dispatched、`generate_options_config_equals` 比对
  call-config 字段,再走带 prepared registration 的 waterfall(TS 同样
  带 waterfall,而非直连 adapter)。
- **for_adapter 剥离 replayState**:assistant 消息的 model source 若
  replayState 存在且历史 provider 的注册 adapter 与目标 adapter 不是同一
  实例(Arc::ptr_eq),则剥掉 replayState 保留 provider/model。
- **markAgentLoopRequest/isAgentLoopRequest 塌缩**:TS WeakSet 对象身份 →
  `GenerateOptions.agent_loop_request: bool` 显式标志(§call-config.rs 注释)。
- **discoverModels 的 baseURL 字段**:Rust 结构体字段名为 `base_url`
  (TS `baseURL`),纯内部类型不序列化,无 wire 影响。
- **测试布局**:`tests/{service,topology,invariant}.rs` 对照上游
  service.spec.ts/topology.spec.ts/invariant.spec.ts;errorChain 分类正则
  (isContextWindowExceededError/isQuotaExceededError)与 properties.spec
  留待 error.rs 分类里程碑。round 19 结束时 dsh-llm 共 59 项测试。

## 20. dsh-tools schema 层(第 20 轮)

- **schema/value 用 serde_json::Value 承载**:TS 的 realm 边界守卫
  (isPlainJsonRecord/isPlainJsonArray/内建原型校验)与敌意 getter/稀疏数组/
  NaN/-0/循环值的运行时包容全部塌缩为类型系统——serde_json 值天然 lossless,
  `is circular`/`must be a lossless JSON value` 诊断不可达,仅保留消息词汇。
- **违规消息逐字节一致**:checkSchemaNode/checkValue 都用显式任务栈复刻 TS 的
  LIFO 遍历顺序(oneOf 先分支后 tail、object 先 enter 违规再 properties 子节点
  再 required/additionalProperties tail),violations 顺序与上游测试断言一致。
- **作者 DSL 塌缩为封闭枚举**:ValueSchemaSpec 九变体取代 TS 结构类型联合;
  InferValue/InferArgs 编译期推断无 Rust 对应物,TS 的 runtime-forged 拒绝
  (未知键、type+oneOf 并存、非 bool additionalProperties、enum/const 标量
  错型、required:false、symbol 键)大部分进类型系统。保留运行时校验:oneOf
  ≥2 分支、enum 非空、编译产物再走 assertSupportedJsonSchema(与 TS 管线同
  构)——const∉enum 就在这层被拒。integer 字面量用 i64(wire 整数镜像)。
- **validateArgs 的诊断路径**:根用 '' 且 diagnosticPath('')='arguments',
  属性 path 无前导点(propertyPath 空根特判)——所以缺失必填是
  `missing required property "path"` 而非 "arguments.path"。
- **深嵌套测试的栈与复杂度**:迭代 walker 栈安全,但 5000 层 serde_json::Value
  的**递归 Drop** 需要大于 2MiB 测试线程栈(大栈 spawn 线程);诊断 path
  字符串每帧全长克隆 → O(depth²) 字符拷贝,测试深度降为 1000(栈安全断言
  等价,上游 5000)。
- **defineTool 留待运行时轮次**:TS 的 defineTool 泛型推断依赖 ToolRunContext/
  ToolDefinition(主入口类型),本轮只落地编译/校验层;Rust 版的强类型 args
  将走 serde DeserializeOwned 泛型(记录为计划偏差)。

## 21. dsh-tools 运行时层(第 21–22 轮)

- **ToolExecutionToken 塌缩为 u64 单调计数**:TS Symbol 身份 → token 值本身;
  TS 的五个 WeakMap(deferredContexts/cancellationStates/contentFinalizers/
  canonicalResults/concludingExecutions)合并为 token 键的
  `Mutex<HashMap<u64, Arc<Mutex<ExecutionState>>>>`。
- **AbortSignal 塌缩为谓词**:`Arc<dyn Fn()->bool + Send + Sync>`;wrapper 替换
  signal 写 `ToolExecution.signal: Mutex<AbortPredicate>` cell,dispatch 时 caller
  与 wrapper **谓词合并**(`||`)即融合——无监听器可泄漏;TS 的
  AbortSignal.any + listener 清理塌缩(文档注明)。
- **waterfall 载荷的 Arc 双重包装**:pre/execute 的 args 是
  `[arc(exec)]`/`[arc(exec), arc(result)]` + NextFn 追加在**末尾**(post 的
  next 在 args[2] 不是 args[1]!);fallback 返回 `arc(Arc<ToolExecutionResult>)`,
  listener 里 `downcast_arc::<Arc<ToolExecutionResult>>(&value)` 后
  `.map(|a| a.as_ref().clone())`(downcast_arc 给的是 Arc<Arc<T>>)。
- **结构化错误穿越 panic 边界**:catch_unwind 拿到的载荷默认丢 code——registry
  错误类用 `std::panic::panic_any(ToolOutputError/ToolNotFoundError)`,
  `tool_error_from_panic` 对载荷 Box 链式 downcast 恢复
  `{ name, code }`(TS 的 errorInfo 从 HarnessError 读),裸 panic 走
  render_panic。
- **async 块 + catch_unwind 的所有权**:dispatch 的 fallback future 需要
  'static → `dispatch_tool_body(self: Arc<Self>, ...)` owned receiver,调用处
  `Self::dispatch_tool_body(Arc::clone(runtime), ...)`;finalize 的 async move
  块捕获 run_ctx 后 Err 分支还要用 → 块外先 `run_ctx_for_error` clone。
- **工具 body future 的 panic**:`AssertUnwindSafe(body).catch_unwind().await`
  捕获 poll 期 panic(工具 execute 是 BoxFuture,panic 发生在 poll 而非构造)。
- **presentation 嵌套判别手写 serde**:ToolResultView 的 card 内嵌套
  search.shape/web.kind 两层 tag——serde 不支持 internal tag 嵌套,Search/
  Web/Read 变体手写 Serialize/Deserialize(先序列化为 serde_json::Map 再补
  card 键);wire 形状逐字节对照 TS。
- **approval 降级**:dsh-user-approval 未移植,`tools/pre-execute` 的 ask 决策
  统一降级 deny("requires approval (not yet supported)")——与 TS 无 approval
  服务的组合等价。
- **code/both 呈现未移植**:wireSchemas 对非 native 模式显式 panic
  (dsh-code-runtime 里程碑);native 管线完整。

## 22. agent-tool-presentation + agent-loop 底座(第 23 轮)

- **llm/stream waterfall 线程的 panic 必须重放**:dsh-llm 的 stream() 在专用
  线程 block_on waterfall,原实现 join Err 降级 fallback——把 invariant 守卫的
  fail panic 吞掉了;改为 `join().unwrap_or_else(|p| resume_unwind(p))`,TS 同步
  dispatch throw 语义等价,INVARIANT 失败传到调用者。
- **RuntimeContextProjection 的 retained 三元态**:TS `{seq,text}|null|undefined`
  → Rust 嵌套 `Option<Option<Retained>>`;listener 与 project() 共享同一
  `Arc<Mutex<...>>` cell(先构造 projection 再 clone cell 进监听器)。
- **session/event 的 subject 比较**:subject 是 `arc(Session)`(Session 可 Clone),
  按 `session.identity()`(Arc::as_ptr usize)比较而不是值相等。
- **llm/stream 守卫读共享 cell**:agent-loop invariant 的 listener 从
  `downcast_arc::<Arc<Mutex<GenerateOptions>>>(&args[0])` 取请求(cell 设计见
  §19),sessions 服务在 listener_ctx 上 get_typed(SessionStore 注册名
  "sessions");fold_request_header + derive_messages 的 JSON 逐字节比较。
- **ctx.inject 的等待 fiber**:presentAs 的 codeRuntime 等待用
  `ctx.inject(InjectSpec::new(["codeRuntime"]), callback)`,服务发布后 callback
  在注入上下文上运行——TS `ctx.inject` 的 Rust 对应(cordis fiber 自动激活)。
- **注入回调错误通道**:`tools_of(&ctx)?` 的 String 错误要
  `map_err(|e| PluginError::new(arc(e)))`(PluginError 无 From<String>)。

## 23. dsh-tools staged scheduler + agent-loop 工具调用调度器(第 24 轮)

- **staged scheduler 公开**:TS 的 TOOL_RUNTIME_SCHEDULER 符号
  (prepare/dispatch/finalize/finish 四段)在 Rust 里是 ToolRuntime 的四个
  pub 方法:prepare_scheduled(input)->Preparation、dispatch_scheduled(run_ctx)
  ->DispatchOutcome、finalize_scheduled/finish_scheduled;execute 用它们重写
  (语义不变,15 项管线测试回归通过)。create_execution 的 Final 分支必须携带
  run_ctx(TS 的 final-result 带 exec)。
- **ToolExecutionInput/ToolExecutionResult 需 Clone/canonical 公开**:
  agent-loop 调度器 clone input;合成 abort 结果的外部构造需要
  `canonical_token` 公开(TS 结果字面量无此字段,Rust 文档注明 registry-owned)。
- **并行池 = FuturesUnordered**:fill_pool 里顺序 await prepare_scheduled
  (ordered pre-execute 不重叠,TS 同),只有 dispatch future 入池;settle 序
  任意、提交序模型序(commit_ready 连续槽屏障)。ts 的 Promise.race +
  allSettled 分别对应 next().await 与 drain。
- **fill_pool 的借用**:async 闭包捕获局部变量会连环 E0499/E0502——提取为
  接收 &mut GroupState 的嵌套 async fn(状态聚合结构体)。
- **tool-result wire 断言路径**:create_tool_result_message 的内容包在
  tool-result block 里,event data 的文本路径是
  `message.content[0].content[0].text`(不是 content[0].text)。
- **scheduler_failure 几乎不可达**:Rust 的 dispatch_scheduled 自带 catch
  (全部归一为 FinalResult),TS 的 dispatch reject 通道只剩排空结构保留。

## 24. ReactLoopAgent 机器(第 25 轮)

- **Arc::new_cyclic 闭包内不能 upgrade**:strong 尚未建立,闭包只能拿
  &Weak——需要自引用的 dispatcher 改为惰性构造
  (`Mutex<Option<AgentEventDispatch>>` + `dispatcher()` 首次构建缓存;
  AgentEventDispatch/ScopeCarrier 加 Clone);inbox 通知闭包运行时才
  upgrade weak ✓。
- **activityDone 的替代不能换 future 对象**:TS 的 promise 对象 resolve
  会唤醒 await 者;Rust 若用 Shared<BoxFuture> 替换字段,旧 waiter 永挂——
  改为 epoch + `tokio::sync::watch`(begin 开新通道、finish send、when_idle
  循环 clone receiver + changed() + epoch 稳定检查)。这是本轮的挂死教训:
  测试 600s 超时定位。
- **固定 script 会无限步进**:工具调用后 step 返回 None(未 concluded) →
  下一 pre_step claim NextStep 空 → messages 空且非首步 → 继续模型调用;
  测试 adapter 必须**按调用次数换脚本**(tool-then-text)。
- **cancel 时序语义**:followup 后立即 cancel 时 driver 可能尚未开 turn →
  无 turn/end(TS 同:turn() 开头 throwIfAborted 在 turn/start 前);测试
  断言"空或 aborted"而非强制 aborted。
- **llm-retry payload 上移**:AgentRequestErrorPayload(增 retryPolicy/
  signal)+ CancellationSignal 从 dsh-llm-retry 移到 dsh-agent,retry
  插件 re-export——loop 发布 agent/request-error 不必依赖 retry 层。
- **dispatcher 热路径**:`self.dispatcher()` 每次先查缓存(Mutex<Option>),
  TS 的"构造一次不分配"以首次惰性构建近似。

## 25. AgentLoop 服务(第 26 轮)

- **AgentFactory 只有两个方法**:TS 的工厂接口在 Rust 侧是
  `async create_agent(owner_ctx, CreateAgentOptions)` +
  `async resume(owner_ctx, ResumeAgentOptions)`——没有 `create`;安装器
  对任意工厂包一层 `Arc<dyn AgentFactory>`(擦除生命周期),AgentLoop
  service 统一持有。`ctx.get_typed::<Arc<AgentFactory>>("agents.factory")`
  注册名要与 AgentRegistry 发布的名称一致。
- **registry 发布/消费的两半契约**:AgentRegistry 侧 publish
  `"agents.factory"` + `"agents.factories"`(ID 表),AgentLoop 安装时
  `ctx.on("agents/created", ...)` 收 `arc(ConfiguredAgent)` 载荷;session
  publish 走 `ctx.provide::<SessionStore>` 的 `prepare`/`enter`/`announce`
  三件套。create 流:prepare(可选 resume 的会话)→ factory.create_agent →
  注册 scope → enter session → announce。PreparedAgent.dispose 逆向:
  announce(会话退出)→ scope 释放 → 注册表删除(先 dispose 再删,避免
  半死条目)。
- **resume 深链路留桩**:TS 的 create 可从持久层重建历史;Rust 的
  `resume`/`resume_with` 返回 `"cannot resume: session persistence backend
  contract is not yet wired"` 占位(与后端里程碑联动的显式失败,不静默
  degrade);测试只钉死"sessionId 与 resumeSessionId 互斥"与"无持久化
  后端时 reject"。
- **settings 段安装**:TS 的 `installSettingsSection` 在 Rust 是
  `install_settings_section(ctx, ns, schema, data, SettingsSectionHooks
  {set_source, on_change, validate})`;schema 经 `schemastery::Schema` +
  `indexmap::IndexMap<String, Schema>` 构造(TS `Map` 序语义)——注意
  schemastery 的 `Schema::object` 参数是 IndexMap 不是 HashMap。AgentLoop
  的 ns 为 `agentLoop`,三字段:maxSteps/resumeSessionId/sessionId(注入
  上下文来自配置 JSON,set_source thunk 每次重新解析)。
- **Arc unsize 的两处 E0277**:`Arc::clone(&service)`/`Arc::clone(&self.
  agent)` 在 `dyn AgentFactory`/`dyn Agent` 上无法从 `&Arc<Self>` 推导
  unsize——直接 `service.clone()`/`self.agent.clone()`(方法调用先解引用
  再重包装,推理路径不同)。`setup_and_publish`/`resume_with` 的接收者
  从 `self: &Arc<Self>` 改回 `&self`(内部需要 owned clone 的地方显式
  `Arc::clone(&self.inner)` 而非依赖 receiver)。
- **effect 闭包的 ctx 逃逸**:安装 factory 侧 effect 的 disposer 要
  `let factory_ctx = ctx.clone()` 后再 move——ctx 借用的闭包不能
  'static;ReactLoopAgent::new 自铸 Scope(而非复用 factory scope),其
  `scope()` 访问器公开供 PreparedAgent.dispose 使用。
- **测试基建**:service 测试用 `Context::root()` + SystemPrompt::install
  + LlmRuntime::install(ScriptedAdapter provider "test") + ToolRuntime::
  install + SessionStore::install + AgentRegistry::install 六件套;
  `SessionPreparation { pub session, dispose() }`;断言覆盖 install 即建
  configured agents、重复 identity 拒绝、互斥校验、resume 拒绝四条。

## 26. dsh-session-title 系列(第 28–29 轮)

- **AbortSignal.any 的同步谓词是硬契约**:`supersedes` 测试断言"第二次
  append 返回后 request[0].signal 已 aborted"——TS 的 any() 谓词同步;若用
  "转发任务把上游 abort 镜像到融合信号"的异步实现,当前任务不 yield 前谓词
  看不到。解法:`SessionTitleSignal` 持有 `sources: Vec<SessionTitleSignal>`,
  `is_aborted()`/`abort_reason()` **同步扫描源**(自身优先),转发任务只负责
  唤醒 `cancelled()` 等待者——谓词同步、唤醒异步,两者兼得。
- **正则 crate 无 look-around**:OSC 序列的 JS `(?:(?!\x07|\x1b\\)[\s\S])*`
  改写为等价否定类交替 `(?:[^\x07\x1b]|\x1b[^\\])*`(regex crate 不支持
  look-ahead,语义逐字节等价)。
- **defer/track 的可移植 spawn**:session/event 观察者经
  `invoke_contained_session_observers` 的 `futures::executor::block_on`
  内联执行(单层,安全),而 `llm/stream` 监听器跑在 dsh-llm 的专用线程
  (无 tokio)——`spawn_detached` 先 `Handle::try_current()`,有则
  `handle.spawn`,无则 `std::thread::spawn(block_on)`;tracked future 经
  `oneshot` 统一回传(线程路径用 catch_unwind 包 block_on,panic 载荷
  `resume_unwind` 到等待者)。counter+Notify 实现 `drain()`
  (TS `Promise.allSettled` 集合)。
- **promise 共享去重**:ensureFallback 的并发去重用
  `Shared<Tracked<...>>`(oneshot 不可克隆,Shared 允许多等待者);refresh
  未 poll 前不启动——"queued fallback 提交前直接 append 的 title 被复用"
  测试依赖 current_thread 的确定性。
- **current_thread 是卸载语义的关键**:fiber.dispose 的 disposer **首次
  poll 同步**清 uid + 置 Unloading(在 yield 前),所以 append 时排队的
  fallback 任务运行时 `service_active()`(uid + FiberState::Active 双检)
  已为 false——"卸载取消排队 fallback"在 current_thread 下确定;而
  persistence 测试的 `store.flush` 在 current_thread 死锁(flush 内部
  spawn+等待),改用 multi_thread。两类测试按需要选 flavor。
- **plugin fiber 服务可见性**:`ctx.plugin(SessionTitlePlugin)` 后根 ctx
  `get_typed("sessionTitle")` 可见(cordis provide 上溯);卸载测试用
  plugin fiber,普通测试直接 `SessionTitleService::install(&root_ctx)`
  (owner fiber = 根,永 ACTIVE)。
- **provider 契约可失败**:TS 的 `generate` 可以 throw;Rust trait 返回
  `Result<_, SessionTitleError>`(message 保真;llm 层的 code/timeoutMs 在
  该层错误类型,经 provider 边界折叠为 message)。TS 结构校验的非对象/非法
  mode/缺 generate 分支类型级不可达;id 非空与重复注册仍是运行时检查。
- **投影单元的 inject 时序**:install 里 `ctx.inject(["sessionProjections"])`
  回调所在 fiber 异步激活——投影测试 install 后 `settle()` 让 fiber 落定
  再 snapshot;HMR 测试证明 fiber dispose 后 title 键消失。
- **session-title-llm 的超时**:`dsh_timeout::deadline` 的 signal 无 Clone
  且 Deadline 有 Drop(字段不可移出)——整包 `Arc::new(deadline(...))`,
  predicate 闭包经 Arc 读 `signal.is_cancelled()`;stream 循环用
  `tokio::select!{ next | deadline.cancelled() | request.signal.cancelled() }`
  三条腿(协作式挂起适配器与延迟完成适配器都覆盖);`#[tokio::test(
  start_paused)]` + `tokio::time::advance` 复刻 fake timers(需要 dev-dep
  tokio 开 `test-util` feature)。错误带 `{code: SESSION_TITLE_TIMEOUT_CODE,
  timeoutMs}` 与 TS 的 reason 形状一致。

## 27. SessionStore caller 重绑 + storage-domain + projection-cache(第 30 轮)

- **SessionStore.create/fork 的调用方重绑(1:1 修复)**:TS 的 create 内
  `this.ctx.effect` 经 cordis Proxy 重绑到**调用方** fiber——会话随属主
  fiber 卸载;Rust 原实现把 detach effect 挂在 store 自己的 ctx(root)上,
  "属主插件 fiber dispose 应 detach 会话"的 cache 测试暴露了偏差。修复:
  `create(caller: &Context, ...)`/`fork(caller: &Context, ...)` 显式调用方
  (沿用 caller-Context 显式参数约定),全仓 ~40 个调用点同步更新(子代理
  批量机械修改 + 全仓 check 验证)。这是继 settings/session-projection
  注册表之后第三处 Proxy 重绑的显式化。
- **storage-domain 的 seam 与塌缩**:TS 的 `storage` Hub(backend 注册表/
  mount/facet)未移植,domain 层的后端契约(`KvUnitDescriptor/KvUnit/
  KvFacet`)与 `DomainFacility` 一起落在 dsh-storage-domain crate——
  facility 自带 backend 注册表(`register_backend`),Hub 落地后回填;
  zod 记录 schema 塌缩为 JSON 校验闭包;`facet-unsupported` 分支类型级
  不可达(无 kv facet 的后端无法注册)。
- **写链 = tokio Mutex 公平锁**:TS promise 链式写队列 → `tokio::sync::
  Mutex<()>` 每作业获取(公平序);close 的排空屏障是最后一次获取
  (先置 Disposing 拒绝新写,再等屏障,再 unit.close,再 on_closed)。
  `OnceCell::get_or_init` 是 **async fn**——`let _ = cell.get_or_init(...)`
  不 await 时 init 从不执行(首版 close 静默不生效的坑);Domain 的
  `close()` 幂等共享 teardown 靠 `get_or_init(...).await`。
- **domain/changed 的发射语义**:cordis `emit` 是 fire-and-forget spawn,
  TS 的内联同步监听器 throw 不可达——"已提交后发射、逐监听器隔离"由
  spawn 天然满足,记偏差;测试断言事件数需先 `settle()`。
- **cache 的安装即打开域**:TS `Service.init` 异步开域,Rust install 内
  `block_on(facility.open(...))` 一次(测试拓扑下单层 block_on 安全);
  域关闭挂 effect disposer。persistence 依赖为显式
  `Arc<dyn SessionPersistenceApi>` 参数(后端注册的具体类型经 Arc 强转
  unsize;fake 在测试里实现 trait + `cordis::Service("sessionPersistence")`
  注册——插件形式的 inject 需要真实服务名)。
- **interval 定时器的 tick 延迟**:TS `setTimeout` 在监听器内同步武装;
  Rust 的 timer 是 `tokio::spawn(sleep)` 任务,sleep 在任务首次运行
  (下一个 yield)才创建——`start_paused` 测试必须在第一次 `advance`
  前 `settle()`,否则计时从 advance 之后才开始。生产语义偏差一个 tick,
  文档注明。
- **cache 的 18 项测试覆盖**:turn/end 与 detach 双强制写点、count/
  interval 节流、fail-soft(注入写失败自愈)、非 JSON 单元状态在
  checkpoint 的 downcast panic(TS 的 put 拒绝塌缩前移)、cachedSnapshot
  身份绑定(createdAt+cwd)与零单元拓扑、coldSnapshot 阶梯读(ver 失配→
  floor 0 单次全读;日志收缩→9/0 两次读;生命周期不符→重绑身份写回)。

## 28. dsh-storage Hub 回填(第 31 轮)

- **后端契约单一家移入 Hub**:第 30 轮落在 storage-domain 的
  `KvUnitDescriptor/KvUnit/KvFacet/StorageBackend/UNIT_NAME_RE` 全部上移
  dsh-storage(storage-domain 改 re-export)——与 TS 依赖方向一致
  (domain 层依赖 storage);`StorageBackend` 恢复 TS 形状:`kv() ->
  Option<Arc<dyn KvFacet>>` + `close()`,"无 kv facet 的后端"重新变得
  可表达,`facet-unsupported` 拒绝在 facility.open 显式重建。
- **DomainFacility 回到 Hub 路由**:install 不再收 backend 列表,改为
  `DomainFacilityConfig { backend: 默认路由, routes: 按域名覆盖 }`,
  内部 `storage.backend.get(name)` + `kv()` + `open(descriptor)`;
  facility 同时 `mount("domain", arc(facility))` 挂载 form(卸载 effect
  先 closeAll 再 unmount),并注册 `storageDomain` 服务。第 30 轮的
  "facility 自带注册表"是 Hub 落地前的临时形态,现已回填。
- **TS 声明合并表单表 → 字符串键 ArcValue**:`StorageForms` 的
  declaration merging 塌缩为 `mount(form: &str, facility: ArcValue)`,
  form 值经 `downcast_arc::<Arc<DomainFacility>>` 取回(注意 Arc 双包装:
  存的是 `arc(facility.clone())`,payload 是 `Arc<DomainFacility>` 本身,
  downcast_arc::<DomainFacility> 会失配)。
- **registry/mount 的过期 disposer 守卫**:register/mount 的 disposer
  只移除"自己的那一次贡献"(`Arc::ptr_eq` 比对当前条目),dispose 后再
  注册、旧 disposer 再触发不会误删后继——TS 同款守卫;parking_lot
  Mutex 不可克隆,表要包 `Arc<Mutex<...>>` 再进闭包。
- **后端错误经 StorageError 传码**:KvUnit/KvFacet 的方法返回
  `Result<_, StorageError>`(code+message);domain 运行时自己的错误仍用
  TS prose 字符串(仓库内无消费者 switch code),边界处 `.map_err(|e|
  e.message)` 折叠——两种错误层并存,文档注明。

## 29. dsh-storage-json + InvariantRegistry caller 重绑(第 32 轮)

- **原子发布上 spawn_blocking**:TS 的 async fs 走 libuv 线程池;Rust 的
  `write_atomic` 是同步 std::fs(临时文件 `wx` + write + sync_all +
  rename + POSIX 目录 fsync),由 unit.publish 经 `tokio::task::
  spawn_blocking` 驱动——首版 async fn 内联阻塞 I/O 会让"手动 poll 一次
  应 Pending"的 drain 测试直接 Ready(单 poll 全跑完),暴露了阻塞问题。
- **drain 测试的死锁教训**:手动 noop-waker poll 让写进入在飞状态后,若
  直接 `close().await`,close 的 drain 屏障等 in_flight 计数归零,而计数
  的递减在 publish future 的续体里(尚未被 poll)——测试本身死锁。正确
  形状:close 放独立 tokio 任务,主任务先 await 写 future(续体跑完释放
  计数),再 await close 任务。
- **InvariantRegistry.register 的 caller 重绑(第四处)**:TS 的 register
  内 `ctx.effect`/`ctx.plugin` 经 Proxy 重绑到**调用方** fiber——伴生插件
  卸载时保留自动释放、子 fiber 的父级是调用方。Rust 原实现挂在 registry
  自己的 owner ctx 上,companion apply 丢弃返回 disposer 后"卸载再注册"
  测试触发 "already registered"。修复:`register(caller: &Context,
  package_name, installer)`,保留清理与子 fiber 都挂调用方;全仓 21 个
  伴生调用点同步更新(子代理批量机械修改)。此前四处 Proxy 重绑:
  settings/session-projection/session-store/invariants。
- **后端即生命周期服务**:TS `ctx.provide(storageBackendServiceKey(
  'json'), backend)`——Rust 让 `JsonStorageBackend` 直接 impl
  `cordis::Service`(service_name = "storage.backend.json"),插件 apply
  里 `ctx.register_service(backend.clone())`;fiber 卸载时服务随 provide
  消失,与 registry 注销(apply 的 effect disposer)互补。
- **JsonKvUnit 的守卫跨 await**:parking_lot MutexGuard 跨 await 让
  async_trait 的 Send future 编译失败——put/delete/setGlobal 改为
  "短临界区变更 → drop 守卫 → publish → 失败时再入锁回滚"三段式;undeclared
  table/global 的 TS 纯 Error 折叠为 StorageError(prose 保真,无码)。

## 30. dsh-storage-sqlite(第 33 轮)

- **rusqlite Connection 的 !Sync**:节点 `node:sqlite` 的 DatabaseSync
  单线程同步;Rust 的 Connection 是 Send+!Sync——共享句柄定为
  `Arc<parking_lot::Mutex<Connection>>`,每个 primitive 锁内同步执行
  (无 await 点),语句复用走 `prepare_cached`(TS 构造时 prepare 一次的
  等价物,免长借)。开库序列(建目录/`wx` 独占建文件/PRAGMA/建表/打戳)
  经 spawn_blocking 进 ready 的 OnceCell。
- **sticky 失败 = OnceCell<Result<..>>**:TS 的 `ready` promise 一旦拒绝
  对所有调用者保持拒绝;Rust 用 `OnceCell<Result<Arc<Mutex<Connection>>,
  StorageError>>` 缓存 Err——粘性一致。`user_version` 打戳放在建表之后
  ("失败留 0 可修复重开"测试钉死:CREATE TABLE 撞 index 抛错时介质仍是
  v0)。
- **pending open 槽位**:双开守卫与关闭排空需要"名字已预留但单元还在
  物化"的中间态——`Slot::{Open, Pending(oneshot::Receiver)}`;open 的同步
  前缀插入 Pending 再 spawn 物化任务(完成换 Open、失败移除),close 的
  drain 对 Pending await receiver(成功则再 close 单元、失败无事可做)。
- **手动 poll 复刻 TS 同步启动**:与 json drain 测试同理,TS 的
  `kv.open(...)` 同步前缀(守卫+预留)在 Rust 是 future 首 poll——测试用
  noop-waker 手动 poll 一次断言 Pending,再在独立任务里 close、主任务
  await open future(物化任务是独立 spawn,不依赖 pinned future 的 waker,
  故 close 的 drain 不会死锁)。
- **原型污染键天然安全**:TS 的 `Object.create(null)` 记录表防
  `__proto__` 覆盖原型;Rust 的 serde_json Map 无原型链,行为自动等价,
  测试仍钉死 `__proto__`/`constructor` 键往返。

## 31. dsh-workspace(第 34 轮)

- **install 边界的同步化**:TS `Service.init` 的 async 打开+引导在 Rust
  塌缩为 `WorkspaceRegistry::install()` 内的单层 `futures::executor::block_on`
  (domain open、recoverPendingMutation、header 索引、bootstrap 各一处;
  测试都跑在 `#[tokio::test(current_thread)]` 里,block_on 只是临时阻塞,
  不嵌套)。TS 的 inject 门控(pending 直到 sessionPersistence 出现)塌缩为
  install 参数——不存在"未启动"中间态。
- **pending-marker 双写协议**:create 先写 `pendingMutation:create` 再 put
  记录、最后写 order;delete 先写 order+pending 再删记录;每条注册表操作
  (`enqueue_operation`)开头重试 `recoverPendingMutation`。注入故障测试
  (selectiveFailureBackend 按 primitive 计数)钉死:标记写失败不留实体、
  记录写失败回滚标记、order 写失败回滚记录、双失败聚合报错且保留可恢复
  标记、delete 提交后标记清理失败仅告警并留待重启恢复。
- **mutate 的"返回原记录"信号**:TS 用对象同一性(`changed === current`)
  判 no-op,Rust 无法测值传递后的地址同一性,改为闭包返回
  `Result<Option<WorkspaceRecord>, String>`:`None` = TS 的 `return record`
  (原样返回),`Some(next)` = 新对象;None 且 prune 无变化才吃哨兵——语义
  与 TS 完全一致(含 setTitle 同标题仍写 updatedAt 的边界)。
- **不变式伴生的异步偏差**:TS `ctx.emit` 同步执行,违规即 throw;Rust
  emit 是 spawn 的火忘分发,违规 panic 落在监听任务里不可观察。伴生
  逻辑抽成纯函数 `check_change(change, registry_has, fail)` 单测全覆盖,
  再经 `installer().install(ctx, fail_collector)` 直接安装监听器做端到端
  断言(事件后 sleep 让 spawn 的监听跑完)。
- **重启测试 = domain close + 新 harness**:TS 的 `fiber.dispose()` 关闭
  domain;Rust 无 fiber 暴露,测试直接 `registry.domain().close()` 后在新
  ctx 上重开同一 pool。Cordis 服务双注册是 panic("already registered"),
  所以 TS 的"同 ctx 重插插件"场景改为新 harness 等价表达。
- **Windows canonicalize 的 `\\?\` 前缀**:`tokio::fs::canonicalize` 返回
  带 `\\?\` 的路径;fixture 与断言全部使用 canonical 拼写比较
  (stored_pool 的 record.path 也要 canonical),否则 bootstrap 的
  by_path 合并会因拼写差异漏并。符号链接 alias 用例用
  `std::os::windows::fs::symlink_dir`(本机已开开发者模式)。
- **Windows 上 symlink 目录别名**验证 `realpathNormalize` 的唯一性 canon;
  规范化后 `\\?\` 前缀对 `tokio::fs::metadata`/`remove_dir_all` 透明。

## 32. spill 三包(第 35 轮)

- **服务擦除注册**:TS 抽象类 `SpillStore` 的 `super(ctx)` 注册在 Rust 是
  `impl Service for dyn SpillStore` + `let erased: Arc<dyn SpillStore> = store;
  ctx.register_service(erased)`;消费者 `ctx.get_typed::<Arc<dyn SpillStore>>`
  单步 downcast 即得 trait 对象。
- **encodeSegment 的 UTF-16 语义**:TS `charCodeAt(i)` 按 UTF-16 code unit
  迭代(非 BMP 字符拆 surrogate pair 双转义);Rust 用 `raw.encode_utf16()`
  逐 unit 判断 `[A-Za-z0-9._-]` 安全集、其余 `~XXXX` 大写 4 位 hex——
  与 TS 逐字节一致(含 `~`→`~007E`、`.`/`..` 整段 token、空串→`~`)。
- **randomBytes(6) → uuid 前 12 hex**:v4 UUID 的 simple 小写 hex 前 12 位
  恰为 48 位密码学随机,拼写与熵完全等价,免引 getrandom;sessionDir 的
  sha256 截 12 位小写 hex 用 `sha2`(磁盘布局与 TS 逐字节一致)。
- **独占创建 = create_new**:TS `open(path, 'wx', 0o600)` 的 symlink 种植
  防御在 Rust 是 `OpenOptions::write(true).create_new(true)`(存在即 EEXIST,
  含 symlink);0700 目录/0600 文件权限位在 `#[cfg(unix)]` 下设置,镜像
  TS spec 的 `win32` 跳过。写完显式 `flush()` 再返回——调用方(测试与
  policy)在 resolve 后立即回读。
- **spill-policy 的 waterfall 形态**:`tools/post-execute` 的 args 是
  `[Arc<ToolExecution>, Arc<Arc<ToolExecutionResult>>, NextFn]`——live
  句柄而非裸值,downcast 目标要取 `Arc<…>` 层;listener 必须先
  `next.call()` 拿下游 decision(TS 先 `await next()`),返回值必须是
  `Some(arc(decision))`(waterfall 对 `None` 吃 dummy_value,下游 downcast
  会炸)。prepend=true 保证 policy 先于后注册的替换 listener。
- **值替换的 lossless 差异**:TS 的 accept-decision `value: ContentBlock[]`
  直接是渲染结果;Rust 的 `value: JsonValue` 要过工具 output schema 校验
  再 render(registry 重验语义),测试的 replacement 传 String 值而非块数组。
- **type 系统吞掉 config 校验**:TS `maxInlineBytes` 的整数/非负运行时校验
  与两个 load 期 rejection 测试在 Rust 由 `Option<u64>` 类型本身表达,
  无法等价复现——记为偏差。
- **`tools/code-dispatch-log` arm 待 code-runtime**:dispatch-log 侧与全部
  code-mode 测试(worker 运行时、慢后端反压、output-limit 捕获)留到
  dsh-code-runtime 里程碑,当前策略只注册模型侧 arm。

## 33. launch-environment + credentials seam(第 36 轮)

- **分层快照 = 纯数据 Arc**:`LaunchEnvironmentSnapshot` 无 ctx 依赖,
  `create_launch_environment_snapshot` 构造期深拷贝(后续改源表不可见),
  查找按 SOURCE_ORDER 常量序而非构造序;Windows 大小写折叠只发生在
  lookupKey(存与查两侧一致),测试用 `cfg(windows)` 双拼写断言。
- **`launchEnvironmentOf` 的服务槽**:ctx slot 用 `get_typed::<Arc<Snapshot>>`
  读取(register_service 的 Arc 值层);回退层用 `std::env::vars()` 现采——
  TS 的 `process.env` 等价。Rust 2024 里 `set_var` 是 unsafe,测试以
  SAFETY 注释 + 进程号唯一变量名规避多线程竞态。
- **credentialRef 校验 panic**:TS throw TypeError;Rust 沿用 crate 惯例
  panic(regex 编译进 OnceLock),测试用 catch_unwind 断言全部非法形态。
- **notifyUpdated 的包含分发**:TS 用 `events.dispatch('emit')` 同步迭代
  listener,Rust 无同步 listener——改为 `ctx.collect(DispatchMode::Emit,
  name, args)` 拿同一快照后逐 listener `catch_unwind(...).await`:每个
  监听器都跑;panic payload 链式 downcast(InvariantError→String→&'static
  str)识别 INVARIANT 码的聚合上抛,其余告警。MemoryCredentials 测试双走
  notify_updated 而非裸 emit,使不变式端到端可观察。
- **invariant 伴生的可观察性**:`installer().install(ctx, fail_collector)`
  直接 await 注册 listener,测试用 `ctx.collect` 抓监听器快照手动驱动,
  绕开 Rust `ctx.emit` 火忘导致的不变式 panic 不可见问题(与 workspace
  §31 同一模式)。
- **fiber 效应需要 tokio 上下文**:InvariantRegistry.register 的
  `caller.effect` 内部 `tokio::spawn`,非 tokio 的 `#[test]` 会 "no reactor
  running" panic——涉及 register 的测试都要 `#[tokio::test(current_thread)]`。
- **credentials-local 待办**:文件后端依赖 YAML 注释保留(TS 用 yaml
  Document;serde_yaml 会丢注释)、chokidar watcher(候选 notify crate)、
  withFileLock(已就绪),以及 dsh-launch-environment 三层分层——
  下一轮专项处理并记录注释保留策略。

## 34. credentials-local(第 37 轮)

- **行级 YAML 编辑替代 AST**:TS 用 yaml Document(setIn/deleteIn 保留注释与
  格式);Rust 无注释保留 YAML AST,改为"serde_yaml 严格校验 + 行级扫描
  定位 entry 区间"的双层方案:set 只替换 `KEY:` 之后到 entry 尾(含缩进
  续行的块标量/折行引号),unset 删区间+上方连续注释块(annotation),文档
  删空输出 `{}\n`。TS 四个 spec 的字节级断言全部满足;显式 `? ` 复合键
  不在行编辑范围内(记录偏差)。
- **标量序列化的隐式类型陷阱**:plain 值必须重新解析回字符串——`1`、
  `true`、`null` 在 YAML 里是隐式类型,不加引号会写坏并发场景(测试
  `keeps_both_refs` 抓到:另一个 provider reconcile 时报 "must be a
  string")。is_plain_safe 最后一道关 = `serde_yaml::from_str::<Value>`
  必须得回 String 且值相等;双引号转义 \n/\r/\t/\" 与 \xNN 控制符。
- **serde_yaml 的 duplicate key 自带检测**:0.9 对重复键直接报错——把错误
  码归一为 TS yaml 库的 `DUPLICATE_KEY` 措辞(describe_yaml_error 检查
  "duplicate"),行级扫描作兜底;空文档(纯注释)解析为 Null → 空 store
  (TS `toJS() ?? {}`)。
- **operation 队列 = tokio Mutex + runtime 约束**:TS 的 promise 链立即
  启动;Rust future 惰性——曾试 oneshot 链(头节点无人 poll 死锁)后回退
  tokio::sync::Mutex(公平 FIFO)。notify 回调线程无 tokio 上下文:回调只
  发 futures channel,debounce/consume 任务在 install 的 runtime 里
  spawn;queue_refresh 用 Handle::try_current 分派(runtime 内 spawn、
  否则 block_on 兜底)。
- **drain 语义**:closed 置位 → 关 watcher → 拿 operation 锁(等所有在飞
  操作);排队操作拿锁后二段检查 closed 报 "disposed before the queued";
  在飞写即使 closed 也照常落盘(reconcile 的 closed 短路只跳过发布)。
- **测试并行干扰**:fake watcher 全局实例表按 **canonical target 路径**
  匹配(Windows `\\?\` 前缀 trim)而非 first(),否则并行测试互相发信号;
  TS 的 eager async 调用在 Rust 要 tokio::spawn 才立即执行(drain 测试的
  in-flight 语义);写锁重试 20ms 起步,并发双 provider 测试稳定通过。
- **reader/writer/factory 三 seam**:gated writer(oneshot gate)复刻 TS
  vi.mock(atomic-write);reader seam 注入 EACCES;watcher factory 注入
  fake 实例——`install_with_seams` 承载全部注入,生产 API 只有 install。

## 35. dsh-sandbox seam(第 38 轮)

- **纯词表包零系统依赖**:sandbox seam 本体是"同一世界进程禁闭能力 seam"
  的类型/词表/编排——WIDER_MODES 静态梯、ESCALATION_TARGETS 闭集、
  ConfinedArgv 的 denial 方言 + runner 失败规则,全部纯数据;SandboxProvider
  的 confine 返回 `Result<ConfinedArgv, SandboxUnavailableError>`(TS 的
  fail-closed throw),HarnessError 承载 SANDBOX_UNAVAILABLE 码。
- **approveEscalation 的泛型通道**:TS 用 `EscalationApprover<A, C>` 结构
  泛型(不 import approval/agent 包);Rust 用 trait
  `EscalationApprover<A, C>` + `EscalationApproval<'a, A, C>` 借用 approver,
  `A = serde_json::Value` 测试桩、`C = String`;abort signal 塌缩为
  `Arc<dyn Fn() -> bool>`(全仓取消约定)。outcome 是闭枚举,TS 的
  assertNever 防御在类型层消解(测试跳过该防御用例,记录偏差)。
- **verbatim 文案保真**:全部拒绝路径逐字对齐 TS(非严格加宽、无审批服务、
  无 agent、rejected/cancelled/unavailable 各自文案、audit reason 的
  `escalate sandbox to {mode}: {justification}` 格式)——两个执行工具族
  (bash/fs)将来依赖同一文案,测试钉死。
- **canonicalPath 的保守回退**:`std::fs::canonicalize` 失败返回原拼写
  (缺根路径匹配不到任何东西直到存在——TS 同);writableRoots 在
  workspace-write 下给 workspace root + `/tmp` + `env::temp_dir()` 三根
  规范化去重(Windows 上 canonicalize 带 `\\?\` 前缀,测试两端都走
  canonicalize 比较)。
- **roots 测试内嵌**:roots.rs 的单元测试放模块内(与 TS roots.spec 同
  结构);vocabulary/escalation 合并为一个集成测试文件(13 项),TS 的三个
  spec 全数覆盖。

## 36. dsh-fs seam(第 39 轮)

- **不透明品牌 + 码表 + 抽象服务**:FsTargetKey/FsVersion 用 Branded
  (无校验构造——TS 同);FsError 13 码枚举 + HarnessError 承载 +
  cause 链(source 转发);FileSystem trait 14 原语,AbortSignal 塌缩为
  `AbortPredicate = Arc<dyn Fn() -> bool>`,streamText 为
  `BoxStream<'static, Result<String, FsError>>`(TS AsyncIterable 的 Rust 形)。
- **ResolveOptions 的 Debug 陷阱**:含取消谓词的 options 结构不能 derive
  Debug(Arc<dyn Fn> 无 Debug)——只 derive Clone/Default。
- **internal/dispatch 预钩**:fs 不变式伴生挂 `internal/dispatch`
  (global)——Rust 的 collect 对每个非 internal 事件内联 block_on 跑预钩
  并 catch_unwind 吞 panic;校验逻辑抽成纯函数 `check_dispatch(event,
  args, fail)` 单测全覆盖(空 targetKey/displayPath/version 三种拒绝),
  与 workspace/credentials 伴生同一可观察性模式。
- **服务双注册 panic**:TS 的 "第二个实现加载 throw" 在 Rust 是
  register_service 的重复注册 panic;fiber dispose 移除服务不可表达,
  由重复注册 panic + disposer 契约覆盖。
- **测试遮蔽坑**:模块级 `target()` 构造器与 trait 方法参数
  `target: &FsTarget` 同名遮蔽,结构体字段初始化里调用构造函数会
  E0618——改名 mk_target 规避;闭包 fail 通道用 Arc<Mutex> 保证 Fn 而非
  FnMut。

## 37. dsh-fs-local(第 40 轮)

- **realpath 身份 + 祖先回退**:resolveLocalTarget 先 canonicalize 全路径,
  失败时逐级上移找最近存在祖先并拼回缺失段(symlink 别名共享 key、
  创建前后 key 稳定);"父段是文件"在 Windows 上报 NotFound——显式
  metadata 检查恢复 TS 的 ENOTDIR 语义(credentials 同款修复)。
- **versionOf 平台近似**:Unix `dev:ino:size:mtimeNs:ctimeNs` 与 TS 逐字节
  同;Windows 用 size/modified/created(Rust std 无 file index)——记录偏差。
- **跨块 UTF-8 流**:TS TextDecoder({fatal,stream}) 的等价物是
  `std::str::from_utf8` 逐块 + error_len None 时保留不完整尾部字节拼入
  下一块(≤3 字节);try_unfold 状态机——闭包必须 move 捕获 + 每次调用
  clone(signal/display_path),否则 FnMut 逃逸借用编译失败(踩坑)。
- **孤儿规则**:`impl From<io::Error> for FsError` 违反孤儿规则(FsError
  在 dsh-fs)——自由函数 `io_to_fs_error` + 全量替换 `?`/`.into()` 调用点。
- **block_in_place 只在多线程 runtime 可用**:current_thread 测试下
  `tokio::task::block_in_place` panic;硬链接直接用同步 std 调用(快且
  全 runtime 可用)。
- **staging 清理不得吞错误码**:TS removeStagingDirOrThrow 保留原始错误
  (FS_NOT_OBSERVED 等)——初版把 originalError 重包成 FsIoError 丢了码,
  守卫创建测试抓到;改为清理成功返回原 FsError、清理失败才合成双失败文案。
- **per-targetKey 锁**:HashMap<String, Arc<tokio::sync::Mutex<()>>>,锁内
  read→guard→write 串行——并发守卫写一胜一 stale(TS 的 promise 链等价);
  锁条目不回收(key 数有界,记录微偏差)。
- **win32 简化边界**:copyFileDaclWin32 no-op、replaceFileWin32 删后 rename
  (内容原子、ACL 继承自目录)——真实 GetFileSecurityW/ReplaceFileW FFI 随
  sandbox-windows-acl 里程碑;fsio 的注入 seam(copyFileDacl/replaceFile/
  linkFile/inspect/removeStaging)保留供测试钉编排。

## 38. dsh-fs-observation-policy(第 41 轮)

- **WeakMap 对象身份 → opaque OwnerKey**:TS 用 `WeakMap<object, …>` 按
  session 对象身份隔离;Rust 无跨包结构性收窄——工具层构造最小视图
  `FsObservationActorHandle { session_key: Option<usize> }`(session 的
  Arc 指针)随 `fs/*` 事件传递,gate 用 `HashMap<OwnerKey, HashMap<key,
  observation>>`。policy 不 import tools/agent/session(与 TS 同约束)。
- **waterfall 无错误通道**:TS 的 edit-intent 监听器 reject 让 waterfall
  promise 拒绝;Rust waterfall 返回 ArcValue 无 Result——编辑拒绝经
  `panic_any(FsError)` 传播,调用方 catch_unwind 拿结构化错误(测试同款
  断言)。write-intent 的 Ok 值直接 arc(intent)。
- **first-wins 语义**:监听器**不调 next()** 直接返回值占位单槽——后续
  监听器不达(测试钉住);注册顺序即优先级(TS 的约定同)。
- **emit 与 settle**:fs/observed 是 fire-and-forget 记录——测试
  settle = 8 次 yield + 10ms sleep 保证 spawn 任务写完再断言;fresh-
  plugin-after-dispose 用例第二次 dispatch 经新 gate 拒绝(FS_NOT_OBSERVED),
  断言用 catch_unwind 而非"无监听器直落 fallback"(dispose 后重装插件
  是新 gate)。
- **HMR dispose 聚合**:apply 返回 disposer = gate.clear + 三个 listener
  disposer 依序调用(TS 的 fiber dispose 可观察面)。

## 39. sandbox-policy + fs-sandbox(第 42 轮)

- **包装后端的不注册构造**:LocalFileSystem 拆 `build`(构造+校验,不注册)
  与 `install`(build + register "fs");SandboxedFileSystem 经 build 持有
  local 委托全部存储机制,自己注册擦除句柄——避免同服务名双注册 panic。
- **sandboxPolicy 服务解析**:defaultMode/workspaceRoot(绝对化+canonical)
  + resolve 的优先级 审批 mode > 会话 sandbox/mode fold > 默认;会话 cwd
  是 workspace 边界;会话套件 effective_sandbox_mode 逆扫 fold、
  set_sandbox_mode append 恰一条事件。systemPrompt context 注入跳过
  (Rust assemble 无 agent 字段)——记录偏差。
- **每调用策略栅栏**:checked_target 现时重解析 displayPath(fresh
  targetKey)后与 writableRoots 逐根 is_path_under——TOCTOU 方向钉死
  (stale targetKey 不写);拒绝是结构化 FS_SANDBOX_DENIED。
- **Windows 身份回退用 canonicalize 等价**:std 无 file index,metadata
  (size+mtime+ctime)近似会让两个同刻创建的空目录碰撞(false positive);
  改用 canonicalize 字符串等价(8.3 别名/大小写归一)——Unix 保持
  dev:ino。
- **测试根必须逃出 temp 授予**:workspace 建在 temp 下时 `..` 逃逸落在
  temp 授予内(合法);TS 根在 HOME 下同理——Rust 测试根建在 temp 的父
  目录下。
- **get_typed 双 Arc 陷阱**:服务注册存 Arc<S>,读时 T 写
  `Arc<S>`(不是 Arc<Arc<S>>)——写双 Arc 会得三层 Arc 类型错配。
- **会话测试需 runtime**:SessionStore 的 fiber effect 内部 tokio::spawn,
  普通 #[test] 会 no-reactor panic——session 相关测试全部
  `#[tokio::test(current_thread)]`。

## 40. dsh-subprocess seam(第 43 轮)

- **纯词表 + 抽象三法**:subprocess seam 是 spawn 词汇与三个抽象原语
  (resolveExecutable/spawn/spawnTerminal)的宿主;Rust 把 Node 流塌缩为
  tokio 字节流(terminal output 为 `BoxStream<'static, Vec<u8>>`,piped
  stdio 为 boxed AsyncRead/AsyncWrite),AbortSignal 为取消谓词——与全仓
  约定一致。
- **explicit-env 墓碑**:TS 的 `env: { name?: string }` 墓碑语义(undefined
  删除环境条目)在 Rust 是 `Option<Vec<(String, Option<String>)>>`
  (None 值 = 删除)。
- **scrubbedParentEnv 双擦洗**:凭据形(KEY/PASSWORD/SECRET/TOKEN,大小写
  不敏感)与 `DSH_` 前缀(先 to_uppercase 再 starts_with)都被滤除;PATH/
  HOME 存活;显式 env 层在擦洗后合并故刻意转发可幸存。测试用进程唯一
  探测名(set_var unsafe 注释)。
- **含取消谓词的 spec 不能 derive Debug/PartialEq**(Arc<dyn Fn> 两者皆
  无)——SpawnSpec/TerminalSpawnSpec 只 derive Clone,与 credentials 的
  options 同类坑。
- **服务擦除注册 + 双注册 panic**:同 spill/fs 模式——`impl Service for
  dyn SubprocessRuntime`;测试桩 StubHandle 直译 TS 的 hand-built handle。

## 41. dsh-subprocess-local(第 44 轮)

- **全 spawn 路径的共享状态**:`TreeShared`(observed/settled/
  child_exited 原子 + grace_timer + observer OnceCell<Shared<BoxFuture>>)
  经 Arc 共享,**绝不能 derive Clone**——手动 Clone 会复制原子位,terminate
  一侧的翻转对 tree_alive 另一侧不可见(本轮踩过);一切 clone 走 Arc。
- **Rust future 惰性 vs TS 事件驱动**:TS 的 `done` promise 由子进程自身
  exit/close 事件驱动,无需消费者;Rust future 不被 poll 就不跑——`done`
  必须**自 spawn 起 tokio::spawn 驱动**。否则 wait_for_exit 的树存活观察
  (Windows 依赖 child_exited 标志,只有 done 任务在 wait() 后置位)与
  wait_for_exit 互相等待,死锁(本轮实测:Windows 上 60s 挂死,即子进程
  自然睡醒)。
- **abort 谓词的 15ms 轮询**:TS AbortSignal 是事件目标(零延迟、once:true
  监听器);Rust 谓词只能轮询。`abort_reacted` 标志必须**在谓词返回 true
  之后**才置位——先 swap 再判谓词会在第一次 false 轮询后就永久锁死反应
  (本轮踩过)。
- **tokio Child 所有权切分**:stdin/stdout/stderr 用 take() 取出后,wait()
  只留进程收割;collect 读者任务必须**与子进程并发**读(否则管道缓冲
  64KiB 写满会反压卡死子进程)。读者分两相:阶段一 select(read vs
  child-exit watch)随子进程退出破出;阶段二有界排空(grace 上限),超时
  drop 流即关端——结算先 join 读者再 seal,保证 seal 后无 chunk 落盘。
- **树存活探测的平台语义**:POSIX `libc::kill(-pid, 0)`(ESRCH=死、
  EPERM=活、其余回退 child_exited;settled 后 Linux 加 /proc 活成员扫描
  防僵尸组);Windows 无组探测 → child_exited 标志。`detached` = tokio
  `process_group(0)`;tree 信号 = kill(-pid, sig) 失败回退 kill(pid, sig)。
- **exit→close 边界塌缩**:pipe 模式 Rust 无法观察"所有写端已关"(读端归
  调用者),done 在直系子进程退出即结算(TS 对后代持管的 pipe 会等到
  drain timer)——记录偏差;collect 模式仍由读者两相 + grace 精确还原。
- **effect 只能经 plugin fiber 排空**:`ctx.effect` 挂到 `self.fiber`;
  根 fiber 的 `dispose_self` 是 None(dispose 空转)。服务释放测试必须
  `ctx.plugin(...)` 建子 fiber,在插件 apply 里 install,fiber.dispose()
  才跑 disposer(dispose_managed_processes 终止整树)。
- **spawnTerminal 桩**:node-pty 无 Rust 等价,留待 PTY 里程碑;process-
  inspector(/proc syscall 检测)随 terminal 一起后置,仅移植
  linuxProcessGroupHasLiveMembers 的 /proc stat 部分(parse_proc_stat
  括号 comm 处理)。
- **测试子进程 helper**:tests 无法传参给 libtest 二进制(过滤参数会 0
  匹配退出 101),改用 `src/bin/child.rs` 真实 bin + `env!("CARGO_BIN_EXE_child")`
  脚本化 stdio/exit/signal/trap 行为——全平台确定性。

## 42. dsh-terminal(第 45 轮)

- **TS async 函数的 sync 前缀必须落在 Rust 调用点**:Rust future 惰性,
  但 TS `async fn` 在首次 await 前同步执行——`spawn`(owner 栅栏/名称
  预留/spawn 预留)、`kill`(closing fence 安装)、`dispose_owned`/
  `dispose_all`(abort 循环 + disposing 置位)、`close_records`(全部 fence
  同步安装)都拆成「调用点执行 sync 前缀 + 返回 BoxFuture」;否则并发
  closeRecords 的 join 语义、`disposing` 即时可见、名称预留即时冲突全部
  失效。
- **match 守卫的临时生命周期死锁**:`match record.closing.lock().clone() {`
  的 MutexGuard 临时存活到整个 match 结束——`None` 分支内
  `install_closing` 再锁同一 parking_lot Mutex 即死锁(不可重入)。必须先
  `let existing = { lock().clone() };` 出块再 match(本轮最隐蔽的 bug)。
- **Shared future 无指针身份 → 代数守卫**:TS `record.closing === closing`
  身份比较;`futures::future::Shared` 不暴露内部 Arc 指针,改用
  `close_generation: AtomicU64` 每安装递增,错误路径仅在代数未变时清
  fence(更晚的并发 retry 不覆盖)。
- **parking_lot guard 跨 await 使 future 非 Send**:`if let Some(x) =
  mutex.lock().clone() { ...await... }` 的 scrutinee guard 同样存活整个
  if-let 体——先 `let x = { lock().clone() };` 出块再 await,否则
  `BoxFuture<'static>`(要求 Send)编译失败。
- **'static future 与 &self 接收者**:trait 异步方法用 `&self`(Arc<Self>
  接收者会破坏 dyn 兼容性 E0038);实现者把可变状态全部 Arc 化
  (`Arc<Mutex<...>>`/`Arc<AtomicBool>`),future 内 clone Arc 移入——
  TS 的 `this` 闭包等义。
- **agent 注册是异步 effect**:`AgentRegistry::register` 经 `caller.effect`
  的 spawned 执行体 enter+announce——测试必须 `yield_until(|| get(id).is_some())`
  等注册生效;is_live_owner 用 `Arc::ptr_eq` 比对 `registry.get(id)` 与
  owner 句柄(TS 对象身份)。
- **fiber dispose 的排空链**:根 fiber 的 `dispose_self` 是 None(dispose
  空转)——服务释放测试必须经 `ctx.plugin(...)` 插件 fiber;fiber 卸载依序
  跑 disposables(服务反注册 disposer、effect wrapper、owner cleanup
  disposer),整个链路是 async drain。
- **spawn 失败路径的释放必须覆盖所有分支**:TS `finally` 在 Rust 没有
  等价物——聚合错误分支若在 `release_spawn` 前提前 return,reservation
  永不 settle,后续 `dispose_all` 的 `await_pending_cleanup` 永久挂起
  (本轮第二个挂死点);重排为「算 final_failure → release → Err」。
- **微任务 vs 任务调度**:TS `done.then(clear)` 的微任务先于 await 续体;
  Rust 的 clear 是 spawned 任务,`await done` 后 start_send 可能撞
  SEND_ACTIVE——测试在 next start_send 前 `yield_now()`(语义等价适配)。
- **错误通道**:coded `TerminalError{message,code}` + `TerminalFailure`
  枚举(Coded/Plain/Aborted/Aggregate)统一服务面;后端 spawn 错误带可选
  cleanup_error(`TerminalBackendSpawnError`,TS AggregateError 塌缩);
  send `done` 拒绝塌缩为 panic(全仓 panic 通道约定);AbortSignal 塌缩为
  谓词 `TerminalAbort`(repo 范围惯例)。
- **oneshot 单发 + Fn 闭包**:tokio oneshot Sender/Receiver 不可 Clone——
  测试门(gate/started/disposal 通道)一律 `Mutex<Option<...>>` 槽位 +
  `lock().take()`,FnBackend 闭包捕获 Arc 每次 clone。

## 43. dsh-shell + dsh-bash-local(第 46 轮)

- **seam 词汇跨包锚定**:`DshEnvironment`/`DshEnvironmentKey`(TS
  `${DSH_ENV_PREFIX}${string}` 模板字面量)先补入 dsh-subprocess 词表,
  dsh-shell 再 pub-use 转发——bash 消费者保持单导入根;settings 命名空间
  常量(`SHELL_SETTINGS_NAMESPACE`)用 `OnceLock<SettingsNamespace>` +
  访问器(settings_namespace 返回 Result,Branded::new 非 const)。
- **擦除注册 + 具体句柄双通道**:`ctx.shell` 注册 `Arc<dyn ShellExecutor>`
  (seam 契约),但 settings/executor 测试需要具体 `LocalBashExecutor`
  (config 访问器)——`get_typed::<Arc<具体>>` 对擦除 Arc 必然失败;测试用
  installer 返回值经插件槽位 `Mutex<Option<Arc<...>>>` 传递(TS 的
  `ctx.shell as LocalBashExecutor` 塌缩)。
- **deadline 融合信号与谓词桥接**:dsh-timeout 的 `deadline(upstream,
  timeout_ms, code)` 以 DeadlineSignal(内部 Arc)为上游,而 shell 的 abort
  是谓词——桥 = `DeadlineSignal::never()` + 15ms 轮询任务在谓词触发时
  `fused.cancel(None)`;fused 信号 `mem::replace` 出 Deadline 存入
  `Arc<DeadlineSignal>` 供子进程谓词克隆(DeadlineSignal 无公开 Clone,
  Deadline 字段置换后仍保留 timers 用于 Drop 清理);run 结算后 abort
  轮询任务(防谓词永不触发的泄漏)。
- **timeout/abort 首因分类**:`timeout_of(fused.reason(), "BASH_TIMEOUT")`
  判定 timedOut;`fused.is_cancelled() && !timedOut` 判定 aborted——单融合
  期限的互斥首因(Timer 先取消带 reason,谓词先取消无 reason)。
- **后台进程 done 必须自主驱动**:bash ShellProcess 的 done 是派生的
  Shared future——`tokio::spawn(driven.await)` 自 start 起驱动(seam 契约
  done 无消费者也结算;与 subprocess-local 同款教训)。
- **onProcessDone 钩子注入化**:TS protected 方法(Rust 无子类化)塌缩为
  `set_on_process_done(Arc<dyn Fn(&BashProcessFacts, String, bool,
  Option<String>)>)`;facts 携带可盖章的 sandbox 槽(供 bash-sandbox 包装器)。
- **WSL bash 启动器语义限制**(Windows):WSL 报告 `/mnt/x` 路径、不转发
  Windows 环境变量、引号参数载荷损坏——POSIX 路径/env/引号断言
  cfg(unix) 门控;引用免费子集(echo/true/cat/重定向)全平台;`bash_available`
  探针真实 spawn 一次 bash 预热(并发首启抖动不进计时测试)。
- **设置段 wiring 时序**:install_settings_section 经 ctx.inject fiber
  异步挂接——测试 await `executor.ready()`(wiring settle);供应商脱落回退
  (fallback disposer → set_source(entry))与命名空间释放(executor fiber
  dispose → scope 反注册)经 provider 的 describe() 观测。

## 44. dsh-code-runtime(第 47 轮)

- **可移植标识符排除集 = 一仓共享契约**:四个集合(RESERVED_BINDING_
  GLOBALS / RESERVED_ERROR_MEMBERS / DUNDER_MEMBER / PORTABLE_RESERVED_
  WORDS)从 TS 常量逐项搬入 Rust `OnceLock<HashSet<&'static str>>` +
  正则访问器——后端 import 共享成员而非各自重声明,保证一个后端合法的
  命名空间列表在全部后端合法(ECMAScript∪Python 并集,含 Python 软关键
  字 type/_);dunder 正则 `^__.+__$` 空中间(`__`/`____`)不匹配。
- **CodeJsonValue = serde_json::Value**:TS 结构型 JSON 类型与全仓
  lossless JSON 形状同一;绑定函数 `Arc<dyn Fn(Value) -> BoxFuture<Value>>`
  ——拒绝塌缩为 panic(全仓拒绝通道);abort 谓词无 reason,预中止 message
  塌缩 "aborted"(文档偏差)。
- **worker-thread 后端推迟**:TS bootstrap 以 Node worker_threads +
  TypeScript 编译运行程序——Rust 需嵌入 JS/TS 运行时(boa/deno_core 级
  依赖),不属于 seam 范围;code-runtime 只落地 Service Definition +
  排除集契约,后端留待里程碑(与 PTY 同类后置)。
- **seam 测试的最小桩模式**:StubRuntime 记录请求/脚本化结果/逐声明序
  调用绑定——trait 方法返回 'static future 需 &self 借用逃逸时,把可变
  字段 Arc 化后在 async 块内 clone(与 terminal/bash 同款教训)。

## 45. dsh-jobs(第 48 轮)

- **声明合并 union 塌缩为 String**:TS 的 `JobKindMap` 声明合并(插件扩展
  union)在运行时本就是不透明 id 命名空间——Rust `kind: String`,注册表
  永不检查成员(文档注明)。
- **同步 throw → Result 通道**:get/read/kill 的契约误用(未知/异主)在
  TS 同步 throw——Rust `Result<_, String>`;`JobHooks.done` 契约"绝不
  reject"→ future 直接产出 JobOutcome;`JobStart.run` 的 throw → panic
  (全仓 throw 等价)。
- **快照不变式的纯校验器**:validate_snapshot(snapshot, owner, &dyn
  Fn(&str)) 纯函数 + jobs-inject 安装器(install 内 get_typed jobs →
  list(None) 校验现有 unowned 记录 → on_job_done 订阅终态快照,disposer
  随注册 context 的 fiber 作用域清理故丢弃返回值)——u64 的积分/非负性
  是类型事实(TS 的 Number.isSafeInteger 检查塌缩);测试用
  `Mutex<Vec<String>>` 捕获 fail 消息(fail 是 Fn 非 FnMut)。
- **抽象 seam 挂载栅栏 = 编译期事实**:TS `new.target === JobRegistry`
  运行时 throw——Rust trait 无运行时实例,组合行命名本包不可能注册空
  `ctx.jobs`(与 code-runtime/terminal 同类)。

## 46. dsh-jobs-local(第 48 轮)

- **Notify 不存 permit → settled 标志二件套**:TS 的 settled promise 有值
  语义(resolve 后晚到的 await 立即返回);`Notify::notify_waiters` 不存
  permit,晚注册的 waiter 永久挂起(本轮两个挂死测试同因)——TrackedJob
  携带 `settled_flag: AtomicBool` + `Notify`,所有等待者(spurious-safe
  循环 `loop { if flag { return } notified().await }`)先查标志。
- **&self 方法内 spawn 的自引用**:start 的 done 驱动任务是 'static
  spawned——`self.clone()` 在 &self 上克隆的是引用;结构体加
  `self_arc: OnceLock<Arc<Self>>`(install 时 set),spawn 里
  `self.self_arc.get().unwrap().clone()`。
- **producer done 的 panic 塌缩**:TS 的 rejection 分支(producer contract
  violation → failed)塌缩为 `catch_unwind(hooks.done().await)` → render_
  panic → settle(failed)——teardown 抛错 cancel 同路径(possible orphan
  detail)。
- **wait 的 sync 前缀 + 融合期限**:TS async fn 的同步段(校验/waiter
  注册)落在调用点;live 时 waiter +1、deadline(TASK_WAIT_TIMEOUT)+
  谓词轮询桥接(terminal/bash 同款),超时返回快照、中止 Err("wait
  aborted")、结算/超时标记 reported;counted 布尔保证终态路径不减计数。
- **teardown 顺序的 TS 语义**:cancelForTeardown 先 reported=true 再
  cancel(抛错 → settle(failed))——取消先于状态转移;disposeAll 先
  listenersClosed(结算不再通知 done 监听)再取消/等待/清 store/
  逐 owner notifyChanged/分离 owner cleanup effects;disposeOwned 等
  producer 结算后删除记录(测试必须先让 producer 释放)。
- **PowerShell 行插入教训**:`List<string>.Insert(i, string[])` 会把整
  数组 join 成一行(测试被毁成一行注释);批量改测试文件必须验证插入
  结果的换行。

## 47. dsh-goal(第 49 轮)

- **缓存键稳定性陷阱**:以 `session.events()` 的快照 Arc 指针做 WeakMap
  键——append 后快照失效重建,**指针漂移**导致缓存键失效、reseed 把
  activation 重置(armed 变 disarmed,resume 检查失效)——改为 agent id
  字符串键(goal 域一切访问都经精确 live agent,session id 恒稳)。
- **借用安全的 clone-out/write-back**:TS 的 WeakMap 就地可变访问在
  Rust 拆成「lock → entry/seed+sync → clone GoalCache → 变异(期望借用
  检查) → commit(append+sync+view+emit 全走 &mut cache) → insert 回
  写」——避免 &mut HashMap 与 &mut 元素的双重借用(E0499)。
- **pending-activation 跨 append 边界**:commit 里 `pending = (session.
  seq(), activation)` → append → sync 按 event.seq 匹配恢复 armed 否则
  disarmed——进程本地 armed 意图与持久日志的精确对齐。
- **goal/change 全量快照 + 严格解码**:每个变更携带完整后置状态
  (last-wins 投影折叠),strict 解码器校验字段集精确、规范化
  (objective trim/blockReason kebab)、revision 递增、计数器与时间戳
  保留(updatedAt 单调)——投影级 fold 对畸形事件返回原状态,严格 fold
  fail-loud。
- **scoped emit 复用**:goal/changed 经 dsh-agent 的
  `emit_agent_event(ctx, agent, name, build)`(agent scope carrier +
  包含式监听)——与 registry 的 agent/created 同机制;监听 payload 用
  downcast 读取。
- **@Remote 塌缩**:TS 的 @Remote('edit') 注解依赖 typert 远程运行时
  (未移植)——方法保持普通同步方法,remoteExportCreate 保留为普通
  方法,偏差记录。

## 48. native-command + session-checkpoint-policy(第 50 轮)

- **waterfall 监听器的 NextFn 单次语义**:`NextFn(Arc<Mutex<Option<
  FnOnce>>>)` 不 Clone、call 单次(二次调用返回 dummy)——监听器闭包必须
  把 args 整体 move 进 async 块,在块内取 `downcast_arc::<NextFn>(&args[1])`
  的引用再 await call。
- **llm/stream 的 cell 双 Arc**:args[0] = `arc(Arc<Mutex<GenerateOptions>>)`
  → `downcast_arc::<Arc<Mutex<GenerateOptions>>>(&value)` 取
  `Arc<Arc<...>>` 后 `as_ref().clone()`——llm invariant 同款。
- **fail-closed 流包装用 flat_map 分流**:`once(flush).flat_map(|r| match
  r { Ok → next(options), Err → 单 Finish(Error) 块流 })`——chain 写法在
  flush 失败后仍会产下游块(TS generator throw 提前终止的等价是分流
  而非链式);首块前 flush 由流惰性保证。
- **CREATE_NO_WINDOW**:TS `windowsHide: true` = Windows
  `creation_flags(0x0800_0000)`(cfg(windows));abort 谓词 15ms 轮询 +
  `start_kill()` 后 `ABORT_ERR` 码(Node 的 error.code 塌缩)。
- **no-shell 执行器的临时值陷阱**:tokio::spawn 要求 'static——测试中
  `run_native_command(&child, &args(&[...]), ...)` 的临时 Vec 借用逃逸;
  绑定命名变量或整块 `async move` 拥有。











## 49. time-context(第 51 轮)

- **ICU 级时区规范化**:TS `Intl.DateTimeFormat(...).resolvedOptions().timeZone`
  会解析 tzdb 链接(US/Eastern→America/New_York)并让 CLDR 别名塌缩
  (Etc/UTC→UTC)。chrono-tz 0.10 与 jiff 都保留链接拼写,不能直接当
  规范器——方案:jiff `TimeZone::get` 只做存在性+偏移,规范判定用内置
  `src/tz_links.rs`(IANA tzdata 2026c backward+etcetera 全部 257 条
  `Link` 行生成,与 jiff-tzdb 0.1.8 内置库同版本)+ CLDR Etc/UTC 家族
  覆盖表;链接命中/别名塌缩即 NotCanonical。表格线性扫描(257 项),生成
  命令见文件头。
- **系统时区**:`iana_time_zone::get_timezone()`(chrono 的 clock 特性已把
  它拉进依赖图);格式化为 jiff strftime `%Y-%m-%dT%H:%M:%S%:z`(RFC3339
  带冒号偏移,等价 TS longOffset slice(3))。
- **`internal/dispatch` 内联钩子的锁重入**:`Session::append` 在持有
  session 状态锁时内联 block_on 跑 internal/dispatch;监听器里调
  `session.events()` 会死锁(parking_lot 不可重入)。伴生必须自维护
  per-session 增量历史(按 session.id 键控 HashMap,安装/session-created
  时播种,dispatch 时校验后 push)——与 session-invariant 伴生的
  trace 缓存同构。(dsh-goal 的 dispatch 监听器读 args[0]=mode 字符串
  判事件名,staging 分支永远不触发,侥幸避开了这个死锁——是潜在偏差,
  待轮次修复。)
- **prepend 瀑布监听器**:`EventOptions::default().prepend(true)`;
  `apply()` 返回的 disposer 只在首次运行时注册(OnceLock 幂等),Plugin
  包装里 `(disposer)().await` 后 `ctx.on` 自动把移除 disposer 挂到
  fiber 上,fiber.dispose() 即移除。
- **TS 返回 `[...decision.messages, reading]`**:进入+提议消息的并集只用于
  浏览器时区派生(requestMessages),返回的决策消息是下游原消息 + 读数,
  不是收集全集——先 clone 再合并。
- **MessageSource::User 合并增强**:TS 的 `user: {kind:'user'}` 由 apiproxy
  包 merge 增强出可选 rpcId/clientTimeZone;Rust 直接建模为
  `User { rpc_id, client_time_zone }`(serde skip-if-None,线格式不变)。

## 50. guard 双策略(第 52 轮)

- **tools/execute 瀑布的结果是双 Arc**:运行期
  `downcast_arc::<Arc<ToolExecutionResult>>(&value)` 取回——监听器返回
  `arc(Arc::new(result))`,不能返回单层 arc(result)。post-execute 是单层
  `arc(PostToolDecision)`。
- **dispatch_tool_body 总会融合 body 信号**:`wrapper_signal() || caller()`
  换入 exec.signal——策略测试断言 body「看到调用方原信号」在 Rust 不成立,
  只能断言行为(信号随上游 abort 变 true)。超时策略的还原发生在 next()
  之后,post-execute 观察到的仍是还原后的上游谓词。
- **scope 过滤器放行未打标监听器**:`scope_target(None, key)` 的 filter 对
  `scope_of(ctx) == None` 的监听器 ctx 直接 true——宿主 ctx 上的策略
  监听器对所有 scoped 分派可见(与 TS 宿主级监听器语义一致)。
- **全局工具解析**:TS `tools.get(name, undefined)` 在直调(无 agent)时仍
  解析全局注册工具;Rust 必须 `tools.get(&name, agent.map(scope_key))`
  传 `None` 而非短路——这是 timeout-policy 直调用例的关键。
- **JSON.stringify 整数格式**:serde_json 的 `Number::as_i64` 只覆盖原生
  整型存储,`from_f64(1.0)` 返回 None——整型折叠需另判
  `as_f64().fract()==0`(repeat-tool-reminder 的 canonical key 依赖)。
- **策略插件的 disposer 惯例**:`apply()` 返回首跑注册(OnceLock 幂等)的
  make_disposer;Plugin 包装在 apply 里 `(disposer)().await` 后由 ctx.on
  把移除 disposer 挂到 fiber 上。

## 51. tool-todo + attachment(第 54–55 轮)

- **两类 apply 约定**:立即注册型(time-context/guard/tool-todo)——
  `apply()` 同步注册、返回「移除」disposer,测试切勿先跑 disposer;
  惰性安装型——返回「安装」disposer(OnceLock 幂等)。按 TS 同步性
  判断:TS apply 里直接 ctx.tools.register/ctx.on = 立即注册型。
- **ctx.inject 返回 Fiber 而非 Disposer**:卸载 = `fiber.dispose().await`;
  回调错误类型是 PluginError(String 需 anyhow 包装)。
- **Rust 工具运行期不校验输入参数 schema**:dispatch 前无
  validate_args——工具体内用共享 `validate_json_schema_value` 引擎自查
  (同一拒绝面,晚一个阶段;已记录偏差)。
- **无 agent 直调也要解析全局工具**:`tools.get(name, agent.map(scope_key))`
  传 None;ScopedLayers::merge 恒含 global 层。
- **抽象服务注册**:`impl Service for dyn Trait` + 具体后端
  `let erased: Arc<dyn Trait> = backend.clone(); ctx.register_service(erased)`
  (fs 同款)。
- **光栅解码**:image crate 替代 sharp——探测 = ImageReader 头解码,
  准入 = 全解码;JPEG 编码测试需 RGB(无 alpha)。
- **硬链接去重的 EEXIST 分支**:冲突对象读回 + digest 比对(损坏 →
  ATTACHMENT_CORRUPT;读失败 → ATTACHMENT_WRITE_FAILED)——目录占位
  的「非预期发布失败」经此路径落 WRITE_FAILED。

## 52. session-query(第 56 轮)

- **抽象服务读取的两种注册风格**:get_typed 精确 downcast 存储类型——
  具体类型注册(如 jsonl 的 `Arc<JsonlSessionPersistence>`)无法按
  `Arc<dyn Trait>` 读取;擦除风格需要 `impl Service for dyn Trait`
  (session-persistence 本轮补上)+ `register_service(erased)`。宿主装配
  时要给具体后端注册擦除别名。
- **可选服务注入是异步的**:ctx.inject 的 fiber 激活后才跑回调——测试
  与消费方在依赖时序时必须等待绑定完成(sleep/yield)。
- **`use` 遮蔽 glob re-export**:lib.rs 里显式 `use crate::types::{X}` 会
  压过 `pub use crate::types::*` 把 X 变私有——要么全 pub use,要么给
  glob 命名冲突项单独 pub use 别名。
- **regex `\s` 不含换行之外的语义差异**:Rust regex 的 `\s` 含 \n;TS
  RegExp 的 `\s` 同样——但 TS 文本过滤器不锚定(test() 任意位置),Rust
  若加 `^...$` 会拒绝前缀文本,勿锚定。
- **fold_session_title 契约**:需要 data.title + messageSeqs 数组 +
  source(serde 反序列化 SessionTitleSource)——只给 title/source 返回
  None。
- **traceSession 排序**:child 列表按 createdAt 升序 + id 字典序;ancestor
  从近到远;complete 时 root = 最远祖先(无祖先则 target)。
- **项目投影闭包生命周期**:`Arc<dyn for<'a> Fn(&LogicalSessionSource<'a>) -> Value>`
  (HRTB)——Value 需 Clone + Send + 'static。

## 53. session-reference + commands(第 57–58 轮)

- **URI canonical 编码**:TS `Buffer.from(JSON.stringify(id))`——payload 是
  JSON 带引号字符串的 base64url,不是裸串;解码后必须重编码比对才算
  canonical。
- **预算保留双循环**:先丢最老非 checkpoint 非最新消息(计数+字节),
  再对最长消息 head/tail 二分截断(TextRetainer::HeadTail + Exact
  omitted)——固定部分放不下时整体 None(TS undefined)。
- **commands 注册的 caller ctx**:TS `this.ctx` 重绑到调用方——Rust
  register(caller, definition) 显式传;ScopedLayers::effect 同步执行
  变更,on_change 通知用 ctx.emit(每监听器已隔离)。
- **Rust regex 无 look-around**:`(?=$|[\t\n\r ])` 边界改为手动检查
  `line[end..]` 首字符。
- **handler 模型**:TS 同步/异步 throw → Rust async Result 闭包;abort
  用 tokio::select! 轮询谓词(15ms)。
- **commands/change 通知非否决**:TS 逐个 catch;Rust ctx.emit 的
  per-listener spawn 隔离已内置。

## 54. host 主程序 + feedback + compaction(第 59–62 轮)

- **MessageSource 合并增强的第三处**:Plugin 变体扩展
  compactionId/sourceCommandId(compaction checkpoint 来源)——所有
  Plugin 字面量构造点需机械补齐两个 None 字段,模式匹配补 `..`;
  time-context 不变式「恰好四个键」检查改为显式判 None。
- **可启动主程序**:crates/host/dsh-host 组合 sessions/agents/
  systemPrompt/tools/invariants + 启动报告,`cargo run -p dsh-host`
  exit 0——目标「可启动运行」的首个产物;webserver/apiproxy 后续叠加。
- **message-feedback 的 storage 侧车**:DomainFacility.open 在安装期
  block_on(projection-cache 同款);Domain.close future 非 Send →
  dispose 同步 block_on(偏差);KvTable::get 同步、put 异步;
  session-persistence 的 listSnapshots 做存在性权威。
- **tool-pairing 平衡缓存**:按 session.id 键控(events() 快照 Arc
  指针随 append 漂移,不能当键);增量折叠需先验证未见尾巴再写缓存
  (坏 append 不留半态)。
- **compaction 括号不变式**:start 持锁到 end;summary/checkpoint 需
  id+sourceCommandId 与 open 匹配;turn 边界不得穿越 open 压缩;
  session/end-seed 让未配对 start 变陈旧(继承孤儿)。
- **可启动验证的负载抖动**:bash-local 与 credentials-local 各有一个
  并发时序测试在全量并行下偶发失败(单跑必过),已列入已知项。

## 55. user-questions/tool-ask-user/user-approval(第 63–64 轮)

- **provider 生命周期的 Arc<Mutex> 包裹**:parking_lot Mutex 不可克隆,
  `register_provider` 返回的 disposer 要持有 provider 槽做 take,必须
  把 Mutex 包进 Arc;disposer 体用 `cordis::make_disposer` + async
  闭包(user-questions 同款单槽 first-wins)。
- **ask 校验梯的顺序**:EMPTY_QUESTIONS → NO_PROVIDER 前先做信号中止
  (ASK_ABORTED),agent 传入时 Agents 注册表 liveness(`Arc::ptr_eq`
  精确实例)+ roots 校验(DELEGATED_CALLER)。
- **approval 的服务安装 + 系统提示注入**:TS 构造函数里
  `ctx.inject(['systemPrompt'], scope => scope.systemPrompt.context(...))`;
  Rust 在 `install` 里用 `ctx.inject(InjectSpec::new(["systemPrompt"]),
  Arc<dyn Fn(&Context, ArcValue) -> BoxFuture<Result<(), PluginError>>>)`
  ——闭包必须显式标注返回类型(BoxFuture<Result<(), PluginError>>)否则
  推断失败;`SystemPrompt::context` 返回的 Disposer 用 `ctx.effect` 挂到
  inject 子 fiber(父插件 fiber 处置时级联,TS「dispose 移除上下文」语义)。
- **never 门必须内置**:decide() 在分派前判 `effective_policy == Never`
  → Rejected(prepend 监听器可绕过任何监听器形门);effectivePolicy =
  overrideOf ?? config.policy ?? Ask(三态回退,previous 永远是完整政策,
  setPolicy 通知文案不含 "undefined")。
- **回答器包含**:`futures::FutureExt::catch_unwind(AssertUnwindSafe(
  waterfall(...))).await` ——同步 throw(回调执行期)与异步 panic(poll 期)
  都落 Err → Unavailable;`downcast_arc::<ApprovalOutcome>` 失败(非词表
  值)同样归一化为 Unavailable;Rust waterfall 本身不含监听器 panic
  (与 TS 不同,TS 有 Promise 链包含)。
- **取消竞速**:信号谓词 15ms 轮询 future 与 answer future `tokio::select!`,
  中止赢时 answer 被 drop(晚到回答丢弃,TS removeEventListener 同款);
  入口先查信号(已中止 → Cancelled,零分派)。
- **审计对围栏**:request() 先 `has_open_turn`(倒扫 turn/start-end)再
  追加 asked(ts `id`/toolName/callId?/reason? 可选字段省略),decide 后
  追加 decided;id 用 uuid v4(approval_request_id 品牌)。
- **invariant 伴生的开放回合**:trace { open_turn, pending } 键控 session
  id;turn/start(open_turn = data.turn)/turn/end(清空)在
  internal/dispatch 预钩内同步折叠(TS 分 pre/post 两钩,Rust 合并——
  与 commands 伴生同款偏差);asked 校验开回合+非空 toolName+无重复 id,
  decided 校验开回合+已配对 id+闭词表 outcome,policy 校验闭词表;
  `fail` panic 被 collect 的 catch_unwind 包含(追加不被否决——偏差)。
- **作用域分派模板**:`scope_target(None, Some(agent.scope_key().clone()))`
  + `ctx.with_filter(carrier.filter)` + waterfall;未标记(根)ctx 的监听器
  始终通过,agent 作用域监听器仅键链匹配时通过(与 dsh-agent dispatch、
  dsh-tools 同款);scope.ctx 用 `create_scope(&ctx, agent 的 scope_key, ...)`
  挂监听。
- **policy 上下文偏差**:Rust AssembleContext 无 agent 字段 → 策略语句
  恒为空串(TS no-agent 分支);NEVER/ASK_SENTENCE 仍导出等待 agent 字段
  落地;测试以轮询 assemble 等待 inject 链异步注册完成(10ms×100)。

## 56. permission-presets(第 65 轮)

- **service 构造期校验序**:保留名 custom → shell 约束(shell 服务
  取 `Arc<dyn ShellExecutor>` 擦除柄,`sandbox_mode()` None 即
  "does not confine")→ derive(EMPTY) 推断默认 → custom 需显式
  defaultPreset → resolve(defaultPreset);全部 Err 返回(TS 构造器 throw)。
- **settings 段布线**:Rust `install_settings_section(ctx, ns, Schema,
  Data, SettingsSectionHooks)`;schema 用 schemastery
  `Schema::constant(Data::String(...))` + `Schema::union(...).required(true)`
  拼 defaultPreset 闭词表;setSource 收到 `Arc<dyn Fn() -> Data>` thunk,
  服务存进 `Arc<Mutex<...>>`(parking_lot Mutex 不可 Clone,需 Arc 包裹
  ——user-questions 同款坑);validate 钩子做表内校验(TS zod union 在
  Rust 无 zod,双保险);`provider.update(ns, patch, None)` 测试直写,
  ready() 等 wiring settle 后再写。
- **pin 双通道**:`session/created` 监听器(announce 内联 block_on,
  expect panic 即 veto 回滚 create——TS sync throw 等价)+ 安装期
  `store.list()` 存量补齐;seed 会话(含 session/end-seed)只补缺失
  事实;`Session::create` 带 seed 时自动追加 end-seed(store.create 的
  CreateSessionOptions.seed)。
- **投影单元状态是 JSON**:Rust 投影契约要求状态 `Arc<serde_json::Value>`
  (checkpoint 持久化 `downcast_arc::<ProjectionValue>`);typed
  `KnobState` 与 JSON 态之间经 `knob_state_to_json/from_json` 适配,
  apply 返回 None 时 caller 保持同一 Arc(change gate 依赖 ptr 相等);
  view 输出经 `validate_permission_select`(TS zod selectSchema 手写
  闭包等价)。
- **命令 handler 的 Err 语义差异**:Rust CommandRuntime 对 handler Err
  会重抛 execute(第 58 轮记录),TS 的 `{kind:'error'}` 结果是
  `Ok(CommandResult::Error { text })`;unknown preset 必须走 Ok(Error)
  才能不丢日志且返回结果对象。
- **可选子件 fiber 结算**:投影/命令两 inject 子 fiber 存入服务,
  `ready()` 逐个 settle——未挂载对应服务的 fiber 停在 Pending 无
  inertia,drain 立即返回(不会挂起);HMR 测试用轮询 snapshot 观察
  键出现/消失(挂载/卸载经子 fiber 级联)。
- **derive 默认链**:sandbox 旋钮回退 shell_default(安装期固定),
  approval 回退 `approval.config().policy ?? ask`——dsh-user-approval
  因此补 `config()` 公开访问器(TS `public config`)。

## 57. skill(第 66 轮)

- **构造环的 Weak 槽**:ScopedLayers 的 on_change 回调要触达注册表,
  但注册表 Arc 尚不存在(TS 捕获 this)——先建 `Arc<Slot(Mutex<
  Option<Weak>>)>` 供回调闭包捕获,注册表建成后回填 downgrade。
- **ScopeKey 键控缓存无需 id 表**:ScopeKey 是"进程唯一单调 id + 锚点
  Arc"值类型,Eq/Hash 按 id——直接 `IndexMap<ScopeKey, u64>` 做
  scope→id 稳定映射(TS WeakMap 对象身份),key_id() 为 pub(crate)。
- **层撤销闭包需 'static**:layers.effect 的 action 收 `&L`;undo 闭包
  若需层内可变表,该表必须自带 Arc(`Arc<Mutex<IndexMap>>`)——不能捕获
  `&SkillLayer`(生命周期错误);NamedEntries 的 insert 返回自带撤销,
  无此问题。
- **invalidation 存活性**:TS 校验"层内当前 provider 条目身份 == 注册
  时句柄";Rust 用每注册的 AtomicBool live 标志(undo 置 false)——作用域
  重建/处置后旧 control 天然失效,无需层身份比对。
- **fiber 处置的 apply 惯性坑**:刚 create_scope 就 dispose 时,ScopePlugin
  fiber 的 apply 链仍在 tokio 队列(inertia Some),`fiber.dispose` 的
  `has_inertia()` 短路不排 unload,drain 只等 apply 完成 → 层注册的
  undo 永不执行(通知/清理泄漏)。测试在 dispose 前 sleep 驱动运行时;
  真实组合中 scope 长命不受影响——记入偏差,后续可在 create_scope
  内 settle。
- **notify 包含**:回调"调用期"与"poll 期"都要在 catch_unwind 内
  (block_on 只包 poll 时同步 throw 会逃逸);warn 经 `named_logger(None)
  .warn(vec![arc(msg)])`(Logger::warn 收 Vec<ArcValue>),测试用
  `LoggerService.exporter()` 装 CaptureExporter 断言告警文本。
- **candidate 校验的编译期塌缩**:TS 运行时校验非字符串标量/非法 rank/
  畸形观察输出;Rust 闭类型使这些不可表示,保留名称语法/非空描述/
  provider 归属三项运行时校验——记录偏差。
- **中止竞速统一消息**:谓词无 reason 载荷 → 统一 SKILL_ABORTED_MESSAGE;
  wait_with_abort = select(业务 future, 15ms 轮询器),入口先查信号;
  TS "缓存发现后、加载前重查"在 Rust 落点于 wait 边界(provider.get
  惰性 future 未 poll),计数器语义差 1(记录偏差)。

## 58. tool-skill(第 67 轮)

- **MessageSource 第三次扩展**:新增 SkillCatalog { form, update?,
  entries: Vec<SkillCatalogEntry> } 与 SkillInvocation { name, form }
  两个 kind(serde tag kebab-case 自动生成 "skill-catalog"/
  "skill-invocation"),SkillCatalogEntry 导出;catalog_history 用
  `serde_json::from_value::<MessageSource>` 解码持久事件源——解码失败
  即"非本插件目录"(typed 枚举让 TS 的畸形种子不可表示,记录偏差)。
- **pre-step 瀑布三件套**:args[0]=AgentPreStepPayload(无 signal——
  dsh-agent 偏差)、args.last()=NextFn、返回值单层 arc(PreStepDecision);
  监听器按注册序外包:invocation 先注册、catalog 后注册、测试 later
  监听器最后 → 决策消息序 [claimed, catalog, invocation];fallback 回显
  声称批次(TS harness 同款),否则提案去重/替换断言会失配。
- **注册身份比对**:dsh-tools 新增 register_arc(caller, Arc<
  ToolDefinition>)——register 内部 Arc::new 后调用同一路径;catalog
  门控用 `tools.get("skill", scope)` 与注册 Arc 的 ptr_eq 判断精确
  定义(作用域同名遮蔽/restrict 均失配 → 不发目录)。
- **工具执行上下文**:ToolRunContext Deref 到 ToolExecution(agent/
  signal 直取);signal 是 `Mutex<AbortPredicate>` 需 lock+clone 转成
  SkillAbort;lookup scope 用 `agent.scope_key().clone()`、cwd 用
  `session.header().cwd.clone()`。
- **digest 与描述**:sha256 对每项 `serde_json::to_string((name,
  description))` 换行拼接(JSON 引号定界,分隔符注入安全);描述
  规范化 split_whitespace join + 截断走 char_boundary 回退
  (TS UTF-16 slice,非 ASCII 边界语义略有差异——记录)。
- **手势正则**:Rust 无 lookahead,`(^|\s)/([a-z0-9]+(-[a-z0-9]+)*)(\s|$)`
  捕获组 2;`/usr/bin`、`5/8`、`foo/hidden` 因后续字符非 \s|$ 不匹配;
  仅 user 源文本块扫描,去重保序。

## 59. skill-badge + plan-mode(第 68 轮)

- **skill-badge 资产嵌入**:正文 `include_str!("../assets/dsh-badge.md")`
  编译期内嵌(TS 运行期读文件);resourceBase 用 `CARGO_MANIFEST_DIR/
  assets`(源树路径,无包安装布局——偏差);PNG 测试校验 IHDR 宽高
  字节 + sha256 官方哈希。
- **plan-mode 意图表**:`HashMap<session.identity(), PendingIntent>`
  (TS WeakMap 对象身份;Session::identity() 是进程内稳定键);
  开回合排队 → 已接受 pre-step 边界提交(注意点:pre-step 监听器在
  Session.append 发布之外,边界 append 不会重入会话锁)。
- **set 四态语义**:noop 判 target(=pending ?? fold)与入参相等;
  开回合分支无条件覆盖 pending 并返回 cancelled/queued(fold==新值 即
  cancelled——pending 仍在,边界看到 fold 相等即清除不追加);空闲
  分支 append 成功后才删 pending(失败可重试)。
- **narration 门控**:`plan_mode_at_last_header` = 最后 request/header
  之前的 fold;told==target 不注入;提交路径 agent.inject、边界路径
  拼进 Enter.messages。
- **exit 工具与 user-questions 接线**:`ctx.get_typed::<Arc<
  UserQuestionService>>`;评审问题带 plan-review intent;驳回码塌缩为
  ASK_ABORTED(user-questions 偏差);批准后写 pending{narrate:false}
  待下一边界提交。
- **测试接线的两个坑**:(a) user-questions 的 agent liveness 梯需
  AgentRegistry——`register()` 经 effect 体异步落地会与紧随的工具调用
  竞态,测试改用 `enter()`(同步)+ `block_on(announce())`;(b) 投影
  单元靠 session/event 发布驱动——detached Session::create 不发布,
  测试会话必须走 store.create(permission-presets 同款)。
- **命令/投影两 inject 子件的异步注册**:测试以轮询 execute/snapshot
  等待子件落地(Pending 子 fiber 无 inertia,不会挂起)。

## 60. skill-filesystem(第 69 轮)

- **双读路径**:ctx.fs 服务存在且非 trusted_host 时走 FileSystem seam
  (resolve/stat/list_dir/read_text,FS_NOT_FOUND/FS_NOT_DIRECTORY/FS_NOT_TEXT
  码表映射缺失/非文本),否则 std/tokio::fs 回退;bundled 根 trusted_host
  直读宿主文件。
- **frontmatter 解析**:serde_yaml::from_str::<serde_yaml::Value> 解
  YAML;`---` 起始行 + 闭括号行扫描;调用政策禁用键
  (disable-model-invocation/user-invocable)+ 布尔词表(true/yes/on/1) +
  legacy 键(disableModelInvocation 等)拒绝;warn-and-skip 全部经
  named_logger。
- **监视子集偏差**:notify RecommendedWatcher 递归监视 + Notify 去抖
  (稳定阈值后失效);chokidar 的祖先监视(缺失根)、轮询模式、max
  projects 逐出未移植;fs/observed 突变钩未接线(Rust 演员柄
  FsObservationActorHandle 无工具名)。
- **Arc<Mutex> 与 Send**:parking_lot Mutex 不 Clone → 共享表包 Arc;
  去抖任务不能在锁守卫上 await(非 Send)→ 锁内克隆 Notify Arc 后
  释放守卫再 await。
- **路径拼接一致性**:测试断言资源基路径时用 Path 组件级比较
  (PathBuf join 与 forward-slash tail 的 Windows 分隔符拼写不同)。
- **home 解析**:dsh_home_paths::resolve_dsh_home(configured, env 闭包)
  + dirs::home_dir 做 agents 默认;workspace 依赖已有 dirs/serde_yaml/
  notify。

## 61. session-query-sqlite(第 70 轮)

- **seam 组合**:`SqliteSearch: SessionQuerySearch` 挂进
  `SessionQueryEngine::install(ctx, seam_config, Some(search))`;
  `engine.ctx` 是公开字段,后端从中取 `sessions` 与可选的
  `sessionPersistence`(erased `Arc<dyn SessionPersistenceApi>`)。
- **同步句柄纪律**:rusqlite `Connection` 是 Send 但非 Sync(内部
  RefCell),不能跨 await 持有 → `tokio::sync::Mutex<()>` 做 TS
  `_tail` 序列化门闩(unit 是 Sync,守卫可跨 await),`parking_lot::
  Mutex<Option<Connection>>` 只锁同步段(BEGIN/INSERT/COMMIT、行读取、
  查询),await(观察持久源)之间释放;`close()` 置 AtomicBool 后取门
  闩关闭(在排队的操作之后,排队操作取闩后见 closed → INDEX_FAILED)。
- **binding 身份**:TS `Symbol()` 身份 → AtomicU64 计数;
  `current_binding()` 轮询 registry(`get_typed::<Arc<dyn
  SessionPersistenceApi>>`)+ `Arc::ptr_eq` 比较,变化即发新身份;
  `inject(["sessionPersistence"])` 子 fiber 在服务卸载时经 effect
  disposer 复位 binding 单元格(补轮询的挂载+卸载双盲区;测试侧在
  unmount 后 sleep 20ms 等子 fiber 复位,沿用第 63 轮模式);身份变化
  → `persistence_epoch += 1` + `global_generation += 1` → 会话游标
  按 `persisted:{epoch}:{generation}` 失效,事件游标按 `live:{gen}`
  只看目标行代。
- **SQL 直译**:CTE 三段(candidates UNION ALL → matched 计数 →
  filtered/ranked ROW_NUMBER 分区取 event_rank=1)逐字移植;`?`
  位置参数顺序与 TS `selectedDocumentsParams` 一致(10 个固定绑定:
  高亮起止 ×2 + 表达式 ×2 + 可见性 ×2 + 计数起 + 起标记字节长);
  `rusqlite::types::Value` 做动态绑定;FTS5 `highlight()`/`MATCH`
  经 bundled(默认含 FTS5)直通;STRICT 表 3.45+ 可用。
- **谓词预算**:buildSessionWhere/buildEventWhere 内部即断言 ≤14,
  `_querySessions` 再断言两者合计 ≤14,`_queryEvents` 断言 1+事件
  谓词 ≤14;绑定上限 32766 在 addRange/addList/总装三处断言;
  `SQLITE_MAX_PAGE_LIMIT = 2^53-2`。
- **片段算法**:`make_snippet` 的 `prefix/suffix` 长度必须按
  `chars().count()` 计('…' 为 3 字节)——按 `.len()` 计会把 5 码点
  片段裁成 "ab…";TS `Array.from` 码点语义在 Rust 用 `Vec<char>`
  对齐(😀 占 1 码点)。
- **游标四键**:version/instance/scope/fingerprint/offset 全对 +
  offset 为 JSON 整数(u64;1e100 解析为 f64 → as_u64 None → 无效);
  generation 不符 → STALE,其余不符 → INVALID;fingerprint =
  canonicalFilters(值 null 先排 + JSON 串排序)的 sha256 base64url。
- **观察稳定**:每搜索两次尝试;快照前后材料化不等(含 revision/
  header 比较)、binding 身份中途变化、live id 集变化 → 重试;
  失败映射:中止优先 → ABORTED,身份已变 → 重试,其余 →
  PERSISTENCE_FAILED 包装(Rust API 只有 String 错误,TS 的类型化
  错误透传在此无对应——偏差);persisted 行复用判据 =
  `last_persistence_identity` 相同 + 已索引 revision 相同,且 live
  遮蔽(initially_live 或 sessions.get)跳过 inspect。
- **偏差清单**:① 真实 `dsh-session-persistence-sqlite` 注册具体
  `Arc<T>` 与 erased 查询面注册互斥(cordis 同 scope 重注册 panic),
  组合集成测试暂用 erased 假后端覆盖——宿主组装时选 erased 别名;
  ② 查询期 SQL 错误包装为 INDEX_FAILED(TS 裸 SqliteError);
  ③ Windows 测试 cwd 用 `C:\...` 绝对路径(session header 校验宿主
  平台绝对性);④ TS 的 "non-error ready failure" 规范化在 Rust 恒为
  String 消息。

## 62. schedule(第 71 轮)

- **线格式**:`ScheduleRecord` serde tag=kind(小写)、字段 camelCase
  (afterSeconds/everySeconds/scheduledAt);`ScheduleChange` tag=operation
  (create/delete/dispatch)+ version=1,dispatch 的 acceptedAt 可选跳过
  序列化;`ScheduleToolError` tag=code(snake_case)含 operation 枚举与
  可选 id;值联合用 untagged enum(View 在前,Error 靠 code 键区分)。
- **时间四件套**:① 正则 `(?!0000)` 前瞻 regex crate 不支持 → 显式
  `starts_with("0000")` 检查;② 毫秒补零必须 `{:0<3}`(右补零),
  `{:<03}` 是左对齐空格填充 → "25" 解析失败静默为 0(上海 .25 案例);
  ③ JS 安全整数 = |值| ≤ 2^53-1,i64 能装下 MAX_SAFE*1000 但 JS 会拒 →
  `checked_mul(1000).filter(≤9_007_199_254_740_991)` 双检;④ `future_instant`
  同时校验 now 与 epoch 的四位年区间(TS `Number.isSafeInteger(now)`
  直译为区间检查)。
- **DST 解析**:chrono-tz 复刻 TS 采样算法——在 localEpoch ±2 天取样
  5 个 UTC 时刻的时区偏移去重,逐候选回投并比对日历字段,取最早
  (overlap 第一瞬间),全不匹配时按 outOfRange 标志分
  time_out_of_range/invalid_rule(gap);`parse_canonical_instant` 用
  chrono `%Y-%m-%dT%H:%M:%S%.3fZ` 严格解析 + 格式化往返。
- **时区别名**:chrono-tz 无 ICU 别名库,内置常用 backward 别名表
  (~80 条:US/*、Canada/*、GB、PRC、Asia/Calcutta 等),未知别名
  invalid_time_zone(偏差)。
- **运行时**:`Agent::run_maintenance` 结果擦除 → 任务闭包写
  `Arc<Mutex<Option<bool>>>` 共享槽,runtime await 后读回;驱动循环
  tokio::spawn + `self_arc()`(OnceLock<Weak> 回取,避免 &self 克隆
  ScheduleRuntime);timer = JoinHandle + abort;`when_idle` 竞速用
  tokio::select!(when_idle vs stop Notify);run_schedule_transaction =
  每 agent 指针键的 tokio Mutex 串行门。
- **flush 屏障**:`sessions.flush` 只在有监听器时返回 true——测试
  必须注册 no-op `session/flush` 确认器,否则 preflight 恒失败。
- **invariant 伴生**:dispatch 内联钩在 append 锁内 → 禁读
  session.events;改按 session.identity() 维护增量折叠 trace(active
  Vec + seenIds),对候选 change 单步 apply_change 验证,等价于
  候选扩展流校验;session/created 时全量 seed。
- **工具**:execute 返回 Ok(json!()) 承载全部错误值(TS 错误也是值
  而非异常);闭包先捕获 `exec.agent.clone()` + `exec.signal.lock().clone()`
  再进 'static future(否则 borrow 生命周期错);present 泛型卡的
  kind/raw_input/locations 均 Option;注册失败 rollback + 记日志
  (TS 重抛)。

## 63. dsh-host M6 组合升级(第 72 轮)

- **持久化 erased 注册统一**:jsonl/sqlite 两个后端的 install 改为
  `let erased: Arc<dyn SessionPersistenceApi> = backend.clone();
  ctx.register_service(erased)` —— cordis 同 scope 重注册 panic,具体
  类型注册会挡住 session-query/corpus/schedule 的 erased 查询面
  (第 70 轮偏差的修复);先 grep 确认全仓无
  `get_typed::<Arc<JsonlSessionPersistence>>` 类具体消费者(coordinator
  直接拿 backend.clone(),不查注册表)。
- **宿主组合**:10 服务同 ctx 挂载顺序 = invariants → sessions →
  agents → systemPrompt → tools → commands → userQuestions →
  jsonl persistence → query-sqlite → schedule::apply;数据目录
  temp + uuid,HostSpine Drop 清理。
- **端到端探针**:store 会话 append + `sessions.flush` → JSONL
  coordinator 的 session/event 写后队列 + flush 监听器落盘
  (快照数=2:live 会话经 coordinator 持久化 + 直写 persisted-only);
  query 引擎 live-preferred 双源各命中 1 条。
- **死锁陷阱**:组合内含同步安装器 `futures::executor::block_on`
  (SqliteSearch::install 开库、coordinator 注册监听器);测试若用
  `current_thread` flavor,内嵌 block_on 无法驱动 tokio::spawn 的
  注入 fiber → 死锁;boot 测试改 multi_thread(worker_threads=2),
  二进制 #[tokio::main] 默认多线程无此问题。
- **invariant 伴生挂载**:session 的 apply 返回 future(block_on),
  schedule/query-sqlite 的 apply 返回 Disposer(直接绑定持有)。

## 64. subagent 契约层(第 73 轮)

- **范围**:本轮只落地 contracts(run/result/capability 类型 + v2 持久
  描述符 + 深度记账 + 提供者 trait + 错误码类);runtime/continuation/
  registry/backends/tools 与观察事件留待后续轮次(偏差)。
- **线格式**:`SubagentDescriptorData` serde tag=mode("one-shot"/
  "continuable")+ camelCase 字段(agentProvider/agentModel/toolFilter);
  解析严格键集合(one-shot 4 键 / continuable 8 键)+ toolFilter
  allow/deny 至少其一 + 数组全串校验;版本闸门 = 非 2 → None(不可
  分类),当前版本畸形 → Err(13 例拒绝表逐条断言消息)。
- **共享类型扩展**:`AgentOptions` 加 `subagent_depth: Option<u64>`
  (TS 的 module augmentation 在 Rust 直接落字段;两个 agent-loop 测试
  结构体字面量补 None);`ToolRestriction` 补
  serde/PartialEq + skip_serializing_if(描述符持久化与断言需要)。
- **seam 差异**:TS `AbortSignal` → 共享中止谓词 `Arc<dyn Fn()->bool>`;
  `prepareContinuable` 可选方法 → 默认 Err("SUBAGENT_NOT_CONTINUABLE")
  的 trait 方法(方法存在即能力在 Rust 用重写表达);
  `SubagentStartRequest.parent: Arc<dyn Agent>`;含 dyn 字段的结构
  不能 derive Debug(手动 Clone)。
- **seed**:`Session::create(child_id, seed, None)` 暂存 + append
  `subagent/descriptor`(无 surfaceOp)→ 取 events(自动含
  session/end-seed 尾事件,与 TS 行为一致)。

## 65. subagent 服务核心(第 74 轮)

- **注册表**:providers 用 `Arc<Mutex<HashMap>>` 字段 + 手写 Clone
  (parking_lot Mutex 不 Clone);register_provider 效应作用域化
  (caller.effect 持有 disposer,disposer 自身 Arc 克隆一份返回——
  Disposer 是 Fn 闭包,进 async move 需先绑定再 clone);重名 →
  DUPLICATE_PROVIDER,处置后 get 为 None + NO_PROVIDER。
- **能力闸门**:四个旗标按请求字段存在与否逐一对照 provider
  capabilities,首个缺失即 UNSUPPORTED_CAPABILITY;校验在委派前
  (测试断言 provider 未收到启动调用);outputSchema 在 Rust 侧只做
  对象根检查(TS assertObjectJsonSchema 完整子集由 tools 运行时
  持有,偏差);maxDepth 校验走 depth::assert。
- **生命周期事件**:TS `ctx.events.dispatch('emit', [carrier, name,
  info])` 的 scope 载波在 Rust 用
  `ctx.with_filter(scope_target(None, parent.scope_key()).filter)`
  建过滤派发上下文再 `events.collect(Emit, Some(&dispatch_ctx), ...)`
  ——载体基过滤略去服务实例(偏差);逐监听器 catch_unwind +
  block_on,失败只记日志(TS 同步抛/异步拒绝对称);observe_run 先
  spawn 终局观察任务再同步发 start(同一线程上 spawn 不内联执行 →
  start→end 顺序保持);runId = UUIDv4 配对。
- **输出选择**:assistant/message 的 message.content 非空数组即替换
  候选(text-delta 只累积回退流);ContentBlock serde 解析复用
  dsh-llm 线格式;settleRun 三态映射 completed→Completed(text 拼合
  过滤非 text 块)/aborted→Killed/其余→Failed(detail=reason 串),
  dispose 失败合并 `detail; dispose failed: ...`。
- **暂拒面**:continuation 操作 → CONTINUATION_UNAVAILABLE、listing →
  UNSUPPORTED_CAPABILITY(manager/projection 未移植,偏差);测试
  flavor multi_thread(生命周期 spawn 需可推进)。

## 66. subagent 进程内驱动 + fork 后端(第 75 轮)

- **组合时点偏差**:TS 在 agents.create 的未发布创建窗内跑 setup
  (policy 追加 + 组合);Rust 侧 loop 的创建窗 setup 收
  `prepared.agent.ctx()` 但该 ctx 不提供 "agent" 服务(TS
  `childCtx.agent` 契约),且发布即返回 → 驱动改为 create 返回后、
  followup 前追加 policy 事件与 persona/restrict 组合(种子与首回合
  之间的相对顺序保持一致,但事件落在发布之后——偏差)。
- **AgentSetup 形态**:`Arc<dyn Fn(&Context) -> BoxFuture<Result<Option<
  Commit>, String>>>`——若未来改用创建窗,setup 闭包需返回
  Result<Option<Commit>>;本轮驱动 setup=None。
- **run 封装**:drivePublishedRun 的 PublishedRun 用
  `Mutex<Option<JoinHandle<SubagentResult>>>` 承载 result 任务 + 
  `Mutex<Option<AgentHandle>>` 承载 dispose;dispose 先 cancel(agent
  cancel cause Parent)再取柄 await(锁守卫不能跨 await → 作用域内
  take);result 单次消费(take 后二次调用报错);中止谓词在
  followup 前再查一次(TS abort 监听器等效)。
- **终局读法**:`readResult` 用 `foldConsumedWork(own).end` 的 reason →
  toStopReason(kind 串匹配 completed/max-tokens/aborted/blocked/
  其余 error);cancelled 且非 completed → aborted 覆盖;输出经
  finalAssistantOutput(同 seam 规则)。
- **fork 前缀**:`rposition(turn/end)` 切片含终界;Session::create
  种子校验要求 user/message 带 surfaceOp + 合法 message data(测试
  种子必须构造完整事件,不能裸 json!({turn}))。
- **能力面**:fork provider outputSchema=false(结构化捕获未移植),
  其余三旗标全开;structured.ts 的 capture 工具 + 指令留待后续。
