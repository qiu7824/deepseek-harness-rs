# Rust Harness 生产升级完整实施计划

> **For Hermes:** 按严格 TDD、真实生产入口和分阶段验收执行；不替换用户现有 Web，不修改 provider、凭据、默认模型或正式配置根。

**目标：** 在保留 Rust Harness 当前自定义 Web、插件与历史/内存优化的前提下，修复两个 P0 异常，完整移植审批安全盾、AtomCode AI 画谱，并差异吸收 DeepSeek Harness 最新能力，最终通过正式 `dsh.exe web --port 58080` 端到端验收。

**架构：** Rust Host 保持唯一运行时和持久化权威；Web 使用现有 `@deepseek-ai/dsh-client-web` 壳与插件模块，不整体替换。新版上游能力通过“协议域、运行时域、Web 插件域”垂直迁移；旧 ApiProxy 仅作为限期兼容桥，最终由 Connection/Remote 网关接管。安全策略在 Host 工具中间件执行，UI 只展示和收集决定，断线、超时、无人确认统一 fail-closed。

**技术栈：** Rust 2024、Tokio、Axum、Cordis、SQLite/JSONL/Zstd、TypeScript/React/Vite、Playwright、Windows 原生路径与进程接口。

---

## 0. 范围与不可破坏约束

### 保留

- 当前分支 `ai/porting` 和现有自定义 Web 结构。
- Rust 原生 Session/Persistence/分页/内存方案。
- `dsh-context-jump`、`dsh-better-sidebar`、`dsh-voice-input` 等现有运行时插件。
- 正式配置根、正式端口 58080、正式生产启动语义。
- 用户当前未提交修改：历史 forward window、JSONL/SQLite 跳转读取、`release/windows/启动DSH管理器.cmd` 删除状态。

### 禁止

- 不用最新版 Web 整体覆盖当前 Web。
- 不切换或写入 provider、凭据、默认模型、账号。
- 不以临时静态页、测试端口、isolated `DSH_HOME` 冒充正式验收。
- 不把安全判断只放在浏览器端。
- 不恢复 Node 作为正式 Host、持久化或插件依赖。
- 不直接翻译 Node/PTY 实现；移植行为不变量。

### 参考权威矩阵

| 来源 | 结构/交互 | 视觉 | 后端语义 | 用法 |
|---|---|---|---|---|
| 当前 Rust Harness Web | 权威 | 权威基线 | 当前生产事实 | 必须保留，不整体替换 |
| 上游 `dsh-v0.1.2-alpha.1` | 新能力与协议权威 | 仅差异参考 | Remote/Connection 等语义权威 | 逐功能移植 |
| `D:/deepwork/atomcode-main` | 审批、安全盾、AI画谱行为权威 | UI 可适配现有 Web | 安全状态机与图谱能力参考 | 完整移植行为，不复制架构 owner |
| dsh-market.com 前 8 皮肤 | 皮肤包行为参考 | 对各自皮肤权威 | 无 | 作为可切换主题包 |
| 当前两个默认主题 | 权威 | 权威 | 无 | 保留 |
| deepseek.com/harness/en | 无 | “官方 Harness”新增皮肤参考 | 无 | 新增独立皮肤，不改默认界面 |

---

## 1. 基线与工作区保护

**当前基线：** `7afe5379f24830b0ab90e9ebca993649162f2903`

**现有用户改动：**

- `crates/host/apiproxy/src/proxy.rs`
- `crates/session/session-persistence/src/index.rs`
- `crates/session/session-persistence/src/lib.rs`
- `crates/session/session-persistence-jsonl/src/index.rs`
- `crates/session/session-persistence-sqlite/src/index.rs`
- `release/windows/启动DSH管理器.cmd` 删除

**已验证：**

- `git diff --check` 通过。
- JSONL 新 forward-window 测试实际运行：1 passed。
- SQLite crate：2 passed。
- AtomCode Web UI：216 passed。
- Rust Web package 没有 `test` script，需补正式 Web 测试入口。

**实施要求：** 每阶段修改前重新执行 `git status --short`；不得 reset、checkout 或覆盖上述改动。

---

# 宏阶段 A：P0 异常关闭

## 2. 工作区地址乱码

### 目标链路

`Windows 目录选择器 UTF-16 → Rust PathBuf/UTF-8 JSON → workspace Remote/RPC → Web store → workspace row/hover/copy`

### 调查重点

- `crates/host/directory-picker-native/src/win32-dialog-*.rs`
- `crates/host/apiproxy/src/api/workspace.rs`
- `crates/host/apiproxy/src/proxy.rs`
- `crates/workspace/workspace/src/*`
- `web/dist/plugins/ui-workspace.js`
- `web/dist/plugins/connection.js`

### TDD

