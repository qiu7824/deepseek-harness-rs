# DeepSeek Harness Rust 移植宏阶段实施计划

> **For Hermes:** 使用并行 `delegate_task` worker 执行；当前 profile 没有 `subagent-driven-development` skill，因此手工执行“独立工作流、主代理集成、一次限时审查”的同等流程。

**目标：** 把当前约 54% 的真实完成度改为 4 个可运行产品宏阶段推进，每个宏阶段验收后记账约 10–13 个百分点，避免再以单包、小竞态或测试数量作为“轮次”。

**架构：** 先完成 Rust CLI/profile/Host 产品入口，再补齐本地执行与平台安全，然后并行落地外部协议生态，最后统一引入 JS 动态运行时并完成 conformance、GUI、CI 与默认切换。各宏阶段内部允许跨多次会话持续开发，只有阶段入口真实可运行且统一验收通过后才增加完成度。

**技术栈：** Rust 2024、Tokio、Cordis Rust port、Axum/Hyper、Serde/YAML、Rusqlite、Windows API/ConPTY、MCP/LSP/ACP/JSON-RPC、待决 JS runtime（`deno_core` / Boa / 隔离 sidecar，必须由阶段 4 spike 决策）。

---

## 0. 执行原则：以后不再按“小轮次”推进

### 0.1 进度记账

当前基线按真实入口加权为约 **54%（保守区间 50%–58%）**。当前候选验收证据：

- `cargo test --workspace`：**1800 passed / 0 failed / 1 ignored**；
- 378 个 `test result` 分组；
- 日志：`C:\Users\Administrator\AppData\Local\Temp\dsh-rs-round129-workspace-final-v7.log`；
- SHA-256：`30991f17ae8b1994ec97a14f5d78adf4f162adad3512bd5fe21546d80cd1c510`；
- goal-round-driver / AgentLoop / Agent / Goal / Session / Host 聚焦测试全绿；
- 关键取消用例 100 次压力通过；session-start 50 次压力通过；
- 直接修改 crate 的隔离 strict Clippy 与定向 rustfmt/diff-check 通过。

百分比只在宏阶段验收完成时更新：

| 维度 | 权重 | 当前 | 阶段 1 后 | 阶段 2 后 | 阶段 3 后 | 阶段 4 后 |
|---|---:|---:|---:|---:|---:|---:|
| Foundation / Cordis / loader / boot | 20 | 18 | 18 | 18 | 18 | 20 |
| Session / Agent / LLM / tools | 20 | 17 | 18 | 19 | 19 | 20 |
| Host / API / CLI / Web surface | 15 | 8 | 15 | 15 | 15 | 15 |
| Execution / sandbox / platform | 15 | 6 | 6 | 15 | 15 | 15 |
| Interop / protocols / dynamic features | 20 | 4 | 5 | 5 | 15 | 20 |
| Conformance / release / cutover | 10 | 1 | 3 | 4 | 5 | 10 |
| **总计** | **100** | **54** | **65** | **76** | **87** | **100** |

这些是验收目标，不是按提交、LOC 或测试数自动获得的分数。

### 0.2 每个宏阶段的固定节奏

1. **阶段开始：** 冻结入口、验收标准和共享文件 owner。
2. **并行实现：** 2–4 个互不重叠工作流；worker 只跑自己的聚焦测试。
3. **纵向集成：** 主代理统一修改 Host/profile/Cargo 共享文件并跑真实入口测试。
4. **阶段测试：**
   - 每个工作流只跑目标 crate/fixture；
   - 集成时跑一次相关 crate 全包；
   - 阶段末最多跑 **2 次** `cargo test --workspace`：一次发现问题、修复后一次最终快照；
   - 不再为每个小修复运行 workspace。
5. **审查：** 每阶段一次短审查，限时 8–10 分钟、最多 6 个生产文件组；只修本阶段 P0/P1。
6. **记账：** 真实入口、workspace、审查三门通过后一次性增加约 10 个百分点。

### 0.3 Token 与时间预算

