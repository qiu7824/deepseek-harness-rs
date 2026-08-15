# 组 C:主机基础设施 — 后端 Rust 1:1 移植包级清单

> 范围:`packages/host` 下 8 个子包 + `apps/cli`(CLI 入口) + `apps/web`(浏览器前端壳)。
> 只读盘点,未修改任何 TS 源码。源码根:`D:\HermesTemp\deepseek-harness`。

---

## 0. 总览与依赖顺序

组 C 是 Web GUI 主机的"传输与外壳"层:一个传输无关的 API 网关契约(`apiproxy`)、一个 node:http 路由注册服务(`webserver`)、SPA 静态托管(`frontend-static`)、Cordis 插件状态只读投影(`plugin-inventory`)、目录选择器能力缝 + 三个后端(`directory-picker` / `-auto` / `-browse` / `-native`),以及把它们组合起来的 CLI(`apps/cli`)和前端壳(`apps/web`)。

**组合启动顺序(自底向上)**:`dsh-invariants` → `webserver`(注册 `ctx.webServer`) → `frontend-static`(占用 fallback 座位) → `directory-picker` 缝 → `directory-picker-browse` / `directory-picker-native` → `directory-picker-auto`(按宿主事实选后端并挂载) → `plugin-inventory` → `apiproxy`(注册 `ctx.apiProxy`,消费 `directoryPicker` 等) → `apps/cli`(组合引导) → `apps/web`(构建 dist,由 frontend-static 托管)。

**核心 wire 协议(apiproxy 定义,各组共享)**:四象限 RPC 消息模型。逻辑消息是 4 成员判别联合 `ClientRequest / ServerResponse / ServerRequest / ClientResponse`,与 HTTP / WebSocket / 进程内 SSE 等物理载体解耦。一元调用走 `POST /api/<method>`,HTTP 状态只表达载体层(404 未知路径 / 415 非 JSON / 400 非 JSON body / 500 处理器崩溃),业务错误恒为 `200 + result.ok=false`。

---

## 1. `@deepseek-ai/dsh-host-apiproxy`

**一句话用途**:API 网关 —— 定义传输无关的 `ApiProxy` 契约(`api/`,浏览器可导入)、fetch 载体对(`fetch/`:host 侧 `toFetchHandler` + client 侧 `AbstractApiClient`/`InProcessApiClient`)、以及提供 `ctx.apiProxy` 的宿主网关插件(`api-proxy.ts`)。本包**不注册任何路由**,物理载体(webserver)自行包裹 `ctx.apiProxy`。

**依赖的 workspace 包**(dependencies,均为 `workspace:^`):
`dsh-attachment`, `dsh-agent`, `dsh-agent-default-model`, `dsh-api-remotes`, `dsh-brand`, `dsh-commands`, `dsh-credentials`, `dsh-goal`, `dsh-host-directory-picker`, `dsh-llm`, `dsh-native-command`, `dsh-session`, `dsh-session-persistence`, `dsh-session-projection`, `dsh-session-projection-cache`, `dsh-session-query`, `dsh-session-title`, `dsh-settings`, `dsh-skill`, `dsh-subagent`, `dsh-jobs`, `dsh-tools`, `dsh-user-approval`, `dsh-user-questions`, `dsh-workspace`, `schemastery`;外部 `fflate`、`zod`。peerDeps:`cordis`, `dsh-agent-presets`, `dsh-cordis-host-runner`, `dsh-invariants`。

**src 顶层模块文件列表**:
- `src/index.ts`(导出面 + `ApiProxyService` 网关插件 + `Config`)
- `src/invariant.ts`
- `src/api-proxy.ts`(`createApiProxy` 实现,~1400 行,核心)
- `src/native-path-opener.ts`(跨平台原生打开器)
- `src/session-export.ts`(会话日志 ZIP 流式导出)
- `src/api/`(契约层,22 个文件:每个域一对 `x.ts` + `x.schema.ts`,加 `index.ts`、`rpc.ts`、`rpc.schema.ts`、`rpc-map.ts`、`session-search.ts`)
- `src/fetch/client.ts`、`src/fetch/handler.ts`

**导出面**:
- 服务:`ApiProxyService`(注册 `ctx.apiProxy`,注入 `agentDefaultModel, agents, attachments, directoryPicker, llm, sessions, subagents, sessionQuery, tools, userQuestions, workspaceRegistry`)。
- 契约接口:`ApiProxy`(根接口,含 `sessions / subagents / host / workspace / skills / agentPresets / events / goals / settings / credentials / llm / downloads / respond`)。
- 载体:`toFetchHandler(api)`、`AbstractApiClient`、`InProcessApiClient`、`IApiClient`。
- 消息层:`RpcId`、`RpcRequest/Response`、四象限全形 `ClientRequest/ServerResponse/ServerRequest/ClientResponse`、`RpcError/RpcErrorCode/RpcErrorDetailsMap`、`transportError`。
- 常量:`SESSION_SEARCH_RESULT_LIMIT=20`、`SESSION_SEARCH_SNIPPET_MAX_CODE_POINTS=240`。

### 1.1 Wire 信封与载体语义(端点清单前置)

所有 `POST /api/<method>` 一元调用:
- 请求 body:`{ type:'client-request', rpcId:string, method:string, payload:{...} }`。
- 响应 body:`{ type:'server-response', rpcId:string(回显), result: { ok:true, value:<V> } | { ok:false, error:{ code, message, details } } }`。
- `Content-Type` 必须 `application/json`(否则 415,防 CSRF 盲写侧信道);body 非 JSON → 400;未知 path → 404;处理器崩溃 → 500。

