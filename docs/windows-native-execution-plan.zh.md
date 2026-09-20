# Windows 原生执行后端实施方案

## 目标与验收边界

在保留 `read-only`、`workspace-write` 与显式完整访问模式语义的前提下，支持 Windows 原生 PowerShell、cmd、Git、Node、Python、Rust/MSVC 的真实开发任务。前台命令、持久终端和后台任务使用同一个权限决策与执行后端。后端不可用时返回明确错误，不自动切换为完整访问。

文件写入隔离、敏感文件保护、网络控制、进程树回收和运行环境兼容性分别验收。独立账号不是虚拟机：不能把同一内核下的文件写入限制宣称为对所有宿主资源的完全隔离。

## Hermes 的可借鉴部分

Hermes 将 local、Docker、SSH 与云执行环境放在统一后端接口后面。local 使用宿主用户权限，不提供本机文件系统隔离；Docker 使用容器边界，并统一路由终端、文件和代码执行。会话容器生命周期、明确的工作目录映射、环境变量过滤、资源上限和显式后端选择适用于原生执行服务。

Windows 原生开发需要访问已有 MSVC、Windows SDK、WPS 与本地工程，采用独立 Windows 身份后端；容器或 SSH 作为独立执行世界保留扩展位置，不把 Windows 路径直接传入容器，也不将远端结果伪装成本地执行。

来源：

- https://hermes-agent.nousresearch.com/docs/user-guide/configuration
- https://github.com/NousResearch/hermes-agent/blob/main/tools/environments/local.py
- https://github.com/NousResearch/hermes-agent/blob/main/tools/environments/docker.py
- https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/security.md

## 架构

```text
工具 / 环境验证 / 终端 / 后台任务
                  │
      会话权限 + 执行环境快照
                  │
       SandboxProvider 后端选择
          ├─ AppContainer
          └─ Windows 独立账号
                  │
       dsh-windows-native 桥接器
                  │ 认证的 IPC
       dsh-command-runner
                  │ 受限令牌 + Job Object / ConPTY
           目标程序及其子进程
```

初始化使用单独的 `dsh-windows-sandbox-setup`，管理员权限只用于账号、ACL 和网络规则维护。业务命令不使用管理员身份。原生桥接器与主程序解耦，避免把所有 Windows 专用依赖带入其他平台。

## 实现原则

1. 固定上游实现提交并保留许可证，复用经过公开测试的独立身份、DPAPI、认证管道、受限令牌、Job Object 和 ConPTY 实现。产品账号、账号组、辅助程序、命名对象和网络规则使用 DSH 专属标识，禁止接管其他应用的沙箱账号。
2. 初始化与执行分离。初始化状态包含协议版本、辅助程序身份与根目录；凭据不得进入 argv、日志或模型上下文。未安装、辅助程序缺失、权限初始化失败、进程启动失败、命令非零退出和未知完成状态分别报告。
3. argv 保持数组直到 Windows 进程创建边界。stdout/stderr 分开转发，stdin 支持关闭，输出背压与尾部排空有上限；取消、超时、退出必须回收对应进程树。
4. 平台和工具链读取目录、项目写入目录、执行临时目录、包管理器缓存分别配置。拒绝整个用户主目录作为工作区。显式保护凭据目录、主程序数据与初始化秘密。
5. 工具环境使用白名单。保留 PATH、系统目录、编码与已确认的编译环境，不自动传递模型密钥、云凭据或代理认证。Rust 使用已解析工具链与受控 Cargo 缓存；项目指定的工具链必须保持一致，缺失时明确报错。
6. 新后端以显式选择启用，旧后端保留可回退能力。后端选择持久化在受保护的主机配置，模型工具参数不能把受限执行改为不受限执行。
7. 原生模式的读取能力按实际身份与 ACL 如实描述。禁止把“未主动授予读取”当成“系统必然拒绝读取”；公共可读目录、重解析点、硬链接与跨会话缓存必须纳入负向测试。
8. Host 控制接口在接受 HTTP/WebSocket 连接前验证本机 TCP 对端的 Windows 用户 SID，只接受与 Host 同一用户的连接。受限账号不能通过本机 API 获得宿主权限；未知身份拒绝访问。原生后端暂不支持未经认证的远程控制接口。
9. 使用两个只读账号槽位及每工作区四个可写账号槽位，禁止只读任务复用写入账号，禁止不同工作区复用写入账号。同一账号同时只运行一个进程树；辅助进程在恢复执行前设置仅允许宿主及 SYSTEM 管理的进程 DACL。无法确认旧进程已退出时隔离槽位，不冒险复用。
10. 工具链发现只读取配置及安装目录，不在宿主执行工作区或 PATH 中提供的探测程序。账号身份取自 OS 令牌 SID，状态及辅助程序使用规范化的 ProgramData 路径，避免中文环境变量错误编码与 MSIX AppData 重定向。

