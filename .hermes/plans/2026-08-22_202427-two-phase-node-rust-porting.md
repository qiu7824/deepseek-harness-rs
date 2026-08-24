# Node → Rust Harness 两阶段完整迁移计划

> **For Hermes:** 按阶段执行，每个生产切片遵循 RED → GREEN → 相关回归 → 真实入口验收。

**目标：** 在不修改用户Provider、模型、凭据和配置根的前提下，使Rust Web Harness与Node rc.2生产语义一致，并用同一Provider、模型、会话条件和真实浏览器入口证明首Token、状态、持久化和功能行为。

**架构：** 先闭合模型输入边界，因为system prompt、工具schema、agent preset和请求前缀决定Provider缓存及首Token；再闭合其余运行时边界，包括图片/Files、生命周期、工具子进程、插件、设置和浏览器。Rust保持现有正式数据根，测试使用隔离根；Node仓库只读，不合并或覆盖脏工作树。

**技术栈：** Rust 1.97.1、dsh-host、dsh-agent-loop、dsh-system-prompt、dsh-tools、dsh-llm-deepseek、嵌入式Web插件、真实RPC和浏览器验收。

---

## 当前证据与已确认差异

最近同机实时请求已经证明：

| 指标 | Node DeepSeek | Rust DeepSeek |
|---|---:|---:|
| Provider/model | `deepseek-official/deepseek-v4-flash` | 相同 |
| Reasoning | `high` | `high` |
| system prompt | 11399字符 | 1232字符 |
| tools | 25 | 19 |
| cacheReadTokens | 10880 | 2944 |
| 首reasoning | 约501ms | 约16423ms |
| 代理路径 | 约149ms | curl表面测试不是主要瓶颈，但Rust手写CONNECT/TLS仍需独立证明 |

**后续实际传输探针修正了根因判断：** 在Rust探针版中，真实一次请求的边界为：

```text
request-send       0ms
TCP connected      0ms
HTTP/TLS handshake 16078ms
response headers   16180ms
first SSE data     16181ms
```

因此必须把“请求语义/缓存前缀差异”和“Rust传输层握手卡顿”拆成两个独立问题。缓存未命中解释请求语义不一致，但不能单独解释本次17秒；17秒直接发生在Rust手写CONNECT、rustls和Hyper握手路径。修复后必须重新测cacheRead和TTFT，不能用其中任一指标替代另一个。

**最新中间实现状态：** 已将DeepSeek传输尝试切换到reqwest，但取消/Drop回归仍未通过；该中间状态不得构建为正式Release。必须先恢复流取消的即时连接关闭语义，再进行真实远端探针。

**规划边界：** 本计划完成规划阶段；后续执行按两个阶段纵向推进。每个阶段先建立RED，再实现GREEN，再重建正式Release和真实入口验收。未通过的中间代码、旧端口通知和旧浏览器证据一律不计入完成。

**缓存迁移执行状态：** Rust生产Agent创建路径已接入Node同源preset mount事务，新建、冷恢复和fork统一解析并挂载实际preset；filesystem/search、web、compaction、subagent-control、pwsh、jobs、goal和subagent等正式插件已进入standard Agent scope。真实`session.create`门禁、Host完整33项、Agent Loop、ApiProxy、AgentPresets和Web三层门禁均GREEN；system、tools和DeepSeek请求字段已按Node `origin/master`复核。取消后的可见assistant前缀、连续多轮append-only wire、assistant reasoning/tool calls/tool results、稳定图片handle和嵌套tool-result图片已有GREEN回归。阶段一代码与生产组合门禁已闭合，仍需最终Release下用同一Provider/模型/档位完成首轮、连续多轮、同根重启和fork的`cacheReadTokens`/首reasoning/首text/完成时间复测后才能记为完成。

---

## 传输层专项验收（阶段一的前置门槛）

### T1. 复现与边界探针

**文件：** `crates/llm/llm-deepseek/src/transport.rs`、`crates/llm/llm-deepseek/tests/deepseek.rs`。

**探针字段：** `request-send`、`tcp-connected`、`connect-tunnel-complete`、`tls-complete`、`http-handshake`、`response-headers`、`first-sse-data`。只输出耗时和非敏感元数据，不输出URL中的凭据、Authorization、正文或消息。

**RED：** loopback长连接取消测试必须在消费者取消后于有界时间内观察到服务端EOF；真实探针必须记录握手超过基线的差异。