- 不为同一个文件反复派大审查；超时一次后记录 `review unavailable`，不无限重派。
- 除非已观察到负载竞态，压力测试默认 10–20 次；100 次仅用于已证实的flake。
- 子代理不读全仓、不运行 workspace、不改 `Cargo.lock`、`PORTING.md` 或 Host composition；这些共享文件由主代理独占。
- 中间更新只在工作流完成、纵向入口打通、阶段验收三类节点发送，不逐工具调用播报。
- 审查建议按等级处理：
  - P0：权限越界、数据丢失、凭据泄漏、默认入口不可用——本阶段修；
  - P1：本阶段验收契约错误——本阶段修；
  - P2/P3：非本阶段、优化、额外压力覆盖——进入 backlog，不递归展开。
- 不执行破坏性 Git 命令；不创建 commit，除非用户明确要求。

### 0.4 当前 dirty worktree 规则

第 128–129 轮成果仍在同一 dirty worktree。任何 worker 必须：

- 只编辑分配到的目录；
- 保留用户与先前轮次改动；
- 不运行 `git reset/clean/checkout --`；
- 不机械格式化整仓；
- 主代理在宏阶段开始/结束各做一次 `git diff --check`。

---

# 宏阶段 1：Rust 产品入口与静态 Profile Boot（已封板：54% → 63%）

> 最终证据：workspace 1826 passed / 0 failed / 1 ignored，384 组；独立 P0/P1 复核通过。计划目标 65% 未机械记满，动态 `!!js`/HMR 与浏览器级 GUI 留待后续。

## 阶段目标

让用户可从真正的 `dsh` Rust CLI 启动 shipped `web` / `headless` profile，而不是直接调用测试用 `compose_host()`。实现静态 shipped profile registry、分层 patch、真实 Host readiness、Web dist 托管和可等待的 async shutdown。

## 阶段真实入口

```bash
cargo run -p dsh-host-cli --bin dsh -- web
cargo run -p dsh-host-cli --bin dsh -- --profile headless "run one task"
cargo run -p dsh-host-cli --bin dsh -- --profile web --dump-config
```

`web` 必须启动真实 HTTP Host、提供 `/api/host.describe` 和 SPA index；`headless` 必须创建真实 Agent、完成一轮、flush session、打印assistant输出并退出。

## 并行工作流

### 工作流 1A：CLI 与 runProfile

**目标：** 把 `DshInvocation::Profile` 从错误分支变成真实启动入口。

**主要文件：**

- 修改：`crates/host/dsh-cli/Cargo.toml`
- 修改：`crates/host/dsh-cli/src/main.rs`
- 修改：`crates/host/dsh-cli/src/lib.rs`
- 修改：`crates/host/dsh-cli/src/profile_boot.rs`
- 新建：`crates/host/dsh-cli/src/run_profile.rs`
- 新建：`crates/host/dsh-cli/tests/profile_run.rs`

**任务：**

1. 增加 `[[bin]] name = "dsh"`，保留 crate 名 `dsh-host-cli`。
2. 定义 `RunProfileRequest / RunProfileHandle / ProfileSurface`。
3. 解析并传递 launcher inner args、`DSH_HOME`、telemetry、patch overlays。
4. 实现 `run_profile()`：prepare → compose → resolve shipped profile → boot → readiness。
5. 实现 SIGINT/SIGTERM 的有界 async shutdown，不调用强制 `process::exit` 直到收敛超时。
6. `plugin` forwarding 独立实现为 `Command` argv 直传；禁止 shell 拼接。

**聚焦验证：**

```bash
cargo test -p dsh-host-cli
cargo test -p dsh-app-boot
```

### 工作流 1B：静态 shipped bundle/profile registry

**目标：** 用 Rust 静态安装器承载 shipped profile；自定义动态 npm/`!!js` row 暂时 fail-loud，不静默跳过。

**主要文件：**

- 修改：`crates/boot/app-boot/src/lib.rs`
- 修改：`crates/vendor/loader/src/*`
- 修改：`crates/vendor/include/src/*`
- 新建：`crates/boot/app-boot/src/shipped_registry.rs`
- 新建：`crates/boot/app-boot/tests/shipped_profiles.rs`
- 修改：`Cargo.toml`（只由主代理修改）

**任务：**

1. 定义 shipped row id → Rust installer 的静态 registry。
2. 把 base/headless/web profile 的当前可用插件映射到 registry。
3. 保留 bundle → profile → home → CLI overlay → telemetry/preset 顺序。
4. 未注册动态包、`!!js`、HMR-only row 返回带row id的明确错误。
5. `--dump-config` 只组合/渲染，不执行installer。
6. 增加 profile manifest 与 patch fixture，验证 shipped行、disabled、isolate、排序和错误。

