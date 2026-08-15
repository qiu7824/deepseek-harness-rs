# Group E — Execution / Filesystem / Sandbox — Package Inventory (Rust 1:1 移植清单)

> 只读盘点。源码根 `D:\HermesTemp\deepseek-harness`。命名空间前缀 `@deepseek-ai/dsh-*`，运行时依赖 `@deepseek-ai/cordis`（DI/Service/Events）与 `@deepseek-ai/schemastery`（config schema，zod 风格）。
> 说明：任务里列出的 13 个“包”在仓库里是 13 个 **分组目录**，每个分组含若干真实 npm 包（`packages/<group>/<name>`）。本清单按分组组织，每节列出其中的真实包。

---

## 1. sandbox（进程沙箱 seam + 三平台后端 + 策略 + Windows ACL 后端）

### 1.1 `@deepseek-ai/dsh-sandbox`（抽象 seam，`ctx.sandbox`）
- **用途**：同机进程隔离能力 seam。`SandboxProvider.confine(argv, policy)` 把要执行的 argv 包成“限制后端 + profile + 分隔符 + 原 argv”，调用方 spawn 返回的 argv 代替自己的。失败必须 fail-closed（`SANDBOX_UNAVAILABLE`），严禁静默非限制透传。
- **workspace 依赖**：`dsh-invariants`、`dsh-llm`、`dsh-session`、`cordis`（peer）。
- **src 顶层文件**：`index.ts`、`roots.ts`、`escalation.ts`、`invariant.ts`。
- **导出面**：
  - 类型：`SandboxMode`（`'read-only' | 'workspace-write' | 'danger-full-access'`）、`ConfinedSandboxMode`、`SandboxExecutionPolicy`、`SandboxPolicy`、`SandboxEnforcement`（`'full' | 'partial'`）、`ConfinedArgv`、`RunnerFailureRule`。
  - 服务：`SandboxProvider`（抽象类，注册为 `ctx.sandbox`）。
  - 常量/错误：`SANDBOX_UNAVAILABLE`、`SandboxUnavailableError`（extends `HarnessError`）。
  - 函数：`canonicalPath`、`writableRoots`（workspace-write 的写允许集 = 去重 canonical 后的 `[workspaceRoot, /tmp, os.tmpdir()]`；read-only 返回 `[]`）。
  - escalation 词汇（来自 `escalation.ts`）：`WIDER_MODES`（read-only→[workspace-write,danger-full-access]；workspace-write→[danger-full-access]）、`ESCALATION_TARGETS`、`validateEscalationArgs`、`sandboxDenialMarker`、`escalationHintMarker`、`approveEscalation`、类型 `EscalationApproval/EscalationApprover/EscalationOutcome/EscalationRequest`。
- **Rust 移植**：`crates/dsh-sandbox/`（trait + 类型，仅依赖 `dsh-llm` 错误与 `dsh-session` 的 SessionId）。难点：把“argv 包装 + denialSignatures + runnerFailureRules”表达成 `enum Backend`/`trait SandboxBackend { fn confine(&self, argv, policy) -> ConfinedArgv }`。依赖顺序：几乎是最底层（在 session/llm 之上、在 shell/fs 之下）。

### 1.2 `@deepseek-ai/dsh-sandbox-policy`（`ctx.sandboxPolicy`，会话级模式解析）
- **用途**：部署默认 + 会话级 `sandbox/mode` 事件折叠 → 每次能力调用解析出 `SandboxExecutionPolicy`（mode + workspaceRoot + sessionId）。owner 还把解析结果注入 systemPrompt 上下文。
- **workspace 依赖**：`dsh-agent`、`dsh-invariants`、`dsh-sandbox`、`dsh-session`、`dsh-system-prompt`、`cordis`（peer）；`schemastery`。
- **src 顶层文件**：`index.ts`、`session-mode.ts`、`invariant.ts`。
- **导出面**：服务 `SandboxPolicyService`（`ctx.sandboxPolicy`）；`Config`、`SandboxPolicyRequest`；再导出 `SANDBOX_MODES`、`effectiveSandboxMode`、`setSandboxMode`。
- **关键逻辑**：`session-mode.ts` 定义会话事件 `'sandbox/mode': { mode, source? }`（log-only，可重放）。`effectiveSandboxMode(events)` 从后往前取最后一个 `sandbox/mode`；`resolve()` 优先级：显式 approved mode > 会话 override > 部署默认；workspaceRoot = 会话 header.cwd ?? 配置 fallback(默认 `process.cwd()`)。
- **Rust 移植**：`crates/dsh-sandbox-policy/`。难点：事件折叠是纯函数，好移植；`setSandboxMode` 即 append 一条事件。依赖 `dsh-sandbox` + `dsh-session`。

### 1.3 `@deepseek-ai/dsh-sandbox-local`（本地后端，选 runner 链）
- **用途**：按平台选 runner：Linux `bwrap`→`landlock`；macOS `seatbelt`；Windows `windows-acl`。多候选用功能探针仲裁，单候选直接选；不可用则 fail-closed。Windows 分支还拥有写授权（workspace SID 的 standing ACE + 每个 live session 的随机私有 temp SID）。
- **workspace 依赖**：`dsh-sandbox-windows-acl`、`node-addon-landlock-run`、`schemastery`；peer `dsh-invariants/dsh-llm/dsh-sandbox/dsh-session/cordis`。
- **src 顶层文件**：`index.ts`、`profiles.ts`、`invariant.ts`。
- **导出面**：服务 `LocalSandboxProvider`（注册 `ctx.sandbox`）；`Config`（`runnerCommand?`、`runnerFailureSignatures?`、`probeTimeoutMs?`）；`SandboxInternals`（测试钩子）。
- **关键逻辑**：`PLATFORM_CHAINS = { linux: [bwrap, landlock], darwin: [seatbelt], win32: [windows-acl] }`；`STATIC_ENFORCEMENT` 中 windows-acl = `'partial'`。`profiles.ts` 生成 bwrap mounts（`--ro-bind / / --dev /dev --proc /proc --die-with-parent`，workspace-write 加 `--tmpfs /tmp --bind wsRoot wsRoot`）、landlock grants（readOnly `/`，readWrite `/dev/null` + workspace-write 加 `/tmp`+workspaceRoot）、Seatbelt SBPL（`(deny file-write*) (allow file-write* (literal "/dev/null")) (allow file-write* (subpath ...))`）。Windows grant 生命周期见 1.4。
- **Rust 移植**：`crates/dsh-sandbox-local/`。难点：探针 = 子进程 spawn 探测（可复用 subprocess）；Landlock 经 `node-addon-landlock-run`（Rust 侧改为直接调用 launcher 二进制/或 `landlock` crate）；Seatbelt 用 `sandbox-exec -p`。依赖 `dsh-sandbox` + `dsh-sandbox-windows-acl`。

