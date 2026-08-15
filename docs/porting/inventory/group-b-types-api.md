# 组 B 包级清单：类型系统 / API 面 / 通用

> 面向「后端 Rust 1:1 移植」的只读盘点。覆盖 8 个顶层包目录：`typert`、`util`、`api`、`sdk`、`preset`、`identity`、`settings`、`session-query`。
>
> **结构说明（重要）**：本仓库是「两层嵌套 workspace」——根 `package.json` 的 `workspaces` 声明为 `packages/*/*`。因此每个顶层目录（如 `packages/api`）本身**不是**一个 npm 包，而是包含多个子包。任务描述中的 `packages/api/src/index.ts` 之类路径实际不存在；真实入口在子包层（如 `packages/api/gateway/src/index.ts`）。下文的「包」均按 npm 子包（`@deepseek-ai/*`）粒度拆解，顶层目录作为分组标题。
>
> 通用约定：除 `util` 家族与少数纯协议包外，几乎每个子包都带一个 `src/invariant.ts`（Cordis 插件，导出 `name` / `inject: ['invariants']` / `apply`），用于在启动时注册进程级不变式；它对 Rust 移植来说对应「runtime 初始化校验」，非业务面。

---

## 1. `packages/typert` —— 编译器无关的类型系统 / Remote RPC 协议栈

一句话用途：**Typert 是贯穿整个仓库的“类型反射 + 跨进程 RPC”基础设施**——它把一个 Cordis Service 上被 `@Remote` 装饰的方法，在编译期生成「参数/返回值 codec 描述 + 调用描述符」，运行时据此完成 Host↔Client 的类型安全远程调用（含参数校验、lookup 对象解析、结果校验）。

### 子包

#### 1.1 `@deepseek-ai/dsh-typert-protocol`
- 用途：**编译器无关的 Remote 元数据与 provider 协议**（纯类型面 + 少量运行时工具，是整套协议的地基）。
- workspace 依赖：`@deepseek-ai/dsh-invariants`、`@deepseek-ai/cordis`（均 peer）。
- src 顶层模块：`index.ts`、`types.ts`、`invariant.ts`。
- 导出面（关键类型/接口）：
  - 类型级关联：`TypertLookup<Host, Wire>`、`TypertContext<Wire>`、`TypertLookupHost`/`TypertLookupWire`/`TypertContextWire`（用 phantom symbol + 条件类型做 Host 对象 ↔ wire 身份的静态绑定）。
  - 合并扩展映射表：`TypertLookupMap`、`TypertContextMap`、`TypertRemoteMap`、`TypertRemoteScopeMap`、`TypertRemoteNamespaceMap`、`TypertRemoteEventSelection`（`interface` 声明合并是扩展点）。
  - RPC 结果/事件：`RemoteFailure`、`RemoteResult<T>`、`TypertForwardableEvent`、`TypertRemoteEvent`、`TypertRemoteNamespace`、`TypertRemoteScopeNamespace`、`TypertRemoteScopeApi`。
  - 调用描述符与 codec：`InvocationDescriptor`、`InvocationParameterDescriptor`、`InvocationSourceLocation`、`TypertSchema`、`TypertCodec`（`strict` 带 Zod schema / `src-json`）、`TypertRemoteContribution`。
  - 注册表契约：`TypertLocalRegistry`、`TypertRemoteRegistry`、`TypertLookupRegistry`、`TypertContextRegistry`、`TypertRegistryContract`（`ctx.typert`）、`TypertRegistryChange`/`Listener`、`TypertDisposer`。
  - lookup/context provider：`TypertLookupResolver/Provider/Definition`、`TypertHostContextProvider/Resolver`、`TypertClientContextBinder`。
  - Client 能力：`TypertClientRemote`（`$mount` / `$on` / `$dispatch`）。
  - 运行时：`bindTypertRemote`、`TypertRemoteService`（抽象基类）、`Remote`/`RemoteScope`（方法装饰器）、`remoteMethods`、`isTypertRemoteSegment`、`TypertLookupFailure`、`TypertGatewayBinding`。

#### 1.2 `@deepseek-ai/dsh-typert-registry`
- 用途：**运行时注册表**——实现上面契约里的四个子注册表（local / remote / lookup / host+client context），并持有 Zod schema 反射。
- workspace 依赖：`@deepseek-ai/dsh-typert-protocol`、`zod`；peer `@deepseek-ai/dsh-invariants`、`@deepseek-ai/cordis`。
- src 顶层模块：`index.ts`、`service.ts`、`types.ts`、`invariant.ts`、`client/index.ts`。
- 导出面：`TypertRegistry`（Service，~720 行，核心实现）、`typertKey`/`typertPackageKey`/`typertEndpoint`；模型类型 `TypertFace`、`TypertPackageModel`、`TypertServiceModel`、`TypertEventModel`、`TypertObjectModel`、`TypertMemberModel`、`TypertTypeModel`、`TypertContribution`、`TypertSchemaRecord`、`TypertPackageRecord`、`TypertSchemaFilter`、`TypertPackageFilter` 等；`client/index.ts` 提供 web 端 `inject`/`apply`（`dsh.client` 平台=web）。

