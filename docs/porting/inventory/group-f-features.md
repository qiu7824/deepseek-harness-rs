# 组 F：功能特性与辅助包清单（Rust 1:1 移植盘点）

> 只读分析。源码根 `D:\HermesTemp\deepseek-harness`，输出目录 `deepseek-harness-rs`。
> 包布局为两级 `packages/<domain>/<pkg>`，每个包用 `tsdown` 编译 `src/*.ts` → `lib/`，入口 `src/index.ts`，并普遍携带 `src/invariant.ts`（注册到 `@deepseek-ai/dsh-invariants` 的包级运行时契约检查）。
> 所有服务通过 Cordis 声明合并挂到 `Context`（`ctx.xxx`），模型工具通过 `ctx.tools.register(defineTool(...))` 注册，配置用 `@deepseek-ai/schemastery` 的 `z.object` 描述。npm 包名统一 `@deepseek-ai/dsh-*`。

---

## 0. 共性说明（Rust 移植先决）

- 本组所有包都建立在组外的“底座”包之上，端口顺序必须先落地这些底座：`cordis`（DI/事件）、`dsh-brand`（branded type）、`dsh-invariants`、`dsh-session`（事件溯源会话日志 `Session.append`/`SessionEventMap`）、`dsh-session-projection`（投影单元 `SessionProjectionMap`）、`dsh-llm`（`HarnessError`、`ContentBlock`、`MessageSourceMap`）、`dsh-agent`（`ctx.agents`、agent 生命周期事件）、`dsh-tools`（`defineTool`/`ObjectJsonSchema`）、`dsh-system-prompt`（`ctx.systemPrompt.section`）、`dsh-scope`（分层 scope）、`dsh-subprocess`/`dsh-timeout`（子进程与超时预算）、`dsh-typert-protocol`（`TypertRemoteService`/`@Remote` 跨进程 RPC）、`dsh-jobs`（后台任务）。
- 类型品牌（`Branded<'GoalId'>` 等）→ Rust `newtype`；`schemastery`/`zod` → serde + 自研 JSON-Schema 描述层（工具 schema 是 JSON-Schema 子集，见 `dsh-tools` 的 `assertObjectJsonSchema`）。
- 每个包惯用 `declare module '@deepseek-ai/cordis' { interface Context {...} interface Events {...} }` 声明合并 —— Rust 侧对应一个中央 service/event 注册表 trait，而不是全局可变单例。

---

## 1. goal 域（4 包）— 同会话目标状态机

### 1.1 `@deepseek-ai/dsh-goal`（packages/goal/goal）
- **用途**：事件溯源（event-sourced）的同会话 goal 状态与生命周期服务 `ctx.goals`。
- **依赖（workspace peer）**：dsh-agent、dsh-session、dsh-session-projection、dsh-brand、dsh-invariants、dsh-llm、dsh-scope、dsh-typert-protocol、cordis；schemastery、zod。
- **src 模块**：`index.ts`、`domain.ts`、`types.ts`、`fold.ts`、`runtime.ts`、`invariant.ts`、`client.ts`。
- **导出面**：
  - 服务：`GoalService`（`ctx.goals`，`static inject=['agents']`，`extends TypertRemoteService`，远程方法 `create/edit/pause/resume/complete/clear`）。
  - 事件：`goal/changed`（agent 作用域 emit，payload `{agent, change}`）；会话事件 `goal/change`（durable，`GoalChangeMeta` = snapshot 或 clear tombstone）。
  - 投影单元：`goal`（`SessionProjectionMap.goal: GoalProjection|null`，last-wins 全量 fold，`stateVersion: 4`）。
  - 类型：`GoalId`、`GoalRef`（CAS 身份）、`GoalSnapshot`、`GoalView`、`GoalPhase`、`GoalActivation`、`GoalBlockReason`、`GoalErrorCode`、`GoalOperation`。
- **状态机（特别说明）**：
  - 持久 phase：`active | paused | blocked | complete`（`GoalPhase`）。
  - 进程本地激活态：`armed | disarmed`（`GoalActivation`，永不持久化，决定“本进程是否可自动续轮”）。
  - 变更动词（`GoalOperation`）：`create | edit | pause | resume | complete | block | clear`。
  - 所有变更走 compare-and-set（`GoalRef = {id, revision}`，每次变更 revision+1），陈旧 revision → `GOAL_STALE_REVISION`。
  - 轮次预算：`roundsStarted`（已启动轮数）+ `maxGoalRounds`（上限），`resume` 前检查是否耗尽；`block` 需带稳定 `code`（kebab-case）+ `message`。
  - 写路径 `commit()` 先 `session.append('goal/change', change)` 再同步本地缓存并 `emit('goal/changed')`；`session-start` 时 disarm。
- **Rust 建议**：`crates/dsh-goal`。难点：CAS 与事件溯源 replay fold（`fold.ts` 严格 fold 与投影轻量 fold 两套）、`TypertRemoteService` 的跨进程 RPC 桥。类型纯值可先行。

### 1.2 `@deepseek-ai/dsh-goal-round-driver`（packages/goal/goal-round-driver）
- **用途**：竞态防护的同会话目标轮驱动（自动续轮）。
- **依赖**：dsh-agent、dsh-goal、dsh-llm、dsh-session、dsh-invariants、cordis。
- **src 模块**：`index.ts`、`prompt.ts`、`invariant.ts`。
- **导出面**：`apply(ctx)` 插件 + `renderGoalRoundPrompt`。监听 `agent/created|disposed|status|session-start|error|inbox/*`、`goal/changed`、`session/event`，在 agent 空闲（`idle`）且 goal `active+armed` 且未超预算时，向 inbox 排队下一轮提示（`source.kind='goal'`）；`agent/pre-step` 做“预约有效性”校验（fail-closed），预算耗尽或排队失败时 `block`（`code='round-limit'|'queue-failed'|'prompt-rejected'`）。
- **Rust 建议**：`crates/dsh-goal-round-driver`，依赖 goal。难点：与 agent inbox 生命周期、pre-step 瀑布、race fence 的精确时序对齐（大量 `state.attempt` 阶段 `queued/claimed/admitted` + `stale/cancelled` 标记）。

