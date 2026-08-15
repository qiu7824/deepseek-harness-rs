# 组 D 包级清单：会话 / 模型循环 (Group D — Session / Model Loop)

> 只读盘点，为「后端 Rust 1:1 移植」提供包级清单。源码根：`D:\HermesTemp\deepseek-harness`。
> 本组覆盖 9 个组目录、共 **40 个 workspace 包**。包目录结构为 `packages/<组>/<子包>`，每个子包是一个独立 npm 包（`@deepseek-ai/dsh-*`，版本 `0.1.0-rc.5`）。
> 约定：下方「依赖」仅列 workspace 内部包（`peerDependencies`/`dependencies` 里的 `@deepseek-ai/*` 与 `@deepseek-ai/cordis`），略去 `devDependencies` 与外部 npm 库（除非是协议关键，如 `koffi`/`sharp`/`@opentelemetry/*`/`@earendil-works/pi-ai`/`eventsource-parser`）。
> 所有包的插件入口都遵循 Cordis 约定：`export const name`、`export const inject`、`export function apply(ctx, config?)`；服务类通过 `export default` 导出，并通过 `declare module '@deepseek-ai/cordis' { interface Context { … } }` 合并到 `ctx`。

---

## 1. packages/session（13 个子包）

**一句话用途**：会话的事件溯源（event-sourced）持久化层 + 会话派生状态（投影/统计/标题/遥测）的完整能力族；核心数据模型（`Session`/`SessionEvent`/`SessionHeader`）在组外的 `@deepseek-ai/dsh-session`（见 loc.json 中 8401 行，属其他组），本组各包都 `peerDependencies` 它。

### 1.1 session-persistence — 抽象持久化缝隙（`ctx.sessionPersistence`）
- 依赖：`dsh-brand` `dsh-invariants` `dsh-session` `dsh-timeout` `cordis`
- src 模块：`index.ts` `coordinator.ts` `revision.ts` `write-behind.ts` `preparations.ts` `invariant.ts`
- 导出面：
  - 服务类：`SessionPersistence`（抽象 Service，键 `'sessionPersistence'`）
  - 编排器：`PersistenceCoordinator`（写路径编排：写批合并、冷会话 prepared 缓存、torn-tail 修复）
  - 类型：`PersistenceBackend<TornMarker>` `StoredPrefix` `StoredSuffix` `PersistenceCoordinatorOptions` `SessionInspection` `SessionRawArtifact` `SessionPersistenceSnapshot` `SessionLocation`
  - 常量/错误：`DEFAULT_PREPARED_SESSION_CACHE_SIZE` `DEFAULT_WRITE_BATCH_MAX_DELAY_MS` `MAX_WRITE_BATCH_DELAY_MS` `SessionFormatUnsupportedError` `SessionPersistenceCorruptionError` `sessionFormatVersionRefusal` `SessionPersistenceRevision`
  - 抽象方法（移植契约）：`locate` `supportsRawArtifacts` `readRaw` `create` `append` `load` `inspect` `readFrom` `list` `listSnapshots` `prepare`

### 1.2 session-persistence-jsonl — JSONL 持久化后端
- 依赖：`dsh-invariants` `dsh-session` `dsh-session-persistence` `cordis` + 外部 `koffi`(FFI→zstd) `schemastery`
- src 模块：`index.ts` `format.ts` `zstd.ts` `zstd-public-decoder.ts` `zstd-private-decoder.ts` `win32.ts` `invariant.ts`
- 导出面：`JsonlSessionPersistence`（`supportsRawArtifacts=true`）、`Config`（`root` 必填、`packChunks`、`compression:'zstd'|'none'`、`preparedSessionCacheSize`、`writeBatchMaxDelayMs`）、`JsonlCompressionSchema`、`JsonlCompression`
- 详见 §10 的目录/文件格式。

### 1.3 session-persistence-sqlite — SQLite 持久化后端
- 依赖：`dsh-invariants` `dsh-session` `dsh-session-persistence` `cordis` + `schemastery`（用 Node 内置 `node:sqlite`）
- src 模块：`index.ts` `schema.ts` `invariant.ts`
- 导出面：`SqliteSessionPersistence`（`supportsRawArtifacts=false`）、`Config`（`path`、`journalMode:'wal'|'delete'|'truncate'|'persist'`）、`SCHEMA_VERSION`
- 详见 §10 的表结构。

### 1.4 session-checkpoint-policy — 语义耐久检查点策略
- 依赖：`dsh-agent` `dsh-llm` `dsh-session` `dsh-session-persistence` `dsh-tools` `cordis`
- src 模块：`index.ts` `invariant.ts`
- 导出面：插件 `name='session-checkpoint-policy'`、`inject=['llm','sessionPersistence','sessions','tools']`、`apply`
- 触发（截获事件）：`llm/stream`（模型请求前 flush）、`tools/execute`（顶层工具派发前 flush，失败 fail-closed）、`agent/pre-step`（每步前 flush）

### 1.5 session-projection — 会话投影缝隙（`ctx.sessionProjections`）
- 依赖：`dsh-invariants` `dsh-session` `cordis` + `zod`
- src 模块：`index.ts` `types.ts` `invariant.ts`
- 导出面：
  - 服务类：`SessionProjectionRegistry`（订阅 `session/event`，对每个注册单元 eager 驱动 `apply`）
  - 类型：`ProjectionDefinition<K,S>`（`key`/`schema`/`init`/`apply`/`view`/`stateVersion`）、`ProjectionSnapshot`、`ProjectionCheckpoint`/`ProjectionCheckpointRow`、`ProjectionChangeListener`、`SessionProjectionMap`
  - 核心语义：whole-value 规则（事件携带完整后态，非增量）；`Object.is` 判改；`register`/`onChanged`/`snapshot`/`checkpoint`/`restore`/`restoreFloor`/`viewCheckpoint`

### 1.6 session-projection-cache — 投影缓存（`ctx.sessionProjectionCache`）
- 依赖：`dsh-session` `dsh-session-persistence` `dsh-session-projection` `dsh-storage-domain` `cordis` + `schemastery` `zod`
- src 模块：`index.ts` `spec.ts` `invariant.ts`
- 导出面：`SessionProjectionCache`（`inject=['storageDomain','sessionProjections','sessionPersistence','sessions']`）、`Config`（`writeEveryEvents`/`writeIntervalMs`）、`projectionCacheDomainSpec`/`checkpointIdentity`/`checkpointRecord`/`checkpointRow`
- 语义：写后写缓存（write-behind，count/interval 节流 + `turn/end` 与 `session/disposed` 两个强制点），冷读梯子（缓存行 + persistence `readFrom` tail + registry `restore` + 回写）；缓存行 `(sessionId,key,ver,seq,val)`，非权威、仅折叠捷径。