### 1.2 REST 端点清单(完整)

一元 RPC 方法注册于 `rpc-map.ts`(`RpcMethodMap`),路由表在 `fetch/handler.ts`(`UNARY_ROUTES`)。共 **52 个 POST 方法** + **4 条特殊路由**。

**sessions 域(12)**:

| 方法 + 路径 | body/payload | 响应 value |
|---|---|---|
| `POST /api/session.list` | `{ cursor?: string }`(v1 保留位未实现) | `{ items: SessionSummary[] }` |
| `POST /api/session.search` | `{ query: string }` | `{ items: SessionSearchItem[]; hasMore: boolean }`(≤20 条) |
| `POST /api/session.create` | `{ workspaceId?; cwd?; sessionId?; agentPreset? }` | `{ sessionId: SessionId; agentPreset?: string }` |
| `POST /api/session.history` | `{ sessionId; beforeSeq?; maxMessages? }` | `{ events: HistoryEntry[]; hasMore: boolean; projections?: SessionProjectionsBlock }` |
| `POST /api/session.models` | `{ sessionId }` | `SessionModels { current: ModelSelection; routable: boolean; groups: ModelProviderGroup[]; failures: ModelCatalogFailure[] }` |
| `POST /api/session.selectModel` | `{ sessionId; provider; model; reasoningEffort? }` | `{ selected: ModelSelection }` |
| `POST /api/session.rename` | `{ sessionId; title }` | `{ title: string; seq: number }` |
| `POST /api/session.fork` | `{ sessionId; atSeq? }` | `{ sessionId }` |
| `POST /api/session.prompt` | `{ sessionId; mode:'queue'\|'steer'; content: PromptContentPart[]; clientTimeZone? }` | `{ accepted: true; command?: { kind:'success'; text?: string } }` |
| `POST /api/session.attachment` | `{ sessionId; attachmentId }` | `{ attachment: ImageAttachmentRef; data: string }` |
| `POST /api/session.updateQueue` | `{ sessionId; itemId; action: QueueAction(edit/remove/steer) }` | `{ accepted: true }` |
| `POST /api/session.cancel` | `{ sessionId }` | `{ accepted: true }` |

**subagents 域(4)**:

| 方法 + 路径 | body/payload | 响应 value |
|---|---|---|
| `POST /api/subagent.list` | `{ parentSessionId }` | `SubagentCatalog { entries: SubagentListEntry[]; parentAvailable: boolean }` |
| `POST /api/subagent.history` | `{ parentSessionId; childSessionId; mode:'one-shot'\|'continuable'; beforeSeq?; maxMessages? }` | `{ events: HistoryEntry[]; hasMore; projections? }` |
| `POST /api/subagent.prompt` | `{ parentSessionId; childSessionId; mode:'continuable'; content: ContentBlock[]; clientTimeZone? }` | `{ messageId: MessageId }` |
| `POST /api/subagent.interrupt` | `{ parentSessionId; childSessionId; mode:'continuable' }` | `{ accepted: true }` |

**host 域(5)**:

| 方法 + 路径 | body/payload | 响应 value |
|---|---|---|
| `POST /api/host.describe` | `{}` | `{ version; cwd; provider?; model?; attachedSessions: number; canOpenPath: boolean }` |
| `POST /api/host.pickDirectory` | `{}`(仅 native 能力) | `{ path: string \| null }` |
| `POST /api/host.listDirectory` | `{ path? }`(仅 browse 能力) | `DirectoryListing { path; home; crumbs: DirectoryEntry[]; entries: DirectoryEntry[]; truncated }` |
| `POST /api/host.createDirectory` | `{ path; name }`(仅 browse 能力) | `{ path }` |
| `POST /api/host.openPath` | `{ path }` | `{ opened: true }` |

**workspace 域(7)**:

| 方法 + 路径 | body/payload | 响应 value |
|---|---|---|
| `POST /api/workspace.list` | `{}` | `{ items: WorkspaceView[]; archivedSessionIds: SessionId[] }` |
| `POST /api/workspace.create` | `{ path }`(现有目录,无 mkdir) | `{ workspace: WorkspaceView; created: boolean }` |
| `POST /api/workspace.rename` | `{ workspaceId; title }` | `{ workspace: WorkspaceView }` |
| `POST /api/workspace.delete` | `{ workspaceId }` | `{ deleted: true }` |
| `POST /api/workspace.insertBefore` | `{ workspaceId; beforeWorkspaceId? }` | `{ workspaceIds: WorkspaceId[] }` |
| `POST /api/workspace.insertSessionBefore` | `{ workspaceId; sessionId; beforeSessionId? }` | `{ workspace: WorkspaceView }` |
| `POST /api/workspace.archiveSession` | `{ sessionId }` | `{ archivedSessionIds: SessionId[] }` |

**skill 域(1)**:`POST /api/skill.list` — `{ sessionId }` → `{ skills: SkillEntry[] }`(`SkillEntry { name; description; whenToUse?; modelInvocable }`)。

**agentPreset 域(6)**:

| 方法 + 路径 | body/payload | 响应 value |
|---|---|---|
| `POST /api/agentPreset.list` | `{}` | `{ presets: AgentPresetEntry[]; authorable: boolean; hasDocument: boolean }` |
| `POST /api/agentPreset.select` | `{ sessionId; agentPreset }` | `{ agentPreset }` |
| `POST /api/agentPreset.read` | `{ agentPreset }` | `{ agentPreset; trust:'system'\|'user'; content; name?; description? }` |
| `POST /api/agentPreset.copy` | `{ from; agentPreset; name? }` | `{ agentPreset }` |
| `POST /api/agentPreset.openDocument` | `{ agentPreset }` | `{ opened: true } \| { opened: false; path: string }` |
| `POST /api/agentPreset.remove` | `{ agentPreset }` | `{}` |