#### 1.3 `@deepseek-ai/dsh-typert-loader`
- 用途：**加载器集成**——校验生成的 Typert manifest 并把贡献注入 registry。
- workspace 依赖：`@deepseek-ai/schemastery`；peer `@deepseek-ai/cordis-plugin-loader`、`@deepseek-ai/dsh-invariants`、`@deepseek-ai/dsh-typert-registry`、`@deepseek-ai/cordis`。
- src 顶层模块：`index.ts`、`invariant.ts`。
- 导出面：`TYPERT_HOST_EXPORT`、`name`/`inject`/`Config`(zod)/`validateTypertManifest`/`apply`。

#### 1.4 `@deepseek-ai/dsh-typert-generator`
- 用途：**构建期 TypeScript 项目分析器 + 模型驱动的 artifact 生成器**（产生 `typert-protocol.d.ts` 与每包 Remote 描述；含 Cordis 目录生成）。
- workspace 依赖：`@jridgewell/gen-mapping`、`typescript`；peer `@deepseek-ai/dsh-invariants`、`@deepseek-ai/cordis`。
- src 顶层模块：`analyzer.ts`、`cordis-catalog.ts`、`emitter.ts`、`index.ts`、`invariant.ts`、`model.ts`、`renderer.ts`、`tsdown-plugin.ts`、`workspace.ts`。
- 导出面：`WorkspaceAnalyzer`/`WorkspaceCaches`/`TypertAnalysisError`、`FaceModelEmitter`/`TypertEmitError`/`ModelEmitResult`、`TypeGraphRenderer`、`WorkspaceTypertGenerator`、`projectCordisCatalog`/`collectServices`/`collectEvents`/`CordisCatalogProjector`、`typertPlugin`（tsdown 插件）、以及一整套 `*Model` 类型图（`TypeNodeModel`、`PackageModel`、`FaceModel`、`WorkspaceModel`、`ServiceModel`、`EventModel` 等，约 40+ 接口）。

### Rust 移植建议（typert）
- crate 路径：`typert-protocol`（trait + serde 契约，无业务依赖）→ `typert-registry`（依赖 protocol）→ `typert-loader`（依赖 registry）→ `typert-generator`（**构建期工具**，不产出运行时 crate）。
- 难点：
  1. `TypertLookup<Host, Wire>` 这类「类型级关联」没有 Rust 直接等价物——用 **trait + associated type**（如 `trait Lookup { type Host; type Wire; }`）替代；`TypertLookupMap` 的 interface 合并扩展在 Rust 里退化为「运行时注册表 + 生成代码注入 impl」。
  2. 条件类型（`X extends Y ? A : B`）、模板字面量类型（`` `${Namespace}/${Method}` ``）、`keyof` 重映射——Rust 无法表达，**只能在生成期展开**（proc-macro 或 build.rs，或直接复用 TS generator 输出协议 JSON 再由 Rust 端消费）。
  3. `@Remote` 装饰器 + `InvocationDescriptor` 的「参数 codec（strict Zod / src-json）+ lookup 参数」→ Rust 端需要一套 `serde` 动态 codec 抽象（参数按 name 映射 + 运行时校验）。
  4. generator 依赖 TS compiler API，**不要移植分析器本身**；要么保留 TS generator 作为“上游 codegen”，要么用 `syn` 重写分析（工作量大）。建议先固化为「协议 JSON 中间表示」。
- 依赖顺序提示：这是**最底层**的类型面，`api`、`sdk` 及几乎所有业务包都挂在它上面；必须在 porting 顺序里最先落地 `typert-protocol`（纯类型），再推进 registry。

---

## 2. `packages/util` —— 通用零依赖原语

一句话用途：**跨包共享的小型纯函数/类型原语**（路径、原子写、超时、输出保留、品牌类型等），无业务耦合。