### 1.7 session-stats — 会话统计投影单元（插件）
- 依赖：`dsh-llm` `dsh-session` `dsh-session-projection` `cordis` + `zod`
- src 模块：`index.ts` `projection.ts` `types.ts` `client.ts` `invariant.ts`
- 导出面：插件 `name='session-stats'`、`inject=['sessionProjections']`、`apply`（注册 `sessionStats` 投影：turn/step 计数、LLM/tool/first-token/decode 墙钟时间）

### 1.8 session-telemetry — 遥测缝隙（`ctx.sessionTelemetry`）
- 依赖：`dsh-agent` `dsh-session` `cordis`
- src 模块：`index.ts` `coordinator.ts` `invariant.ts`
- 导出面：`SessionTelemetryBackend`（抽象 Service）、`SessionTelemetrySink`（`emit`/`flush?`/`shutdown`）、`SessionTelemetryCoordinator`、`SessionTelemetryRecord`（`channel:'ledger'|'ops'`、`severity`、`attributes`、`body`）、`SessionTelemetrySeverity`、`SessionTelemetrySharingStatus`
- 事件：`session-telemetry/record`（waterfall，脱敏扩展点）

### 1.9 session-telemetry-otel — OTel 后端
- 依赖：`dsh-command-feedback` `dsh-llm` `dsh-session` `dsh-session-telemetry` `dsh-anonymous-user-id` `cordis` + `@opentelemetry/*`
- src 模块：`index.ts` `invariant.ts`
- 导出面：`OpenTelemetrySessionBackend`、`SessionTelemetryMode`（`FULL`/`FEEDBACK_ONLY`/`DISABLED`，默认 `DISABLED`）、`Config`（`mode`/`exporter`/`processor`/`shutdownTimeoutMillis`）、`DEFAULT_SHUTDOWN_TIMEOUT_MILLIS`

### 1.10 session-title — 标题服务与提供者注册表（`ctx.sessionTitle`）
- 依赖：`dsh-brand` `dsh-llm` `dsh-session` `dsh-session-projection` `cordis` + `schemastery` `zod`
- src 模块：`index.ts` `normalize.ts` `types.ts` `client.ts` `invariant.ts`
- 导出面：`SessionTitleService`、`SessionTitleProvider`（`id`/`automatic:'first-prompt'|'all-prompts'`/`generate`）、`SessionTitleProviderId`、`SessionTitleSnapshot`/`SessionTitleEventData`/`SessionTitleSource`、`SessionTitleInvalidError`、`foldSessionTitle`/`collectSessionTitleMessages`/`normalizeSessionTitle`/`fallbackSessionTitle`/`truncateTitleUtf8`
- 事件（log-only）：`session/title`；可选投影 key `'title'`

### 1.11 session-title-llm — 共享 LLM 标题生成策略
- 依赖：`dsh-llm` `dsh-session` `dsh-session-title` `dsh-timeout` `cordis` + `schemastery`
- src 模块：`index.ts` `invariant.ts`
- 导出面：`registerSessionTitleLlmProvider`、`generateSessionTitleWithLlm`、`SessionTitleLlmConfig`/`ResolvedSessionTitleLlmConfig`、`SessionTitleLlmConfigFields`/`SessionTitleLlmConfigSchema`、`SESSION_TITLE_TIMEOUT_CODE`
- 事件（log-only）：`session/title-llm-request`

### 1.12 session-title-all-prompts-llm — all-prompts 标题提供者插件
- 依赖：`dsh-llm` `dsh-session` `dsh-session-title` `dsh-session-title-llm` `cordis` + `schemastery`
- src 模块：`index.ts` `invariant.ts`；导出：`name='session-title-all-prompts-llm'`、`inject=['sessionTitle','llm','sessions']`、`Config`、`apply`

### 1.13 session-title-first-prompt-llm — first-prompt 标题提供者插件
- 依赖：同上；src 模块：`index.ts` `invariant.ts`；导出：`name='session-title-first-prompt-llm'`、`inject`、`Config`、`apply`

**Rust 移植建议**
- crate 路径：`crates/session/` 下拆 `session-persistence`（trait + coordinator）、`session-persistence-jsonl`、`session-persistence-sqlite`、`session-projection`（registry + `ProjectionDefinition` trait）、`session-projection-cache`、`session-title`、`session-telemetry`、以及插件小 crate（checkpoint-policy/stats/telemetry-otel/title-*-llm）。
- 难点：① 事件溯源日志 + 崩溃修复（torn-tail truncate + synthetic closers）的跨后端一致性——`PersistenceCoordinator` 把「读/修/写编排」从「物理字节存取」里抽离，Rust 建议同构：`trait PersistenceBackend<TornMarker>` + `Coordinator`；② 持久化格式必须字节级兼容（见 §10，尤其 zstd 帧边界、packChunks 存储行、seq 连续性校验）；③ 投影 eager fold 的增量/缓存正确性（`restoreFloor` 的 seq-1 锚点、缓存行身份绑定 `createdAt+cwd`）。
- 依赖顺序：`dsh-session`（组外，先行）→ `session-persistence` → `session-persistence-jsonl`/`sqlite` → `session-projection` → `session-projection-cache`/`session-stats`/`session-title`；`session-checkpoint-policy`/`session-telemetry*` 依赖 `dsh-agent`/`dsh-llm`，排在会话层之后。

---

## 2. packages/llm（5 个子包）

**一句话用途**：Provider 中立的 LLM 服务接口 + DeepSeek/pi-ai 两个适配器 + 重试策略 + 重放感知的 token 计量。