### 1.4 `@deepseek-ai/dsh-sandbox-windows-acl`（Windows 受限令牌后端）
- **用途**：WRITE_RESTRICTED 受限令牌 + 能力 SID 写 ACL 白名单。镜像 `huoyaoyuan/windows-acl-restrict-poc`，但每个 API 失败都抛错（带 API 名 + Win32 code），绝不无限制 spawn child。
- **workspace 依赖**：`koffi`（FFI）；peer `dsh-invariants/cordis`。
- **src 顶层文件**：`index.ts`、`spawn.ts`、`token.ts`、`acl.ts`、`grant.ts`、`runner.ts`、`ffi.ts`、`win32-abi.ts`、`workspace-sid.ts`、`path-boundary.ts`、`errors.ts`、`invariant.ts`。
- **导出面**：`AclSandbox`（类）、`AclSandboxOptions`、`AclSandboxSpawnOptions`、`AclSandboxChildResult`、`AclSandboxChild`、`AclWriteGrant`、`workspaceWriteSid`、`tempWriteSid`、`assertTempRootOutsideWorkspace`、`Win32Error`、`quoteArg`。
- **Rust 移植**：`crates/dsh-sandbox-windows-acl/`（`#[cfg(windows)]`）。难点见 §14。

---

## 2. subprocess（子进程 seam + 本地实现）

### 2.1 `@deepseek-ai/dsh-subprocess`（抽象 seam，`ctx.subprocess`）
- **用途**：受管进程树 + 有界 spill 输出 + 升级式 kill。`spawn(spec)` 立即返回 live handle；`spawnTerminal(spec)` 分配真实 PTY。
- **workspace 依赖**：`dsh-invariants`、`cordis`（peer）。
- **src 顶层文件**：`index.ts`、`types.ts`、`invariant.ts`。
- **导出面**：
  - 服务：`SubprocessRuntime`（`ctx.subprocess`），抽象方法 `resolveExecutable`/`spawn`/`spawnTerminal`。
  - 常量：`DSH_ENV_PREFIX = 'DSH_'`、`SENSITIVE_ENV_PATTERN = /KEY|PASSWORD|SECRET|TOKEN/i`；函数 `scrubbedParentEnv()`（去掉敏感 + `DSH_*` 的环境基）。
  - 类型（`types.ts`）：`SubprocessSpawnSpec`/`SubprocessHandle`/`SubprocessOutcome`/`SubprocessStdio`/`SubprocessOutputMode`（`'pipe'|'inherit'|SubprocessCollect`）/`SubprocessCollect`/`CollectedOutput`/`SubprocessOutputReader`/`SubprocessOutputRead`/`SubprocessStdinMode`/`SubprocessTerminalSpawnSpec`/`SubprocessTerminalHandle`/`SubprocessTerminalSignal`/`SubprocessTerminalForeground`、`DshEnvironment`/`DshEnvironmentKey`。
- **Rust 移植**：`crates/dsh-subprocess/`。难点：stdio 三态 + offset 游标读取器 + 树级终止抽象。底层（仅 invariants+cordis）。

### 2.2 `@deepseek-ai/dsh-subprocess-local`（本地实现）
- **用途**：detached 进程树；POSIX 发信号给进程组（回退直接 child），Windows `taskkill /T` 杀树；PTY 用 `node-pty`；spill 输出；宿主退出同步强停。
- **workspace 依赖**：`node-pty`；peer `dsh-invariants/dsh-subprocess/dsh-timeout/cordis`。
- **src 顶层文件**：`index.ts`、`spawn.ts`、`terminal.ts`、`process-inspector.ts`、`invariant.ts`。
- **导出面**：服务 `LocalSubprocessRuntime`（`ctx.subprocess`）；`SpawnInternals` 测试钩子。
- **Rust 移植**：`crates/dsh-subprocess-local/`。难点：spawn 管道捕获（POSIX 用 fork+exec+pipe，Windows 用 CreateProcess+匿名管道 + PeekNamedPipe 轮询 drain）；PTY 需 `portable-pty`/`nix`（POSIX）与 Windows ConPTY（`conpty` crate）。依赖 `dsh-subprocess` + `dsh-timeout`。

---

## 3. shell（bash/pwsh 执行 seam + 工具）

### 3.1 `@deepseek-ai/dsh-shell`（抽象 seam，`ctx.shell`）
- **用途**：前台命令 + 后台进程句柄。`resolve(request)→spec`、`run(spec)`、`start(spec)`。作业 id/所有权/轮询/通知归 `dsh-jobs`。
- **workspace 依赖**：`dsh-invariants/dsh-subprocess/dsh-sandbox/cordis/dsh-settings`（peer）。
- **src 顶层文件**：`index.ts`、`types.ts`、`render.ts`、`invariant.ts`。
- **导出面**：服务 `ShellExecutor`（`ctx.shell`）；`SHELL_SETTINGS_NAMESPACE`；`parseExitStatus`/`ParsedExitStatus`；类型 `ShellExecRequest/Spec`、`ShellRunResult`、`ShellProcess`、`ShellProcessRead`、`ShellProcessStatus`（`running|completed|killed`）、`ShellSandboxInfo`；再导出 `DSH_ENV_PREFIX`、`CollectedOutput`、`DshEnvironment` 等。
- **Rust 移植**：`crates/dsh-shell/`。依赖 `dsh-subprocess` + `dsh-sandbox`。

### 3.2 `@deepseek-ai/dsh-shell-env`（`ctx.shellEnv`）
- **用途**：工具无关的受管 `DSH_*` 环境注册表；内置 `DSH_HOME`/`DSH_SHELL`/`DSH_SESSION_ID`，插件可注册可枚举事实。
- **workspace 依赖**：`dsh-shell`、`dsh-home-paths`、`dsh-session-persistence`、`dsh-tools`、`schemastery`。
- **src 顶层文件**：`index.ts`、`invariant.ts`。
- **导出面**：服务 `ShellEnvRegistry`；`BashEnvContributor`/`BashEnvVariable`/`BashEnvVariableInfo`；`Config`；`name`/`inject`/`apply`。
- **Rust 移植**：`crates/dsh-shell-env/`。难点低。