### 1.3 `@deepseek-ai/dsh-tool-goal`（packages/goal/tool-goal）
- **用途**：模型工具 `get_goal` / `create_goal` / `update_goal`，带执行期权限检查。
- **依赖**：dsh-agent、dsh-goal、dsh-invariants、dsh-llm、dsh-session、dsh-system-prompt、dsh-tools、cordis；schemastery。
- **src 模块**：`index.ts`、`authority.ts`、`wrapup.ts`、`invariant.ts`。
- **导出面**（工具注册名 + schema 要点）：
  - `get_goal`：无参，返回 `{goal: {...id,revision,objective,phase,roundsStarted,maxGoalRounds,blockedReason?} | null, activation}`。
  - `create_goal`：`{objective: string, max_goal_rounds?: number}`，仅允许直接人类请求（`requireDirectHuman`）。
  - `update_goal`：`{goal_id, revision, action: edit|pause|resume|complete|blocked, objective?, max_goal_rounds?, blocked_reason?}`；`edit/pause/resume` 需直接人类，`blocked` 需满足 `blockedAfterConsecutiveRounds`（默认 3）最小轮数；`complete/blocked` 在 goal 轮内把 wrapup 上下文 `deferContext`。
  - prompt 段 `tool:goal`（order 114）。
- **Rust 建议**：`crates/dsh-tool-goal`，依赖 tool-goal。难点：authority 判定（直接人类 vs 子代理 vs goal 轮）与 `blocked` 阈值策略。

### 1.4 `@deepseek-ai/dsh-command-goal`（packages/goal/command-goal）
- **用途**：人类 `/goal` 斜杠命令（show/create/edit/pause/resume/clear）。
- **依赖**：dsh-commands、dsh-goal、dsh-invariants、cordis。
- **src 模块**：`index.ts`、`invariant.ts`。
- **导出面**：注册命令 `goal`（`ctx.commands.register`，语法 `<objective>|clear|edit <objective>|pause|resume`）。
- **Rust 建议**：`crates/dsh-command-goal`，薄适配层，依赖 commands + goal。

---

## 2. plan 域（1 包）

### 2.1 `@deepseek-ai/dsh-plan-mode`（packages/plan/plan-mode）
- **用途**：按 agent 记录的 plan 模式（事件溯源，last-wins），带部署指引段、`/plan` 命令、`exit_plan_mode` 退出工具（用户审核后退出）。
- **依赖**：dsh-agent、dsh-commands(可选)、dsh-invariants、dsh-llm、dsh-session、dsh-session-projection、dsh-system-prompt、dsh-tools、dsh-user-questions、cordis；zod。
- **src 模块**：`index.ts`、`types.ts`、`invariant.ts`、`client.ts`。
- **导出面**：服务 `PlanModeController`（`ctx.planMode`，`get/set`）；会话事件 `plan/mode {active}`（log-only、last-wins）；投影单元 `plan {active, pending}`；命令 `plan`；工具 `exit_plan_mode`（`{plan: string}` → `{approved: true}`，经 `userQuestions.ask` 审核）。`foldPlanMode` 纯 fold。
- **Rust 建议**：`crates/dsh-plan-mode`。难点不大，主要是 pre-step 边界处“pending 选择 → 下一个 accepted 步骤提交 `plan/mode`”的时序；`exit_plan_mode` 依赖 user-questions 审核通道。

---

## 3. todo 域（1 包）

### 3.1 `@deepseek-ai/dsh-tool-todo`（packages/todo/tool-todo）
- **用途**：模型工具 `todo_write`，基于事件溯源会话日志的待办清单投影（状态机：planned/in_progress/completed 等，整体 REPLACE 式更新）。
- **依赖**：dsh-agent、dsh-invariants、dsh-session、dsh-session-projection、dsh-tools、cordis；schemastery、zod。
- **src 模块**：`index.ts`、`types.ts`、`invariant.ts`、`client.ts`。
- **导出面**：工具 `todo_write`（`{todos: [{content, status}]}`，整表替换）；投影单元 `todo`；会话事件（todo 变更）。
- **Rust 建议**：`crates/dsh-tool-todo`，模式与 goal/plan 一致（投影 + 工具），可作为“投影单元模式”的第三个参考实现。

---

## 4. skill 域（4 包）

### 4.1 `@deepseek-ai/dsh-skill`（packages/skill/skill）
- **用途**：skill 提供者注册表 `ctx.skills`（分层：global + scope chain，rank 去重，`list/get/snapshot`，注册 `registerProvider` / `register` 运行时 skill）。
- **依赖**：dsh-invariants、dsh-llm、dsh-scope、cordis；schemastery。
- **src 模块**：`index.ts`、`invariant.ts`。
- **导出面**：服务 `SkillRegistry`（`ctx.skills`）；事件 `skills/change`；类型 `SkillProvider`、`SkillCandidate`、`SkillDefinition`、`SkillSummary`、`SkillInvocationPolicy`、`SkillSource`、`SkillResourceBase`；函数 `renderSkillContent`、`isSkillName` 等；`BUNDLED_SKILL_RANK=600`。
- **Rust 建议**：`crates/dsh-skill`。难点：分层合并/遮蔽规则（nearest layer wins，rank 仅层内决胜）与 catalog 缓存失效（revision 计数 + `collectCacheMaxEntries`）。