### 2.1 llm — LLM 服务缝隙（`ctx.llm`）
- 依赖：`dsh-attachment` `dsh-brand` `dsh-invariants` `dsh-timeout` `cordis` + `schemastery`
- src 模块：`index.ts` `types.ts` `message.ts` `content.ts` `assembler.ts` `adapter-failure.ts` `api-key.ts` `attribution.ts` `brand.ts` `call-config.ts` `error.ts` `never.ts` `retry-policy.ts` `invariant.ts`
- 导出面：
  - 服务类：`LlmRuntime`（键 `'llm'`）、`LlmAdapter`（抽象，`stream` 唯一必选）
  - 注册 API：`registerAdapter(providers, adapter)`、`registerConfigurableProviders`、`registerModelDiscovery`、`prepareCall`、`resolveCallConfig`、`resolveModelInfo`、`listModels`、`listProviders`、`providerRetryPolicy`
  - 类型：`GenerateOptions` `Message` `ContentBlock` `StreamChunk`（`text-delta`/`reasoning-delta`/`tool-call`/`finish` 等）`TokenUsage` `FinishReason` `LlmFailure` `RetryPolicy`/`ResolvedRetryPolicy` `LlmModelInfo`/`LlmResolvedModelInfo`/`LlmProviderInfo` `ModelModality` `CallId`
  - 类/工具：`BlockAssembler`（chunk 装配）、`LlmError`/`HarnessError`、`createUserMessage`/`freezeMessage`、`assertUsableApiKey`/`normalizeApiKey`、`callConfigEquals`/`deepFreeze`/`isAgentLoopRequest`/`markAgentLoopRequest`、`attributionHeaders`、`CONTEXT_WINDOW_EXCEEDED_CODE`/`QUOTA_EXCEEDED_CODE`/`INVALID_CREDENTIAL_CODE`
  - 事件：`llm/stream`（waterfall，拦截每次流式调用）；emit 事件 `llm/adapters-updated`

### 2.2 llm-deepseek — DeepSeek chat-completions 适配器
- 依赖：`dsh-credentials` `dsh-launch-environment` `dsh-llm` `dsh-settings` `dsh-timeout` `dsh-anonymous-user-id` `cordis` + `eventsource-parser` `schemastery`
- src 模块：`index.ts` `adapter.ts` `serialize.ts` `sse.ts` `translate.ts` `types.ts` `invariant.ts`
- 导出面：`DeepSeekAdapter`、`name='llm-deepseek'`、`inject=['llm']`、`Config`（`apiKeyEnv`/`baseURL`/`thinking`/`reasoningEffort`/`maxTokens`/`defaultContextWindow`/`models`/`streamIdleTimeoutMs`/`retryPolicy`）、`resolveAdapterOptions`、`PUBLIC_BASE_URL`、`DEFAULT_CONTEXT_WINDOW`/`DEFAULT_MAX_TOKENS`/`DEFAULT_STREAM_IDLE_TIMEOUT_MS`、`ResolvedDeepSeekOptions`
- 注册 provider route：`deepseek-official`（默认模型 `deepseek-v4-flash`/`deepseek-v4-pro`）。详见 §11 协议。

### 2.3 llm-pi-ai — 通用 pi-ai 适配器
- 依赖：`dsh-attachment` `dsh-credentials` `dsh-launch-environment` `dsh-llm` `dsh-settings` `dsh-timeout` `cordis` + `@earendil-works/pi-ai` `schemastery`
- src 模块：`index.ts` `adapter.ts` `catalog.ts` `config.ts` `context.ts` `discovery.ts` `provider.ts` `replay.ts` `stream.ts` `invariant.ts`
- 导出面：`PiAiAdapter`、`name='llm-pi-ai'`、`inject=['llm']`、`Config`、`supportedProtocols`、`PiAiProviderProfile`/`PiAiModelProfile`/`PiAiCompatProfile`/`PiAiThinkingFormat` 等
- 一个插件实例可持多条 provider 路由；手声明路由可配 `api: openai-completions` 等协议与 `compat.thinkingFormat`（推理方言）。设计上作为 `llm-deepseek` 的「设计验证孪生」。

### 2.4 llm-retry — 按 provider 路由的重试策略
- 依赖：`dsh-brand` `dsh-agent` `dsh-llm` `dsh-session` `dsh-timeout` `cordis` + `schemastery`
- src 模块：`index.ts` `brand.ts` `history.ts` `types.ts` `invariant.ts`
- 导出面：`name='llm-retry'`、`inject=['agents']`、`apply`、`RetryId`、`LlmRetryEventData`/`LlmRetryStartedEventData`、`RetryInternals`
- 事件：`agent/request-error`（waterfall）；会话事件 `llm/retry`/`llm/retry-started`；退避 = 指数 + jitter，尊重 `Retry-After`；`mode:'always'`（无界）或 `'normal'`（maxRetries）

### 2.5 token-meter — 重放感知 token 计量（`ctx.tokenMeter`）
- 依赖：`dsh-compaction` `dsh-llm` `dsh-session` `dsh-session-projection` `cordis` + `schemastery` `zod`
- src 模块：`index.ts` `estimate.ts` `surface-fold.ts` `surface-projection.ts` `usage-projection.ts` `breakdown-projection.ts` `projection.ts` `types.ts` `client.ts` `invariant.ts`
- 导出面：`TokenMeter`（`measure`/`estimateMessage`）、`TokenMeasurement`/`TokenMeasurementBaseline`/`TokenSurfaceNode`、`estimateHeader`/`estimateMessage`/`estimateContent`/`ROLE_OVERHEAD`
- 可选注册 3 个投影：`tokenUsage`/`contextPressure`/`contextBreakdown`

**Rust 移植建议**
- crate 路径：`crates/llm/llm`（trait `LlmAdapter` + `LlmRuntime`）、`crates/llm/llm-deepseek`、`crates/llm/llm-pi-ai`、`crates/llm/llm-retry`、`crates/llm/token-meter`。
- 难点：① 流式协议与 SSE 解析（见 §11）；② `LlmRuntime` 的适配器注册 + `llm/stream` waterfall 拦截 + `prepareCall` 的「配置/注册绑定」一次性派发语义（HMR 安全）；③ `token-meter` 的重放折叠状态机（`request/header`、`step/start`、`step/end`、`assistant/message` 的 usage 锚点与 source seq 回放）；④ 重试策略的耐久化（每次重试在等待前落盘）。
- 依赖顺序：`dsh-attachment`/`dsh-timeout`/`dsh-session`（组外）→ `llm` → `llm-deepseek`/`llm-pi-ai`/`llm-retry`/`token-meter`。

---

## 3. packages/context（4 个子包）

**一句话用途**：把工作区/时间/tmux/跨会话等环境上下文注入到模型请求历史（均为 `agent/pre-step` 扩展点的消费者）。

### 3.1 agent-instructions — AGENTS.md/CLAUDE.md 指令加载器
- 依赖：`dsh-agent` `dsh-fs` `dsh-llm` `dsh-home-paths` `dsh-session` `dsh-tools` `cordis` + `schemastery`
- src 模块：`index.ts` `config.ts` `digest.ts` `files.ts` `render.ts` `state.ts` `invariant.ts`
- 导出面：`name='agent-instructions'`、`apply`、`Config`、`discoverBaselineInstructionFiles`/`loadBaselineInstructions`、`renderWorkspaceContext`、`InstructionFile`/`LoadedInstructionFile`/`RenderedWorkspaceContext`/`TruncatedInstruction`
- 触发：`agent/pre-step`（组上下文进 inbox）、`tools/result`（read/write/edit 触碰文件 → 刷新指令）、`session/event`（step 边界批量投递）