### 子包（7 个，除 brand 外均带 `invariant.ts`）
| 子包 | 用途 | src 模块 | 关键导出 |
|---|---|---|---|
| `@deepseek-ai/dsh-brand` | 类型级 nominal 品牌 | `index.ts` | `Branded<B>`（`string & { readonly [BRAND]: B }`） |
| `@deepseek-ai/dsh-atomic-write` | 原子文件替换 | `index.ts` | `writeFileAtomic`、`withFileLock`、`WriteFileAtomicOptions` |
| `@deepseek-ai/dsh-home-paths` | DSH 家目录路径助手 | `index.ts` | `dshHomePath`、`resolveDshHome`、`defaultDshHome`、`expandHomePath`、`canonicalizeWatchPath`、`dshHomeDisplay`、`DSH_HOME_ENV`/`DSH_HOME_DIR_NAME` |
| `@deepseek-ai/dsh-launch-environment` | 记录每层来源的启动环境快照 | `index.ts` | `createLaunchEnvironmentSnapshot`、`LaunchEnvironmentSnapshot`/`Entry`/`Source`、`launchEnvironmentOf`、`DSH_LAUNCH_ENVIRONMENT_KEY` |
| `@deepseek-ai/dsh-native-command` | 无 shell 的 `execFile` 封装 | `index.ts` | `runNativeCommand`、`NativeCommandRunner` |
| `@deepseek-ai/dsh-output-retention` | 有界输出保留 | `index.ts` | `ItemRetainer<T>`、`TextRetainer`、`Omitted`、`RetainedItems`/`RetainedText`、`describeOmitted`、`formatRetentionNotice` |
| `@deepseek-ai/dsh-timeout` | 超时/截止原语 | `index.ts` | `TimeoutReason`、`clampTimeout`、`deadline`、`idleWatchdog`、`timeoutOf`、`MAX_TIMER_DELAY_MS` |

- 依赖：所有子包的 peer 依赖都是 `@deepseek-ai/dsh-invariants` + `@deepseek-ai/cordis`（仅 harness 契约依赖，运行期是「零外部依赖」语义）。

### Rust 移植建议（util）
- crate 路径：`brand`（最先）→ 其余 6 个平级小 crate（`atomic-write`、`home-paths`、`launch-environment`、`native-command`、`output-retention`、`timeout`）。可合并为一个 `dsh-util` crate + feature 开关，或保持一包一 crate（贴合 1:1）。
- 难点：`Branded<B>` 的 phantom 品牌 → Rust `newtype` + `PhantomData` 或 `#[transparent]` 包装（注意 `Branded` 是 `string & {...}`，Rust 需保留「可当 `&str` 用」的语义）；`withFileLock` 的独占锁语义 → `fs2`/`fslock`；`timeoutOf` 从 `AbortSignal.reason` 提取 → `tokio` 取消语义映射；每个包的 `invariant.ts` → 对应 runtime 校验函数。

---

## 3. `packages/api` —— agent 对外 API 契约面（**重点**）

一句话用途：**把“后端能做什么”以类型安全的 Remote 方法暴露给客户端/外部调用方**。分两层：`gateway` 是「传输无关的调用引擎」，`remotes` 是「BFF 装配层」（决定暴露哪些能力 + 如何定位 agent/session）。

### 3.1 `@deepseek-ai/dsh-api-gateway`
- 用途：**Typert Remote 的 Host 端调度器 + Client API 端点**（即 `TypertClientRemote` 的实现——`$mount`/`$on`/`$dispatch`，以及传输无关的 `invoke`）。
- workspace 依赖：`@deepseek-ai/dsh-typert-protocol`；peer `@deepseek-ai/dsh-client-connection`、`@deepseek-ai/dsh-invariants`、`@deepseek-ai/dsh-typert-registry`、`@deepseek-ai/cordis`。
- src 顶层模块：`index.ts`（`TypertGatewayService` ~685 行 + `TypertGatewayError`）、`types.ts`（`InvokeRemoteRequest`、`TypertGatewayErrorCode`、`TypertGateway` 契约 + `ctx.typertGateway`）、`invariant.ts`、`client/index.ts`（`inject=['typert','connection']`、`ClientRemote = TypertClientRemote`）。
- 导出面：`TypertGatewayService`（默认导出）、`TypertGatewayError`、`TypertGatewayErrorCode`（17 个错误码）、`InvokeRemoteRequest`、`TypertGateway`。
- 关键点：`InvokeRemoteRequest = { namespace, method, args, signal? }`；`invoke()` 负责「按 endpoint 找 descriptor → 解析 lookup/context → 校验 args codec → 调用 → 校验 result codec」，**不关心 HTTP/WS/stdio 等载体**（载体由 `client-connection` 包负责）。