1. 新增包含中文、空格、Emoji、非 BMP 字符的 Windows 路径 fixture。
2. 验证 native picker 边界输出等于原始 Unicode。
3. 验证 Workspace API JSON 往返不转义错、不按本地代码页重解码。
4. 验证 Web workspace label、hover 全路径与复制值精确一致。
5. 正式浏览器选择真实中文目录后刷新和重启仍一致。

### 通过条件

- 地址显示、悬浮、复制、会话 cwd、重启恢复五处一致。
- 不以 `to_string_lossy()` 掩盖非法转换；Windows 原生边界显式 UTF-16 解码失败。

## 3. 停止操作刷新网页且无法停止

### 目标链路

`InputBar stop button → conversation.cancel → session.cancel → Agent::cancel → provider/tool/subprocess cancellation → Agent Idle → UI running cleared`

### 调查重点

- `web/dist/plugins/ui-conversation.js:243-247, 3673-3680, 3892-3930`
- `crates/host/apiproxy/src/proxy.rs:2932-3001`
- Agent cancel、when_idle、工具/终端 abort 和 WebSocket reconnect。

### TDD

1. Web DOM 测试：停止按钮始终 `type="button"`，点击不得触发 navigation/reload。
2. Connection 测试：Cancel RPC 失败只显示错误，不重建页面。
3. Host 测试：普通流式回答、工具调用、持久终端分别能从 cancel 到 Idle。
4. 竞态测试：连续点击、取消时切会话、RPC 迟到、WebSocket 断开。
5. 浏览器 E2E：监听 `framenavigated`、`beforeunload`、network，点击停止后 URL/document identity 不变。

### 通过条件

- 页面不刷新。
- 10 秒内进入 Idle；超时有明确错误，不假成功。
- Queue 保留，pending approval/question fail-closed。

---

# 宏阶段 B：审批与安全盾

## 4. 对话内审批卡片

### 行为模型

- `allow once`
- `allow always`（按稳定 grant key 持久化）
- `deny`
- 无确认、超时、断线、切会话、取消、Host 退出：按安全盾策略自动执行或自动拒绝；默认自动拒绝。

### Host

- 审批请求具有稳定 `rpcId`、`approvalId`、tool、风险类别、参数摘要、作用域、grant key。
- pending approval 生命周期绑定 owning Agent generation。
- 旧 generation 的回复拒绝。
- 超时或通道消失不能返回 Null 后继续执行。

### Web

- 审批卡片在对话轨迹中可恢复、可回答、已解决后显示结果。
- 不把完整敏感参数重复展示或写入日志。
- 支持安全盾默认策略说明。

### 测试

- 允许一次、永久允许、拒绝、超时、断线、重连恢复、取消竞态、迟到答复。
- 浏览器点击后验证真实工具是否执行/未执行，而非只看卡片状态。

## 5. 安全盾完整规则

### 从 AtomCode 移植的能力

- 通用 `ApprovalMiddleware`。
- Safe/Risky 工具分级。
- SensitivePathGate：`.env`、SSH、云凭据、密钥目录等敏感读取审批。
- WriteApprovalGate：工作区内、工作区外、敏感路径写入差异策略。
- BashWorkspaceGate：只读/破坏性、工作区内外、危险命令判断。
- Credential shell gate：凭据提取和外传硬阻断。
- 子代理敏感路径硬拒绝，禁止 AutoRespond 自批准。
- Plan/PTC/Hook 强制 ask 不得被后续 auto-approve 绕过。
- PolicyIntervention 安全恢复说明。

### 安全盾设置

建议设置项：

- 无人确认策略：`deny`（默认）/`allow-safe-only`/`allow-all`。
- 审批超时。
- 风险工具默认策略。
- 工作区外写入策略。
- 敏感读取策略。
- 凭据 shell 策略（严格/询问，非交互永远拒绝）。
- grant 管理与清空。

设置须走正式 Settings RPC、持久化、刷新与同根重启恢复。

---

# 宏阶段 C：AI 画谱

## 6. Rust 原生代码图谱后端

### 从 AtomCode 移植

- 多语言 tree-sitter 符号抽取。
- `list_symbols`
- `read_symbol`
- `find_references`
- `trace_callers`
- `trace_callees`
- `trace_chain`
- `blast_radius`
- `file_dependencies`
- gitignore-aware 文件遍历。
- 共享惰性 CodeIndex、文件指纹失效与重建。
- 大仓库索引阈值和明确降级。

### 目标模块

新建职责内聚的 Rust crate，例如：

- `crates/codeintel/codeintel`
- `crates/codeintel/tool-codeintel`
- `crates/codeintel/codeintel-tree-sitter`

不得把业务图谱塞入 kernel 或 Host 大文件。

### 测试