### 3.2 session-reference — 跨会话引用解析（`ctx.sessionReferenceResolver`）
- 依赖：`dsh-agent` `dsh-compaction` `dsh-llm` `dsh-output-retention` `dsh-session` `dsh-session-query` `cordis` + `schemastery`
- src 模块：`index.ts` `config.ts` `projection.ts` `serialization.ts` `types.ts` `uri.ts` `invariant.ts`
- 导出面：`SessionReferenceResolver`（`inject=['sessionQuery']`）、`SessionReferenceError`/`SessionReferenceErrorCode`、`SESSION_REFERENCE_SCHEME`/`encodeSessionReferenceUri`/`decodeSessionReferenceUri`/`formatSessionReferenceMention`/`parseSessionReferenceText`、`MAX_REFERENCES`/`DEFAULT_CANDIDATE_LIMIT`/`DEFAULT_MAX_REFERENCE_BYTES`
- 语义：只读、不可信快照（`<referenced-sessions>` JSON），按 cwd 亲和度排序候选，字节预算裁剪。

### 3.3 time-context — 时间上下文注入（插件）
- 依赖：`dsh-agent` `dsh-session` `cordis` + `schemastery`
- src 模块：`index.ts` `request-zone.ts` `timestamp.ts` `invariant.ts`
- 导出面：`name='time-context'`、`inject=['agents']`、`apply`、`Config`（`timeZone`/`refreshIntervalMs`）
- 触发：`agent/pre-step`（`prepend:true`），每步或按刷新间隔注入「当前时间 + 距上一消息耗时」。

### 3.4 tmux-context — tmux 位置上下文注入（插件）
- 依赖：`dsh-agent` `dsh-shell` `dsh-session` `cordis` + `schemastery`
- src 模块：`index.ts` `invariant.ts`
- 导出面：`name='tmux-context'`、`inject=['agents']`、`apply`、`Config`（`refreshIntervalMs`）
- 触发：`agent/pre-step`（`prepend:true`，仅 `step===1`），通过 `ctx.shell` 跑 `tmux display-message` 且校验 `$TMUX_PANE` 的 `pane_tty` 等于本进程 tty；状态未变则不重复注入。

**Rust 移植建议**
- crate 路径：`crates/context/agent-instructions`、`session-reference`、`time-context`、`tmux-context`。
- 难点：agent-instructions 的基线/作用域 diff（`InstructionVersionCache`、`baselineIdentity`）与「文件触碰 → 收件箱」异步投影串行化（`projectionTails`）；session-reference 的不可信上下文围栏与 URI 编解码；time/tmux 的注入去重（读历史日志 fold 而非进程内状态）。
- 依赖顺序：`dsh-fs`/`dsh-session-query`/`dsh-shell`（组外）→ 各 context 包。

---

## 4. packages/compaction（4 个子包）

**一句话用途**：把过长的会话历史压缩成摘要节点（LLM 摘要 + 无模型工具结果裁剪），并提供 `/compact` 命令。

### 4.1 compaction — 压缩缝隙（`ctx.compaction`）
- 依赖：`dsh-brand` `dsh-commands` `dsh-llm` `dsh-session` `cordis`
- src 模块：`index.ts` `brand.ts` `checkpoint.ts` `tool-pairing.ts` `types.ts` `invariant.ts`
- 导出面：`CompactionEngine`（抽象 Service）、`CompactionResult`、`CompactionTrigger`（`pressure`|`context-overflow`）、`ManualCompactionError`/`ManualCompactionErrorCode`、`CompactionId`、`compactCheckpointSource`/`isCompactCheckpointSource`/`CompactionCheckpointSource`、`toolPairingBalancedBefore`/`toolPairingBalancedAfter`
- 抽象方法：`compactIfNeeded`/`compactNow`/`compactRegion`

### 4.2 compaction-basic — 基础压缩后端
- 依赖：`dsh-agent` `dsh-compaction` `dsh-commands` `dsh-llm` `dsh-session` `dsh-token-meter` `dsh-compaction-tool-result-pruner`(可选) `cordis` + `schemastery`
- src 模块：`index.ts` `config.ts` `region.ts` `summarizer.ts` `types.ts` `invariant.ts`
- 导出面：`BasicCompactionEngine`（`inject=['llm','tokenMeter','sessions']`）、`BasicCompactionConfig`/`ModelCompactPolicyConfig`/`ResolvedConfig` 等
- 触发：`agent/pre-step`（pressure 阈值）、`agent/request-error`（`CONTEXT_WINDOW_EXCEEDED` → overflow 恢复）、`agent/status` 与 `session/event`（重置溢出重试计数）

### 4.3 compaction-tool-result-pruner — 无模型工具结果裁剪（`ctx.toolResultPruner`）
- 依赖：`dsh-compaction` `dsh-llm` `dsh-session` `dsh-token-meter` `cordis` + `schemastery`
- src 模块：`index.ts` `config.ts` `types.ts` `invariant.ts`
- 导出面：`ToolResultPruner`（`inject=['tokenMeter']`）、`ToolResultPruneConfig`/`PruneResult`/`PrunedEntry`、`codePointLength`/`DEFAULTS`/`PRUNE_MARKER`
- 语义：head/middle/tail 字符预算裁剪，`surfaceOp:replace` 替换 + `compaction/prune` 影子计价事件（可重放、纯消费者可扣除）。

### 4.4 command-compact — `/compact` 命令
- 依赖：`dsh-commands` `dsh-compaction` `cordis`
- src 模块：`index.ts` `invariant.ts`
- 导出面：`name='command-compact'`、`inject=['commands','compaction']`、`apply`（注册 `compact` 命令，转调 `compactNow`）

**Rust 移植建议**
- crate 路径：`crates/compaction/compaction`、`compaction-basic`、`compaction-tool-result-pruner`、`command-compact`。
- 难点：① surface 范围替换（按 surface 位置而非 seq 顺序，可见 seq 可非单调）与 tool-call/tool-result 配对边界检查；② 摘要 LLM 调用的「复用同 system/tools/messages 前缀以不失效 KV cache」；③ 摘要与修剪的耐久化顺序（`compaction/start`→…→`compaction/end` 标记对、`compaction/prune` 影子事件）；④ 手动压缩的 `runMaintenance`（agent 空闲串行化）。
- 依赖顺序：`dsh-commands`/`dsh-token-meter`/`dsh-llm` → `compaction` → `compaction-basic`/`compaction-tool-result-pruner` → `command-compact`。