### 3.2 `@deepseek-ai/dsh-api-remotes`
- 用途：**Remote BFF 装配 + Host Agent/Session lookup 策略**——决定把哪些 Host 包的 Remote 贡献挂到客户端，并实现「从 wire id 解析到具体 agent/session」的归属策略（含 subagent 所有权校验）。
- workspace 依赖：`@deepseek-ai/dsh-typert-protocol`；peer 一大串：`cordis`、`dsh-agent`、`dsh-api-gateway`、`dsh-commands`、`dsh-credentials`、`dsh-goal`、`dsh-cordis-host-runner`、`dsh-host-plugin-inventory`、`dsh-invariants`、`dsh-agent-presets`、`dsh-llm`、`dsh-message-feedback`、`dsh-session`、`dsh-session-persistence`、`dsh-settings`、`dsh-typert-registry`。
- src 顶层模块：`index.ts`、`agent-lookup.ts`、`remote-events.ts`、`types.ts`、`invariant.ts`、`client/index.ts`。
- 导出面：
  - 事件白名单：`API_REMOTE_FORWARDED_EVENTS`、`ApiRemoteForwardedEvent`（编译期 `satisfies TypertForwardableEvent[]` 门禁）。
  - lookup 策略：`ApiRemoteSessionNotFound`、`ApiRemoteSubagentSessionOwnership`、`apiRemoteSubagentOwnershipError`、`createApiRemoteAgentResolver`、`hasApiRemoteSubagentOwner`、`inspectApiRemoteSession`、`ApiRemoteAgentOptions`/`ApiRemoteAgentResult`/`ApiRemoteLookupError`。
  - `client/index.ts`：**Client Remote 装配**——`$mount` 这 5 个命名空间：`commands`、`goal`、`cordis-host-runner`（动态插件）、`host-plugin-inventory`、`message-feedback`；并**海量 re-export** 客户端契约类型（`ClientResponse`、`ContentBlock`、`SessionId`、`WorkspaceView`、`SubagentCatalog`、`JobView`、`ToolCallView`、`RpcRequest/Response` 等来自 `client-connection`）+ 动态插件类型（`DynamicCordisPackage`、`CordisInspectProviderView` 等来自 `cordis-host-runner/types`）+ `JsonValue`（来自 `dsh-session/types`），并声明 `ctx.remote: TypertClientRemote`。

### api 与 sdk 的内容边界（特别说明）
- **`api`（gateway + remotes）= 进程内的「能力暴露面」**：它回答「后端有哪些方法可被外部调用、参数如何校验、结果如何校验、事件如何转发、调用如何定位到 agent/session」。它绑定在 Cordis 运行时内，产出的是 `ctx.remote` 这种**进程内类型安全 RPC 面**（Client 侧直接调 `ctx.remote.commands.xxx(...)`）。
- **`sdk` = 进程外的「运行时可编程面」**：它把整个 harness 当作一个子进程，通过 **stdio JSON-RPC** 从任意外部进程驱动 agent turn（`DeepSeekHarness.run(...)`），与 `api` 的进程内 RPC 是**两条不同通道**。二者唯一共享的是底层的 `@deepseek-ai/cordis` 与部分业务类型（`ContentBlock`、`SessionEvent`、`SessionId`）。
- `api` 面向「harness 自身 Client 端与宿主集成」；`sdk` 面向「外部开发者/脚本以库方式驱动 harness 子进程」。

### Rust 移植建议（api）
- crate 路径：`api-gateway`（实现 dispatch 引擎 + 动态 codec 边界）→ `api-remotes`（依赖 gateway + 各业务包，做装配与 lookup 策略）。
- 难点：
  1. `TypertGatewayService.invoke` 的**动态分派**（`namespace/method` → 生成的方法 + 参数 codec）在 Rust 里要用「注册表 + `Box<dyn>`/`Any` + 生成代码 `match`」；没有 TS 的反射/装饰器，**方法描述符必须由 codegen 产出**。
  2. `agent-lookup` 的「wire id → live agent/session + subagent 所有权校验」→ 定义 `trait AgentResolver`，由业务包提供实现，remotes 只做编排。
  3. `TypertClientRemote.$mount/$on/$dispatch` 的「fiber 生命周期 + 事件订阅隔离」→ Rust 用 RAII 作用域 + 订阅句柄（`Drop` 自动注销）。
  4. `API_REMOTE_FORWARDED_EVENTS` 的编译期 shape 门禁（`satisfies`）→ Rust 用 `const` 断言/宏在编译期校验事件签名。
- 依赖顺序：必须在 `commands`/`goal`/`session`/`host-plugin-inventory`/`message-feedback` 等**业务包之后**实现（remotes 是它们的装配面）；`gateway` 只依赖 `typert-*`，可先落地。

---

## 4. `packages/sdk` —— 客户端 SDK（进程外驱动）

一句话用途：**外部进程通过 stdio JSON-RPC 驱动一个 harness 运行时子进程**，跑 agent turn 并流式收取事件。

### 子包

#### 4.1 `@deepseek-ai/dsh-sdk-protocol`
- 用途：**SDK 共享 wire 协议**——换行分隔 JSON-RPC stdio 传输 + 命名的请求/结果/通知类型。
- workspace 依赖：peer `@deepseek-ai/dsh-invariants`、`@deepseek-ai/dsh-llm`、`@deepseek-ai/dsh-session`、`@deepseek-ai/dsh-subagent`、`@deepseek-ai/cordis`。
- src 顶层模块：`index.ts`、`types.ts`、`transport.ts`、`invariant.ts`。
- 导出面：`JsonRpcLineTransport`、`JsonRpcResponseError`、`JsonRpcTransportPeer`；wire 类型 `InitializeParams`/`InitializeResult`、`SessionPromptParams`/`SessionPromptResult`、`SdkRunStatus`、`SessionEventNotification`、`SessionStatusNotification`、`SubagentStartedNotification`、`SubagentFinishedNotification`、`HarnessSdkNotificationMap`（4 个通知）、`HarnessSdkRequestMap`（`initialize`/`session/prompt`/`shutdown`）。

