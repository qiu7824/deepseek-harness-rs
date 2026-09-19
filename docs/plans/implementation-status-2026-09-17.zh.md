# 统一方案实施状态复核

核对日期：2026-09-17。代码基线：`e7bafb32f0fecccb1fd6fb754c0b1e097a5b31fc`。

## 代码与远端状态

- 当前分支：`codex/agent-practical-reliability-20260916`。
- 核对开始时工作树干净。CLI 变更已包含在 a57df15012、ce7af255a8、e7bafb32f0 中。
- `git ls-remote` 确认 origin 上该分支与本地 HEAD 相同。
- 当前公开最新版本仍为 `v0.1.3-alpha.22-r3`，发布时间 2026-09-16 03:10:04 UTC；该版本早于本轮新增提交。分支推送不等于新版发布。

## 分项状态

| 范围 | 可证明的进展 | 尚不能证明的内容 |
|---|---|---|
| G01 执行可靠性 | 原生执行、步骤执行、脚本执行、错误分类和输出状态代码已提交；已有定向测试记录 | A01—A08 全部实际场景、安装版与全平台验收 |
| G02 工具发现与环境 | Host 已安装 discovery、environment_probe、execution_profiles 及对应接口 | 全部 UI 发布接线、单次配置、运行时精确授权和原始故障完全恢复 |
| G03/G04 任务验收与恢复 | task-runtime、TaskExecution、技能验证桥接已在 Host 接线 | 全部任务内容检查器、WPS 布局验收和崩溃副作用实机矩阵 |
| G05 真正远端执行 | remote-execution crate 与 HTTP 模块存在，定向测试已有记录 | Host 未声明 remote_execution_http 模块，未找到 RemoteRuntime/install_routes 接线；不能视为正式入口可用 |
| G06 应用权限 | 控制器权限接口和 computer_permissions.rs 存在 | Host 未声明或安装 computer_permissions 服务；设置管理及完整协议闭环未验收 |
| G07 评测 | 团队协议夹具、回归与 evaluation 代码存在 | 36 项验收证据矩阵、真实模型业务成功率、最终版本平台验收 |
| G08 技能版本 | SkillLifecycle、候选工具、验证桥接与设置源文件存在 | 新设置插件尚未进入正式 manifest；完整 UI/验证/恢复闭环未证明 |

## 前端发布接线

源码存在 `ui-settings-skill-revisions.js`、`ui-settings-tool-discovery.js`、`ui-task-execution.js`，但当前正式 `web/dist/plugins/manifest.json` 未找到这些插件入口。仅源文件通过语法检查不能证明安装后的界面可以使用。

## CLI 验收证据的实际覆盖

基础文档记录 `cargo check --workspace` 与执行相关定向测试通过，CLI 对话还报告 dsh-host 244 项通过、2 项忽略。历史结果可以作为线索，最终发布仍需要与构建身份关联的原始日志和对应范围核验。

`D:\codex操作目录\team-five-projects-20260917` 中五份 evidence.json 均为 passed=true、realModelCalls=0，检查字段均为 peerDelivery、taskCas、stableMessageRetry、originBoundary、restartRecovery、forkIsolation、treeDeletion。

这些证据支持五个隔离目录里的团队协议与生命周期用例通过，不证明五类业务项目已经开发完成，也不证明外部模型已完成各业务任务。

## 真实模型测试的错误通过标记

`D:\codex_ops\real-bilibili-team-20260917\evidence.json` 写有 passed=true，但同一文件显示：

- turns=0；
- tasks 为空；
- leadRunning=true；
- 实现成员 status=failed，错误为提供方 HTTP 400 / INVALID_REQUEST。

对应脚本仅以“团队存在成员”设置 passed，未验证业务任务、完成轮次或产物，因此此通过标记无效。原始证据保留；不能计入真实模型业务成功。

## 后续完成条件

1. 接通远端执行、应用权限和全部新增前端入口，检查配置与真实执行一致。
2. 补齐环境精确授权、一次性选择、图片/WPS 与任务验收残项。
3. 修正真实模型验收条件，核对 HTTP 400 的请求与提供方兼容性，完成真实多轮任务和独立产物验证。
4. 建立 A01—A36 的结果索引，逐项关联提交/二进制/平台；缺失或忽略不计通过。
5. 完成最终候选构建、跨平台 CI、发布标签、完整安装包与校验和回读后，才能报告最终发布完成。