---

## 5. packages/interaction（5 个子包）

**一句话用途**：人机交互面——斜杠命令注册表、用户批准缝隙、提问缝隙、以及权限预设。

### 5.1 commands — 人类命令注册表（`ctx.commands`）
- 依赖：`dsh-agent` `dsh-brand` `dsh-scope` `dsh-session` `dsh-typert-protocol` `cordis` + `zod`
- src 模块：`index.ts` `brand.ts` `types.ts` `invariant.ts`
- 导出面：`CommandRuntime`（`TypertRemoteService`，`@Remote` 的 `list`/`execute`）、`CommandDefinition`/`CommandInvocation`/`CommandDescriptor`/`CommandResult`/`CommandExecution`、`parseCommand`、`CommandId`
- 事件（log-only）：`command/run`、`command/done`；emit 事件 `commands/change`；scoped 层支持 per-agent 覆盖。

### 5.2 permission-presets — 权限预设（`ctx.permissionPresets`）
- 依赖：`dsh-shell` `dsh-commands` `dsh-sandbox` `dsh-sandbox-policy` `dsh-session` `dsh-session-projection` `dsh-settings` `dsh-user-approval` `cordis` + `schemastery` `zod`
- src 模块：`index.ts` `types.ts` `client.ts` `invariant.ts`
- 导出面：`PermissionPresetService`（`inject=['shell','approval','sessions']`）、`Config`/`PresetSpec`、`effectivePermissionPreset`/`applyKnobEvent`、`CUSTOM_PRESET`/`PERMISSION_SETTINGS_NAMESPACE`
- 事件（log-only）：`permission/preset`；投影 key `'permissions'`；命令 `/permission`；默认预设 `workspace-write`/`danger-full-access`。

### 5.3 tool-ask-user — `ask_user_question` 工具
- 依赖：`dsh-agent` `dsh-tools` `dsh-user-questions` `cordis`
- src 模块：`index.ts` `invariant.ts`
- 导出面：`name='tool-ask-user'`、`inject=['tools','userQuestions']`、`apply`（`defineTool` 注册 `ask_user_question`，转调 `ctx.userQuestions.ask`）

### 5.4 user-approval — 用户批准缝隙（`ctx.approval`）
- 依赖：`dsh-agent` `dsh-brand` `dsh-llm` `dsh-scope` `dsh-session` `dsh-system-prompt` `cordis` + `schemastery`
- src 模块：`index.ts` `types.ts` `invariant.ts`
- 导出面：`ApprovalService`、`ApprovalRequest`/`ApprovalOutcome`（`allowed-once`/`rejected`/`cancelled`/`unavailable`）、`ApprovalPolicy`（`ask`|`never`）、`APPROVAL_POLICIES`、`effectiveApprovalPolicy`/`setApprovalPolicy`、`ApprovalRequestId`
- 事件：`approval/request`（waterfall，scope-filtered）；log-only 会话事件 `approval/asked`/`approval/decided`/`approval/policy`。默认 fail-closed。

### 5.5 user-questions — 用户提问缝隙（`ctx.userQuestions`）
- 依赖：`dsh-agent` `dsh-llm` `cordis`
- src 模块：`index.ts` `types.ts` `invariant.ts`
- 导出面：`UserQuestionService`（`registerProvider`/`ask`）、`UserQuestionProvider`、`AskUserQuestionRequest`、`AskUserQuestionItem`/`AskUserQuestionAnswer`/`AskUserQuestionIntent`/`AskUserQuestionOption`、`UserQuestionError`

**Rust 移植建议**
- crate 路径：`crates/interaction/commands`、`user-approval`、`user-questions`、`permission-presets`、`tool-ask-user`。
- 难点：① commands 的 scoped 层（global + per-agent 阴影）与 `@Remote` 的 typert 协议投影；② approval 的 waterfall 分发 + `never` 策略在服务自身请求路径内确定性拒绝（不依赖监听器顺序）+ 审计事件对（asked/decided）必须在 turn 内；③ user-questions 的 live-agent 校验（`CALLER_NOT_LIVE`/`DELEGATED_CALLER`）；④ permission-presets 把 sandbox-mode 与 approval-policy 两个 knob 写回各自规范 setter，并持久化为 `permission/preset` 意图事件。
- 依赖顺序：`dsh-tools`/`dsh-sandbox-policy`/`dsh-system-prompt`（组外）→ `commands`/`user-approval`/`user-questions` → `permission-presets`/`tool-ask-user`。

---

## 6. packages/attachment（2 个子包）

**一句话用途**：不可变二进制附件（图片）存储缝隙 + 内容寻址的本地实现。

### 6.1 attachment — 附件存储缝隙（`ctx.attachments`）
- 依赖：`dsh-brand` `dsh-invariants` `cordis`
- src 模块：`index.ts` `brand.ts` `error.ts` `types.ts` `invariant.ts`
- 导出面：`AttachmentStore`（抽象 Service）、`AttachmentId`、`AttachmentError`、`ImageAttachmentLimits`/`ImageAttachmentRef`/`StoredImageAttachment`/`SaveImageAttachment`/`ImageMediaType`
- 方法：`validateImage`/`saveImage`/`readImage`（readImage 校验字节仍匹配引用）

### 6.2 attachment-local — 本地内容寻址后端
- 依赖：`dsh-attachment` `dsh-home-paths` `cordis` + `schemastery` `sharp`
- src 模块：`index.ts` `image.ts` `store.ts` `invariant.ts`
- 导出面：`LocalAttachmentStore`、`Config`（`dshHome`/`maxImageBytes`/`maxImagesPerMessage`/`maxMessageImageBytes`/`maxImagePixels`）、`DEFAULT_MAX_IMAGE_BYTES`(5MB)/`DEFAULT_MAX_IMAGES_PER_MESSAGE`(20)/`DEFAULT_MAX_MESSAGE_IMAGE_BYTES`(100MB)/`DEFAULT_MAX_IMAGE_PIXELS`(40M)、`detectImage`
- 存储根：`{DSH_HOME}/attachments/v1`，内容寻址；用 `sharp` 做栅格解码校验。