### 工作流 1C：Host handle、Web surface 与 async shutdown

**目标：** 解决现有 `HostSpine::drop` 在后台writer仍活跃时直接删目录的风险，形成产品级Host生命周期。

**主要文件：**

- 修改：`crates/host/dsh-host/src/lib.rs`
- 修改：`crates/host/dsh-host/src/main.rs`
- 修改：`crates/host/dsh-host/tests/boot.rs`
- 修改：`crates/host/webserver/src/*`
- 修改：`crates/host/frontend-static/src/*`
- 新建：`crates/host/dsh-host/tests/shutdown.rs`

**任务：**

1. 把 `HostSpine` 改为显式 `HostHandle::shutdown().await`。
2. shutdown顺序：停止接收 → dispose fibers → cancel/drain Agent/Goal/jobs → flush sessions → drain JSONL/SQLite/search → 关闭server/sockets → 删除临时root。
3. `Drop` 只发诊断，不直接删除仍在使用的数据目录。
4. 托管 `web/dist`，保持Windows路径遍历拒绝、HEAD、MIME和SPA fallback。
5. index tap注入 `window.__DSH_BOOT__` 与可信Host信息。
6. readiness返回真实bound address/port，支持port 0测试。

### 工作流 1E：DeepSeek生产LLM adapter

**目标：** 让默认 `deepseek-official` 路由拥有真实HTTP/SSE生产实现；缺凭据、HTTP错误、截断流与取消均fail-loud，不以scripted adapter冒充产品能力。

**主要创建路径：**

- `crates/llm/llm-deepseek/`

**任务：**

1. 移植DeepSeek chat-completions请求序列化、text/reasoning/tool-call/usage翻译与严格`[DONE]` SSE边界。
2. 每请求快照base URL、key reference与模型配置；Bearer key不得进入日志或错误文本。
3. 注册`deepseek-official`、V4 Flash/Pro目录、context/maxTokens/reasoning元数据。
4. 用loopback fake HTTP服务验证真实网络请求、并行tool call、HTTP错误、取消与截断。
5. shipped profile安装adapter；无`DEEPSEEK_API_KEY`时首个模型请求返回明确credential错误。

**聚焦验证：**

```bash
cargo test -p dsh-llm-deepseek
cargo clippy -p dsh-llm-deepseek --all-targets --no-deps -- -D warnings
```

### 工作流 1D：产品级E2E

**目标：** 用真实子进程而非进程内helper验收CLI。

**新建：**

- `crates/host/dsh-cli/tests/web_profile_process.rs`
- `crates/host/dsh-cli/tests/headless_profile_process.rs`
- `crates/host/dsh-cli/tests/plugin_forwarding.rs`

**验收场景：**

1. 临时 `DSH_HOME` 初始化web profile。
2. spawn `dsh web`，读取readiness，HTTP请求`/`与`/api/host.describe`。
3. 发SIGINT，确认0/约定退出码、session已flush、临时目录不被重新创建。
4. scripted adapter驱动headless task并打印最后assistant文本。
5. unsupported dynamic row fail-loud且不启动半个Host。

## 阶段 1 统一验收

```bash
cargo test -p dsh-host-cli -p dsh-app-boot -p dsh-host -p dsh-host-webserver -p dsh-host-frontend-static
cargo clippy -p dsh-host-cli -p dsh-app-boot -p dsh-host --all-targets --no-deps -- -D warnings
cargo test --workspace
```

**完成定义：** 用户通过Rust `dsh`命令启动web/headless并可干净关闭；不是“compose函数测试通过”。

---

# 宏阶段 2：本地执行、PTY 与平台安全（已封板：63% → 75%）

> 最终证据：workspace 1851 passed / 0 failed / 1 ignored，401 组；目标 crate
> strict Clippy、定向 rustfmt/diff-check 和独立 P0/P1 复核通过。Windows 实际隔离
> 采用 AppContainer + 临时 ACL package SID；WRITE_RESTRICTED token 因同时限制
> .NET 启动所需注册表访问而不作为生产 runner。

## 阶段目标

让production profile中的模型真正获得受控 foreground/background shell、持久PTY、job工具和平台sandbox；重点完成当前Windows主机的PowerShell/ConPTY/ACL路径，并提供Linux/macOS runner契约。