### 3.3 执行器实现（bash/pwsh 的 local 与 sandbox 变体）
| 包 | 服务/类 | 说明 | src 文件 |
|---|---|---|---|
| `dsh-bash-local` | `LocalBashExecutor`（`ctx.shell`） | 经 subprocess 跑 `bash`；`ENV_OVERRIDES`、`Config`、`assertServiceableBashConfig` | `index.ts`,`invariant.ts` |
| `dsh-bash-sandbox` | `SandboxBashExecutor extends LocalBashExecutor` | 每个命令经 `ctx.sandbox` confine，报告 denial/enforcement facts；`Config = LocalConfig` | `index.ts`,`helpers.ts`,`invariant.ts` |
| `dsh-pwsh-local` | `PwshLocalExecutor`（`ctx.shell`） | PowerShell；`ENV_OVERRIDES`、`ENCODING_PREAMBLE`、`resolvePwshPath` | `index.ts`,`resolve.ts`,`invariant.ts` |
| `dsh-pwsh-sandbox` | `SandboxPwshExecutor extends PwshLocalExecutor` | sandbox 变体 | `index.ts`,`helpers.ts`,`invariant.ts` |

- **Rust 移植**：`crates/dsh-bash-local/`、`dsh-bash-sandbox/`、`dsh-pwsh-local/`、`dsh-pwsh-sandbox/`。难点：shell 语义（`bash -c` 包装、exit status 解析、编码 preamble for pwsh）、sandbox 变体把 `confine` 结果拼进 argv。

### 3.4 工具层
| 包 | 工具 | inject | 说明 |
|---|---|---|---|
| `dsh-tool-bash` | `bash`（可带 background + sandbox escalation） | `tools,shell,systemPrompt,shellEnv` | src `index.ts`,`background.ts`,`render.ts`,`invariant.ts` |
| `dsh-tool-pwsh` | `pwsh` | 同上 | src `index.ts`,`background.ts`,`render.ts`,`invariant.ts` |
| `dsh-tool-bash-persistent` | 持久 bash（基于 PTY 服务） | `tools,terminals` | src `index.ts`,`invariant.ts` |

- **Rust 移植**：`crates/dsh-tool-bash/` 等。难点：schema 生成 + escalation 审批编排 + 后台作业接入 `dsh-jobs`。

---

## 4. terminal（持久 PTY 会话 seam + bash 后端 + 工具）

### 4.1 `@deepseek-ai/dsh-terminal`（`ctx.terminals`）
- **用途**：owner 作用域的持久 PTY 会话注册表：id、发布、授权、交互式 send/read/signal、await 清理。后端拥有终端机制，服务拥有 id/授权/清理。
- **workspace 依赖**：`dsh-agent`、`dsh-brand`、`dsh-invariants`、`cordis`（peer）。
- **src 顶层文件**：`index.ts`、`types.ts`、`invariant.ts`。
- **导出面**：服务 `TerminalSessionService`（`ctx.terminals`）；`TerminalError`/`TerminalErrorCode`；`TerminalSessionId`；类型 `TerminalBackend`/`TerminalBackendSession`/`TerminalSpawnRequest/Result`/`TerminalSendRequest/Result`/`TerminalSendOperation`/`TerminalReadRequest/Result`/`TerminalSignalResult`/`TerminalSessionSnapshot/Status`/`TerminalSignal`/`TerminalWaitReason`；`TerminalBackendCleanupError`。
- **Rust 移植**：`crates/dsh-terminal/`。难点：owner 授权（精确 Agent 身份）、并发 send 互斥（`SEND_ACTIVE`）、pending spawn 回滚。

### 4.2 `@deepseek-ai/dsh-terminal-bash`
- **用途**：基于 subprocess terminal 原语的持久 shell PTY 后端。
- **workspace 依赖**：`dsh-terminal`、`dsh-sandbox`、`dsh-sandbox-policy`、`dsh-session`、`dsh-subprocess`、`schemastery`。
- **src 顶层文件**：`index.ts`、`session.ts`、`config.ts`、`sanitize.ts`、`invariant.ts`。
- **导出面**：`BashTerminalBackend implements TerminalBackend`；`name`/`inject`/`apply`；`Config`。
- **Rust 移植**：`crates/dsh-terminal-bash/`。难点：PTY 会话 → 滚动缓冲 → 前景进程组信号 → 等待原因（`stdin_read|inferred_idle|timeout|session_exit`）。

### 4.3 `@deepseek-ai/dsh-tool-terminal`
- **用途**：6 个模型可见的持久 PTY 工具（open/send/signal/read/kill/list），owner 隔离 + 后台作业集成。
- **workspace 依赖**：`dsh-terminal`、`dsh-output-retention`、`dsh-jobs`、`dsh-tools`、`schemastery`。
- **src 顶层文件**：`index.ts`、`render.ts`、`invariant.ts`。
- **导出面**：`name`/`inject`（`terminals,tools,systemPrompt`）/`Config`/`apply`；`DEFAULT_MAX_RESULT_BYTES`。
- **Rust 移植**：`crates/dsh-tool-terminal/`。

---

## 5. fs（文件系统 seam + 实现 + 观察策略 + 工具）

### 5.1 `@deepseek-ai/dsh-fs`（抽象 seam，`ctx.fs`）
- **用途**：单执行世界文件能力：稳定 target 身份、containment、UTF-8 读、二进制拒绝、原子写/编辑（version 守卫）。定义 `fs/*` 策略事件。
- **workspace 依赖**：`dsh-brand`、`dsh-invariants`、`dsh-llm`、`dsh-sandbox`、`cordis`（peer）。
- **src 顶层文件**：`index.ts`、`types.ts`、`invariant.ts`。
- **导出面**：
  - 服务：`FileSystem`（`ctx.fs`），抽象方法 `resolve/processPath/fileUrl/contains/stat/lstat/readText/streamText/readBytes/listDir/writeText/editText`；`get sandboxMode()`。
  - 事件：`fs/write-intent`（waterfall）、`fs/edit-intent`（waterfall）、`fs/observed`（emit）。
  - 值：`FsError`（code `FsErrorCode`）、`FsTargetKey`、`FsVersion`。
  - 类型：`FsTarget`/`FsInfo`/`FsPathInfo`/`FsDirEntry`/`FsWriteIntent`/`FsWriteOutcome`/`FsEditRequest`/`FsEditOutcome`/`FsObservation`。
  - `FsErrorCode`：`FS_NOT_FOUND/NOT_DIRECTORY/NOT_TEXT/NOT_REGULAR_FILE/TOO_LARGE/PERMISSION_DENIED/SANDBOX_DENIED/IO_ERROR/STALE_VERSION/NOT_OBSERVED/AMBIGUOUS_EDIT/EDIT_NOT_FOUND/ABORTED`。
- **Rust 移植**：`crates/dsh-fs/`。难点：opaque `FsTargetKey`/`FsVersion` 品牌类型；`editText` 字面替换（old/new/replaceAll + LF 归一化）。