### 4.2 `@deepseek-ai/dsh-skill-filesystem`（packages/skill/skill-filesystem）
- **用途**：本地文件系统 skill 提供者（默认 provider 名 `filesystem`）：发现目录 bundle（`SKILL.md`）与平铺 `.md`，解析 YAML frontmatter（name/description/invocation/metadata），chokidar 监听根目录变更。
- **依赖**：dsh-fs、dsh-home-paths、dsh-invariants、dsh-skill、cordis；chokidar、yaml、schemastery。
- **src 模块**：`index.ts`、`invariant.ts`。
- **导出面**：`FileSystemSkillProvider`；root 顺序（project-dsh 100 / project-agents 200 / custom 300 / user-dsh 400 / user-agents 500 / bundled 600）；watcher 稳定阈值/轮询/最大项目数配置。
- **Rust 建议**：`crates/dsh-skill-filesystem`。难点：文件监听（Rust `notify` 替代 chokidar）与“缺失根目录/祖先监听”降级逻辑；YAML frontmatter 解析。

### 4.3 `@deepseek-ai/dsh-skill-badge`（packages/skill/skill-badge）
- **用途**：内置 `dsh-badge` skill 提供者（assets/dsh-badge.md）。
- **依赖**：dsh-invariants、dsh-skill、cordis。
- **src 模块**：`index.ts`、`invariant.ts`。
- **导出面**：provider 名 `dsh-badge`，单个候选，source=`bundled`，rank=`BUNDLED_SKILL_RANK`。
- **Rust 建议**：`crates/dsh-skill-badge`，静态资源 + 单候选 provider，最简。

### 4.4 `@deepseek-ai/dsh-tool-skill`（packages/skill/tool-skill）
- **用途**：模型工具 `skill`（按名加载完整指令）+ 会话级 `<available_skills>` 目录注入 + `/name` 用户手势注入。
- **依赖**：dsh-agent、dsh-invariants、dsh-llm、dsh-skill、dsh-tools、cordis；schemastery。
- **src 模块**：`index.ts`、`invariant.ts`。
- **导出面**：工具 `skill`（`{name}` → `{name, provider, resourceBase?, content}`，`<skill_content>` 渲染）；`agent/pre-step` 注入目录（`source.kind='skill-catalog'`，SHA256 digest 决定重发）；`/name` 手势（`skill-invocation` 来源）仅 `source.kind==='user'` 触发。
- **Rust 建议**：`crates/dsh-tool-skill`。难点：目录 digest 判定与 pre-step 注入顺序（指令注入排在目录之后、靠近模型答案）。

---

## 5. subagent 域（11 包）— 注册表 + 多后端

### 5.1 `@deepseek-ai/dsh-subagent`（packages/subagent/subagent）— 抽象 seam
- **用途**：命名 provider 注册表服务 `ctx.subagents`（多 provider 共存，类似 LLM adapter 注册表而非单执行器）。
- **依赖（peer）**：dsh-agent、dsh-agent-presets(可选)、dsh-brand、dsh-invariants、dsh-llm、dsh-sandbox(可选)、dsh-sandbox-policy(可选)、dsh-scope、dsh-session、dsh-session-persistence(可选)、dsh-session-projection(可选)、dsh-session-projection-cache(可选)、dsh-jobs(可选)、dsh-tools、dsh-user-approval(可选)、cordis；zod。
- **src 模块**：`index.ts`、`types.ts`、`error.ts`、`depth.ts`、`lifecycle.ts`、`continuation.ts`、`activation-setup-registry.ts`、`list-children.ts`、`descriptor.ts`、`descriptor-seed.ts`、`assistant-output.ts`、`out-of-process.ts`、`projection.ts`、`projection-types.ts`、`run-settlement.ts`、`child-agent.ts`、`invariant.ts`、`client.ts`。
- **导出面**：
  - 服务 `SubagentRuntime`（`ctx.subagents`）：`registerProvider / getProvider / list / start / startContinuable / followup / interrupt / reportFrom / registerContinuableSetup / drainContinuableDescendants / listChildren / listDescendants`。
  - 事件：`subagent/provider-added|removed`、`subagent/start`、`subagent/end`（作用域 dispatch，`subagent/start` 与 `end` 成对）。
  - 投影：`subagent` identity/timing 两个投影单元。
  - 类型：`SubagentProvider`（`name/capabilities{outputSchema,depthLimit,toolFilter,persona}/inheritsParentContext/start/prepareContinuable?`）、`SubagentRun`、`SubagentResult`、`SubagentStopReason`（`completed|aborted|error|max-tokens|refusal`，merge-extensible）、`SubagentDescriptorData`（versioned）。
- **Rust 建议**：`crates/dsh-subagent`（seam + registry），依赖 agent/session/jobs/scope。难点：能力校验（fail-loud）、continuable 子代理管理器（持久化冷恢复、Activation 生命周期）、descriptor 投影 fold 与 listChildren 的三级降级（live snapshot → 投影缓存 → 持久化检查）、委托深度 `maxDepth`。

### 5.2 后端（provider 注册名见下）
| 包 | 默认 provider 名 | 机制 | src 模块 |
|---|---|---|---|
| `subagent-spawn-in-process` | `spawn` | 进程内新建 fresh 子 agent（`ctx.agents`） | `index.ts`,`invariant.ts` |
| `subagent-fork-in-process` | `fork` | 进程内 fork，子 agent 以父日志已完成轮前缀为 seed（继承对话） | `index.ts`,`invariant.ts` |
| `subagent-in-process-driver` | （非 provider，共享驱动） | spawn/fork 共用：在 `ctx.agents` 上驱动子 agent；`structured.ts` 处理 outputSchema 结构化结果 | `index.ts`,`structured.ts`,`invariant.ts` |
| `subagent-acp` | `acp` | 出进程：spawn 子进程，经 ACP 协议驱动（依赖 `@agentclientprotocol/sdk`） | `index.ts`,`run.ts`,`invariant.ts` |
| `subagent-dsh-sdk` | `dsh-sdk` | 出进程：spawn DSH 运行时子进程，经 stdio JSON-RPC + TS SDK client（`dsh-sdk-client`） | `index.ts`,`run.ts`,`invariant.ts` |
| `subagent-claude-code` | `claude-code`（固定） | one-shot，经官方 Claude Agent SDK（`@anthropic-ai/claude-agent-sdk`） | `index.ts`,`process.ts`,`run.ts`,`invariant.ts` |
| `subagent-codex` | `codex`（固定） | one-shot，经官方 codex app-server stdio 协议（`@openai/codex`） | `index.ts`,`run.ts`,`wire.ts`,`invariant.ts` |

