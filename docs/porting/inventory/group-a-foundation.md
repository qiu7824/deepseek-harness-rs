# Group A — 基础运行时(Foundation Runtime)包级清单

> 目标:为"后端 Rust 1:1 移植"提供包级清单。只读分析,不修改 TS 源码。
>
> - TS 源码根:`D:\HermesTemp\deepseek-harness`
> - Rust 目标仓库:`D:\deepwork\deepseek-harness-rs`(workspace 声明 `crates/vendor/*`、`crates/core/*`、`crates/feature/*`、`crates/exec/*`、`crates/host/*`、`crates/app/*`、`apps/*`;已落地 crate:`crates/vendor/cordis`、`crates/vendor/cosmokit`)
> - 盘点日期:见仓库 git 状态;包版本均为 `0.1.0-rc.5`(cordis 生态为各自版本)

## 0. 结构说明与依赖拓扑(先读)

### 0.1 monorepo 实际布局与命名映射

任务描述中的 `vendor/cordis`、`packages/core`、`packages/boot`、`packages/bundle` 与实际布局的对应关系如下:

- `packages/core`、`packages/boot`、`packages/bundle` 是**目录组,不是单包**。每个组下是若干 leaf 包:
  - `packages/core/*` → 8 个包
  - `packages/boot/*` → 2 个包
  - `packages/bundle/*` → 3 个包
- `vendor/` 下**没有字面名为 `cordis-plugin-*` 的目录**;cordis 生态插件的目录名与 npm 包名不同,映射如下:

| vendor 目录 | npm 包名 (`@deepseek-ai/…`) | 角色 |
|---|---|---|
| `vendor/cordis` | `cordis` | 核心 DI/插件框架 |
| `vendor/cosmokit` | `cosmokit` | 通用工具库(非插件) |
| `vendor/schemastery` | `schemastery` | 类型驱动 schema 校验器(非插件) |
| `vendor/loader` | `cordis-plugin-loader` | 插件加载器 |
| `vendor/include` | `cordis-plugin-include` | 配置文件 include |
| `vendor/group` | `cordis-plugin-group` | 嵌套插件组 |
| `vendor/hmr` | `cordis-plugin-hmr` | 模块热替换 |
| `vendor/timer` | `cordis-plugin-timer` | 定时器服务 |
| `vendor/logger-console` | `cordis-plugin-logger-console` | 控制台日志导出器 |

> 因此 "vendor/cordis 及 vendor/ 下所有 cordis 相关包" 实际覆盖 9 个包:1 个核心 + 6 个插件(loader/include/group/hmr/timer/logger-console)+ 2 个支撑库(cosmokit/schemastery)。

### 0.2 包数统计

| 组 | 包数 | 包列表 |
|---|---|---|
| vendor/cordis 生态 | 9 | cordis, cosmokit, schemastery, loader, include, group, hmr, timer, logger-console |
| packages/core | 8 | agent, scope, session, system-prompt, tools, agent-loop, agent-default-model, agent-tool-presentation |
| packages/boot | 2 | app-boot, cmdline |
| packages/bundle | 3 | base, headless, web-app |
| **合计** | **22** | |

### 0.3 workspace 依赖方向(自底向上,移植顺序)

```
cosmokit (无依赖)
  ├─ schemastery (依赖 cosmokit + @standard-schema/spec)
  └─ cordis (依赖 cosmokit + @standard-schema/spec)  ← 核心
       ├─ cordis-plugin-timer (依赖 cordis)
       ├─ cordis-plugin-group (依赖 cordis + loader)
       ├─ cordis-plugin-loader (依赖 cordis + cosmokit)
       │    ├─ cordis-plugin-include (依赖 loader + cordis + cosmokit + js-yaml)
       │    └─ cordis-plugin-hmr (依赖 cordis + loader + include + timer + schemastery + cosmokit)
       └─ cordis-plugin-logger-console (依赖 cordis + cosmokit + schemastery)

dsh-scope (依赖 cordis + invariants)
  ├─ dsh-session (依赖 cordis + scope + brand/invariants/llm/typert)
  ├─ dsh-system-prompt (依赖 cordis + scope + llm + invariants + schemastery)
  ├─ dsh-agent (依赖 cordis + scope + session + system-prompt + typert + llm + invariants)
  │    ├─ dsh-agent-default-model (依赖 agent + llm + settings + invariants + schemastery)
  │    └─ dsh-agent-loop (依赖 agent + session + system-prompt + tools + llm + scope + session-persistence + settings + schemastery)
  ├─ dsh-tools (依赖 cordis + agent + scope + session + system-prompt + llm + code-runtime + invariants + schemastery + user-approval)
  └─ dsh-agent-tool-presentation (依赖 tools + invariants + schemastery)

dsh-cmdline (依赖 cordis + loader + invariants)
dsh-app-boot (依赖 group/hmr/include/loader + launch-environment + invariants + home-paths + system-prompt + cordis)

dsh-base      (bundle:依赖几乎全部 core/feature 插件,见 cordis.patch.yml)
  ├─ dsh-headless (依赖 base + cmdline + code-runtime-worker-thread + agent + llm + session + schemastery + loader + cordis)
  └─ dsh-web-app  (依赖 base + app-boot + cmdline + 大量 host/client 插件 + schemastery + loader + cordis)
```

---

## 1. Cordis 核心机制详解(移植基础,优先精读)

`@deepseek-ai/cordis` v4.0.1(vendor/cordis)是整套系统的地基:一个**依赖注入容器 + 插件生命周期 + 事件总线**元框架。Rust 侧必须先精确复刻它的语义,其余 21 个包才有落点。

源码文件(9 个):`context.ts`、`events.ts`、`fiber.ts`、`logger.ts`、`reflect.ts`、`registry.ts`、`service.ts`、`utils.ts`、`index.ts`。公开入口 `index.ts` re-export **除 `reflect.ts` 之外**的 7 个模块(`reflect.ts` 是内部实现,但 `ctx.reflect` 对外可见)。依赖:`@deepseek-ai/cosmokit`(runtime dep)、`@standard-schema/spec`(runtime dep)、`cordis-plugin-include`/`cordis-plugin-loader`(optional peer)。

### 1.1 Context —— 被 Proxy 包裹的依赖容器(context.ts)

- 每个上下文实例 `new Context()` 返回一个 `Proxy`(handler = `ReflectService.handler`)。
- **属性读取(get trap)** 顺序:`isSpecialProperty`(symbol / `_` 前缀 / 数字串 / `prototype`/`then`)→ 直接 `Reflect.get`;否则查 own props(经 `getTraceable` 追踪)→ 查 `reflect.props` 里的 accessor → 触发 `internal/get` waterfall → 最终沿 fiber 链解析服务(见 1.3)。读未注入服务抛 `cannot get property "x" without inject`(增强错误栈)。
- **属性写入(set trap)**:非 `provide` 声明的属性写抛错;accessor 走 set hook;服务属性走 `internal/set` waterfall 到 `reflect.set`。
- **作用域三件套**(全部是 symbol-keyed,原型链继承,父上下文不被突变):
  - `extend(meta)`:原型链继承创建子上下文(服务作用域继承)。
  - `isolate(name, label?)`:`[symbols.isolate]` 映射 `服务名 → 隔离 label`(Symbol)。同 label 的两个 isolate 合并作用域;不同实现互不干扰。
  - `intercept(name, config)`:`[symbols.intercept]` 映射 `服务名 → 拦截配置`,插件启动时合并进该服务的 resolved config(祖先先、`Service[symbols.resolveConfig]`)。