### 5.2 `@deepseek-ai/dsh-fs-local`（本地后端）
- **用途**：realpath 派生 target 身份（别名共享 stale 守卫）；写经 symlink 更新目标；每 targetKey 的 FIFO 锁串行化变更。
- **workspace 依赖**：`koffi`、`schemastery`；peer `dsh-fs/dsh-invariants/cordis`。
- **src 顶层文件**：`index.ts`、`fsio.ts`、`win32.ts`、`invariant.ts`。
- **导出面**：服务 `LocalFileSystem`（`ctx.fs`）；`Config`（`cwd`、`diffBasisMaxBytes` 默认 10 MiB）；`FsIoInternals`。
- **Rust 移植**：`crates/dsh-fs-local/`。难点：原子写（temp+fsync+rename）、高精度版本令牌（Windows 用 koffi 拿 stat identity）、`readForEdit`/`restoreLineEndings`（CRLF 保留）。

### 5.3 `@deepseek-ai/dsh-fs-sandbox`（sandbox 强制后端）
- **用途**：`SandboxedFileSystem extends LocalFileSystem`，只给两个变更（writeText/editText）加 per-call 策略 fence；读透传。read-only 拒绝所有变更；workspace-write 要求 target 在 `writableRoots` 内；danger-full-access 不设限。
- **workspace 依赖**：`dsh-fs`、`dsh-fs-local`、`dsh-sandbox`、`dsh-sandbox-policy`、`cordis`。
- **src 顶层文件**：`index.ts`、`containment.ts`、`invariant.ts`。
- **导出面**：服务 `SandboxedFileSystem`（`static inject = ['sandboxPolicy']`，覆盖 `sandboxMode`）；`Config = LocalConfig`。
- **Rust 移植**：`crates/dsh-fs-sandbox/`。难点：`isPathUnder`（词法快路径 + 祖先 stat 同身份回退，处理 8.3/case）；重新 canonicalize 后再写以收窄 TOCTOU。

### 5.4 `@deepseek-ai/dsh-fs-observation-policy`（事件-only，无服务）
- **用途**：弱 owner/target map 记录 `fs/observed`；`fs/write-intent`/`fs/edit-intent` 据此派生 guard（未观察→`createIfAbsent`；已 present→`replaceIfVersion`；edit 未观察→`FS_NOT_OBSERVED`）。
- **src 顶层文件**：`index.ts`、`types.ts`、`invariant.ts`。
- **导出面**：`name`/`apply`；类型 `FsObservationActor`。
- **Rust 移植**：`crates/dsh-fs-observation-policy/`。难点：WeakMap 语义 → `Weak<dyn Owner>` 键 + `HashMap<targetKey, Observation>`。

### 5.5 工具层
| 包 | 工具 | inject | src 文件 |
|---|---|---|---|
| `dsh-tool-fs` | `read`/`write`/`edit` | `tools,fs,systemPrompt` | `index.ts`,`read.ts`,`write.ts`,`edit.ts`,`diff.ts`,`read-target.ts`,`read-render.ts`,`read-image.ts`,`sandbox.ts`,`session-cwd.ts`,`error.ts`,`invariant.ts` |
| `dsh-tool-fs-search` | `glob`/`grep`（调打包的 ripgrep） | `tools,systemPrompt,subprocess` | `index.ts`,`glob.ts`,`grep.ts`,`search-core.ts`,`direct-call.ts`,`presentation.ts`,`ripgrep.d.ts`,`invariant.ts` |
| `dsh-tool-str-replace-editor` | `view`/`create`/`replace`/`insert` | `tools,fs` | `index.ts`,`invariant.ts` |

- **Rust 移植**：`crates/dsh-tool-fs/`、`dsh-tool-fs-search/`（ripgrep 已是 Rust，直接复用 `ignore`+`grep` crate）、`dsh-tool-str-replace-editor/`。`dsh-tool-fs` 依赖 `diff`（Rust 用 `similar`）。

---

## 6. workspace（工作区注册表）

### `@deepseek-ai/dsh-workspace`（`ctx.workspaceRegistry`）
- **用途**：基于 storage-domain 的持久工作区记录（id/path/title/sessionIds/时间戳），header 校验的会话归属，稳定顺序，一次性历史 bootstrap，`pendingMutation` 两写恢复。
- **workspace 依赖**：`dsh-brand`、`dsh-storage-domain`、`dsh-invariants`、`dsh-session`、`dsh-session-persistence`、`dsh-storage`、`cordis`；`zod`。
- **src 顶层文件**：`index.ts`、`types.ts`、`entity.ts`、`spec.ts`、`paths.ts`、`invariant.ts`。
- **导出面**：服务 `WorkspaceRegistry`（`static inject = ['storageDomain','sessionPersistence']`）；`WorkspaceId`（brand）、`Workspace`、`WorkspaceMoveInvalidError`、`WorkspaceUnknownSessionError`、`WorkspaceOrderInvalidError`；`workspaceDomainSpec`（`defineDomain({name:'workspace', version:2, global: workspaceDomainState, tables:{workspaces: workspaceRecord}})`）、`workspaceRecord`、`workspaceDomainState`、`realpathNormalize`。
- **Rust 移植**：`crates/dsh-workspace/`。难点：domain schema（zod → serde/schemars）、bootstrap 历史折叠、`pendingMutation` 崩溃恢复。依赖 storage-domain + session-persistence。

---

## 7. storage（存储 hub + domain 数据形式 + JSON/SQLite 后端）

### 7.1 `@deepseek-ai/dsh-storage`（`ctx.storage` hub）
- **用途**：命名后端注册表（`backend`）+ 挂载的数据形式（`forms`）。hub 自身不做 IO。
- **workspace 依赖**：`dsh-invariants`、`cordis`（peer）。
- **src 顶层文件**：`index.ts`、`backend.ts`、`registry.ts`、`error.ts`、`invariant.ts`。
- **导出面**：服务 `Storage`（`ctx.storage`，`mount/form/domain`）；`BackendRegistry`；`StorageError`/`StorageErrorCode`；`UNIT_NAME_RE = /^[a-z][a-z0-9_]*$/`；`storageBackendServiceKey(name)`；类型 `StorageBackend`/`KvFacet`/`KvUnit`/`KvUnitDescriptor`；`StorageForms`。
- **关键契约**：`KvUnit`：`loadAll()`、`putRecord`、`deleteRecord`、`setGlobal`、`close`；写顺序由调用方负责，每次调用保证介质原子 + 落盘。

