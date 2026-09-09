# DeepSeek Harness Rust 对上游 v0.1.5-alpha.2 的评估

评估对象：`dsh-v0.1.5-alpha.2`，发布提交 `b2e3b2a0125854567a4a5fcba75782e42fe84901`。

依据：[官方发布说明](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.1.5-alpha.2)及[alpha.1 → alpha.2 源码差异](https://github.com/deepseek-ai/deepseek-harness/compare/dsh-v0.1.5-alpha.1...dsh-v0.1.5-alpha.2)。

## 结论

当前 Rust 工作树已经出现会话 V3、`system/message`、V0→V3 迁移、Sidebar、产出文件、MCP 游标防重复、Provider Base URL 校验和反馈投递等实现。此次补齐了同供应商多账号切换、批注提交、UU 4.39.2.1561 签名偏移、正式环境部署和界面回归，alpha.2 相关协议与产品面已进入**可运行、可测试、可发布验证**阶段。

仍不建议把所有上游实验能力宣称为完全同构。主要保留差异集中在上游 Agent Teams、部分实验插件全局面板和 `uu_terminal` 对被控端终端环境的前置要求；UU 桌面画面控制已通过正式客户端实机闭环。

## 本轮闭环结果

- 同一供应商的多个登录会话按 `accountScope` 保存，`/provider-auth/providers` 返回账号目录，`/provider-auth/switch` 切换活动会话并刷新模型目录与账号用量作用域；侧栏统计入口支持快速切换。
- 浏览器/UU 画面批注保存线条和文字，提交时携带归一化坐标与当前截图；Host 通过附件存储形成 `UserMessage` 的文字+图片内容，提交前执行会话归属、大小和图片格式校验。
- UU 4.39.2.1561 使用签名数据偏移 `0x3b08118`，旧版 4.38.3.9325 保持 `0x3b06e78`；正式 `start → capture` 返回 `phase=ready`、视口 `1066×600`。`uu_terminal` 仍单独受远端终端环境检查影响，不与桌面控制链混淆。
- 账号徽标和设置按钮共享左对齐基线；等待模型状态不再被图标模式隐藏；完成的思考行默认折叠且可再次展开。

本轮验证包括 `cargo test --workspace --lib`（全部通过，包含 Host 161 项、Computer Use 49 项、LLM 54 项等）、发布契约 62 项、响应渲染 DOM、模型管理 React DOM，以及正式环境 Provider 账号切换接口和 UU 桌面控制实测。

## 逐项结论

| alpha.2 项目 | 当前 Rust 判断 | 后续动作 |
|---|---|---|
| Markdown/代码/HTML/PDF/图片右侧预览 | 已有 Rust Web 工作台、文件预览和 Host 文件读取基础，但未确认已按上游 `document-preview` 的类型、分页、PDF 和 HTML 隔离协议装配 | **P1**：补齐统一 document preview provider，验证大小、MIME、权限、取消、HTML 隔离和发布包资源 |
| 模型显式交付文件 | 已有 artifacts、产出文件和默认应用/文件管理器打开能力；需要核对事件是否由模型显式声明、文件卡片是否进入主会话和 Sidebar | **P1**：统一 `deliverable` 事件、预览、默认应用打开、文件管理器定位和过期文件处理 |
| `/feedback` 明细反馈 | Rust `/feedback` 仍主要记录文本；消息反馈已有 note 和独立投递，但没有完全对应上游命令的分类/明细表单 | **P1**：补命令级明细、校验、确认状态和离线重试；普通对话不能触发投递 |
| pi-ai 配置失效后的模型设置 | Rust 有模型目录、配置快照和诊断路径，需验证目录刷新后错误模型仍可见并可修复 | **P1 回归**：不能因单个失效配置让整个模型设置入口消失 |
| 自定义 Provider Base URL | Rust 已有 `Url::parse`、路径规范化、HTTPS/loopback 限制和测试 | **已有，补 E2E**：覆盖缺少 `/v1`、尾斜杠、错误 scheme、凭据和发现地址 |
| Windows 文件夹选择器前台显示 | Rust 使用原生/PowerShell 选择器并有 Windows 专门实现 | **P1 回归**：验证 Web 窗口在后台时选择器仍置前且取消可恢复 |
| Composer 空白占位提示 | 当前输入层已有 `trim()` 空状态判断 | **P2 回归**：输入空格、删除、切换会话、仅附件发送 |
| 子代理工具筛选后的指导 | Rust 有 `ToolRestriction` 和子代理工具裁剪；需确认 system prompt 中的工具指导也同步裁剪 | **P1**：工具 schema、工具说明和 persona 指导必须使用同一可见工具集合 |
| MCP 重复分页游标 | `remote_http.rs` 已用 `seen` 集合和上限拒绝重复/不终止分页 | **已有**：补启动失败后保留上一组工具的回归 |
| Session V3 | 当前 Rust 已为 `SESSION_FORMAT_VERSION = 3`，有 `LEGACY_SESSION_FORMAT_VERSION`、`system/message` 和迁移模块 | **P0 发布门禁**：验证迁移原子性、旧文件保留、V3 恢复/fork、未知事件和旧二进制拒绝 |
| Web 插件面板 API | 当前布局已有 Sidebar/Workbench，但需核对 `sidebar.panellist`、全局 `main` 面板及旧 `conversation` slot 迁移 | **P1**：统一 Rust 插件 manifest、slot 注册和静态 bundle |
| minimal 默认工具 | `config/agent-presets/minimal` 仍包含 `dsh-tool-str-replace-editor`，与上游 shell-only 默认不一致 | **P1 必做**：默认仅持久 shell，旧编辑器改为显式启用；standard/code 保持文件工具 |
| Agent Teams | 上游只提供可显式安装的实验包 | **不直接移植**：Rust 保持现有子代理模型，单独评估团队编排协议 |

## 当前最高风险

1. **V3 虽已实现，但发布链可能仍不完整。** 必须确认 JSONL/Zstd、迁移临时文件、恢复准备、fork seed、导出、反馈和子代理历史全部使用同一 V3 语义。
2. **Web 源码、dist、manifest 和随附插件可能不同步。** 文档预览和显式交付涉及多个插件，不能只更新 `web/src` 或只替换 `dist`。
3. **minimal 工具清单仍有明确行为差异。** 这是可被用户直接观察的默认契约，应在下一候选版前修复或明确声明差异。
4. **Rust 的 `/feedback` 与消息反馈是两条路径。** 需要明确命令反馈、消息评价、远端投递和本地持久化的边界，避免重复上报或显示“已提交”但实际没有接收端。

## 推荐执行顺序

1. 先做 V3 迁移与跨版本门禁：V0/V2/V3 fixture、恢复、fork、导出、未知事件和断电恢复。
2. 修复 minimal 默认工具，固定所有 profile 的工具快照。
3. 接通 document preview 和 explicit deliverables，完成 Windows/Linux/macOS 的文件权限、HTML 隔离和默认应用打开测试。
4. 补 `/feedback` 明细反馈和投递状态。
5. 完成 Provider 设置、MCP、文件夹选择器和空白 Composer 回归。
6. 同步源插件、发布 dist、manifest、哈希和 Release 包，最后再做完整候选包验收。

在上述门禁通过前，建议 Rust 对外表述为“已吸收 alpha.2 的部分协议和工作台能力，保持 Rust 自有文件、会话和插件兼容边界”，不要标记为完全兼容上游 `v0.1.5-alpha.2`。