- 构造时安装 5 个内置服务:`fiber`(根 Fiber)、`reflect`(ReflectService)、`registry`(RegistryService)、`events`(EventsService)、`logger`(LoggerService),并通过 `mixin` 把 `events/registry/reflect/fiber` 的方法铺到 `ctx` 上(因此有 `ctx.on/emit/plugin/inject/effect/get/set/provide/accessor/mixin`)。

### 1.2 Service —— 服务基类与"可调用服务"(service.ts)

- `Service` 抽象类。子类 `constructor(ctx, name)` 里调 `super(ctx, name)`,内部执行 `ctx.reflect.provide(name, self, check)`,服务即注册并被当前 fiber 拥有(卸载自动移除)。
- 静态 symbols(常量字段,亦是 `utils.symbols`):
  - `init` —— 构造后的异步初始化方法(如 `async *[Service.init]()`,generator effect)。
  - `check` —— 可用性谓词(依赖方只在 check 通过时加载)。
  - `config` —— 幽灵类型参数(拦截配置类型)。
  - `invoke` —— 让服务**可调用**的调用体(如 `ctx.logger()`、`ctx.registry()` 等 callable service)。
  - `extend` —— 派生扩展实例的 helper。
  - `tracker` / `resolveConfig` —— 追踪元数据 / 拦截配置合并。
- `[symbols.resolveConfig](base?, head?)`:沿 `[Context.intercept]` 原型链向上收集该服务的拦截配置(根→叶),`base` 前插、`head` 后插,`Config.merge` 存在则用它,否则 `Object.assign` 浅合并。
- `[symbols.filter]`:隔离过滤 —— `ctx[symbols.isolate][name] === this.ctx[symbols.isolate][name]`(决定依赖方是否看到本作用域的实现)。
- `static [Symbol.hasInstance]`:跨 Proxy 沿构造链判断实例(替代 `instanceof`)。

### 1.3 ReflectService —— 服务解析与"call interception"(reflect.ts,核心中的核心)

`ctx.reflect` 是服务解析 + Proxy handler + accessor/mixin 的实现。

- **存储**:`store: Dict<Impl, symbol>` —— 服务实现按**隔离 label(Symbol)**键控(不是按名字!);`props: Dict<Property>` —— 声明的上下文属性(service/accessor)。
- `provide(name, value, check?)`:`effect` 注册;在 `ctx.root[symbols.isolate][name] ??= Symbol(name)` 分配 label;`store[key] = {name, value, fiber, check}`;当前 fiber `store[name] = impl`;返回 disposer(卸载时 `delete store[key]` 并 `notify([name])` 唤醒依赖方)。重复提供抛错。
- `get(name, strict)` / `set(name, value)`:按 label 读/写;`set` 仅允许**提供它的那个 fiber** 写入。
- `notify(names, filter)`:遍历 registry 所有 runtime 的所有 fiber,对被影响的依赖 `_checkImpl` + `_refresh`(重载或卸载),并 `emit('internal/service', name, value)`(带 scope filter)。
- **call interception / 追踪机制**(见 1.8 `getTraceable`):从 ctx 读到服务时不是返回裸对象,而是返回一个 **traceable Proxy**,它把服务的 `ctx` 属性(通过 `tracker.property`)重绑定到**读取方的 active context**。这样服务方法内部读 `this.ctx.xxx` 时得到的是调用者作用域,而非服务注册时的作用域。这是整个 DI 正确性的关键。
- `accessor(name, {get, set})`:计算属性(如 `ctx.agent`),fiber 卸载时移除。
- `mixin(source, mixins)`:把服务的成员转发成 `ctx` 上的 accessor,方法绑定到服务(如 `ctx.on` → `ctx.events.on`)。

### 1.4 RegistryService 与 Plugin 注册机制(registry.ts)

- `Plugin` 三种形状:`Function`(fn(ctx, config))、`Constructor`(new (ctx, config))、`Object`(`{apply(ctx, config)}`)。
- `Plugin.Base` 元数据:`name`、`Config`(StandardSchemaV1 校验器)、`inject`(依赖:数组或 `名→拦截配置` map)、`provide`、`intercept`。
- `Plugin.Runtime`:一个 plugin callback 共享的可变记录 `{name, fibers: DisposableList<Fiber>, callback, Config}`。同 callback 的多次 `ctx.plugin()` 产生多个 fiber,共享一个 runtime。
- `ctx.plugin(plugin, config)`:`resolve` 出 callback → 取/建 runtime → `new Fiber(ctx, config, Inject.resolve(inject), runtime, stack)` → 返回 `Fiber & PromiseLike<Fiber>`(await 即 `fiber.await()`)。
- `ctx.inject(deps, callback)` = `ctx.plugin({inject, apply: callback})`:依赖变化时自动卸载/重跑。
- `Inject` 装饰器:`@Inject(name, config)` 作用在类(写 `static inject`)或方法(延迟到依赖可用后调用)。

### 1.5 Fiber —— 插件生命周期与 effect(fiber.ts)

- 一次 `ctx.plugin()` 产生一个 Fiber(插件运行时实例)。根上下文 fiber `uid=0`。
- **状态机** `FiberState`:PENDING → LOADING → ACTIVE →(UNLOADING)→ DISPOSED;FAILED。状态变化 `emit('internal/status', fiber, oldState)`。
- **依赖等待(epoch 机制)**:构造时按 `inject` 逐项 `_checkImpl`;`_refresh()` 把"所有依赖 impl 的 fiber.uid"拼成 epoch 字符串;epoch 从 `INACTIVE` 变非空 → `_reload()`,反之 `_unload()`。任一依赖 provide/dispose → `reflect.notify` → 依赖方 `_checkImpl`+`_refresh` → 自动重载/卸载。
- **`effect(execute, label)`**:注册可逆副作用,立即执行,返回 disposer(单次,幂等)。`execute` 可返回:disposer 函数 / 其 Promise / (async) iterable(generator)逐条 yield disposer。卸载时按注册**逆序**运行 disposer(可异步,await)。标签生成诊断树 `getEffects()`。
- `_reload()`:`await Promise.resolve()` 切微任务 → `_resolveConfig`(`internal/config` waterfall → `runtime.Config['~standard'].validate`)→ `_execute(runner)`(构造/调用 plugin callback)。失败 → `_error`、状态 FAILED、日志。
- `_unload()`:`Promise.all(_disposables.clear())` 逆序释放,再判断是否重载。
- `update(config, noSave)`:`internal/update` waterfall(HMR/持久化可 veto/替换),默认 `restart()`(卸载+重载)。
- `await()`:等待 in-flight 生命周期稳定,rethrow 启动错误。
- `ValidationError`:标准 schema 校验失败聚合 issues 为多行消息;`CordisError('INACTIVE_EFFECT')`:已释放 fiber 上注册 effect 抛错。