**goal 域(6)**:

| 方法 + 路径 | body/payload | 响应 value |
|---|---|---|
| `POST /api/goal.create` | `{ sessionId; objective; maxGoalRounds? }` | `{ ref: GoalRef { id; revision } }` |
| `POST /api/goal.edit` | `{ sessionId; ref; objective?; maxGoalRounds? }` | `{ ref }` |
| `POST /api/goal.pause` | `{ sessionId; ref }` | `{ ref }` |
| `POST /api/goal.resume` | `{ sessionId; ref }` | `{ ref }` |
| `POST /api/goal.complete` | `{ sessionId; ref }` | `{ ref }` |
| `POST /api/goal.clear` | `{ sessionId; ref }` | `{ cleared: true }` |

**settings 域(5)**:

| 方法 + 路径 | body/payload | 响应 value |
|---|---|---|
| `POST /api/settings.describe` | `{}`(loopback-only) | `{ writable; hasDocument; namespaces: SettingsNamespaceView[] }` |
| `POST /api/settings.openDocument` | `{}` | `{ opened: true }` |
| `POST /api/settings.update` | `{ ns; patch: object; expectedRevision? }` | `SettingsNamespaceView` |
| `POST /api/settings.replace` | `{ ns; section: object; expectedRevision? }` | `SettingsNamespaceView` |
| `POST /api/settings.mutate` | `{ ns; ops: SettingsPathOpView[]; expectedRevision? }` | `SettingsNamespaceView` |

`SettingsNamespaceView { ns; schema(未知,JSON 化 schemastery 信封); value; base?; user?; applies:'live'\|'restart'; secrets: SettingsSecretView[]; revision }`。

**credentials 域(3)**:

| 方法 + 路径 | body/payload | 响应 value |
|---|---|---|
| `POST /api/credentials.describe` | `{ refs: string[] }` | `{ credentials: Record<string, CredentialView> }`(`CredentialView { configured; source?; writable }`,不含值) |
| `POST /api/credentials.set` | `{ ref; value }` | `{}` |
| `POST /api/credentials.unset` | `{ ref }` | `{}` |

**llm 域(3)**:

| 方法 + 路径 | body/payload | 响应 value |
|---|---|---|
| `POST /api/llm.providers` | `{}` | `{ providers: ConfigurableProviderView[] }` |
| `POST /api/llm.models` | `{}` | `{ groups: ModelProviderGroup[]; failures: ModelCatalogFailure[] }` |
| `POST /api/llm.discoverModels` | `{ settingsNs; provider?; baseURL?; api?; apiKey? }` | `{ models: DiscoveredModelView[] }` |

### 1.3 特殊路由(无 wire 信封)

| 方法 + 路径 | 说明 | 响应 |
|---|---|---|
| `GET /api/events.mux` | 全会话聚合 mux 流(SSE) | `text/event-stream`;每帧 `data: {"type":"server-request","rpcId":..,"method":"<frame.type>","payload":{MuxFrame}}`;开流先发 `: connected\n\n` 注释行 |
| `GET /api/events.host` | 宿主级信息流(SSE) | 同上,帧为 `HostFrame` |
| `GET\|HEAD /api/session.export` | 会话日志 ZIP 下载,query `sessionId`(+可选 `includeDescendants`) | 附件响应;缺根会话 404、缺服务 500 于首字节前 |
| `POST /api/respond` | 客户端响应回传(body = `ClientResponse`) | `RpcReceipt { accepted:true } \| { accepted:false; reason:'not-pending'\|'bad-response' }` |

### 1.4 SSE/事件流(帧并集)

`MuxFrame`(mux 流):`session/event`(原始事件透传 + 可选 `view`)、`session/subscribed`(基线)、`approval/requested`(可回答 server-request,稳定 rpcId)、`approval/resolved`、`question/requested` / `question/resolved`、`session/queue`(完整待处理 inbox 快照)、`session/jobs`(后台任务快照)、`session/projection`(单投影单元值,higher-seq-wins)、`stream/error`。

`HostFrame`(host 流):`host/session-added`、`host/session-removed`、`host/session-status`、`host/agent-error`、`host/workspace-changed`、`host/workspace-removed`、`host/workspace-order-changed`、`host/archived-sessions-changed`、`host/remote-event`(允许清单由 `dsh-api-remotes` 的 `API_REMOTE_FORWARDED_EVENTS` 持有,逐字转发)、`stream/error`。

**approval/question 应答**:`approval/requested` 与 `question/requested` 是可回答的 `ServerRequest`(稳定 rpcId);应答为 `ClientResponse` 经 `POST /api/respond`,payload 分别为 `ApprovalResponsePayload { sessionId; approvalId; outcome:'allowed-once'\|'rejected' }` 与 `QuestionResponsePayload { sessionId; answer: AskUserQuestionAnswer }`。

**错误码闭集**(`RpcErrorDetailsMap`,~38 个):`bad-request / cancelled / session-not-found / model-unavailable / session-conflict / invalid-time-zone / workspace-attach-failed / workspace-not-found / workspace-invalid-path / workspace-name-conflict / workspace-move-invalid / directory-unreadable / directory-exists / directory-create-failed / directory-picker-unavailable / agent-preset-read-only / agent-preset-locked / agent-preset-conflict / agent-preset-not-found / agent-preset-invalid / agent-busy / attachment-error / queue-item-not-found / steer-unavailable / command-error / unknown-command / settings-rejected / settings-not-exposed / settings-conflict / credential-rejected / model-discovery-failed / title-invalid / fork-unavailable / subagent-parent-unavailable / subagent-not-found / subagent-catalog-diagnostic / subagent-not-resumable / subagent-unauthorized / subagent-delivery-unavailable / internal`。