#### 4.2 `@deepseek-ai/dsh-sdk-client`
- 用途：**TypeScript 客户端 SDK**——`DeepSeekHarness`（高层 turn API，管理一个运行时子进程）+ 低层 `HarnessClient`。
- workspace 依赖：peer `@deepseek-ai/dsh-invariants`、`@deepseek-ai/dsh-llm`、`@deepseek-ai/dsh-sdk-protocol`、`@deepseek-ai/dsh-session`、`@deepseek-ai/cordis`。
- src 顶层模块：`api.ts`、`client.ts`、`dispose.ts`、`index.ts`、`invariant.ts`、`types.ts`。
- 导出面：`DeepSeekHarness`（`AsyncDisposable`：`start`/`session`/`run`/`close`）、`HarnessSession`（`run`）、`RunOptions`、`RunResult`、`normalizeInput`、`finalResponse`、`HarnessClient`、`TransportClosedError`、`RequestTimeoutError`、`SdkProtocolError`、`NotificationSubscription`、`isRecord`、`disposeRuntimeProcess`、`HarnessNotification`、`NotificationFilter`、`HarnessClientOptions`、`DeepSeekHarnessOptions`，以及 `JsonRpcResponseError`、`ContentBlock` 转发。

#### 4.3 `@deepseek-ai/dsh-sdk-jsonrpc-server`
- 用途：**stdio JSON-RPC server 插件**——harness 进程内监听 stdio，供外部 SDK 客户端调用。
- workspace 依赖：`@deepseek-ai/schemastery`；peer `@deepseek-ai/dsh-agent`、`@deepseek-ai/dsh-invariants`、`@deepseek-ai/dsh-llm`、`@deepseek-ai/dsh-llm-deepseek`、`@deepseek-ai/dsh-scope`、`@deepseek-ai/dsh-sdk-protocol`、`@deepseek-ai/dsh-session`、`@deepseek-ai/dsh-subagent`、`@deepseek-ai/cordis`。
- src 顶层模块：`index.ts`、`server.ts`、`invariant.ts`。
- 导出面：`HarnessSdkJsonRpcServer`、`HarnessSdkJsonRpcServerOptions`、`JsonRpcConfig`（schemastery schema）、`name`/`inject=['agents']`/`apply`。

### Rust 移植建议（sdk）
- crate 路径：`sdk-protocol`（纯 wire 类型 + serde，无业务依赖）→ `sdk-client`（子进程管理 + 高层 run 循环）→ `sdk-jsonrpc-server`（server 侧 dispatch，依赖 agent/session/llm）。
- 难点：
  1. **帧协议必须 1:1**：换行分隔 JSON-RPC（每行一个 JSON 对象），方法名与通知名固定（`initialize`/`session/prompt`/`shutdown` + `session.event`/`session.status`/`subagent.started`/`subagent.finished`）。
  2. 子进程生命周期（`AsyncDisposable`、失败重试换新实例、幂等 close）→ `Drop` + 显式 `close()` 语义，注意 panic/取消安全。
  3. `HarnessSession.run` 的「enqueue → 等到 inbox 收据 → 收集事件直到 `session.status=idle`」事件循环 → 需要精确复刻顺序与超时。
  4. `SubagentFinishedNotification` 只报 in-process 子代理（remote 不报）——移植时保留此边界语义。
- 依赖顺序：`sdk-protocol` 最早；`sdk-client` 与 `sdk-jsonrpc-server` 都只依赖 protocol + 业务类型，可并行，但 server 依赖 agent/session 子系统。

---

## 5. `packages/preset` —— Agent 预置组合 / Persona

一句话用途：**把「agent 由哪些插件组成」外部化为 `cordis.yml` 预设文件，按会话挂载**；并注入部署级 persona 段落。

### 子包

#### 5.1 `@deepseek-ai/dsh-agent-presets`
- 用途：**从 preset `cordis.yml` 文件做每会话 agent 组合**（发现、元数据、作者化、挂载、会话绑定）。
- workspace 依赖：`js-yaml`、`@deepseek-ai/schemastery`；peer `@deepseek-ai/cordis-plugin-include`、`@deepseek-ai/cordis-plugin-loader`、`@deepseek-ai/dsh-agent`、`@deepseek-ai/dsh-atomic-write`、`@deepseek-ai/dsh-invariants`、`@deepseek-ai/dsh-home-paths`、`@deepseek-ai/dsh-scope`、`@deepseek-ai/dsh-session`、`@deepseek-ai/dsh-settings`、`@deepseek-ai/dsh-system-prompt`、`@deepseek-ai/cordis`。
- src 顶层模块：`authoring.ts`、`discovery.ts`、`index.ts`、`invariant.ts`、`metadata.ts`、`mount.ts`、`preset.ts`、`session.ts`、`types.ts`。
- 导出面：`AgentPresets`（Service，~570 行）、`SETTINGS_NAMESPACE`、`AgentPresetSettings`/`AgentPresetSettingsSchema`、`COMPOSITION_FILE`（`agent.cordis.yml`）、`discoverPresets`、`scanRoot`、`readComposition`/`copyComposition`/`deleteComposition`、`readPresetMetadata`/`renderPresetMetadata`、`METADATA_FILE`（`preset.yml`）、`resolveSessionPreset`、`PresetBearingSession`、`PresetMountError`/`UnknownPresetError`、`AgentPreset`/`Config`/`PresetRoot`/`PresetTrust`/`PRESET_ID`、`mountPreset`/`livePresetMounts`/`leakedServices`/`serviceForAgent`/`inactiveRows` 等。