### T2. 替换HTTP/TLS实现

**范围：** DeepSeek Chat Completions和Responses请求；Files API单独保留其现有reqwest客户端。

**要求：** 使用成熟异步HTTP客户端处理代理、CONNECT、TLS、HTTP/1流式响应、NO_PROXY和连接超时；禁止代理TLS失败后静默直连；保留远程明文fail-closed；保留请求取消、Drop、超时、响应体上限和SSE逐帧语义。

### T3. 取消与所有权

**要求：** UI/Agent取消必须关闭请求worker、响应body和底层连接；取消signal必须贯穿`GenerateOptions.signal → drive_owned_request → request_chunks/request_responses_chunks → transport::post → reqwest::send/response.chunk`；首HTTP响应头之前、首SSE之后和Provider流结束后三种边界都必须可取消。不得只丢弃mpsc receiver或只abort消费者测试任务；测试必须真实设置取消signal并等待请求任务收敛。不得在Drop中无限等待未受控线程，使用可取消异步任务或可验证的RAII join边界。

**验收：** `dropping_pending_stream_closes_the_loopback_connection` GREEN；主请求完成、Provider错误、首Token前取消、首Token后取消、Host shutdown均通过。

### T4. 真实远端性能门禁

**同一条件记录：** request-start、request-body-ready、TCP、CONNECT、TLS、HTTP headers、首SSE、首reasoning、首text、finish、Idle。

**通过条件：** Rust握手不再出现约16秒；首SSE与响应头之间不出现异常空洞；修复后再比较Node/Rust请求body、cacheReadTokens和TTFT。无完整边界时间线不得宣称速度修复。

---

## 阶段一执行顺序修正

1. 先闭合T1–T3传输和取消门槛；
2. 删除或关闭临时探针，只保留正式可控诊断接口；
3. 完成Node/Rust request/header差分器；
4. 迁移完整system sections、Runtime Context顺序和preset上下文；
5. 补齐真实工具实现、schema、排序、权限和生产注册；
6. 对齐请求参数、消息前缀和缓存布局；
7. 用同一DeepSeek模型同条件复测首reasoning、首text、cacheRead和完成时间；
8. 真实浏览器发送、刷新、Host重启和Idle验收；
9. 阶段一只有全部门禁通过才标记完成。

## 阶段二执行顺序修正

1. Files失效file-id单次重试和精确索引失效；
2. Files并发winner/loser、远端清理、expiry和配额；
3. 统一图片投影、量化卸载、嵌套tool-result和Responses fallback；
4. 工具、子进程、LSP、终端、子Agent和取消/teardown差异；
5. 外部Cordis插件：实现隔离动态运行时，或正式收敛静态插件并修改UI诚实提示；
6. 设置、目录、任务、画布、TTS/STT和模型状态真实浏览器闭环；
7. 最终串行门禁、Release替换、稳定根重启、PID/RPC/bundle核对和临时物清理。

## 本轮禁止提前做的事

- 不用`cacheReadTokens=0`单独解释17秒；
- 不用最终文本或HTTP 200代替传输边界证据；
- 不把reqwest中间实现作为正式Release，直到取消回归通过；
- 不在Node/Rust请求未同条件时强行改用户Provider、模型或凭据；
- 不复用被停止端口、旧Release SHA或旧探针日志作为新证据。

---

## 已完成或部分完成：

- Rust真实Running → Idle桥和断线清理；
- 状态文案分为等待响应、推理中、生成回复；
- 目录Browse选择、持久化和同根重启后端链路；
- DeepSeek Files严格解析、multipart上传、索引、file-id请求和整请求inline回退；
- Attachment批量预验证、请求变体、预算、WebP alpha基础；
- Host请求桥容量300 MiB；
- 自动标题生产接入；
- TTS/STT异常复位、任务面板、AI画布基础；
- Rust静态插件加载和社区dsh-market隔离失败边界。

仍未闭合或未验证：