### 1.5 Rust 移植建议

- **crate 路径**:`crates/host/apiproxy`。拆三层:契约层(`api/` → serde 结构体 + 判别枚举,`zerocopy` 可选)、载体层(`fetch/handler.rs` → axum handler;`fetch/client.rs` → 供 wasm/浏览器复用的 client trait)、实现层(`api-proxy.rs` → 组合 `Context` 的 trait 实现)。
- **HTTP 框架映射(axum)**:`toFetchHandler` → 单个 `axum::Router`;52 个 `POST /api/:method` → 一个通配路由 + 方法分发表(或宏生成 52 条路由);`GET /api/events.mux|host` → axum SSE(用 `axum::response::sse` 或手写 `Body::from_stream`,每帧 `ServerRequest` JSON);`GET /api/session.export` → `Body::from_stream` 流式 ZIP;`POST /api/respond` → 独立 handler。
- **难点**:
  - SSE 双流与 `RpcRequest<Frame>` 信封、`stream/error` 收尾、`since`(v1 未实现)需 1:1 复刻。
  - ZIP 流式导出:JS 用 `fflate` 的流式 `Zip`/`ZipDeflate`;Rust 用 `async-zip`(或 `zip` + 手动分块)+ `async-compression`(DEFLATE level 0–9,默认 6),需复刻 64 KiB 背压门(`ResponseCapacityGate`)与代理对切断。
  - 图像 base64 解码的"非规范 base64 拒绝"逻辑(`decodeBase64`)需等价实现。
  - `session.prompt` 的 slash-command 前置拦截(`command-error`/`unknown-command`)与 `clientTimeZone` IANA 校验/规范化(依赖 `chrono-tz` + ICU 或等价 `Intl` 规范化)。
  - 原生打开器(`native-path-opener.rs`):macOS `osascript`/`open`,Linux `xdg-open`/`$BROWSER`,WSL `wslpath`,Windows `Invoke-Item`(PowerShell)—— 均走 `dsh-native-command` 的无 shell 命令运行器。
- **依赖顺序提示**:在 `directory-picker` 之后移植(它消费 `ctx.directoryPicker`);同时依赖会话/工作区/模型等大量领域包,是组 C 的"顶端"但非本组内先行项。

---

## 2. `@deepseek-ai/dsh-host-webserver`

**一句话用途**:Web 路由注册插件 —— 一个 `node:http` server + `webServer` 服务(HTTP 路由 + upgrade 路由注册表、index 变换 taps、唯一 fallback 座位)。不认识任何 harness 概念,不托管文件(文件由 frontend-static 通过 fallback 钩子负责)。

**依赖的 workspace 包**:dependencies `schemastery`;peerDeps `cordis`、`dsh-invariants`。

**src 顶层模块文件列表**:`src/index.ts`、`src/invariant.ts`。

**导出面**:
- 服务:`WebServer`(注册 `ctx.webServer`;`Config { host:'127.0.0.1'|'0.0.0.0', port }`)。
- 类型:`WebRoute { kind:'exact'|'prefix', path, handler }`、`WebUpgradeRoute { path, handler }`、`WebRouteKind`。
- 方法:`register(route)`、`registerUpgrade(route)`、`registerFallback(handler)`(唯一占有)、`tapIndex(transform)`、`applyIndexTaps(html)`、getter `port`/`host`。
- 语义:exact 先于 prefix;prefix 最长前缀优先;unmatched → fallback(未注册时 404);`upgrade` 事件 → 精确路径 upgrade 路由;tracked upgraded sockets 在关闭时显式销毁。

**Rust 移植建议**:
- **crate 路径**:`crates/host/webserver`。映射到 `axum::Router` + `tower::ServiceBuilder`。`WebRoute(exact/prefix)` → axum 精确路径 + 嵌套前缀路由(`nest`/`Router::route("/prefix/*path")`);`registerUpgrade` → axum 的 WebSocket 升级(`ws` extractor);`registerFallback` → `Router::fallback`;`tapIndex` → 一个应用于 index 响应的 layer(闭包栈)。
- **难点**:node:http 的 `upgrade` 生命周期与 axum/hyper 的 `on_upgrade` 差异;upgraded socket 的关闭跟踪(`closeAllConnections` 不含 upgraded socket);`host: 0.0.0.0` vs 127.0.0.1 的绑定语义;port 0 的 OS 分配端口回读。
- **依赖顺序提示**:本组最底层之一,仅依赖 `invariants` 与 `schemastery`(schema 校验 DSL),应最先移植。

---

## 3. `@deepseek-ai/dsh-host-frontend-static`

**一句话用途**:SPA dist 服务器 —— 占用 webserver 的 fallback 座位,托管构建产物;index-tap 注入、越界拒绝(403)、SPA index 回退、未知扩展 octet-stream、非 GET/HEAD 405。

**依赖的 workspace 包**:dependencies `schemastery`;peerDeps `dsh-host-webserver`、`dsh-invariants`、`cordis`。

**src 顶层模块文件列表**:`src/index.ts`、`src/invariant.ts`。