### 7.2 `@deepseek-ai/dsh-storage-domain`（`ctx.storage.domain`）
- **用途**：schema 校验 + 事件发射的 KV domain。`defineDomain({name,version,global,tables})`；`open` 打开 unit、校验记录（zod）、构造 `Domain`；每次持久写后发射 `domain/changed` 事件。
- **workspace 依赖**：`dsh-storage`、`schemastery`、`zod`。
- **src 顶层文件**：`index.ts`、`domain.ts`、`spec.ts`、`events.ts`、`error.ts`、`invariant.ts`。
- **导出面**：服务 `DomainFacility`（`ctx.storageDomain` + 挂在 `ctx.storage.domain`）；`defineDomain`/`domainTable`/`descriptorOf`；`DomainError`/`DomainErrorCode`；事件 `'domain/changed'(change: DomainChanged)`（`{operation:'put'|'deleted', domain, table, key, value?}`）；类型 `Domain`/`DomainGlobal`/`KvTable`/`DomainSpec`/`DomainChanged`。
- **Rust 移植**：`crates/dsh-storage-domain/`。难点：zod 校验 → serde；写链（每 unit 单写链保证事件顺序）；`invalid-record` 定位。

### 7.3 `@deepseek-ai/dsh-storage-json`（后端 `json`）
- **用途**：root 下每个 unit 一个 `<unit>.json` 人类可读文件，原子整文件重写。见 §14 目录/格式。
- **workspace 依赖**：`dsh-storage`、`schemastery`。
- **src 顶层文件**：`index.ts`、`unit.ts`、`atomic.ts`、`format.ts`、`invariant.ts`。
- **导出面**：`JsonStorageBackend`（注册 backend `json`）；`name`/`inject`/`Config`（`root` 必填）/`apply`。

### 7.4 `@deepseek-ai/dsh-storage-sqlite`（后端 `sqlite`）
- **用途**：一个 DB 文件承载所有路由 unit，document-per-row（`key TEXT`/`value TEXT` JSON）。见 §14 schema。
- **workspace 依赖**：`dsh-storage`、`schemastery`；Node 内建 `node:sqlite`（`DatabaseSync`）。
- **src 顶层文件**：`index.ts`、`schema.ts`、`unit.ts`、`invariant.ts`。
- **导出面**：`SqliteStorageBackend`（注册 backend `sqlite`）；`STORAGE_SQLITE_SCHEMA_VERSION = 1`；`JournalMode`；`name`/`inject`/`Config`（`path`、`journalMode?` 默认 `wal`）/`apply`。
- **Rust 移植**：`crates/dsh-storage-sqlite/`（`rusqlite`）。难点：`STRICT` 表、`user_version` 版本戳、`ON CONFLICT DO UPDATE` upsert、JSON 文本值。

---

## 8. spill（溢出存储 seam + 本地实现 + 策略）

### 8.1 `@deepseek-ai/dsh-spill`（`ctx.spillStore`）
- **用途**：`saveText` 持久化超大文本，返回 locator + 检索指引。极简：只有 `saveText`。
- **workspace 依赖**：`dsh-brand`、`dsh-invariants`、`dsh-llm`、`dsh-session`、`cordis`（peer）。
- **src 顶层文件**：`index.ts`、`types.ts`、`invariant.ts`。
- **导出面**：服务 `SpillStore`（`ctx.spillStore`）；`SpillLocator`；类型 `SaveTextSpill`/`SpillRef`/`SpillOwner`/`SpillSource`。

### 8.2 `@deepseek-ai/dsh-spill-local`（本地后端）
- **用途**：session 作用域私有文件。见 §14 目录/格式。
- **workspace 依赖**：`dsh-spill`、`schemastery`。
- **src 顶层文件**：`index.ts`、`store.ts`、`invariant.ts`。
- **导出面**：服务 `LocalSpillStore`；`Config`（`root?`，缺省用 tmpdir 下 `mkdtemp('dsh-spill-')`）；再导出 `encodeSegment`/`privateRoot`/`saveTextFile`/`sessionDir`。

### 8.3 `@deepseek-ai/dsh-spill-policy`（事件-only，无服务）
- **用途**：`tools/post-execute` 结果变换器 + `tools/code-dispatch-log` 日志臂。超过 `maxInlineBytes` 的结果 spill 后替换为 head/tail 预览 + notice。best-effort（失败保留原文）。跳过 `read` 与嵌套调用。
- **workspace 依赖**：`dsh-invariants`、`dsh-llm`、`dsh-output-retention`、`dsh-session`、`dsh-spill`、`dsh-tools`、`schemastery`。
- **src 顶层文件**：`index.ts`、`types.ts`、`invariant.ts`。
- **导出面**：`name = 'spill-policy'`、`inject = ['tools']`、`Config`（`maxInlineBytes?`）、`apply`、类型 `SpillPolicyExec`。
- **Rust 移植**：`crates/dsh-spill-policy/`。难点：notice 字节预算预留、head/tail 保留（`TextRetainer`）。

---

## 9. jobs（后台作业注册表 + 本地实现 + 工具）

### 9.1 `@deepseek-ai/dsh-jobs`（`ctx.jobs` 抽象 seam）
- **用途**：作业 id、session 作用域访问、生命周期状态、完成监听、owner 清理；生产者保留执行资源。
- **workspace 依赖**：`dsh-agent`、`dsh-brand`、`dsh-invariants`、`dsh-session`、`cordis`（peer）。
- **src 顶层文件**：`index.ts`、`types.ts`、`brand.ts`、`invariant.ts`。
- **导出面**：服务 `JobRegistry`（抽象，构造时 fail-loud）；`JobId`；类型 `JobStatus`（`running|stopping|completed|killed|failed`）、`JobKind`/`JobKindMap`（bash/subagent）、`JobStart`/`JobHooks`/`JobOutcome`/`JobSnapshot`/`JobRead`/`JobDoneListener`/`JobsChangedListener`。抽象方法 `start/list/get/read/kill/wait/onJobDone/onJobsChanged/attachController`。
- **Rust 移植**：`crates/dsh-jobs/`。依赖 `dsh-agent`/`dsh-session`。

### 9.2 `@deepseek-ai/dsh-jobs-local`（本地实现）
- **用途**：内存注册表，按 owner 分桶，作用域分层（`dsh-scope`）。生命周期见 §14。
- **workspace 依赖**：`dsh-scope`、`dsh-timeout`、`schemastery`；peer `dsh-agent/dsh-jobs`。
- **src 顶层文件**：`index.ts`、`invariant.ts`。
- **导出面**：服务 `LocalJobRegistry`；`Config`（`maxConcurrentJobsPerOwner` 默认 10）；`TASK_WAIT_TIMEOUT`。

### 9.3 `@deepseek-ai/dsh-tool-jobs`
- **用途**：模型可见工具 `job_output`/`job_list`/`job_kill`。
- **workspace 依赖**：`dsh-jobs`、`dsh-output-retention`、`dsh-tools`、`schemastery`。
- **src 顶层文件**：`index.ts`、`invariant.ts`。
- **导出面**：`name`/`inject`（`tools,jobs,systemPrompt`）/`Config`（`CompletionDelivery`）/`PublicJobSnapshot`/`statusLine`/`apply`。
- **Rust 移植**：`crates/dsh-tool-jobs/`。

