# DeepSeek Harness Rust 视频与截图素材

本目录只包含无声视频片段和已验收截图。所有 Harness 画面均采自正式 Rust Host：`dsh.exe web --port 58080`，采用独立 Chrome 页面级 CDP 截取；没有录入桌面、任务管理器、终端或其他窗口。

## 01-screenshots

| 文件 | 内容 | 可用于表达 | 不应据此宣称 |
|---|---|---|---|
| `01-rust-home.png` | 正式 Rust Harness 首页 | 原界面由 Rust Host 托管 | 性能或内存数值 |
| `02-settings.png` | 设置页 | 模型、插件、目录、预设、安全等配置入口 | 每项能力已完全实现 |
| `03-plugin-list.png` | 4 个已启用插件 | 独立 Web 插件被发现并启用 | 插件没有任何权限或联网行为 |
| `04-workspace-menu.png` | 工作区选择菜单 | 工作区选择和会话入口 | 工作区内容或权限范围 |
| `05-workspace-selected.png` | 已选工作区与模型 | 会话工作区、模型选择、输入框展开插件 | 模型已完成某个任务 |
| `06-task-entered.png` | 专用演示任务输入 | 真实任务可提交 | 该任务已成功完成 |
| `07-task-started.png` | 任务开始 | Rust Harness 已进入请求流程 | 已收到模型结果 |
| `08-tool-running.png` | `Glob · *` 工具执行 | 真实工具状态与停止控制 | 工具最终成功或返回了什么 |
| `09-runtime-metrics-strip.png` | 状态栏裁剪 | 首 Token、工具耗时、缓存命中等当次真实显示值 | 跨版本性能结论或基准结果 |
| `10-github-repo.png` | GitHub 仓库首页 | 仓库公开、项目名、Rust 占比 | 当前为稳定正式版 |
| `11-github-quickstart.png` | README Quick start | `dsh.exe web` 与默认 `58080` 地址 | 所有平台均已完整验收 |
| `12-github-releases.png` | Release 列表 | 版本与 `Pre-release` 状态 | 正式稳定发布 |
| `13-release-detail.png` | rc.8.3 详情 | 打包加固和已知限制 | ACP/Python SDK 已无已知限制 |

## 02-silent-clips

与截图同名的 MP4 均为无声 H.264 素材，1920×1080、30 fps，无字幕、无音乐、无旁白。

`08-tool-progression.mp4` 使用真实连续状态帧，先显示等待模型，再显示 `Glob · *` 工具执行。它只证明前端接收到并展示了工具执行状态，不证明工具最终成功。

## 03-script

- `video-script.md`：按现有真实素材重写的 65 秒分镜与口播。
- `voiceover.txt`：可直接用于声音克隆或真人录音的连续口播。
- `memory-sample.json`：Node 与 Rust 当前运行状态的 10 次内存采样。

内存采样结果：

- Node Host + esbuild：Working Set 218.3 MB；Private 282.3 MB。
- Rust Host：Working Set 45.3 MB；Private 91.3 MB。

这是同一台机器、同一时段的当前运行状态。Node 与 Rust 未加载同一批会话，也未执行同一个任务，因此不能把这些数字表述为严格同负载基准，不能宣称降低倍数或百分比。

## 公开使用边界

- 仓库地址：`https://github.com/qiu7824/deepseek-harness-rs`
- 当前 Release 必须标注为 `Pre-release`。
- 不把当前内存采样称为同会话、同任务基准。
- 不把工作区写入模式描述成硬只读沙箱。
- 不把 `ACP real prompt/cancel` 和 Python SDK turn regression 描述为已解决。
- 本轮没有实证会话恢复、模型持久化、文件跳转、图片输入、Skill、Subagent、Workflow、MCP、LSP 或 ACP 完整可用性，脚本不再宣称这些能力。