#### 5.2 `@deepseek-ai/dsh-persona`
- 用途：**组合作者化的部署 persona 段落**（注入系统提示）。
- workspace 依赖：`@deepseek-ai/schemastery`；peer `@deepseek-ai/dsh-invariants`、`@deepseek-ai/dsh-system-prompt`、`@deepseek-ai/cordis`。
- src 顶层模块：`index.ts`、`invariant.ts`。
- 导出面：`PERSONA_ORDER`、`PERSONA_SECTION`、`name`/`inject=['systemPrompt']`/`Config`(zod)/`apply`。

### Rust 移植建议（preset）
- crate 路径：`agent-presets`、`persona`（persona 很小，可并入 preset 或独立）。
- 难点：
  1. 动态 `cordis.yml` → Rust 端需要**运行时插件装载器**（对应 `cordis-plugin-loader` + `include`），这是整仓移植的大前提之一。
  2. `mount.ts` 的 fiber 生命周期与**泄漏检测**（`leakedServices`、`inactiveRows`）→ Rust 资源作用域（RAII）+ 挂载审计。
  3. preset 发现/信任（`PresetTrust: 'system' | 'user'`，多 root 扫描）→ 文件系统遍历 + 信任边界。
- 依赖顺序：依赖 `cordis-plugin-loader`/`include`（装载机制）、`settings`、`session`、`home-paths`、`atomic-write`、`system-prompt`。

---

## 6. `packages/identity` —— 匿名用户身份

一句话用途：**共享的匿名用户身份**，供遥测与反馈关联使用（每用户一个持久化随机 id）。

### 子包 `@deepseek-ai/dsh-anonymous-user-id`
- workspace 依赖：peer `@deepseek-ai/dsh-brand`、`@deepseek-ai/dsh-invariants`、`@deepseek-ai/dsh-home-paths`、`@deepseek-ai/cordis`。
- src 顶层模块：`index.ts`、`invariant.ts`。
- 导出面：`AnonymousUserId`（`Branded<'AnonymousUserId'>`）、`ANONYMOUS_USER_ID_FILE_NAME`（`.anonymous-user-id`）、`AnonymousUserIdOptions`、`getOrCreateAnonymousUserId`。
- Rust 移植：`anonymous-user-id` crate（依赖 `brand`/`home-paths`）。难点：幂等创建 + 文件持久化（与 `atomic-write` 复用）。工作量很小，可作早期 smoke-test crate。

---

## 7. `packages/settings` —— 用户设置抽象

一句话用途：**`ctx.settings` 抽象设置 seam**（schema 注册、读写、冲突检测、脱敏）+ 一个 `settings.yaml` 文件后端。

### 子包

#### 7.1 `@deepseek-ai/dsh-settings`
- 用途：**抽象用户设置 seam**（`ctx.settings`，动态 schema 注册 + 多 provider 合并）。
- workspace 依赖：peer `@deepseek-ai/dsh-brand`、`@deepseek-ai/dsh-invariants`、`@deepseek-ai/cordis`、`@deepseek-ai/schemastery`。
- src 顶层模块：`index.ts`、`redact.ts`、`types.ts`、`invariant.ts`。
- 导出面：`SettingsProvider`（抽象基类，~900 行）、`settingsNamespace`、`SettingsNamespace`（Branded）、`SettingsUpdateSource`、`SettingsRegisterOptions`、`SettingsDescriptor`、`SettingsDescribeOptions`、`SettingsScope`、`deepEqualJson`、`SettingsConflictError`、`SettingsPathOp`、`SettingsSectionHooks`、`installSettingsSection`、`SettingsApplies`；脱敏 `redactSecrets`、`RedactedSecret`、`RedactedValue`。

#### 7.2 `@deepseek-ai/dsh-settings-file`
- 用途：**`settings.yaml` 文件后端** provider（含监听、锁、并发）。
- workspace 依赖：`chokidar`、`yaml`、`@deepseek-ai/schemastery`；peer `@deepseek-ai/dsh-atomic-write`、`@deepseek-ai/dsh-invariants`、`@deepseek-ai/dsh-home-paths`、`@deepseek-ai/dsh-settings`、`@deepseek-ai/cordis`。
- src 顶层模块：`index.ts`、`invariant.ts`。
- 导出面：`FileSettingsProvider`（~370 行，默认导出）、`Config`、`resolveSpec`。