- Rust/TS/Python/Go/Java/C/C++ 等 fixture。
- 重名符号、递归、跨文件依赖、循环、忽略目录、同秒修改、删除/新增文件。
- 大仓库阈值不触发 O(repo) 阻塞。

## 7. AI 画谱 Web

当前 bundle 已有 `ui-code-graph.js`，先审计其真实 Host 数据源；补齐：

- 图谱构建状态、错误、取消。
- 节点/边搜索与筛选。
- callers/callees/path/impact 视图。
- 点击文件/符号打开真实位置。
- 大图虚拟化和布局取消。
- 工作区切换时按 workspace identity 隔离缓存。

正式验收：真实 Rust 仓库构图、选择符号、展开依赖、跳文件，并与工具结果一致。

---

# 宏阶段 D：皮肤系统与 Web 差异移植

## 8. 皮肤中心完全重做

### 皮肤清单

保留两个默认：

1. 当前默认亮色
2. 当前默认暗色

市场前 8 个预设（按 dsh-market.com 当前目录顺序）：

3. 鲸吟 `whale-song`
4. 蓝色幻想 `blue-fantasy`
5. 夕港 `harbor`
6. Windows XP Luna `xp`
7. 龙的传人 `dragon-heir`
8. Minecraft 方块世界 `minecraft`
9. 交易终端 `trading`
10. 初音未来 `miku`

新增：

11. 官方 Harness `official-harness`，视觉参考 `deepseek.com/harness/en`，但用于产品 UI 主题，不复制营销页面结构。

### 交互

- 设置页使用下拉切换，不用替换整页。
- 每项显示名称、缩略预览、作者/来源、亮暗能力。
- 主题热切换，不刷新页面。
- 当前皮肤和 light/dark mode 持久化。
- 背景媒体失败时回落到可读 token 主题。
- reduced-motion、低性能模式、移动端可用。

### 实现边界

- 使用现有 Theme tokens/Slots/插件生命周期。
- 市场皮肤作为本地可审计资源包，不运行未经审计的远程脚本。
- 皮肤 stop/update 必须完全释放样式、媒体、定时器。

## 9. 最新 Web 差异移植，不替换现有 Web

逐项选择性吸收：

- 初始加载与会话传输优化。
- `/`、`@` 菜单与标签改进。
- 问答草稿跨会话保留。
- 自动折叠过程、内容宽度、每轮用量、轮次导航。
- 流式代码高亮、中英文间距、问答卡片。
- 图片即时回显、Trajectory 图片、模型定位附件。
- 字号设置和配置扩展槽。

每项通过插件级 diff 移植，现有自定义 sidebar、跳转、语音和主题行为做回归测试。

---

# 宏阶段 E：最新版核心能力

## 10. 子代理模型

- 复用现有 Rust `AgentOptions` 和 `resolve_child_agent_options`。
- 补 provider/model/reasoning effort/max tokens 设置 UI。
- 显式模型 allowlist；未授权模型 fail-closed。
- Claude Code/Codex provider 配置映射。
- 保存草稿、策略-only 状态、父会话切换与重启恢复。
- 验证子代理真实请求 header 使用所选模型，不只验证 UI。

## 11. 协议封板

### 新架构

- Connection 拥有 transport、Fetch route、WebSocket generation、heartbeat。
- Remote namespaces 按 Host、Session、Settings、Credentials、Workspace、Subagent、Preset 等域注册。
- 一次性 Token 同时保护 HTTP Remote 和 WebSocket。
- 静态资源不要求 Token；业务 API 必须验证。
- 旧 `/api/<method>` 作为有期限兼容桥并记录消费者。

### 四态退出

1. 新 Remote 实现完成。
2. 当前 Web 与插件消费者切换。
3. 旧 ApiProxy bridge 仍保留并标记。
4. 所有消费者归零后删除旧 method map、handler、依赖和 fallback，才称协议封板。

### 验收

- URL Token 不落日志、Session 或设置。
- HTTP/WebSocket 未授权拒绝。
- reconnect 不重复事件、不恢复旧 generation。
- 设置/凭据/目录选择/会话导出均走新域。

## 12. 多模态与压缩

- 图片发送本地即时回显与 rpcId 去重。
- 图片附件持久化、Trajectory 展示、点击定位。
- DeepSeek Files API 复用保持。
- Token meter 新增 image tokens，按实际发送变体宽高与 provider/model policy 估算。
- Compaction 阈值和 breakdown 计入图片。
- 长图片压缩可取消、有耗时预算、不阻塞 Host。
- resume/fork/compaction 保留附件身份与 file-id/inline 语义。

## 13. 终端与 Headless

### 终端

- 进程表每轮一次快照。
- signal 绑定 process generation，防 PID 复用。
- Linux 管道读不误判为输入等待。
- Bash 大量子进程不阻塞 Host。
- PowerShell 启动探测跨平台稳健。
- Web 结果卡可展开。
- Dispose 仅终止 owned process tree。