- **特别说明**：`fork`/`spawn` 是进程内后端（`localAgent` 存在），`acp`/`dsh-sdk`/`claude-code`/`codex` 是出进程/第三方后端（`localAgent` 不存在，结果经 subprocess 协议回传）。`fork` 语义：子 agent 继承父已完成轮（`inheritsParentContext: true`），`spawn` 不继承。
- **Rust 建议**：进程内后端 `crates/dsh-subagent-spawn-in-process` / `-fork-in-process` / `-in-process-driver`（依赖 agent）；出进程后端各自一个 crate（`-acp` 依赖 ACP client，`-dsh-sdk` 依赖 SDK client，`-claude-code`/`-codex` 依赖第三方协议适配）。出进程后端移植成本最高，建议最后做或降级为进程内实现。

### 5.3 工具（3 包）
- `@deepseek-ai/dsh-tool-subagent`（packages/subagent/tool-subagent）：工具 `subagent`（可配 `toolName`），`{description, prompt, run_in_background?}`；三态输出 `foreground{runId,output} | background{jobId} | continuable{subagentId}`。背景策略 `backgroundMode: one-shot|continuable`；`maxDepth`（默认 3）、`toolFilter`、`persona`、`agentOptions`。prompt 段 `tool:subagent`（order 116.5）。
- `@deepseek-ai/dsh-tool-subagent-control`（packages/subagent/tool-subagent-control）：工具 `send_message`（`{subagent_id, message}` → `{messageId}`）、`interrupt_agent`（`{agent_id}` → `{accepted}`）、`list_agents`（`list-agents.ts`，枚举 children/descendants）。
- `@deepseek-ai/dsh-tool-subagent-report`（packages/subagent/tool-subagent-report）：子作用域工具 `report`（`{output}` → `{messageId}`），经 `registerContinuableSetup` 注入每个 continuable 子代理；`reportDelivery: wakeup|quiet`。
- **Rust 建议**：`crates/dsh-tool-subagent` / `-control` / `-report`，薄工具层，依赖 subagent seam。

---

## 6. workflow 域（4 包）— 脚本执行器

### 6.1 `@deepseek-ai/dsh-workflow`（packages/workflow/workflow）— 抽象 seam
- **用途**：workflow 能力 seam `ctx.workflowEngine`，抽象 `start(request)` 返回 `WorkflowRun`；定义 run 词汇与 `workflow/*` 事件。
- **依赖**：dsh-agent、dsh-brand、dsh-invariants、dsh-llm、dsh-session、cordis。
- **src 模块**：`index.ts`、`types.ts`、`runtime-types.ts`、`invariant.ts`。
- **导出面**：抽象服务 `WorkflowEngine`（`ctx.workflowEngine`，`start()`，`emitWorkflowEvent`）；事件 `workflow/start|phase|log|agent-start|agent-end|end`（观察型，无执行语义）；`WorkflowError`（`fatal` 标志 + `WorkflowErrorCode`：`SCRIPT_PARSE|META_INVALID|INVALID_ARGUMENT|UNSUPPORTED_OPTION|UNSUPPORTED_SCHEMA|AGENT_CAP|ITEM_CAP|AGENT_START|AGENT_RESULT|RESULT_UNSERIALIZABLE|CANCELLED`）；`WorkflowMeta{name,description,whenToUse?,phases?}`、`WorkflowStopReason{completed|cancelled|error}`。
- **Rust 建议**：`crates/dsh-workflow`（纯类型 + 抽象 seam + 事件）。

### 6.2 `@deepseek-ai/dsh-workflow-worker-thread`（packages/workflow/workflow-worker-thread）— 脚本执行器（特别说明）
- **用途**：worker-thread 引擎实现：在独立线程用 `node:vm` 沙箱执行模型编写的编排脚本，`agent()` 调用经消息协议桥回宿主 `ctx.subagents`。
- **依赖**：dsh-agent、dsh-brand、dsh-invariants、dsh-llm、dsh-session、dsh-subagent、dsh-tools、dsh-workflow、cordis；schemastery。
- **src 模块**：`index.ts`、`host.ts`（WorkerRun：Worker 生命周期/取消/宽限终止/子代理注册表）、`worker.ts`、`runtime.ts`（vm hook：`agent/parallel/pipeline/phase/log/args`，并发与总量上限、取消、结果物化）、`realm.ts`（realm 信任模型/`materializeFromRealm`）、`protocol.ts`（Host↔Worker JSON 消息）、`session.ts`、`meta.ts`、`types.ts`、`invariant.ts`。
- **脚本 hook 契约**：`agent(prompt,opts)`（`opts.label/phase/schema/provider/model`，`outputSchema` 限 JSON-Schema 子集 `type/properties/required/additionalProperties/items/enum/const/oneOf`）、`parallel(thunks)`（并发、thunk 抛错→null，fatal 例外）、`pipeline(items,...stages)`（无阶段屏障、stage 抛错→该项 null）、`phase(title)`、`log(msg)`、全局 `args`。无 fs/网络/timer/Node API。
- **关键难点**：进程内 worker 与脚本 realm 之间的“失配错误不能被脚本伪造”的 fatality 判定（`instanceof` 跨 realm 不成立，故宿主侧判 `isFatalWorkflowError`）；worker 死亡/取消宽限（`disposeGraceMs`）下“每次 `agent-start` 恰好配一次 `agent-end`”的合成账本；结构化克隆边界。
- **Rust 建议**：`crates/dsh-workflow-worker-thread` 需要重新设计 —— Rust 没有 `node:vm`，脚本执行器要用嵌入式 JS 运行时（`deno_core`/`boa_engine`）或换成 Rust 原生编排 DSL（若“1:1”要求 JS 语义则依赖 `deno_core`）。这是本组移植难度最高的一项。