---

## 10. schedule（agent 作用域持久定时提醒）

### `@deepseek-ai/dsh-schedule`
- **用途**：`after`（延迟秒）、`at`（绝对/本地时区一次性）、`every`（固定频率，≥300s）提醒，存于会话事件日志（`schedule/change` 事件），disposable 定时器投影驱动。
- **workspace 依赖**：`dsh-agent`、`dsh-brand`、`dsh-invariants`、`dsh-llm`、`dsh-session`、`dsh-session-persistence`、`dsh-tools`、`cordis`。
- **src 顶层文件**：`index.ts`、`domain.ts`、`runtime.ts`、`persistence.ts`、`transaction.ts`、`tools.ts`、`types.ts`、`invariant.ts`。
- **导出面**：`name`/`inject`（`agents,sessions,tools,sessionPersistence`）/`apply`；`SCHEDULE_CHANGE_VERSION = 1`、`MIN_EVERY_INTERVAL_SECONDS = 300`；`ScheduleId`、`ScheduleInputError`、`ScheduleLogError`、`allocateScheduleId`、`createAfter/At/EveryScheduleRecord`、`decodeScheduleChange`、`foldScheduleEvents`、`resolveEveryOccurrence`、`scheduleView`、`renderReminderFraming`、`renderEveryReminderBatchFraming`、`registerScheduleTools`。
- **关键逻辑**：`foldScheduleEvents` 纯重放；`dueDecision` 选到期 one-shot 或 every 批或 next wake；`ScheduleRuntime` 用 `setTimeout`（上限 `MAX_TIMER_DELAY_MS`）+ agent idle 边界 + `runMaintenance` 驱动；dispatch 后 append `schedule/change`（operation dispatch，含 `acceptedAt` for every）。
- **Rust 移植**：`crates/dsh-schedule/`。难点：IANA 时区/本地日历投影（`chrono-tz`）、DST gap/overlap 处理、定时器 + idle 驱动。依赖 session 事件日志。

---

## 11. code-runtime（代码执行 seam + worker-thread 实现）

### 11.1 `@deepseek-ai/dsh-code-runtime`（`ctx.codeRuntime` 抽象 seam）
- **用途**：运行模型写的一个程序，绑定宿主 async 函数。错误是结果字段，不是 reject。
- **workspace 依赖**：`dsh-invariants`、`cordis`（peer）。
- **src 顶层文件**：`index.ts`、`types.ts`、`invariant.ts`。
- **导出面**：服务 `CodeRuntime`（`ctx.codeRuntime`，抽象 `language`/`isolation`/`run`）；`RESERVED_BINDING_GLOBALS`、`RESERVED_ERROR_MEMBERS`、`DUNDER_MEMBER`、`PORTABLE_RESERVED_WORDS`；类型 `CodeRunRequest`/`CodeRunResult`/`CodeRunFailure`/`CodeBindingNamespace`/`CodeBindingFunction`/`CodeBindingErrorClass`/`CodeJsonValue`。
- **Rust 移植**：`crates/dsh-code-runtime/`。难点：可移植标识符/保留字集合、lossless JSON 绑定桥。

### 11.2 `@deepseek-ai/dsh-code-runtime-worker-thread`
- **用途**：每 run 一个 fresh worker 跑 type-strip 后的 TS；空环境 + heap cap + 忙时/墙钟预算 + 终止（含同步循环）。containment，非安全边界。
- **workspace 依赖**：`dsh-code-runtime`、`dsh-session`、`dsh-timeout`、`schemastery`。
- **src 顶层文件**：`index.ts`、`worker.ts`、`bootstrap.ts`、`protocol.ts`、`worker-json.ts`、`output-json.ts`、`invariant.ts`。
- **导出面**：服务 `WorkerThreadCodeRuntime`（`language='typescript'`, `isolation='worker-thread'`）；`Config`（`computeMs` 60s、`maxWallMs` 600s、`maxOutputBytes` 64 MiB、`maxOldGenerationSizeMb` 512）。
- **Rust 移植**：`crates/dsh-code-runtime-worker-thread/`。难点：JS worker + ELU 忙时计量 → Rust 侧难以 1:1（建议映射为 `deno_core`/`boa`/子进程 + 资源限制）。作为“隔离执行 TS”的后端，Rust 可改选 `deno_core` 或进程级 RLIMIT。

---

## 12. e2b（远程 E2B 沙箱适配器）

### 12.1 `@deepseek-ai/dsh-e2b`
- **用途**：共享 E2B 沙箱生命周期（Sandbox 创建/销毁、控制环境变量、shell 参数 quote）。
- **workspace 依赖**：`e2b` SDK `2.29.1`、`schemastery`；peer `dsh-invariants/cordis`。
- **src 顶层文件**：`index.ts`、`invariant.ts`。
- **导出面**：`E2BRuntime extends Service`；`Config`；`quoteE2BShellArg`、`e2bControlEnvs`；再导出 SDK 类型 `CommandHandle/CommandResult/EntryInfo`。
- **Rust 移植**：`crates/dsh-e2b/`。难点：E2B 是远程 HTTP/WebSocket API，用 `reqwest`+`e2b` Rust SDK（或自实现）。

### 12.2 `@deepseek-ai/dsh-fs-e2b` / `@deepseek-ai/dsh-subprocess-e2b`
- **用途**：分别把 `ctx.fs`、`ctx.subprocess` 的 seam 用 E2B Sandbox API 实现。
- **src 顶层文件**：`fs-e2b/src/index.ts`（+invariant）；`subprocess-e2b/src/index.ts`（+`environment.ts`/`process.ts`/`output.ts`/`remote.ts`/`terminal.ts`/invariant）。
- **导出面**：`E2BFileSystem extends FileSystem`、`E2BSubprocessRuntime extends SubprocessRuntime`。
- **Rust 移植**：`crates/dsh-fs-e2b/`、`dsh-subprocess-e2b/`。难点：远端 target 身份、流式输出、远端 PTY。

---

## 13. credentials（凭据引用 seam + 本地文件 provider）

### 13.1 `@deepseek-ai/dsh-credentials`（`ctx.credentials`）
- **用途**：设置/组合文件只携带引用（环境变量名），provider 拥有值。`resolve/describe/set/unset`；空值在处处视为不存在。
- **workspace 依赖**：`dsh-brand`、`dsh-invariants`、`cordis`（peer）。
- **src 顶层文件**：`index.ts`、`types.ts`、`invariant.ts`。
- **导出面**：服务 `CredentialProvider`（`ctx.credentials`）；`credentialRef(value)`（POSIX 标识符校验）；`CredentialRef`、`ResolvedCredential`、`CredentialInfo`；事件 `'credentials/updated'(ref)`（emit，含 INVARIANT 重抛语义）。
- **Rust 移植**：`crates/dsh-credentials/`。