### Rust 移植建议（settings）
- crate 路径：`settings`（抽象 seam + 动态 schema）→ `settings-file`（serde_yaml + `notify` 监听 + 文件锁）。
- 难点：
  1. **动态 schema 注册**（每个命名空间带 Zod schema，运行时校验）→ Rust 端需要「类型擦除 schema 注册表 + 值校验」（可借 `serde`/`schemars`/`valuable` 或自研轻量 dynamic schema）。
  2. `redactSecrets` 的 secret 脱敏（按 schema 标 secret 字段）→ 遍历 + 标记替换。
  3. `SettingsPathOp` 的路径补丁语义、冲突检测（`deepEqualJson`）→ 语义保留。
- 依赖顺序：依赖 `brand`、`atomic-write`、`home-paths`、`schemastery`；被 `preset`、`api-remotes`、`session-query` 等消费。

---

## 8. `packages/session-query` —— 会话历史查询（**重点说明**）

一句话用途：**对会话历史做统一查询**——读、追溯（lineage/event 替换链）、过滤、全文搜索，并以「live 会话优先、persisted 兜底」的逻辑语料合并两者。

### 子包

#### 8.1 `@deepseek-ai/dsh-session-query`（服务契约）
- 用途：**组合式会话查询服务契约**——具体读/追溯/过滤是后端无关的，全文搜索/排序/游标由后端实现。
- workspace 依赖：peer `@deepseek-ai/dsh-brand`、`@deepseek-ai/dsh-invariants`、`@deepseek-ai/dsh-llm`、`@deepseek-ai/dsh-session`、`@deepseek-ai/dsh-session-title`、`@deepseek-ai/dsh-session-persistence`(optional)、`@deepseek-ai/cordis`。
- src 顶层模块：`config.ts`、`corpus.ts`、`cursor.ts`、`documents.ts`、`extraction.ts`、`filters.ts`、`index.ts`、`invariant.ts`、`sources.ts`、`tracing.ts`、`types.ts`。
- 导出面：`SessionQueryEngine`（抽象基类，`ctx.sessionQuery`；**抽象**：`searchSessions`/`searchEvents`；**具体**：`listSessions`/`readSession`/`filterSessions`/`readTitle`/`readTitleSnapshots`/`listEvents`/`filterEvents`/`readSurface`/`traceSession`/`traceEvent`/`readEvent`）、`SessionSearchCursor`、`SessionQueryError`/`SessionQueryErrorCode`（17 个错误码）、`Config`、`extractSessionEventText`、`buildSessionEventRecords`/`buildSessionEventSearchDocuments`、过滤/追溯助手，以及一整套类型（`SessionRecord`、`SessionEventRecord`、`SessionSearchRequest`、`SessionEventSearchRequest`、`SessionSearchPage`、`SessionLineageTrace`、`SessionEventTrace`、`SessionEventWindow`、`SessionSurfaceSnapshot`、`SessionLogSnapshot`、`SessionAvailability`、`SessionResultFilter` 等）。

#### 8.2 `@deepseek-ai/dsh-session-query-sqlite`（后端实现）
- 用途：**`ctx.sessionQuery` 的 SQLite FTS5 后端**（全文搜索 + 排序 + 游标）。
- workspace 依赖：`@deepseek-ai/schemastery`；peer `@deepseek-ai/dsh-invariants`、`@deepseek-ai/dsh-session`、`@deepseek-ai/dsh-session-persistence`(optional)、`@deepseek-ai/dsh-session-query`、`@deepseek-ai/cordis`。
- src 顶层模块：`index.ts`（`SqliteSessionQueryEngine` ~1100 行）、`query.ts`、`schema.ts`、`invariant.ts`。
- 导出面：`SqliteSessionQueryEngine`（默认导出）、`Config`、`OpenAt`、常量（`SESSION_QUERY_SQLITE_PATH_KEY`/`DEFAULT_LIMIT=20`/`MAX_LIMIT=100`/`SNIPPET_CHARS=240`/`SCHEMA_VERSION=8`/`APPLICATION_ID`）、`openSearchDatabase`、查询构建器（`normalizeSessionRequest`/`normalizeEventRequest`、`buildSessionWhere`/`buildEventWhere`、`quoteFtsData`/`sanitizeFtsText`、`requestFingerprint`、`makeSnippet`、`FTS_HIGHLIGHT_START/END`、`SQLITE_MAX_PAGE_LIMIT`/`SQLITE_PORTABLE_VARIABLE_LIMIT`/`SQLITE_FTS5_OUTER_PREDICATE_LIMIT`）。