## 阶段真实入口

从真实Agent/Host会话执行：

1. foreground pwsh/bash；
2. background进程→job_output/job_kill；
3. open/send/read/signal/kill persistent terminal；
4. read-only写拒绝；workspace-write只写工作区/tmp；danger-full-access按策略放行；
5. Host shutdown后无残留子进程或PTY。

## 并行工作流

### 工作流 2A：sandbox-local 与三平台runner

**主要创建路径：**

- `crates/sandbox/sandbox-local/`
- `crates/sandbox/sandbox-windows-acl/`
- `native/landlock-run/` 或 `crates/sandbox/landlock-run/`

**修改：**

- `crates/sandbox/sandbox/`
- `crates/sandbox/sandbox-policy/`
- `Cargo.toml` target-specific dependencies

**任务：**

1. Linux：bwrap→landlock探针链，失败fail-closed；保持exit 125契约。
2. macOS：Seatbelt profile生成与`/dev/null`/workspace roots一致性。
3. Windows：AppContainer + package SID + ACL grant 生命周期。
4. 每个Win32调用失败都带API名/error code；绝不无限制回退。
5. 平台capability报告`full/partial/unavailable`。

### 工作流 2B：subprocess PTY 与 terminal backend

**主要文件：**

- 修改：`crates/subprocess/subprocess-local/src/terminal.rs`
- 修改：`crates/subprocess/subprocess-local/src/process_inspector.rs`
- 新建：`crates/terminal/terminal-bash/`
- 新建：`crates/terminal/tool-terminal/`

**任务：**

1. Windows ConPTY、POSIX PTY真实后端。
2. send/read offset、foreground process、signal、kill、wait reason。
3. owner隔离、并发send互斥、spawn失败回滚。
4. terminal session与jobs/Host shutdown联动。

### 工作流 2C：shell/pwsh与模型工具

**主要创建路径：**

- `crates/shell/shell-env/`
- `crates/shell/bash-sandbox/`
- `crates/shell/pwsh-local/`
- `crates/shell/pwsh-sandbox/`
- `crates/shell/tool-bash/`
- `crates/shell/tool-pwsh/`
- `crates/shell/tool-bash-persistent/`

**修改：**

- `crates/shell/bash-local/`
- `crates/jobs/*`
- Host shipped registry/profile composition

**任务：**

1. Bash/Pwsh resolve/run/start一致语义。
2. 敏感环境与`DSH_*` scrub/managed env。
3. background job注册、完成通知、读取与kill。
4. sandbox escalation审批与denial marker。
5. 注册生产工具和system-prompt段。

### 工作流 2D：真实平台E2E

**测试：**

- Windows：ConPTY、pwsh encoding、restricted token、ACL边界、进程树kill。
- Linux：bwrap/landlock capability probe；无能力环境必须明确skip原因或fail-closed。
- macOS：Seatbelt profile golden + CI runner实际执行。
- 共通：Host/profile启动后由scripted agent调用工具。

## 阶段 2 统一验收

```bash
cargo test -p dsh-subprocess-local -p dsh-terminal -p dsh-terminal-bash -p dsh-tool-terminal
cargo test -p dsh-shell -p dsh-bash-local -p dsh-pwsh-local -p dsh-jobs -p dsh-tool-jobs
cargo test -p dsh-sandbox -p dsh-sandbox-policy -p dsh-sandbox-local
cargo test --workspace
```

**完成定义：** production profile里真正执行受控命令/PTY，并在权限和shutdown边界上有OS级证据。

---

# 宏阶段 3：外部协议与可扩展生态（已封板，约 86%）

## 阶段目标

一次性完成MCP、LSP、ACP、stdio SDK和出进程subagent的真实互操作，让Rust Host能消费外部工具/语言服务器并被Python/ACP客户端驱动。

## 并行工作流

### 工作流 3A：MCP client

**创建：**

- `crates/mcp/mcp-client/`
- `crates/mcp/mcp-test-server/`（dev/test-only）

**能力：** stdio + streamable HTTP、reconnect、tool schema转换、`mcp__server__tool`命名、取消/teardown。

**验收：** fixture server注册工具，真实Agent调用并收到结果；server重启后受控重连。

### 工作流 3B：LSP client与tool

**创建：**

