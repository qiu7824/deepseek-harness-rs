# 项目任务与记忆导入

## 项目任务

“项目任务”位于“产物”右侧，同一个工作区的对话共用根目录中的 `PROJECT_TASKS.md`。查看面板不会创建空文件，保存第一条任务时才建立清单。

任务包含待办、进行中、已完成、需更新、反馈五种状态，优先级为 P0 至 P3，P0 最高。反馈和验收说明保存在任务的说明字段中。已完成任务继续保留，便于追踪。

AI 可通过 `project_tasks` 工具读取和更新同一份清单。当前工作区的未完成任务摘要会按数量限制进入上下文，无需在每段回复中重复输出整个文件。完成状态应在实际验证后更新。

文件中的受管区域使用以下格式，区域外的项目说明会保留：

```markdown
<!-- dsh-project-tasks:start -->
- [ ] [P1] [todo] 完善导入校验 <!-- task:import-validation -->
  验收条件：重复导入不产生重复记忆。
<!-- dsh-project-tasks:end -->
```

浏览器保存使用文件摘要检查并发修改。文件被其他窗口或 AI 改动后，旧版本保存会被拒绝；编辑框保留草稿，可刷新后合并。无法识别的受管内容会明确报错，不直接覆盖。

## 本地记忆来源

“记忆与上下文”中的“从其他 Agent 导入”可检测下列本地文件：

| 来源 | 记忆或规则位置 |
| --- | --- |
| Codex | `~/.codex/memories/MEMORY.md`、`memory_summary.md` |
| Hermes | `~/.hermes/memories/MEMORY.md`、`USER.md` |
| Claude Code | `~/.claude/CLAUDE.md`、项目记忆目录中的 Markdown |
| Gemini CLI | `~/.gemini/GEMINI.md` |
| OpenClaw | `~/.openclaw/workspace/MEMORY.md`、`USER.md` |
| Windsurf | 全局规则及已登记工作区中的 `.windsurf/rules` |
| Cursor / Devin | 已登记工作区中的 `.cursor/rules`、`.devin/rules` |

检测结果表示发现了本地数据，不代表对应程序仍在运行。未识别到记忆文件的应用不会被当成可导入来源。不会扫描聊天数据库、登录配置或整段会话日志。

先选择来源并预览，再确认导入。分类采用本地规则，包含用户偏好、操作约束、已知错误、工具能力和项目知识；可在记忆列表中调整。常见凭据和私钥会被过滤，预览内容仍应由使用者核对。

自动同步为可选开关，每分钟检查已选择的来源。稳定标识用于去重；手动编辑或删除过的记忆不会被来源同步覆盖或重新创建。导入按批次原子保存，读取、条目数量和总内容大小都有上限。

全局记忆可供各工作区使用，也可选择工作区范围。记忆注入仍受上下文预算控制；导入不意味着所有内容都会在每次请求中完整载入。

## 小菜单设置

“设置 → 小菜单设置”控制轨迹（归集）、产物、代码图谱、上下文四个入口。开关会保存并立即生效；隐藏入口不会删除数据或停用工具。对话和项目任务保持可访问。

## 连接诊断

模型页提供 Codex 网络检查。检查只发送不带账号凭据的请求，用于区分网络无法连接和服务端已经响应；返回 HTTP 401、403 或 405 也说明请求已到达 HTTP 服务，不代表订阅权限已验证。

请求失败日志保留有限长度的底层错误链，用于区分连接、超时和 TLS 等原因。Windows 启动时，在没有显式代理环境或文件配置的情况下继承已启用的系统静态代理；显式配置仍优先，回环地址始终直连。

设计参考：[Backlog.md](https://github.com/MrLesk/Backlog.md)、[Hermes 记忆](https://hermes-agent.nousresearch.com/docs/user-guide/features/memory)、[Claude Code 记忆](https://code.claude.com/docs/en/memory)、[Gemini CLI 上下文](https://geminicli.com/docs/cli/gemini-md/)、[OpenClaw 记忆](https://docs.openclaw.ai/concepts/memory)。