- standard的system sections与Node组合工具已进入真实Agent scope；最终Release下的真实Provider请求快照、字段顺序和缓存指标仍待捕获；
- Rust/Node standard preset、工作目录、Runtime Context顺序、工具集合和缓存前缀代码门禁已一致；仍待最终Release真实Provider缓存时间线；
- DeepSeek请求与流式响应迁移已通过除一项外的全套门禁；Windows上首响应前取消后物理TCP连接未在2秒内关闭，业务流会结束但传输driver teardown仍待闭合；
- Files失效file-id单次重试、进程内同variant单飞、winner/loser清理和同scope过期记录远端删除已完成并通过并发/DELETE成功与失败组合门禁；
- 嵌套tool-result图片、稳定request image handle、统一request variant、Responses fallback及Node等价数量/字节量化卸载已完成；
- 静态Rust插件已具备profile配置发现、inventory/status、启停、持久化和失败回滚；CLI正式进程已通过插件启停、设置、Provider、图片、会话持久化与同`DSH_HOME`重启闭环；任意外部JavaScript/ESM/npm插件动态加载与HMR仍不存在，未知插件会明确失败，最终Chrome/Edge交互验收仍待完成；
- Node/Rust同配置同会话浏览器端到端验收未闭合；
- 最终Release和稳定入口需在最后一次代码修改后重新构建验证。

---

# 阶段一：模型输入、请求缓存与首Token闭环

**阶段目标：** 对同一DeepSeek模型和同一用户输入，Rust与Node的`request/header`在生产语义上完整一致或有逐项记录的必要差异；Rust首reasoning/首text不再因遗漏输入上下文、工具schema、传输握手或多轮历史重排而显著慢于Node。阶段一必须同时闭合首轮稳定前缀缓存和后续多轮append-only对话缓存，不能只修第一轮“你好”。

### 1. 建立Node/Rust请求快照差分器

**文件/工具：**
- Node只读事件：`D:/HermesTemp/deepseek-harness` 的 `session.history`；
- Rust事件：`crates/host/apiproxy` session history；
- 新测试辅助：`crates/host/dsh-host/tests` 或 `crates/llm/llm-deepseek/tests`。

**验收：** 自动输出并比较 provider、model、reasoning、maxTokens、system hash/长度、工具名称/schema hash、消息数量/长度、cache read、首reasoning、首text、finish、Idle。禁止输出凭据和完整敏感上下文。

### 2. 迁移完整系统提示section

**重点文件：**
- Rust：`crates/host/dsh-host/src/lib.rs`、`crates/core/system-prompt/src/lib.rs`、`crates/core/agent-loop/src/agent.rs`；
- Node权威：`packages/boot/app-boot/src/index.ts`、`packages/core/system-prompt`、`packages/context/*`、`packages/preset/persona`、相关生产bundle。

**内容：**
- Harness身份、源码checkout与正式工作目录的区分；
- GUI/当前页面语义；
- Windows工具平台约束；
- 中文可见输出约束；
- Runtime Context、AGENTS/CLAUDE、Skill catalog顺序；
- agent preset人格、权限、工作目录和任务规则；
- 后台任务、目标、子Agent、文件、终端、网络和审批规则；
- section order、替换语义、重复装载和缓存稳定前缀。

**验收：** Rust system prompt长度、section顺序和关键稳定前缀与Node快照一致；动态cwd等变量只在动态尾部变化，不破坏可缓存前缀。

### 3. 补齐模型工具目录和schema

**重点文件：**
- Rust：`crates/core/tools`、`crates/host/dsh-host/src/lib.rs`、各`crates/*/tool-*`；
- Node权威：`packages/tools`、`packages/fs/tool-*`、`packages/interaction/*`、`packages/subagent/*`、`packages/web/*`、`packages/workflow/*`、`packages/todo/*`、`packages/skill/*`。

**至少核对：**
`ask_user_question`、`edit`、`glob`、`grep`、`interrupt_agent`、`list_agents`、`read`、`read_image`、`send_message`、`subagent_fork`、`todo_write`、`web_search`、`write`，以及Rust独有terminal工具的Node等价映射。

**验收：** 25工具名称、description、parameters schema、排序、权限和可用条件逐项差分；不能只补名称占位。

### 4. 修正Agent preset和请求组装

**重点文件：**
- `crates/core/agent-loop/src/agent.rs` 的 `pre_step`、`build_request`；
- `crates/core/agent/src/model_selection.rs`；
- `crates/host/apiproxy/src/proxy.rs` 的持久model selection；
- Node：`packages/preset/*`、`packages/core/agent-loop/src/agent.ts`。

**验收：** 新会话和恢复会话模型选择、preset、cwd一致；不把Node的`sol-max-engineer`强写到Rust用户配置；选择来源必须来自真实会话选择事件。

