# DeepSeek Harness Rust alpha.2 行为适配与验收

目标行为基线：DeepSeek Harness `dsh-v0.1.3-alpha.2`。实现版本：Rust `0.1.3-alpha.9`。Rust 保持独立的双向分页、有界历史、原生进程管理和发布版本。

官方范围见[发布说明](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.1.3-alpha.2)及[alpha.1 → alpha.2 比较](https://github.com/deepseek-ai/deepseek-harness/compare/dsh-v0.1.3-alpha.1...dsh-v0.1.3-alpha.2)。适配依据为实际行为、数据契约与正式入口。

## 实现清单

| 工作项 | Rust 实现 | 验收位置 |
|---|---|---|
| Web 持续恢复 | 最大退避只限制间隔；3 秒慢握手提示、15 秒硬截止；旧连接释放后重连，忽略旧代迟到事件 | `connection_controller_harness.js`、API gateway 生命周期回归 |
| 标准预设启动 | entry/fiber 归属在发布和激活前同步安装；standing 挂载 single-flight，失败驱逐，旧代清理使用身份比较 | Cordis `plugin_setup`、Loader `group_ownership`、实际 standard 配置的 24 路并发回归 |
| 普通进程生命周期 | Windows 挂起创建后绑定 Job，再恢复主线程；根进程结果与整棵执行树结束独立；Linux 探测 cgroup/systemd scope | `subprocess-local/tests/managed_lifecycle.rs`、Linux 编译与平台矩阵 |
| pid 契约 | 普通公共 handle 以受管生命周期为界；内部系统标识和终端前台进程组能力保留 | 消费者编译、取消和 Host 退出测试 |
| 子代理发送队列 | `requestId`、queue/steer、sending、去重、编辑、删除及身份校验接入真实 inbox | Agent/Subagent/API 测试、`e2e_queue_references.py` |
| 对话引用 | 原生候选菜单/chip、可恢复草稿、成功发送清理；与对应消息原子入队并在 claim 时注入 | `session_reference_input_harness.cjs`、真实队列引用 E2E |
| 旧 Windows 会话恢复 | 普通路径与系统规范路径仅在解析到同一真实目录时视为等价；保留工作区隔离 | `session_cwd_tests`、`e2e_session_workspace_resume.py` |
| 长引用补读 | 按模型能力计算预算，显式预算优先；同一序列生成预览和完整快照，提供真实 scratch 读取入口 | 10k Unicode 引用实存读回及源继续增长回归 |
| persona | prefix/suffix 与旧 persona/text 兼容，冲突诊断、complete、作用域覆盖；全局设置和预设入口共用解析器 | `system-prompt/tests/persona_layers.rs`、Host 设置回归 |
| 模型切换通知 | 下一次真正接纳请求记录一次持久通知；仅改变推理等级不产生模型切换通知 | 实际 AgentLoop 多回合及 Provider 失败重试 fixture |
| Astra/Responses | 完成即结算、静默连接可取消、传输失败受控恢复；保留原输出阶段和不透明重放状态 | `llm-deepseek` 单元及真实 HTTP fixture |
| Anthropic 重放 | 保留签名、redacted thinking、返回模型和原请求 alias；模型或端点切换不复用旧签名 | 工具往返及流完成/取消 HTTP fixture |
| 缓存计量 | 输入、缓存读、缓存写互斥记账；官方 Responses 路由使用稳定缓存键 | usage、端点隔离和重放回归 |
| PTC 展开 | 保留结构化 callView/resultView，嵌套调用可展开，长输出分段展示，未知退出码明确标示 | `ptc_tool_cards_dom_harness.cjs` |
| 滚动和上下文条 | 修正隐藏样式、迟挂载、页边界和订阅生命周期；固定底部立即结算，用户向上滚动释放跟随 | rail、conversation、history retention 回归 |
| 外部 Codex 完整过程 | 保留提前通知和半帧数据；公开摘要、消息、命令、文件及 MCP 结果进入独立步骤和持久子会话 | Wire、transcript、assistant steps 测试 |
| 默认读写工具 | standard/code/cordis 提供 read/write/edit；Headless 沿用配置的默认模型；minimal 保留显式旧编辑器 | `e2e_headless_tools.py` |
| 反馈日志 | put/delete 事件进入会话日志，兼容旧 sidecar；CAS、重启、fork 和生命周期隔离 | `message-feedback/tests/durable_log.rs` |
| 独立反馈提交 | 固定截取包、预览、本地保存；用户显式配置后提交，同身份重试，地址变更拒绝旧确认 | `feedback_delivery::tests`、反馈 UI harness |
| 在应用中打开 | Host 探测固定应用、原生菜单/语义图标、目录校验、参数数组启动；桌面程序独立于命令清理 | `open_in_app::tests`、workspace browser DOM 和平台入口 |
| 控件与资源 | 共享原生 Button/Modal/Switch/SecretField；runtime 源、dist、manifest 哈希同步 | 产品门禁、SettingsScope 和 UI harness |
| Provider 兼容字段 | 明确预算字段、vLLM priority、Responses 输出限制开关；协议不匹配或无法执行的配置显式拒绝 | `compat_tests.rs`、Host schema 验证 |

## 关键运行契约

### 连接、重试和收尾

持续重连不累积旧订阅。取消后停止分发缓冲帧，等待旧回调结束；迟到 ready、close 或错误不能修改新连接。

Responses 的 `response.completed` 和 Anthropic 的 `message_stop` 是完成边界。尾部 HTTP 断开不会把已完成请求重新判为失败。可恢复错误通过正式安装的重试策略处理，取消不被重试；失败原因保留底层错误链。

普通执行的 `done` 仅代表命令根进程产生结果，`wait_for_exit` 确认管理范围内进程结束。取消保留已收集输出，重复终止幂等，失败仍保留可重试归属。Linux 的进程组退路不能保证清理已经脱离进程组的后代，能力通过 `management_backend` 明确标示。

### 队列、引用与过程可见性

请求 ID、消息 ID 和队列项身份分别管理。前端回显与 Host 快照按请求身份合并，发送中禁止编辑/删除/插话，失败草稿可恢复。one-shot 只读边界和直接父子归属继续生效。

引用附加上下文绑定对应人类消息，在其进入真实回合时一起交付，避免污染正在执行的回合。编辑会重新解析引用，删除和 claim 清理驻留映射。截断快照包含固定序列、遗漏量、保存状态和读取提示；保存失败不会返回虚构路径。

外部子代理的过程消息、公开推理摘要和工具结果分别记录。回合结束以真实终止事件为准；一条中间说明不能充当最终完成。适配器私有 replay 只留在后台历史和原始导出中，不进入普通历史页、实时 mux 或引用文本。

### 模型与配置

| 配置面 | 字段 | 兼容规则 |
|---|---|---|
| system-prompt | personaPrefix / personaSuffix | 旧 persona 映射到 prefix；冲突拒绝 |
| persona 插件 | prefix / suffix | 旧 text 映射到 prefix；冲突拒绝 |
| complete | 完整覆盖 | prefix 作为完整系统提示，不追加其他段 |
| includeRuntimeContext | 运行环境说明 | 全局与预设作用域遵循相同开关 |
| 模型 provenance | requestedModel / responseModel | 返回模型只影响记录，不改变已选路由 |
| compat | thinkingTokenBudgetField / vllmPriority / supportsMaxOutputTokens | Provider 与模型层合并，按实际协议校验和执行 |

`thinkingTokenBudgetField` 支持 `thinking_token_budget`、`thinking_budget`、`thinking_budget_tokens`；旧 `supportsThinkingTokenBudget` 别名兼容。预算可按稳定推理等级配置。`supportsMaxOutputTokens` 仅控制 Responses 输出限制字段，Codex 订阅入口的专有约束继续校验。

缓存命中以供应商实际用量为准。稳定前缀、工具定义、输出阶段和重放内容影响复用；不通过增加无用提示或改写用量制造命中率。旧权威日志不被静默重写。

## 内存与性能

性能修复包括：

- 引用裁剪从反复全量序列化改为单遍字节计数与有界写入。
- 恢复消费已拥有事件，保留序列、envelope 和 surface 验证。
- 投影事件监听借用共享事件，避免整份深拷贝。
- 退休 checkpoint 不重新创建会话 cells；后台同会话只保留一个 writer 和最新待写 cut。
- 输入 shell 释放队列订阅，子代理等待后代使用通知而非空转。
- Wire 帧和待处理通知有总字节限制；浏览器不接收不透明 replay。

性能矩阵使用合法 Rust 事件、固定 1k/10k/100k fixture 和相同文件摘要，分别记录冷列表、热列表、历史页、恢复/退休、工作集与 private commit。密集未合并 fixture 属于兼容读取压力场景；它不能直接代表当前 writer 已合并文本块的最佳路径，也不能把活跃模型长上下文与空闲 Host 的内存混为一谈。

真实浏览器验收涵盖未加载节点定位、顶端向下滚动、侧栏调宽、引用发送、主/子队列、localhost 网页、反馈本地保存和应用菜单；DOM 单元测试单独列示。

## 存储和正式入口边界

Rust `SESSION_FORMAT_VERSION` 为 0，官方 alpha.1/alpha.2 为 2。投影缓存 v5 和会话日志版本是独立编号。既有 Rust 用户数据升级保持可读，未知权威格式明确拒绝。

以下内容属于独立范围：

- 官方 v2 文件导入需要专门迁移与回滚验证，未通过修改版本常量宣称兼容。
- Node SEA/bootstrap、Python wheel 和 JavaScript 专属所有权优化不直接移植。
- SDK/ACP 组件和 E2B 适配代码不等于完整正式产品入口；现有未完成入口仍明确其边界。
- 未配置真实接收端时反馈只保留本地状态，不显示远端提交成功。

## 免费模型与产物

`core`、`skin` 的编译、功能与安装门禁独立运行。`free` 必须同时具有当前精确模型的免费价格证据、匿名推理、工具往返和当前二进制校验和，才进入该平台产物集合。

供应商返回仅限 OpenCode 客户端使用时，Rust 标记匿名不可用。此类限制不会被身份伪装或付费账号回退掩盖。未通过的免费版不生成发布包，失败证据和平台选择报告保留供核验。

正式包校验前端哈希、版本、运行资源和 core/skin/free 差异；Windows 安装器检查实际内容与隔离安装行为，Linux/macOS 由平台矩阵编译、测试和包装。跨平台结果必须对应同一候选提交，交叉编译不能替代实机运行声明。

## 验收入口

- 核心：Cordis/Loader/agent-presets 初始化顺序；Agent/inbox、subagent、llm-retry、Responses/Anthropic、session/projection、引用和反馈持久化回归。
- Host：`e2e_queue_references.py`、`e2e_headless_tools.py`、模型与设置保留 E2E、工作区/设备设置、安装包内运行时验证。
- Web：connection、queue、reference input、rail、conversation、assistant steps、PTC、workspace browser、feedback submission、SettingsScope 与原有产品门禁。
- 性能：`memory_fixture.py`、`memory_scenarios.py` 及固定候选的真实浏览器验收。
- 桌面：独立 UU SDK H.264 传输、同源限制、控制权隔离、关闭/重连与输入队列；锁屏后的真实输入需要人工解锁再验证。

最终证据以发布工作流、候选产物校验和及版本说明为准。