### 1.6 EventsService —— 事件总线(events.ts)

- **5 种 dispatch mode**(`DispatchMode`):
  - `emit`:同步调用所有监听,忽略/不 await 返回值(返回的 Promise 用 `void …catch` 兜底)。
  - `parallel`:并发 await 全部,`Promise.allSettled`,有 reject 抛 `AggregateError`。
  - `serial`:顺序 await,直到某个返回 bail 值(非 null/false/undefined)。
  - `bail`:同步顺序调用,直到 bail 值。
  - `waterfall`:洋葱模型;末参是 `next` continuation,监听外层先跑,不调 `next()` 即 veto 后续链。
- `dispatch(type, args)`:**解析监听 + 应用上下文过滤**。首参若是对象/函数则当作 `thisArg`;若 `thisArg[Context.filter]` 存在,只保留 `hook.global || !filter || filter(hook.ctx)` 的监听 —— 这是 scope 过滤事件分发的落点。非 `internal/` 事件先 `emit('internal/dispatch', …)`。
- `on/once(name, listener, options)`:监听经 `reflect.bind` 追踪;`bail('internal/listener', …)` 允许拦截(如 `internal/update` 特判);`register` 用 `fiber.effect` 存储 → 卸载自动移除。
- **内置事件 `Events`**:`internal/plugin`(fiber 创建/释放)、`internal/status`(状态迁移)、`internal/config`(waterfall,解析 raw config)、`internal/service`(服务绑定变化)、`internal/update`(waterfall,配置热更新)、`internal/get`/`internal/set`(waterfall,代理读/写拦截)、`internal/listener`(bail,监听注册拦截)、`internal/dispatch`(emit,派发诊断)。

### 1.7 LoggerService —— 可调用日志服务(logger.ts)

- `ctx.logger` 是 callable service:`ctx.logger(name?)` 创建命名 Logger;直接 `ctx.logger.info(...)` 用 fiber 派生名。
- `exporters` 注册表(内置一个 1000 条环形 buffer exporter);`Message{sn,ts,name,type,level,args,fiber}` 结构化记录;printf 风格格式化(`%s %d %i %f %o %O %c %C`,可覆写 `formatters`);级别 `ERROR/INFO/WARN/DEBUG`。
- `LoggerService.Intercept{name?, level?}`:经 `[symbols.intercept]` 合并。

### 1.8 utils —— symbols / DisposableList / 追踪 Proxy(utils.ts)

- `symbols`(全局 `Symbol.for('cordis.*')`,跨副本稳定):`shadow`、`receiver`、`original`、`metadata`、`initHooks`、`checkProto`;`effect/filter/isolate/intercept`(context);`init/check/config/invoke/extend/tracker/resolveConfig`(service)。
- `DisposableList<T>`:有序可释放集合,O(1) 按值删除;`clear()` 返回逆序快照。
- `getTraceable(ctx, value)` / `createTraceable` / `createShadow` / `createShadowMethod`:**call interception 实现**。tracker 带 `{associate, property, noShadow}`;get 时若 `prop === tracker.property` 返回调用方 ctx;若 `associate` 且存在 `associate.prop` 属性则转发到 ctx 上的对应访问器;方法读取经 shadow 重绑定 `this`。callable service 经 `createCallable` + `applyTraceable` 派发 `symbols.invoke`。
- `composeError` / `buildOuterStack`:异步错误长栈拼接(把外层调用栈拼进异步 throw)。
- `joinPrototype`、`isConstructor`、`isObject`、`withProps` 等辅助。

### 1.9 Rust 移植要点(cordis 核心)

- **crate 路径**:`crates/vendor/cordis`(命名建议 `dsh-cordis`)。
- **难点**:
  1. **动态 proxy + 原型链继承**在 Rust 无直接等价物,需用 `Arc`/`RwLock` 显式实现作用域链(isolate/intercept/shadow 三层 map 的不可变共享 + 写时复制)。
  2. **`any` 服务值 + 追踪重绑定**:TS 依赖鸭子类型;Rust 需 `Arc<dyn Any>` + downcast,或 trait object;call interception 的"读方 ctx 重绑定"需改成"服务方法显式携带 caller 上下文"或在 trait 层传递。
  3. **effect/生命周期**:fiber 的 epoch 重载/卸载状态机可映射为 `tokio::task` + `JoinHandle`,disposer 逆序释放映射为 RAII/drop guard 栈。
  4. **事件总线**:5 种 dispatch mode 需精确复刻(bail/serial/waterfall 语义、context filter)。
  5. **schema 校验集成**:config 校验委托给 `dsh-schemastery` 实现 Standard Schema 语义。
  6. **异步错误长栈**:Rust 用 `thiserror` + `#[from]` 链式,无长栈需求,可简化为 `anyhow::Context`。
- **依赖顺序**:先 `dsh-cosmokit` → 再 `dsh-schemastery` → 再 `dsh-cordis`。

---

## 2. vendor/cordis 生态包(9 个)

### 2.1 @deepseek-ai/cosmokit(vendor/cosmokit,v1.8.2)
- **用途**:通用工具库(cordis/schemastery 及全仓的地基工具)。
- **workspace 依赖**:无(零依赖)。
- **src 模块**:`array.ts`、`types.ts`、`misc.ts`、`string.ts`、`time.ts`、`index.ts`(6 个,`index.ts` re-export 前五个)。
- **导出面**(按模块):`array`(集合去重/归一化)、`types`(运行时类型判断、`Binary`、`clone`、`deepEqual`、`isNullable`/`isPlainObject` 等)、`misc`(`Dict`/`Awaitable`/`Promisify` 类型、`valueMap`/`pick`/`filterKeys`、`defineProperty` 等)、`string`(`hyphenate` 等大小写/路径/属性格式化)、`time`(时间常量/解析/格式化)。
- **Rust 建议**:`crates/vendor/cosmokit`(命名 `dsh-cosmokit`)。难点低,主要是 `deepEqual`、`clone`、`Binary`(对应 `serde_json::Value` + bytes)、`Dict`(→ `serde_json::Map` / `IndexMap<String, Value>`)。零依赖,最优先移植。