**导出面**:`name = 'frontend-static'`、`inject = ['webServer']`、`Config { distIndex: string }`、`serveStatic(pathname, res, distRoot, distIndex, renderIndex)`、`apply(ctx, config)`(注册 fallback)。MIME 表覆盖 `.html/.js/.css/.svg/.json/.map/.webmanifest`,其余 `application/octet-stream`。

**Rust 移植建议**:
- **crate 路径**:`crates/host/frontend-static`。映射到 `tower-http::services::ServeDir` + 自定义 `fallback` handler,或直接手写 axum fallback。`serveStatic` 的遍历拒绝(resolve + normalize + `sep` 前缀检查)用 `std::path` 规范化 + `strip_prefix` 复刻。
- **难点**:Windows 反斜杠路径分隔符与遍历拒绝边界(源码特别注释了 `sep` 而非 `/`);`distIndex` 通过 `!!js` 表达式注入的部署事实(RS 移植时改为启动参数);index-tap 注入链(与 webserver 的 `tapIndex` 联动)。
- **依赖顺序提示**:在 `webserver` 之后。

---

## 4. `@deepseek-ai/dsh-host-plugin-inventory`

**一句话用途**:当前 Cordis Loader 插件状态的只读 Remote 投影(供客户端 UI 列出插件及其 fiber 状态)。

**依赖的 workspace 包**:dependencies `zod`;peerDeps `cordis-plugin-loader`、`dsh-brand`、`dsh-invariants`、`dsh-typert-protocol`、`cordis`。

**src 顶层模块文件列表**:`src/index.ts`、`src/invariant.ts`、`src/types.ts`。

**导出面**:
- 服务:`PluginInventoryGateway extends TypertRemoteService`(注册 `ctx.pluginInventory`;注入 `loader`)。
- 方法:`@Remote('list') list(): PluginInventorySnapshot`,遍历 `ctx.loader.entries()`(跳过 group),返回 `{ entries: PluginInventoryEntry[] }`;`PluginInventoryEntry { entryId; moduleName; enabled; fiberPhase: 'pending'|'loading'|'active'|'failed'|'unloading'|null }`。
- 类型(从 `types.ts`):`PluginEntryId`、`PluginFiberPhase`、`PluginInventoryEntry`、`PluginInventorySnapshot`。

**Rust 移植建议**:
- **crate 路径**:`crates/host/plugin-inventory`。这是一个 Typert 协议的 `Remote` 服务(客户端通过类型化 RPC 调用 `list()`)。Rust 侧映射为:实现一个 `PluginInventory` trait(单方法 `list() -> PluginInventorySnapshot`),由 Typert 等价物注册进 RPC 注册表。fiber 状态枚举映射为 `enum PluginFiberPhase`。
- **难点**:`FiberState` 是跨包 const enum(0–5),投影 `DISPOSED -> null` 的映射;依赖 Cordis Loader 的插件树内省(RS 侧若自研组合器需同等的 entry 迭代器)。
- **依赖顺序提示**:依赖 `cordis-plugin-loader`(组合器)与 `dsh-typert-protocol`(客户端 RPC 协议),这两者在 Rust 移植中属于核心基础设施,应先于本包。

---

## 5. `@deepseek-ai/dsh-host-directory-picker`(能力缝)

**一句话用途**:抽象工作目录选择能力缝(`ctx.directoryPicker`)。以判别能力而非方法集暴露:后端有 `native`(宿主显示器上一个 OS 选择器)与 `browse`(应用内浏览器的列表/创建原语,可服务远程客户端)两种交互形状;`merge-extensible`,未知 kind 的默认行为是隐藏选择控件而非报错。

**依赖的 workspace 包**:peerDeps `dsh-invariants`、`cordis`(无 dependencies)。

**src 顶层模块文件列表**:`src/index.ts`、`src/invariant.ts`。

**导出面**:
- 抽象服务:`abstract class DirectoryPicker extends Service`(注册 `ctx.directoryPicker`,一个 context 一个实现;`abstract capability(): DirectoryPickerCapability`)。
- 类型:`DirectoryPickerNativeCapability { kind:'native'; pick(signal): Promise<string|null> }`、`DirectoryPickerBrowseCapability { kind:'browse'; list(path?,signal?); createDirectory(path,name) }`、`DirectoryEntry`、`DirectoryListing`、`DirectoryPickerCapabilities`/`DirectoryPickerCapability`。
- 错误:`DirectoryPickerError(code: 'directory-unreadable'|'directory-exists'|'directory-create-failed', path, message)`。

**Rust 移植建议**:
- **crate 路径**:`crates/host/directory-picker`。映射为一个 `trait DirectoryPicker { fn capability(&self) -> DirectoryPickerCapability; }` + `enum DirectoryPickerCapability { Native(...), Browse(...) }`(Rust 侧判别能力天然映射 enum)。`merge-extensible` 用 enum 扩展点或 `Any` 兜底表达。
- **难点**:无(纯抽象层)。
- **依赖顺序提示**:本组最底层,先移植;`apiproxy` 与 `-auto` 都依赖它。

---

## 6. `@deepseek-ai/dsh-host-directory-picker-auto`(自适应选择器)

**一句话用途**:目录选择器缝的自适应选择器 —— boot 时一次性采样宿主事实(bind host、SSH 启动、显示会话、Linux 选择器二进制),并把匹配的交互(`native`/`browse`)作为真实 Loader 条目挂载进内存根树(每个交互是 Host 后端 + Client 表面的成对条目)。

**依赖的 workspace 包**:peerDeps `cordis`、`cordis-plugin-loader`、`dsh-client-ui-directory-picker-browse`、`dsh-client-ui-directory-picker-native`、`dsh-host-directory-picker-browse`、`dsh-host-directory-picker-native`、`dsh-host-webserver`、`dsh-invariants`。