- `crates/lsp/lsp/`
- `crates/lsp/lsp-stdio/`
- `crates/lsp/tool-lsp/`
- `crates/lsp/lsp-test-server/`（dev/test-only）

**能力：** Content-Length framing、initialize/shutdown、UTF-8/UTF-16协商、workspace进程池、transient-open、4个规范化操作。

**验收：** fake server + 可选真实typescript/rust analyzer fixture，定位/hover结果与golden一致。

### 工作流 3C：ACP服务端与stdio JSON-RPC SDK

**创建：**

- `crates/acp/acp/`
- `crates/sdk/jsonrpc-runtime/`
- `crates/sdk/jsonrpc-client/`
- `apps/jsonrpc-agent/`

**修改：** 顶层 `python/sdk` 与 `python/sdk-runtime` 的Rust二进制发现/打包。

**能力：** initialize/newSession/prompt/cancel、NDJSON framing、approval、agent idle结算、streaming assistant文本、错误码。

**验收：** 现有Python SDK启动Rust runtime并完成真实turn；ACP fixture客户端完成会话和取消。

### 工作流 3D：出进程subagent providers

**创建：**

- `crates/subagent/subagent-acp/`
- `crates/subagent/subagent-dsh-sdk/`
- `crates/subagent/subagent-codex/`
- `crates/subagent/subagent-claude-code/`

**修改：**

- `crates/subagent/subagent/`
- `crates/subagent/tool-subagent/`
- Host/profile static registry

**能力：** provider capability、one-shot/continuable、interrupt、output schema、外部进程teardown。第三方binary缺失时报告unavailable，不伪造成功。

## 阶段 3 统一验收

```bash
cargo test -p dsh-mcp-client -p dsh-lsp -p dsh-lsp-stdio -p dsh-tool-lsp
cargo test -p dsh-acp -p dsh-sdk-jsonrpc-runtime -p dsh-subagent
python -m pytest python/sdk/tests
cargo test --workspace
```

**完成定义：** 外部MCP/LSP/ACP/Python SDK与至少一个出进程subagent通过真实协议fixture互操作，不是只定义Rust trait。

**最终证据：** 428 个 test-result 分组，1885 passed / 0 failed / 1 ignored；
`dsh-rs-macro3-workspace-final-v3.log` SHA-256
`46799e45386e6aacbe8e08fe1928d8d0eefc14d9f7fe6c94147adcfdf3442e58`；
已知P1短复核 `passed: true`、无P0/P1。

---

# 宏阶段 4：JS动态运行时、Conformance 与默认切换（87% → 100%）

## 阶段目标

解决剩余最大结构性阻塞：模型编写的JS/TS workflow、code runtime、动态Cordis插件和loader `!!js`；随后完成golden fixture、跨平台CI、GUI验收和Rust默认切换。

## 决策门 4A：JS runtime spike（阶段内，不单独记百分比）

**候选：** `deno_core`、Boa、隔离Node-compatible sidecar。

**必须运行同一组fixture：**

1. async/Promise；
2. ES module与受控import；
3. TypeScript type stripping；
4. host async function bridge；
5. structured clone / JSON realm物化；
6. infinite loop可中止；
7. wall/compute/memory/output budget；
8. Windows/Linux/macOS构建与启动成本。

**决策规则：**

- 能满足1:1 JS语义和资源中止者优先；
- 若纯Rust引擎不满足，不用Rust DSL假装1:1，选隔离sidecar并把协议固定；
- spike只保留选择结果、fixture与决策文档，删除失败原型。

## 并行工作流

### 工作流 4B：code-runtime worker

**创建：**

- `crates/code-runtime/code-runtime-worker-thread/`
- `crates/js-runtime/`（若采用共享嵌入式引擎）

**能力：** fresh realm、binding bridge、compute/wall/output/memory预算、同步死循环终止、错误物化。

### 工作流 4C：workflow与ralph

**创建：**

- `crates/workflow/workflow/`
- `crates/workflow/workflow-worker-thread/`
- `crates/workflow/tool-workflow/`
- `crates/workflow/tool-ralph/`

**能力：** `agent/parallel/pipeline/phase/log/args`、outputSchema、fatality、agent-start/end配对、cancel与worker death账本、Ralph fixed script。

### 工作流 4D：动态Cordis与loader/HMR

**创建：**