### 2.2 @deepseek-ai/schemastery(vendor/schemastery,v3.18.1)
- **用途**:类型驱动 schema 校验器,实现 Standard Schema V1;cordis 的 `Plugin.Config` 校验 + 全仓配置校验/表单渲染的 schema 源。
- **workspace 依赖**:`@deepseek-ai/cosmokit`、`@standard-schema/spec`。
- **src 模块**:单文件 `index.ts`(902 行)。
- **导出面**:
  - `Schema`(default,callable + `new`):工厂 `any/never/const/string/number/natural/percent/boolean/date/regExp/arrayBuffer/bitset/function/is/array/dict/tuple/object/union/intersect/transform/lazy`;`resolve`、`from`、`extend(type, resolve)`、`ValidationError`。
  - schema 实例方法链:`required/hidden/loose/role/link/default/comment/description/disabled/collapse/deprecated/experimental/pattern/max/min/step/set/push/simplify/i18n/extra`、`toString`、`toJSON`(递归引用保序序列化)。
  - `~standard` getter → `StandardSchemaV1.Props`(`validate` 返回 `{value}` 或 `{issues}`)。
  - 内置 resolver:`any/never/const/string/number/boolean/bitset/function/is/array/dict/tuple/object/union/intersect/transform/lazy`;`Schema.extend` 注册自定义类型。
- **Rust 建议**:`crates/vendor/schemastery`(命名 `dsh-schemastery`)。难点:**可调用 + 链式 builder + 递归/共享引用序列化**。Rust 侧可做成 `Schema` 枚举节点 + builder(不仿 JS 的 callable 语法,而用 `Schema::object(...)` 关联函数 + `.required()` 链式);`~standard` 对应一个 `StandardSchema` trait(`validate(&Value) -> Result<T, Vec<Issue>>`)。依赖 `dsh-cosmokit` + `serde`/`serde_json`。

### 2.3 @deepseek-ai/cordis-plugin-timer(vendor/timer,v1.1.3)
- **用途**:定时器服务,把 `setTimeout/setInterval` 包装为 fiber 拥有的可释放定时器。
- **workspace 依赖**:`@deepseek-ai/cordis`(peer)、`@deepseek-ai/cosmokit`。
- **src 模块**:`index.ts`(1 个)。
- **导出面**:`TimerService`(default,service 名 `timer`)——`timeout/interval/throttle/debounce/setTimeout/setInterval`,并 `mixin` 到 ctx;声明 `Context` 增补 `timer` 及 `Pick<TimerService, …>`。effect 内注册定时器,卸载清空。
- **Rust 建议**:`crates/vendor/timer`(命名 `dsh-cordis-timer`)。用 `tokio::time`。低难度,适合作为 cordis 之上的第一个插件级 smoke test。

### 2.4 @deepseek-ai/cordis-plugin-loader(vendor/loader,v1.0.2)
- **用途**:插件加载器 —— 拥有 entry 树,按名解析/import 插件并 `ctx.plugin()`。
- **workspace 依赖**:`@deepseek-ai/cordis`(peer)、`@deepseek-ai/cosmokit`;optional peer `node-addon-require-builtin`。
- **src 模块**:`index.ts`、`internal.ts`、`config/entry.ts`、`config/group.ts`、`config/isolate.ts`、`config/tree.ts`、`config/utils.ts`(7 个)。
- **导出面**:
  - `Loader`(default,service 名 `loader`,继承 `EntryTree`):`name`、`internal`(Node 内部 `ModuleLoader`)、`builtins`(include/group 内置)、`envData`、`import/load`、`write()`(内存树 no-op)、`[Service.check]`(`await` 拦截:依赖方等 loader 任务排空)、`showLog`、`locate`、`exit`、`unwrapExports`。
  - `Entry` / `EntryOptions`(config/entry.ts):单个 entry 节点。
  - `EntryGroup`(config/group.ts):嵌套组;`Group`(default,静态 `[EntryGroup.key]`)。
  - `isolate`(config/isolate.ts,default plugin)+ `LocalRealm`/`GlobalRealm`(继承 `Realm`)。
  - `EntryTree`(config/tree.ts):树的持久化/事务更新。
  - `interpolate`/`isJsExpr`/`JsExpr`/`evaluate`(config/utils.ts):`!!js` 表达式求值。
  - `ModuleLoader`/`ModuleJob`/`ModuleWrap`/`ResolveResult`/`LoadResult`/`ModuleLoaderV1/V2`/`ModulePhase`(internal.ts,Node 22–24 内部模块加载器兼容)。
  - 事件:`exit`、`loader/config-update`、`loader/entry-init`、`loader/partial-dispose`、`loader/patch-context`;Context 增补 `loader`;Fiber 增补 `entry`。
- **Rust 建议**:`crates/vendor/loader`(命名 `dsh-cordis-loader`)。难点最高之一:**Node ESM/CJS 动态 import + 内部 ModuleLoader** 在 Rust 无等价物 —— 移植策略应是"静态注册表/显式模块清单"替代动态 import(`builtins` 表 + 声明式 entry),`!!js` 表达式求值需自研微型解释器或换配置 DSL。`isolate`/`Group`/`Include` 的服务隔离语义要在 `dsh-cordis` 的 isolate 机制上落地。

### 2.5 @deepseek-ai/cordis-plugin-include(vendor/include,v1.0.6)
- **用途**:YAML/JSON 配置文件 include;实现 entry-list YAML 方言(`!!js` 表达式)与 patch 语义。
- **workspace 依赖**:`@deepseek-ai/cordis-plugin-loader`(peer)、`@deepseek-ai/cordis`(peer)、`@deepseek-ai/cosmokit`、`js-yaml`。
- **src 模块**:`index.ts`(1 个)。
- **导出面**:
  - `Include`(default,继承 `EntryTree`,静态 `[EntryGroup.key]`):`filename`、`refresh()`、`stop()`、`write()`;`Include.Config{path, initial?, patches?, enableLogs?}`。
  - `applyEntryPatches(data, patches, warn)`:**THE patch 语义**(id 定位覆盖 + `insert` 插入 + `group` 转换 + `disabled`/`name` 校验),离线 `dsh --dump-config` 与 boot 共用同一实现。
  - `PatchOptions`、`entryListSchema`(YAML `!!js` 方言 schema)。
- **Rust 建议**:`crates/vendor/include`(命名 `dsh-cordis-include`)。难点:YAML 解析(`serde_yaml`)+ `!!js` 表达式 round-trip + 事务式 apply/rollback。依赖 loader 的 entry 树 API。

### 2.6 @deepseek-ai/cordis-plugin-group(vendor/group,v1.0.1)
- **用途**:嵌套插件组(`cordis:group` 内置),给一组插件一个 `isolate` realm。
- **workspace 依赖**:`@deepseek-ai/cordis-plugin-loader`(peer)、`@deepseek-ai/cordis`(peer)。
- **src 模块**:`index.ts`(1 个,`export default Group`,re-export 自 loader 的 `Group`)。
- **导出面**:`Group`(来自 loader 的 `EntryGroup`)。
- **Rust 建议**:`crates/vendor/group`(命名 `dsh-cordis-group`)。薄封装,随 loader 一起做。