**Rust 移植建议**
- crate 路径：`crates/attachment/attachment`、`attachment-local`。
- 难点：图片内容寻址（哈希路径）与「引用=内容摘要」校验；`sharp` 栅格解码 → Rust 用 `image`/`zune-*` crate 替换。
- 依赖顺序：`dsh-home-paths`（组外）→ `attachment` → `attachment-local`。

---

## 7. packages/feedback（2 个子包）

**一句话用途**：会话/消息级反馈的记录与持久化（用户反馈 + 逐消息评分/备注）。

### 7.1 command-feedback — `/feedback` 命令与 `feedback/record` 事件
- 依赖：`dsh-commands` `dsh-session` `dsh-session-telemetry` `dsh-anonymous-user-id` `cordis`
- src 模块：`index.ts` `invariant.ts`
- 导出面：`name='command-feedback'`、`inject=['commands']`、`apply`、`recordFeedback`
- 事件（log-only）：`feedback/record`（`{text}`）；命令 `/feedback <text>`

### 7.2 message-feedback — 逐消息评分/备注侧车（`ctx.messageFeedback`）
- 依赖：`dsh-brand` `dsh-llm` `dsh-session` `dsh-session-persistence` `dsh-storage-domain` `dsh-typert-protocol` `cordis` + `schemastery` `zod`
- src 模块：`index.ts` `spec.ts` `types.ts` `invariant.ts`
- 导出面：`MessageFeedbackService`（`TypertRemoteService`，`@Remote('list'/'put'/'delete')`）、`Config`（`maxNoteBytes`）、`messageFeedbackDomainSpec`/`messageFeedbackRowSchema`/`messageFeedbackItemSchema` 等、`MessageFeedbackRow`/`MessageFeedbackItem`/`MessageFeedbackVersion`
- 语义：storage-domain 侧车，绑定 `createdAt+cwd` 身份；乐观版本并发（`ifVersion`）；耐久屏障（live flush / cold re-read）后才写侧车。

**Rust 移植建议**
- crate 路径：`crates/feedback/command-feedback`、`message-feedback`。
- 难点：message-feedback 的版本冲突语义、按 session 串行化操作队列（`operationTails`）、身份绑定（id 复用/换 store 不串台）。
- 依赖顺序：`dsh-session-persistence`/`dsh-storage-domain` → `message-feedback`；`dsh-commands`/`dsh-session-telemetry` → `command-feedback`。

---

## 8. packages/hooks（3 个子包）

**一句话用途**：Claude Code / Codex 命令行 hook 的共享 wire 协议 + 两个桥接插件。

### 8.1 hook-protocol — 共享 hook 协议（非插件库）
- 依赖：`dsh-shell` `dsh-session` `cordis`
- src 模块：`index.ts` `codec.ts` `detached.ts` `events.ts` `matcher.ts` `merge.ts` `runner.ts` `types.ts` `invariant.ts`
- 导出面：`matchesMatcher`/`matcherDiagnostic`、`parseHookOutput`、`runHook`/`RunHookOptions`/`RunHookResult`/`DEFAULT_HOOK_TIMEOUT_MS`、`mergeHookOutputs`/`MergedDecision`/`MergedHookOutcome`、`appendHookInvoked`/`appendHookResult`/`summarizeStderr`/`DEFAULT_STDERR_SUMMARY_MAX_CHARS`/`HookInvocation`/`HookResultRecord`、`createDetachedRuns`/`DetachedRuns`、`CommandHook`/`HookDialect`/`HookOutput`/`MatcherGroup`/`MatcherMode`
- 语义：matcher（regex/literal）+ stdin(JSON payload)/stdout/exit-code codec + 多 hook 限制性合并 + 会话事件 `hook/*`。详见 §12。

### 8.2 hooks-claude-code — Claude Code hook 桥
- 依赖：`dsh-agent` `dsh-hook-protocol` `dsh-llm` `dsh-session` `dsh-session-persistence` `dsh-subagent` `dsh-tools` `cordis` + `schemastery`
- src 模块：`index.ts` `config.ts` `invariant.ts`
- 导出面：`name='hooks-claude-code'`、`inject=['shell']`、`apply`、`Config`（`configPath`/`pluginRoot`/`projectDir`/`defaultTimeoutMs`/`stderrSummaryMaxChars`）
- 支持点：`SessionStart`/`UserPromptSubmit`/`PreToolUse`/`PostToolUse`/`Stop`/`SubagentStart`/`SubagentStop`；camelCase payload、`${CLAUDE_PROJECT_DIR}`/`${CLAUDE_PLUGIN_ROOT}` 替换、`CLAUDE_PROJECT_DIR` env。

### 8.3 hooks-codex — Codex hook 桥
- 依赖：`dsh-agent` `dsh-hook-protocol` `dsh-llm` `dsh-session` `dsh-session-persistence` `dsh-tools` `cordis` + `schemastery`
- src 模块：`index.ts` `config.ts` `invariant.ts`
- 导出面：`name='hooks-codex'`、`inject=['shell']`、`apply`、`Config`（`configPath`/`model`/`defaultTimeoutMs`/`stderrSummaryMaxChars`）
- 支持点：`SessionStart`/`UserPromptSubmit`/`PreToolUse`/`PostToolUse`/`Stop`；regex-only matcher、snake_case payload、无 trailing newline、仅 blocking 决策。

**Rust 移植建议**
- crate 路径：`crates/hooks/hook-protocol`、`hooks-claude-code`、`hooks-codex`。
- 难点：子进程 stdin/stdout/exit-code 编解码、超时/中止（detached run 排空）、多 hook 结果限制性合并（最严格决策）、两方言 payload/替换规则差异（Claude 有 env 替换 + 环境变量，Codex 仅 regex + snake_case）。
- 依赖顺序：`dsh-shell`（组外）→ `hook-protocol` → `hooks-claude-code`/`hooks-codex`（再依赖 `dsh-tools`/`dsh-subagent`）。

---

## 9. packages/guard（2 个子包）

**一句话用途**：对 agent 工具循环的守卫——重复调用提醒与工具调用超时。

### 9.1 repeat-tool-reminder — 重复工具调用提醒（插件）
- 依赖：`dsh-agent` `dsh-tools` `cordis` + `schemastery`
- src 模块：`index.ts` `invariant.ts`
- 导出面：`name='repeat-tool-reminder'`、`apply`、`Config`（`thresholds`[默认 3,5,8]、`include`/`exclude`、`argumentsPreviewChars`）
- 触发：`tools/post-execute`（观察并计数，不否决，注入 `additionalContext` 提醒）、`agent/pre-step`（用户消息时重置链）。参数深度 key-sort 规范化后比较。详见 §12。