### 6.3 `@deepseek-ai/dsh-tool-workflow`（packages/workflow/tool-workflow）
- **用途**：模型工具 `workflow`（可配名），执行 `{script, meta, args?}`。
- **依赖**：dsh-agent、dsh-invariants、dsh-llm、dsh-session、dsh-system-prompt、dsh-tools、dsh-workflow、cordis；schemastery。
- **src 模块**：`index.ts`、`types.ts`、`invariant.ts`。
- **导出面**：工具 `workflow`；输出 `{runId, agentsStarted, result}`（`result` 为脚本 JSON 返回值，`maxResultChars` 截断）；把 `workflow/*` 事件投影为会话记录事件 `tool-workflow/run-start|agent-start|agent-end|run-end`；prompt 段 `tool:workflow`（order 115）。
- **Rust 建议**：`crates/dsh-tool-workflow`，依赖 workflow seam + worker-thread 引擎。

### 6.4 `@deepseek-ai/dsh-tool-ralph`（packages/workflow/tool-ralph）
- **用途**：模型工具 `ralph` —— 前台新鲜 agent 循环（Ralph loop）：固定脚本每轮起一个 fresh 结构化子代理，仅传不可变 objective + 上一轮有界结构化 handoff。
- **依赖**：dsh-agent、dsh-invariants、dsh-llm、dsh-subagent、dsh-system-prompt、dsh-tools、dsh-workflow、cordis；schemastery。
- **src 模块**：`index.ts`、`invariant.ts`。
- **导出面**：工具 `ralph`（`{objective, maxRounds?}`）；固定 `RALPH_SCRIPT`（`reportSchema` 五字段 `status/summary/evidence/nextSteps/blocker`，`status ∈ continue|complete|blocked`，`maxHandoffChars` 上限）；默认 `subagentProvider='spawn'`（要求 fresh、不支持继承父上下文、支持 outputSchema）；`maxRounds`（默认 256）为部署上限。
- **Rust 建议**：`crates/dsh-tool-ralph`，依赖 tool-workflow + subagent，可作为固定编排脚本的应用样例。

---

## 7. mcp 域（1 包）— MCP 客户端

### 7.1 `@deepseek-ai/dsh-mcp-client`（packages/mcp/mcp-client）
- **用途**：**MCP 客户端**桥：连接外部 MCP 服务器（`stdio` 子进程 或 `streamable-http`），把服务器工具注册到 `ctx.tools`，命名 `mcp__<serverName>__<rawName>`。
- **依赖**：dsh-invariants、dsh-llm、dsh-subprocess、dsh-timeout、dsh-tools、cordis；`@modelcontextprotocol/sdk`、schemastery、zod。
- **src 模块**：`index.ts`、`connection.ts`、`tools.ts`、`transport.ts`、`invariant.ts`。
- **导出面**：插件 `apply`（每个实例连一个服务器；`serverName` 命名空间保留防冲突）；配置 `Config = StdioConfig | StreamableHttpConfig`（含 `reconnect` 策略）；`McpResult`。
- **特别说明**：是**客户端**（消费外部 MCP server 的能力），不是服务端。
- **Rust 建议**：`crates/dsh-mcp-client`。Rust 侧用 `rmcp` 或自研 MCP 协议实现替代 `@modelcontextprotocol/sdk`；难点在 stdio/HTTP transport 抽象与自动重连、工具 schema 转 `dsh-tools` 的 `ObjectJsonSchema`。

---

## 8. lsp 域（3 包）— LSP 客户端

### 8.1 `@deepseek-ai/dsh-lsp`（packages/lsp/lsp）— 抽象 seam
- **用途**：LSP 能力 seam `ctx.lsp`：provider 注册表（branded id + 扩展名映射，原子冲突检查），仅暴露 4 个规范化操作 `goToDefinition/findReferences/goToImplementation/hover`，无 JSON-RPC 逃生口。
- **依赖**：dsh-brand、dsh-invariants、dsh-llm、cordis。
- **src 模块**：`index.ts`、`types.ts`、`brand.ts`、`invariant.ts`。
- **导出面**：服务 `Lsp`（`ctx.lsp`，`registerProvider/query`）；`LspError`（`LSP_INVALID_PROVIDER|LSP_CONFLICT|LSP_UNAVAILABLE|LSP_DISPOSED|LSP_UNSUPPORTED_OPERATION|LSP_MALFORMED_RESPONSE`）；`finalExtension()` 规范化；类型 `LspProvider/LspQueryRequest/LspQueryResult/LspLocation/LspPosition/LspRange/LspHover`。
- **Rust 建议**：`crates/dsh-lsp`（seam + 类型）。

### 8.2 `@deepseek-ai/dsh-lsp-stdio`（packages/lsp/lsp-stdio）
- **用途**：通用 stdio **客户端** provider：按配置 spawn 语言服务器（如 typescript-language-server），JSON-RPC 帧化/翻译，transient-open 查询，每 canonical workspace 单飞一个进程，失败替换重试。
- **依赖**：dsh-brand、dsh-fs、dsh-invariants、dsh-llm、dsh-lsp、dsh-subprocess、dsh-timeout、cordis；schemastery。
- **src 模块**：`index.ts`、`connection.ts`、`framing.ts`、`host.ts`、`instance.ts`、`protocol.ts`、`translate.ts`、`abort.ts`、`invariant.ts`。
- **导出面**：`LocalLspProvider`（每 `servers.<id>` 一个）；导出 `LspInstance`、`LspConnection`、`MessageDecoder`、`normalizeLocations/Hover`、`negotiatePositionEncoding` 等；配置含 `maxMessageBytes/maxStderrBytes/maxDocumentBytes/shutdownTimeoutMs/killGraceMs`。
- **特别说明**：是**客户端**（spawn 并驱动外部语言服务器），通过 `ctx.fs` 读源码、`ctx.subprocess` spawn。
- **Rust 建议**：`crates/dsh-lsp-stdio`。Rust 用 `lsp-server`/`tower-lsp` 客户端侧或自研 JSON-RPC 帧化（Content-Length 头）替代；难点：transient-open 生命周期、位置编码协商（UTF-16 vs UTF-8）、进程池与替换重试。