- `crates/extensions/cordis-host-runner/`
- `crates/extensions/tool-cordis/`

**修改：**

- `crates/vendor/loader/`
- `crates/vendor/include/`
- `crates/vendor/hmr/`
- static profile registry

**能力：** `!!js`、动态host plugin定义/运行/停止、审批、inspect、配置/HMR重建。客户端React半边不移植，由现有Web dist继续承担。

### 工作流 4E：Conformance、GUI、CI、release cutover

**创建/修改：**

- `tests/conformance/`
- `tests/fixtures/{wire,session,storage,profile,protocol}/`
- `.github/workflows/` 或当前CI目录
- `web/`浏览器验收脚本
- Rust `dsh` release packaging
- `PORTING.md`

**任务：**

1. TS/Rust differential fixture：wire、session、storage、profile composition、MCP/LSP/ACP。
2. 导入剩余上游后端测试，明确不适用的浏览器-only项。
3. Windows/Linux/macOS CI与平台capability matrix。
4. Playwright/浏览器级现有`web/dist`验收。
5. 长时稳定性、资源泄漏、shutdown、恢复、性能基线。
6. Python runtime binary打包；Rust `dsh`成为默认入口，TS Host保留参考/fallback。
7. 关闭所有“1:1完成声明”前的P0/P1；P2偏差写入compatibility document。

## 阶段 4 统一验收

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings   # 先清理/批准既有baseline后再启用
python -m pytest python/sdk/tests
# platform CI: windows-latest, ubuntu-latest, macos-latest
# browser E2E against Rust dsh web
```

**完成定义：** Rust默认二进制覆盖shipped profile、动态workflow/plugin、平台执行和外部协议；golden/conformance/CI/GUI均通过后，才将完成度记为100%。

---

# 5. 阶段内并行与合并规则

## 5.1 Worker分配

- 每个worker只拥有一个目录族，例如 `crates/mcp/*` 或 `crates/lsp/*`。
- 主代理独占：根 `Cargo.toml`、`Cargo.lock`、`PORTING.md`、shipped profile registry、Host composition。
- worker返回：修改文件清单、聚焦测试命令、真实结果、剩余阻塞；不返回长过程日志。
- 同一共享文件不得被两个worker同时修改。

## 5.2 集成顺序

```text
工作流独立GREEN
  → 主代理合并依赖与Host/profile接线
  → 真实入口E2E
  → 相关crate全包
  → workspace第一次
  → 只修失败项
  → workspace最终快照
  → 一次限时审查
  → 更新PORTING与百分比
```

## 5.3 测试失败策略

- 先判断：本阶段回归 / 既有flake / 环境能力缺失。
- 本阶段回归：写最小RED、修复、只跑该RED和相关crate。
- 既有flake：隔离复现并记录；只在最终workspace再跑一次，不循环全仓。
- 环境能力：capability probe + 明确skip/fail-closed；不得伪造输出。
- 同一问题连续3次修复失败：停止补丁叠加，重新审视架构并向用户汇报。

---

# 6. 计划执行时的首个动作

下次开始执行时，不再回到第129轮继续加契约。按以下顺序：

1. 用现有 v7 证据更新 `PORTING.md`，把第129轮作为约54%的冻结基线；
2. 建立阶段1的验收fixture和CLI子进程RED；
3. 并行启动 1A（CLI/runProfile）、1B（静态registry）、1C（Host shutdown/Web）；
4. 主代理只在三路聚焦GREEN后做Host/profile集成；
5. 阶段1完整验收后一次性报告约65%，而不是每完成一个函数更新0.x%。

---

# 7. 明确不再采用的低效模式

- 不再把一个上游包的几十个测试逐条作为独立“轮次”。
- 不再每修一条并发边界就运行workspace和双审查。
- 不再因为审查提出跨阶段建议而无限递归扩展当前阶段。
- 不再用测试数量、LOC、勾选率或轮次数量夸大进度。
- 不在同一宏阶段反复重写最终日志版本v1/v2/v3…；最多“发现问题快照 + 最终快照”两份。
- 不为等待一个卡住的reviewer消耗无界时间；超时即记录并用既有测试/主审查收口。

该计划以真实可运行产品入口为推进单位，目标是在维持诚实验收的前提下，把每次可见进度从约1个百分点提高到约10–13个百分点。