### 13.2 `@deepseek-ai/dsh-credentials-local`
- **用途**：文件后端 `$DSH_HOME/.credentials.yaml`，分层：继承进程环境（只读，赢）> `.credentials.yaml`（可写）> 项目 `.env` > 用户 `.env`。写时读改写（跨进程文件锁 + 保留注释/格式）；chokidar 热加载。
- **workspace 依赖**：`dsh-atomic-write`、`dsh-credentials`、`dsh-launch-environment`、`dsh-home-paths`、`schemastery`、`yaml`、`chokidar`。
- **src 顶层文件**：`index.ts`、`invariant.ts`。
- **导出面**：服务 `LocalCredentialProvider`；`CREDENTIALS_FILENAME = '.credentials.yaml'`；`resolveSpec`、`parseCredentialsDocument`；`Config`（`path/dshHome/watch/debounceMs`）。
- **Rust 移植**：`crates/dsh-credentials-local/`。难点：跨进程文件锁（`fs2`）、YAML 注释保留编辑（`serde_yaml` 不保留注释 → 需行级编辑）、chokidar → `notify`。依赖 `dsh-atomic-write`。

---

## 14. 专题详解（任务指定）

### 14.1 sandbox 权限模式枚举与判定逻辑

**三种模式**（`SandboxMode`，`dsh-sandbox` 定义）：
- `read-only`：只允许必需 sink（`/dev/null`）；拒绝一切写。
- `workspace-write`：允许写 `workspaceRoot` + `/tmp` + 平台 temp（`os.tmpdir()`）；`writableRoots()` 生成 canonical、去重后的该集合。
- `danger-full-access`：绕过限制。

**判定（per-call 解析）**，`dsh-sandbox-policy`：
1. 会话级覆盖：会话事件日志中最后一条 `sandbox/mode`（`effectiveSandboxMode` 从后往前扫）。
2. 优先级：显式 approved `mode`（升级批准结果）> 会话 override > 部署默认 `Config.mode`（缺省 `read-only`）。
3. workspaceRoot = 会话 `header.cwd` ?? 配置 fallback（缺省 `process.cwd()`）。
4. 执行侧 `ctx.sandboxPolicy.resolve({session})` 把 mode+root+sessionId 盖到每次能力调用；`setSandboxMode` 只是 append 事件，下次调用生效。

**各平台执行方言**（`dsh-sandbox-local`）：
- Linux bwrap：`--ro-bind / /` 全只读，workspace-write 加 `--bind wsRoot wsRoot --tmpfs /tmp`。denial 签名 `read-only file system`。
- Linux landlock：readOnly `/`，readWrite `/dev/null`(+`/tmp`,workspaceRoot)。denial 签名 `permission denied`；launcher 自报 partial（老 ABI）。
- macOS Seatbelt：SBPL `(deny file-write*) (allow file-write* (literal "/dev/null")) (allow file-write* (subpath ...))`。denial 签名 `operation not permitted`。
- Windows ACL（下述）。

**Windows 机制——是受限令牌（Restricted Token），不是 Job Object**：
- `CreateRestrictedToken` 带 `DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED`。Job Object 只用于 `stdio: 'inherit'` spawn 的 **kill-on-close 孤儿兜底**（`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`），不是隔离手段。
- **限制 SID 列表**（`createRestrictedToken`）：
  - `read-only`：`[logon SID, Everyone]`（无写 SID）。
  - `workspace-write`：`[logon SID, Everyone, workspace SID, 可选 temp SID]`。
- **能力 SID**：`workspaceWriteSid(canonicalWorkspacePath)` 每 workspace 确定；`tempWriteSid(privateTempDir)` 每 live session/workspace 对随机。通过 `SetEntriesInAclW` 给对应目录 DACL 加写 ACE（grant），使 pass-2 交叉检查只在这些目录放行写。
- **授权生命周期**：workspace ACE 常驻（跨 session 复用缓存，从不撤销）；temp ACE 可撤销（provider dispose 时撤）。`AclWriteGrant` 由 `dsh-sandbox-local` 持有（`manageDacls:false` 时 seam 拥有 DACL）。
- **已知边界（enforcement = `'partial'`）**：Everyone 必须留在限制列表；NTFS 硬链接可把已授权文件别名到工作区外路径；console 隔离不可用（`CREATE_NO_WINDOW/CREATE_NEW_CONSOLE` 子进程 `STATUS_DLL_INIT_FAILED`）；只限写，不限读/网络/进程可见性；CIM/WMI 不可用（Authenticated Users 缺席）。
- **spawn**：`CreateProcessAsUserW`（受限令牌）+ 匿名管道 stdio；`PeekNamedPipe` 轮询 drain；`bInheritHandles=1`。

**fs-sandbox（进程内 fence，可信代码对模型路径）**：read-only 抛 `FS_SANDBOX_DENIED`；workspace-write 重新 canonicalize 后判 `isPathUnder`（词法 + 祖先 stat 同 dev/ino 回退）；danger-full-access 直接委托。

**升级（escalation）**：`WIDER_MODES` 严格放宽表；`approveEscalation` 先检查严格放宽 → 审批通道 → 映射结果，所有失败路径抛不同文本，未运行任何东西。

### 14.2 jobs 后台任务生命周期

**状态机**：`running` →（可选）`stopping` → 恰好一个终态 `completed | killed | failed`。

**注册（`start(spec)`）**，`dsh-jobs-local`：
1. 前置检查：必须有 controller 服务该 owner（否则拒绝）；kind/label 非空；`outputLimitBytes` 正整数；owner live；`maxConcurrentJobsPerOwner`（默认 10）上限；`ensureOwnerCleanup`（owner 作用域 cleanup，跨 fiber）。
2. `spec.run()` 同步返回 `JobHooks`（`cancel`/`done: Promise<JobOutcome>`/`readOutput?`）。
3. 分配 id `<kind>-N`（per-kind 计数）；记录 `running`，`done.then` 挂 settle（reject 转 `failed`）；注册完成后 `notifyChanged`。

**读（`read`）**：stream 类用 `readOutput()` 消耗游标；final-output 类 live 时空、终态返回 `JobOutcome.output`（幂等）。终态 read 置 `reported = true`。

**取消（`kill`）**：终态 → 置 reported，返回 `already-finished`；live → 先 `cancel(reason)`，置 `stopping` + reported + `notifyChanged`，返回 `requested`。

**等待（`wait`）**：`deadline`（`dsh-timeout`）+ waiter 计数；结算释放 waiter；wait 成功或 timeout 后置 reported。