### 8.3 `@deepseek-ai/dsh-tool-lsp`（packages/lsp/tool-lsp）
- **用途**：模型工具 `lsp`（只读，4 操作，one-based UTF-16 坐标 → 零基）。
- **依赖**：dsh-invariants、dsh-llm、dsh-lsp、dsh-system-prompt、dsh-timeout、dsh-tools、cordis；schemastery。
- **src 模块**：`index.ts`、`render.ts`、`session-cwd.ts`、`invariant.ts`。
- **导出面**：工具 `lsp`（`{operation, file_path, line, character}` → `locations{locations[],resolvedWorkspaceUri} | hover{contents,range}`）；`maxLocations`（默认 100）、`maxResultChars`（默认 16000）、`timeoutMs`（默认 60000）；prompt 段 `tool:lsp`（order 112）。
- **Rust 建议**：`crates/dsh-tool-lsp`，依赖 lsp seam。

---

## 9. acp 域（1 包）— ACP 服务端

### 9.1 `@deepseek-ai/dsh-acp`（packages/acp/acp）
- **用途**：**Agent Client Protocol 服务端**（automation-only），经 JSON-RPC stdio 暴露 harness 会话给程序化客户端（`AgentSideConnection`）。
- **依赖**：dsh-agent、dsh-invariants、dsh-session、dsh-user-approval、cordis；`@agentclientprotocol/sdk`、schemastery。
- **src 模块**：`index.ts`、`codec.ts`、`invariant.ts`。
- **导出面**：插件 `apply`；方法 `initialize/authenticate/newSession/prompt/cancel`；仅转发已提交 assistant 文本（`agent_message_chunk`），无 presentation；`approval/request` 一次性 allow-once/reject-once；`AcpConfig{provider?,model?,stream?}`。
- **特别说明**：是**服务端**（对外提供 ACP 协议，供 `subagent-acp` 等客户端驱动）；生产走 stdio，测试注入 `stream`。
- **Rust 建议**：`crates/dsh-acp`。Rust 侧用 `rmcp` 的 agent 端或自研 ACP/JSON-RPC 协议；难点：prompt 结算要等“整 agent idle”（`whenIdle`）而非单 turn 结束；`max-tokens` 映射为 `end_turn`。

---

## 10. extensions 域（4 包）— 动态 Cordis 插件

### 10.1 `@deepseek-ai/dsh-cordis-host-runner`（packages/extensions/cordis-host-runner）
- **用途**：动态插件定义注册表 + 宿主半边沙箱生命周期 + invoke 处理器表（模型挂载双半边插件）。
- **依赖**：cordis、dsh-agent、dsh-brand、dsh-invariants、dsh-llm、dsh-scope、dsh-session、dsh-tools、dsh-typert-protocol；schemastery、zod。
- **src 模块**：`index.ts`、`registry.ts`、`lifecycle.ts`、`guard.ts`、`sandbox.ts`、`inspect-registry.ts`、`types.ts`、`invariant.ts`。
- **导出面**：服务 `DynamicCordisRunnerService`（`ctx.dynamicCordisRunner`，`define/undefine/run/stop/reference/listPlugins/inspectPlugin/inspectPackage/snapshot/inventory`，`@Remote`：`runHostHalf/getClientCode/resolveRequestRun/invoke/syncInspectManifest/...`）+ `CordisInspectRegistryService`（`ctx.cordisInspect`）；事件 `cordis/request-run`、`cordis/request-run-resolved`、`cordis/dynamic-package`、`cordis/dynamic-retract`；ID 品牌 `CordisDynamicPluginId/PackageId/PluginRunId/ApprovalRequestId`；VM 求值（`evaluateHostCode`，`vmTimeoutMs`）。
- **Rust 建议**：`crates/dsh-cordis-host-runner`。与 workflow 一样，宿主半边要 `node:vm` 求值 JS —— Rust 需 `deno_core`/`boa_engine` 才能保持“1:1 JS 插件”语义；否则需定义 Rust 原生插件 ABI。版本指针（currentPackageId/nextPackageId）、审批状态机、远程 RPC 是核心难点。

### 10.2 `@deepseek-ai/dsh-cordis-client-runner`（packages/extensions/cordis-client-runner）
- **用途**：浏览器半边动态插件（事件订阅、闭包求值、guard 门面、loader 入口）。
- **依赖（peer）**：cordis-plugin-loader、dsh-api-remotes、dsh-client-connection、dsh-client-modules、dsh-client-runtime、dsh-client-ui-slots、dsh-client-ui-theme、dsh-invariants、cordis、react；`dsh.client` 声明。
- **src 模块**：`index.ts`、`invariant.ts`、`client/{api-catalog,evaluator,guard,inspect-registry,orchestrator,providers,runtime,slot-catalog,timer,index}.ts`。
- **Rust 建议**：浏览器半边在纯 Rust 后端移植中通常**不移植**（对应 Web 前端）；在清单中标记为“客户端/UI，不属后端 1:1 范围”。

