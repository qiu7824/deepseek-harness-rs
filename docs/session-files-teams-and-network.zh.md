# 会话、附件、团队与网络配置

## 会话归档和删除

归档只改变所选会话的列表状态，历史记录保留。永久删除会先停止主会话及其子智能体，再按由深到浅的顺序删除各自的会话日志、会话目录和列表引用。停止或写入失败会返回错误，保留尚未删除的祖先记录，便于从原入口重试。

删除范围沿 `origin: subagent` 的父子关系确定。用户创建的独立分支保留；工作区内的上传文件、代码和交付成果保留。全局共享的内容寻址附件存储具有独立生命周期，不等同于某一个会话目录。

删除过程与新消息准入、冷恢复和空闲退役协调，避免删除期间重新启动会话或由迟到写入重建日志。循环父子关系在删除前拒绝。

## 通用附件

输入框支持选择文件、拖放和粘贴文件。图片继续使用图像附件通道，其他文件以原始字节保存到接收会话工作区的 `.dsh-attachments` 目录。模型获得文件名称、准确路径和字节数，可调用已有文件工具处理内容。

每条消息最多包含 16 个普通文件，单文件上限为 16 MiB，普通文件合计上限为 64 MiB；图片沿用部署的图像限制。普通文件可以为空。文件名不得包含路径分隔符、控制字符或系统保留名称。

目录按会话和内容摘要隔离，同名不同内容具有不同路径。保存采用不覆盖的原子发布；重试复用相同内容，遇到内容冲突返回错误。提交失败保留输入草稿，切换会话不会串用附件。发送后的文件卡片使用准确路径打开，原始消息内容保持完整。

## Agent Teams

团队功能需显式启用，修改后重启 Host：

```json
{
  "agent-teams": {
    "enabled": true,
    "maxMembers": 8
  }
}
```

模型工具 `agent_team` 提供以下操作：

| 操作 | 行为 |
|---|---|
| `status` | 读取成员、运行状态、任务版本和待投递数量 |
| `create` | 负责人创建具名成员，使用 `fresh` 或 `fork` 上下文 |
| `message` | 向负责人或同团队成员投递消息，返回消息标识与投递状态 |
| `task` | 创建或更新共享任务，必须提供 `expectedRevision`；0 表示创建 |
| `interrupt` | 负责人中断指定成员当前执行 |

只有用户明确要求组建团队或使用队友时，模型才应创建成员。主会话是负责人，成员是其直接、可继续的子智能体。普通子任务不会自动成为成员；用户分支拥有独立团队身份，不接管继承日志中的团队。

成员名称在首次登记时保留，创建失败后也不能复用。创建意图先写入日志，随后启动预留身份的子智能体；恢复时核对真实父子关系及初始消息，无法证明准入成功时记录失败。

团队消息先进入持久队列，目标记录消息后才确认投递。重试使用同一 `messageId`；同一标识不能改换发送者、接收者或内容。一个接收者之前的消息尚未投递时，后续消息保持顺序等待。消息来源记录团队、发送者和消息身份，不冒充人类消息。

任务使用单调递增版本阻止陈旧写入。成员只能为自己领取任务，不能改写其他成员已领取的任务；负责人可调整分配。依赖关系必须存在且不能形成循环，前置任务完成后才可开始。释放任务时将 `owner` 设为 null，并将状态改为 `pending`。任务分配与 `writeScopes` 用于协作，不提供文件系统锁。

会话标题栏中的“团队”面板显示成员和共享任务，可打开成员会话；功能关闭时不显示该入口。主会话永久删除时，团队成员遵循同一子智能体清理规则。

