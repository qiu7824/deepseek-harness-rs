# DeepSeek Harness v0.1.5-rc.1 的 Rust 适配与验收

基线：上游提交 `183f08e9c6dde7e36cd2318eaee70b0da08fb35e`；Rust Windows 维护版本 `0.1.3-alpha.11`。验证日期：2026-09-11。

## 上游增量

rc.1 的发布文案覆盖多个早期版本。相对 alpha.2，源码增量集中在新 Flash 模型、缺省选择、文档内部滚动与行定位、侧栏引导和统计显示。Session V3、MCP、进程管理等属于累计能力，不能重复计算为 rc.1 新实现。

依据：[发布说明](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.1.5-rc.1)、[模型目录](https://github.com/deepseek-ai/deepseek-harness/blob/183f08e9c6dde7e36cd2318eaee70b0da08fb35e/packages/llm/llm-deepseek/src/index.ts)、[基础缺省配置](https://github.com/deepseek-ai/deepseek-harness/blob/183f08e9c6dde7e36cd2318eaee70b0da08fb35e/packages/bundle/base/cordis.patch.yml)。

## 实现与验收

| 项目 | 当前行为 | 验证 |
| --- | --- | --- |
| 新 Flash 目录 | `deepseek-flash` / DeepSeek-V41-Flash，官方路由声明图片输入、1,000,000 token 上下文及 `in-history` 更新 | 配置与能力测试通过；实际安装服务返回完整元数据 |
| 旧配置升级 | 为官方路由补充新内置条目，保留旧模型、显式选择和用户字段偏好；第三方端点不套用官方能力 | 新旧目录、用户覆盖及第三方隔离测试通过 |
| 动态系统提示词 | 能力从模型解析传至已准备请求；支持路由追加变化，不支持路由整理首部系统消息 | 追加、替换、清空、新序列、旧日志恢复及压缩回归通过 |
| V3 系统统计 | 系统消息与普通历史分别计数，按表面位置处理替换和压缩 | 分类与反向序号范围回归通过；原始会话不改写 |
| 代码预览 | 复用 CodeMirror；工具栏独立，行号跳转、换行切换、横向滚动及阅读位置恢复 | 实际浏览器验证 2,400 行文件、第 1,500 行定位及切换恢复 |
| Markdown / HTML / PDF | 复用独立滚动容器；HTML 保持隔离；PDF 使用随附渲染资源并保留页码与缩放 | 真实浏览器渲染、PDF 翻页、缩放、切换和刷新恢复通过 |
| 文件交付 | `present` 工具及交付事件投影；产物按回合关联 | 工具、投影和前端回归通过 |
| minimal 预设 | 缺省工具为原生 Shell；说明同步为 shell-only | 预设及发布契约检查；既有用户预设保持独立 |
| 工具指导 | 指导与实际父子作用域和可见工具一致 | 工具指导回归通过 |
| 暂停目标 | Web 暂停立即取消当前执行；明确暂停后模型不能自行 resume | 目标状态及模型恢复策略回归通过 |
| 全局面板与反馈 | 保留 Rust 工作台扩展；全局面板入口和反馈明细接入 | 全局面板、状态隔离及反馈回归通过 |
| 模型管理 | API 与账号分区、单连接管理、搜索、筛选、批量可见性及草稿保护 | React DOM 与实际界面检查通过 |

图片与工具调用协议使用适配器 fixture 覆盖；没有以真实新 Flash 推理调用替代这项代码验收，也没有宣称已经验证所有第三方服务。UU 新客户端的实际连接、截图、断开另见[执行与升级核查](execution-and-upgrade-audit.zh.md)。

## 数据与产物一致性

运行时源模块、dist 与 manifest 哈希同步；CodeMirror、PDF 随附资源及 vendor-lock 一致。安装前后核对原始会话文件、账号凭据、设置与运行目录配置。Windows 优化构建和宿主执行验收针对同一程序哈希完成。

会话迁移支持已有 Rust V0→V3 路径，包含读取、追加、恢复、fork、压缩及原文件保护。官方 V0/V1/V2/V3 的显式导入、原件保留和图片恢复见[会话与文件能力](session-files-teams-and-network.zh.md)。

## 后续独立项目

通用附件上传、共享代理策略、显式启用的 Agent Teams 与按能力声明启用的 Responses Lite 已接入，配置和边界见[能力说明](session-files-teams-and-network.zh.md)。跨平台包由发布工作流分别构建和验收，真实账号及解锁后的 UU 操作由使用环境验收。

Windows 本机维护结果不替代 Linux/macOS 启动和发布包回归，也不构成对未知供应商协议或未来 ABI 的无条件兼容保证。