**src 顶层模块文件列表**:`src/index.ts`、`src/invariant.ts`、`src/probe.ts`、`src/resolve.ts`。

**导出面**:
- 插件:`name='directory-picker-auto'`、`inject=['webServer','loader']`、`apply(ctx)`(按 `resolveDirectoryPickerBackend` 结果 `loader.create` 挂载后端+表面)。
- 常量:`BACKEND_PACKAGES`/`SURFACE_PACKAGES`(kind → 包名映射)。
- `probe.ts`:`canExecute`、`hasLinuxChooserBinary`(PATH 扫描 zenity/kdialog)。
- `resolve.ts`:`resolveDirectoryPickerBackend(facts)` —— 纯函数:bind 非 127.0.0.1 → browse;SSH_CONNECTION/SSH_TTY 存在 → browse;darwin/win32 → native;linux 无 chooser 二进制 → browse;linux 有 DISPLAY/WAYLAND_DISPLAY + chooser → native;其余 → browse。

**Rust 移植建议**:
- **crate 路径**:`crates/host/directory-picker-auto`。映射为一个纯函数 `resolve_backend(&HostFacts) -> BackendKind` + 启动时 `match` 选择实现(`Box<dyn DirectoryPicker>` 或泛型选择),无需运行时挂载(TS 的动态 Loader 条目在 Rust 静态组合中直接静态分派)。
- **难点**:TS 侧"运行时挂载 Host 后端 + Client 表面成对条目"的语义在 Rust 静态组合下退化为编译期二选一;保留 `probe`(Linux 上 zenity/kdialog 探测用 `which`/PATH 扫描)。
- **依赖顺序提示**:在 `webserver` + `directory-picker` + `-browse` + `-native` 之后。

---

## 7. `@deepseek-ai/dsh-host-directory-picker-browse`(浏览后端)

**一句话用途**:目录选择器缝的浏览后端 —— 注册 `ctx.directoryPicker` 的 `browse` 能力:单层目录列表 + 子目录创建,走 Node stdlib(per-OS 适配),不在宿主显示器渲染。

**依赖的 workspace 包**:dependencies `dsh-host-directory-picker`、`schemastery`;peerDeps `dsh-invariants`、`cordis`。

**src 顶层模块文件列表**:`src/index.ts`、`src/invariant.ts`。

**导出面**:`default class BrowseDirectoryPicker extends DirectoryPicker`(能力对象 `{ kind:'browse', list, createDirectory }`);`Config { maxEntries: number, 默认 1000 }`;导出辅助函数 `fullyQualified(path,platform)`、`boundedInsert(window,candidate,keep)`(name-sorted 有界窗口)、`raceAbort`、`ListingCandidate`。`list` 用 `opendir` 流式读入 name 排序的有界窗口(maxEntries+1 证明截断);`createDirectory` 非递归 `mkdir`(EEXIST → `directory-exists`)。

**Rust 移植建议**:
- **crate 路径**:`crates/host/directory-picker-browse`。`opendir` 流式 → `tokio::fs::read_dir`;有界 name-sorted 窗口 → `BTreeSet`/binary insert + 有界容量;`mkdir` → `tokio::fs::create_dir`(AlreadyExists → directory-exists)。`fullyQualified` 的 Windows 驱动器/UNC 判定用 `std::path` + 正则复刻;中止信号用 `tokio::select!` + `CancellationToken` 复刻 `raceAbort`。
- **难点**:Windows 下"完全限定路径"判定(`\foo`、`/foo`、`\\server` 拒绝)是安全边界,必须等价;符号链接进入性探测(`stat` probe);100k 子目录下的 O(log keep) 有界窗口。
- **依赖顺序提示**:在 `directory-picker` 之后。

---

## 8. `@deepseek-ai/dsh-host-directory-picker-native`(原生后端)

**一句话用途**:目录选择器缝的原生后端 —— 注册 `ctx.directoryPicker` 的 `native` 能力,每次 pick 在宿主显示器打开一个 OS 选择器:macOS `osascript`、Linux `zenity`(KDialog 兜底)、Windows 在**子进程**中打开现代 `IFileOpenDialog`(koffi 驱动的 COM 对话,子进程主线程阻塞)。

**依赖的 workspace 包**:dependencies `dsh-host-directory-picker`、`dsh-native-command`、外部 `koffi`;peerDeps `dsh-invariants`、`cordis`。

**src 顶层模块文件列表**:`src/index.ts`、`src/invariant.ts`、`src/native-picker.ts`、`src/win32-dialog.ts`、`src/win32-dialog-host.ts`、`src/win32-dialog-logic.ts`、`src/win32-dialog-worker.ts`、`src/win32-dialog-bindings.ts`。

**导出面**:`default class NativeDirectoryPicker extends DirectoryPicker`(`pick: signal => pickNativeDirectory(signal)`);`pickNativeDirectory(signal, internals?)`;类型 `DirectoryPickerInternals`、`DirectoryPickerRunner`;`win32-dialog.ts` 的 `pickWin32Directory`、`DIALOG_TITLE`。`./worker` 子路径导出 `win32-dialog-worker`。