### 10.3 `@deepseek-ai/dsh-tool-cordis`（packages/extensions/tool-cordis）
- **用途**：自引用 cordis 工具集：`cordis_inspect_list / cordis_inspect_query / cordis_inspect_self`（只读检查）+ `cordis_define / cordis_run / cordis_stop / cordis_undefine`（挂载/处置模型编写的插件）+ `@pluginId` 上下文注入。
- **依赖**：dsh-agent、dsh-cordis-host-runner、dsh-invariants、dsh-llm、dsh-scope、dsh-session、dsh-system-prompt、dsh-tools、cordis。
- **src 模块**：`index.ts`、`api-catalog.ts`、`fiber-state.ts`、`inspect.ts`、`present.ts`、`prompt.ts`、`providers.ts`、`invariant.ts`。
- **导出面**：7 个工具（`cordis_inspect_list/query/self`、`cordis_define/run/stop/undefine`）；`cordis_define` 参数含 `plugin{kind:new|existing, idPrefix?|pluginId}`、`code{host?,client?}`；`cordis_run` 返回 `awaiting-approval|starting|running`；`@pluginId` 手势经 `agent/pre-step` 注入 `<cordis_dynamic_plugin_context>`。
- **Rust 建议**：`crates/dsh-tool-cordis`，依赖 cordis-host-runner。

### 10.4 `@deepseek-ai/dsh-client-ui-cordis`（packages/extensions/ui-cordis）
- **用途**：浏览器 UI —— cordis_define 工具行的 keyed 卡片 + run/stop 开关。
- **依赖（peer）**：dsh-api-remotes、dsh-client-connection、dsh-cordis-client-runner、dsh-client-locale、dsh-client-runtime、dsh-client-ui-*、dsh-invariants、cordis、react。
- **src 模块**：`index.ts`、`invariant.ts`、`client/{card-model,dynamic-port,events,inventory,locales,run-card-index,slots,status,index}.ts`。
- **Rust 建议**：纯客户端 UI，不属后端 1:1 范围。

---

## 11. runtime-diagnostics 域（1 包）

### 11.1 `@deepseek-ai/dsh-invariants`（packages/runtime-diagnostics/invariants）
- **用途**：包级运行时不变量注册表 `ctx.invariants`（每个 workspace 包从 `./invariant` 伴生模块注册检查；普通入口与诊断解耦）。
- **依赖**：cordis；schemastery。
- **src 模块**：`index.ts`、`invariant.ts`。
- **导出面**：服务 `InvariantRegistry`（`ctx.invariants`，`register(packageName, installer)`）；`InvariantError`（`code='INVARIANT'`，`packageName`）；配置 `enabled / package_allowlist / package_blocklist`（正则过滤）。
- **Rust 建议**：`crates/dsh-invariants`，是**底座依赖**（几乎所有包都 peer 依赖它），应最先移植。

---

## 12. examples 域（3 包）

| 包 | 用途 | src 模块 | 备注 |
|---|---|---|---|
| `@deepseek-ai/dsh-acp-demo`（packages/examples/acp-demo） | ACP 自动化服务端应用：agent spine + JSONL 持久化 + ACP transport，bin `dsh-acp-demo` | `bin.ts`,`index.ts`,`invariant.ts` | 组合类样例，验证 ACP + 持久化栈 |
| `@deepseek-ai/dsh-agent-spine-demo`（packages/examples/agent-spine-demo） | 默认无 executor/UI 的 agent spine：fallback 会话标题、provider 路由重试、可选持久化 goal | `index.ts`,`invariant.ts` | 是 `python/sdk-runtime` 的默认运行时组合，依赖 goal/skill/tool-* |
| `@deepseek-ai/dsh-sdk-jsonrpc-demo`（packages/examples/jsonrpc-demo） | 启动外部 Cordis 配置的 stdio JSON-RPC SDK 运行时，bin `dsh-jsonrpc-agent` | `bin.ts`,`index.ts`,`packaged-bin.ts`,`runner.ts`,`invariant.ts` | 验证 SDK 运行时 |

- **Rust 建议**：这三个是“应用组合”而非库，移植时对应集成测试/示例二进制，不产出公共 crate。

---

## 13. test-support 域（6 包）— 测试工具（不随运行时发布）

| 包 | 用途 | src 模块 |
|---|---|---|
| `@deepseek-ai/dsh-acp-snapshot` | ACP 测试套件：子进程启动器、快照场景 harness、输出规范化器、suite 工厂 | `harness.ts`,`launcher.ts`,`normalize.ts`,`suite.ts`,`index.ts`,`invariant.ts` |
| `@deepseek-ai/dsh-agent-loop-testkit` | 具体 agent loop 测试的前置挂载 | `index.ts`,`invariant.ts` |
| `@deepseek-ai/dsh-client-test-runtime` | jsdom slot 测试运行时（真实 Cordis Context + SlotRegistry + web-react 渲染器 + 测试双实例） | `fixtures.ts`,`locale-env.ts`,`remote.ts`,`sessions.ts`,`settings-scope.ts`,`snapshot.ts`,`translate.ts`,`workspaces.ts`,`index.ts`,`invariant.ts` |
| `@deepseek-ai/dsh-llm-mock-server` | 可脚本化 OpenAI 兼容 HTTP/SSE 故障服务器（LLM 恢复测试） | `bin.ts`,`cli.ts`,`index.ts`,`invariant.ts` |
| `@deepseek-ai/dsh-llm-replay` | 回放 LLM 插件：从录制会话 JSONL 重建 chunk，短路 `llm/stream`（无 key 快照测试） | `index.ts`,`invariant.ts` |
| `@deepseek-ai/dsh-loader-smoke` | 子进程 + 直连 agent harness，无 key 真实 Loader 冒烟测试 | `agent-turn.ts`,`index.ts`,`invariant.ts` |

- **Rust 建议**：测试工具 → `crates/dsh-test-support/*`（dev-dependency 范围）；`llm-mock-server` 可用 `axum` + SSE 实现；`client-runtime` 依赖 jsdom/React，属前端测试，后端移植中可略。

---

## 14. 顶层 `python/` 目录（特别说明）