**结算（`settle`）**：first-wins 记录终态 + `finishedAt`；释放 waiters；`markSettled`；`notifyChanged`；最后通知 `onJobDone` 监听器（owner-relative，作用域分层：全局层 + owner scope 链；返回 promise 被观察不 await；`listenersClosed` 后不再通知）。

**作用域/监听**：`ScopedLayers<JobLayer>`（controllers/listeners/changed 分层）；`attachController` 决定谁可 start；`onJobDone`/`onJobsChanged` 按注册作用域投递。

**清理**：owner 销毁 → `disposeOwned`（cancel-for-teardown + await settled + delete + notifyChanged）；服务销毁 → `disposeAll`（close listeners + cancel 全部 + await + 清空 + notifyChanged + detach owner effects）。teardown 的 cancel 若 throw 则 force-fail 该记录（标记“可能孤儿”）。

### 14.3 storage 与 spill 的目录布局与文件格式

**storage-json**：
- 目录：配置 `root` 下，每个 unit 一个文件 `<unit>.json`。
- 格式：pretty-printed JSON + 尾换行：
  ```json
  { "unit": { "name": "...", "version": N },
    "global": <value | null>,
    "tables": { "<table>": { "<key>": <value>, ... }, ... } }
  ```
- 版本戳：`unit.version`（不匹配 → `version-mismatch`）；`global` null 表示“未写”。
- 原子写：同目录 `.`+uuid+`.tmp`，`open('wx',0600)` → write → fsync → `rename`（覆盖）→ POSIX fsync 父目录；内存为权威，写失败回滚内存。

**storage-sqlite**：
- 一个 DB 文件承载所有 unit；`PRAGMA user_version = 1`（物理布局版本，非 0 且非 1 拒绝）。
- 表：`units(name TEXT PK, version INTEGER)`（每 unit 版本戳）、`unit_globals(unit TEXT PK→units, value TEXT)`、每 unit 表 `u_<unit>_<table>(key TEXT PK, value TEXT NOT NULL) STRICT`。
- 值列存 `JSON.stringify` 文本；upsert `INSERT ... ON CONFLICT(key) DO UPDATE SET value=excluded.value`；`journal_mode` 默认 `wal`。
- 每写单语句原子；`user_version` 在 schema 完成后才盖（崩溃后重试）。

**spill-local**：
- 根：配置 `root`，或缺省 `mkdtemp(os.tmpdir()/dsh-spill-*)`（0700）。
- session 目录：`<root>/session-<sha256(sessionId)[:12]>`（0700）。
- 文件：`<randomBytes(6).hex>-<encodeSegment(suggestedName)>`，独占 `open('wx', 0600)`。
- `encodeSegment`：可逆单段编码，保留 `[A-Za-z0-9._-]`（除 `~`），其余转 `~XXXX`（大写 4 位十六进制）；`.`/`..`/空串转义，防遍历/注入。

**spill-policy**：结果 UTF-8 超过 `maxInlineBytes` 时 spill 全文，替换为 `head/tail 预览 + 空行 + notice`；notice 预算从 cap 内预留（worst-case 定价）；替换超 cap 则保留原文。`retrievalHint = "Use read with offset/limit, or grep this path to search within it."`。

---

## 15. 依赖顺序提示（自底向上）

1. 基础设施：`dsh-brand`、`dsh-invariants`、`cordis`（DI/Service/Events 内核）、`schemastery`（schema）。
2. `dsh-llm`（`HarnessError`/`CallId`）、`dsh-session`（SessionId/事件日志）、`dsh-timeout`、`dsh-scope`、`dsh-home-paths`、`dsh-atomic-write`、`dsh-launch-environment`、`dsh-settings`。
3. 本组 leaf：`dsh-sandbox`（+`dsh-sandbox-policy`、`dsh-sandbox-windows-acl`、`dsh-sandbox-local`）、`dsh-subprocess`（+local）、`dsh-storage`（+json/sqlite）、`dsh-spill`（+local）、`dsh-credentials`（+local）、`dsh-code-runtime`（+worker-thread）。
4. 组合层：`dsh-fs`（+local/sandbox/observation-policy）→ `dsh-shell`（+local/sandbox）→ `dsh-terminal`（+bash）→ `dsh-storage-domain` → `dsh-workspace` → `dsh-jobs`（+local）。
5. 工具/策略层（依赖 `dsh-tools`/`dsh-output-retention`/`dsh-system-prompt`/`dsh-user-approval`）：`dsh-tool-bash/pwsh/bash-persistent`、`dsh-tool-terminal`、`dsh-tool-fs/fs-search/str-replace-editor`、`dsh-tool-jobs`、`dsh-shell-env`、`dsh-spill-policy`、`dsh-schedule`、`dsh-e2b`/`fs-e2b`/`subprocess-e2b`。

**横向耦合提示**：`dsh-shell` 与 `dsh-subprocess` 共享 `CollectedOutput`/`DshEnvironment`（shell 再导出）；`dsh-terminal` 与 `dsh-subprocess` 各自独立定义同构的 `TerminalSignal`（注释要求同步改）；`dsh-sandbox` 的 `writableRoots` 是 Seatbelt profile 与 fs-sandbox fence 的唯一共享来源，避免 bash 与 fs 的可写集漂移。

---

## 附：分组 → 真实包速查

| 任务分组 | 真实包 |
|---|---|
| sandbox | dsh-sandbox, dsh-sandbox-policy, dsh-sandbox-local, dsh-sandbox-windows-acl |
| subprocess | dsh-subprocess, dsh-subprocess-local |
| shell | dsh-shell, dsh-shell-env, dsh-bash-local, dsh-bash-sandbox, dsh-pwsh-local, dsh-pwsh-sandbox, dsh-tool-bash, dsh-tool-bash-persistent, dsh-tool-pwsh |
| terminal | dsh-terminal, dsh-terminal-bash, dsh-tool-terminal |
| fs | dsh-fs, dsh-fs-local, dsh-fs-observation-policy, dsh-fs-sandbox, dsh-tool-fs, dsh-tool-fs-search, dsh-tool-str-replace-editor |
| workspace | dsh-workspace |
| storage | dsh-storage, dsh-storage-domain, dsh-storage-json, dsh-storage-sqlite |
| spill | dsh-spill, dsh-spill-local, dsh-spill-policy |
| jobs | dsh-jobs, dsh-jobs-local, dsh-tool-jobs |
| schedule | dsh-schedule |
| code-runtime | dsh-code-runtime, dsh-code-runtime-worker-thread |
| e2b | dsh-e2b, dsh-fs-e2b, dsh-subprocess-e2b |
| credentials | dsh-credentials, dsh-credentials-local |