### 9.2 timeout-policy — 工具调用超时策略（插件）
- 依赖：`dsh-llm` `dsh-timeout` `dsh-tools` `cordis`
- src 模块：`index.ts` `invariant.ts`
- 导出面：`name='timeout-policy'`、`inject=['tools']`、`apply`、`TOOL_TIMEOUT`
- 触发：`tools/execute` 包装器——按工具的 `timeoutMs` 在 `exec.signal` 上装 deadline，临时替换 signal 派发，超时后返回 `TOOL_TIMEOUT` 结构化结果。详见 §12。

**Rust 移植建议**
- crate 路径：`crates/guard/repeat-tool-reminder`、`timeout-policy`。
- 难点：repeat-tool-reminder 的规范化比较（参数深层键排序）+ 通配符 include/exclude；timeout-policy 的 deadline 作用域（`timeoutOf(signal, TOOL_TIMEOUT)` 区分本插件的超时与嵌套外层 deadline）。
- 依赖顺序：`dsh-tools`/`dsh-timeout`（组外）→ 两个 guard 包。

---

## 10. 特别说明 A：session 持久化目录结构与文件格式

### 10.1 JSONL 后端（session-persistence-jsonl）
目录布局（`format.ts`）：
```
{root}/
├── {projectKey(cwd)}/            # 项目目录；cwd 为 undefined 时用 _no-cwd
│   └── {encodeSegment(id)}/      # 会话目录
│       └── session.jsonl         # 或 session.jsonl.zstd（按 compression）
└── …
```
- `projectKey(cwd)`：把 cwd 变成人类可读目录名，形如 `--slug--`（分隔符 `/ \ :` → `-`，不安全码元 → `~XXXX`，截断到 251 字符，空则 `root`）。**有损**（仅用于导航分组，不用于定位）。
- `encodeSegment(id)`：把 `SessionId` 编码为单一路径段（**无损、单射**）。安全字符 `[A-Za-z0-9._-]` 原样保留，`~` 特判，其余码元 → `~XXXX`（4 位大写十六进制，UTF-16 码元）；`.` → `~002E`、`..` → `~002E~002E`。防止路径穿越/绝对路径/NUL。

文件格式（每会话一个追加式文件）：
- 第 1 行 = **header 记录**（JSON 单行）：`{"type":"session","version":<SESSION_FORMAT_VERSION>,"id":…,"createdAt":…,"cwd"?,"parentSession"?,"seedLength"?,"origin"?:("subagent"),"delegationDepth":…,"agentPreset"?}`。读取前先校验 `version`，未知版本抛 `SessionFormatUnsupportedError`（提示「升级 harness」而非「损坏」）。
- 后续行 = **事件记录**（JSONL）。`packChunks:true`（默认）时，连续 `assistant/chunk` 增量会打包成 `text-chunks` / `reasoning-chunks` / `tool-call-chunks` 存储行（无损、约省 60%）；读取端无差别（`scanLog` 一律按存储行解码）。每个事件的 `data` 必须 JSON 可序列化（`append` 校验）。
- 物理编码：`compression:'zstd'`（默认）= 多个可独立解码的 Zstandard 帧串联，**首帧恰好是 header 单行**，其后每帧是事件批；`'none'` = 明文 UTF-8 JSONL。
- 耐久性：临时文件写入 + `fsync` + `link()`（POSIX，避免 `rename` 覆盖）+ 目录 `fsync`；追加用 `append+fsync`，部分写失败 `truncate` 回滚；torn 尾修复 = `truncate` 到 committed 字节 + 重放恢复的事件 + 追加合成 closers。
- 修订号（revision）：`dev:ino:size:mtimeNs:ctimeNs` 拼接（stat bigint）。列表只读首帧/首行（header），随会话数而非日志大小扩展。

### 10.2 SQLite 后端（session-persistence-sqlite）
- 单库文件（`config.path` 或 `:memory:`），`SCHEMA_VERSION=15`，`application_id=0x44534850`，`PRAGMA foreign_keys=ON`，默认 `journal_mode=wal`。
- 表：
  - `persistence_state(singleton INTEGER PK CHECK(=1), store_id TEXT)` —— 库级身份。
  - `sessions(id TEXT PK, version, created_at, cwd?, parent_session?, seed_length?, origin?, delegation_depth?, agent_preset?, incarnation TEXT, revision INTEGER)` —— 行存在即「已物化」（惰性物化，create 不落盘）。
  - `events(session_id FK→sessions ON DELETE CASCADE, seq, type, time, data TEXT(JSON), source_event_seqs TEXT(JSON)?, surface_op TEXT(JSON)?, ignorable INTEGER?, PK(session_id,seq))`。
- 事件 1:1 映射为行，`data` 存 JSON 文本；`source_event_seqs`/`surface_op` 为 JSON 编码的可空列，`ignorable` 为 `1|NULL`。
- 事务语义：`appendBatch` 单事务写 rows+events 并 `revision+1`；`commitRepair` 单事务 `DELETE torn tail + INSERT closers`。torn tail 判定与 JSONL 一致：最后一个 `turn/end` 之后的 seq 空洞/坏行才被容忍。

---

## 11. 特别说明 B：llm 包支持的模型协议

### 11.1 llm-deepseek（直接 fetch + SSE，OpenAI 兼容 chat-completions）
- 端点：`POST {baseURL}/chat/completions`（`baseURL` 默认 `https://api.deepseek.com`，可被 `$DEEPSEEK_BASE_URL` 覆盖）。
- 头：`authorization: Bearer <key>`、`content-type: application/json`、`accept: text/event-stream`、`attributionHeaders()`、`x-deepseek-harness-user-id`/`x-deepseek-harness-session-id`/`x-deepseek-harness-compact`。
- 请求体（`serializeRequest`）：`model`、`messages`（system/user/assistant/tool 四角色；harness 的 tool-result 块展开为独立 `{role:'tool', tool_call_id, content}`）、`stream:true`、`stream_options:{include_usage:true}`、`thinking:{type:'enabled'|'disabled'}`、`reasoning_effort:'off'|'high'|'max'`、`tools`、`temperature`、`max_tokens`、`stop`。**始终流式**。
- 流式：SSE 解析（`parseSse`）→ `translate` → harness `StreamChunk`（文本增量/推理增量/工具调用/finish）。DeepSeek 思考模式：`reasoning_content` 仅在 tool-call 回合回传（官方 passback 规则），纯文本回合丢弃以省 token。
- 错误：HTTP 状态 → `LlmError` code（401/403→`AUTH`，429→`RATE_LIMIT`，400 内文案含上下文溢出→`CONTEXT_WINDOW_EXCEEDED`，>=500→`SERVER`，其余 `HTTP_<status>`）；尊重 `Retry-After`（秒或 HTTP 日期）与 `x-request-id`/`x-deepseek-request-id`；流空闲看门狗 → `TIMEOUT`。
- 能力：`inputModalities:['text']`（**纯文本，拒绝图片**）；thinking/effort 元数据暴露 `reasoning.efforts`（off/high/max）。