## 实施阶段

| 阶段 | 交付 | 通过条件 |
| --- | --- | --- |
| A | 原生桥接器、固定依赖、专属身份、初始化状态检查 | 独立编译；无凭据输出；未初始化时命令不派发 |
| B | 前台执行和环境隔离 | Node 管道子进程、Python subprocess、Rust/MSVC 实际构建通过；真实退出码保留 |
| C | SandboxProvider 集成、明确后端选择与错误映射 | 环境验证与真实工具使用同一后端；失败不越权重试 |
| D | 终端、后台、取消和恢复 | 交互输入、EOF、ConPTY、超时和完整进程树回收通过 |
| E | 权限与生命周期审查 | 只读禁止写入；工作区外写入失败；凭据拒绝；冷启动、并发与异常退出可恢复 |
| F | 安装及生产切换 | 无运行任务时备份替换；二进制哈希匹配；新安装实测通过；保留回退入口 |

每个阶段保留机器可读证据。尚未通过的能力不得标记为受支持，也不得通过增加等待时间、改用另一个 Shell 或关闭隔离使测试表面通过。

离线网络模式的 WFP/防火墙配置已完成初始化，但本机连接阻断尚未通过实测；运行时对该模式返回 `NETWORK_POLICY_UNAVAILABLE`，不派发命令。普通联网的文件写入隔离与离线网络隔离分别交付和验收。

普通用户宿主验收仍需真实的非管理员 Windows 登录环境。开发桌面的降权模拟进程在执行桥接器前出现 DLL 初始化失败；“最低权限”计划任务在该桌面仍获得管理员令牌，不能作为普通用户验收证据。该门禁通过前，原生后端保留显式实验开关，不自动成为全局默认后端。

## 实施验收记录

2026-09-20 的原型验证已覆盖真实 Host 环境接口、原生执行器与权限负向检查：

- Host 的 PowerShell、Node、Python、Cargo、rustc、Git 六项启动验证返回 `windows-native / ready`。
- Node 子进程 pipe/inherit、fork/IPC、worker、npm 脚本、Python 子进程及无依赖 Rust 项目的实际编译测试通过。
- 工作区写入通过；只读写入、工作区外写入、宿主配置读取被拒绝。
- 退出码 7 保留；超时、结束父进程和正常退出后的子进程树回收通过。
- ConPTY 交互输入通过；四个并发写入执行使用四个不同账号，第五个请求不派发并返回繁忙。
- 受限命令获取辅助进程写入句柄被拒绝；普通宿主用户 HTTP 请求成功，沙箱账号访问同一 Host 控制服务被拒绝。

上述结果不代替离线网络、真实非管理员登录及完整跨机器矩阵验收。普通联网原生后端可作为显式实验实现进行后续验收，AppContainer 保持保留及回退能力。

## 验收矩阵

- Node：版本、spawn/spawnSync、inherit/pipe、fork/IPC、worker、npm 脚本与含空格路径。
- Rust：实际工具链解析、cargo test --offline、全新构建目录、构建脚本和 MSVC 链接。
- Python：隔离导入、临时文件、捕获子进程输出与子进程树超时。
- Shell：PowerShell 5/7、cmd、Unicode、原生 stderr 与退出码 7、持久终端输入。
- 权限：只读模式、工作区内写入、外部哨兵、敏感文件、子进程继承、读写模式交替、链接越界。
- 生命周期：取消、超时、输入关闭、宿主结束、未完成 IPC、重复初始化、重启与错误恢复。
- 系统：普通用户运行、管理员初始化、中文用户名、跨磁盘、不同安装目录、辅助程序缺失或不匹配。

## 参考实现

- https://openai.com/index/building-codex-windows-sandbox/
- https://github.com/openai/codex/tree/bb5054fe47abe73ecbbd454751066a28c89f4bb9/codex-rs/windows-sandbox-rs
- https://github.com/libuv/libuv/pull/5181
- https://rust-lang.github.io/rustup/environment-variables.html
- https://doc.rust-lang.org/cargo/guide/cargo-home.html
