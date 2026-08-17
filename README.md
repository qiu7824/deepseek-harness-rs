# DeepSeek Harness — Rust 后端移植

这是 DeepSeek Harness Host/后端的独立 Rust 1:1 移植项目。源版本基线是 `@deepseek-ai/dsh-root 0.1.0-rc.5`，只读参考源码位于：

```text
D:\HermesTemp\deepseek-harness
```

本项目位于：

```text
D:\deepwork\deepseek-harness-rs
```

## 目标

- 用 Rust 重写 DSH Host 侧全部运行时：Cordis 组合/插件生命周期、会话/LLM/工具循环、持久化、文件与沙箱、子进程/终端、goal/skill/subagent/workflow、MCP/LSP/ACP、Host API、Web server 与 CLI。
- 保留现有浏览器前端及 wire/storage 协议，最终能够由 Rust Host 直接托管并运行现有 GUI。
- 对安全边界保持 fail-closed：沙箱、批准、凭证、路径边界与子进程执行不能静默降级。
- 维护 TypeScript 包 → Rust crate 的完整映射、测试与兼容性状态。

## 当前状态

> 这是跨多轮的大型工程，尚未完成全量运行替换。

- 已复制 Web 前端、配置和 examples，未复制 `node_modules`。
- 已完成 241 个 workspace 包的自动 LOC 清点；剔除纯浏览器包后，Host/后端及共享基础约 192 个包、161,745 行源码、206,707 行测试。
- 已完成六组包级盘点，见 `docs/porting/inventory/`。
- 已完成最底层 `@deepseek-ai/cordis` 生态，Rust crate 位于 `crates/vendor/`。
- M6 Host 外壳：`dsh-host` 可启动组合、webserver、frontend-static、directory-picker、plugin-inventory、apiproxy 契约与组合层（103 项测试）、CLI 骨架、profile-boot 均已落地。
- 最新一轮（第 125 轮）：dsh-e2b 命令面扩展（cwd/流式回调/abort/后台命令）+ 新建 `crates/e2b/subprocess-e2b` 适配器（environment/remote/output/process/index，6 项聚焦测试全绿），全量 1664 项通过。

完整阶段、风险和状态见 [`PORTING.md`](PORTING.md)。

## 项目结构

```text
apps/                       # 最终 Rust CLI / Host 可执行程序
crates/vendor/              # cordis / schemastery / cosmokit 等底座
crates/core/                # agent/session/tools/system-prompt 等核心域
crates/exec/                # fs/sandbox/subprocess/terminal/jobs 等执行域
crates/feature/             # goal/skill/subagent/workflow/mcp/lsp 等能力域
crates/host/                # apiproxy/webserver/frontend-static 等 Host 外壳
crates/app/                 # boot/profile/bundle/composition 引导
web/                        # 从 apps/web 复制的现有浏览器前端（含 dist）
config/                     # 从 apps/cli/config 复制的 preset/config 资源
examples/                   # 上游 examples 的独立副本
docs/porting/inventory/     # TypeScript 包级盘点
```

## 构建

当前机器的 Cargo 不在默认 PATH，可用完整路径：

```powershell
C:\Users\Administrator\.cargo\bin\cargo.exe check --workspace
C:\Users\Administrator\.cargo\bin\cargo.exe test --workspace
```

或先加入 PATH：

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo check --workspace
```

工具链由 `rust-toolchain.toml` 固定为 Rust 1.97.1。

## 兼容性原则

1. **wire 协议不改名**：Host API 的 52 个一元 RPC、SSE、下载、respond 路由保持现有路径与信封结构。
2. **存储格式不随意迁移**：Session JSONL/zstd/SQLite、storage、spill、attachments、settings 等格式以现有实现为权威。
3. **Cordis 行为优先**：依赖 epoch、isolate/intercept、waterfall、effect 逆序清理、service 可见性与插件更新行为必须有对应测试。
4. **安全 fail-closed**：沙箱、批准、凭证、目录边界和子进程执行不可因未实现而透传。
5. **前端保持兼容**：Rust Host 应直接托管现有 `web/dist`，不要求重写前端。

## 许可证

沿用上游 MIT 许可证，见 `LICENSE`；第三方声明见 `THIRD_PARTY_NOTICES.md`。
