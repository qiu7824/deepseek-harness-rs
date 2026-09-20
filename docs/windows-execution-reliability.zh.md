# Windows 通用执行环境可靠性修复方案

## 范围与状态

范围覆盖 PowerShell 5/7、cmd、Git、Node、Python、原生编译工具、前台命令、持久终端、后台作业以及环境验证。语言工具链只是消费者，权限、进程、管道、超时与结果协议由统一执行层负责。

截至 2026-09-20，已完成跨实现复现与部分执行层修复；通用 Windows 沙箱后端尚未完成验收。当前生产安装仍为提交 `3804870a02`，不能将实验执行器的结果视为已部署功能。

## 已确认的问题

| 层次 | 故障机制 | 影响 |
| --- | --- | --- |
| 环境验证 | 探测固定五秒包含执行器清理；外层工具十五秒又短于底层准备预算 | 可运行的程序被误判超时，随后反复换命令 |
| 终端装配 | 安装终端后才注册执行配置服务，提前保存的空服务引用不会更新 | 已选 PowerShell，持久终端仍启动 cmd |
| 失败记忆 | 仅按连续、完全相同的工具参数计数 | 查询工具说明、改写路径或切换 PTY 后，环境错误重复发生 |
| PowerShell 5 | AppContainer 无法正常读取用户策略状态 | 脚本出现 AuthorizationManager/UnauthorizedAccess；不等同于脚本语法错误 |
| 开发环境 | 普通启动环境未必含编译器与 Windows SDK 搜索路径 | 不只 Cargo，Node 原生扩展及其他 C/C++ 消费者也受影响 |
| AppContainer | 读取限制与普通桌面开发工具的运行依赖不匹配 | 用户配置、工具链、共享运行库、祖先目录访问失败 |
| 子进程 IPC | 受限进程创建的命名管道未向限制身份授予所需写权限 | Node 自身启动成功，但捕获子进程输出时 EPERM 或挂起 |
| Git 所有权 | 仓库所有者与运行用户不同 | dubious ownership 是独立安全检查，不能与配置文件拒绝访问混为一谈 |

`Test-Path` 的拒绝访问不能证明程序未安装；应用 stderr 中的 access denied 也不能单独证明沙箱策略作出了拒绝决定。诊断必须同时保留执行阶段、身份、退出状态及策略信息。

## 实测证据

测试在独立临时工作区进行；未修改全局 Git 配置、全局 PowerShell 执行策略或现有业务项目。以下为单机样本，不代表所有 Windows 安装均具有相同结果。

| 实现与配置 | 场景 | 结果 |
| --- | --- | --- |
| 官方 Node Harness `0.1.6-alpha.2` 的 AclSandbox | cmd、PowerShell 脚本、Cargo/rustc 版本 | 成功 |
| 同一 Node 后端 | Cargo 调用子编译器 | 拒绝访问 |
| 同一 Node 后端 | Node `spawnSync(process.execPath, ['--version'])`，管道输出 | `EPERM`，测试退出码 11 |
| Harness AppContainer 实验 | PowerShell 5/7 脚本、Python 子进程 | 在对应运行依赖与既有策略明确后成功 |
| 同一 AppContainer 实验 | 完整 Rust 测试 | 补齐实际共享运行库读取权限后成功，不是通用修复 |
| 同一 AppContainer 实验 | Node 子进程管道 | 仍失败或挂起 |
| 同用户 WRITE_RESTRICTED 实验 | Git 精确工作区信任、Python、PowerShell、Cargo | 样本成功 |
| 同一受限令牌实验 | Node 子进程 inherit/ignore 输出 | 成功 |
| 同一受限令牌实验 | Node 子进程 pipe 输出 | `EPERM`；仅设置 TokenDefaultDacl 不足以解决 |
| 同一受限令牌实验 | 工作区外写入、非零退出 | 外写被拒绝；退出码 7 保留 |
| Codex CLI `0.154.0`，独立权限配置 | Node 管道子进程、全新 Cargo 构建测试 | 成功 |
| 同一 Codex 配置 | 工作区外写入 | `EPERM` |
| 同一 Codex 配置 | 用户 AppData 下 Python 3.8 | SpawnChild 拒绝访问，说明仍需按运行依赖配置读取范围 |

命名管道最小复现还验证了：读取端能够打开，写入端返回 Windows 错误 5；实际管道 DACL 与修改后的令牌默认 DACL 不同。因此，不能把增大超时或更换 Shell 宣称为 IPC 修复。

## 可独立交付的执行层修改

1. 环境验证拆分准备、启动、执行、清理预算。支持就绪协议的执行器拥有十五秒命令期限，外层等待允许清理完成，并返回实际超时阶段。
2. 泛型终端在启动时解析执行配置；明确指定的 cmd/bash 后端不被替换。前台与 PTY 使用同一环境发现逻辑。
3. 对系统 Windows PowerShell 查询既有有效策略，只在对应子进程环境中传递；不写注册表、不生成新的 Bypass 策略、不执行用户 Profile。
4. 使用 `find-msvc-tools` 安装发现库提供的 PATH/LIB/LIBPATH/INCLUDE，供原生构建消费者共享；保留显式执行环境覆盖权。
5. 环境失败计数独立于普通重复调用计数，跨参数改写与中间查询保留；只有确认的执行成功、验证 ready 或新用户输入才清除。终端 unknown 不算成功。
6. 保留 stderr/stdout/终端视口中的有限诊断证据；提醒属于建议，不替代安全决策，也不自动绕过沙箱。