### 2.7 @deepseek-ai/cordis-plugin-hmr(vendor/hmr,v1.0.16)
- **用途**:模块热替换;watch 文件、分类 accepted/declined、清 ESM/CJS 缓存、重 import 并重载插件。
- **workspace 依赖**:`@deepseek-ai/cordis-plugin-timer`(peer)、`@deepseek-ai/cordis`(peer)、`@babel/code-frame`、`chokidar`、`@deepseek-ai/cosmokit`、`picomatch`、`@deepseek-ai/schemastery`。
- **src 模块**:`index.ts`、`error.ts`(2 个)。
- **导出面**:
  - `Hmr`(default,service 名 `hmr`,`static inject = ['loader','timer']`):`registerConfig(filename, refresh)`、`getLinked`、`partialReload`、`Hmr.Config{base?, root, ignored, debounce}`(`z` schema)。
  - `handleError`(error.ts)。
  - 事件:`hmr/change`、`hmr/reload`、`hmr/config-update-failed`;Context 增补 `hmr`。
- **Rust 建议**:`crates/vendor/hmr`(命名 `dsh-cordis-hmr`)。Node 模块缓存重载在 Rust 无对应物 —— 除非 Rust 侧采用**可重载的动态库(如 `libloading`)或进程内脚本引擎**,否则 HMR 只能降级为"配置/插件清单热重载"(watch + 重建 fiber)。建议标记为**低优先级/可裁剪**。

### 2.8 @deepseek-ai/cordis-plugin-logger-console(vendor/logger-console,v1.0.1)
- **用途**:控制台日志导出器(node 用 `util.inspect`,browser 分支)。
- **workspace 依赖**:`@deepseek-ai/cordis`(peer)、`@deepseek-ai/cosmokit`、`@deepseek-ai/schemastery`、`supports-color`。
- **src 模块**:`index.ts`、`browser.ts`、`shared.ts`(3 个)。
- **导出面**:`ConsoleExporter`(default,继承 shared 的 `ConsoleExporter`,覆写 `o/O` formatter 为 `util.inspect`);`shared.ts` 的 `ConsoleExporter` base(`LabelStyle`、`ColorSupportLevel`、`ConsoleExporter.Config`)。
- **Rust 建议**:`crates/vendor/logger-console`(命名 `dsh-cordis-logger-console`)。对应 `tracing-subscriber` fmt 层或自写 exporter;低难度。

---

## 3. packages/core(8 个)

### 3.1 @deepseek-ai/dsh-scope(packages/core/scope)
- **用途**:作用域上下文注册原语 —— 铸造带不透明身份 tag 的 cordis 上下文 + 路由型事件 carrier(scope-filtered dispatch 的基础)。
- **workspace 依赖**:`@deepseek-ai/dsh-invariants`(peer)、`@deepseek-ai/cordis`(peer)。
- **src 模块**:`index.ts`、`store.ts`、`scoped-events.generated.ts`、`invariant.ts`(4 个)。
- **导出面**:
  - `createScope(ctx, key, options?)` → `Scope{ctx, rawDispose, dispose}`(以空 plugin 为 backing fiber)。
  - `scopeTarget(base, key)` → `Scoped<T>` carrier(实现 `[Context.filter]`:base filter + scope chain 向上匹配);`scopeOf`、`isScopeCarrier`、`carrierKeyOf`。
  - scope 链:`bindScopeParent`/`ScopeParentBinding.rebind`、`scopeParentOf`、`scopeChainOf`(cycle 检测;注册向下继承、事件向上传播)。
  - `store.ts`:`ScopeLayer`、`NamedEntries<V>`、`AnonymousEntries<V>`、`ScopedLayers<L>`(按 scope 分层的注册存储)。
  - 类型:`ScopeKey`、`Scoped<T>`、`CreateScopeOptions`。
- **Rust 建议**:`crates/core/scope`(命名 `dsh-scope`)。这是上层一切"作用域/agent 隔离"的基石,须紧跟 cordis 之后移植。难点:scope chain 的 WeakMap 语义(Rust 用 `Arc<dyn Any>` key + `Weak` parent 链)、`ScopedLayers` 分层存储。

### 3.2 @deepseek-ai/dsh-session(packages/core/session)
- **用途**:事件溯源(event-sourced)会话存储。
- **workspace 依赖**:`@deepseek-ai/dsh-brand`、`dsh-invariants`、`dsh-llm`、`dsh-scope`、`dsh-typert-protocol`(均 peer)、`@deepseek-ai/cordis`(peer)。
- **src 模块**:`index.ts`、`types.ts`、`json.ts`、`preparation.ts`、`repair.ts`、`chunk-rows.ts`、`surface.ts`、`request-header.ts`、`known-event-types.ts`、`invariant.ts`(10 个)。
- **导出面**:
  - `SessionStore`(default,service 名 `sessions`):`prepare/create/restore`、`get/list/roots`、`enter/announce/detach`、`flush`、`isOwnedBy`;事件 `session/created`、`session/disposed`、`session/event`、`session/flush`(scope-filtered)。
  - `Session`(类,`ctx.sessions` 的条目):事件日志、`seq`、`append`、`followup`、header。
  - `SessionId`(branded id)、`SessionHeader`、`CreateSessionOptions`/`RestoredSessionOptions`/`PrepareSessionOptions`、`SessionEventMap`(全事件词汇表)/`SessionEvent<T>`/`SessionEventType`。
  - `SessionPreparation`(Disposable 事务边界)、`snapshotJsonValue`/`isJsonValue`/`JsonValue`、`packChunkRuns`/`decodeStorageRecord`/`ChunkRow`/`StorageRecord`、`interruptedTurnClosers`/`TOOL_NOT_STARTED`/`TOOL_OUTCOME_UNKNOWN`、`foldSurface`/`SurfaceManager`/`SessionSurface`、`canonicalHeader`/`foldRequestHeader`/`headerEquals`、`KNOWN_SESSION_EVENT_TYPES`、`adoptSessionEvent`/`snapshotSessionEvent`、`SessionForkError`/`SessionForkSource`。
- **Rust 建议**:`crates/core/session`(命名 `dsh-session`)。难点:事件溯源 append-only 日志 + 序列化边界(`serde_json` preserve_order)+ branded id + 快照/折叠。依赖 scope + invariants + llm 类型。

### 3.3 @deepseek-ai/dsh-system-prompt(packages/core/system-prompt)
- **用途**:系统提示词组装注册表 —— 有序 section、动态 context、工具 schema、prompt 变量。
- **workspace 依赖**:`@deepseek-ai/dsh-invariants`、`dsh-llm`、`dsh-scope`(peer)、`@deepseek-ai/cordis`(peer)、`@deepseek-ai/schemastery`(dep)。
- **src 模块**:`index.ts`、`invariant.ts`(2 个)。
- **导出面**:
  - `SystemPrompt`(default,service 名 `systemPrompt`):`section/context/suppressRuntimeContext/tools/variable` 注册 + `assemble(context)` 组装;`Config{includeHarnessIdentity, includeRuntimeContext, persona, toolOrder}`(z schema)。
  - 事件 `system-prompt/assemble`(waterfall,scope-filtered)、`system-prompt/change`。
  - 常量:`PERSONA_SECTION='deployment:persona'`、`PERSONA_ORDER=0`、`TOOL_ORDER_REST='<unlisted-tools>'`。
  - 渲染:`renderPrompt`、`renderContextSnapshot`、`renderContextSections`、`joinContextSections`。
  - 类型:`PromptSection/PromptContext/AssembledSection/AssembledContext/PromptAssembly/AssembleContext/ToolProviderResult`。