### 5. 修正请求参数与缓存前缀

**核对：** `stream_options.include_usage`、thinking、reasoning_effort、max_tokens、temperature、stop、消息角色、assistant reasoning passback、工具排序、动态system尾部。

**验收：** 同一DeepSeek请求Node/Rust cacheReadTokens显著接近；首reasoning/首text差异必须有时间线解释，不接受只看最终文本。

### 5A. 首轮稳定前缀缓存

**目标：** 首个用户消息之前的system、tools、preset和静态运行规则形成字节级稳定前缀；Host重启、随机Web端口变化、新会话ID和cwd变化不重写该稳定前缀。

**实现要求：**
- system sections按`order + stable name`确定性排序；
- 工具名称和canonical JSON schema确定性排序；
- 动态GUI URL、源码位置、cwd、session ID、时间、模型运行状态移到动态尾部；
- preset静态人格与动态选择来源分离；
- 不通过删除Node规则缩短前缀；
- 不伪造Provider usage或cacheReadTokens。

**回归：**
- 同配置两次组装的稳定前缀SHA相同；
- 仅端口、session ID或cwd变化时稳定前缀SHA不变；
- 工具schema真实变化时SHA必须变化；
- 同一Provider/model第二个新会话应出现真实缓存读取。

### 5B. 后续多轮对话缓存

**目标：** 多轮会话采用只追加历史语义。已发送的system、tools和历史messages不得因新回合、标题、投影或恢复而被重写；新用户消息只追加在尾部，使Provider能够复用之前对话前缀。

**Node权威语义：**
- user/assistant/tool角色与内容块顺序；
- assistant `reasoning_content`回放；
- tool_call ID、tool result关联和finish顺序；
- 图片file-id/inline表示在一次会话历史中的稳定性；
- compaction只在明确边界替换前缀，替换后建立新的稳定前缀；
- session title请求不得进入主对话messages或改写主请求前缀。

**实现要求：**
- 每个历史message进入请求前先canonical化一次并持久复用；
- 不在每轮重新生成旧消息ID、tool call ID、JSON属性顺序或图片data URL；
- 不把动态Token、运行状态、GUI端口、标题或时间写入历史messages；
- assistant reasoning和tool calls按Provider协议原样回放；
- 会话恢复、Host重启和fork不得重排既有messages；
- Compaction后只替换明确的压缩区间，后续继续append-only。

**多轮缓存门禁：**
1. 新会话第一轮“你好”记录冷请求；
2. 同会话第二轮追加“继续”，既有请求前缀SHA必须与第一轮一致；
3. 第三轮包含工具调用，旧message/tool前缀保持不变；
4. Host同根重启后继续对话，历史前缀SHA保持不变；
5. fork后父历史前缀保持一致，只在fork尾部产生分支；
6. Compaction前后分别验证旧前缀明确替换、新前缀随后稳定；
7. 实际比较每轮`cacheReadTokens`、uncached input、首reasoning和首text；
8. 不接受只看总Token或最终文本。

### 5C. 缓存可观测性

在`request/header`或专用诊断事件中记录非敏感摘要：稳定system SHA、tools SHA、history prefix SHA、动态尾部长度、消息数量、Provider usage。禁止记录API Key、Authorization和完整敏感正文。

### 6. 阶段一门禁

必须通过：

- Rust `dsh-llm-deepseek`全套；
- Agent Loop和system-prompt相关测试；
- Host生产组合测试；
- Node/Rust只读请求快照差分；
- 同一真实Provider/模型的“你好”首Token和完成时间；
- 真实浏览器模型选择、发送、reasoning、最终回复和Idle清理。

阶段一完成标准：当前Rust新会话DeepSeek请求不再出现system 1232/tools 19的明显缺口，且首Token慢点不再由Rust输入组装差异解释。

---

# 阶段二：其余生产运行时、插件、设置与封板

**阶段目标：** 完成Node rc.2其他生产语义迁移，并通过真实Web入口、RPC、持久化、重启和最终Release验收。

### 7. DeepSeek Files恢复和并发闭环

**重点文件：**
- `crates/llm/llm-deepseek/src/files_api.rs`；
- `crates/llm/llm-deepseek/src/upload_index.rs`；
- `crates/llm/llm-deepseek/src/lib.rs`；
- Node权威：`packages/llm/llm-deepseek/src/{adapter.ts,file-store.ts,files-api.ts,upload-index.ts}`。