**关键实现细节**:
- `native-picker.ts`:平台分发;darwin `osascript -e "choose folder…"`(User canceled/-128 → null);win32 → `pickWin32Directory`;linux `zenity --file-selection --directory` 失败→`kdialog --getexistingdirectory`;取消(exit 1)→ null。
- `win32-dialog.ts`(主线程驱动):spawn 子进程,映射消息协议(`showing{threadId}`/`done{path}`/`error`),abort 时每 150ms 向对话框线程窗口投 `WM_CLOSE`(最多 20 次),超时 `kill()` 兜底。
- `win32-dialog-worker.ts`(子进程入口):阻塞于模态 `Show`,经 IPC 上报;绑定 `process.send`。
- `win32-dialog-logic.ts`(纯序列):`setThreadDpiAwareness` → `CoInitializeEx(STA)` → `CoCreateInstance(CLSID_FileOpenDialog)` → `SetOptions(FOS_PICKFOLDERS|FOS_FORCEFILESYSTEM|FOS_NOCHANGEDIR)` → `SetTitle` → `Show`(取消 = `HRESULT 0x800704c7`)→ `GetResult`+`IShellItem::GetDisplayName(SIGDN_FILESYSPATH)` → 释放。
- `win32-dialog-bindings.ts`(koffi FFI):加载 `ole32/user32/kernel32`,手写 COM vtable 槽位调用(IFileOpenDialog vtable:IUnknown 0-2、IModalWindow `Show`=3、IFileDialog `SetOptions`=9/`SetTitle`=17/`GetResult`=20;IShellItem `GetDisplayName`=5);`closeThreadWindows` 用 `EnumThreadWindows`+`PostMessageW(WM_CLOSE)`;GUID 手写 little-endian 编码。koffi 惰性 import,非 Windows 不加载。

**Rust 移植建议**:
- **crate 路径**:`crates/host/directory-picker-native`。**Win32 是最大难点**:建议直接用 `windows` crate(`windows::Win32::UI::Shell::IFileOpenDialog`、`windows::Win32::UI::Controls::Dialogs`、`windows::Win32::UI::WindowsAndMessaging`)或 `rfd`/`native-dialog` crate 替代 koffi 手写 FFI —— 但必须保留"子进程/子线程阻塞 + `WM_CLOSE` abort + `kill` 兜底"的三层中止语义。若坚持 1:1,`windows` crate 的 COM 接口已含 IFileOpenDialog vtable。
- **macOS**:`osascript` 子进程(`std::process::Command`);**Linux**:`zenity`/`kdialog` 子进程 + PATH 探测。均走 `dsh-native-command` 的 Rust 等价物(无 shell 命令运行器,`Command::new` 直连)。
- **难点**:COM STA 初始化/配对 `CoUninitialize`;DPI awareness 级联(per-monitor-v2 → per-monitor → system);跨线程 `EnumThreadWindows`+`PostMessageW` 关窗;子进程 IPC 协议复刻。
- **依赖顺序提示**:依赖 `directory-picker` 与 `dsh-native-command`;Windows 部分可独立排期(风险最高)。

---

## 9. `apps/cli`(`@deepseek-ai/dsh`,CLI 入口)

**一句话用途**:`dsh` 命令行入口 —— profile boot、插件管理(pnpm 转发)、`web` 别名与浏览器 UI 启动、config dump。

**依赖的 workspace 包**:dependencies 数十个(完整清单见 `package.json`),其中与本组直接相关的 Host 基础设施为 **devDependencies**:`dsh-host-frontend-static`、`dsh-host-apiproxy`、`dsh-host-webserver`(它们通过 profile bundle 机制被 web profile 组合)。其余为 `cordis-plugin-*`(loader/include/hmr/timer)、`dsh-app-boot`、`dsh-cmdline`、`dsh-launch-environment`、`dsh-home-paths` 及大量工具/命令包。外部:`commander`、`js-yaml`、`node-addon-require-builtin`。

**src 顶层模块文件列表**:`src/bin.ts`、`src/args.ts`、`src/profile-boot.ts`、`src/plugin.ts`、`src/dump-config.ts`、`src/process-shutdown.ts`。

**导出面 / 组合引导流程**:
- `bin.ts`(入口):`parseDshArgs` 解析后 switch 到三种模式(`profile` / `plugin` / `dump-config`),动态 import 各自模块。
- `args.ts`(commander 适配):`--profile` / `--patch`(可重复)/ `--dump-config` / `--dump-default-config`;子命令 `web`(硬编码 `--profile web` 别名)、`plugin --profile <name> <pnpm args>`;launcher 只解析自己拥有的 flag,其余原样传给被引导的树(注入的 app 插件各自解析)。
- `profile-boot.ts`(组合引导核心):`prepareProfile`(重写空根 `cordis.yml` `[]`);`composeProfile` 按序叠 patch 层 —— **bundle 层(`dsh.profile.bundles` 顺序)→ profile 自有 `cordis.patch.yml` → 家目录 `$DSH_HOME/cordis.patch.yml` → `--patch` overlays → telemetry 开关**;`SHIPPED_PRESET_ROOT` 把 `config/agent-presets/` 以 `trust:'system'` 注入 `agent-presets` 行;`runProfile` 调 `boot()` 挂载树,注册 SIGTERM/SIGINT、fail-loud、HMR 监听(`cordis-plugin-hmr`/`timer`)。
- `plugin.ts`:`dsh plugin` = 在 profile 目录内转发 pnpm 命令(`spawnSync`,Windows 走 `.cmd` shell),之后按已安装状态 reconcile `dsh.profile.bundles`(声明 `dsh.bundle` 的包加入层栈)。
- `dump-config.ts`:`--dump-config` 组合各 patch 层并打印,不 boot、不 eval `!!js`。
- `process-shutdown.ts`:有界升级式退出(5s 宽限 → 强制 exit)。