- **Rust 建议**:`crates/core/system-prompt`(命名 `dsh-system-prompt`)。难点:变量插值(严格 `{{name}}`)+ 工具排序 + 分层 scope 阴影;依赖 scope + schemastery。

### 3.4 @deepseek-ai/dsh-agent(packages/core/agent)
- **用途**:Agent 接口、注册表、initiator 作用域与事件词汇表(创建委托给 agent-loop)。
- **workspace 依赖**:`@deepseek-ai/dsh-invariants`、`dsh-llm`、`dsh-scope`、`dsh-session`、`dsh-system-prompt`、`dsh-typert-protocol`(peer)、`@deepseek-ai/cordis`(peer)。
- **src 模块**:`index.ts`、`runtime-types.ts`、`types.ts`、`inbox.ts`、`consumed-work.ts`、`model-selection.ts`、`dispatch.ts`、`invariant.ts`(8 个)。
- **导出面**:
  - `AgentRegistry`(default,service 名 `agents`):`create/resume`(经 `AgentFactory`)、`register/enter/announce/get/list/roots/isOwnedBy`、`currentInitiator/requireInitiator/withInitiator/withoutInitiator`(AsyncLocalStorage 因果归属)、`setFactory`。
  - 类型:`Agent`、`AgentOptions`、`CreateAgentOptions`/`ResumeAgentOptions`、`AgentHandle`、`AgentFactory`、`AgentSetup`/`AgentSetupCommit`;`agentCarrier`/`agentEvents`/`assembleContextFor`/`emitAgentEvent`/`AgentEventDispatch`/`AgentSubjectEvent`(dispatch.ts)。
  - Context 增补 `agents`、`agent?`;事件 `agent/created`/`agent/disposed`/`agent/session-start`(见 dispatch/events)。
- **Rust 建议**:`crates/core/agent`(命名 `dsh-agent`)。难点:AsyncLocalStorage 的 initiator 传播 → `tokio` 的 task-local(spawn 时显式携带),`AgentRegistry` 的 rollback 覆盖的 create/resume 事务。依赖 scope + session + system-prompt。

### 3.5 @deepseek-ai/dsh-tools(packages/core/tools)
- **用途**:工具注册表与执行管线;tool schema(→ JSON Schema / TS / Python SDK)、presentation、code-mode 桥。
- **workspace 依赖**:`@deepseek-ai/dsh-agent`、`dsh-code-runtime`、`dsh-invariants`、`dsh-llm`、`dsh-scope`、`dsh-session`、`dsh-system-prompt`、`dsh-user-approval`(peer)、`@deepseek-ai/cordis`(peer)、`@deepseek-ai/schemastery`(dep)。
- **src 模块**:`index.ts`、`types.ts`、`schema.ts`、`json-schema.ts`、`ts-types.ts`、`py-types.ts`、`code-mode.ts`、`presentation.ts`、`testing.ts`、`invariant.ts`(10 个)。
- **导出面**:
  - `ToolRuntime`(default,service 名 `tools`):工具注册/执行调度管线、`presentAs(mode)`、`restrict()`、guards、tool-call 调度(scheduler token `TOOL_RUNTIME_SCHEDULER`)。
  - schema 体系:`ValueSchemaSpec`/`ParameterSchemaSpec`/`ParameterJsonSchema`/`InferValue`/`InferArgs`、`valueSchemaSpecToJsonSchema`/`parameterSchemaSpecToJsonSchema`、`validateArgs`/`ToolArgsError`、`defineTool`/`DefineToolOptions`。
  - `json-schema.ts`:`JsonSchemaNode`/`ObjectJsonSchema`/`JsonSchemaError`/`assertSupportedJsonSchema`/`validateJsonSchemaValue`。
  - code-mode:`RUN_CODE_NAME='run_code'`、`SDK_SECTION_ORDER=150`、`createRunCodeTool`、`CodeRunFailedError`、`CodeSdkLanguage`。
  - SDK 生成:`jsonSchemaToTs`/`renderToolsSdk`/`jsonSchemaToPy`/`renderToolsSdkPy`。
  - presentation:`ToolCallKind`/`FileLocation`/`FileDiff`/`ToolCallView`/`ToolResultView`/`WebSource` 等 UI 展示类型。
  - 执行结果:`ToolResult`/`ToolExecution`/`ToolExecutionResult`/`ToolExecutionSuccess|Failure`/`ToolErrorInfo`/`ToolFailure`/`ToolNotFoundError`/`ToolOutputError`/`TOOL_ABORTED`/`TOOL_ABORTED_BEFORE_DISPATCH`/`ToolPresentationMode`/`ToolGuard`/`ToolRestriction`。
- **Rust 建议**:`crates/core/tools`(命名 `dsh-tools`)。难点:参数 schema→JSON Schema/TS/Py 三端代码生成、工具调度器、presentation 视图模型。依赖 agent + session + system-prompt + scope + schemastery。

### 3.6 @deepseek-ai/dsh-agent-loop(packages/core/agent-loop)
- **用途**:具体 agent loop 插件 —— 构造 scoped `ReactLoopAgent`,经 agent/session 注册表发布,拥有有序 teardown。
- **workspace 依赖**:`@deepseek-ai/dsh-agent`、`dsh-invariants`、`dsh-llm`、`dsh-scope`、`dsh-session`、`dsh-session-persistence`、`dsh-system-prompt`、`dsh-tools`、`dsh-settings`(peer)、`@deepseek-ai/cordis`(peer)、`@deepseek-ai/schemastery`(dep)。
- **src 模块**:`index.ts`、`agent.ts`、`runtime-context.ts`、`tool-calls.ts`、`constants.ts`、`invariant.ts`(6 个)。
- **导出面**:
  - `AgentLoop`(default,service 名 `agentLoop`,`static inject=['agents','sessions','llm','tools','systemPrompt']`):`create`、`createAgent`、`resume`/`resumeWith`(实现 `AgentFactory`)、`prepare`/`setupAndPublish`(rollback 覆盖)。
  - `Config{maxParallelToolCalls?, agents: [...]}`(z schema)、`DEFAULT_MAX_PARALLEL_TOOL_CALLS`、`LauncherAgentIdentity`、`ConfiguredAgentIdentities`、`CONFIGURED_AGENT_IDENTITIES_KEY`、`AGENT_LOOP_SETTINGS_NAMESPACE/SCHEMA`、`AgentLoopSettings`。
  - 事件 `agent-loop/config-start-failed`;Context 增补 `agentLoop`、`configuredAgentIdentities?`。
- **Rust 建议**:`crates/core/agent-loop`(命名 `dsh-agent-loop`)。难点:React loop 状态机、工具调用调度(`tool-calls.ts`)、create/resume 事务与 abort 融合(AbortController → `tokio_util::sync::CancellationToken`)。依赖 agent+session+tools+system-prompt。

