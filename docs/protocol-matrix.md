# 模型协议与账号支持

提供商通过设置中的 `llm-pi-ai` 接入 Rust 适配器；DeepSeek 原生提供商使用独立的 `llm-deepseek` 设置。协议类型决定请求和流式事件转换方式。

| 协议 | 实现范围 |
|---|---|
| OpenAI-compatible Chat Completions | 文本、图片、推理、流式工具调用、工具结果、usage 和错误处理 |
| OpenAI Responses | 输入与工具转换、reasoning 回放、SSE、图片、usage、完成与错误事件 |
| OpenAI Codex Responses | ChatGPT 设备码授权、令牌续期、账号路由和 Codex 请求约束 |
| Anthropic Messages | 文本、图片、工具调用及结果、thinking 与签名回放、流式文本和 JSON 参数、usage |
| Azure OpenAI Responses | 尚无独立的 Azure 认证与路由适配器 |
| AWS Bedrock / Google Vertex | 尚无独立 SDK 认证与协议适配器 |
| Copilot ACP | 不作为模型 API 协议提供，与 Copilot API 账号路由不同 |

## 账号登录

设置中的原生账号入口支持 GitHub Copilot、Qwen、MiniMax 国际、MiniMax 中国、Nous Portal、ChatGPT / Codex 和 xAI Grok。Claude Code 入口调用已安装的官方客户端进行登录和状态检查，供 Claude Code 子智能体使用，不导入订阅令牌。

原生设备码登录提供验证网址、验证码、轮询、取消和到期处理。凭据通过本机凭据服务保存，浏览器只接收登录状态和当前授权流程需要的信息。令牌到期时自动续期；授权失效时提示重新连接。退出登录清除本机账号授权。

账号调用取决于供应商账户的订阅、授权和模型权限，用户需在供应商页面完成本人授权。API Key 提供商使用独立的密钥设置。

账号流程参考 [Hermes 官方提供商文档](https://hermes-agent.nousresearch.com/docs/user-guide/features/fallback-providers) 与[官方源码](https://github.com/NousResearch/hermes-agent)。Hermes 的全部 API、云 SDK、进程代理与插件协议不等同于账号登录列表。

## 模型能力

模型目录可返回上下文容量、最大输出、输入模态和推理档位。提供商明确返回的能力优先；已知具体型号使用保守映射，未知型号保留自动配置。`reasoning: true` 不足以推导出 low、high 或 xhigh 等档位。

显示开关控制选择器候选项，保留已有会话的模型引用和解析能力。更改提供商配置不会删除会话、工作区或记忆。

## 免费版验证

免费版使用 `ling-3.0-flash-fin-free`。发布流程先确认官方目录中的精确模型 ID，再进行不含任何凭据的真实推理、工具调用和工具结果续接。打包要求成功证据不超过 24 小时，包内包含 `free-model-verification.json`；验证失败时不生成免费版。

## 验证入口

- `crates/llm/llm-deepseek/src/responses.rs`：Responses 转换。
- `crates/llm/llm-deepseek/src/anthropic.rs`：Messages 转换与流式协议测试。
- `crates/host/dsh-host/src/provider_auth.rs`：账号流程、续期及受限端点。
- `crates/host/dsh-host/src/model_capabilities.rs`：模型能力识别。
- `tools/verify_free_model_catalog.py`：实时免费模型验证。

协议测试验证请求与事件转换；每个账户实际能调用的模型以供应商授权和实时响应为准。
