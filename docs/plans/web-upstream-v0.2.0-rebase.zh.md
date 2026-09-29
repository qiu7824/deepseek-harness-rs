# Web 换装上游 v0.2.0-rc.1 客户端：清单与分阶段计划

状态核对：2026-09-29。上游基线 [dsh-v0.2.0-rc.1](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.2.0-rc.1)（2026-09-28）。路线已确定为 **Web 整体换成上游 0.2.0 客户端**，Flutter 逐项补齐。

## 构建链路（已完成）

`tools/build_upstream_web.py` 把解压后的上游源码复制到仓库外（默认 `../upstream/<tag>`），提交为本地 Git 快照（客户端构建会读取 `git rev-parse HEAD`），用 `packageManager` 固定的 pnpm（经 corepack）安装，依次编译 Host 面（生成 `lib/typert.remote-client.d.ts`）、客户端库和 Web 外壳。Electron 桌面包不构建。

```bash
python tools/build_upstream_web.py --source D:/deepseek-harness-node-v0.2.0-rc.1
```

输出：Web 外壳在 `<work>/apps/web/dist`，各客户端插件在对应包的 `lib/client*.js`；Remote 契约清单写入 [`docs/upstream/remote-contract-dsh-v0.2.0-rc.1.json`](../upstream/remote-contract-dsh-v0.2.0-rc.1.json)。

## 关键发现

1. **Rust 宿主没有 Typert 网关。** 现有 `/api` 使用旧一代点号方法表（`POST /api/session.list`，`rpc_map.rs` 共 103 个方法），`/api/<ns>/<method>` 只是把 `/` 换成 `.` 查旧表；没有 `/api/remote.mux`。0.2.0 客户端的会话、工作区、设置、凭据、终端、文件、作业、账号等主干都走 Typert Remote。
2. **客户端模块系统换代。** 0.2.0 由宿主按启用的客户端插件组装启动图，以 `window.__DSH_BOOT__` 注入 `index.html`，并在 `/plugins/<id>/…` 提供合并脚本（combo）与拆分块。Rust 宿主目前提供的是旧式 `web/dist/plugins/*.js` + `manifest.json`。
3. **我们的 Web 是 8 月的 fork。** `web/dist` 为当时的预编译产物，`web/src/runtime-plugins` 中 35 个文件是在编译产物上直接修改的覆盖版本，无法随上游重建。
4. 连接信封兼容：`client-request {rpcId, method, payload}` → `server-response {rpcId, result: {ok, value | error}}` 与现有实现同形，Typert 载荷为 `{args}`，错误码为 `gateway/*`。

## Remote 契约清单（138 个端点，12 个流，29 个命名空间）

“已有”指 Rust 旧方法表中存在同名方法，仍需按 Typert 参数（如 `agentId`、`sessionId` 查找与冷恢复）和返回形状适配；其余为新增。

| 命名空间 | 端点 | 已有 | 新增端点 |
|---|---:|---:|---|
| `account` | 11 | 0 | ackBonusNotified、cancelSignIn、getBalance、getProfile、getState、getUnnotifiedBonuses、hasRunningAccountTasks、signOut、startSignIn、watch（流）、watchExpiry（流） |
| `agentPresets` | 3 | 3 | — |
| `commands` | 2 | 2 | — |
| `credentials` | 3 | 3 | — |
| `directoryPicker` | 3 | 0 | createDirectory、list、pick |
| `dynamicCordisRunner` | 12 | 0 | getClientCode、inventory、invoke、reportClientGuardFailure、reportRenderFailure、resolveInspectQuery、resolveRequestRun、runHostHalf、settleUserRun、stopFromPanel、syncInspectManifest、undefineFromPanel |
| `fileReferences` | 1 | 0 | list |
| `fileUploads` | 1 | 0 | upload |
| `goals` | 7 | 6 | get |
| `job` | 3 | 0 | follow（流）、kill、list（流） |
| `llm` | 3 | 1 | listConfigurableProviders、listProviders |
| `messageFeedback` | 3 | 3 | — |
| `officeToPdf` | 2 | 0 | generation、render |
| `permissionPresets` | 1 | 0 | catalog |
| `pluginInventory` | 1 | 1 | — |
| `pluginManager` | 12 | 0 | cancelInstall、inspect、installBundle、listBundles、listPlugins、listVersionExemptions、registries、removeBundle、setBundleEnabled、setPluginEnabled、setVersionExemption、waitForInstall |
| `pluginRegistryProbe` | 1 | 0 | fastest |
| `productAnalytics` | 3 | 0 | enabled、report、watchPolicy（流） |
| `schedule` | 5 | 5 | — |
| `session` | 19 | 10 | canOpenWorkspacePath、control（流）、follow（流）、initializeDefaultModel、modelCatalog、openWorkspacePath、page、projections、workspacePathApplications |
| `sessionFeedback` | 1 | 1 | — |
| `sessionReferenceResolver` | 1 | 0 | candidates |
| `settings` | 5 | 4 | openSettingsDocument |
| `skills` | 1 | 1 | — |
| `speech` | 6 | 0 | cancelPreparation、catalog、configure、follow（流）、prepare、transcribe |
| `subagents` | 2 | 1 | interruptByParent |
| `terminal` | 10 | 0 | close、create、environment、follow（流）、list、rename、resize、retain（流）、shells、write |
| `workspace` | 11 | 7 | follow（流）、initializeDefault、pinSession、unpinSession |
| `workspaceFiles` | 5 | 0 | changes（流）、list、read、readBytes、stat |