### 3.7 @deepseek-ai/dsh-agent-default-model(packages/core/agent-default-model)
- **用途**:Agent 默认模型选择(无 session 选择时的 fallback)。
- **workspace 依赖**:`@deepseek-ai/dsh-agent`、`dsh-invariants`、`dsh-llm`、`dsh-settings`(peer)、`@deepseek-ai/cordis`(peer)、`@deepseek-ai/schemastery`(dep)。
- **src 模块**:`index.ts`、`invariant.ts`(2 个)。
- **导出面**:`AgentDefaultModelConfig`(default,service 名 `agentDefaultModel`):`currentSelection()`、`saveSelection()`;`Config{provider, model}`(z schema)、`AGENT_DEFAULT_MODEL_SETTINGS_NAMESPACE/SCHEMA`、`AgentDefaultModelSettings`。
- **Rust 建议**:`crates/core/agent-default-model`(命名 `dsh-agent-default-model`)。低难度,依赖 agent + settings。

### 3.8 @deepseek-ai/dsh-agent-tool-presentation(packages/core/agent-tool-presentation)
- **用途**:Agent 面工具呈现选择器(native / code / both),一个 preset 一行。
- **workspace 依赖**:`@deepseek-ai/dsh-invariants`、`dsh-tools`(peer)、`@deepseek-ai/cordis`(peer)、`@deepseek-ai/schemastery`(dep)。
- **src 模块**:`index.ts`、`invariant.ts`(2 个)。
- **导出面**:`name='tool-presentation'`、`inject=['tools']`、`Config{mode: ToolPresentationMode}`(z schema)、`apply(ctx, config)`(调 `ctx.tools.presentAs(mode)`;code 模式 `ctx.inject(['codeRuntime'], …)`)。
- **Rust 建议**:`crates/core/agent-tool-presentation`(命名 `dsh-agent-tool-presentation`)。薄插件,依赖 tools。

---

## 4. packages/boot(2 个)

### 4.1 @deepseek-ai/dsh-cmdline(packages/boot/cmdline)
- **用途**:把 launcher 的不可变命令行交给 app 插件(`cmdlineArgs`/`appExit` 服务)。
- **workspace 依赖**:`@deepseek-ai/cordis-plugin-loader`(peer)、`@deepseek-ai/dsh-invariants`(peer)、`@deepseek-ai/cordis`(peer)。
- **src 模块**:`index.ts`、`invariant.ts`(2 个)。
- **导出面**:`CmdlineArgs`、`AppExit`、`CmdlineHost`、`provideCmdline(ctx, host)`、`parseCmdline(ctx, program)`、`internals`(stdout/stderr);Context 增补 `cmdlineArgs?`、`appExit?`。
- **Rust 建议**:`crates/app/cmdline`(命名 `dsh-cmdline`;boot 组无独立 member glob,建议归 `crates/app/*`)。依赖 commander → Rust 用 `clap`;低难度。

### 4.2 @deepseek-ai/dsh-app-boot(packages/boot/app-boot)
- **用途**:app bin 的共享 boot 胶水:.env 加载、fail-loud Loader 守卫、snapshot 感知配置解析、profile patch 层、Loader boot 序列。
- **workspace 依赖**:`@deepseek-ai/cordis-plugin-group/hmr/include/loader`、`dsh-launch-environment`、`dsh-invariants`、`dsh-home-paths`、`dsh-system-prompt`(peer)、`@deepseek-ai/cordis`(peer)、`js-yaml`(dep)。
- **src 模块**:`index.ts`、`profile.ts`、`invariant.ts`(3 个)。
- **导出面**:
  - `boot(binName, configPath, patches?, prepare?, bareModuleBaseUrl?)` → 根 Context(核心入口);`mountRootInclude`、`resolveConfigPath`、`loadEnv`、`loadLayeredEnv`、`loadOptionalPatches`/`loadOverlayPatches`、`renderConfigDump`、`watchUserPatches`、`installFailLoud`/`FAIL_LOUD_RELEASE_TIMEOUT_MS`、`assertEntriesLoaded`/`assertEntriesActivated`、`addHarnessSourceSection`/`HARNESS_SOURCE_SECTION`、`UserPatchWatchOptions`、`ConfigDumpLayer`、`FailLoudProcess`。
  - `profile.ts`:`composeEntries`、`DEFAULT_PROFILE_BUNDLES`、`PROFILE_PATCH_FILENAME`、`PROFILE_TEMPLATES`、`PROFILES_DIR`、`initProfile/loadProfile`、`readProfileManifest/writeProfileManifest`、`resolveBundleDir/resolveProfileDir`、`healProfilesModuleFallback`、`DshBundleManifest/DshProfileManifest/DshManifestSection/Profile/ProfileLayer/ProfileManifest`。
- **Rust 建议**:`crates/app/app-boot`(命名 `dsh-app-boot`)。难点:`.env` 分层加载、profile 清单 YAML、config dump、fail-loud 进程守卫(对应 `tokio::signal` + `tracing`);依赖 loader/include/group + home-paths。

---

## 5. packages/bundle(3 个)

> 三个 bundle 的实质都在 `cordis.patch.yml`(由 `dsh.bundle.patch` manifest 字段声明),`src/index.ts` 多数只携带少量运行时胶水。`dsh-base` 的 patch 是"每个 profile 的第一 patch 层",把基础插件行插到空 profile 根上。

### 5.1 @deepseek-ai/dsh-base(packages/bundle/base)
- **用途**:共享 dsh core 的 profile bundle(纯 `cordis.patch.yml`,无运行时 API)。
- **workspace 依赖**:几乎全部 core/feature 插件(118 项 dependencies,见 package.json)——包括 cordis-plugin-hmr/timer、agent、agent-loop、session、system-prompt、tools、fs、goal、subagent、llm 系列、skill、storage 等;peer `dsh-invariants`、`cordis`。
- **src 模块**:`index.ts`、`invariant.ts`(2 个;`index.ts` 仅 `export {}`)。
- **导出面**:无(声明式 patch 清单)。
- **Rust 建议**:`crates/app/base`(命名 `dsh-base`,patch 清单 → Rust 侧配置/清单文件)。实质是把全量基础插件行声明为一个可组合的 profile;移植时对应一份"默认 feature 组合"。

### 5.2 @deepseek-ai/dsh-headless(packages/bundle/headless)
- **用途**:一次性直接 Agent 驱动(无 Host/HTTP/浏览器层),跑一个 task 后 flush session、打印末条 assistant 文本并退出。
- **workspace 依赖**:`@deepseek-ai/dsh-cmdline`、`dsh-code-runtime-worker-thread`、`@deepseek-ai/schemastery`、`commander`(dep);peer `cordis-plugin-loader`、`dsh-agent`、`dsh-agent-default-model`、`dsh-invariants`、`dsh-llm`、`dsh-session`、`cordis`。
- **src 模块**:`index.ts`、`startup.ts`、`invariant.ts`(3 个)。
- **导出面**:`name='headless-runner'`、`inject=['agentDefaultModel','agents','sessions']`、`Config{task}`(z schema)、`apply(ctx, config)`、`internals`;`startup.ts`(启动入口)。
- **Rust 建议**:`apps/headless`(命名 `dsh-headless`)。适合作为 Rust 侧首个端到端 smoke bin;依赖 core 全套。

