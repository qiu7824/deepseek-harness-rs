# DSM 官方协议迁移矩阵

审计基线：Node 仓库 `D:/HermesTemp/deepseek-harness` 的 `origin/master`，以当前固定权威 commit 为准；Node 仓库只读。

## 结论摘要

Node 官方支持分为两条适配路径：

1. `llm-deepseek`：DeepSeek 官方专用适配器，主要使用 OpenAI-compatible Chat Completions，并包含针对 Responses 的图片回退/部分转换逻辑。
2. `llm-pi-ai`：基于 pi-ai provider catalog 的通用路由适配器。Node 代码明确声明并处理 OpenAI Completions、OpenAI Responses、Azure OpenAI Responses、OpenAI Codex Responses、Anthropic Messages 和 Bedrock Converse Stream 等 API 类型；具体可用 provider/model 由安装的 pi-ai catalog 决定。

Rust 当前只有 `crates/llm/llm-deepseek`，没有 `llm-pi-ai` 等价适配器。因此，Rust 已有 `responses.rs` 不等于已经支持通用 OpenAI Responses Provider。

## 协议矩阵

| Node 官方协议/API | Node 证据 | Rust 当前状态 | 请求/流能力 | 下一步 |
|---|---|---|---|---|
| OpenAI Chat Completions | `packages/llm/llm-deepseek/src/adapter.ts`、`packages/llm/llm-pi-ai/src/catalog.ts` | DeepSeek 专用适配器已实现；通用 catalog 路由未实现 | DeepSeek 请求、SSE、reasoning、tool、image、usage 已有实现和 fixture；通用 provider 未组合 | 保留专用适配器；新增通用路由时复用共享模型/凭据语义 |
| OpenAI Responses | `packages/llm/llm-pi-ai/src/catalog.ts` 的 `OpenAIResponsesCompat`；DeepSeek Files/Image fallback 测试涉及 `/responses` | 部分实现，不是通用入口 | `llm-deepseek/src/responses.rs` 已有请求转换、SSE translator、图片fallback；缺通用 provider catalog 入口和完整协议矩阵 | 先完成 wire RED，再实现显式 Responses 路由、tool/reasoning/image/usage/error 全链 |
| Azure OpenAI Responses | Node `OpenAIResponsesCompat` API catalog 类型 | 未实现 | 没有 Azure endpoint/auth/Responses 路由 | 通用 pi-ai 适配器阶段实现；没有真实配置前只做 fixture |
| OpenAI Codex Responses | Node `OpenAIResponsesCompat` API catalog 类型 | 未实现 | 没有 Codex OAuth/Responses 路由 | 必须保留认证边界，不得用测试凭据伪造；另列为可选 provider |
| Anthropic Messages | Node `AnthropicMessagesCompat`、`llm-pi-ai` adapter | 未实现 | 没有 Anthropic Messages 请求或事件投影 | 通用 adapter 阶段；验证thinking/tool/image/usage/error |
| Bedrock Converse Stream | Node `BedrockCompat`、`llm-pi-ai` adapter | 未实现 | 没有 AWS credential/Converse wire | 先做兼容边界设计；不得主动配置账户或凭据 |
| pi-ai catalog provider routes | Node `catalog.ts` 的 `catalogProviderIds/catalogModels/catalogProvider` | 未实现 | Rust 没有 pi-ai provider catalog snapshot/route resolver | 新增 Rust provider catalog 抽象或明确兼容范围；不注册只有schema没有执行能力的假provider |

## Responses 必须验证的 wire

- 请求路径：`/responses`；不能误发 `/chat/completions`。
- system/developer/input 消息转换和顺序保持。
- assistant reasoning/reasoning summary 的回放与输出投影。
- tool definition、tool call、tool result 的关联ID和结束事件。
- 图片输入：file reference、inline fallback、文件失效恢复。
- SSE：`response.output_text.delta`、reasoning、tool call、usage、`response.completed`及错误事件。
- usage：input/output/cached token 字段不混淆，不能伪造cache命中。
- abort、idle timeout、HTTP错误和body decode错误分类。

## Rust 证据

- `crates/llm/llm-deepseek/src/responses.rs`
- `crates/llm/llm-deepseek/src/lib.rs` 的 `request_responses_chunks`
- `crates/llm/llm-deepseek/tests/deepseek.rs` 的 Responses/image fallback fixture
- `crates/llm/llm-deepseek/src/transport.rs` 的 `Accept-Encoding: identity`

## 记账规则

- “已支持”：Rust适配器、测试fixture、Host/profile组合和真实生产入口全部有证据。
- “部分实现”：存在局部转换或fixture，但缺少正式路由、完整事件或生产入口。
- “库级未组合”：底层crate测试通过，但正式Host没有依赖/注册/配置入口。
- “未实现”：没有真实请求执行路径，不能用类型名或schema代替。
- 每次Rust代码修改会使旧Release、旧进程和旧浏览器证据失效，需重新验证。
