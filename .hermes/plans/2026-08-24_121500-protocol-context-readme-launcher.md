# 协议、超长上下文、开源文档与运行开关实施计划

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** 补齐官方OpenAI Responses及其它协议差异、将超长上下文快速跳转提取为插件、重写中英文开源README，并为Windows运行包提供简单可见的启动/停止开关。

**Architecture:** 先以Node仓库`origin/master`固定commit为语义权威，建立官方协议矩阵，再逐协议实现Rust适配器和真实Provider/fixture测试。超长上下文跳转只使用现有history分页、ConversationLocationIndex和RPC，不重复实现历史加载；UI作为独立纯Web插件加载到Profile运行目录。运行开关保持正式`dsh.exe web`入口，使用PowerShell 7 + WPF单文件管理器启动/停止/查看状态，不引入zsui、不修改Provider/模型/凭据。

**Tech Stack:** Rust 1.97.1、Tokio、reqwest、现有LLM adapters、Cordis Web slots、PowerShell 7 WPF、GitHub Actions Release。

---

## 现状与边界

### 已确认

- `crates/llm/llm-deepseek/src/responses.rs`已经存在Responses请求转换、SSE翻译及部分图片fallback测试，但入口仍处于DeepSeek适配器内部，不代表通用Provider矩阵完成。
- Node权威代码在`D:/HermesTemp/deepseek-harness`，只能通过`origin/master`对象只读审计。
- `web/dist/plugins/client-runtime.js`已有分页加载、`ConversationLocationIndex`、Turn/Step坐标和older-history接口。
- 当前未发现独立“超长上下文快速跳转”UI插件；已有产物跳转和工具位置视图不能替代Turn/Step跳转。
- 当前仓库有大量中英文示例README，但缺一份重新组织后的顶层开源README。
- 正式入口是`dsh.exe web`；Windows用户偏好PowerShell 7 + WPF单文件，不使用zsui。

### 明确不做

- 不修改真实Provider、API Key、默认模型、凭据引用或账户登录状态。
- 不把Node私有路径开放给sandbox。
- 不把语音、任务面板、AI画布重新塞回内置对话bundle。
- 不删除正式会话、artifact、用户未知文件或其他Profile数据。
- 不用HTTP 200、静态bundle或单元测试代替真实生产验收。

---

## 阶段一：建立官方协议差异矩阵

### Task 1：固定Node官方协议来源与清单

**Files:**
- Read only: `D:/HermesTemp/deepseek-harness` via `git show origin/master:<path>`
- Create: `docs/protocol-matrix.md`
- Test/utility: `tools/compare_provider_matrix.py`

**Step 1:** 从Node `origin/master`读取Provider package、adapter、README和测试中的协议名、请求路径、wire格式、stream事件、tool/image/reasoning/cache语义。

**Step 2:** 以表格记录：`openai-completions`、`openai-responses`、DeepSeek官方、Anthropic兼容、Gemini/Google兼容及Node实际存在的其它协议。只记录有Node证据的协议，未发现的写“未在权威commit中发现”。

**Step 3:** 对Rust当前代码逐项标记：已实现、部分实现、库级未组合、未实现、未验证。

**Acceptance:** 每个“已完成”单元格必须有Rust文件、测试和真实入口证据；不能把`responses.rs`存在等同于协议完成。

### Task 2：补齐OpenAI Responses入口

**Files:**
- Modify: `crates/llm/llm-deepseek/src/lib.rs`
- Modify: `crates/llm/llm-deepseek/src/responses.rs`
- Modify: provider selection/config files identified by matrix
- Tests: `crates/llm/llm-deepseek/tests/deepseek.rs`
- Create/modify: protocol fixture tests

**Step 1:** 写RED测试：Responses请求路径、system/input转换、assistant reasoning、tool call/result、image input、usage/cache、SSE terminal event。

**Step 2:** 实现显式协议选择和请求构建，保持Chat Completions与Responses不可混用；保留`Accept-Encoding: identity`、超时、取消和错误分类。

**Step 3:** 加入相同输入下Chat/Responses wire snapshot比较，只保存路径、字段、哈希和长度，不记录credential或完整敏感body。

**Step 4:** 运行：

```bash
cargo test -p dsh-llm-deepseek --all-targets
```

**Acceptance:** fixture和真实配置下均能收到首事件、文本/推理/tool事件和`turn/end: completed`；错误body不能再出现未分类decode失败。

### Task 3：逐协议处理其它官方支持项

**Files:** 由协议矩阵确定，不提前臆测。

每个协议重复TDD：请求fixture RED → adapter最小实现 → stream/tool/image/reasoning/cache测试 → Host组合 → 真实入口验收。若Node有而Rust暂不安全/不可兼容，写进README兼容矩阵和Release说明，不注册假工具或假Provider。

---

## 阶段二：超长上下文快速跳转插件

### Task 4：先确认现有分页与坐标RPC

**Files:**
- Read/modify if needed: `web/dist/plugins/client-runtime.js`
- Read/modify if needed: `crates/host/apiproxy/src/proxy.rs`
- Tests: `crates/host/dsh-cli/tests/web.rs` and client runtime fixture

**Step 1:** 写RED浏览器/fixture测试：打开超长会话，分页加载早期历史，索引Turn/Step，点击跳转后滚动位置落在目标消息。

**Step 2:** 复用已有`history`分页和`ConversationLocationIndex`；禁止重新保存全量raw history、禁止一次性加载所有超长事件。

**Step 3:** 定义插件slot：

```text
conversation.header.right 或 conversation.view
```

插件只接收session snapshot、location index、loadOlder、scrollToSeq等受控接口；没有文件、命令、网络或凭据权限。

