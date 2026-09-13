# DeepSeek Harness Desktop

Windows 原生阅读客户端，使用 ZSUI Surface 和 WebUI 原始 SVG 资源。

支持会话分组、搜索、切换、历史阅读、主题切换、消息复制、思考与工具详情展开，以及 Markdown 标题、列表、引用、表格、链接、代码和工作区图片。窗口和内容布局随尺寸与 DPI 调整。当前客户端通过本机 Host API 读取真实会话，缺少 Host 时显示连接错误。

当前版本提供原生会话阅读。新会话、模型、设置、发送和代码图谱入口打开已配置的网页版。图片仅自动读取当前会话工作区内的本地文件；远程图片显示链接。

## 启动

`dsh-desktop.exe` 默认连接 `http://127.0.0.1:58080`；如果现有服务不可用，会自动启动同目录随附的 Host，并在退出时回收自己启动的进程树。已运行的外部 Host 不受影响。

可用 `--url http://127.0.0.1:端口` 指定其他本机 Host，`--dark` 选择深色，`--width 1280 --height 800` 指定初始窗口大小。

## 构建与验证

`cargo build -p dsh-desktop --profile desktop`

`cargo test -p dsh-desktop`

`--fixture path.json --smoke output-directory` 从隔离会话快照生成原生窗口截图与内存报告。默认运行不含示例数据。资源编译进程序，不依赖外部字体包或 WebView；正文使用系统字体回退。
