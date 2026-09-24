# DeepSeek Harness Desktop

Flutter Windows 客户端，通过本机 HTTP RPC 与 WebSocket 连接 Rust Host。

## 功能

- 连接已有服务，或启动完整安装目录中的 Host。
- 任务列表、搜索、创建、工作目录选择及模型切换。
- 中文 Markdown 对话、流式回复、思考过程、工具调用和停止执行。
- 有界历史分页、断线重连、待处理审批恢复及多问题批量回答。
- 按任务保存输入草稿、深浅主题和自适应布局。

连接地址默认是 `http://127.0.0.1:58080`。客户端只接受回环地址。关闭窗口保留后台服务及其任务；“停止任务”仅取消所选任务。

任务、凭据和模型设置由 Host 管理。桌面偏好与草稿保存于 `%LOCALAPPDATA%\DeepSeek Harness Desktop\preferences.json`。

## 构建

需要 Flutter 3.47.5 / Dart 3.13.4，以及 Visual Studio 的 Windows C++ 桌面构建组件。Flutter SDK 路径应使用 ASCII 字符，以避免 Windows 着色器编译器的路径编码问题。

```powershell
flutter pub get
flutter analyze
flutter test
flutter build windows --release
```

发布时保留整个 `build/windows/x64/runner/Release` 目录，其中包含可执行文件、Flutter 运行库和资源。客户端使用已有的完整 Harness 安装，也可将完整 Host 安装目录放在客户端旁的 `host` 子目录中。

## 验证

SDK 测试位于 `../../packages/dsh_client_dart/test`。设置 `DSH_TEST_HOST` 和 `DSH_TEST_CWD` 后，`live_host_test.dart` 可对隔离 Host 运行真实协议测试；该 Host 必须使用独立数据根与确定性模型测试服务。

Windows 界面集成测试：

```powershell
flutter test integration_test/desktop_flow_test.dart -d windows
```

## 范围

当前版本提供文本任务闭环。文件上传、图片查看、终端、差异审阅、Web 插件、远程服务和自动更新尚未接入。模型账号与高级设置继续由已有 Web 界面管理。