### Headless

- 进度与工具状态写 stderr。
- 最终答案仅 stdout。
- JSON/ACP/SDK stdout 不混日志。
- Cancel/shutdown 后正确终态和退出码。

---

# 宏阶段 F：扩展生态

## 14. 插件设置扩展槽

- Provider card、settings footer、通用设置 section 注册 API。
- Host inventory 返回扩展声明、状态、错误。
- Client slot 注册受生命周期管理。
- 插件启停、更新、失败、刷新、重启恢复完整验收。
- 纯 Web 插件从 profile 运行目录加载，不固化到 Rust，不依赖 Node。

## 15. 第三方语言

- Locale service 支持第三方 namespace/dictionary 注册。
- 语言下拉动态列出内置与插件语言。
- 当前 locale 持久化；缺键按插件语言→基础语言→英文回退。
- 插件停用后字典卸载且 UI 不崩溃。
- HTML `lang`、日期、数字、相对时间同步更新。

## 16. DeepSeek 插件元数据

- 请求默认可附带启用插件包名与版本。
- 配置可关闭。
- 不包含配置值、本地路径、凭据或源码。
- 本地未知版本使用明确匿名/unknown 策略。
- 会话日志增量上传选项默认关闭，独立 consent、失败不影响主请求。
- 通过本地 mock provider 验证真实 request header/body，敏感字段扫描为零。

## 17. Inspector

- 共享 Inspector protocol。
- Host/Client producer。
- Runtime 经 CDP Worker 调试。
- Host Fetch 映射到 CDP Network。
- Cordis 服务/插件树映射到 CDP DOM。
- Console、response body、事件流回放、增量更新。
- Inspector 默认只绑定 loopback，并复用一次性 Token/开发授权。
- 不在生产默认 UI 暴露高风险控制面。

---

# 宏阶段 G：验证与发布

## 18. 分层门禁

每个 tracer bullet：

1. 写行为 RED。
2. 运行确认因缺陷/缺功能失败。
3. 最小实现 GREEN。
4. 目标 crate/package 测试。
5. 真实 Host composition 测试。
6. 浏览器 E2E。
7. 刷新与同根重启恢复。

## 19. 正式生产入口验收

最终只使用：

```text
dsh.exe web --port 58080
```

并使用原正式配置根。

验收用户旅程：

- 中文工作区路径选择、显示、复制、重启。
- 启动长回复/工具/终端并停止，页面不刷新、Agent Idle。
- 审批允许一次/永久/拒绝/超时自动策略。
- 安全盾敏感读取、写入、危险 Bash、凭据外传、子代理策略。
- AI 画谱真实仓库构图和跳转。
- 11 套主题下拉热切换和持久化。
- 新 Web 差异功能。
- 子代理指定模型真实请求。
- Remote Token 拒绝未授权连接。
- 图片上传、显示、压缩、恢复。
- Headless stdout/stderr。
- 插件设置槽、第三方语言、元数据、Inspector。

## 20. 压力、内存与回归

- 历史/跳转沿用指定真实问题会话，100–200 次压力。
- 停止/重启/切会话/审批断线循环 100 次。
- 主题切换 200 次，Style/DOM/媒体无线性增长。
- AI 图谱重复构建和 workspace 切换不保留旧图。
- 采样 Host Working Set/Private Bytes、浏览器 Heap/DOM、handles/threads。
- 静置常驻值不得随次数线性增长。

## 21. 最终质量门

- `cargo fmt --check`
- 受影响 crate tests
- 正式 Web tests/typecheck/build
- production Host E2E
- workspace tests
- `git diff --check`
- 最新代码独立 P0/P1 只读审查
- 最后一次改动后重新构建正式 Release 并重跑关键 E2E

---

## 阶段顺序与保守记账

| 阶段 | 内容 | 封板后累计目标 |
|---|---|---:|
| A | 两个 P0 bug | 10% |
| B | 审批与安全盾 | 22% |
| C | AI 画谱 | 34% |
| D | 皮肤与 Web 差异 | 48% |
| E1 | 子代理模型与协议 | 62% |
| E2 | 多模态、终端、Headless | 74% |
| F | 插件槽、语言、元数据、Inspector | 88% |
| G | 正式入口、压力、内存、Release | 100% |

比例只在对应生产用户旅程完成并通过当前代码证据后记账；测试数、LOC、子代理报告不计完成度。

## 当前唯一开工顺序

1. 保护现有历史分页改动并建立 Web 测试入口。
2. 关闭工作区乱码。
3. 关闭停止刷新/无法停止。
4. 再进入审批安全盾，不并行修改同一运行时生命周期。

该顺序用于避免在协议和安全大改前继续携带两个 P0 基础缺陷。