团队模型参考上游的[持久团队设计](https://github.com/deepseek-ai/deepseek-harness/blob/fb2c4b9e698e30edb738bca4cf0618587db7d203/.agents/notes/implemented/feature/2026-08-05-agent-teams.md)，Rust 使用独立的工具入口和显式启用配置。

## 官方历史导入

```text
dsh history import <源目录或导出.zip> --to <目标 DSH_HOME>
```

导入支持已发布 V0、V1、V2、V3 的 JSONL 与 Zstandard 日志，以及包含日志和图片的导出 ZIP。旧消息身份、系统提示、局部序号引用和继承边界经过转换；V2/V3 的内嵌响应流保留。输出使用当前 Rust 日志格式，并通过会话恢复与消息结构验证。

所有日志先完成预检，再发布到目标目录。源文件保持原样，每个导入会话保留 `import-source.original`。已有不同内容的会话、跨目录身份冲突、未知必需事件、无效版本及不完整记录会被拒绝；相同输入可重试。

ZIP 中的图片按日志引用的内容摘要验证并恢复，不按任意归档路径解压。导入源目录时可提供完整源 Home 的附件对象；缺少图片时，需要包含图片的导出 ZIP 或目标中已有的同一对象。单日志解码上限为 64 MiB，单批次最多 1000 个会话、256 MiB 输入，ZIP 最多 4096 个条目。

源格式依据：[V0→V1](https://github.com/deepseek-ai/deepseek-harness/blob/fb2c4b9e698e30edb738bca4cf0618587db7d203/packages/session/session-format-v0-to-v1/src/migration.ts)、[V1→V2](https://github.com/deepseek-ai/deepseek-harness/blob/fb2c4b9e698e30edb738bca4cf0618587db7d203/packages/session/session-format-v1-to-v2/src/migration.ts)、[V2→V3](https://github.com/deepseek-ai/deepseek-harness/blob/fb2c4b9e698e30edb738bca4cf0618587db7d203/packages/session/session-format-v2-to-v3/src/migration.ts)。

## 出站代理

Host 的模型推理、模型发现、账号服务、模型文件上传、搜索、公开网页读取、HTTP MCP 和反馈投递共用出站代理策略。UU 控制器的 HTTP 请求使用同一实现；受控浏览器的本机 CDP 连接始终直连。

读取 `HTTP_PROXY`、`HTTPS_PROXY`、`ALL_PROXY`、`NO_PROXY` 及其小写形式。进程环境覆盖启动目录 `.env`，启动目录覆盖 Home `.env`；同层小写值优先，显式空值保留其覆盖作用。策略在启动时固定。

代理地址支持 HTTP 和 HTTPS。`ALL_PROXY` 用作未指定协议的后备；`NO_PROXY` 使用主机、域名后缀、IP、CIDR 和 `*` 绕过规则。回环地址与 localhost 始终绕过代理。无效代理地址返回不含凭据的错误，SOCKS 地址不被静默当作直连。

公开网页读取继续验证 URL、目的地址和重定向，因此目标域名仍需能通过本地 DNS 验证；直接请求固定已验证地址。使用代理时，代理自身的后续解析和转发由代理配置决定。代理凭据不会通过该策略额外注入模型代码执行环境。

## Responses Lite

模型目录明确返回 `use_responses_lite` 时，能力保留到模型配置；也可在支持该协议的 Responses 连接或模型中设置：

```json
{
  "compat": {
    "useResponsesLite": true
  }
}
```

模型级设置优先，显式 false 可关闭连接级默认值。该选项只适用于 `openai-responses`，未声明能力的模型保持既有请求格式。

Lite 请求将工具定义放入 `additional_tools` 输入项，将系统指令作为 developer 历史项，保留指令变更顺序；移除顶层工具和指令，关闭并行工具调用，并使用 `reasoning.context: all_turns`。图像 detail 字段按协议移除，加密推理仍沿原有提供方、端点、模型和账号隔离规则重放。请求携带 Lite 协议标记，继续使用 Responses 流式结果处理。

公开 API 参考为 [Responses 请求](https://developers.openai.com/api/reference/cli/resources/responses/methods/create)；Lite 的额外约定按 Codex 客户端实现适配。能力声明不代表账号授权，也不保证服务端缓存命中。

## 资源版本与验证

`npm run build:runtime-plugins --prefix web` 同步前端 source、dist 和各模块摘要，同时将 manifest 总版本绑定到 Web 包版本。发布校验同时检查工作区、Web、安装器、manifest 和二进制身份，拒绝同版本号但来源提交不同的程序。

消息反馈的确认、撤回、分类持久化和文件交付展示见 [rc.2 适配验收](upstream-v0.1.5-rc.2-evaluation.zh.md)。真实账号切换、服务端缓存表现和解锁后的 UU 输入属于使用环境验收，不用模拟服务结果代替。