### 5.3 @deepseek-ai/dsh-web-app(packages/bundle/web-app)
- **用途**:浏览器面 bundle 的运行时胶水插件:解析前端 dist、注册 web-surface prompt、`DSH_WEB_URL` 变量、URL 行。
- **workspace 依赖**:`dsh-agent-presets`、`dsh-api-remotes`、`dsh-app-boot`、`dsh-cmdline`、`dsh-cordis-host/client-runner`、大量 `dsh-client-*` UI 插件、`dsh-host-*`(webserver/apiproxy/frontend-static 等)、`dsh-storage-*`、`dsh-workspace`、`@deepseek-ai/schemastery`、`commander`(dep);peer `cordis-plugin-loader`、`dsh-shell-env`、`dsh-invariants`、`dsh-system-prompt`、`cordis`。
- **src 模块**:`index.ts`、`startup.ts`、`invariant.ts`(3 个)。
- **导出面**:`name='web-app'`、`inject=['webServer']`、`Config{printUrl, surfaceContext, trustedHosts}`(z schema)、`apply(ctx, config)`、`resolveLanTrust`、`WebRuntimeValues`、`internals`。
- **Rust 建议**:`apps/web-app`(命名 `dsh-web-app`)。依赖 axum(已在 workspace deps)+ host 层;优先级低于 headless,可后置。

---

## 6. Rust 移植建议汇总

### 6.1 crate 路径映射(对齐已声明的 workspace member glob)

| TS 包 | 建议 crate 路径 | 建议 crate 名 |
|---|---|---|
| vendor/cordis | `crates/vendor/cordis` | `dsh-cordis` |
| vendor/cosmokit | `crates/vendor/cosmokit` | `dsh-cosmokit` |
| vendor/schemastery | `crates/vendor/schemastery` | `dsh-schemastery` |
| vendor/loader | `crates/vendor/loader` | `dsh-cordis-loader` |
| vendor/include | `crates/vendor/include` | `dsh-cordis-include` |
| vendor/group | `crates/vendor/group` | `dsh-cordis-group` |
| vendor/hmr | `crates/vendor/hmr` | `dsh-cordis-hmr` |
| vendor/timer | `crates/vendor/timer` | `dsh-cordis-timer` |
| vendor/logger-console | `crates/vendor/logger-console` | `dsh-cordis-logger-console` |
| packages/core/scope | `crates/core/scope` | `dsh-scope` |
| packages/core/session | `crates/core/session` | `dsh-session` |
| packages/core/system-prompt | `crates/core/system-prompt` | `dsh-system-prompt` |
| packages/core/agent | `crates/core/agent` | `dsh-agent` |
| packages/core/tools | `crates/core/tools` | `dsh-tools` |
| packages/core/agent-loop | `crates/core/agent-loop` | `dsh-agent-loop` |
| packages/core/agent-default-model | `crates/core/agent-default-model` | `dsh-agent-default-model` |
| packages/core/agent-tool-presentation | `crates/core/agent-tool-presentation` | `dsh-agent-tool-presentation` |
| packages/boot/cmdline | `crates/app/cmdline` | `dsh-cmdline` |
| packages/boot/app-boot | `crates/app/app-boot` | `dsh-app-boot` |
| packages/bundle/base | `crates/app/base` | `dsh-base` |
| packages/bundle/headless | `apps/headless` | `dsh-headless` |
| packages/bundle/web-app | `apps/web-app` | `dsh-web-app` |

> boot/bundle 两组没有对应 member glob(`crates/boot`、`crates/bundle` 不存在),故建议并入 `crates/app/*` 与 `apps/*`;最终以 Rust 仓库 owner 的布局决策为准。

### 6.2 移植顺序(自底向上,每批内可并行)

1. **地基批**:`dsh-cosmokit` → `dsh-schemastery` → `dsh-cordis`(核心,含 Context/Service/Reflect/Registry/Fiber/Events/Logger/traceable)。
2. **cordis 插件批**:`dsh-cordis-timer`、`dsh-cordis-logger-console`、`dsh-cordis-loader`(先做 entry 树与 `builtins`,动态 import 换成静态注册表)→ `dsh-cordis-include`、`dsh-cordis-group`;(`dsh-cordis-hmr` 可裁剪/后置)。
3. **core 批**:`dsh-scope` → `dsh-session`、`dsh-system-prompt` → `dsh-agent`、`dsh-tools` → `dsh-agent-loop`、`dsh-agent-default-model`、`dsh-agent-tool-presentation`。
4. **boot/app 批**:`dsh-cmdline` → `dsh-app-boot` → `dsh-base` → `apps/headless` → `apps/web-app`。

### 6.3 全局难点清单

- **动态性 → 静态性**:TS 的 Proxy、鸭子类型、`any` 服务、动态 import、AsyncLocalStorage、WeakMap 作用域链,均无 Rust 直接等价物。移植原则:
  - 服务 → `Arc<dyn Any>` downcast 或 trait object + `TypeId` 注册表;
  - call interception → 服务方法显式携带 caller 上下文(而非 proxy 重绑定);
  - scope/isolate/intercept 作用域 → 不可变共享 map + 写时复制(`Arc`/`im` 风格);
  - AsyncLocalStorage → `tokio` task-local / 显式参数;
  - 动态 import(loader/hmr)→ 静态模块注册表 / 声明式清单(必要时 `libloading`)。
- **effect 生命周期**:fiber 的 epoch 状态机 + disposer 逆序释放 → RAII guard + `JoinHandle`,异步卸载顺序需保留(reverse order)。
- **schema 体系**:`dsh-schemastery` 需同时支撑 config 校验(Standard Schema 语义)与 JSON Schema/TS/Python 三端代码生成(见 tools)。
- **事件溯源 session**:append-only + 快照/折叠 + branded id + 严格 JSON 边界,`serde_json` 需 `preserve_order`(workspace 已配)。
- **可裁剪项**:`dsh-cordis-hmr`(依赖 Node 模块缓存)、console logger 的 ANSI 细节、`util.inspect` 对象格式化,可先做最小实现。

### 6.4 现有 workspace 已备依赖(复用,勿重复引入)

`tokio`(全功能)、`serde`/`serde_json`(preserve_order)/`serde_yaml`、`anyhow`/`thiserror`、`tracing`/`tracing-subscriber`、`async-trait`、`parking_lot`/`dashmap`/`once_cell`、`regex`、`chrono`、`uuid`、`futures`、`indexmap`、`axum`+`tower`+`http`(web 层)、`which`/`dirs`。schema 校验建议基于 `serde` 自研(对齐 schemastery 语义)而非引入第三方 validator,以便精确复刻 `intersect/transform/lazy/role/meta` 等行为。