#### 8.3 `@deepseek-ai/dsh-tool-session-query`（模型工具层）
- 用途：**面向模型的会话历史搜索/追溯/事件读取工具**（带 workspace 授权）。
- workspace 依赖：`@deepseek-ai/schemastery`；peer `@deepseek-ai/dsh-invariants`、`@deepseek-ai/dsh-llm`、`@deepseek-ai/dsh-session`、`@deepseek-ai/dsh-session-query`、`@deepseek-ai/dsh-system-prompt`、`@deepseek-ai/dsh-timeout`、`@deepseek-ai/dsh-tools`、`@deepseek-ai/cordis`。
- src 顶层模块：`index.ts`、`input.ts`、`invariant.ts`、`operations.ts`、`presentation.ts`、`service-boundary.ts`、`workspace-access.ts`。
- 导出面：`name`/`inject=['tools','systemPrompt','sessionQuery']`/`Config`(zod)/`apply`，`DEFAULT_MAX_SEARCH_RESULTS=100`、`DEFAULT_SEARCH_TIMEOUT_MS=30000`，以及 `toolInput`/`operations`/`presentation`/`serviceBoundary`/`workspaceAccess` 模块对象。

#### 8.4 `@deepseek-ai/dsh-session-log-export`（Web UI）
- 用途：**Web 会话日志导出命令 + 下载对话框**（纯前端，Client 平台）。
- workspace 依赖：peer `@deepseek-ai/cordis` + 一串 `dsh-client-*`（locale/runtime/ui-commands/ui-conversation/ui-primitives/ui-slots）+ `dsh-commands`/`dsh-invariants` + `react`。
- src 顶层模块：`index.ts`、`invariant.ts`、`client/controller.ts`、`client/index.ts`、`client/locales.ts`、`client/Dialog.tsx`（编译产物）。
- 导出面：`session-log-download` 命令、`SessionLogDownloadController`、`SessionLogDownloadEntry`/`State`/`Status`、`sessionLogZipFilename`、`downloadUrl`、`NS`/`zh`/`en`（i18n）。

### session-query 是做什么的（一句话说明）
它是 **「会话历史检索层」**：把「当前 live 会话 + 已持久化日志」抽象成一个逻辑语料（`SessionCorpus`），向上提供统一的读（精确事件/窗口/完整日志）、追溯（session lineage、事件位置替换链）、过滤、标题折叠，以及全文搜索（后端实现，默认 SQLite FTS5），再经 `tool-session-query` 暴露为模型可调用的工具（受 workspace 授权约束）。`session-log-export` 则是同一主题下的前端导出 UI。

### Rust 移植建议（session-query）
- crate 路径：`session-query`（引擎 trait + 具体读/追溯/过滤/语料）→ `session-query-sqlite`（`rusqlite` + FTS5，实现搜索/游标）→ `tool-session-query`（模型工具注册，依赖 tools/system-prompt/timeout）。`session-log-export` 属前端，**不在后端 Rust 1:1 范围**（可标注为 Web 侧）。
- 难点：
  1. **FTS5 查询构建**：保留 `SQLITE_PORTABLE_VARIABLE_LIMIT=32766`（绑定参数上限）、`SQLITE_FTS5_OUTER_PREDICATE_LIMIT=14`（外层谓词上限）、`quoteFtsData`/`sanitizeFtsText` 的转义、`makeSnippet` 的 `\uFDD0/\uFDD1` 高亮标记。
  2. **游标分页**：`SessionSearchCursor`（Branded 字符串）与 `SESSION_QUERY_STALE_CURSOR` 检测（索引代际）。
  3. **live-preferred 语料合并**：`SessionCorpus` 把 live session 优先于 persisted，并做 header 兼容性校验（`assertSessionHeadersCompatible`）、replay 校验（`Session.create`）。
  4. `workspace-access` 的授权边界（模型工具只读被授权 workspace 的历史）→ 权限模型。
- 依赖顺序：依赖 `session`、`session-title`、`session-persistence`、`brand`、`llm`；`tool-session-query` 再依赖 `tools`/`timeout`/`system-prompt`。

---

## 附：全组依赖方向速览（porting 顺序参考）

```
brand → (home-paths / atomic-write / timeout / native-command / output-retention / launch-environment)
  → settings → settings-file
  → identity(anonymous-user-id)

typert-protocol → typert-registry → typert-loader   (typert-generator 为构建期，独立)
  → api-gateway → api-remotes（依赖 commands/goal/session/动态插件/inventory/feedback）

session (+ session-title / session-persistence) → session-query → session-query-sqlite
  → tool-session-query（依赖 tools/system-prompt/timeout）

sdk-protocol → (sdk-client, sdk-jsonrpc-server)（server 依赖 agent/session/llm）

preset(agent-presets / persona) 依赖 loader/include + settings + session + system-prompt
```

要点：`typert` 与 `util`（尤其 `brand`）是最底层；`api` 与 `sdk` 是两条**并行**的对外通道（进程内类型安全 RPC vs 进程外 stdio JSON-RPC）；`session-query` 是会话历史检索层，横跨 `session`/`session-persistence` 之上。