## 通用后端设计

### 安全模型

采用独立低权限 Windows 本地账号作为沙箱身份，而不是把真实登录用户 SID 加入写限制身份。后者会重新允许该用户已有的大量写权限，破坏工作区边界。

管理员权限仅用于一次性安装或经过授权的权限维护，业务命令始终以低权限身份执行。账号凭据必须通过系统保护存储，不进入模型上下文、命令行或日志。安装、修复、卸载均需有所有权标记和可恢复记录，禁止操作其他软件的沙箱账号。

读路径与写路径分离：读取所需开发环境，工作区与每次执行独占的临时目录可写；凭据目录、未授权项目与系统配置不自动开放。对 Everyone 写权限、重解析点、硬链接及共享对象边界必须有明确能力声明和负向测试。

不允许“沙箱启动失败 → 自动无沙箱重试”。无法建立边界时，返回 setup-required 或 setup-failed，命令不派发。

### 运行环境

- 区分主机发现与沙箱内验证，缓存以解释器身份、执行配置版本和策略指纹失效。
- PATH、工作目录、编码、临时目录、进程树与输出管道由公共执行服务统一管理。
- Git 普通配置与凭据代理分离；所有权信任只允许精确工作区及明确授权，不写全局 `safe.directory=*`。
- 包管理器缓存不得通过修改真实用户的 rustup/npm/pip 配置解锁；使用授权缓存或执行私有目录。
- PowerShell 的退出码、原生命令 stderr、终止异常分开处理，不能把任意 stderr 当作非零退出。
- Node 验证必须覆盖子进程输出捕获、IPC/fork、worker 与本地包脚本；Python 必须覆盖导入、临时文件与 subprocess。

### 生命周期与交互

- Job Object 管理完整进程树；取消、超时、关闭 PTY、关闭会话和主机退出均回收所属进程。
- 后台任务提供可查询的结束状态及真实退出码，未观察到完成时返回 unknown，不能伪造成功。
- 子代理使用规范会话标识、完整对话路由与父子关系；主任务和子任务的取消、导航不复用简化窗口的冲突事件。
- RPC 空响应、非 JSON、断流与业务错误分别报告；不能只向界面抛出 Unexpected end of JSON input。
- 长对话验证会话卸载、输出缓冲上限、订阅清理、后台资源租约与内存回落；执行环境修复不等于内存泄漏已消除。

## 验收门槛

1. 普通用户、管理员启动、中文用户名、空格路径、不同磁盘、只读目录与安装路径变化均有覆盖。
2. 每个已发现工具先验证版本，再验证真实工作：Git 状态与配置、Node 包脚本及管道子进程、Python 导入及子进程、原生编译链接、PowerShell 脚本。
3. 工作区内读写通过，外部未授权写入失败，只读模式写入失败；临时目录不能横向访问其他执行实例。
4. 前台、PTY、后台三条路径执行一致，并验证取消、退出码 7、超时、子进程树回收与输出截断。
5. 环境缺失、策略拒绝、权限准备失败、程序非零退出、传输错误和未知完成状态可区分，重复失败不会无限换包装器重试。
6. 使用真实模型完成跨语言任务后，再测试子代理导航和长对话资源释放。
7. 所有已运行任务结束后才备份替换安装；核对二进制版本与哈希，失败恢复原安装。

独立账号的安装与系统权限维护需要单独授权。在此之前，不发布“通用沙箱已修复”的结论，也不替换生产安装为未验收的实验后端。

执行配置、Shell、终端、失败提醒及沙箱库的定向单元回归共 39 项通过；这些测试覆盖公共执行逻辑，不替代独立账号后端及真实模型对话的完整验收。

## 参考实现

- [Codex Windows 沙箱说明](https://learn.chatgpt.com/docs/windows/windows-sandbox)：管理员安装与受限运行的产品边界。
- [Codex token.rs](https://github.com/openai/codex/blob/main/codex-rs/windows-sandbox-rs/src/token.rs)：当前源码区分普通能力令牌与“独立沙箱账号用户 SID”令牌；后者不能套用真实登录用户。
- [Codex setup_orchestrator.rs](https://github.com/openai/codex/blob/main/codex-rs/windows-sandbox-rs/src/setup_orchestrator.rs)：读写授权与安装准备协调。
- [Hermes local.py](https://github.com/NousResearch/hermes-agent/blob/main/tools/environments/local.py)：本地主机执行、环境与 Shell 管理可借鉴；默认 local 路径不是等价的文件隔离后端。Hermes 仅作源码审查，未作运行实测。
- [libuv process-stdio.c](https://github.com/libuv/libuv/blob/v1.x/src/win/process-stdio.c)、[pipe.c](https://github.com/libuv/libuv/blob/v1.x/src/win/pipe.c)：Windows 子进程输出依赖命名管道，程序版本查询不足以覆盖此路径。