**内容：**
- provider拒绝过期/无效file-id时精确失效mapping；
-只允许一次有界重试；
- 重试仍失败返回真实错误；
- 同一variant并发上传winner/loser；
- loser远端文件删除；
- expiry refresh、配额上限、远端孤儿清理；
- 索引格式版本和未来格式保护。

### 8. 统一图片管线与投影

**内容：**
-完整normalization、编码和alpha；
-嵌套tool-result图片；
-文本模型图片占位；
-按字节/数量量化卸载最老前缀；
-取消、失败、缓存损坏和大图边界；
-300 MiB桥和200 MiB aggregate admission一致。

**验收：** 原图、变体、Files、inline、Responses fallback、工具结果图片均有loopback和真实Provider证据。

### 9. 工具、子进程和生命周期完整迁移

**核对：**
- terminal/native-command stdout/stderr排空、取消和退出；
- LSP真实取消；
- subprocess-e2b、shell、workspace-write、sandbox；
- one-shot/continuable subagent、fork、interrupt、drain、child-first dispose；
- tool schema、权限、审批和错误投影。

**验收：** 每项必须有真实生产Host入口或明确记录为隔离命令，不以trait/API存在代替闭环。

### 10. 外部Cordis插件运行时

**现状：** Rust能读`plugins.json`，但只有静态builtin，无法解析`dshmarket`等npm/TypeScript插件。

**二选一但必须实现真实结果：**

- 实现隔离profile的npm包解析、安装审计、Host贡献、浏览器bundle、生命周期、卸载和重启恢复；
- 或正式限制为静态Rust插件，并在UI中诚实显示“外部Cordis插件不可用”，不保留误导性的可安装入口。

稳定产品根不得执行未经审计的第三方安装。

### 11. 设置、任务、画布、语音和目录真实浏览器闭环

**验收：**
- 工作目录和环境目录：选择、导航、当前目录回填、保存、刷新、同根重启；
- 数据目录只读且显示真实compose-time稳定根；
- 任务面板左侧入口、状态、水波纹、持久化；
- AI画布读取真实trajectory并持久化标记；
- STT仅写草稿、不自动发送；
- TTS播放/停止/异常复位；
- 模型选择、发送、Running、reasoning、text、finish、Idle；
- Host断线不会永久显示活动状态。

### 12. 最终封板

最后一次代码修改后串行执行：

1. `cargo fmt --all`；
2. 相关crate test和Host 31项生产门禁；
3. `cargo check`/目标平台构建；
4. `node --check`、manifest JSON解析、bundle SHA和rev检查；
5. Node/Rust请求快照差分；
6. 真实浏览器点击和刷新/重启；
7. Release构建和SHA；
8. 停止本轮启动的旧实例；
9. 稳定数据根启动；
10. HTTP、RPC、监听PID、bundle实际内容验证；
11. 删除本轮临时脚本、隔离profile、测试根和诊断日志；
12. 独立只读复核最新代码。

## 不能算完成的情况

- 只增加工具名或UI节点；
- 只通过trait、配置项或单元测试；
- 只看HTTP 200；
- 只看最终文本；
- 只比较模型名而不比较request/header；
- 只把Node system prompt删短来“对齐”；
- 用空配置、测试凭据或切换Provider绕过失败；
- 把Node脏工作树合并到Rust；
- 在最后一次修改后复用旧Release、旧浏览器或旧独立审查证据。

## 主要阻断

1. 阶段一system/tool/preset完整迁移未完成，这是当前首Token差异的主要可疑根因；
2. 外部Cordis插件运行时未实现；
3. Files失效ID、并发和配额恢复未闭合；
4. 真实浏览器Chrome/Node/Rust同条件验收尚未完整完成；
5. Node rc.2包含大量非Files模块版本与行为变更，需要按上表逐项验收，不能将431文件变更简单视为全部生产差异。

## 阶段完成记账

- 阶段一：只有请求header差分、完整system/tools/preset迁移、同模型首Token复测全部通过后才记为完成。
- 阶段二：只有Files恢复、图片投影、工具/子进程、插件边界、设置/浏览器和最终Release全部通过后才记为完成。
- 当前：阶段一未完成；阶段二仅部分切片完成；项目不封板。
