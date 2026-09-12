# DeepSeek Harness Rust：v0.1.5-rc.2 适配与验收

验收日期：2026-09-12。Rust 基线：`0.1.3-alpha.12`，基线提交 `8ffeb5735f7bc6bd762282e9bd7b93e3cd0161ef`。上游目标提交：`fb2c4b9e698e30edb738bca4cf0618587db7d203`。

## 1. 结论与范围

**反馈确认、纯撤回、失败草稿保留和消息评价分类持久化已完成；文件展示采用适合 Rust 现有文件行的紧凑布局、类型图标和完整列表。** 影响评价按钮与文件行显示的回合投影问题同时修复。

[上游发布说明](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.1.5-rc.2)与[标签比较](https://github.com/deepseek-ai/deepseek-harness/compare/dsh-v0.1.5-rc.1...dsh-v0.1.5-rc.2)对应反馈交互和交付文件展示两组变化。category 是上游已有字段，Rust 增加该可选字段属于兼容补齐；不涉及新的 Session 格式、模型适配器或进程运行时迁移。

| 项目 | 状态 | 行为 |
|---|---|---|
| 新评价与更换评价 | 完成 | 先确认，确认前不写入；取消保留原评价 |
| 撤回已有评价 | 完成 | 同方向点击只撤回，串行核对后失效的撤回不执行 put |
| 失败与重试 | 完成 | 草稿保留，错误可单独关闭；冲突后使用更新的版本重试 |
| 消息评价分类 | 完成 | 正负评价共用七类可选分类，贯通校验、返回、日志、投影和版本比较 |
| 文件展示 | 完成选择性适配 | 紧凑布局、类型图标、长名称截断、全部文件列表与完整路径 |
| 回合末尾动作显示 | 完成 | 保持活动回合和步骤的数据存储身份，避免评价和文件行丢失 |
| 新版本发布与安装 | 未执行 | 代码与隔离候选已验收，Rust 版本号保持不变 |

## 2. 反馈语义与持久化

[消息反馈界面与控制器](../web/src/runtime-plugins/ui-message-feedback.js)等待当前评价加载完成，再决定打开表单或撤回。`retract` 只删除匹配方向的当前评价；排队期间评价已改变或已经撤回时直接完成，不产生新评价。

表单绑定会话、消息和方向。更换方向从空说明、空分类开始，编辑时回填当前内容；空说明省略 note，未选择分类省略 category。保存中的重复点击只发起一次写入。失败保留草稿并恢复提交按钮，关闭错误提示不清空内容；关闭重开或切换消息、会话后，迟到响应不会关闭新表单。表单支持可访问名称、自动聚焦、Escape 关闭和焦点返回。

评价确认记录本地消息反馈，不启动模型请求。向接收端发送仍经过独立的[反馈投递流程](../crates/host/dsh-host/src/feedback_delivery.rs)，保留预览、显式发送和重试标识。

[消息反馈服务](../crates/feedback/message-feedback/src/lib.rs)在请求与记录中增加可选 `category`，复用会话反馈分类：`task-result`、`instruction-following`、`product-interaction`、`service-stability`、`resource-cost`、`security-privacy-permission`、`other`。

非法分类在写入前拒绝；仅分类变化也更新版本，相同方向、说明与分类保持幂等。旧记录缺少 category 时按无分类读取，无需批量迁移。日志恢复可重建分类；清除分类、删除评价继续使用版本比较，过期版本不能覆盖当前记录。[负反馈经验桥接](../crates/feedback/message-feedback/src/recorded.rs)继续仅在进入负面评价状态时记录经验，修改说明或分类不重复触发。

上游依据：[评价按钮](https://github.com/deepseek-ai/deepseek-harness/blob/fb2c4b9e698e30edb738bca4cf0618587db7d203/packages/client/ui-message-feedback/src/client/MessageFeedbackActions.tsx)、[控制器](https://github.com/deepseek-ai/deepseek-harness/blob/fb2c4b9e698e30edb738bca4cf0618587db7d203/packages/client/ui-message-feedback/src/client/controller.ts)、[弹窗状态](https://github.com/deepseek-ai/deepseek-harness/blob/fb2c4b9e698e30edb738bca4cf0618587db7d203/packages/client/ui-message-feedback/src/client/dialog.ts)。

## 3. 文件展示与回合末尾投影

[交付文件组件](../web/src/runtime-plugins/ui-deliverables.js)保留显式交付优先、无声明时回退自动产物、去重、回合隔离与历史关闭边界。文件行顶部间距为 4px，按钮间距为 6px，图标为 20px，文件名可收缩截断。同名文件按完整路径打开；折叠数量按钮可展开全部文件，列表显示完整路径并提供预览与文件操作入口。

图标使用自有固定 SVG 路径与文本，覆盖常见代码、文档、表格、图片和压缩文件，未知类型使用通用图标。SVG 不含共享 id、外部引用或动态注入内容，不引入上游第三方图标资产。实现保留 Rust 文件行结构，不宣称与 Node 卡片或品牌图标逐像素一致。

完整 Host 验收发现：步骤数据先于回合末尾数据发布时，`ConversationLocationIndex.replaceData()` 会删除暂时为空的存储，而时间线仍引用旧对象；后续末尾数据写入新对象，导致复制、评价和文件行无法显示。[客户端运行时](../web/src/runtime-plugins/client-runtime.js)现在保留活动回合与步骤的存储，窗口淘汰后仍释放失效存储。新增回归在修复前失败、修复后通过，完整页面也已验证动作恢复显示。

上游设计依据：[交付样式](https://github.com/deepseek-ai/deepseek-harness/blob/fb2c4b9e698e30edb738bca4cf0618587db7d203/packages/client/ui-deliverables/src/client/Deliverables.module.css)、[文件卡片](https://github.com/deepseek-ai/deepseek-harness/blob/fb2c4b9e698e30edb738bca4cf0618587db7d203/packages/client/ui-deliverables/src/client/PresentedFileCard.tsx)、[代码图标](https://github.com/deepseek-ai/deepseek-harness/blob/fb2c4b9e698e30edb738bca4cf0618587db7d203/packages/client/ui-primitives/src/CodeFileIcon.tsx)。

## 4. 验收结果

下列 JavaScript harness 位于 `tools/tests`；DOM 测试使用 React 19 与 JSDOM，依赖目录通过命令行参数或 `DSH_REACT_TEST_MODULES` 指定。

| 检查 | 结果与覆盖 |
|---|---|
| `cargo test --offline --locked --release -p dsh-message-feedback` | 5 项通过：1 项经验桥接单测、4 项持久化集成测试，包含旧记录、分类 CAS 与日志恢复 |
| `message_feedback_confirmation_harness.cjs` | 通过：确认前零写入、取消、草稿保留、冷加载与过期撤回、冲突重试、重复提交和迟到响应隔离 |
| `produced_files_presentation_harness.cjs` | 通过：图标、未知类型、同名路径、长名称、窄行计算、主题属性、完整列表及准确路径操作 |
| `turn_tail_location_harness.cjs` | 通过：分阶段发布保持存储身份，空中间态与窗口淘汰正确清理 |
| `session_feedback_dom_harness.cjs`、`feedback_submission_dom_harness.cjs` | 通过：会话反馈、独立投递、显式确认和重试隔离 |
| `directory_picker_flow_harness.cjs` | 通过：系统目录选择返回、工作区同步、取消与失败提示 |
| `explicit_deliverables_harness.cjs` | 通过：显式声明、旧产物回退、关闭边界、去重和回合隔离 |
| `history_transactions_harness.cjs`、`history_retention_harness.cjs` | 通过：12 项历史事务检查；85 页窗口轮换的有界保留 |
| `python -B -m unittest tools.tests.test_product_surface_contract tools.tests.test_release_product_contract tools.tests.test_release_integrity -q` | 58 项通过；发送状态检查与重连后重新读取版本的既有实现一致 |
| `cargo build --offline --locked --release -j4 -p dsh-host-cli --bin dsh` | 完整 CLI 候选构建通过；存在编译告警 |
| 完整 Host 与真实浏览器 | 隔离本地数据、模拟模型；验证确认前无评价、分类与说明保存、反向评价空草稿、Escape 取消、焦点返回、同向撤回、文件列表及准确路径预览 |
| 桌面与窄屏视觉 | 浅色与深色检查通过；390×844 窄屏无页面横向溢出，长中文名称和重复名称可辨认 |
| 资源与格式 | 三个变更模块 source / dist 一致，manifest 修订匹配 dist SHA-256 前 16 位；JS 语法、Rust 格式与 diff 空白检查通过 |

新增交互与回合末尾回归已接入[发布工作流](../.github/workflows/release.yml)。验证覆盖代码与隔离候选运行，未执行全 workspace 测试、真实模型请求、跨平台安装器验收或新版本发布；默认应用启动、失效文件与全部键盘菜单组合不在完整浏览器验收覆盖内。