**Step 4:** UI提供：Turn列表、Step列表、工具/产物位置、当前定位、高度过长时的搜索/筛选；点击目标时按需向前分页，分页完成后滚动到目标。

**Step 5:** 将插件放在`release/plugins/dsh-context-jump`，通过正式外部Web插件发现器加载；不要把UI重新编入`ui-conversation.js`。

**Acceptance:** 约2万事件冷会话下首屏不加载全量；从尾部跳到早期Turn、从早期跳到尾部、刷新、断线恢复均保持正确；内存和请求次数有账可记。

---

## 阶段三：顶层README中英文重写

### Task 5：重写中文README

**Files:**
- Rewrite: `README.zh.md`
- Review: `README.md`

内容结构：

1. 项目定位与Rust迁移边界；
2. 支持平台和Release下载；
3. Windows快速启动；
4. Web正式入口；
5. Profile、工作区、插件目录；
6. 原生Web插件安装命令和固定commit安全要求；
7. 协议支持矩阵；
8. 子智能体、工作流、终端、ACP、MCP、LSP真实状态；
9. 缓存、长流、超长上下文分页；
10. 构建与测试；
11. 已知限制；
12. 安全边界、凭据处理、插件同源权限说明。

禁止写入迁移对话、AI生成、用户纠正、过程叙述、虚假“全部完成”。

### Task 6：重写英文README

**Files:**
- Rewrite: `README.md`
- Cross-check: `README.zh.md`

英文与中文内容一一对应，命令、路径、Release tag、支持状态必须一致。通过脚本检查两份README都包含同一Release下载名、插件安全说明和Known limitations。

---

## 阶段四：Windows运行包开关

### Task 7：定义launcher进程契约

**Files:**
- Create: `tools/windows-launcher/dsh-service.ps1`
- Create: `tools/windows-launcher/dsh-service.json`
- Test: PowerShell smoke script

契约：

- 启动：`dsh.exe web --port 58080`，使用明确`DSH_HOME`；
- 停止：只停止由launcher记录的PID/命令，不广播杀进程；
- 状态：PID、端口、exe路径、版本、运行时间；
- 日志：独立launcher日志目录，不写credential；
- 重启：stop成功后再start，等待端口监听；
- 发布包：launcher与exe同级，资源路径按exe相邻布局解析。

### Task 8：WPF单文件中文开关

**Files:**
- Create: `tools/windows-launcher/DshServiceManager.ps1`
- Create: `tools/windows-launcher/DshServiceManager.lnk` only if reproducible and requested
- Docs: README Windows section

UI只提供：启动、停止、重启、状态、打开Web、复制地址、打开日志目录。不得增加Provider、模型、凭据或重复“工作目录”设置。

使用PowerShell 7 + WPF，隐藏控制台，后台启动；不能使用zsui。

### Task 9：真实运行包验收

**Steps:**

1. 解压完整Windows包到临时目录；
2. 启动WPF开关；
3. 验证PID和58080监听；
4. Chrome打开正式URL，验证RPC/WS和页面；
5. 点击停止，确认监听消失且只停止目标PID；
6. 再启动、重启并验证Profile恢复；
7. 删除临时解压目录，不动正式数据根。

---

## 阶段五：统一门禁与发布

### Task 10：阶段性测试

每个协议/插件/launcher阶段完成后集中运行：

```bash
cargo fmt --all -- --check
python tools/verify_product_surface.py
cargo test -p dsh-llm-deepseek --all-targets
cargo test -p dsh-host --test boot -- --test-threads=1
cargo test -p dsh-host-cli --test web -- --test-threads=1
cargo test -p dsh-mcp-client --all-targets
cargo test -p dsh-tool-lsp --all-targets
cargo test -p dsh-terminal --all-targets
cargo test -p dsh-subagent --all-targets
cargo test -p dsh-workflow-node --all-targets
```

ACP已知失败必须单独保留并在状态中标记，不可用其它测试覆盖。

### Task 11：Release资产检查

GitHub Actions固定到immutable action commit；每个归档只允许：

```text
dsh binary
web/dist
config/agent-presets
plugins
README.md / README.zh.md
LICENSE
PLUGIN_SECURITY.md
launcher assets on Windows
```

禁止包含：源码、测试目录、`.git`、临时日志、凭据、完整node_modules、snapshot和开发工具缓存。

### Task 12：最终发布

- commit前独立安全审查；
- 新tag，不移动或重写旧tag；
- Windows/Linux/macOS矩阵构建；
- 每个资产SHA-256；
- Release列出已完成、部分完成、未验证和已知阻断；
- Chrome真实验收使用最终Release，而非旧进程/旧SHA。

---

## 关键风险与决策

- OpenAI Responses不是简单改URL：必须验证request/input、reasoning、tool、image、usage和SSE事件全链路。
- 快速跳转不能通过一次性加载全历史解决，否则违反当前超长会话内存约束。
- Web插件与主页面同源执行，必须固定commit、审计权限并在README显式说明。
- WPF开关不替代正式`dsh.exe web`入口，只管理它；不能创建第二个服务或端口。
- ACP真实prompt/cancel当前失败，除非修复并完成同样的真实E2E，否则不能标记迁移全部完成。

## 规划阶段交付

本计划只更新规划，不执行上述实现。执行时按阶段推进，每完成约10%宏阶段后集中测试、修复、统一实装，再保留单一正式入口和最终Release证据。

**Plan path:** `.hermes/plans/2026-08-24_121500-protocol-context-readme-launcher.md`

**Next decision:** 是否按阶段一先实现Responses协议矩阵和Rust入口，再进入超长上下文插件与README/Windows开关。