> 注:任务所指 `apps/cli/config` 下的 `cordis.yml` 实际是 **运行时生成的空根配置**(`PROFILE_ROOT_FILENAME='cordis.yml'`,内容 `[]`),真实组合由 `dsh-app-boot` 的 profile bundle/patch 机制承载(`apps/cli/config` 目录现仅有 `agent-presets/`)。组合清单在 profile manifest 的 `dsh.profile.bundles`,模板定义于 `@deepseek-ai/dsh-app-boot`。

**Rust 移植建议**:
- **crate 路径**:`crates/apps/cli`(最终二进制 `dsh`)。commander → `clap`(子命令 `web`、`plugin`,passthrough 参数用 `trailing_var_arg`);profile boot 的分层 patch 组合逻辑映射为配置加载器(读 bundle 清单 + YAML patch 叠加,`serde_yaml`);`plugin` 子命令转发 → `std::process::Command` pnpm。
- **难点**:动态 import 按模式隔离 → Rust 静态二进制下直接函数分派;Cordis 组合器(`boot`/`loader`/`cordis.yml` patch 叠加)本身不属于本组,是更大的移植前提;HMR 监听与进程信号关闭语义需用 `tokio::signal` + 宽限退出复刻。
- **依赖顺序提示**:组合引导是"顶端",需在所有被组合的服务包之后移植。

---

## 10. `apps/web`(`@deepseek-ai/dsh-web-frontend`,前端壳)

**一句话用途**:Web 应用入口 —— vite 构建于 `@deepseek-ai/dsh-client-web` 壳库之上;`dist/` 由 `apps/cli` 的 `dsh web` 托管。

**依赖的 workspace 包**:dependencies `dsh-client-web`、`react`、`react-dom`;devDeps `cordis-plugin-group`、`dsh-client-modules`、`dsh-client-ui-primitives`、`dsh-client-ui-slots`、`dsh-client-web-react`、`dsh-cmdline`、`dsh-pwsh-local`、`@vitejs/plugin-react`、`vite`、`vitest`、`typescript` 等。

**src 顶层模块文件列表**:`src/main.ts`、`src/node-module-stub.ts`;另有 `index.html`、`vite.config.ts`、`tsconfig.json`。

**后端 API 面(只总结,不深挖 UI)**:`main.ts` 极薄 —— `new AppWebEntry(el).run()`,一切 loader 持有、模块表播种、AppRoot 门、插件组装都在 `@deepseek-ai/dsh-client-web`。壳通过 `dsh-client-web` 内部的 fetch 载体(`AbstractApiClient` 的子类,`resolveBase()` 取 `location.origin`)请求上文 §1 的全部 `/api/*` 端点(52 个 POST RPC + `events.mux`/`events.host` SSE + `session.export` + `respond`)。`window.__DSH_BOOT__` 由 `dsh web` 注入(经 frontend-static 的 index-tap),vite 的 `rejectStandaloneServe()` 插件在 `serve` 模式直接报错 —— 本目录**不是独立应用**。

**Rust 移植建议**:
- **crate 路径**:Rust 侧无需独立 crate —— `apps/web` 只是构建产物;Rust 后端由 `frontend-static` 托管 `dist/`。前端壳本身(TS)不移植,只需保证 `index.html` 的 `window.__DSH_BOOT__` 注入(index-tap)与静态托管一致。
- **难点**:无后端逻辑;唯一关联是 `dsh web` 启动时需等价地把 boot 清单注入 index(对应 `webserver.tapIndex`)。
- **依赖顺序提示**:最后;依赖 `frontend-static` + `webserver` + `apiproxy` 全部就绪。

---

## 附:组 C 端点总数与要点摘要

**端点总数:56 条 HTTP 路由** = 52 个一元 RPC(`POST /api/<method>`) + 2 条 SSE 流(`GET /api/events.mux`、`GET /api/events.host`) + 1 条下载(`GET|HEAD /api/session.export`) + 1 条应答回传(`POST /api/respond`)。

**各包要点摘要**:
1. **apiproxy** —— 组 C 核心与最大工作量:传输无关 `ApiProxy` 契约 + 52 方法 `RpcMethodMap` + fetch 载体对 + SSE 双流 + 流式 ZIP 会话导出 + 38 个错误码闭集。移植时先落 serde 契约与 axum 分发表,再攻坚 SSE/ZIP 流。
2. **webserver** —— node:http 路由/upgrade/fallback/index-tap 注册服务,映射 axum Router,注意 upgrade(WebSocket)与 upgraded socket 关闭跟踪。
3. **frontend-static** —— SPA dist 托管,映射 tower-http ServeDir + SPA fallback,注意 Windows 遍历拒绝与 index-tap 注入。
4. **plugin-inventory** —— Loader 插件状态只读投影(Typert Remote),映射为单方法 trait + RPC 注册。
5. **directory-picker(缝)** —— 判别能力抽象(`native`/`browse`),映射 Rust trait + enum。
6. **directory-picker-auto** —— boot 采样 + 纯函数后端选择,映射为启动期静态分派。
7. **directory-picker-browse** —— 流式单层列表 + mkdir,映射 tokio::fs + 有界排序窗口 + 完全限定路径安全判定。
8. **directory-picker-native** —— **风险最高**:Win32 `IFileOpenDialog`(koffi 手写 COM FFI + 子进程 + `WM_CLOSE` abort),Rust 侧建议 `windows` crate;macOS/Linux 走子进程命令。
9. **apps/cli** —— 组合引导(分层 patch 叠加 + boot + HMR + 信号退出),映射 clap + 配置加载器;是被组合服务之上的"顶端"。
10. **apps/web** —— 纯前端构建产物,Rust 侧只托管 dist + 注入 boot 清单,无后端逻辑。