- **用途**：**Python SDK**，用于把 DeepSeek Harness 作为子进程驱动。客户端 SDK 经 **stdio 上的 newline-delimited JSON-RPC** 与内置运行时通信（对应 `dsh-sdk-*` 的 stdio JSON-RPC 协议）。
- **结构**：
  - `sdk/`（`deepseek-harness-sdk` / `deepseek_harness`）：高层 turns API + 低层 JSON-RPC client。模块 `api.py`、`client.py`、`errors.py`、`models.py`（pydantic 模型）、`__init__.py`。
  - `sdk-runtime/`（`deepseek-harness-runtime-bin` / `deepseek_harness_runtime`）：内置运行时二进制与默认 agent 配置。含 `package.json`（`dsh-jsonrpc-agent-pkg` 部署根，`pnpm deploy` 物化 node_modules 闭包）、`platforms.json`、`hatch_build.py`、`runtime/cordis.yml`（默认组合，几乎列出全部 `dsh-*` 包）。
- **依赖关系**：`sdk` 依赖 `deepseek-harness-runtime-bin==0.0.0.dev0`（经 `tool.uv.sources` editable 指向 `../sdk-runtime`）；`sdk-runtime` 是“依赖即部署根”，把 TS 运行时打成可分发 Python 包。
- **Rust 建议**：若 Rust 后端需 Python 客户端，需保留/重写 `sdk`（用 `pydantic` 或 `msgspec` + 自研 JSON-RPC client）；`sdk-runtime` 的 TS 运行时闭包在 Rust 移植后被替换为 Rust 二进制产物。协议契约（stdio NDJSON JSON-RPC）是稳定边界，必须在移植中保持不变。

---

## 15. 顶层 `native/landlock-run/` 目录（特别说明）

- **语言**：**纯 C11**（`packages/entry/src/main.c`，约 300 行），直接使用 Landlock UAPI（`landlock_create_ruleset/add_rule/restrict_self` syscall 444/445/446），除 libc（musl 静态链接）外无依赖；另含 TypeScript 入口包（`packages/entry/src/index.ts`）与 `scripts/*.mjs|*.ts` 构建/发布脚本。
- **做什么**：**Linux Landlock 沙箱启动器**（self-restrict-then-exec）。在自身安装 Landlock 规则集后 `execvp` 包装命令，规则集跨 `execve` 继承，使命令及其子进程受限，而调用进程不受限。用于 `bwrap` 不可用（未安装/非特权 user namespace 被禁/LSM 禁止 mount）的 Linux 主机。**是 Linux-only 沙箱的底层 rung**。
- **CLI 契约**：`landlock-run [--ro <path>]... [--rw <path>]... -- <argv>...`、`landlock-run --probe`；`--ro` 授予路径下 read+execute，`--rw` 授予完整文件系统访问，其余全拒（allow-list）；`--probe` 构建最大规则集并报告内核是否真正执行（`full`/`partial`）。fail-closed：规则集无法创建或内核不执行则**不 exec**、退出码 `125`。
- **打包**：按平台 npm 包 `@deepseek-ai/node-addon-landlock-run-linux-{x64,arm64}`（binary carrier），入口包 `entry` 提供 JS API `launcherPath()/probe()/grantArgs()`，probe 返回 `full|partial|unusable`。
- **Rust 建议**：`crates/dsh-landlock-run`（或复用）。C 代码体积小且仅依赖稳定内核 syscall，可直接保留为 build 脚本编译的 C 二进制（`cc` crate 编译 `main.c`），或改写为 Rust（`rustix`/直接 `syscall` + `landlock` crate）实现同 CLI 契约与退出码 `125`。契约 `docs/cli-contract.md` 是稳定边界。

---

## 16. 依赖顺序提示（Rust 移植）

推荐自底向上（组 F 内部，底座组先于本组）：

1. **底座（组外，先落地）**：`dsh-invariants` → `dsh-brand`/`dsh-session`/`dsh-llm`/`dsh-tools`/`dsh-system-prompt`/`dsh-scope` → `dsh-agent`/`dsh-subprocess`/`dsh-timeout`/`dsh-typert-protocol`/`dsh-jobs`/`dsh-session-projection`。
2. **本组抽象 seam（无外部协议依赖）**：`dsh-skill` → `dsh-lsp` → `dsh-workflow` → `dsh-subagent` → `dsh-goal` → `dsh-plan-mode`。
3. **本组提供者/驱动**：`dsh-skill-filesystem`/`-badge`、`dsh-lsp-stdio`、`dsh-subagent-spawn/fork-in-process` + `-in-process-driver`、`dsh-goal-round-driver`。
4. **本组工具层**：`dsh-tool-skill`、`dsh-tool-lsp`、`dsh-tool-subagent`/`-control`/`-report`、`dsh-tool-goal`、`dsh-command-goal`、`dsh-tool-todo`、`dsh-tool-workflow`、`dsh-tool-ralph`、`dsh-tool-cordis`。
5. **外部协议（最后/独立）**：`dsh-mcp-client`、`dsh-acp`（服务端）、`dsh-subagent-acp`/`-dsh-sdk`/`-claude-code`/`-codex`（出进程后端）。
6. **执行沙箱（最高风险，可并行评估）**：`dsh-workflow-worker-thread`（JS 脚本执行）与 `dsh-cordis-host-runner`（JS 插件求值）——若坚持 JS 语义需引入嵌入式 JS 运行时（`deno_core`/`boa_engine`），否则需定义 Rust 原生替代 DSL/插件 ABI。
7. **辅助**：`dsh-landlock-run`（C 二进制，`cc` 编译）、`python/sdk`（Python 客户端，独立）、examples/test-support（集成测试，最后）。

> 约定：本组文档仅覆盖“后端”包（Host 侧 Node 进程）。`packages/extensions/cordis-client-runner`、`ui-cordis`、`packages/test-support/client-runtime` 等浏览器/React 侧包属 Web 前端，不纳入 Rust 后端 1:1 范围，仅在上表登记。
