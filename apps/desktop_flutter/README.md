# DeepSeek Harness Desktop

Flutter 桌面客户端，通过本机 HTTP RPC 与 WebSocket 连接 Rust Host；Windows、macOS 与 Linux 的实际验收状态见[平台矩阵](../../docs/desktop-platforms.zh.md)。

## 功能

- 连接已有服务，或启动完整安装目录中的 Host。
- 会话列表、搜索、创建、工作目录选择及模型切换。
- 中文 Markdown 对话、流式回复、思考过程、工具调用和停止执行。
- 有界历史分页、断线重连、待处理审批恢复及多问题批量回答。
- 按会话保存输入草稿、深浅主题和自适应布局。
- 未发送的新会话草稿按 Host 与工作区隔离保存；关闭窗口前完成草稿写入。
- 分组命令搜索、F6 焦点分区、可配置会话／工具标签切换与停止快捷键。
- 统一语义图标与中文文案，14—18 号对话正文偏好、完整系统文字缩放及代码换行切换。
- 图片与文件附件、Windows 剪贴板图片和复制文件粘贴、拖放、文件预览与保存。
- 代码图谱、上下文、计划、子智能体、后台作业、终端及差异审阅工作台。
- 工作台开始页提供工作区文件、新建终端和 Computer Use 浏览器入口；资源操作在显式选择后执行。
- 侧栏顶部同排提供定时、知识、插件三个入口；左下角设置直达，登录后显示“账号”入口，身份与连接详情通过提示和菜单查看；插件在独立主页面管理。
- 控件提供中文悬停提示；工作区展开与新会话目标选择独立，关闭搜索清除筛选条件。
- 输入框以紧凑的单一入口显示型号与思考等级，菜单内分层选择模型或等级；目录缓存即时展示，连续切换保留最后意图，选择后立即收起并返回输入框。新会话默认值有序后台保存，上下文与压缩设置通过独立上下文入口查看。
- 会话统计、用量和上下文通过三个独立浮层查看；会话日志、反馈和协作入口收于右上角更多菜单。
- 工作台支持通过 `+` 打开工具标签，独立切换和关闭；切换时保留文件阅读位置、终端内容和子任务草稿，隐藏标签暂停读取。
- 隐藏预览按时限或内存压力释放并恢复位置；文件错误提供作用域隔离的重试和可展开技术详情。
- 模型账号、模型显示开关、技能与 MCP 管理、快捷键配置。

连接地址默认是 `http://127.0.0.1:58080`。客户端只接受回环地址。关闭窗口保留后台服务及其任务；“停止当前执行”仅取消当前会话的执行。

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

Windows 图片粘贴支持 `Ctrl+V`、`Shift+Insert` 和输入框右键菜单，纯文本粘贴保留选择范围替换行为。macOS/Linux 已有原生剪贴板接入，其构建和实机验证状态见[平台矩阵](../../docs/desktop-platforms.zh.md)。附件读取与上传受数量和总大小限制，切换任务会取消迟到导入。

Web 插件不能直接作为 Flutter 原生界面运行。全局提醒与定时执行任务提供独立管理入口；远程服务连接、完整首次引导、自动更新和跨平台安装验收状态见[更新计划](../../docs/plans/更新计划.md)和[平台矩阵](../../docs/desktop-platforms.zh.md)。设计规范和图标验证见[桌面设计系统](../../docs/desktop-fidelity/design-system-2026-09-29.md)。