## 分阶段计划与验收

| 阶段 | 内容 | 验收 |
|---|---|---|
| A 构建与清单 | 构建脚本、契约清单、本计划 | 已完成 |
| B 网关内核 | 一元调用分派（`args` 精确字段校验、`gateway/*` 错误码、`agentId`/`sessionId` 查找与冷恢复、取消）；`/api/remote.mux` WebSocket（open/item/end/cancel、error 帧、2 秒心跳、每流 256 KiB 上行缓冲、重复 open 关闭连接）；`$events` 转发流与 `ready` 帧 | 以上游网关协议行为为准的 Rust 测试：未知端点、字段缺失／多余、查找失败、取消、上行溢出、`end` 后 item、断线重连 |
| C 客户端模块服务 | 从构建产物读取各插件 `dsh.client` 声明，组装启动图并注入 `window.__DSH_BOOT__`；提供 `/plugins` 下的 combo 与拆分块、source map；插件启停更新启动图 | 新 Web 外壳在 Rust 宿主上完成启动，所有插件无加载失败 |
| D 主干命名空间 | `session`、`workspace`、`settings`、`credentials`、`llm`、`agentPresets`、`goals`、`commands`、`schedule`、`subagents`、`skills`、反馈、`pluginInventory`、`permissionPresets`、`fileReferences`、`sessionReferenceResolver`、`directoryPicker` | 新建会话、发送、流式、取消、历史翻页、冷恢复、分叉、归档、模型切换在新 Web 上通过 |
| E 新功能命名空间 | `workspaceFiles`、`terminal`、`job`、`officeToPdf`、`account`、`fileUploads`、`pluginManager`/`pluginRegistryProbe`、`speech`、`dynamicCordisRunner`；`productAnalytics` 默认关闭 | 右侧边栏文件／终端／文档预览、后台作业、账号页、插件安装可用 |
| F 本地功能重建 | 以源码叠加层（overlay）方式重建 Rust 版独有插件：知识库、代码图谱、项目任务与小菜单、技能与 MCP、技能版本、工具发现、Windows 沙箱、目录与运行环境、文件操作、提醒与定时任务、时间上下文；逐项评估 35 个覆盖文件中的行为差异 | 构建脚本把 overlay 编入上游工作树；各功能 DOM 回归通过 |
| G 回归与切换 | 迁移 `tools/tests` 的 DOM 测试、e2e、发行打包；切换前保留旧 Web 作为回退 | 发行包安装后新 Web 全流程通过，旧会话可读 |
| H Flutter 补齐 | 工作过程展示档位、`@` 文件与会话引用、首次引导与账号页、会话置顶／复制 ID／字号、团队看板、macOS 语音及麦克风权限 | 原生组件测试与 Windows 实机核对 |

B、C 是后续所有阶段的前提；D 中 `session` 控制器（上游约 3,900 行，含 `follow`/`page`/`projections`/`control`）替换现有历史读取与 `events.mux`，是工作量和回归风险最大的一项。

## 风险

- 上游客户端对宿主语义的依赖不止于方法签名（如会话日志分页区间、重连补齐、冷恢复所有权），需以上游实现和测试为准逐项核对。
- 切换期间新旧两套 `/api` 并存，旧 Flutter 客户端继续使用点号方法表，不能因新网关而破坏。
- 上游客户端内含产品分析与官方品牌插件；Rust 发行默认关闭分析上报，品牌插件按发行配置决定。
