# Computer Use 能力与兼容范围

应用版本：0.1.3-alpha.22。核对日期：2026-09-16。

## 能力矩阵

| 能力 | Rust 实现与验收边界 |
| --- | --- |
| Windows 截图、窗口绑定、鼠标和键盘动作 | 已实现原生控制器；目标、控制权、取消与输入释放由宿主校验 |
| Windows 控件树与语义动作 | 已接入 UI Automation；应用自身是否暴露控件模式决定可用操作 |
| 浏览器页面与标签页操作 | 已实现独立 CDP 适配器；回归覆盖本机隔离页面，不代表所有第三方网站兼容 |
| 持久化 JavaScript | 独立受限 Node 内核，动作经宿主检查；运行环境必须满足实际能力检测 |
| UU 远端控制 | 已有绑定设备、连接复用、画面、人工接管及终端路径；需要实际设备在线并完成登录解锁 |
| 应用级长期授权与撤销列表 | 尚未完成与 Codex 同等的管理闭环；工具审批和目标绑定不能替代应用级权限 |
| 剪贴板细分授权 | 尚未完成独立读写授权的完整管理界面 |
| OpenAI 原生 computer 工具协议 | 尚未形成 computer 调用、动作结果与截图返回的完整适配闭环；普通函数工具不能据此称为协议兼容 |
| macOS/Linux 原生桌面 | 条件能力和构建覆盖，不等同 Windows 能力或跨平台实机验收 |
| Codex 完整产品兼容 | 不成立；浏览器插件、应用授权、Office 专用集成和锁屏能力是独立范围 |

## 官方对照

Codex 的 Computer Use 提供应用访问审批、可撤销的长期允许列表，并将这些权限与文件和命令沙箱分开。Windows 使用前台桌面；macOS 的系统权限及锁屏使用另有条件。参见 [Computer Use 官方说明](https://learn.chatgpt.com/docs/computer-use)。

OpenAI API 的 computer 工具由模型返回动作，应用执行后提供新的画面继续交互；该协议接入与桌面控制器本身是不同层次。参见 [Computer use API](https://developers.openai.com/api/docs/guides/tools-computer-use)。

上游 Harness 的支持范围按固定源码和实际提供方分别核对；拥有同名入口、动作 schema 或截图结果，均不能证明 Playwright MCP、Chrome DevTools MCP、Stagehand 与原生桌面提供方全部完成迁移。详见 [上游能力评估](upstream-v0.1.6-alpha.1-evaluation.zh.md)。

## 验证入口

- `tools/e2e_computer_use.py`：正式 Host、隔离网页、浏览器动作、画面和会话边界。
- `crates/computer-use/tool-computer-use-command`：工具参数、目标控制、取消、JavaScript 内核及执行器回归。
- `crates/computer-use/desktop-controller`：Windows 原生窗口与控件适配。
- `tools/e2e_uu_desktop.py`：实际绑定设备的独立验收；设备缺失时不计为通过。

构建通过证明代码可编译，模拟测试证明受测契约；供应商账号、第三方应用、远端设备及操作系统真机能力仍需对应环境验证。
