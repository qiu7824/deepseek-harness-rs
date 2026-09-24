# dsh_client

DeepSeek Harness 本机 Dart SDK，采用 `dart:io`，不依赖 Flutter。

`DshClient` 提供 Host、会话、历史、模型、消息及交互响应接口。`EventChannel` 管理 WebSocket 生命周期与重连；审批和提问保留 Host 分配的 `rpcId`。写操作不会自动重试，传输结果不确定时通过 `DshException.outcomeUnknown` 明确返回。

`ConversationWindow` 同时限制事件数量和 UTF-8 字节数。历史读取使用 Host 的原始序列水位，避免合并后的历史片段与实时事件重复。阅读旧历史时仅标记有新消息。`projectTranscript` 隐藏非用户编写的上下文注入，并以最终助手消息替换同一步的流式片段。

```powershell
dart pub get
dart analyze
dart test
```

`live_host_test.dart` 仅在设置隔离测试端点 `DSH_TEST_HOST` 和工作目录 `DSH_TEST_CWD` 时执行，不访问默认用户实例。