### 11.2 llm-pi-ai（库支撑，多协议）
- 基于 `@earendil-works/pi-ai`，可服务多条 provider 路由；手声明路由可配 `api`（如 `openai-completions`，另有 anthropic 等）与 `compat.thinkingFormat`（如 `deepseek`）推理方言。
- 同为流式 `LlmAdapter`；路由集/重试策略变更时原子 `replace` 重注册。

### 11.3 统一契约（llm 核心）
- `LlmRuntime.stream(options) -> AsyncIterable<StreamChunk>`，被 `llm/stream` waterfall 包裹（重试、重放、路由、检查点都可拦截）。
- `StreamChunk` 终态为 `finish`，`FinishReason`：`stop`/`error`/`aborted`/`max-tokens`/`tool-calls`。
- `BlockAssembler` 把 chunk 流装配为 `ContentBlock[]`（text/reasoning/tool-call/tool-result）。

---

## 12. 特别说明 C：guard/hooks 的触发机制

### 12.1 hooks 触发机制（hook-protocol + 两桥）
执行原语（`runner.ts`）：`runHook(shell, hook, {payload, cwd, env, signal, defaultTimeoutMs, expectedEventName})` 通过 `ctx.shell` 起子进程，把 JSON payload 写到 **stdin**，读 **stdout + exit code**，用 `parseHookOutput` 解码为 `HookOutput`（`decision`/`additionalContext`/`systemMessage`/`updatedInput`/`hookSpecificOutput`）；`mergeHookOutputs` 取最严格决策。每 hook 在会话日志写 `hook/invoked`→`hook/result` 对（带 stderr 摘要）。

桥接点在 Cordis 事件上（均 `ctx.on(...)`）：
| Hook 点 | Claude Code | Codex | 触发事件 |
|---|---|---|---|
| SessionStart | ✅（emit 型，detached） | ✅（emit 型，detached） | `agent/session-start` |
| UserPromptSubmit | ✅ | ✅ | `agent/pre-step`（→ `PreStepDecision`，deny→reject / 附 context） |
| PreToolUse | ✅（deny/ask） | ✅（仅 deny） | `tools/pre-execute`（→ `PreToolDecision`） |
| PostToolUse | ✅（block+context） | ✅（block+context） | `tools/post-execute`（→ `PostToolDecision`） |
| Stop | ✅（block→`agent.steer` 强制续） | ✅（同） | `agent/turn-stopping` |
| SubagentStart/Stop | ✅ | ❌ | `subagent/start` / `subagent/end` |

匹配：Claude Code matcher 支持 literal/regex；Codex 仅 regex。payload 方言：CC 用 camelCase + `${CLAUDE_PROJECT_DIR}`/`${CLAUDE_PLUGIN_ROOT}` 替换 + `CLAUDE_PROJECT_DIR` env；Codex 用 snake_case + 无 trailing newline + `model` 字段 + `permission_mode`。两者都带 `session_id`/`transcript_path`/`cwd`/`hook_event_name`。

### 12.2 guard 触发机制
- **timeout-policy**：`ctx.on('tools/execute', wrapper)` —— 读取工具声明的 `timeoutMs`，用 `deadline(exec.signal, timeoutMs, TOOL_TIMEOUT)` 派生 signal，临时替换 `exec.signal` 后 `next()` 派发，`finally` 恢复上游 signal；若本插件 deadline 触发（`timeoutOf(d.signal, TOOL_TIMEOUT)`），返回结构化 `TOOL_TIMEOUT` 结果（`isError:true`），否则透传。**不丢弃/不竞速工具 promise**（工具自行响应 abort 到达静默态）。
- **repeat-tool-reminder**：`ctx.on('tools/post-execute')` —— 对 agent-loop 调用把 `exec.arguments` 深键排序后 `JSON.stringify` 规范化，与上一调用 key 比较形成连续重复计数；命中 `thresholds` 时把提醒作为 `additionalContext` 注入（gentle 首档 + detailed 后续档），**只观察不否决**（`next()` 后折叠到下游决策）。`ctx.on('agent/pre-step')` 看到用户消息即重置链（跨用户插话不算循环）。

---

## 13. 依赖顺序总览（组 D 内部 + 关键组外依赖）

组外先决（非本组）：`@deepseek-ai/dsh-session`（Session/SessionEvent/SessionHeader/StorageRecord 编解码，**本组几乎所有包的地基**）、`dsh-agent`/`dsh-tools`/`dsh-agent-loop`（模型循环宿主）、`dsh-timeout`、`dsh-brand`、`dsh-invariants`、`dsh-storage-domain`/`dsh-storage-*`、`dsh-shell`、`dsh-commands`、`dsh-session-query`、`cordis`。

组 D 建议移植顺序：
1. **llm**（`LlmRuntime`/`LlmAdapter`/`StreamChunk`/`BlockAssembler`/retry-policy）—— 被 session-title、compaction、checkpoint-policy、guard、hooks 等广泛消费。
2. **session-persistence**（trait + `PersistenceCoordinator`）→ **session-persistence-jsonl** / **session-persistence-sqlite**（§10 格式字节级兼容）。
3. **session-projection** → **session-projection-cache** / **session-stats** / **session-title**。
4. **compaction** → **compaction-basic** / **compaction-tool-result-pruner** / **command-compact**。
5. **interaction**（commands / user-approval / user-questions / permission-presets / tool-ask-user）。
6. **context**（agent-instructions / session-reference / time-context / tmux-context）。
7. **hooks**（hook-protocol → 两桥）、**guard**、**attachment**、**feedback**、**session-checkpoint-policy** / **session-telemetry**（依赖 agent/llm，最后收口）。

---

*盘点方法：glob 枚举 `packages/<组>/**/src/**` + read 读 package.json / src/index.ts（40 子包），grep spill 文件做路径清单；关键持久化/协议文件深读 `format.ts`/`schema.ts`/`adapter.ts`/`serialize.ts`。*
